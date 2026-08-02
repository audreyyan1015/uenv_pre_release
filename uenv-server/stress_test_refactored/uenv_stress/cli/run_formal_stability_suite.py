#!/usr/bin/env python3
"""正式稳定性验收套件入口。

这个文件实现 UEnv 稳定性验收的完整执行流程，覆盖自检、基线、长稳、容量、突发和故障阶段。它面向评审关心的验收问题：压测输入是否冻结、调度是否可恢复、资源是否稳定、Episode 是否有可追溯记录、故障注入后是否能恢复到一致状态。

实现逻辑是：先绑定 replay 服务地址并校验轨迹、清单、磁盘预算和 fleet 配置；然后用 PersistentLedger 记录每个 Episode 的计划时间、启动状态、完成状态和结果文件；Workloads 根据 DSCodeBench、规则任务和 SWE-bench Pro 生成 Episode；异步 producer 按阶段到达率投放任务，execute 通过 gRPC 调用 UEnv Server；资源采样、健康探测和故障探测同时写入证据文件；最后清理本次创建的进程并生成可审计的验收产物。"""

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
from urllib.request import urlopen

import grpc

from uenv_stress.core import episode
from uenv_stress.core import result as result_api
from uenv_stress.core import suite_metrics
from uenv_stress.core import stability_test_common as stability
from uenv_stress.workloads import dscodebench
from uenv_stress.workloads import rule_tasks
from uenv_stress.workloads import swebench_pro


PACKAGE_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_CONFIG = (
    PACKAGE_ROOT / "uenv_stress" / "config" / "stability_suite.json"
)


def bind_replay_url(
    replay_url: str,
    *,
    episode_id: str,
    task: str,
    sequence: int,
    trace_id: str,
    source_model: str,
    pair_id: str,
) -> str:
    """Bind ordinary Code/Math model calls to one replay Episode."""
    parsed = urlsplit(replay_url)
    query = dict(parse_qsl(parsed.query, keep_blank_values=True))
    query.update(
        {
            "uenv_episode_id": episode_id,
            "uenv_dataset": task,
            "uenv_sequence": str(sequence),
            "uenv_trace_id": trace_id,
            "uenv_source_model": source_model,
            "uenv_pair_id": pair_id,
        }
    )
    return urlunsplit((parsed.scheme, parsed.netloc, parsed.path, urlencode(query), parsed.fragment))


def capture_replay_health(replay_url: str, target: Path) -> dict[str, Any]:
    parsed = urlsplit(replay_url)
    health_url = urlunsplit((parsed.scheme, parsed.netloc, "/health", "", ""))
    try:
        with urlopen(health_url, timeout=10) as response:
            document = json.loads(response.read().decode("utf-8"))
        if not isinstance(document, dict):
            raise ValueError("replay health response must be a JSON object")
        document["available"] = bool(document.get("ok"))
        document["source_url"] = health_url
    except Exception as exc:
        document = {
            "available": False,
            "source_url": health_url,
            "error": f"{type(exc).__name__}: {exc}",
        }
    target.write_text(
        json.dumps(document, indent=2, ensure_ascii=False, sort_keys=True),
        encoding="utf-8",
    )
    return document


