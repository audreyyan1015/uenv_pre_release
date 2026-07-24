#!/usr/bin/env python3
"""Run isolated selfcheck/reference/stability/capacity/burst/fault phases."""

from __future__ import annotations

import argparse
import asyncio
import csv
import hashlib
import json
import math
import os
import random
import signal
import shutil
import sqlite3
import subprocess
import sys
import time
import uuid
from pathlib import Path
from typing import Any, Iterator
from urllib.parse import parse_qsl, urlencode, urlsplit, urlunsplit

import grpc

import stability_test_common as stability
import stress_test_common as stress


HERE = Path(__file__).resolve().parent
DEFAULT_CONFIG = HERE / "stability_suite.json"


def bind_replay_url(replay_url: str, *, episode_id: str, task: str) -> str:
    """Bind ordinary Code/Math model calls to one replay Episode."""
    parsed = urlsplit(replay_url)
    query = dict(parse_qsl(parsed.query, keep_blank_values=True))
    query.update({"uenv_episode_id": episode_id, "uenv_dataset": task})
    return urlunsplit((parsed.scheme, parsed.netloc, parsed.path, urlencode(query), parsed.fragment))


def arrival_segments(
    config: dict[str, Any], phase: str, duration: float
) -> list[dict[str, Any]]:
    if phase in {"reference", "stability"} and config["load"].get("formal_arrival_segments"):
        configured = config["load"]["formal_arrival_segments"]
    else:
        configured = [{
            "mode": "batch" if phase == "burst" else config["load"]["arrival_mode"],
            "fraction": 1.0,
            "batch_size": config["load"]["batch_size"],
        }]
    result = []
    elapsed = 0.0
    for index, segment in enumerate(configured):
        segment_duration = (
            duration - elapsed
            if index == len(configured) - 1
            else duration * float(segment["fraction"])
        )
        result.append({
            "mode": str(segment["mode"]),
            "batch_size": int(segment.get("batch_size", 1)),
            "start_seconds": elapsed,
            "duration_seconds": segment_duration,
        })
        elapsed += segment_duration
    return result


