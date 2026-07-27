#!/usr/bin/env python3
"""OlymMATH、SciTab 和 PubMedQA 规则任务规模压测。

这个文件实现面向规则类任务的 1024+ Worker 压力场景。它复用一组真实 Math/Science worker，在三个数据集和多个 parallel mode 下重复投放 Episode，用于验证调度容量、规则 reward/plugin 路径、数据集覆盖和 replay LLM 负载。

实现逻辑是：生成 worker 配置并启动隔离 fleet；为每个数据集读取冻结样本，构造对应 env/reward payload；按配置的 parallel mode、capacity wave、到达率和目标 Episode 数投放任务；LLM 端使用冻结真实轨迹的 replay 服务而不是重新调用模型；运行期间采集 simulator/replay 统计、EpisodeObservation、worker 负载和资源指标；最终生成每个数据集和每种模式的 summary 并清理自有进程。"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import tarfile
import tempfile
import time
import uuid

from uenv_stress.core import distributed_runtime as base
from uenv_stress.scale.dscodebench_pressure import (
    FLEET_SUPERVISOR_SOURCE,
    TCP_PROBE,
    completed_worker_coverage,
    isolated_server_config,
    parse_private_worker_ports,
    put_worker_config_archive,
)


MODES = ("sync", "one_step_off_policy", "fully_async")
TASKS = ("olymmath", "scitab", "pubmedqa")
PACKAGE_DIR = Path(__file__).resolve().parents[1]
COMMON_SOURCE = (PACKAGE_DIR / "core" / "stress_test_common.py").read_text(
    encoding="utf-8-sig"
)
RULE_TASK_SOURCE = (PACKAGE_DIR / "workloads" / "rule_tasks.py").read_text(
    encoding="utf-8"
).replace(
    # rule_tasks.py 会作为扁平文件上传到运行目录（同目录只有 stress_test_common.py），
    # 包路径 import 在那里必然失败，下发前改写为扁平 import。
    "from uenv_stress.core.stress_test_common import rule_reward_config",
    "from stress_test_common import rule_reward_config",
)


TRACE_REPLAY_SERVER = r'''#!/usr/bin/env python3
import argparse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import re
import threading
import time
from urllib.parse import parse_qs, urlparse

parser = argparse.ArgumentParser()
parser.add_argument("--port", type=int, required=True)
parser.add_argument("--seed", type=int, required=True)
parser.add_argument("--trace-sampling-strategy", choices=("round_robin_episode",), required=True)
for task in ("olymmath", "scitab", "pubmedqa"):
    parser.add_argument(f"--{task}-trace", required=True)
args = parser.parse_args()

records = {}
for task in ("olymmath", "scitab", "pubmedqa"):
    path = getattr(args, f"{task}_trace")
    loaded = []
    with open(path, "r", encoding="utf-8") as source:
        for line in source:
            if not line.strip():
                continue
            row = json.loads(line)
            turns = row.get("turns") or []
            if not turns or not str(turns[0].get("assistant_output", "")):
                continue
            if any(
                float(turn.get("replay_wait_ms", 0) or 0) <= 0
                or not str(turn.get("latency_source", ""))
                for turn in turns
            ):
                raise SystemExit(
                    f"{task}: every replay turn requires positive replay_wait_ms "
                    f"and latency_source: {path}"
                )
            loaded.append(row)
    if not loaded:
        raise SystemExit(f"{task}: trace corpus has no replayable rows: {path}")
    records[task] = loaded

lock = threading.Lock()
trace_cursors = {task: 0 for task in records}
stats = {
    task: {
        "hits": 0,
        "misses": 0,
        "calls": 0,
        "latencies_ms": [],
        "trace_selection_counts": {},
    }
    for task in records
}

def content_text(request):
    chunks = []
    for message in request.get("messages", []):
        content = message.get("content", "")
        if isinstance(content, str):
            chunks.append(content)
        elif isinstance(content, list):
            chunks.extend(
                str(item.get("text", ""))
                for item in content
                if isinstance(item, dict)
            )
    return "\n".join(chunks)

def percentile(values, fraction):
    values = sorted(values)
    if not values:
        return 0.0
    return values[min(len(values) - 1, int((len(values) - 1) * fraction))]

class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        parsed = urlparse(self.path)
        if parsed.path != "/stats":
            self.send_error(404)
            return
        requested = parse_qs(parsed.query).get("dataset", [""])[0]
        with lock:
            selected = {requested: stats[requested]} if requested in stats else dict(stats)
            body_doc = {
                "datasets": {
                    task: {
                        "records": len(records[task]),
                        "sampling_strategy": args.trace_sampling_strategy,
                        "assigned_episodes": trace_cursors[task],
                        "completed_cycles": trace_cursors[task] // len(records[task]),
                        "next_trace_slot": trace_cursors[task] % len(records[task]),
                        "trace_selection_counts": dict(value["trace_selection_counts"]),
                        "hits": value["hits"],
                        "misses": value["misses"],
                        "calls": value["calls"],
                        "replay_wait_ms": {
                            "p50": percentile(value["latencies_ms"], 0.50),
                            "p95": percentile(value["latencies_ms"], 0.95),
                            "p99": percentile(value["latencies_ms"], 0.99),
                        },
                    }
                    for task, value in selected.items()
                }
            }
        self.respond(body_doc)

    def do_POST(self):
        size = int(self.headers.get("content-length", "0"))
        request = json.loads(self.rfile.read(size).decode("utf-8") if size else "{}")
        text = content_text(request)
        found = re.search(r"\[UENV_SCALE dataset=(olymmath|scitab|pubmedqa) item_id=([^\]]+)\]", text)
        task = found.group(1) if found else ""
        item_id = found.group(2) if found else ""
        if task not in records:
            with lock:
                for value in stats.values():
                    value["misses"] += 1
            self.respond({"error": {"message": "missing UENV_SCALE dataset marker"}}, status=400)
            return
        with lock:
            selection_ordinal = trace_cursors[task]
            trace_slot = selection_ordinal % len(records[task])
            trace_cursors[task] += 1
            row = records[task][trace_slot]
            trace_id = str(row.get("trace_id") or f"{task}-trace-{trace_slot:08d}")
            usage = stats[task]["trace_selection_counts"]
            usage[trace_id] = usage.get(trace_id, 0) + 1
        turn = row["turns"][0]
        latency = float(turn["replay_wait_ms"])
        time.sleep(latency / 1000.0)
        output = str(turn["assistant_output"])
        token_count = max(1, int(turn.get("target_qwen3_tokens") or 1))
        response_ids = list(range(1, token_count + 1))
        logprobs = [
            {"token": output if index == 0 else "", "token_id": token_id, "logprob": -0.1}
            for index, token_id in enumerate(response_ids)
        ]
        with lock:
            stats[task]["hits"] += 1
            stats[task]["calls"] += 1
            stats[task]["latencies_ms"].append(latency)
        self.respond({
            "choices": [{
                "message": {"role": "assistant", "content": output},
                "finish_reason": "stop",
                "logprobs": {"content": logprobs},
            }],
            "usage": {"prompt_tokens": 8, "completion_tokens": token_count},
            "uenv_response_ids": response_ids,
            "uenv_model_version": {
                "rollout_param_version": 1,
                "rollout_policy_version": f"math-rule-{task}-trace-replay-1",
            },
            "uenv_trace_replay": {
                "dataset": task,
                "trace_id": trace_id,
                "trace_slot": trace_slot,
                "trace_corpus_size": len(records[task]),
                "selection_strategy": args.trace_sampling_strategy,
                "dataset_id": row.get("dataset_id", ""),
            },
        })

    def respond(self, document, status=200):
        body = json.dumps(document).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_args):
        pass

ThreadingHTTPServer(("127.0.0.1", args.port), Handler).serve_forever()
'''


LOAD_CLIENT = r'''#!/usr/bin/env python3
import argparse
import asyncio
import json
import time
import uuid
import grpc
from grpc import aio as grpc_aio
from uenv.v1 import adapter_core_pb2, scheduler_pb2
import stress_test_common as common
import rule_tasks

parser = argparse.ArgumentParser()
parser.add_argument("--server", required=True)
parser.add_argument("--workers", type=int, required=True)
parser.add_argument("--capacity", type=int, required=True)
parser.add_argument("--task", choices=rule_tasks.TASK_NAMES, required=True)
parser.add_argument("--dataset-path", required=True)
parser.add_argument("--dataset-limit", type=int, required=True)
parser.add_argument("--mode", required=True)
parser.add_argument("--model-url", required=True)
parser.add_argument("--run-id", required=True)
parser.add_argument("--output", required=True)
parser.add_argument("--registration-timeout", type=int, required=True)
parser.add_argument("--batch-timeout", type=int, required=True)
parser.add_argument("--exact-batches", type=int, required=True)
parser.add_argument("--episode-batch-size", type=int, required=True)
parser.add_argument("--concurrent-batches", type=int, required=True)
args = parser.parse_args()
rows = rule_tasks.load_task_rows(args.task, args.dataset_path)[:args.dataset_limit]

async def main():
    channel = grpc_aio.insecure_channel(args.server, options=[
        ("grpc.max_receive_message_length", 64 * 1024 * 1024),
        ("grpc.max_send_message_length", 64 * 1024 * 1024),
    ])
    health = channel.unary_unary(
        "/uenv.bridge.v1.AdapterCoreService/HealthCheck",
        request_serializer=lambda value: value.SerializeToString(),
        response_deserializer=adapter_core_pb2.HealthCheckResponse.FromString,
    )
    list_workers = channel.unary_unary(
        "/uenv.scheduler.v1.ControlPlaneService/ListWorkers",
        request_serializer=lambda value: value.SerializeToString(),
        response_deserializer=scheduler_pb2.ListWorkersResponse.FromString,
    )
    execute = channel.unary_unary(
        "/uenv.bridge.v1.AdapterCoreService/ExecuteBatch",
        request_serializer=lambda value: value.SerializeToString(),
        response_deserializer=adapter_core_pb2.ExecuteBatchResponse.FromString,
    )
    deadline = time.monotonic() + args.registration_timeout
    expected_prefix = f"stress-{args.run_id}-worker-"
    while time.monotonic() < deadline:
        try:
            ok = await health(adapter_core_pb2.HealthCheckRequest(), timeout=3)
            response = await list_workers(scheduler_pb2.ListWorkersRequest(), timeout=3)
            owned = [row.worker_id for row in response.workers if row.worker_id.startswith(expected_prefix)]
            if ok.ok and len(owned) == args.workers:
                break
        except grpc.RpcError:
            pass
        await asyncio.sleep(1)
    else:
        raise RuntimeError(f"Math Workers not ready: expected={args.workers}")

    submitted = completed = failed = rpc_errors = protocol_errors = 0
    rewards = []
    latencies = []
    used_items = {}
    missing_ids = []
    episode_observations = []
    lock = asyncio.Lock()
    semaphore = asyncio.Semaphore(args.concurrent_batches)

    async def send_batch(sequence):
        nonlocal submitted, completed, failed, rpc_errors, protocol_errors
        batch_id = str(uuid.uuid4())
        samples = []
        expected = []
        for index in range(args.episode_batch_size):
            ordinal = sequence * args.episode_batch_size + index
            row = rows[ordinal % len(rows)]
            task_id = f"math-rule-{args.task}-{args.run_id}-{args.mode}-{ordinal}"
            env, reward = rule_tasks.build_env_payload(
                args.task, row, index=ordinal % len(rows), task_id=task_id,
                scale_marker=True,
            )
            dataset_item_id = env["dataset_item_id"]
            used_items[dataset_item_id] = used_items.get(dataset_item_id, 0) + 1
            sample = common.make_sample_envelope(
                adapter_core_pb2,
                batch_id=batch_id,
                sample_index=index,
                request_id=task_id,
                env_type="math",
                parallel_mode=args.mode,
                env_config=env,
                reward_config=reward,
                sample_context={
                    "stress_run_id": args.run_id,
                    "dataset": args.task,
                    "dataset_item_id": dataset_item_id,
                    "scale_evidence": True,
                    "sequence": ordinal,
                    "replay_strategy": "round_robin_episode",
                },
                timeout_seconds=args.batch_timeout,
                max_steps=1,
                model_url=args.model_url,
                model_name="math-rule-trace-replay",
            )
            samples.append(sample)
            expected.append(sample.request_id)
        planned_at = time.time()
        async with lock:
            submitted += len(samples)
        started = time.monotonic()
        dispatched_at = time.time()
        try:
            response = await execute(
                adapter_core_pb2.ExecuteBatchRequest(
                    request_id=batch_id, batch_id=batch_id, samples=samples
                ),
                timeout=args.batch_timeout,
            )
            elapsed = (time.monotonic() - started) * 1000
            terminal_at = time.time()
            batch_observations = common.observe_episode_batch(
                samples,
                response.results,
                suite="scale",
                run_id=args.run_id,
                phase=args.mode,
                planned_at=planned_at,
                dispatched_at=dispatched_at,
                terminal_at=terminal_at,
                batch_rpc_latency_ms=elapsed,
            )
            received = {}
            for result in response.results:
                received[result.request_id] = received.get(result.request_id, 0) + 1
            missing = sorted(set(expected) - set(received))
            duplicates = [key for key, count in received.items() if count > 1]
            unknown = sorted(set(received) - set(expected))
            async with lock:
                latencies.append(elapsed)
                episode_observations.extend(batch_observations)
                missing_ids.extend(missing)
                failed += len(missing) + len(duplicates)
                protocol_errors += len(missing) + len(duplicates) + len(unknown)
                for result in response.results:
                    rewards.append(float(result.reward))
                    if result.status in {"completed", "success"}:
                        completed += 1
                        if args.mode != "sync":
                            parsed = common.sample_result_dict(result)
                            if not parsed["training_trace_valid"]:
                                protocol_errors += 1
                    else:
                        failed += 1
        except grpc.RpcError as exc:
            terminal_at = time.time()
            batch_observations = common.observe_episode_batch(
                samples,
                [],
                suite="scale",
                run_id=args.run_id,
                phase=args.mode,
                planned_at=planned_at,
                dispatched_at=dispatched_at,
                terminal_at=terminal_at,
                batch_rpc_latency_ms=(time.monotonic() - started) * 1000,
                rpc_error_code=exc.code().name if exc.code() else "UNKNOWN",
                rpc_error_message=exc.details() or "",
            )
            async with lock:
                rpc_errors += len(samples)
                missing_ids.extend(expected)
                episode_observations.extend(batch_observations)
        finally:
            semaphore.release()

    started = time.monotonic()
    tasks = []
    for sequence in range(args.exact_batches):
        await semaphore.acquire()
        tasks.append(asyncio.create_task(send_batch(sequence)))
    if tasks:
        await asyncio.gather(*tasks)
    elapsed = time.monotonic() - started
    result = common.stress_result_document(
        run_id=args.run_id,
        environment=f"math:{args.task}",
        parallel_mode=args.mode,
        elapsed_seconds=elapsed,
        registered_workers=args.workers,
        configured_workers=args.workers,
        worker_capacity=args.capacity,
        batch_size=args.episode_batch_size,
        concurrent_batches=args.concurrent_batches,
        openhands_agents=0,
        openhands_agent_capacity=0,
        submitted=submitted,
        completed=completed,
        failed=failed,
        rpc_error_episodes=rpc_errors,
        rpc_error_batches=0,
        protocol_errors=protocol_errors,
        latencies_ms=latencies,
        rewards=rewards,
    )
    result.update({
        "dataset": {
            "name": args.task,
            "path": args.dataset_path,
            "loaded_rows": len(rows),
            "unique_items": len(used_items),
            "reuse_factor": submitted / max(1, len(used_items)),
            "real_input": True,
        },
        "batch_size": args.episode_batch_size,
        "concurrent_batches": args.concurrent_batches,
        "planned_batches": args.exact_batches,
        "capacity_waves": submitted / max(1, args.workers * args.capacity),
        "missing_result_ids": sorted(set(missing_ids)),
        "infrastructure": {
            "passed": bool(submitted and completed == submitted and not failed and not rpc_errors and not protocol_errors)
        },
        "model_quality": {
            "observation_only": True,
            "average_reward": sum(rewards) / len(rewards) if rewards else 0.0,
            "note": "Trace replay scale evidence is not new model-quality evidence.",
        },
    })
    observation_path = args.output + ".episode-observations.jsonl"
    observation_count = common.write_episode_observations_jsonl(
        observation_path, episode_observations
    )
    result["episode_observations"] = {
        "schema_version": common.EPISODE_OBSERVATION_SCHEMA_VERSION,
        "artifact_path": observation_path,
        "row_count": observation_count,
        "submitted_count": submitted,
        "complete": observation_count == submitted,
        "worker_attribution": "unavailable_in_adapter_result",
    }
    with open(args.output, "w", encoding="utf-8") as target:
        json.dump(result, target, indent=2, sort_keys=True)
    await channel.close()
    if not result["infrastructure"]["passed"]:
        raise SystemExit(1)

asyncio.run(main())
'''


def worker_config(
    run_dir: str,
    worker_id: str,
    port: int,
    obs_port: int,
    capacity: int,
    private_ip: str,
) -> str:
    return f'''server:
  endpoint: "{base.SERVER_PRIVATE_IP}:{base.SERVER_PORT}"
worker:
  id: "{worker_id}"
  listen: "0.0.0.0:{port}"
  advertise_endpoint: "{private_ip}:{port}"
  max_concurrent: {capacity}
scheduler:
  mode: "remote"
env:
  types: ["math"]
  backend: "process"
  plugin_dir: "{run_dir}/bundle/plugins"
pool:
  warmup_size: 0
  prewarm_on_startup: false
  max_idle_time: 600
  cool_timeout: 60
  max_episode_count: 100000
logging:
  level: "error"
  file: "{run_dir}/logs/{worker_id}.runtime.log"
wal:
  dir: "{run_dir}/wal/{worker_id}"
observability:
  metrics_listen: "127.0.0.1:{obs_port}"
  health_listen: "127.0.0.1:{obs_port}"
hub:
  enabled: false
'''


def _sha256_remote(client, path: str) -> str:
    _, output, _ = base.run(client, f"sha256sum {base.q(path)}")
    return output.split()[0]


def _simulator_stats(worker_clients: dict, task: str) -> dict:
    combined = {
        "records": 0,
        "hits": 0,
        "misses": 0,
        "calls": 0,
        "assigned_episodes": 0,
        "completed_cycles": 0,
        "sampling_strategy": "round_robin_episode",
        "per_node": {},
    }
    for host, client in worker_clients.items():
        _, output, _ = base.run(
            client,
            f"python3 -c \"import urllib.request; print(urllib.request.urlopen("
            f"'http://127.0.0.1:{base.MODEL_PORT}/stats?dataset={task}', timeout=10).read().decode())\"",
        )
        value = json.loads(output)[
            "datasets"
        ][task]
        combined["per_node"][host] = value
        for field in (
            "records", "hits", "misses", "calls",
            "assigned_episodes", "completed_cycles",
        ):
            combined[field] += int(value.get(field, 0))
    combined["passed"] = (
        combined["records"] > 0
        and combined["hits"] == combined["calls"]
        and combined["misses"] == 0
    )
    return combined


def run_scale(args: argparse.Namespace) -> dict:
    workers_count = args.workers
    run_id = (
        f"math-rule-pressure-{workers_count}x{args.capacity}-"
        f"{time.strftime('%Y%m%d-%H%M%S')}-{uuid.uuid4().hex[:6]}"
    )
    server_run = f"/tmp/uenv-{run_id}"
    worker_run = f"/opt/uenv-stress/runs/{run_id}"
    local_run = args.artifacts / run_id
    local_run.mkdir(parents=True)
    nodes = list(base.WORKER_NODES)
    assignments = base.worker_assignments(workers_count)
    indexes_by_host = {
        node.host: indexes for node, indexes in zip(nodes, assignments)
    }
    ports = parse_private_worker_ports(args.private_worker_port_range, workers_count)
    obs_ports = [base.OBS_PORT + index for index in range(workers_count)]
    server = None
    worker_clients = {}
    server_pid = None
    model_pids = {node.host: None for node in nodes}
    supervisors = {node.host: None for node in nodes}
    worker_pids = {node.host: [] for node in nodes}
    fleet_metrics_paths = {node.host: "" for node in nodes}
    before = None
    cleanup_errors = []
    results = []
    error = None
    outcome = None
    try:
        server = base.connect(base.SERVER_HOST, args.password)
        worker_clients = base.connect_worker_nodes(args.password)
        before = base.protected_snapshot(server)
        base.assert_port_free(server, base.SERVER_PORT, base.SERVER_HOST)
        for node in nodes:
            indexes = indexes_by_host[node.host]
            base.assert_ports_free(
                worker_clients[node.host],
                [ports[index] for index in indexes]
                + [obs_ports[index] for index in indexes]
                + [base.MODEL_PORT],
                node.host,
            )
        build = base.source_and_binary_manifest(server, include_code_plugin=False)
        base.run(server, f"test -x {base.q(base.SOURCE_CODE_BIN)}")
        build["binaries"]["math_plugin"] = {
            "path": base.SOURCE_CODE_BIN,
            "sha256": _sha256_remote(server, base.SOURCE_CODE_BIN),
        }
        dataset_hashes = {}
        for task in TASKS:
            path = args.task_config[task]["dataset_path"]
            base.run(server, f"test -e {base.q(path)}")
            if task == "olymmath":
                _, listing, _ = base.run(
                    server,
                    f"find {base.q(path)} -maxdepth 1 -name 'OlymMATH-*.jsonl' -type f -print0 "
                    "| sort -z | xargs -0 sha256sum",
                )
                dataset_hashes[task] = hashlib.sha256(listing.encode()).hexdigest()
            else:
                dataset_hashes[task] = _sha256_remote(server, path)
        base.run(
            server,
            f"install -d -m 0755 {base.q(server_run)}/bundle/plugins/math "
            f"{base.q(server_run)}/generated/uenv/v1",
        )
        for node in nodes:
            base.run(
                worker_clients[node.host],
                f"install -d -m 0755 {base.q(worker_run)}/logs "
                f"{base.q(worker_run)}/wal {base.q(worker_run)}/traces",
            )
            for task in TASKS:
                trace = args.task_config[task]["trace_corpus_path"]
                base.run(worker_clients[node.host], f"test -f {base.q(trace)}")
        base.run(
            server,
            " && ".join([
                f"install -m 0755 {base.q(base.SOURCE_WORKER_BIN)} {base.q(server_run)}/bundle/uenv-worker",
                f"install -m 0755 {base.q(base.SOURCE_CODE_BIN)} {base.q(server_run)}/bundle/uenv-math-plugin",
                f"cp -a {base.q(base.SOURCE_REPO)}/plugins/math/. {base.q(server_run)}/bundle/plugins/math/",
                f"tar -C {base.q(server_run)}/bundle -czf {base.q(server_run)}/bundle.tgz .",
            ]),
            timeout=180,
        )
        with tempfile.NamedTemporaryFile(prefix=run_id, suffix=".tgz", delete=False) as temp:
            local_bundle = Path(temp.name)
        try:
            with server.open_sftp() as sftp:
                sftp.get(f"{server_run}/bundle.tgz", str(local_bundle))
            for node in nodes:
                client = worker_clients[node.host]
                with client.open_sftp() as sftp:
                    sftp.put(str(local_bundle), f"{worker_run}/bundle.tgz")
                base.run(
                    client,
                    f"install -d -m 0755 {base.q(worker_run)}/bundle && "
                    f"tar -C {base.q(worker_run)}/bundle -xzf {base.q(worker_run)}/bundle.tgz",
                )
        finally:
            local_bundle.unlink(missing_ok=True)
        base.put_text(
            server,
            f"{server_run}/server.yaml",
            isolated_server_config(workers_count, args.capacity),
        )
        base.put_text(server, f"{server_run}/load_client.py", LOAD_CLIENT, 0o755)
        base.put_text(server, f"{server_run}/stress_test_common.py", COMMON_SOURCE)
        base.put_text(server, f"{server_run}/rule_tasks.py", RULE_TASK_SOURCE)
        base.put_text(server, f"{server_run}/tcp_probe.py", TCP_PROBE, 0o755)
        for node in nodes:
            client = worker_clients[node.host]
            base.put_text(client, f"{worker_run}/trace_replay_server.py", TRACE_REPLAY_SERVER, 0o755)
            base.put_text(client, f"{worker_run}/worker_fleet_supervisor.py", FLEET_SUPERVISOR_SOURCE, 0o700)
            documents = {}
            for index in indexes_by_host[node.host]:
                worker_id = f"stress-{run_id}-worker-{index:04d}"
                documents[f"{worker_run}/worker-{index:04d}.yaml"] = (
                    worker_config(
                        worker_run, worker_id, ports[index], obs_ports[index],
                        args.capacity, node.private_ip,
                    ),
                    0o600,
                )
            put_worker_config_archive(client, worker_run, documents, run_id)
        proto_root = f"{base.SOURCE_REPO}/proto"
        base.run(
            server,
            " ".join([
                "/usr/bin/protoc", "-I", base.q(proto_root),
                f"--python_out={base.q(server_run)}/generated",
                f"{base.q(proto_root)}/uenv/v1/common.proto",
                f"{base.q(proto_root)}/uenv/v1/episode.proto",
                f"{base.q(proto_root)}/uenv/v1/scheduler.proto",
                f"{base.q(proto_root)}/uenv/v1/adapter_core.proto",
            ]),
        )
        base.run(
            server,
            f"touch {base.q(server_run)}/generated/uenv/__init__.py "
            f"{base.q(server_run)}/generated/uenv/v1/__init__.py",
        )
        server_cmd = " ".join([
            "env", "UENV_SERVER_CONFIG_STRICT=1", "UENV_TRAJECTORY_ENABLED=0",
            "UENV_OBS_ENABLED=0", "UENV_LOG_ANSI=0",
            f"UENV_ADDR={base.SERVER_PRIVATE_IP}:{base.SERVER_PORT}",
            f"UENV_CONFIG_PATH={server_run}/server.yaml",
            "RUST_LOG=info", base.SERVER_BIN,
        ])
        server_pid = base.start_owned(
            server, server_cmd, f"{server_run}/server.log",
            base.SERVER_BIN, base.SERVER_BIN,
        )
        model_command = " ".join([
            "python3", "-B", f"{worker_run}/trace_replay_server.py",
            "--port", str(base.MODEL_PORT),
            "--seed", str(args.simulator_seed),
            "--trace-sampling-strategy", args.trace_sampling_strategy,
            *sum(
                (
                    [f"--{task}-trace", args.task_config[task]["trace_corpus_path"]]
                    for task in TASKS
                ),
                [],
            ),
        ])
        for node in nodes:
            client = worker_clients[node.host]
            model_pids[node.host] = base.start_owned(
                client, model_command, f"{worker_run}/model.log",
                "/usr/bin/python3.12", f"{worker_run}/trace_replay_server.py",
            )
        worker_env = {
            "UENV_SERVER_CONFIG_STRICT": "1",
            "UENV_TRAJECTORY_ENABLED": "0",
            "UENV_OBS_ENABLED": "0",
            "UENV_LOG_ANSI": "0",
            "UENV_WORKER_EPISODE_TIMEOUT_SECS": str(args.batch_timeout),
            "UENV_LLM_HTTP_TIMEOUT_SECS": str(args.batch_timeout),
            "UENV_MATH_PLUGIN_BIN": f"{worker_run}/bundle/uenv-math-plugin",
            "UENV_PLUGIN_READY_TIMEOUT_SECS": str(args.plugin_ready_timeout),
            "UENV_WORKER_REGISTER_MAX_ATTEMPTS": str(args.register_attempts),
            "UENV_WORKER_REGISTER_RETRY_BACKOFF_MS": str(args.register_backoff_ms),
            "RUST_LOG": "error",
        }
        for node in nodes:
            client = worker_clients[node.host]
            indexes = indexes_by_host[node.host]
            fleet_spec = {
                "workers": [
                    {
                        "worker_id": f"stress-{run_id}-worker-{index:04d}",
                        "config": f"{worker_run}/worker-{index:04d}.yaml",
                        "argv": [
                            f"{worker_run}/bundle/uenv-worker", "--config",
                            f"{worker_run}/worker-{index:04d}.yaml", "serve",
                        ],
                        "env": worker_env,
                        "log": f"{worker_run}/logs/worker-{index:04d}.log",
                    }
                    for index in indexes
                ]
            }
            spec_path = f"{worker_run}/fleet.json"
            pid_path = f"{worker_run}/fleet-pids.json"
            metrics_path = f"{worker_run}/fleet-metrics.json"
            base.put_text(client, spec_path, json.dumps(fleet_spec, sort_keys=True), 0o600)
            supervisors[node.host] = base.start_owned(
                client,
                f"python3 -B {worker_run}/worker_fleet_supervisor.py "
                f"--spec {spec_path} --pid-file {pid_path} --metrics-file {metrics_path}",
                f"{worker_run}/fleet-supervisor.log",
                "/usr/bin/python3.12",
                f"{worker_run}/worker_fleet_supervisor.py",
            )
            fleet_metrics_paths[node.host] = metrics_path
            deadline = time.monotonic() + max(60, workers_count // 2)
            while time.monotonic() < deadline:
                status, _, _ = base.run(client, f"test -s {base.q(pid_path)}", check=False)
                if status == 0:
                    fleet = json.loads(base.get_text(client, pid_path))
                    worker_pids[node.host] = [int(row["pid"]) for row in fleet["workers"]]
                    break
                time.sleep(0.5)
            else:
                raise RuntimeError(f"{node.host}: Math Worker fleet did not publish PID manifest")
        for node in nodes:
            indexes = indexes_by_host[node.host]
            for port in sorted({ports[indexes[0]], ports[indexes[-1]]}):
                base.run(
                    server,
                    f"python3 -B {server_run}/tcp_probe.py {node.private_ip} {port} "
                    f"--wait-seconds {args.registration_timeout}",
                    timeout=args.registration_timeout + 15,
                )
        base.assert_protected_unchanged(server, before)
        manifest = {
            "run_id": run_id,
            "environment": "math",
            "datasets": list(TASKS),
            "real_workers": workers_count,
            "worker_capacity": args.capacity,
            "modes": list(args.modes),
            "episodes_per_dataset_mode": args.episode_batch_size * args.exact_batches,
            "minimum_capacity_waves": args.min_waves,
            "dataset_sha256": dataset_hashes,
            "trace_corpora": {
                task: args.task_config[task]["trace_corpus_path"] for task in TASKS
            },
            "trace_sampling_strategy": args.trace_sampling_strategy,
            "source_and_binaries": build,
            "worker_nodes": [
                {"host": node.host, "private_ip": node.private_ip} for node in nodes
            ],
            "protected_server": before,
            "evidence_boundary": args.evidence_boundary,
        }
        (local_run / "manifest.json").write_text(
            json.dumps(manifest, indent=2, sort_keys=True), encoding="utf-8"
        )
        for task in TASKS:
            task_config = args.task_config[task]
            for mode in args.modes:
                remote_result = f"{server_run}/result-{task}-{mode}.json"
                command = " ".join([
                    f"PYTHONPATH={server_run}:{server_run}/generated",
                    "python3", "-B", f"{server_run}/load_client.py",
                    "--server", f"{base.SERVER_PRIVATE_IP}:{base.SERVER_PORT}",
                    "--workers", str(workers_count),
                    "--capacity", str(args.capacity),
                    "--task", task,
                    "--dataset-path", base.q(task_config["dataset_path"]),
                    "--dataset-limit", str(task_config["dataset_limit"]),
                    "--mode", mode,
                    "--model-url", f"http://127.0.0.1:{base.MODEL_PORT}/v1",
                    "--run-id", run_id,
                    "--output", remote_result,
                    "--registration-timeout", str(args.registration_timeout),
                    "--batch-timeout", str(args.batch_timeout),
                    "--exact-batches", str(args.exact_batches),
                    "--episode-batch-size", str(args.episode_batch_size),
                    "--concurrent-batches", str(args.concurrent_batches),
                ])
                base.run(
                    server, command,
                    timeout=args.registration_timeout + args.batch_timeout + 240,
                )
                result = json.loads(base.get_text(server, remote_result))
                local_observations = (
                    local_run / f"episode-observations-{task}-{mode}.jsonl"
                )
                local_observations.write_text(
                    base.get_text(
                        server, remote_result + ".episode-observations.jsonl"
                    ),
                    encoding="utf-8",
                )
                result["episode_observations"]["local_artifact"] = str(
                    local_observations
                )
                replay = _simulator_stats(worker_clients, task)
                result["trace_replay"] = replay
                coverage = completed_worker_coverage(
                    server, f"{server_run}/server.log",
                    f"stress-{run_id}-worker-",
                    f"math-rule-{task}-{run_id}-{mode}-",
                )
                coverage["expected_workers"] = workers_count
                coverage["passed"] = coverage["unique_completed_workers"] == workers_count
                result["worker_dispatch_coverage"] = coverage
                result["status"] = (
                    "passed"
                    if result["infrastructure"]["passed"]
                    and replay["passed"]
                    and coverage["passed"]
                    and result["capacity_waves"] >= args.min_waves
                    else "failed"
                )
                results.append(result)
                (local_run / f"result-{task}-{mode}.json").write_text(
                    json.dumps(result, indent=2, sort_keys=True), encoding="utf-8"
                )
                if result["status"] != "passed":
                    raise RuntimeError(f"{task}/{mode} scale evidence failed")
        fleet_metrics = {
            node.host: json.loads(
                base.get_text(worker_clients[node.host], fleet_metrics_paths[node.host])
            )
            for node in nodes
        }
        outcome = {
            "run_id": run_id,
            "scale": f"{workers_count}x{args.capacity}",
            "datasets": list(TASKS),
            "results": results,
            "fleet_resource_metrics": {"per_node": fleet_metrics},
            "status": "passed",
        }
    except Exception as exc:
        error = f"{type(exc).__name__}: {exc}"
        (local_run / "error.txt").write_text(error, encoding="utf-8")
        outcome = {
            "run_id": run_id,
            "scale": f"{workers_count}x{args.capacity}",
            "datasets": list(TASKS),
            "status": "failed",
            "error": error,
        }
    finally:
        for client in worker_clients.values():
            client.close()
        if server:
            server.close()
        worker_clients = {}
        server = None
        for node in nodes:
            try:
                worker_clients[node.host] = base.connect(node.host, args.password)
            except Exception as exc:
                cleanup_errors.append(f"{node.host} reconnect: {exc}")
        try:
            server = base.connect(base.SERVER_HOST, args.password)
        except Exception as exc:
            cleanup_errors.append(f"server reconnect: {exc}")
        for node in nodes:
            client = worker_clients.get(node.host)
            if not client:
                continue
            try:
                base.stop_owned(
                    client, supervisors[node.host], "/usr/bin/python3.12",
                    f"{worker_run}/worker_fleet_supervisor.py",
                )
            except Exception as exc:
                cleanup_errors.append(str(exc))
            try:
                base.stop_owned(
                    client, model_pids[node.host], "/usr/bin/python3.12",
                    f"{worker_run}/trace_replay_server.py",
                )
            except Exception as exc:
                cleanup_errors.append(str(exc))
            try:
                indexes = indexes_by_host[node.host]
                base.assert_ports_free(
                    client,
                    [ports[index] for index in indexes]
                    + [obs_ports[index] for index in indexes]
                    + [base.MODEL_PORT],
                    node.host,
                )
            except Exception as exc:
                cleanup_errors.append(str(exc))
        if server:
            try:
                base.stop_owned(server, server_pid, base.SERVER_BIN, base.SERVER_BIN)
            except Exception as exc:
                cleanup_errors.append(str(exc))
            try:
                if before:
                    base.assert_protected_unchanged(server, before)
                base.assert_port_free(server, base.SERVER_PORT, base.SERVER_HOST)
            except Exception as exc:
                cleanup_errors.append(str(exc))
        for client in (*worker_clients.values(), server):
            if client:
                client.close()
        if cleanup_errors:
            (local_run / "cleanup-errors.txt").write_text(
                "\n".join(cleanup_errors), encoding="utf-8"
            )
            outcome = {
                "run_id": run_id,
                "scale": f"{workers_count}x{args.capacity}",
                "datasets": list(TASKS),
                "status": "failed",
                "error": "cleanup failed: " + " | ".join(cleanup_errors),
            }
    assert outcome is not None
    outcome["cleanup"] = {
        "attempted": True,
        "passed": not cleanup_errors,
        "errors": cleanup_errors,
        "protected_process_unchanged": not cleanup_errors,
        "isolated_ports_released": not cleanup_errors,
    }
    return outcome


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--workers", type=int, required=True)
    parser.add_argument("--capacity", type=int, required=True)
    parser.add_argument("--private-worker-port-range", required=True)
    parser.add_argument("--mode", action="append", choices=MODES)
    parser.add_argument("--episode-batch-size", type=int, required=True)
    parser.add_argument("--concurrent-batches", type=int, required=True)
    parser.add_argument("--exact-batches", type=int, required=True)
    parser.add_argument("--min-scale-episode-waves", type=int, required=True)
    parser.add_argument("--registration-timeout", type=int, required=True)
    parser.add_argument("--batch-timeout", type=int, required=True)
    parser.add_argument("--plugin-ready-timeout-seconds", type=int, required=True)
    parser.add_argument("--worker-register-max-attempts", type=int, required=True)
    parser.add_argument("--worker-register-retry-backoff-ms", type=int, required=True)
    parser.add_argument("--simulator-seed", type=int, required=True)
    parser.add_argument(
        "--trace-sampling-strategy",
        choices=("round_robin_episode",),
        required=True,
    )
    parser.add_argument("--evidence-boundary", required=True)
    parser.add_argument("--artifacts", type=Path, required=True)
    for task in TASKS:
        parser.add_argument(f"--{task}-dataset", required=True)
        parser.add_argument(f"--{task}-dataset-limit", type=int, required=True)
        parser.add_argument(f"--{task}-trace", required=True)
    base.add_runtime_arguments(parser, require_code_plugin=True)
    args = parser.parse_args()
    base.configure_from_args(args)
    args.password = os.environ.get("UENV_PASS")
    if not args.password:
        parser.error("UENV_PASS is required")
    args.modes = tuple(args.mode or MODES)
    args.min_waves = args.min_scale_episode_waves
    args.plugin_ready_timeout = args.plugin_ready_timeout_seconds
    args.register_attempts = args.worker_register_max_attempts
    args.register_backoff_ms = args.worker_register_retry_backoff_ms
    args.task_config = {
        task: {
            "dataset_path": getattr(args, f"{task}_dataset"),
            "dataset_limit": getattr(args, f"{task}_dataset_limit"),
            "trace_corpus_path": getattr(args, f"{task}_trace"),
        }
        for task in TASKS
    }
    if args.workers < 1024:
        parser.error("formal Math rule scale evidence requires at least 1024 Workers")
    required = args.workers * args.capacity * args.min_waves
    if args.episode_batch_size * args.exact_batches < required:
        parser.error(
            f"every dataset/mode requires at least {required} episodes"
        )
    if args.concurrent_batches < args.exact_batches:
        parser.error(
            "--concurrent-batches must be >= --exact-batches for backlog submission"
        )
    args.artifacts.mkdir(parents=True, exist_ok=True)
    summary = run_scale(args)
    summary_path = (
        args.artifacts
        / f"math-rule-pressure-summary-{time.strftime('%Y%m%d-%H%M%S')}.json"
    )
    summary_path.write_text(
        json.dumps([summary], indent=2, sort_keys=True), encoding="utf-8"
    )
    print(f"[math_rule_pressure] status={summary['status']} summary={summary_path}")
    return 0 if summary["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