def arrival_segments(
    config: dict[str, Any], phase: str, duration: float
) -> list[dict[str, Any]]:
    if phase in {"reference", "stability", "pressure"} and config["load"].get(
        "formal_arrival_segments"
    ):
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

    COLUMNS = result_api.EPISODE_OBSERVATION_FIELDS
    INTEGER_COLUMNS = {
        "schema_version", "sample_index", "sequence", "dispatch_started", "done",
        "terminal_count", "trace_slot", "trace_corpus_size", "actual_steps",
        "response_tokens", "training_trace_valid", "result_checksum_valid",
    }
    REAL_COLUMNS = {
        "planned_at", "dispatched_at", "deadline", "terminal_at", "timeout_seconds",
        "end_to_end_ms", "batch_rpc_latency_ms", "reward",
    }

    def __init__(self, path: Path) -> None:
        self.connection = sqlite3.connect(path)
        self.connection.execute("PRAGMA journal_mode=WAL")
        self.connection.execute("PRAGMA synchronous=NORMAL")
        definitions = []
        for column in self.COLUMNS:
            sql_type = (
                "INTEGER" if column in self.INTEGER_COLUMNS
                else "REAL" if column in self.REAL_COLUMNS
                else "TEXT"
            )
            constraint = " PRIMARY KEY" if column == "request_id" else " NOT NULL"
            definitions.append(f"{column} {sql_type}{constraint}")
        self.connection.execute("CREATE TABLE episode (" + ",".join(definitions) + ")")
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
        *,
        envelope: Any | None = None,
        suite: str = "stability",
        run_id: str = "",
        phase: str = "",
        arrival_mode: str = "",
    ) -> None:
        observation = (
            result_api.episode_observation_from_envelope(
                envelope,
                suite=suite,
                run_id=run_id,
                phase=phase,
                planned_at=now,
                arrival_mode=arrival_mode,
            )
            if envelope is not None
            else result_api.new_episode_observation(
                suite=suite,
                run_id=run_id,
                phase=phase,
                task=task,
                dataset=task,
                episode_id=request_id,
                request_id=request_id,
                batch_id=batch_id,
                sample_index=sample_index,
                planned_at=now,
                timeout_seconds=timeout,
                deadline=now + timeout,
                arrival_mode=arrival_mode,
                replay_strategy="round_robin_episode",
            )
        )
        self.connection.execute(
            "INSERT INTO episode(" + ",".join(self.COLUMNS) + ") VALUES("
            + ",".join("?" for _ in self.COLUMNS) + ")",
            tuple(observation[column] for column in self.COLUMNS),
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
        stored = self.connection.execute(
            "SELECT " + ",".join(self.COLUMNS) + " FROM episode WHERE request_id=?",
            (result.request_id,),
        ).fetchone()
        if stored is None:
            raise RuntimeError(f"terminal result has unknown request_id {result.request_id}")
        observation = dict(zip(self.COLUMNS, stored))
        origin = observation["dispatched_at"]
        planned = observation["planned_at"]
        timeout = observation["timeout_seconds"]
        terminal_count = observation["terminal_count"]
        deadline = observation["deadline"]
        terminal_count = int(terminal_count) + 1
        converted = result_api.sample_result_dict(result)
        trajectory = converted["trajectory"]
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
        elif str(result.status).lower() in stability.TERMINAL_SUCCESS and bool(trajectory):
            failure_class = "none"
        else:
            failure_class = "uenv_error"
        finalized = result_api.finalize_episode_observation(
            observation,
            result=converted,
            terminal_at=now,
            dispatched_at=float(origin or 0.0),
            terminal_count=terminal_count,
            failure_class=failure_class,
        )
        self.connection.execute(
            "UPDATE episode SET "
            + ",".join(f"{column}=?" for column in self.COLUMNS if column != "request_id")
            + " WHERE request_id=?",
            tuple(finalized[column] for column in self.COLUMNS if column != "request_id")
            + (result.request_id,),
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
                finalized["result_checksum"],
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
               failure_class='no_terminal_result',terminal_at=?,
               end_to_end_ms=CASE
                 WHEN COALESCE(dispatched_at,planned_at)>0
                 THEN MAX(0,(?-COALESCE(dispatched_at,planned_at))*1000)
                 ELSE 0 END,
               training_trace_errors_json='["no SampleResult"]'
               WHERE dispatch_started=1 AND terminal_count=0
                 AND (? >= COALESCE(deadline,dispatched_at+timeout_seconds,planned_at+timeout_seconds)+?)""",
            (now, now, now, grace),
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

    def export_jsonl(self, target: Path) -> int:
        self.connection.commit()
        rows = self.connection.execute(
            "SELECT " + ",".join(self.COLUMNS) + " FROM episode ORDER BY planned_at"
        )
        return result_api.write_episode_observations_jsonl(
            target,
            (dict(zip(self.COLUMNS, row)) for row in rows),
        )

    def count(self) -> int:
        return int(self.connection.execute("SELECT count(*) FROM episode").fetchone()[0])

    def close(self) -> None:
        self.connection.commit()
        self.connection.close()


class Workloads:
    def __init__(self, config: dict[str, Any]) -> None:
        self.config = config
        self.rows: dict[str, list[dict[str, Any]]] = {}
        self.rows_by_dataset_id: dict[str, dict[str, dict[str, Any]]] = {}
        self.replay_traces: dict[str, list[dict[str, Any]]] = {}
        self.rows["dscodebench"] = dscodebench.load_dscodebench_jsonl(
            config["tasks"]["dscodebench"]["dataset_path"]
        )
        swe_task = config["tasks"]["swebench_pro"]
        catalog = json.loads(Path(swe_task["dataset_path"]).read_text(encoding="utf-8"))
        instance_ids = json.loads(Path(swe_task["instance_list"]).read_text(encoding="utf-8"))
        if len(instance_ids) != 50 or len(set(instance_ids)) != 50:
            raise ValueError("SWE-bench Pro instance list must contain 50 unique IDs")
        self.rows["swebench_pro"] = [catalog[instance_id] for instance_id in instance_ids]
        for task in rule_tasks.TASK_NAMES:
            self.rows[task] = rule_tasks.load_task_rows(
                task, config["tasks"][task]["dataset_path"]
            )
        for task, rows in self.rows.items():
            if not rows:
                raise ValueError(f"no workload rows for {task}")
            indexed: dict[str, dict[str, Any]] = {}
            for row in rows:
                dataset_id = stability.trace_dataset_id(row)
                if dataset_id in indexed:
                    raise ValueError(
                        f"duplicate workload dataset identity {task}/{dataset_id}"
                    )
                indexed[dataset_id] = row
            self.rows_by_dataset_id[task] = indexed
        for task, task_config in config["tasks"].items():
            trace_path = Path(task_config["trace_file"])
            traces = [
                json.loads(line)
                for line in trace_path.read_text(encoding="utf-8").splitlines()
                if line.strip()
            ]
            if not traces:
                raise ValueError(f"no replay traces for {task}")
            self.replay_traces[task] = traces

    def replay_selection(self, task: str, sequence: int) -> dict[str, Any]:
        trace, slot = stability.select_trace_for_sequence(
            self.replay_traces[task],
            sequence=sequence,
            sampling_policy=str(self.config["tasks"][task]["sampling_policy"]),
        )
        return {
            "trace_id": str(trace["trace_id"]),
            "source_model": str(trace["source_model"]),
            "pair_id": stability.trace_pair_id(trace),
            "trace_slot": slot,
        }

    def payload(
        self,
        task: str,
        index: int,
        task_id: str,
        replay_url: str,
        replay_selection: dict[str, Any],
    ) -> tuple[dict[str, Any], dict[str, Any]]:
        pair_id = str(replay_selection["pair_id"])
        row = self.rows_by_dataset_id[task].get(pair_id)
        if row is None:
            raise ValueError(
                f"replay pair {task}/{pair_id} has no matching workload row"
            )
        if task == "dscodebench":
            return dscodebench.dscodebench_env_payload(
                row, task_id=task_id, min_steps_before_terminate=1
            ), dscodebench.dscodebench_reward_config()
        if task == "swebench_pro":
            return swebench_pro.swe_openhands_env_payload(
                instance_id=str(row["instance_id"]), benchmark_variant="pro", command_mode="full_shell",
                mode="fully_async", agent_pool_id="openhands-default",
                driver_entrypoint="integrations/openhands/run_swebenchpro_official.py", workspace_dir="/app",
                max_iterations=250, llm_config_path=self.config["tasks"][task]["openhands_llm_config"],
                instances_catalog=self.config["tasks"][task]["dataset_path"],
            ), swebench_pro.swe_reward_config()
        return rule_tasks.build_env_payload(
            task, row, index=index, task_id=task_id
        )


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
        path = Path(task_config["trace_file"])
        trace_stats[task] = stability.validate_trace_file(
            path,
            dataset=dataset_names[task],
            minimum=int(task_config["min_valid_traces"]),
        )
        traces = [
            json.loads(line)
            for line in path.read_text(encoding="utf-8").splitlines()
            if line.strip()
        ]
        if task in stability.PAIRED_TASK_NAMES:
            pairing = stability.validate_paired_trace_order(
                traces, expected_pairs=int(task_config["expected_pairs"])
            )
            manifest_pairing = trace_manifest.get("pairing", {}).get(task)
            if manifest_pairing != pairing:
                raise ValueError(f"{task} pairing evidence does not match trace corpus")
            trace_stats[task]["pairing"] = pairing
        else:
            if len(traces) != 50:
                raise ValueError("SWE-bench Pro must contain exactly 50 traces")
            if any(
                stability.source_model_family(trace["source_model"]) != "doubao"
                for trace in traces
            ):
                raise ValueError("SWE-bench Pro formal traces must be Doubao only")
            if not bool(trace_manifest.get("swebench_pro", {}).get("doubao_only")):
                raise ValueError("trace manifest must declare SWE-bench Pro doubao_only")
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
        "rate_basis": config["load"]["rate_basis"],
        "latency_replay": config["latency_replay"],
        "tasks": {
            task: {
                "allocation_share": config["tasks"][task]["allocation_share"],
                "target_rate_eps": config["tasks"][task]["target_rate_eps"],
                "sampling_policy": config["tasks"][task]["sampling_policy"],
                "replay_proxy_p95_seconds": config["tasks"][task][
                    "replay_proxy_p95_seconds"
                ],
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


def load_fleet(
    path: Path, required_slots: int, *, allow_overload: bool = False
) -> dict[str, Any]:
    fleet = json.loads(path.read_text(encoding="utf-8"))
    workers = fleet.get("workers")
    if not isinstance(workers, list) or not workers:
        raise ValueError("fleet manifest requires a non-empty workers list")
    ids = [str(worker["worker_id"]) for worker in workers]
    if len(set(ids)) != len(ids):
        raise ValueError("fleet worker_id values must be unique")
    total_capacity = sum(int(worker["capacity"]) for worker in workers)
    if total_capacity < required_slots and not allow_overload:
        raise ValueError(f"registered fleet capacity {total_capacity} < required logical slots {required_slots}")
    fleet["total_capacity"] = total_capacity
    fleet["capacity_assessment"] = {
        "required_logical_slots": required_slots,
        "available_capacity": total_capacity,
        "capacity_sufficient": total_capacity >= required_slots,
        "expected_overload_multiple": (
            required_slots / total_capacity if total_capacity else math.inf
        ),
        "intentional_overload": bool(allow_overload and total_capacity < required_slots),
    }
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
            replay_selection = workloads.replay_selection(task, sequence)
            env_config, reward_config = workloads.payload(
                task,
                sequence,
                request_id,
                replay_url,
                replay_selection,
            )
            envelope = episode.make_sample_envelope(
                pb2, request_id=request_id, batch_id=batch_id, sample_index=local_sequence % batch_size,
                env_type=str(task_config["env_type"]), parallel_mode=str(config["run"]["parallel_mode"]),
                env_config=env_config, reward_config=reward_config,
                sample_context={
                    "run_id": run_id, "task": task, "dataset": task,
                    "sequence": sequence, "arrival_mode": mode,
                    "trace_id": replay_selection["trace_id"],
                    "source_model": replay_selection["source_model"],
                    "pair_id": replay_selection["pair_id"],
                },
                timeout_seconds=int(task_config["timeout_seconds"]), max_steps=int(task_config["max_steps"]),
                model_url=bind_replay_url(
                    replay_url,
                    episode_id=request_id,
                    task=task,
                    sequence=sequence,
                    trace_id=replay_selection["trace_id"],
                    source_model=replay_selection["source_model"],
                    pair_id=replay_selection["pair_id"],
                )
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
                envelope=envelope,
                suite="stability",
                run_id=run_id,
                phase=phase,
                arrival_mode=mode,
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
    replay_health: dict[str, Any] = {}
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
        replay_health = await asyncio.to_thread(
            capture_replay_health,
            args.replay_url,
            artifacts / "replay-health.json",
        )
        for shard in shards:
            if shard.channel is not None:
                await shard.channel.close()
        if args.export_episode_csv:
            ledger.export_csv(artifacts / "episode.csv")
            ledger.export_jsonl(artifacts / "episode-observations.jsonl")
        ledger.close()
    return {
        "status": status,
        "error": error,
        "started_unix": started,
        "observed_seconds": active_observed_seconds or (time.time() - started),
        "replay_health_available": bool(replay_health.get("available")),
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
    parser.add_argument(
        "--phase",
        choices=(
            "selfcheck",
            "reference",
            "stability",
            "pressure",
            "capacity",
            "burst",
            "fault",
        ),
        required=True,
    )
    parser.add_argument("--task", choices=stability.TASK_NAMES)
    parser.add_argument("--duration-seconds", type=float)
    parser.add_argument("--endpoint", default="127.0.0.1:8088")
    parser.add_argument("--replay-url", default="http://127.0.0.1:8899/v1/chat/completions")
    parser.add_argument("--gen-dir", type=Path, required=True)
    parser.add_argument("--fleet-manifest", type=Path, required=True)
    parser.add_argument(
        "--artifacts",
        type=Path,
        default=Path("/opt/uenv-stress/runs"),
    )
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
    fleet = load_fleet(
        args.fleet_manifest,
        required["total_slots"],
        allow_overload=args.phase == "pressure",
    )
    required["fleet_capacity"] = fleet["capacity_assessment"]
    disk_budget = verify_disk_budget(config, args, required)
    if args.execute and not args.development_only:
        for field in ("resource_probe_argv", "cleanup_argv", "cleanup_probe_argv"):
            if not isinstance(fleet.get(field), list) or not fleet[field]:
                raise ValueError(f"formal execution fleet manifest requires {field}")
    if (
        args.execute
        and args.phase != "fault"
        and not str(fleet.get("server_log_path") or "").strip()
    ):
        raise ValueError(
            "execution fleet manifest requires server_log_path "
            "for per-Worker and per-Worker×dataset metrics"
        )
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
        "rate_basis": config["load"]["rate_basis"],
        "latency_basis": config["latency_replay"]["latency_basis"],
        "capacity_assessment": fleet["capacity_assessment"],
        "episode_csv_exported": args.export_episode_csv,
        "episode_observations": {
            "schema_version": result_api.EPISODE_OBSERVATION_SCHEMA_VERSION,
            "fields": list(result_api.EPISODE_OBSERVATION_FIELDS),
            "authoritative_artifact": "episode.sqlite",
            "table": "episode",
            "optional_jsonl": "episode-observations.jsonl" if args.export_episode_csv else "",
        },
        "suite_metrics_contract": {
            "schema_version": suite_metrics.SUITE_METRICS_SCHEMA_VERSION,
            "artifact": "suite-metrics.json",
            "status": "planned" if not args.execute else "pending",
        },
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
            "-m",
            "uenv_stress.stability.faults",
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
        completed = subprocess.run(
            command,
            cwd=PACKAGE_ROOT,
            text=True,
            capture_output=True,
        )
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
        try:
            replay_health_path = artifacts / "replay-health.json"
            replay_health = (
                json.loads(replay_health_path.read_text(encoding="utf-8"))
                if replay_health_path.is_file()
                else {"available": False, "error": "replay-health.json missing"}
            )
            expected_worker_ids = [
                str(worker["worker_id"]) for worker in fleet["workers"]
            ]
            worker_load = suite_metrics.parse_worker_load_log(
                Path(str(fleet.get("server_log_path") or "")),
                run_id=args.run_id,
                expected_worker_ids=expected_worker_ids,
            )
            metrics = suite_metrics.build_stability_suite_metrics(
                ledger_path=artifacts / "episode.sqlite",
                run_id=args.run_id,
                phase=args.phase,
                duration_seconds=float(args.duration_seconds),
                parallel_mode=str(config["run"]["parallel_mode"]),
                planned_rates={
                    task: stability.phase_rate(config["tasks"][task], args.phase)
                    for task in stability.TASK_NAMES
                },
                expected_worker_ids=expected_worker_ids,
                worker_load=worker_load,
                replay_health=replay_health,
                resource_csv=artifacts / "resource.csv",
                resource_sample_seconds=float(config["run"]["resource_sample_seconds"]),
                cleanup=manifest["cleanup"],
            )
            metrics_path = artifacts / "suite-metrics.json"
            metrics_path.write_text(
                json.dumps(metrics, indent=2, ensure_ascii=False, sort_keys=True),
                encoding="utf-8",
            )
            manifest["suite_metrics_contract"].update({
                "status": "recorded",
                "artifact": str(metrics_path),
                "complete": bool(metrics["complete"]),
                "data_quality": metrics["data_quality"],
            })
            if not metrics["complete"]:
                return_code = 1
                manifest["status"] = "failed"
                manifest["suite_metrics_error"] = (
                    "required mentor-facing metrics are incomplete"
                )
        except Exception as exc:
            return_code = 1
            manifest["status"] = "failed"
            manifest["suite_metrics_contract"]["status"] = "failed"
            manifest["suite_metrics_error"] = f"{type(exc).__name__}: {exc}"
        (artifacts / "manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True), encoding="utf-8")
    return return_code


if __name__ == "__main__":
    raise SystemExit(main())