class PersistentLedger:
    """Disk-backed ledger: a 72-hour run must not retain ~78M rows in RAM."""

    COLUMNS = (
        "request_id", "task", "batch_id", "sample_index", "planned_at", "timeout_seconds",
        "dispatch_started", "dispatched_at", "deadline", "terminal_at", "status", "error_code",
        "error_message", "result_checksum", "result_checksum_valid", "terminal_count", "failure_class",
    )

    def __init__(self, path: Path) -> None:
        self.connection = sqlite3.connect(path)
        self.connection.execute("PRAGMA journal_mode=WAL")
        self.connection.execute("PRAGMA synchronous=NORMAL")
        self.connection.execute("""
            CREATE TABLE episode (
              request_id TEXT PRIMARY KEY, task TEXT NOT NULL, batch_id TEXT NOT NULL,
              sample_index INTEGER NOT NULL, planned_at REAL NOT NULL, timeout_seconds REAL NOT NULL,
              dispatch_started INTEGER NOT NULL DEFAULT 0, dispatched_at REAL, deadline REAL,
              terminal_at REAL, status TEXT NOT NULL DEFAULT 'planned', error_code TEXT NOT NULL DEFAULT '',
              error_message TEXT NOT NULL DEFAULT '', result_checksum TEXT NOT NULL DEFAULT '',
              result_checksum_valid INTEGER NOT NULL DEFAULT 0,
              terminal_count INTEGER NOT NULL DEFAULT 0, failure_class TEXT NOT NULL DEFAULT 'pending'
            )
        """)
        self.connection.execute("CREATE INDEX episode_pending ON episode(dispatch_started, terminal_count)")
        self.connection.execute("CREATE INDEX episode_task_dispatched ON episode(task, dispatched_at)")
        self.connection.execute("""
            CREATE TABLE episode_terminal_event (
              event_id INTEGER PRIMARY KEY AUTOINCREMENT,
              request_id TEXT NOT NULL,
              terminal_at REAL NOT NULL,
              status TEXT NOT NULL,
              error_code TEXT NOT NULL,
              failure_class TEXT NOT NULL,
              result_checksum TEXT NOT NULL
            )
        """)
        self.connection.execute(
            "CREATE INDEX terminal_event_request ON episode_terminal_event(request_id, terminal_at)"
        )
        self.pending_writes = 0

    def _commit_periodically(self) -> None:
        self.pending_writes += 1
        if self.pending_writes >= 1000:
            self.connection.commit()
            self.pending_writes = 0

    def plan(
        self,
        request_id: str,
        task: str,
        batch_id: str,
        sample_index: int,
        now: float,
        timeout: float,
    ) -> None:
        self.connection.execute(
            """INSERT INTO episode(
                   request_id,task,batch_id,sample_index,planned_at,timeout_seconds
               ) VALUES(?,?,?,?,?,?)""",
            (request_id, task, batch_id, sample_index, now, timeout),
        )
        self._commit_periodically()

    def dispatched(self, request_id: str, now: float) -> None:
        self.connection.execute(
            """UPDATE episode
               SET dispatch_started=1,dispatched_at=?,deadline=?+timeout_seconds,status='dispatched'
               WHERE request_id=?""",
            (now, now, request_id),
        )
        self._commit_periodically()

    def terminal(self, result: Any, now: float) -> None:
        row = self.connection.execute(
            """SELECT dispatched_at,planned_at,timeout_seconds,terminal_count,deadline
               FROM episode WHERE request_id=?""",
            (result.request_id,),
        ).fetchone()
        if row is None:
            raise RuntimeError(f"terminal result has unknown request_id {result.request_id}")
        origin, planned, timeout, terminal_count, deadline = row
        terminal_count = int(terminal_count) + 1
        converted = stress.sample_result_dict(result)
        trajectory = converted["trajectory"]
        checksum_payload = {
            "status": str(result.status),
            "error_code": str(result.error_code),
            "trajectory": trajectory,
        }
        result_checksum = hashlib.sha256(
            json.dumps(
                checksum_payload, ensure_ascii=False, sort_keys=True, separators=(",", ":")
            ).encode("utf-8")
        ).hexdigest()
        checksum_valid = bool(trajectory) and len(result_checksum) == 64
        normalized_error = str(result.error_code or "").upper()
        config_error = (
            normalized_error.startswith("1")
            or normalized_error.startswith("ERR_REQUEST")
            or "INVALID_REQUEST" in normalized_error
            or "PROTOCOL" in normalized_error
        )
        if terminal_count > 1:
            failure_class = "duplicate_terminal_result"
        elif config_error:
            failure_class = "test_config_error"
        elif now > float(deadline if deadline is not None else float(origin if origin is not None else planned) + float(timeout)):
            failure_class = "late_result"
        elif str(result.status).lower() in stability.TERMINAL_SUCCESS and checksum_valid:
            failure_class = "none"
        else:
            failure_class = "uenv_error"
        self.connection.execute(
            """UPDATE episode SET terminal_at=?,status=?,error_code=?,error_message=?,
               result_checksum=?,result_checksum_valid=?,terminal_count=?,failure_class=?
               WHERE request_id=?""",
            (
                now, result.status, result.error_code, result.error_message,
                result_checksum, int(checksum_valid), terminal_count, failure_class,
                result.request_id,
            ),
        )
        self.connection.execute(
            """INSERT INTO episode_terminal_event(
                   request_id,terminal_at,status,error_code,failure_class,result_checksum
               ) VALUES(?,?,?,?,?,?)""",
            (
                result.request_id,
                now,
                str(result.status),
                str(result.error_code),
                failure_class,
                result_checksum,
            ),
        )
        self._commit_periodically()

    def reconcile(self, now: float, grace: float) -> None:
        expired = self.connection.execute(
            """SELECT request_id FROM episode
               WHERE dispatch_started=1 AND terminal_count=0
                 AND (? >= COALESCE(deadline,dispatched_at+timeout_seconds,planned_at+timeout_seconds)+?)""",
            (now, grace),
        ).fetchall()
        self.connection.execute(
            """UPDATE episode SET status='timeout',error_code='NO_TERMINAL_RESULT',
               error_message='no terminal result before reconciliation grace expired',
               failure_class='no_terminal_result'
               WHERE dispatch_started=1 AND terminal_count=0
                 AND (? >= COALESCE(deadline,dispatched_at+timeout_seconds,planned_at+timeout_seconds)+?)""",
            (now, grace),
        )
        self.connection.executemany(
            """INSERT INTO episode_terminal_event(
                   request_id,terminal_at,status,error_code,failure_class,result_checksum
               ) VALUES(?,?,'timeout','NO_TERMINAL_RESULT','no_terminal_result','')""",
            [(str(row[0]), now) for row in expired],
        )
        self.connection.commit()

    def pending_count(self) -> int:
        return int(self.connection.execute(
            "SELECT count(*) FROM episode WHERE dispatch_started=1 AND failure_class='pending'"
        ).fetchone()[0])

    def latest_reconcile_at(self, grace: float) -> float:
        value = self.connection.execute(
            """SELECT max(COALESCE(deadline,dispatched_at+timeout_seconds,planned_at+timeout_seconds)+?)
               FROM episode WHERE dispatch_started=1 AND terminal_count=0""",
            (grace,),
        ).fetchone()[0]
        return float(value or 0.0)

    def config_error_count(self) -> int:
        return int(self.connection.execute(
            "SELECT count(*) FROM episode WHERE failure_class='test_config_error'"
        ).fetchone()[0])

    def export_csv(self, target: Path) -> None:
        self.connection.commit()
        with target.open("w", encoding="utf-8", newline="") as output:
            writer = csv.writer(output)
            writer.writerow(self.COLUMNS)
            for row in self.connection.execute("SELECT " + ",".join(self.COLUMNS) + " FROM episode ORDER BY planned_at"):
                writer.writerow(row)

    def close(self) -> None:
        self.connection.commit()
        self.connection.close()


class Workloads:
    def __init__(self, config: dict[str, Any]) -> None:
        self.config = config
        self.rows: dict[str, list[dict[str, Any]]] = {}
        self.rows["dscodebench"] = stress.load_dscodebench_jsonl(config["tasks"]["dscodebench"]["dataset_path"])
        swe_task = config["tasks"]["swebench_pro"]
        catalog = json.loads(Path(swe_task["dataset_path"]).read_text(encoding="utf-8"))
        instance_ids = json.loads(Path(swe_task["instance_list"]).read_text(encoding="utf-8"))
        if len(instance_ids) != 50 or len(set(instance_ids)) != 50:
            raise ValueError("SWE-bench Pro instance list must contain 50 unique IDs")
        self.rows["swebench_pro"] = [catalog[instance_id] for instance_id in instance_ids]
        self.rows["olymmath"] = []
        for path in sorted(Path(config["tasks"]["olymmath"]["dataset_path"]).glob("OlymMATH-*.jsonl")):
            for row in path.read_text(encoding="utf-8").splitlines():
                if row.strip():
                    self.rows["olymmath"].append(json.loads(row))
        scitab = json.loads(Path(config["tasks"]["scitab"]["dataset_path"]).read_text(encoding="utf-8"))
        self.rows["scitab"] = scitab if isinstance(scitab, list) else list(scitab.values())
        pubmed = json.loads(Path(config["tasks"]["pubmedqa"]["dataset_path"]).read_text(encoding="utf-8"))
        self.rows["pubmedqa"] = [{"pmid": key, **value} for key, value in pubmed.items()]
        for task, rows in self.rows.items():
            if not rows:
                raise ValueError(f"no workload rows for {task}")

    def payload(self, task: str, index: int, task_id: str, replay_url: str) -> tuple[dict[str, Any], dict[str, Any]]:
        row = self.rows[task][index % len(self.rows[task])]
        if task == "dscodebench":
            return stress.dscodebench_env_payload(row, task_id=task_id, min_steps_before_terminate=1), stress.dscodebench_reward_config()
        if task == "swebench_pro":
            return stress.swe_openhands_env_payload(
                instance_id=str(row["instance_id"]), benchmark_variant="pro", command_mode="full_shell",
                mode="fully_async", agent_pool_id="openhands-default",
                driver_entrypoint="integrations/openhands/run_swebenchpro_official.py", workspace_dir="/app",
                max_iterations=250, llm_config_path=self.config["tasks"][task]["openhands_llm_config"],
                instances_catalog=self.config["tasks"][task]["dataset_path"],
            ), stress.swe_reward_config()
        if task == "olymmath":
            dataset = "olymmath-hard" if str(row.get("difficulty", "")).upper() == "HARD" else "olymmath-easy"
            env = {"task_name": dataset, "data_source": dataset, "dataset": dataset, "question": row["problem"],
                   "language": row.get("language", ""), "difficulty": row.get("difficulty", ""), "task_id": task_id}
            return env, stress.rule_reward_config(str(row["answer"]))
        if task == "scitab":
            table = row["table_content_values"]
            question = f"Table:\n{json.dumps(table, ensure_ascii=False)}\nClaim: {row['claim']}\nReturn supports, refutes, or not enough info."
            return {"task_name": "scitab", "data_source": "scitab", "dataset": "scitab", "question": question,
                    "claim": row["claim"], "table_content_values": table, "task_id": task_id}, stress.rule_reward_config(str(row["label"]))
        context = "\n".join(str(value) for value in row["CONTEXTS"])
        question = f"Context:\n{context}\nQuestion: {row['QUESTION']}\nReturn yes, no, or maybe."
        return {"task_name": "pubmedqa", "data_source": "pubmedqa", "dataset": "pubmedqa", "question": question,
                "pmid": row["pmid"], "task_id": task_id}, stress.rule_reward_config(str(row["final_decision"]))


def configure_imports(gen_dir: Path) -> tuple[Any, Any]:
    sys.path.insert(0, str(gen_dir))
    from uenv.v1 import adapter_core_pb2  # type: ignore
    return adapter_core_pb2, adapter_core_pb2.SampleResult


def verify_formal_inputs(config: dict[str, Any], development_only: bool) -> dict[str, Any]:
    if development_only:
        return {"development_only": True}
    dataset_manifest = stability.verify_manifest(Path(config["datasets"]["manifest"]), root=Path(config["datasets"]["root"]))
    trace_manifest = stability.verify_manifest(Path(config["traces"]["manifest"]), root=Path(config["traces"]["root"]))
    if bool(dataset_manifest.get("development_only")) or bool(trace_manifest.get("development_only")):
        raise ValueError("formal run refuses development_only dataset or trace manifests")
    expected_tasks = set(stability.TASK_NAMES)
    if set(dataset_manifest.get("datasets", {})) != expected_tasks:
        raise ValueError("dataset_manifest.json must describe exactly all five stability tasks")
    if set(trace_manifest.get("datasets", {})) != expected_tasks:
        raise ValueError("trace_manifest.json must describe exactly all five stability tasks")
    frozen_dataset_files = {str(item["path"]) for item in dataset_manifest.get("files", [])}
    dscodebench_relative = Path(config["tasks"]["dscodebench"]["dataset_path"]).resolve().relative_to(
        Path(config["datasets"]["root"]).resolve()
    ).as_posix()
    if dscodebench_relative not in frozen_dataset_files:
        raise ValueError("DSCodeBench data file is not frozen by dataset_manifest.json")
    allowed_prefixes = tuple(
        str(value).lower() for value in config["traces"].get("formal_source_model_prefixes", [])
    )
    if not allowed_prefixes:
        raise ValueError("formal trace config requires formal_source_model_prefixes")
    source_models = trace_manifest.get("source_models", {})
    for task in stability.TASK_NAMES:
        values = source_models.get(task)
        if not isinstance(values, list) or not values:
            raise ValueError(f"trace manifest has no source model for {task}")
        if not all(str(value).lower().startswith(allowed_prefixes) for value in values):
            raise ValueError(f"{task} trace source is outside formal allowed model prefixes")
    openhands_config = Path(config["tasks"]["swebench_pro"]["openhands_llm_config"])
    if not openhands_config.is_file():
        raise ValueError(f"SWE-bench Pro replay config is missing: {openhands_config}")
    trace_stats = {}
    dataset_names = {"swebench_pro": "swe-bench-pro", "dscodebench": "dscodebench", "olymmath": "olymmath", "scitab": "scitab", "pubmedqa": "pubmedqa"}
    for task, task_config in config["tasks"].items():
        trace_stats[task] = stability.validate_trace_file(
            Path(task_config["trace_file"]), dataset=dataset_names[task], minimum=int(task_config["min_valid_traces"])
        )
    return {
        "development_only": False,
        "dataset_manifest": dataset_manifest,
        "trace_manifest": trace_manifest,
        "trace_stats": trace_stats,
        "openhands_replay_config": {
            "path": str(openhands_config),
            "sha256": stability.sha256_file(openhands_config),
        },
    }


def acceptance_fingerprint(
    config: dict[str, Any],
    fleet: dict[str, Any],
    args: argparse.Namespace,
    admission: dict[str, Any],
) -> dict[str, Any]:
    identity = fleet.get("acceptance_identity")
    if args.execute and not args.development_only:
        required = {
            "repo_git_sha", "server_binary_sha256", "worker_binary_sha256",
            "plugin_binary_sha256", "server_config_sha256", "protocol_version",
        }
        if not isinstance(identity, dict) or not required.issubset(identity):
            raise ValueError(
                f"formal fleet manifest acceptance_identity must contain {sorted(required)}"
            )
        if identity.get("server_env") != config.get("server_env"):
            raise ValueError("formal fleet manifest server_env does not match stability config")
    worker_capacities = sorted(int(worker["capacity"]) for worker in fleet["workers"])
    material = {
        "config_sha256": stability.sha256_file(args.config),
        "dataset_manifest_sha256": stability.sha256_file(Path(config["datasets"]["manifest"]))
        if not args.development_only else "",
        "trace_manifest_sha256": stability.sha256_file(Path(config["traces"]["manifest"]))
        if not args.development_only else "",
        "openhands_replay_config_sha256": admission.get("openhands_replay_config", {}).get("sha256", ""),
        "acceptance_identity": identity or {},
        "server_env": config.get("server_env", {}),
        "worker_count": len(worker_capacities),
        "worker_capacities": worker_capacities,
        "total_capacity": int(fleet["total_capacity"]),
        "arrival_mode": config["load"]["arrival_mode"],
        "formal_arrival_segments": config["load"].get("formal_arrival_segments", []),
        "tasks": {
            task: {
                "allocation_share": config["tasks"][task]["allocation_share"],
                "target_rate_eps": config["tasks"][task]["target_rate_eps"],
                "latency_profile": config["tasks"][task]["latency_profile"],
            }
            for task in stability.TASK_NAMES
        },
        "seed": config["run"]["seed"],
    }
    encoded = json.dumps(material, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return {"sha256": hashlib.sha256(encoded).hexdigest(), "material": material}


def verify_disk_budget(
    config: dict[str, Any], args: argparse.Namespace, required: dict[str, Any]
) -> dict[str, int]:
    task_names = (
        [args.task]
        if args.phase == "selfcheck" and args.task
        else list(stability.TASK_NAMES)
    )
    estimated_episodes = math.ceil(
        sum(stability.phase_rate(config["tasks"][task], args.phase) for task in task_names)
        * float(args.duration_seconds)
    )
    bytes_per_episode = int(config["run"].get("estimated_persisted_bytes_per_episode", 768))
    fixed_bytes = int(config["run"].get("estimated_fixed_artifact_bytes", 2 * 1024**3))
    required_bytes = math.ceil(
        (estimated_episodes * bytes_per_episode + fixed_bytes)
        * float(config["run"].get("disk_headroom_multiplier", 1.2))
    )
    root = args.artifacts.resolve()
    probe = root if root.exists() else root.parent
    free_bytes = int(shutil.disk_usage(probe).free)
    if args.execute and free_bytes < required_bytes:
        raise ValueError(
            f"insufficient artifact disk: free={free_bytes} required={required_bytes} "
            f"estimated_episodes={estimated_episodes}"
        )
    return {
        "estimated_episodes": estimated_episodes,
        "estimated_required_bytes": required_bytes,
        "observed_free_bytes": free_bytes,
        "required_slots": int(required["total_slots"]),
    }


def load_fleet(path: Path, required_slots: int) -> dict[str, Any]:
    fleet = json.loads(path.read_text(encoding="utf-8"))
    workers = fleet.get("workers")
    if not isinstance(workers, list) or not workers:
        raise ValueError("fleet manifest requires a non-empty workers list")
    ids = [str(worker["worker_id"]) for worker in workers]
    if len(set(ids)) != len(ids):
        raise ValueError("fleet worker_id values must be unique")
    total_capacity = sum(int(worker["capacity"]) for worker in workers)
    if total_capacity < required_slots:
        raise ValueError(f"registered fleet capacity {total_capacity} < required logical slots {required_slots}")
    fleet["total_capacity"] = total_capacity
    return fleet


class AvailabilitySampler:
    """Choose at most one real submission result for each wall-clock second."""

    def __init__(self) -> None:
        self._by_second: dict[int, dict[str, Any]] = {}
        self._lock = asyncio.Lock()
        self._last_emitted_second = -1

    async def record_submission(self, timestamp: float, ok: bool, error: str = "") -> None:
        second = int(timestamp)
        async with self._lock:
            if second <= self._last_emitted_second:
                return
            self._by_second.setdefault(
                second,
                {
                    "timestamp": second,
                    "probe_type": "episode_rpc",
                    "reachable": ok,
                    "rpc_status": "write_ok" if ok else "write_failed",
                    "error_type": error,
                },
            )

    async def pop(self, second: int) -> dict[str, Any] | None:
        async with self._lock:
            self._last_emitted_second = max(self._last_emitted_second, second)
            for stale in [value for value in self._by_second if value < second]:
                self._by_second.pop(stale, None)
            return self._by_second.pop(second, None)


async def health_probe(
    endpoint: str,
    pb2: Any,
    path: Path,
    stop: asyncio.Event,
    sampler: AvailabilitySampler,
) -> None:
    channel = grpc.aio.insecure_channel(endpoint)
    rpc = channel.unary_unary(
        "/uenv.bridge.v1.AdapterCoreService/HealthCheck",
        request_serializer=pb2.HealthCheckRequest.SerializeToString,
        response_deserializer=pb2.HealthCheckResponse.FromString,
    )
    try:
        while True:
            second = int(time.time())
            await asyncio.sleep(max(0.0, second + 1 - time.time()))
            row = await sampler.pop(second)
            if row is None:
                ok, error, rpc_status = False, "", ""
                try:
                    response = await rpc(pb2.HealthCheckRequest(), timeout=0.8)
                    ok = bool(response.ok)
                    rpc_status = str(getattr(response, "status", "") or ("ok" if ok else "not_ok"))
                except Exception as exc:  # availability evidence must retain connection errors
                    error = f"{type(exc).__name__}:{exc}"
                    rpc_status = "rpc_error"
                row = {
                    "timestamp": second,
                    "probe_type": "health_check",
                    "reachable": ok,
                    "rpc_status": rpc_status,
                    "error_type": error,
                }
            stability.append_csv(
                path,
                row,
                ["timestamp", "probe_type", "reachable", "rpc_status", "error_type"],
            )
            if stop.is_set():
                break
    finally:
        await channel.close()


def proc_metrics(pids: list[int]) -> dict[str, int]:
    rss, fds, threads, processes = 0, 0, 0, 0
    for pid in pids:
        root = Path("/proc") / str(pid)
        try:
            status = (root / "status").read_text(encoding="utf-8").splitlines()
            values = {line.split(":", 1)[0]: line.split(":", 1)[1].strip() for line in status if ":" in line}
            rss += int(values.get("VmRSS", "0 kB").split()[0]) * 1024
            threads += int(values.get("Threads", "0"))
            fds += sum(1 for _ in (root / "fd").iterdir())
            processes += 1
        except (FileNotFoundError, PermissionError, ProcessLookupError, ValueError):
            continue
    return {"rss_bytes": rss, "open_fds": fds, "threads": threads, "processes": processes}


async def resource_probe(fleet: dict[str, Any], path: Path, stop: asyncio.Event, interval: float) -> None:
    pids = [int(fleet["server"]["pid"])] + [int(worker["pid"]) for worker in fleet["workers"]]
    while not stop.is_set():
        started = time.time()
        if fleet.get("resource_probe_argv"):
            completed = await asyncio.to_thread(
                subprocess.run, [str(value) for value in fleet["resource_probe_argv"]],
                check=True, text=True, capture_output=True,
            )
            measured = json.loads(completed.stdout)
        else:
            measured = proc_metrics(pids)
        row = {"timestamp": started, **measured, "running_containers": int(measured.get("running_containers", 0)),
               "worker_exits": int(measured.get("worker_exits", 0)),
               "oom_events": int(measured.get("oom_events", 0)),
               "fd_exhaustions": int(measured.get("fd_exhaustions", 0)),
               "thread_exhaustions": int(measured.get("thread_exhaustions", 0)),
               "uenv_crashes": int(measured.get("uenv_crashes", 0)),
               "manual_restarts": int(measured.get("manual_restarts", 0))}
        stability.append_csv(path, row, list(row))
        try:
            await asyncio.wait_for(
                stop.wait(),
                timeout=max(0.0, interval - (time.time() - started)),
            )
        except asyncio.TimeoutError:
            pass


class StreamShard:
    def __init__(
        self,
        index: int,
        endpoint: str,
        pb2: Any,
        ledger: PersistentLedger,
        availability: AvailabilitySampler,
    ) -> None:
        self.index, self.endpoint, self.pb2, self.ledger = index, endpoint, pb2, ledger
        self.availability = availability
        self.queue: asyncio.Queue[Any | None] = asyncio.Queue(maxsize=2048)
        self.channel: Any = None
        self.call: Any = None
        self.reader: asyncio.Task | None = None

    async def connect(self) -> None:
        self.channel = grpc.aio.insecure_channel(self.endpoint)
        method = self.channel.stream_stream(
            "/uenv.bridge.v1.AdapterCoreService/ExecuteBatchStream",
            request_serializer=self.pb2.SampleEnvelope.SerializeToString,
            response_deserializer=self.pb2.SampleResult.FromString,
        )
        self.call = method()
        self.reader = asyncio.create_task(self.read_results())

    async def read_results(self) -> None:
        try:
            async for result in self.call:
                self.ledger.terminal(result, time.time())
        except grpc.aio.AioRpcError:
            pass

    async def run(self) -> None:
        await self.connect()
        while True:
            envelope = await self.queue.get()
            if envelope is None:
                break
            while True:
                try:
                    if self.reader is not None and self.reader.done():
                        await self.channel.close()
                        await self.connect()
                    await self.call.write(envelope)
                    dispatched_at = time.time()
                    self.ledger.dispatched(envelope.request_id, dispatched_at)
                    await self.availability.record_submission(dispatched_at, True)
                    break
                except grpc.aio.AioRpcError as exc:
                    await self.availability.record_submission(
                        time.time(), False, f"{exc.code().name}:{exc.details()}"
                    )
                    if self.channel is not None:
                        await self.channel.close()
                    await asyncio.sleep(0.5)
                    await self.connect()
        try:
            await self.call.done_writing()
        except grpc.aio.AioRpcError:
            pass

    async def close(self) -> None:
        await self.queue.put(None)


async def produce_task(
    task: str, task_config: dict[str, Any], *, config: dict[str, Any], phase: str,
    duration: float, replay_url: str, pb2: Any, workloads: Workloads,
    ledger: PersistentLedger, shards: list[StreamShard], run_id: str,
) -> None:
    rate = stability.phase_rate(task_config, phase)
    seed = int(config["run"]["seed"]) + stability.TASK_NAMES.index(task)
    origin_ns = time.monotonic_ns()
    lag_limit = float(config["load"]["scheduler_lag_abort_seconds"])
    sequence = 0
    for segment_index, segment in enumerate(arrival_segments(config, phase, duration)):
        mode = segment["mode"]
        batch_size = segment["batch_size"]
        segment_start_ns = origin_ns + int(segment["start_seconds"] * 1_000_000_000)
        segment_stop_ns = segment_start_ns + int(segment["duration_seconds"] * 1_000_000_000)
        for local_sequence, deadline_ns in stability.iter_planned_times(
            rate,
            mode=mode,
            batch_size=batch_size,
            seed=seed + segment_index * 1009,
            start_ns=segment_start_ns,
        ):
            if deadline_ns >= segment_stop_ns:
                break
            delay = (deadline_ns - time.monotonic_ns()) / 1_000_000_000
            if delay > 0:
                await asyncio.sleep(delay)
            lag = (time.monotonic_ns() - deadline_ns) / 1_000_000_000
            if lag > lag_limit:
                raise RuntimeError(f"load generator overload task={task} scheduler_lag={lag:.3f}s")
            batch_index = local_sequence // batch_size
            batch_id = f"{run_id}-{task}-segment-{segment_index}-batch-{batch_index}"
            request_id = f"{run_id}-{task}-{sequence}"
            env_config, reward_config = workloads.payload(task, sequence, request_id, replay_url)
            envelope = stress.make_sample_envelope(
                pb2, request_id=request_id, batch_id=batch_id, sample_index=local_sequence % batch_size,
                env_type=str(task_config["env_type"]), parallel_mode=str(config["run"]["parallel_mode"]),
                env_config=env_config, reward_config=reward_config,
                sample_context={
                    "run_id": run_id, "task": task, "dataset": task,
                    "sequence": sequence, "arrival_mode": mode,
                },
                timeout_seconds=int(task_config["timeout_seconds"]), max_steps=int(task_config["max_steps"]),
                model_url=bind_replay_url(replay_url, episode_id=request_id, task=task)
                if task != "swebench_pro" else "",
                model_name=f"uenv-trace-{task}",
            )
            ledger.plan(
                request_id,
                task,
                batch_id,
                local_sequence % batch_size,
                time.time(),
                float(task_config["timeout_seconds"]),
            )
            shard = shards[int.from_bytes(hashlib.sha256(request_id.encode()).digest()[:8], "big") % len(shards)]
            await shard.queue.put(envelope)
            sequence += 1


async def execute(args: argparse.Namespace, config: dict[str, Any], fleet: dict[str, Any], artifacts: Path) -> dict[str, Any]:
    pb2, _result_type = configure_imports(args.gen_dir)
    workloads = Workloads(config)
    ledger = PersistentLedger(artifacts / "episode.sqlite")
    stop = asyncio.Event()
    availability = AvailabilitySampler()
    probes = [
        asyncio.create_task(
            health_probe(
                args.endpoint,
                pb2,
                artifacts / "availability.csv",
                stop,
                availability,
            )
        ),
        asyncio.create_task(resource_probe(fleet, artifacts / "resource.csv", stop, float(config["run"]["resource_sample_seconds"]))),
    ]
    shards = [
        StreamShard(index, args.endpoint, pb2, ledger, availability)
        for index in range(int(config["run"]["stream_shards"]))
    ]
    shard_tasks = [asyncio.create_task(shard.run()) for shard in shards]
    started = time.time()
    tasks = [args.task] if args.phase == "selfcheck" else list(stability.TASK_NAMES)
    status = "completed"
    error = ""
    active_observed_seconds = 0.0
    try:
        await asyncio.gather(*[
            produce_task(
                task, config["tasks"][task], config=config, phase=args.phase,
                duration=args.duration_seconds, replay_url=args.replay_url, pb2=pb2,
                workloads=workloads, ledger=ledger, shards=shards, run_id=args.run_id,
            ) for task in tasks
        ])
        await asyncio.sleep(max(0.0, started + float(args.duration_seconds) - time.time()))
        active_observed_seconds = time.time() - started
        stop.set()
        await asyncio.gather(*probes, return_exceptions=True)
        for shard in shards:
            await shard.close()
        await asyncio.gather(*shard_tasks)
        grace = float(config["run"]["reconciliation_grace_seconds"])
        while ledger.pending_count():
            now = time.time()
            ledger.reconcile(now, grace)
            if not ledger.pending_count():
                break
            latest = ledger.latest_reconcile_at(grace)
            await asyncio.sleep(max(0.05, min(1.0, latest - now)))
        if ledger.pending_count():
            raise RuntimeError(f"ledger still has {ledger.pending_count()} pending dispatched episodes")
        if ledger.config_error_count():
            raise RuntimeError(
                f"formal run invalid: ledger contains {ledger.config_error_count()} request/config errors"
            )
    except Exception as exc:
        status, error = "failed", f"{type(exc).__name__}: {exc}"
        raise
    finally:
        stop.set()
        await asyncio.gather(*probes, return_exceptions=True)
        for shard in shards:
            if shard.channel is not None:
                await shard.channel.close()
        if args.export_episode_csv:
            ledger.export_csv(artifacts / "episode.csv")
        ledger.close()
    return {
        "status": status,
        "error": error,
        "started_unix": started,
        "observed_seconds": active_observed_seconds or (time.time() - started),
    }


def append_operator_log(path: Path, command: list[Any], reason: str) -> None:
    with path.open("a", encoding="utf-8") as target:
        target.write(json.dumps({
            "timestamp": time.time(),
            "operator": "stability_runner",
            "command": [str(value) for value in command],
            "reason": reason,
        }, ensure_ascii=False, sort_keys=True) + "\n")


def cleanup_fleet(fleet: dict[str, Any], operator_log: Path) -> dict[str, int]:
    cleanup = fleet.get("cleanup_argv")
    probe = fleet.get("cleanup_probe_argv")
    if not isinstance(cleanup, list) or not cleanup or not isinstance(probe, list) or not probe:
        return {"remaining_workers": -1, "remaining_containers": -1, "remaining_processes": -1}
    append_operator_log(operator_log, cleanup, "clean up test-owned fleet")
    subprocess.run([str(value) for value in cleanup], check=True)
    append_operator_log(operator_log, probe, "verify no test-owned processes or containers remain")
    completed = subprocess.run([str(value) for value in probe], check=True, text=True, capture_output=True)
    result = json.loads(completed.stdout)
    return {key: int(result[key]) for key in ("remaining_workers", "remaining_containers", "remaining_processes")}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--config", type=Path, default=DEFAULT_CONFIG)
    parser.add_argument("--phase", choices=("selfcheck", "reference", "stability", "capacity", "burst", "fault"), required=True)
    parser.add_argument("--task", choices=stability.TASK_NAMES)
    parser.add_argument("--duration-seconds", type=float)
    parser.add_argument("--endpoint", default="127.0.0.1:8088")
    parser.add_argument("--replay-url", default="http://127.0.0.1:8899/v1/chat/completions")
    parser.add_argument("--gen-dir", type=Path, required=True)
    parser.add_argument("--fleet-manifest", type=Path, required=True)
    parser.add_argument("--artifacts", type=Path, default=Path("artifacts"))
    parser.add_argument("--development-only", action="store_true")
    parser.add_argument(
        "--export-episode-csv",
        action="store_true",
        help="Stream the authoritative SQLite ledger to episode.csv after the run (large for 72h)",
    )
    parser.add_argument("--fault-source-run-id")
    parser.add_argument("--fault-ledger-db", type=Path)
    parser.add_argument("--fault-server-log", type=Path)
    parser.add_argument("--execute", action="store_true")
    args = parser.parse_args()
    if args.phase == "selfcheck" and not args.task:
        parser.error("--phase selfcheck requires --task")
    if args.phase == "fault" and (
        not args.fault_source_run_id or not args.fault_ledger_db or not args.fault_server_log
    ):
        parser.error(
            "--phase fault requires --fault-source-run-id, --fault-ledger-db and --fault-server-log"
        )
    return args


def main() -> int:
    args = parse_args()
    config = stability.load_config(args.config)
    args.duration_seconds = stability.phase_duration(config, args.phase, args.duration_seconds) if args.phase != "fault" else 0
    required = stability.required_capacity(config, "reference" if args.phase == "fault" else args.phase)
    fleet = load_fleet(args.fleet_manifest, required["total_slots"])
    disk_budget = verify_disk_budget(config, args, required)
    if args.execute and not args.development_only:
        for field in ("resource_probe_argv", "cleanup_argv", "cleanup_probe_argv"):
            if not isinstance(fleet.get(field), list) or not fleet[field]:
                raise ValueError(f"formal execution fleet manifest requires {field}")
    admission = verify_formal_inputs(config, args.development_only)
    fingerprint = acceptance_fingerprint(config, fleet, args, admission)
    artifact_run_id = f"{args.phase}-{time.strftime('%Y%m%d-%H%M%S')}-{uuid.uuid4().hex[:8]}"
    args.run_id = args.fault_source_run_id if args.phase == "fault" else artifact_run_id
    artifacts = (args.artifacts / artifact_run_id).resolve()
    artifacts.mkdir(parents=True, exist_ok=False)
    resolved = {**config, "active": {"phase": args.phase, "duration_seconds": args.duration_seconds, "capacity": required}}
    (artifacts / "resolved_config.json").write_text(json.dumps(resolved, indent=2, sort_keys=True), encoding="utf-8")
    manifest: dict[str, Any] = {
        "schema_version": 1, "run_id": args.run_id, "phase": args.phase,
        "duration_seconds": args.duration_seconds, "development_only": args.development_only,
        "episode_csv_exported": args.export_episode_csv,
        "started_unix": time.time(), "admission": admission, "fleet": fleet,
        "acceptance_fingerprint": fingerprint,
        "disk_budget": disk_budget,
        "status": "preflight_passed", "uenv_crashes": 0, "manual_restarts": 0,
        "oom_events": 0, "fd_exhaustions": 0, "thread_exhaustions": 0,
    }
    if not args.execute:
        (artifacts / "manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True), encoding="utf-8")
        print(f"[stability] preflight PASS artifacts={artifacts}")
        return 0
    if args.phase == "fault":
        command = [
            sys.executable,
            str(HERE / "inject_stability_faults.py"),
            "--run-id",
            args.run_id,
            "--fleet-manifest",
            str(args.fleet_manifest),
            "--ledger-db",
            str(args.fault_ledger_db),
            "--fault-csv",
            str(artifacts / "fault.csv"),
            "--server-log",
            str(args.fault_server_log),
            "--execute",
        ]
        append_operator_log(
            artifacts / "operator.log", command, "run isolated formal fault phase"
        )
        completed = subprocess.run(command, text=True, capture_output=True)
        manifest.update({
            "status": "completed" if completed.returncode == 0 else "failed",
            "fault_stdout": completed.stdout,
            "fault_stderr": completed.stderr,
            "finished_unix": time.time(),
        })
        (artifacts / "manifest.json").write_text(
            json.dumps(manifest, indent=2, sort_keys=True), encoding="utf-8"
        )
        return completed.returncode
    try:
        result = asyncio.run(execute(args, config, fleet, artifacts))
        manifest.update(result)
        manifest["status"] = "completed"
        return_code = 0
    except Exception as exc:
        manifest["status"] = "failed"
        manifest["error"] = f"{type(exc).__name__}: {exc}"
        return_code = 1
    finally:
        manifest["finished_unix"] = time.time()
        try:
            manifest["cleanup"] = cleanup_fleet(fleet, artifacts / "operator.log")
        except Exception as exc:
            manifest["cleanup"] = {
                "remaining_workers": -1, "remaining_containers": -1, "remaining_processes": -1,
                "error": f"{type(exc).__name__}: {exc}",
            }
            return_code = 1
        (artifacts / "manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True), encoding="utf-8")
    return return_code


if __name__ == "__main__":
    raise SystemExit(main())
