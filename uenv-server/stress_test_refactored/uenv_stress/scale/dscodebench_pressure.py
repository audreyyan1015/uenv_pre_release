#!/usr/bin/env python3
"""分布式 DSCodeBench Code worker 压测。

这个文件实现真实 Code worker 的多轮与扩容压力测试。它用于验证 UEnv Server 调度、Code worker 执行、模型模拟器/replay、Episode 结果记录、worker 覆盖和资源采样在 DSCodeBench 代码任务下是否稳定。

实现逻辑是：先把 server 配置、worker 配置和必要的公共代码打包发送到各 worker 主机；在 server 主机启动隔离 server，在 worker 主机按 round-robin 启动 Code worker 和本地模型模拟服务；run_scale 根据目标 worker 数、容量波次、任务样本和并发参数投放 Episode；模型模拟器按任务 ID 生成确定性延迟、token 和结果；脚本持续解析日志、统计每个 worker 的完成覆盖、模型 step 时延、资源采样和错误分布；结束时写 summary、EpisodeObservation、worker coverage、资源和清理证据，并只停止本次 run_id 拥有的进程。"""

from __future__ import annotations

import argparse
import io
import json
import os
from pathlib import Path
import re
import tarfile
import tempfile
import time
import uuid

from uenv_stress.core import distributed_runtime as base


# 三种并行模式都要测，避免只验证同步路径。
MODES = ("sync", "one_step_off_policy", "fully_async")

# load_client.py 会被写入远端 /tmp/uenv-<run_id>/ 运行目录。
# 它需要导入 stress_test_common.py，所以这里提前读取文件内容，后面一并下发。
PACKAGE_DIR = Path(__file__).resolve().parents[1]
COMMON_SOURCE = (PACKAGE_DIR / "core" / "stress_test_common.py").read_text(
    encoding="utf-8-sig"
)
REAL_LLM_PROXY_SOURCE = (PACKAGE_DIR / "providers" / "ark" / "proxy.py").read_text(
    encoding="utf-8"
)
REAL_LLM_PREFLIGHT_SOURCE = (
    PACKAGE_DIR / "providers" / "ark" / "preflight.py"
).read_text(encoding="utf-8")
FLEET_SUPERVISOR_SOURCE = (
    PACKAGE_DIR / "core" / "fleet_supervisor.py"
).read_text(encoding="utf-8")


def put_worker_config_archive(worker, worker_run: str, documents: dict[str, tuple[str, int]], run_id: str) -> None:
    """Upload many Worker YAMLs as one tarball.

    At 1024+ workers, one SFTP open/write/chmod per YAML can dominate startup
    before any pressure is applied to UEnv itself.  The archive keeps the exact
    per-file content and mode while turning those small writes into one upload.
    """
    with tempfile.NamedTemporaryFile(prefix=f"{run_id}-worker-configs-", suffix=".tgz", delete=False) as tmp:
        local_archive = Path(tmp.name)
    try:
        with tarfile.open(local_archive, "w:gz") as tar:
            for path, (content, mode) in documents.items():
                data = content.encode("utf-8")
                info = tarfile.TarInfo(name=Path(path).name)
                info.size = len(data)
                info.mode = mode
                info.mtime = int(time.time())
                tar.addfile(info, io.BytesIO(data))
        remote_archive = f"{worker_run}/worker-configs.tgz"
        with worker.open_sftp() as sftp:
            sftp.put(str(local_archive), remote_archive)
        base.run(worker, f"tar -C {base.q(worker_run)} -xzf {base.q(remote_archive)}", timeout=120)
    finally:
        local_archive.unlink(missing_ok=True)


MODEL_SIMULATOR = r'''#!/usr/bin/env python3
# 这个脚本会临时写到 worker 机器上运行。
# 它提供一个最小 OpenAI-compatible HTTP 接口。trace_replay 模式下回复和等待
# 时间都必须来自冻结真实轨迹；如果轨迹缺失则直接返回错误，不再走模拟兜底。
import argparse
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import math
import os
from pathlib import Path
import random
import re
import threading
import time
from urllib.parse import parse_qs, urlparse

parser = argparse.ArgumentParser()
parser.add_argument("--port", type=int, required=True)
parser.add_argument("--zero-latency", action="store_true")
parser.add_argument("--seed", type=int, default=20260720)
parser.add_argument("--dataset-jsonl", default="")
parser.add_argument("--simulator-mode", choices=("template", "trace_replay"), default="template")
parser.add_argument("--trace-corpus-path", default="")
parser.add_argument(
    "--trace-sampling-strategy",
    choices=("round_robin_episode",),
    default="round_robin_episode",
)
args = parser.parse_args()
attempts_by_task = {}
attempts_lock = threading.Lock()
dataset_oracle = {}
trace_records = []
trace_episodes_by_id = {}
trace_episodes = []
trace_assignment_by_task = {}
trace_cursor = 0
trace_selection_counts = {}
trace_exhausted_turn_reuses = 0
trace_hits = 0
trace_misses = 0
trace_hits_by_task = {}
trace_misses_by_task = {}
observed_latencies_ms = []
if args.dataset_jsonl:
    with open(args.dataset_jsonl, "r", encoding="utf-8") as source:
        for line in source:
            if line.strip():
                row = json.loads(line)
                dataset_oracle[str(row["problem_id"])] = str(row["ground_truth_code"])

def expand_trace_document(document):
    if not isinstance(document, dict):
        return
    turns = document.get("turns")
    has_direct_output = any(
        document.get(field) is not None
        for field in ("assistant_output", "response_text", "content", "response")
    )
    if isinstance(turns, list) and not has_direct_output:
        parent_fields = {
            field: document.get(field)
            for field in (
                "trace_id", "episode_id", "task_id", "request_id",
                "dataset_problem_id", "problem_id", "benchmark", "dataset",
                "source", "schema_version",
            )
            if document.get(field) is not None
        }
        for turn in turns:
            if isinstance(turn, dict):
                expanded = dict(parent_fields)
                expanded.update(turn)
                yield expanded
        return
    yield document

def iter_json_records(path):
    if not path:
        return
    source_path = Path(path)
    candidates = []
    if source_path.is_dir():
        candidates.extend(sorted(source_path.rglob("dscodebench_trace_corpus.jsonl")))
        candidates.extend(sorted(source_path.rglob("*.jsonl")))
        candidates.extend(sorted(source_path.rglob("*.json")))
    else:
        candidates.append(source_path)
    seen = set()
    for candidate in candidates:
        key = str(candidate)
        if key in seen or not candidate.exists():
            continue
        seen.add(key)
        if candidate.suffix.lower() == ".jsonl":
            with candidate.open("r", encoding="utf-8") as source:
                for line in source:
                    if line.strip():
                        yield from expand_trace_document(json.loads(line))
        elif candidate.suffix.lower() == ".json":
            document = json.loads(candidate.read_text(encoding="utf-8"))
            if isinstance(document, list):
                for item in document:
                    if isinstance(item, dict):
                        yield from expand_trace_document(item)
            elif isinstance(document, dict):
                records = document.get("records") or document.get("turns") or document.get("trace")
                if isinstance(records, list):
                    for item in records:
                        if isinstance(item, dict):
                            yield from expand_trace_document(item)
                else:
                    yield from expand_trace_document(document)

def normalize_response_ids(values):
    out = []
    for value in values or []:
        if isinstance(value, int):
            out.append(value)
        elif isinstance(value, str) and value.lstrip("-").isdigit():
            out.append(int(value))
    return out

def normalize_trace_record(raw, episode=None):
    if not isinstance(raw, dict):
        return None
    episode = episode if isinstance(episode, dict) else {}
    output = raw.get("assistant_output")
    if output is None:
        output = raw.get("response_text") or raw.get("content")
    if output is None and isinstance(raw.get("response"), dict):
        choices = raw["response"].get("choices") or []
        output = ((choices[0].get("message") or {}).get("content") if choices else "")
    output = output if isinstance(output, str) else json.dumps(output, ensure_ascii=False)
    if not output:
        return None
    response_ids = normalize_response_ids(raw.get("response_ids") or raw.get("uenv_response_ids"))
    logprobs = raw.get("logprobs") if isinstance(raw.get("logprobs"), list) else []
    if not response_ids and logprobs:
        response_ids = normalize_response_ids([item.get("token_id") for item in logprobs if isinstance(item, dict)])
    try:
        turn_index = int(raw.get("turn_index", raw.get("attempt", 1)))
    except Exception:
        turn_index = 1
    trace_id = str(
        raw.get("trace_id")
        or episode.get("trace_id")
        or raw.get("episode_id")
        or episode.get("episode_id")
        or raw.get("task_id")
        or raw.get("request_id")
        or raw.get("provider_response_id")
        or ""
    )
    return {
        "schema_version": int(raw.get("schema_version", 1) or 1),
        "benchmark": str(raw.get("benchmark") or raw.get("dataset") or "DSCodeBench"),
        "task_id": str(raw.get("task_id", "")),
        "trace_id": trace_id,
        "dataset_problem_id": str(raw.get("dataset_problem_id") or raw.get("problem_id") or ""),
        "turn_index": max(1, turn_index),
        "assistant_output": output,
        "response_ids": response_ids,
        "logprobs": logprobs,
        "replay_wait_ms": float(raw.get("replay_wait_ms", 0) or 0),
        "latency_source": str(raw.get("latency_source", "")),
        "provider_response_id": str(raw.get("provider_response_id", "")),
        "source": str(raw.get("source", "")),
    }

if args.trace_corpus_path:
    for raw_record in iter_json_records(args.trace_corpus_path):
        raw_turns = raw_record.get("turns") if isinstance(raw_record, dict) else None
        candidates = raw_turns if isinstance(raw_turns, list) else [raw_record]
        for raw_turn in candidates:
            record = normalize_trace_record(raw_turn, raw_record)
            if not record:
                continue
            if not record["trace_id"]:
                record["trace_id"] = f"record-{len(trace_records):08d}"
            trace_records.append(record)
            trace_episodes_by_id.setdefault(record["trace_id"], []).append(record)

for trace_id, turns in trace_episodes_by_id.items():
    trace_episodes.append({
        "trace_id": trace_id,
        "turns": sorted(turns, key=lambda item: int(item["turn_index"])),
    })

if args.simulator_mode == "trace_replay" and not trace_records:
    raise SystemExit("--simulator-mode trace_replay requires a non-empty DSCodeBench trace corpus")
if args.simulator_mode == "trace_replay" and any(
    record["replay_wait_ms"] <= 0 or not record["latency_source"]
    for record in trace_records
):
    raise SystemExit(
        "trace_replay requires positive replay_wait_ms and latency_source on every turn"
    )

def replay_record(task_id, attempt):
    """Round-robin whole collected trajectories, then advance turns in-place."""
    global trace_cursor, trace_exhausted_turn_reuses
    if not trace_episodes:
        return None, None
    if task_id not in trace_assignment_by_task:
        slot = trace_cursor % len(trace_episodes)
        trace_cursor += 1
        trace_assignment_by_task[task_id] = slot
        trace_id = trace_episodes[slot]["trace_id"]
        trace_selection_counts[trace_id] = trace_selection_counts.get(trace_id, 0) + 1
    slot = trace_assignment_by_task[task_id]
    episode = trace_episodes[slot]
    turns = episode["turns"]
    if not turns:
        return episode, None
    turn_slot = max(attempt - 1, 0)
    if turn_slot >= len(turns):
        trace_exhausted_turn_reuses += 1
        turn_slot = len(turns) - 1
    return episode, turns[turn_slot]

def numeric_stats(values):
    values = list(values)
    if not values:
        return {"count": 0, "min": 0, "max": 0, "mean": 0}
    return {
        "count": len(values),
        "min": min(values),
        "max": max(values),
        "mean": sum(values) / len(values),
    }

def histogram(values):
    out = {}
    for value in values:
        key = str(value)
        out[key] = out.get(key, 0) + 1
    return dict(sorted(out.items(), key=lambda item: int(item[0])))

def task_id_matches_prefix(task_id, prefix):
    if not prefix:
        return True
    prefixes = {prefix, prefix.replace("_", "-")}
    return any(task_id.startswith(candidate) for candidate in prefixes)

class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        parsed = urlparse(self.path)
        if parsed.path != "/stats":
            self.send_error(404)
            return
        prefix = parse_qs(parsed.query).get("prefix", [""])[0]
        with attempts_lock:
            counts = {
                task_id: count
                for task_id, count in attempts_by_task.items()
                if task_id_matches_prefix(task_id, prefix)
            }
            replay_hits = sum(
                count for task_id, count in trace_hits_by_task.items()
                if task_id_matches_prefix(task_id, prefix)
            )
            replay_misses = sum(
                count for task_id, count in trace_misses_by_task.items()
                if task_id_matches_prefix(task_id, prefix)
            )
            observed_latencies = list(observed_latencies_ms)
            replay_cursor = trace_cursor
            replay_selection_counts = dict(trace_selection_counts)
            exhausted_turn_reuses = trace_exhausted_turn_reuses
        ordered = sorted(counts.values())
        body = json.dumps({
            "prefix": prefix,
            "task_count": len(ordered),
            "total_model_calls": sum(ordered),
            "min_steps": min(ordered) if ordered else 0,
            "max_steps": max(ordered) if ordered else 0,
            "step_histogram": histogram(ordered),
            "simulator": {
                "kind": (
                    "trace-replay-dscodebench-real-llm-corpus"
                    if args.simulator_mode == "trace_replay"
                    else "template-dscodebench-oracle-compatibility-mode"
                ),
                "mode": args.simulator_mode,
                "seed": args.seed,
                "replay_wait_ms": {
                    "observed": numeric_stats(observed_latencies),
                    "zero_latency": args.zero_latency,
                },
                "oracle_rows": len(dataset_oracle),
                "trace_replay": {
                    "corpus_path": args.trace_corpus_path,
                    "records": len(trace_records),
                    "trajectories": len(trace_episodes),
                    "sampling_strategy": args.trace_sampling_strategy,
                    "assigned_episodes": replay_cursor,
                    "completed_cycles": (
                        replay_cursor // len(trace_episodes) if trace_episodes else 0
                    ),
                    "next_trace_slot": (
                        replay_cursor % len(trace_episodes) if trace_episodes else 0
                    ),
                    "trace_selection_counts": replay_selection_counts,
                    "exhausted_turn_reuses": exhausted_turn_reuses,
                    "hits": replay_hits,
                    "misses": replay_misses,
                    "contract": {
                        "selection_keys": ["dataset", "episode_arrival_ordinal", "turn_index"],
                        "episode_binding": "one assigned trajectory for the Episode lifetime",
                        "output_fields": ["assistant_output", "response_ids", "logprobs", "replay_wait_ms"],
                    },
                },
            },
            "examples": dict(list(sorted(counts.items()))[:20]),
        }).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        global trace_hits, trace_misses
        size = int(self.headers.get("content-length", "0"))
        raw = b""
        if size:
            raw = self.rfile.read(size)
        task_id = "unknown"
        dataset_problem_id = ""
        try:
            request = json.loads(raw.decode() if raw else "{}")
            message = request.get("messages", [{}])[0].get("content", "")
            found = re.search(r"Task ID: ([A-Za-z0-9_.:-]+)", message)
            if found:
                task_id = found.group(1)
            dataset_found = re.search(r"Dataset Problem ID: ([A-Za-z0-9_.:-]+)", message)
            if dataset_found:
                dataset_problem_id = dataset_found.group(1)
        except Exception:
            pass
        with attempts_lock:
            attempt = attempts_by_task.get(task_id, 0) + 1
            attempts_by_task[task_id] = attempt
            trace_episode, trace_record = (
                replay_record(task_id, attempt)
                if args.simulator_mode == "trace_replay"
                else (None, None)
            )
        sleep_ms = 0.0
        if trace_record:
            sleep_ms = (
                0.0
                if args.zero_latency
                else float(trace_record["replay_wait_ms"])
            )
        else:
            sleep_ms = 0.0
        with attempts_lock:
            observed_latencies_ms.append(sleep_ms)
        time.sleep(sleep_ms / 1000)
        if trace_record:
            with attempts_lock:
                trace_hits += 1
                trace_hits_by_task[task_id] = trace_hits_by_task.get(task_id, 0) + 1
            content = str(trace_record["assistant_output"])
            response_ids = normalize_response_ids(trace_record.get("response_ids"))
            if not response_ids:
                response_ids = [42]
            logprobs = trace_record.get("logprobs") if isinstance(trace_record.get("logprobs"), list) else []
            if not logprobs:
                logprobs = [{"token": content, "token_id": response_ids[0], "logprob": -0.1}]
            trace_source = {
                "trace_id": (trace_episode or {}).get("trace_id", ""),
                "provider_response_id": trace_record.get("provider_response_id", ""),
                "dataset_problem_id": trace_record.get("dataset_problem_id", ""),
                "turn_index": trace_record.get("turn_index", attempt),
            }
        elif args.simulator_mode == "trace_replay":
            with attempts_lock:
                trace_misses += 1
                trace_misses_by_task[task_id] = trace_misses_by_task.get(task_id, 0) + 1
            body = json.dumps({
                "error": {
                    "message": "DSCodeBench trace replay miss: no real LLM output available",
                    "dataset_problem_id": dataset_problem_id,
                    "turn_index": attempt,
                }
            }).encode()
            self.send_response(503)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        elif dataset_problem_id in dataset_oracle:
            content = dataset_oracle[dataset_problem_id]
            response_ids = [42]
            logprobs = [{"token": content, "token_id": 42, "logprob": -0.1}]
            trace_source = {"oracle": True, "dataset_problem_id": dataset_problem_id}
        else:
            content = "def add(a, b):\n    return a + b"
            response_ids = [42]
            logprobs = [{"token": content, "token_id": 42, "logprob": -0.1}]
            trace_source = {"template_fallback": True}
        body = json.dumps({
            "choices": [{
                "message": {"role": "assistant", "content": content},
                "finish_reason": "stop",
                "logprobs": {"content": logprobs}
            }],
            "usage": {"prompt_tokens": 8, "completion_tokens": 8},
            "uenv_model_version": {
                "rollout_param_version": 1,
                "rollout_policy_version": (
                    "dscodebench_pressure-trace-replay-policy-1"
                    if args.simulator_mode == "trace_replay"
                    else "dscodebench_pressure-policy-1"
                )
            },
            "uenv_response_ids": response_ids,
            "uenv_trace_replay": trace_source,
        }).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *_args):
        pass

ThreadingHTTPServer(("127.0.0.1", args.port), Handler).serve_forever()
'''


LOAD_CLIENT = r'''#!/usr/bin/env python3
# 这个脚本会临时写到 server 机器上运行。
# 它负责等待 worker 注册，然后持续向 AdapterCore ExecuteBatch 提交 Code episode。
# 外层编排脚本只负责启动进程；真正的压测请求是在这里发出的。
import argparse
import asyncio
import json
import time
import uuid
import grpc
from grpc import aio as grpc_aio
from uenv.v1 import adapter_core_pb2, scheduler_pb2
import stress_test_common as stress_common

parser = argparse.ArgumentParser()
parser.add_argument("--server", required=True)
parser.add_argument("--workers", type=int, required=True)
parser.add_argument("--slots", type=int, required=True)
parser.add_argument("--mode", required=True)
parser.add_argument("--duration", type=float, required=True)
parser.add_argument("--model-url", required=True)
parser.add_argument("--run-id", required=True)
parser.add_argument("--output", required=True)
parser.add_argument("--max-steps", type=int, required=True)
parser.add_argument("--code-wrong-steps", type=int, required=True)
parser.add_argument("--min-steps", type=int, required=True)
parser.add_argument("--model-name", required=True)
parser.add_argument("--model-mode", choices=("real", "simulator"), required=True)
parser.add_argument("--simulator-mode", choices=("template", "trace_replay"), default="template")
parser.add_argument("--trace-corpus-path", default="")
parser.add_argument("--trace-sampling-strategy", choices=("round_robin_episode",), default="round_robin_episode")
parser.add_argument("--dataset-jsonl", required=True)
parser.add_argument("--dataset-limit", type=int, default=8)
parser.add_argument("--dataset-offset", type=int, default=0)
parser.add_argument("--registration-timeout", type=int, default=180)
parser.add_argument("--batch-timeout", type=int, default=180)
parser.add_argument("--exact-batches", type=int, default=0)
parser.add_argument("--episode-batch-size", type=int, default=0)
parser.add_argument("--concurrent-batches", type=int, default=0)
args = parser.parse_args()
dataset_rows = stress_common.load_dscodebench_jsonl(
    args.dataset_jsonl,
    limit=args.dataset_limit,
    offset=args.dataset_offset,
)

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
    # 等待本次压测启动的 worker 全部注册。只统计带本次 run_id 前缀的 worker，
    # 避免把其它服务里的 worker 算进去。
    deadline = time.monotonic() + args.registration_timeout
    registered = []
    while time.monotonic() < deadline:
        try:
            ok = await health(adapter_core_pb2.HealthCheckRequest(), timeout=3)
            response = await list_workers(scheduler_pb2.ListWorkersRequest(), timeout=3)
            registered = [item.worker_id for item in response.workers]
            expected_prefix = f"stress-{args.run_id}-worker-"
            owned = [item for item in registered if item.startswith(expected_prefix)]
            if ok.ok and len(owned) == args.workers:
                break
        except grpc.RpcError:
            pass
        await asyncio.sleep(1)
    else:
        raise RuntimeError(f"workers not ready expected={args.workers} registered={registered}")

    episode_batch_size = args.episode_batch_size or args.workers
    concurrent_batches = args.concurrent_batches or args.slots
    if episode_batch_size < 1:
        raise ValueError("episode batch size must be positive")
    submitted = completed = failed = rpc_errors = protocol_errors = 0
    rpc_error_details = []
    missing_result_ids = []
    duplicate_result_ids = []
    unknown_result_ids = []
    rewards = []
    latencies = []
    actual_step_counts = []
    training_trace_token_counts = []
    episode_observations = []
    dataset_problem_usage = {}
    lock = asyncio.Lock()
    semaphore = asyncio.Semaphore(concurrent_batches)
    tasks = set()

    batch_sequence = 0

    def make_sample(batch_id, index, sample_ordinal):
        task_id = f"dscodebench-pressure-{args.run_id}-{args.mode}-{batch_id}-{index}"
        mode_offset = {
            "sync": 0,
            "one_step_off_policy": args.workers,
            "fully_async": args.workers * 2,
        }.get(args.mode, 0)
        row = dataset_rows[(mode_offset + sample_ordinal) % len(dataset_rows)]
        problem_id = str(row["problem_id"])
        dataset_problem_usage[problem_id] = dataset_problem_usage.get(problem_id, 0) + 1
        env_config = stress_common.dscodebench_env_payload(
            row,
            task_id=task_id,
            min_steps_before_terminate=args.min_steps,
        )
        return stress_common.make_sample_envelope(
            adapter_core_pb2,
            batch_id=batch_id,
            sample_index=index,
            env_type="code",
            parallel_mode=args.mode,
            env_config=env_config,
            reward_config=stress_common.dscodebench_reward_config(),
            sample_context={
                "stress_run_id": args.run_id,
                "gate": 3,
                "max_steps": args.max_steps,
                "dataset": "DSCodeBench",
                "dataset_problem_id": problem_id,
                "sequence": sample_ordinal,
                "replay_strategy": "round_robin_episode",
            },
            timeout_seconds=args.batch_timeout,
            max_steps=args.max_steps,
            model_url=args.model_url,
            model_name=args.model_name,
        )

    async def send_batch():
        # 一次 ExecuteBatch 发送显式数量的样本。外层 semaphore 控制同时在飞的
        # batch 数，args.slots 表示每个 worker 的并发容量。
        nonlocal submitted, completed, failed, rpc_errors, protocol_errors, batch_sequence
        batch_id = str(uuid.uuid4())
        current_batch = batch_sequence
        batch_sequence += 1
        samples = [
            make_sample(batch_id, index, current_batch * episode_batch_size + index)
            for index in range(episode_batch_size)
        ]
        planned_at = time.time()
        expected_ids = [sample.request_id for sample in samples]
        async with lock:
            submitted += len(samples)
        started = time.monotonic()
        dispatched_at = time.time()
        try:
            response = await execute(
                adapter_core_pb2.ExecuteBatchRequest(
                    request_id=batch_id, batch_id=batch_id, samples=samples
                ), timeout=args.batch_timeout,
            )
            elapsed = (time.monotonic() - started) * 1000
            terminal_at = time.time()
            batch_observations = stress_common.observe_episode_batch(
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
            async with lock:
                latencies.append(elapsed)
                episode_observations.extend(batch_observations)
                result_counts = {}
                for result in response.results:
                    result_counts[result.request_id] = result_counts.get(result.request_id, 0) + 1
                received_ids = set(result_counts)
                batch_missing = sorted(set(expected_ids) - received_ids)
                batch_duplicates = sorted(
                    request_id for request_id, count in result_counts.items() if count > 1
                )
                batch_unknown = sorted(received_ids - set(expected_ids))
                missing_result_ids.extend(batch_missing)
                duplicate_result_ids.extend(batch_duplicates)
                unknown_result_ids.extend(batch_unknown)
                failed += len(batch_missing) + len(batch_duplicates)
                protocol_errors += len(batch_missing) + len(batch_duplicates) + len(batch_unknown)
                for result in response.results:
                    rewards.append(result.reward)
                    if result.status in {"completed", "success"}:
                        completed += 1
                        parsed = stress_common.sample_result_dict(result)
                        actual_step_counts.append(parsed["actual_steps"])
                        if args.mode != "sync":
                            if not parsed["training_trace_valid"]:
                                protocol_errors += 1
                            training_trace_token_counts.append(len(parsed["response_ids"]))
                    else:
                        failed += 1
        except grpc.RpcError as exc:
            terminal_at = time.time()
            batch_observations = stress_common.observe_episode_batch(
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
                episode_observations.extend(batch_observations)
                missing_result_ids.extend(expected_ids)
                if len(rpc_error_details) < 10:
                    rpc_error_details.append({
                        "code": exc.code().name if exc.code() else "UNKNOWN",
                        "details": exc.details() or "",
                        "batch_id": batch_id,
                        "sample_count": len(samples),
                    })
        finally:
            semaphore.release()

    # worker-scale 下要求一次性形成 backlog：exact_batches 个 batch 要尽快全部
    # 提交给 UEnv，再异步等待结果。duration 模式仍保留滚动补充语义。
    started = time.monotonic()
    submit_started = started
    while (
        batch_sequence < args.exact_batches
        if args.exact_batches > 0
        else time.monotonic() - started < args.duration
    ):
        await semaphore.acquire()
        # The condition above may have been evaluated before acquire blocked.
        # Recheck after a previous task releases the slot, otherwise an exact
        # one-batch run can enqueue a second batch with stale loop state.
        if args.exact_batches > 0 and batch_sequence >= args.exact_batches:
            semaphore.release()
            break
        if args.exact_batches == 0 and time.monotonic() - started >= args.duration:
            semaphore.release()
            break
        task = asyncio.create_task(send_batch())
        tasks.add(task)
        task.add_done_callback(tasks.discard)
    client_submit_seconds = time.monotonic() - submit_started
    if tasks:
        await asyncio.gather(*tasks)
    elapsed = time.monotonic() - started
    # 统一用公共模块生成结果，保证 DSCodeBench pressure 和其它脚本的统计字段含义一致。
    document = stress_common.dscodebench_pressure_result_document(
        run_id=args.run_id,
        mode=args.mode,
        configured_workers=args.workers,
        worker_capacity=args.slots,
        elapsed_seconds=elapsed,
        submitted=submitted,
        completed=completed,
        failed=failed,
        rpc_error_episodes=rpc_errors,
        protocol_errors=protocol_errors,
        latencies_ms=latencies,
        rewards=rewards,
    )
    document["max_steps"] = args.max_steps
    document["code_wrong_steps"] = args.code_wrong_steps
    document["min_steps"] = args.min_steps
    document["model_mode"] = args.model_mode
    document["simulator_mode"] = args.simulator_mode
    document["trace_corpus"] = {
        "path": args.trace_corpus_path,
        "sampling_strategy": args.trace_sampling_strategy,
        "schema": {
            "schema_version": 1,
            "benchmark": "DSCodeBench",
            "required_replay_fields": [
                "dataset_problem_id",
                "turn_index",
                "assistant_output",
                "response_ids",
                "logprobs",
                "replay_wait_ms",
            ],
        },
    }
    document["batch_size"] = episode_batch_size
    document["concurrent_batches"] = concurrent_batches
    document["requested_episode_concurrency"] = episode_batch_size * concurrent_batches
    document["backlog_submission"] = {
        "strategy": "submit_exact_batches_then_collect" if args.exact_batches > 0 else "duration_window_refill",
        "planned_batches": args.exact_batches if args.exact_batches > 0 else batch_sequence,
        "submitted_batches": batch_sequence,
        "submitted_to_uenv": submitted,
        "client_submit_seconds": client_submit_seconds,
        "worker_slots": args.workers * args.slots,
        "target_backlog_ratio": submitted / max(1, args.workers * args.slots),
    }
    document["rpc_error_details"] = rpc_error_details
    document["missing_result_ids"] = sorted(set(missing_result_ids))
    document["duplicate_result_ids"] = sorted(set(duplicate_result_ids))
    document["unknown_result_ids"] = sorted(set(unknown_result_ids))
    document["dataset"] = {
        "name": "DSCodeBench",
        "path": args.dataset_jsonl,
        "loaded_rows": len(dataset_rows),
        "loaded_items": len(dataset_rows),
        "offset": args.dataset_offset,
        "problem_ids": [str(row["problem_id"]) for row in dataset_rows],
        "submitted_episodes": submitted,
        "unique_problem_count": len(dataset_problem_usage),
        "unique_items": len(dataset_problem_usage),
        "reuse_factor": submitted / len(dataset_problem_usage) if dataset_problem_usage else 0.0,
        "problem_usage_top20": dict(
            sorted(dataset_problem_usage.items(), key=lambda item: (-item[1], item[0]))[:20]
        ),
        "real_input": True,
    }
    document["actual_step_stats"] = {
        "task_count": len(actual_step_counts),
        "min_steps": min(actual_step_counts) if actual_step_counts else 0,
        "max_steps": max(actual_step_counts) if actual_step_counts else 0,
        "total_steps": sum(actual_step_counts),
    }
    document["training_trace_stats"] = {
        "required": args.mode != "sync",
        "task_count": len(training_trace_token_counts),
        "min_response_tokens": min(training_trace_token_counts) if training_trace_token_counts else 0,
        "total_response_tokens": sum(training_trace_token_counts),
    }
    planned_batches = args.exact_batches if args.exact_batches > 0 else batch_sequence
    submission_strategy = (
        "submit_all_batches_then_collect"
        if args.exact_batches > 0
        else "duration_window_refill"
    )
    document.update(stress_common.pressure_result_metrics(
        configured_workers=args.workers,
        worker_capacity=args.slots,
        batch_size=episode_batch_size,
        planned_batches=planned_batches,
        concurrent_batches=concurrent_batches,
        submission_strategy=submission_strategy,
        client_submit_seconds=client_submit_seconds,
        submitted_batches=batch_sequence,
        elapsed_seconds=elapsed,
        submitted=submitted,
        completed=completed,
        failed=failed,
        rpc_error_episodes=rpc_errors,
        protocol_errors=protocol_errors,
        latencies_ms=latencies,
    ))
    stress_common.assert_pressure_result_schema(document)
    observation_path = args.output + ".episode-observations.jsonl"
    observation_count = stress_common.write_episode_observations_jsonl(
        observation_path, episode_observations
    )
    document["episode_observations"] = {
        "schema_version": stress_common.EPISODE_OBSERVATION_SCHEMA_VERSION,
        "artifact_path": observation_path,
        "row_count": observation_count,
        "submitted_count": submitted,
        "complete": observation_count == submitted,
        "worker_attribution": "unavailable_in_adapter_result",
    }
    with open(args.output, "w", encoding="utf-8") as target:
        json.dump(document, target, indent=2, sort_keys=True)
    print(json.dumps(document, indent=2, sort_keys=True), flush=True)
    await channel.close()
    if completed != submitted or failed or rpc_errors or protocol_errors:
        raise SystemExit(1)

asyncio.run(main())
'''


TCP_PROBE = r'''#!/usr/bin/env python3
# server 机器用这个脚本主动连 worker 内网地址，确认 worker advertise_endpoint 可达。
import argparse, socket, time
p = argparse.ArgumentParser(); p.add_argument("host"); p.add_argument("port", type=int)
p.add_argument("--wait-seconds", type=float, default=60)
a = p.parse_args(); deadline = time.monotonic() + a.wait_seconds
while True:
    try:
        socket.create_connection((a.host, a.port), 2).close()
        print("tcp_probe=ok")
        break
    except OSError:
        if time.monotonic() >= deadline:
            raise
        time.sleep(0.25)
'''


def server_config(workers: int, capacity: int) -> str:
    """生成隔离 server 的配置。

    这里的 server 只服务本次 DSCodeBench pressure 压测，绑定 8099，不使用正式 server。
    completed_async_max_entries 等容量按 worker*slot 放大，避免压测时结果缓存太小。
    """
    total = workers * capacity
    return f'''port: {base.SERVER_PORT}
admin_http_port: 0
admin_http_bind: "127.0.0.1"
scheduler:
  strategy: round_robin
  worker_degraded_threshold_secs: 400
  schedule_retry_interval_ms: 20
  heartbeat_interval_ms: 5000
  heartbeat_timeout_secs: 30
episode:
  default_timeout_secs: 180
  stale_warning_secs: 90
  max_attempts: 3
  queue_dynamic: true
  queue_max_in_flight: 0
  broadcast_capacity: {max(1024, total * 4)}
  completed_async_ttl_secs: 3600
  completed_async_max_entries: {max(10000, total * 8)}
'''


def worker_config(run_dir: str, worker_id: str, port: int, obs_port: int, capacity: int, private_ip: str) -> str:
    """生成单个 Code worker 的配置。

    worker 监听 0.0.0.0:<port>，并把所属节点的内网地址 advertise 给 server。
    env.types 只启用 code，plugin_dir 指向本次压测解包出来的 bundle。
    """
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
  types: ["code"]
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


def _parse_private_port_range(value: str, workers_count: int) -> list[int]:
    if workers_count == 1:
        if value:
            raise ValueError("--private-worker-port-range is only valid when --workers > 1")
        return [base.WORKER_PORT]
    if not value or "-" not in value:
        raise ValueError(
            "--private-worker-port-range START-END is required when --workers > 1; "
            "the range must already be open between the Server and Worker hosts"
        )
    start_text, end_text = value.split("-", 1)
    start, end = int(start_text), int(end_text)
    if start <= 0 or end > 65535 or end < start:
        raise ValueError(f"invalid private worker port range: {value}")
    ports = list(range(start, end + 1))
    if len(ports) < workers_count:
        raise ValueError(
            f"private worker port range {value} has {len(ports)} ports, "
            f"but {workers_count} workers were requested"
        )
    return ports[:workers_count]


def _merge_numeric_stats(items: list[dict]) -> dict:
    """合并多个节点的 count/min/max/mean 统计。count 加权求 mean。"""
    count = sum(int(item.get("count", 0)) for item in items)
    if count <= 0:
        return {"count": 0, "min": 0, "max": 0, "mean": 0}
    total = sum(float(item.get("mean", 0)) * int(item.get("count", 0)) for item in items)
    return {
        "count": count,
        "min": min(float(item.get("min", 0)) for item in items if int(item.get("count", 0)) > 0),
        "max": max(float(item.get("max", 0)) for item in items),
        "mean": total / count,
    }


def _merge_histograms(items: list[dict]) -> dict:
    """合并多个节点的直方图计数。"""
    merged: dict[str, int] = {}
    for item in items:
        for key, value in (item or {}).items():
            merged[str(key)] = merged.get(str(key), 0) + int(value)
    return dict(sorted(merged.items(), key=lambda item: int(item[0])))


def _model_step_stats(worker_clients: dict, prefix: str) -> dict:
    """汇总所有 worker 节点上模型模拟器的 /stats。

    每个节点各跑一个模型模拟器，task_id 前缀过滤在各节点本地完成，
    这里把各节点的计数合并成一份全局视图，并保留 per_node 细分，
    方便确认每个节点分摊到的模型调用量。
    """
    code = (
        "import urllib.parse,urllib.request; "
        f"prefix={prefix!r}; "
        f"url='http://127.0.0.1:{base.MODEL_PORT}/stats?prefix='+urllib.parse.quote(prefix); "
        "print(urllib.request.urlopen(url, timeout=10).read().decode())"
    )
    per_node: dict[str, dict] = {}
    for host, client in worker_clients.items():
        _, output, _ = base.run(client, f"python3 -c {base.q(code)}", timeout=20)
        per_node[host] = json.loads(output)
    if len(per_node) == 1:
        # 单节点保持改造前的返回结构，只附加 per_node 细分。
        merged = dict(next(iter(per_node.values())))
        merged["per_node"] = per_node
        return merged
    node_stats = list(per_node.values())
    simulators = [item.get("simulator", {}) for item in node_stats if isinstance(item.get("simulator"), dict)]
    first_simulator = simulators[0] if simulators else {}
    merged_simulator = dict(first_simulator)
    if simulators:
        merged_simulator["oracle_rows"] = first_simulator.get("oracle_rows", 0)
        merged_replay = dict(first_simulator.get("trace_replay", {}))
        merged_replay["hits"] = sum(int(item.get("trace_replay", {}).get("hits", 0)) for item in simulators)
        merged_replay["misses"] = sum(int(item.get("trace_replay", {}).get("misses", 0)) for item in simulators)
        merged_replay["assigned_episodes"] = sum(
            int(item.get("trace_replay", {}).get("assigned_episodes", 0))
            for item in simulators
        )
        merged_replay["completed_cycles"] = sum(
            int(item.get("trace_replay", {}).get("completed_cycles", 0))
            for item in simulators
        )
        merged_replay["exhausted_turn_reuses"] = sum(
            int(item.get("trace_replay", {}).get("exhausted_turn_reuses", 0))
            for item in simulators
        )
        selection_counts: dict[str, int] = {}
        for item in simulators:
            for trace_id, count in item.get("trace_replay", {}).get(
                "trace_selection_counts", {}
            ).items():
                selection_counts[trace_id] = selection_counts.get(trace_id, 0) + int(count)
        merged_replay["trace_selection_counts"] = selection_counts
        merged_simulator["trace_replay"] = merged_replay
    examples: dict = {}
    for item in node_stats:
        examples.update(item.get("examples", {}))
    return {
        "prefix": prefix,
        "task_count": sum(int(item.get("task_count", 0)) for item in node_stats),
        "total_model_calls": sum(int(item.get("total_model_calls", 0)) for item in node_stats),
        "min_steps": min((int(item.get("min_steps", 0)) for item in node_stats), default=0),
        "max_steps": max((int(item.get("max_steps", 0)) for item in node_stats), default=0),
        "step_histogram": _merge_histograms([item.get("step_histogram", {}) for item in node_stats]),
        "simulator": merged_simulator,
        "examples": dict(list(sorted(examples.items()))[:20]),
        "per_node": per_node,
    }


def _log_timestamp(line: str) -> str:
    match = re.search(
        r"\d{4}-\d{2}-\d{2}[ T]\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:?\d{2})?",
        line,
    )
    return match.group(0) if match else ""


def _line_worker_id(line: str) -> str:
    match = re.search(r"worker_id=([^\s,]+)", line)
    return match.group(1).strip('",') if match else ""


def _completed_worker_coverage(
    server,
    log_path: str,
    worker_prefix: str,
    request_prefix: str = "",
) -> dict:
    """Summarize per-Worker episode load from server logs.

    The server's `episode_completed` marker is the authoritative observed
    completion signal.  Start/assignment markers are recorded when present; on
    older logs without such markers, first/last completion time is still a
    lower-bound load window for that Worker.
    """
    rows: dict[str, dict] = {}
    start_markers = (
        "episode_started",
        "episode_start",
        "episode_dispatched",
        "episode_assigned",
        "dispatch_episode",
        "lease_granted",
    )
    for line_no, line in enumerate(base.get_text(server, log_path).splitlines(), start=1):
        if request_prefix and request_prefix not in line:
            continue
        worker_id = _line_worker_id(line)
        if not worker_id.startswith(worker_prefix):
            continue
        row = rows.setdefault(worker_id, {
            "worker_id": worker_id,
            "started_episodes_observed": 0,
            "completed_episodes": 0,
            "first_start_timestamp": "",
            "last_start_timestamp": "",
            "first_completion_timestamp": "",
            "last_completion_timestamp": "",
            "first_observed_timestamp": "",
            "last_observed_timestamp": "",
            "first_observed_line": line_no,
            "last_observed_line": line_no,
        })
        ts = _log_timestamp(line)
        if ts and not row["first_observed_timestamp"]:
            row["first_observed_timestamp"] = ts
        if ts:
            row["last_observed_timestamp"] = ts
        row["last_observed_line"] = line_no
        if any(marker in line for marker in start_markers):
            row["started_episodes_observed"] += 1
            if ts and not row["first_start_timestamp"]:
                row["first_start_timestamp"] = ts
            if ts:
                row["last_start_timestamp"] = ts
        if "episode_completed" in line:
            row["completed_episodes"] += 1
            if ts and not row["first_completion_timestamp"]:
                row["first_completion_timestamp"] = ts
            if ts:
                row["last_completion_timestamp"] = ts
    per_worker = sorted(rows.values(), key=lambda item: item["worker_id"])
    worker_ids = [row["worker_id"] for row in per_worker if row["completed_episodes"] > 0]
    return {
        "unique_completed_workers": len(worker_ids),
        "worker_ids": worker_ids,
        "load_timeline": {
            "unique_workers_observed": len(per_worker),
            "total_started_episodes_observed": sum(row["started_episodes_observed"] for row in per_worker),
            "total_completed_episodes_observed": sum(row["completed_episodes"] for row in per_worker),
            "workers_without_completion": [
                row["worker_id"] for row in per_worker if row["completed_episodes"] == 0
            ],
            "per_worker": per_worker,
            "observability_note": (
                "Counts are parsed from server.log. episode_completed is counted as completed load; "
                "start timestamps are populated only when the server emits start/assignment markers."
            ),
        },
    }


def _prepare_worker_trace_corpus(worker, worker_run: str, trace_corpus_path: str, run_id: str) -> str:
    """Return a Worker-local trace corpus path, uploading local corpus if needed."""
    if not trace_corpus_path:
        return ""
    candidate = Path(trace_corpus_path)
    if candidate.exists():
        if candidate.is_file():
            remote_path = f"{worker_run}/dscodebench-trace-corpus.jsonl"
            with worker.open_sftp() as sftp:
                sftp.put(str(candidate), remote_path)
            return remote_path
        remote_dir = f"{worker_run}/trace-corpus"
        with tempfile.NamedTemporaryFile(prefix=f"{run_id}-dscodebench-trace-", suffix=".tgz", delete=False) as tmp:
            local_archive = Path(tmp.name)
        try:
            with tarfile.open(local_archive, "w:gz") as tar:
                for path in sorted(candidate.rglob("*")):
                    if path.is_file():
                        tar.add(path, arcname=str(path.relative_to(candidate)))
            base.run(worker, f"install -d -m 0755 {base.q(remote_dir)}")
            remote_archive = f"{worker_run}/trace-corpus.tgz"
            with worker.open_sftp() as sftp:
                sftp.put(str(local_archive), remote_archive)
            base.run(worker, f"tar -C {base.q(remote_dir)} -xzf {base.q(remote_archive)}", timeout=120)
            return remote_dir
        finally:
            local_archive.unlink(missing_ok=True)
    if trace_corpus_path.startswith("/"):
        return trace_corpus_path
    raise ValueError(
        "--trace-corpus-path must be a Worker absolute path or an existing local file/directory"
    )


# Public compatibility names used by other real-Worker scale lanes. They keep
# fleet startup and dispatch-coverage semantics aligned while the remaining
# workload-specific implementation is migrated into smaller core modules.
isolated_server_config = server_config
parse_private_worker_ports = _parse_private_port_range
completed_worker_coverage = _completed_worker_coverage


def run_scale(
    workers_count: int,
    capacity: int,
    worker_ports: list[int],
    modes: tuple[str, ...],
    duration: int,
    artifacts: Path,
    password: str,
    max_steps: int,
    code_wrong_steps: int,
    min_steps: int,
    model_mode: str,
    llm_config: str,
    dataset_jsonl: str,
    dataset_limit: int,
    dataset_offset: int,
    exact_batches: int,
    registration_timeout: int,
    batch_timeout: int,
    fleet_supervisor_threshold: int,
    simulator_seed: int,
    simulator_mode: str,
    trace_corpus_path: str,
    trace_sampling_strategy: str,
    plugin_ready_timeout_seconds: int,
    worker_register_max_attempts: int,
    worker_register_retry_backoff_ms: int,
    episode_batch_size: int,
    concurrent_batches: int,
    acceptance_purpose: str,
    code_python: str,
) -> dict:
    """执行一组 DSCodeBench pressure 规模。

    workers_count 是 worker 数，capacity 是每个 worker 的并发 slot 数。
    这个函数完成以下步骤：
    1. 连接 server 和全部 worker 节点；
    2. 检查正式 server 未被影响、各节点端口空闲；
    3. 打包 worker 和 code plugin，从 server 机器传到各 worker 节点；
    4. 启动隔离 server、各节点本机的模型模拟器和真实 Code worker；
    5. 依次跑三种 parallel_mode；
    6. 清理本次启动的进程并复查端口。
    """
    run_id = f"dscodebench-pressure-{workers_count}x{capacity}-{time.strftime('%Y%m%d-%H%M%S')}-{uuid.uuid4().hex[:6]}"
    server_run = f"/tmp/uenv-{run_id}"
    worker_run = f"/opt/uenv-stress/runs/{run_id}"
    local_run = artifacts / run_id
    local_run.mkdir(parents=True)
    # worker 按索引 round-robin 分配到各节点：worker i 落在节点 i % len(nodes)。
    # 每个节点使用相同的 worker_run 路径（不同机器互不冲突），但拥有独立的
    # SSH 连接、bundle、模型模拟器、fleet supervisor 和端口检查。
    nodes = list(base.WORKER_NODES)
    assignments = base.worker_assignments(workers_count)
    node_worker_indexes = {node.host: indexes for node, indexes in zip(nodes, assignments)}
    server = None
    worker_clients: dict = {}
    server_pid = None
    # PID 是每台机器各自的资源，必须按节点记录；路径类字符串在各节点相同，
    # 保持标量以兼容单节点时的 manifest 结构。
    model_pids: dict[str, int | None] = {node.host: None for node in nodes}
    fleet_supervisor_pids: dict[str, int | None] = {node.host: None for node in nodes}
    worker_pids_by_node: dict[str, list[tuple[int, str]]] = {node.host: [] for node in nodes}
    fleet_metrics_paths: dict[str, str] = {node.host: "" for node in nodes}
    fleet_resource_metrics: dict = {}
    model_script = ""
    llm_config_sha256 = ""
    effective_trace_corpus_path = ""
    real_llm_trace_output_dir = ""
    before = None
    cleanup_errors: list[str] = []
    results: list[dict] = []
    error: str | None = None
    outcome: dict | None = None
    ports = worker_ports
    obs_ports = [base.OBS_PORT + i for i in range(workers_count)]
    try:
        # 连接 server 和全部 worker 节点后，第一件事是记录正式 server 快照。
        server = base.connect(base.SERVER_HOST, password)
        worker_clients = base.connect_worker_nodes(password)
        before = base.protected_snapshot(server)
        base.assert_port_free(server, base.SERVER_PORT, base.SERVER_HOST)
        for node in nodes:
            indexes = node_worker_indexes[node.host]
            node_ports = [ports[i] for i in indexes]
            node_obs_ports = [obs_ports[i] for i in indexes]
            base.assert_ports_free(
                worker_clients[node.host],
                node_ports + node_obs_ports + [base.MODEL_PORT],
                node.host,
            )
        print(
            f"[dscodebench_pressure:{workers_count}x{capacity}] preflight ports={ports[0]}-{ports[-1]} "
            f"worker_nodes={json.dumps({node.host: node_worker_indexes[node.host] for node in nodes})}",
            flush=True,
        )
        build = base.source_and_binary_manifest(server, include_code_plugin=True)
        print(f"[dscodebench_pressure:{workers_count}x{capacity}] source/binary manifest ready git={build['git_sha'][:12]}", flush=True)
        base.run(server, f"test -f {base.q(dataset_jsonl)}", timeout=20)
        _, dataset_hash_out, _ = base.run(server, f"sha256sum {base.q(dataset_jsonl)}", timeout=30)
        dataset_sha256 = dataset_hash_out.split()[0]
        print(f"[dscodebench_pressure:{workers_count}x{capacity}] dataset manifest ready sha256={dataset_sha256[:12]}", flush=True)

        # bundle 先在 server 机器上制作，再通过本地临时文件转传到 worker 机器。
        # 这样 worker 机器不需要直接访问 /home/uenv 源码目录。
        base.run(server, f"install -d -m 0755 {base.q(server_run)}/bundle/plugins/code/scripts {base.q(server_run)}/generated/uenv/v1")
        for node in nodes:
            base.run(
                worker_clients[node.host],
                f"install -d -m 0755 {base.q(worker_run)}/logs {base.q(worker_run)}/wal",
            )
        base.run(server, " && ".join([
            f"install -m 0755 {base.q(base.SOURCE_WORKER_BIN)} {base.q(server_run)}/bundle/uenv-worker",
            f"install -m 0755 {base.q(base.SOURCE_CODE_BIN)} {base.q(server_run)}/bundle/uenv-code-plugin",
            f"strip {base.q(server_run)}/bundle/uenv-worker {base.q(server_run)}/bundle/uenv-code-plugin",
            f"cp -a {base.q(base.SOURCE_REPO)}/plugins/code/. {base.q(server_run)}/bundle/plugins/code/",
            f"tar -C {base.q(server_run)}/bundle -czf {base.q(server_run)}/bundle.tgz .",
        ]), timeout=180)
        print(f"[dscodebench_pressure:{workers_count}x{capacity}] worker/plugin bundle built", flush=True)
        with tempfile.NamedTemporaryFile(prefix=run_id, suffix=".tgz", delete=False) as tmp:
            local_bundle = Path(tmp.name)
        try:
            print(f"[dscodebench_pressure:{workers_count}x{capacity}] copying worker/plugin bundle to {len(nodes)} worker node(s)", flush=True)
            with server.open_sftp() as sftp: sftp.get(f"{server_run}/bundle.tgz", str(local_bundle))
            for node in nodes:
                with worker_clients[node.host].open_sftp() as sftp:
                    sftp.put(str(local_bundle), f"{worker_run}/bundle.tgz")
        finally:
            local_bundle.unlink(missing_ok=True)
        for node in nodes:
            base.run(
                worker_clients[node.host],
                f"install -d -m 0755 {base.q(worker_run)}/bundle && tar -C {base.q(worker_run)}/bundle -xzf {base.q(worker_run)}/bundle.tgz",
            )
        print(f"[dscodebench_pressure:{workers_count}x{capacity}] worker/plugin bundle installed", flush=True)
        base.put_text(server, f"{server_run}/server.yaml", server_config(workers_count, capacity))
        base.put_text(server, f"{server_run}/load_client.py", LOAD_CLIENT, 0o755)
        base.put_text(server, f"{server_run}/stress_test_common.py", COMMON_SOURCE)
        base.put_text(server, f"{server_run}/tcp_probe.py", TCP_PROBE, 0o755)
        for node in nodes:
            client = worker_clients[node.host]
            base.put_text(client, f"{worker_run}/model_simulator.py", MODEL_SIMULATOR, 0o755)
            base.put_text(client, f"{worker_run}/ark_real_llm_proxy.py", REAL_LLM_PROXY_SOURCE, 0o700)
            base.put_text(client, f"{worker_run}/ark_real_llm_preflight.py", REAL_LLM_PREFLIGHT_SOURCE, 0o700)
            base.put_text(client, f"{worker_run}/worker_fleet_supervisor.py", FLEET_SUPERVISOR_SOURCE, 0o700)
        print(f"[dscodebench_pressure:{workers_count}x{capacity}] runtime scripts uploaded", flush=True)
        worker_dataset = f"{worker_run}/DSCodeBench.json"
        if model_mode == "simulator":
            print(f"[dscodebench_pressure:{workers_count}x{capacity}] copying DSCodeBench dataset to {len(nodes)} worker node(s)", flush=True)
            with tempfile.NamedTemporaryFile(prefix=run_id, suffix="-dscodebench.json", delete=False) as tmp:
                local_dataset = Path(tmp.name)
            try:
                with server.open_sftp() as sftp:
                    sftp.get(dataset_jsonl, str(local_dataset))
                for node in nodes:
                    with worker_clients[node.host].open_sftp() as sftp:
                        sftp.put(str(local_dataset), worker_dataset)
            finally:
                local_dataset.unlink(missing_ok=True)
            print(f"[dscodebench_pressure:{workers_count}x{capacity}] DSCodeBench dataset copied", flush=True)
            if simulator_mode == "trace_replay":
                for node in nodes:
                    node_corpus_path = _prepare_worker_trace_corpus(
                        worker_clients[node.host], worker_run, trace_corpus_path, run_id
                    )
                    if effective_trace_corpus_path and node_corpus_path != effective_trace_corpus_path:
                        raise RuntimeError(
                            f"trace corpus path diverged across nodes: {node.host}={node_corpus_path} "
                            f"vs {effective_trace_corpus_path}"
                        )
                    effective_trace_corpus_path = node_corpus_path
                print(
                    f"[dscodebench_pressure:{workers_count}x{capacity}] DSCodeBench trace corpus ready "
                    f"path={effective_trace_corpus_path}",
                    flush=True,
                )
        for node in nodes:
            worker_documents = {}
            for i in node_worker_indexes[node.host]:
                worker_id = f"stress-{run_id}-worker-{i:04d}"
                worker_documents[f"{worker_run}/worker-{i:04d}.yaml"] = (
                    worker_config(worker_run, worker_id, ports[i], obs_ports[i], capacity, node.private_ip),
                    0o600,
                )
            put_worker_config_archive(worker_clients[node.host], worker_run, worker_documents, run_id)
            print(
                f"[dscodebench_pressure:{workers_count}x{capacity}] worker configs archive uploaded "
                f"node={node.host} count={len(worker_documents)}",
                flush=True,
            )
        # load_client.py 需要 Python 版 protobuf message，所以每次用当前 proto 生成。
        proto_root = f"{base.SOURCE_REPO}/proto"
        proto = " ".join([
            "/usr/bin/protoc", "-I", base.q(proto_root), f"--python_out={base.q(server_run)}/generated",
            f"{base.q(proto_root)}/uenv/v1/common.proto", f"{base.q(proto_root)}/uenv/v1/episode.proto",
            f"{base.q(proto_root)}/uenv/v1/scheduler.proto", f"{base.q(proto_root)}/uenv/v1/adapter_core.proto",
        ])
        base.run(server, proto)
        base.run(server, f"touch {base.q(server_run)}/generated/uenv/__init__.py {base.q(server_run)}/generated/uenv/v1/__init__.py")
        print(f"[dscodebench_pressure:{workers_count}x{capacity}] generated Python protobuf stubs", flush=True)
        # 关闭 trajectory/obs，减少压测以外的写入和背景工作。
        # Scale acceptance needs the assignment.worker_id on every completion
        # to prove that all real Workers, rather than only N registry entries,
        # actually executed an episode.
        server_log_filter = "info" if acceptance_purpose in {"worker-scale", "single-worker-diagnostic"} else "warn"
        server_cmd = " ".join([
            "env", "UENV_SERVER_CONFIG_STRICT=1", "UENV_TRAJECTORY_ENABLED=0", "UENV_OBS_ENABLED=0", "UENV_LOG_ANSI=0",
            f"UENV_ADDR={base.SERVER_PRIVATE_IP}:{base.SERVER_PORT}", f"UENV_CONFIG_PATH={server_run}/server.yaml",
            f"RUST_LOG={server_log_filter}", base.SERVER_BIN,
        ])
        server_pid = base.start_owned(server, server_cmd, f"{server_run}/server.log", base.SERVER_BIN, base.SERVER_BIN)
        if model_mode == "real":
            model_script = "ark_real_llm_proxy.py"
            real_llm_trace_output_dir = f"{worker_run}/trace-corpus"
            for node in nodes:
                client = worker_clients[node.host]
                _, mode_out, _ = base.run(client, f"stat -c %a {base.q(llm_config)}")
                if mode_out.strip() != "600":
                    raise RuntimeError(f"{node.host}: real LLM config must have mode 0600")
                _, hash_out, _ = base.run(client, f"sha256sum {base.q(llm_config)}")
                node_llm_sha256 = hash_out.split()[0]
                if llm_config_sha256 and node_llm_sha256 != llm_config_sha256:
                    raise RuntimeError(f"{node.host}: real LLM config hash diverged across nodes")
                llm_config_sha256 = node_llm_sha256
                base.run(client, f"install -d -m 0700 {base.q(real_llm_trace_output_dir)}")
            model_command = (
                f"python3 -B {worker_run}/{model_script} --port {base.MODEL_PORT} "
                f"--config {base.q(llm_config)} "
                f"--trace-output-dir {base.q(real_llm_trace_output_dir)}"
            )
        else:
            model_script = "model_simulator.py"
            model_command = (
                f"python3 -B {worker_run}/model_simulator.py --port {base.MODEL_PORT} "
                f"--seed {simulator_seed} "
                f"--dataset-jsonl {base.q(worker_dataset)} "
                f"--simulator-mode {base.q(simulator_mode)} "
                f"--trace-corpus-path {base.q(effective_trace_corpus_path)} "
                f"--trace-sampling-strategy {base.q(trace_sampling_strategy)}"
            )
        # 每台 worker 节点各跑一个模型端点（模拟器或 real-LLM 代理）。
        # worker.yaml 里的 model URL 是 127.0.0.1:<MODEL_PORT>，天然指向本机端点。
        for node in nodes:
            client = worker_clients[node.host]
            model_pids[node.host] = base.start_owned(
                client,
                model_command,
                f"{worker_run}/model.log",
                "/usr/bin/python3.12",
                f"{worker_run}/{model_script}",
            )
            if model_mode == "real":
                _, preflight_out, _ = base.run(
                    client,
                    f"python3 -B {worker_run}/ark_real_llm_preflight.py "
                    f"--url http://127.0.0.1:{base.MODEL_PORT}/v1",
                    timeout=240,
                )
                print(f"[dscodebench_pressure:{workers_count}x{capacity}] node={node.host} {preflight_out.strip()}", flush=True)
        # 每个 worker 都有独立 yaml、独立 WAL 和独立日志，方便定位某个 worker 的问题。
        worker_env = {
            "UENV_SERVER_CONFIG_STRICT": "1",
            "UENV_TRAJECTORY_ENABLED": "0",
            "UENV_OBS_ENABLED": "0",
            "UENV_LOG_ANSI": "0",
            "UENV_WORKER_EPISODE_TIMEOUT_SECS": str(batch_timeout),
            "UENV_LLM_HTTP_TIMEOUT_SECS": str(batch_timeout),
            "UENV_CODE_PLUGIN_BIN": f"{worker_run}/bundle/uenv-code-plugin",
            "UENV_CODE_EVAL_SCRIPT": f"{worker_run}/bundle/plugins/code/scripts/evaluate_code.py",
            "UENV_PLUGIN_READY_TIMEOUT_SECS": str(plugin_ready_timeout_seconds),
            "UENV_WORKER_REGISTER_MAX_ATTEMPTS": str(worker_register_max_attempts),
            "UENV_WORKER_REGISTER_RETRY_BACKOFF_MS": str(worker_register_retry_backoff_ms),
            "OMP_NUM_THREADS": "1",
            "OPENBLAS_NUM_THREADS": "1",
            "MKL_NUM_THREADS": "1",
            "NUMEXPR_NUM_THREADS": "1",
            "TF_NUM_INTRAOP_THREADS": "1",
            "TF_NUM_INTEROP_THREADS": "1",
            "CUDA_VISIBLE_DEVICES": "",
            "RUST_LOG": "error",
        }
        if code_python:
            worker_env["UENV_CODE_PYTHON"] = code_python
        # 每个节点只启动分配到本机的 worker（fleet supervisor 也只管本机那批）。
        for node in nodes:
            client = worker_clients[node.host]
            indexes = node_worker_indexes[node.host]
            if workers_count >= fleet_supervisor_threshold:
                fleet_spec = {
                    "workers": [
                        {
                            "worker_id": f"stress-{run_id}-worker-{i:04d}",
                            "config": f"{worker_run}/worker-{i:04d}.yaml",
                            "argv": [
                                f"{worker_run}/bundle/uenv-worker",
                                "--config",
                                f"{worker_run}/worker-{i:04d}.yaml",
                                "serve",
                            ],
                            "env": worker_env,
                            "log": f"{worker_run}/logs/worker-{i:04d}.log",
                        }
                        for i in indexes
                    ]
                }
                fleet_spec_path = f"{worker_run}/fleet.json"
                fleet_pid_path = f"{worker_run}/fleet-pids.json"
                fleet_metrics_path = f"{worker_run}/fleet-metrics.json"
                base.put_text(client, fleet_spec_path, json.dumps(fleet_spec, sort_keys=True), 0o600)
                fleet_command = (
                    f"python3 -B {worker_run}/worker_fleet_supervisor.py "
                    f"--spec {fleet_spec_path} --pid-file {fleet_pid_path} "
                    f"--metrics-file {fleet_metrics_path} "
                    f"--resource-csv {worker_run}/fleet-resources.csv "
                    f"--run-id {run_id}"
                )
                fleet_supervisor_pids[node.host] = base.start_owned(
                    client,
                    fleet_command,
                    f"{worker_run}/fleet-supervisor.log",
                    "/usr/bin/python3.12",
                    f"{worker_run}/worker_fleet_supervisor.py",
                )
                fleet_metrics_paths[node.host] = fleet_metrics_path
                fleet_deadline = time.monotonic() + max(60, workers_count // 2)
                while time.monotonic() < fleet_deadline:
                    status, _, _ = base.run(client, f"test -s {base.q(fleet_pid_path)}", check=False)
                    if status == 0:
                        fleet_pid_document = json.loads(base.get_text(client, fleet_pid_path))
                        break
                    time.sleep(0.5)
                else:
                    raise RuntimeError(f"{node.host}: real Worker fleet supervisor did not publish its PID manifest")
                if fleet_pid_document.get("worker_count") != len(indexes):
                    raise RuntimeError(f"{node.host}: fleet PID manifest count mismatch: {fleet_pid_document}")
                worker_pids_by_node[node.host] = [
                    (int(item["pid"]), str(item["config"]))
                    for item in fleet_pid_document["workers"]
                ]
            else:
                for i in indexes:
                    config = f"{worker_run}/worker-{i:04d}.yaml"
                    env_parts = [f"{key}={value}" for key, value in worker_env.items()]
                    cmd = " ".join([
                        "env",
                        *env_parts,
                        f"{worker_run}/bundle/uenv-worker",
                        "--config",
                        config,
                        "serve",
                    ])
                    pid = base.start_owned(
                        client,
                        cmd,
                        f"{worker_run}/logs/worker-{i:04d}.log",
                        f"{worker_run}/bundle/uenv-worker",
                        config,
                    )
                    worker_pids_by_node[node.host].append((pid, config))
        endpoint_ready_timeout = min(registration_timeout, max(60, workers_count // 4))
        for node in nodes:
            indexes = node_worker_indexes[node.host]
            node_ports = [ports[i] for i in indexes]
            for port in sorted({node_ports[0], node_ports[-1]}):
                base.run(
                    server,
                    f"python3 -B {server_run}/tcp_probe.py {node.private_ip} {port} "
                    f"--wait-seconds {endpoint_ready_timeout}",
                    timeout=endpoint_ready_timeout + 15,
                )
        print(f"[dscodebench_pressure:{workers_count}x{capacity}] private Worker range endpoints reachable", flush=True)
        base.assert_protected_unchanged(server, before)
        # manifest 记录本轮实际启动的端口、PID 和受保护 server 快照。
        # 后续排查时先看 manifest，再看 result/log。
        # 单节点时 owned_pids/fleet_metrics_path 保持改造前的标量结构；
        # 多节点时改为 per_node 字典，键是节点 host。
        owned_pids_by_node = {
            node.host: {
                "model": model_pids[node.host],
                "fleet_supervisor": fleet_supervisor_pids[node.host],
                "workers": [p for p, _ in worker_pids_by_node[node.host]],
            }
            for node in nodes
        }
        manifest = {
            "run_id": run_id, "gate": 3, "acceptance_purpose": acceptance_purpose,
            "environment": "code", "real_workers": workers_count,
            "worker_capacity": capacity, "worker_slots": workers_count * capacity,
            "requested_episode_concurrency": episode_batch_size * concurrent_batches, "modes": list(modes),
            "episode_batch_size": episode_batch_size,
            "concurrent_batches": concurrent_batches,
            "duration_per_mode_seconds": duration, "max_steps": max_steps,
            "code_wrong_steps": code_wrong_steps, "min_steps": min_steps, "worker_ports": ports,
            "worker_nodes": [{"host": node.host, "private_ip": node.private_ip} for node in nodes],
            "node_worker_indexes": {node.host: node_worker_indexes[node.host] for node in nodes},
            "code_python": code_python or "python3",
            "plugin_ready_timeout_seconds": plugin_ready_timeout_seconds,
            "worker_registration_retry": {
                "max_attempts": worker_register_max_attempts,
                "backoff_ms": worker_register_retry_backoff_ms,
            },
            "model_mode": model_mode,
            "simulator_mode": simulator_mode,
            "model_kind": (
                "real-ark-chat-completions+ark-tokenization"
                if model_mode == "real"
                else (
                    "trace-replay-real-dscodebench-llm-corpus"
                    if simulator_mode == "trace_replay"
                    else "template-dataset-oracle-compatibility-mode"
                )
            ),
            "trace_corpus": {
                "schema_version": 1,
                "benchmark": "DSCodeBench",
                "capture_output_dir": real_llm_trace_output_dir,
                "requested_path": trace_corpus_path,
                "effective_worker_path": effective_trace_corpus_path,
                "sampling_strategy": trace_sampling_strategy,
                "required_fields": [
                    "dataset_problem_id",
                    "turn_index",
                    "assistant_output",
                    "response_ids",
                    "logprobs",
                    "replay_wait_ms",
                ],
                "note": (
                    "Real LLM capture writes DSCodeBench turns; trace_replay scale tests replay "
                    "those real assistant outputs and do not use ground_truth_code as model output."
                ),
            },
            "dataset": {
                "name": "DSCodeBench",
                "path": dataset_jsonl,
                "sha256": dataset_sha256,
                "limit": dataset_limit,
                "offset": dataset_offset,
                "real_input": True,
            },
            "llm_config": llm_config if model_mode == "real" else "",
            "llm_config_sha256": llm_config_sha256,
            "source_and_binaries": build,
            "protected_server": before,
            "owned_pids": (
                {"server": server_pid, **owned_pids_by_node[nodes[0].host]}
                if len(nodes) == 1
                else {"server": server_pid, "per_node": owned_pids_by_node}
            ),
            "fleet_metrics_path": (
                fleet_metrics_paths[nodes[0].host]
                if len(nodes) == 1
                else {node.host: fleet_metrics_paths[node.host] for node in nodes}
            ),
        }
        base.put_text(server, f"{server_run}/manifest.json", json.dumps(manifest, indent=2, sort_keys=True))
        for node in nodes:
            base.put_text(
                worker_clients[node.host],
                f"{worker_run}/manifest.json",
                json.dumps(manifest, indent=2, sort_keys=True),
            )
        (local_run / "manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True))
        # 同一组 worker 不重启，连续跑三种模式，减少启动成本。
        for mode in modes:
            remote_result = f"{server_run}/result-{mode}.json"
            load_client_args = [
                f"PYTHONPATH={server_run}:{server_run}/generated", "python3", "-B", f"{server_run}/load_client.py",
                "--server", f"{base.SERVER_PRIVATE_IP}:{base.SERVER_PORT}", "--workers", str(workers_count),
                "--slots", str(capacity), "--mode", mode, "--duration", str(duration),
                "--model-url", f"http://127.0.0.1:{base.MODEL_PORT}/v1", "--run-id", run_id, "--output", remote_result,
                "--max-steps", str(max_steps), "--code-wrong-steps", str(code_wrong_steps),
                "--min-steps", str(min_steps), "--model-mode", model_mode,
                "--simulator-mode", simulator_mode,
                "--trace-sampling-strategy", trace_sampling_strategy,
                "--dataset-jsonl", dataset_jsonl,
                "--dataset-limit", str(dataset_limit),
                "--dataset-offset", str(dataset_offset),
                "--registration-timeout", str(registration_timeout),
                "--batch-timeout", str(batch_timeout),
                "--exact-batches", str(exact_batches),
                "--episode-batch-size", str(episode_batch_size),
                "--concurrent-batches", str(concurrent_batches),
                "--model-name", ("proxy-selected-versioned-model" if model_mode == "real" else "dscodebench-pressure-model"),
            ]
            if effective_trace_corpus_path:
                load_client_args.extend(["--trace-corpus-path", effective_trace_corpus_path])
            command = " ".join(load_client_args)
            print(f"[dscodebench_pressure:{workers_count}x{capacity}] mode={mode} start", flush=True)
            command_timeout = registration_timeout + max(batch_timeout, duration + 240)
            _, out, err = base.run(server, command, timeout=command_timeout)
            if err: print(err, flush=True)
            result = json.loads(base.get_text(server, remote_result))
            local_observations = local_run / f"episode-observations-{mode}.jsonl"
            local_observations.write_text(
                base.get_text(server, remote_result + ".episode-observations.jsonl"),
                encoding="utf-8",
            )
            result["episode_observations"]["local_artifact"] = str(local_observations)
            step_stats = _model_step_stats(worker_clients, f"dscodebench-pressure-{run_id}-{mode}-")
            result["model_step_stats"] = step_stats
            trace_replay_stats = (
                step_stats.get("simulator", {}).get("trace_replay", {})
                if isinstance(step_stats.get("simulator"), dict)
                else {}
            )
            result["trace_replay"] = {
                "required": model_mode == "simulator" and simulator_mode == "trace_replay",
                "corpus_path": trace_replay_stats.get("corpus_path", effective_trace_corpus_path),
                "records": trace_replay_stats.get("records", 0),
                "trajectories": trace_replay_stats.get("trajectories", 0),
                "sampling_strategy": trace_replay_stats.get("sampling_strategy", trace_sampling_strategy),
                "assigned_episodes": trace_replay_stats.get("assigned_episodes", 0),
                "completed_cycles": trace_replay_stats.get("completed_cycles", 0),
                "next_trace_slot": trace_replay_stats.get("next_trace_slot", 0),
                "trace_selection_counts": trace_replay_stats.get("trace_selection_counts", {}),
                "exhausted_turn_reuses": trace_replay_stats.get("exhausted_turn_reuses", 0),
                "hits": trace_replay_stats.get("hits", 0),
                "misses": trace_replay_stats.get("misses", 0),
                "passed": (
                    model_mode != "simulator"
                    or simulator_mode != "trace_replay"
                    or (
                        trace_replay_stats.get("records", 0) > 0
                        and trace_replay_stats.get("trajectories", 0) > 1
                        and trace_replay_stats.get("sampling_strategy") == "round_robin_episode"
                        and len(trace_replay_stats.get("trace_selection_counts", {})) > 1
                        and trace_replay_stats.get("hits", 0) == step_stats.get("total_model_calls", 0)
                        and trace_replay_stats.get("misses", 0) == 0
                    )
                ),
            }
            expected_min_steps = min_steps
            step_validation = {
                "expected_min_steps": expected_min_steps,
                "max_allowed_steps": max_steps,
                "completed_episodes": result["completed"],
                "stats": step_stats,
                "passed": (
                    step_stats["task_count"] == result["completed"]
                    and step_stats["min_steps"] >= expected_min_steps
                    and step_stats["max_steps"] <= max_steps
                    and result["trace_replay"]["passed"]
                    and result["actual_step_stats"]["task_count"] == result["completed"]
                    and result["actual_step_stats"]["min_steps"] >= expected_min_steps
                    and result["actual_step_stats"]["max_steps"] <= max_steps
                    and (
                        mode == "sync"
                        or result["training_trace_stats"]["min_response_tokens"] > 0
                    )
                ),
            }
            result["step_validation"] = step_validation
            if not step_validation["passed"]:
                raise RuntimeError(f"actual multi-step validation failed: {step_validation}")
            if acceptance_purpose in {"worker-scale", "single-worker-diagnostic"}:
                coverage = _completed_worker_coverage(
                    server,
                    f"{server_run}/server.log",
                    f"stress-{run_id}-worker-",
                    # 不传 request_prefix：episode_completed 行的 episode/batch ID 是
                    # 纯 UUID，任何按请求前缀的过滤都会得到 0；worker_prefix 已含
                    # 本次 run_id（时间戳+哈希），足以把统计限定在本次运行。
                )
                coverage["expected_workers"] = workers_count
                coverage["passed"] = coverage["unique_completed_workers"] == workers_count
                result["worker_dispatch_coverage"] = coverage
                if not coverage["passed"]:
                    raise RuntimeError(
                        "not every real Worker completed an episode: "
                        f"expected={workers_count} actual={coverage['unique_completed_workers']}"
                    )
            results.append(result)
            (local_run / f"result-{mode}.json").write_text(json.dumps(result, indent=2, sort_keys=True))
            print(f"[dscodebench_pressure:{workers_count}x{capacity}] mode={mode} PASS throughput={result['throughput_eps']:.2f} ep/s", flush=True)
        (local_run / "results.json").write_text(json.dumps(results, indent=2, sort_keys=True))
        # Preserve the scheduler/event evidence before optional trace export.
        (local_run / "server.log").write_text(
            base.get_text(server, f"{server_run}/server.log"),
            encoding="utf-8",
        )
        if model_mode == "real" and real_llm_trace_output_dir:
            # 多节点时每台机器各有一份 trace 产物，文件名带 host 后缀区分；
            # 单节点保持改造前的文件名。
            local_jsonl_by_node: dict[str, str] = {}
            for node in nodes:
                client = worker_clients[node.host]
                suffix = "" if len(nodes) == 1 else f"-{node.host}"
                remote_trace_archive = f"{worker_run}/dscodebench-trace-corpus{suffix}.tgz"
                base.run(
                    client,
                    f"tar -C {base.q(real_llm_trace_output_dir)} -czf {base.q(remote_trace_archive)} .",
                    timeout=120,
                )
                local_trace_archive = local_run / f"dscodebench-trace-corpus{suffix}.tgz"
                with client.open_sftp() as sftp:
                    sftp.get(remote_trace_archive, str(local_trace_archive))
                local_trace_dir = local_run / f"trace-corpus{suffix}"
                local_trace_dir.mkdir(parents=True, exist_ok=True)
                with tarfile.open(local_trace_archive, "r:gz") as tar:
                    tar.extractall(local_trace_dir)
                trace_jsonl = local_trace_dir / "dscodebench_trace_corpus.jsonl"
                if trace_jsonl.exists():
                    local_jsonl_by_node[node.host] = str(trace_jsonl)
            if local_jsonl_by_node:
                if len(nodes) == 1:
                    manifest["trace_corpus"]["local_jsonl"] = local_jsonl_by_node[nodes[0].host]
                else:
                    manifest["trace_corpus"]["local_jsonl_per_node"] = local_jsonl_by_node
                (local_run / "manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True))
        if any(fleet_metrics_paths.values()):
            per_node_metrics = {}
            for node in nodes:
                if not fleet_metrics_paths[node.host]:
                    continue
                metrics_text = base.get_text(
                    worker_clients[node.host], fleet_metrics_paths[node.host]
                )
                per_node_metrics[node.host] = json.loads(metrics_text)
                (local_run / f"fleet-metrics-{node.host}.json").write_text(
                    metrics_text, encoding="utf-8"
                )
                (local_run / f"fleet-resources-{node.host}.csv").write_text(
                    base.get_text(
                        worker_clients[node.host],
                        f"{worker_run}/fleet-resources.csv",
                    ),
                    encoding="utf-8",
                )
            fleet_resource_metrics = (
                next(iter(per_node_metrics.values()))
                if len(nodes) == 1
                else {"per_node": per_node_metrics}
            )
            (local_run / "fleet-metrics.json").write_text(
                json.dumps(fleet_resource_metrics, indent=2, sort_keys=True)
            )
        outcome = {
            "run_id": run_id,
            "scale": f"{workers_count}x{capacity}",
            "results": results,
            "status": "passed",
            "fleet_resource_metrics": fleet_resource_metrics,
        }
    except Exception as exc:
        error = f"{type(exc).__name__}: {exc}"
        (local_run / "error.txt").write_text(error)
        outcome = {"run_id": run_id, "scale": f"{workers_count}x{capacity}", "status": "failed", "error": error}
    finally:
        # 清理阶段使用新的 SSH 连接。长时间压测后旧连接可能断开，
        # 重新连接可以提高 cleanup 成功率。每个 worker 节点各自重连、各自清理。
        had_workers = bool(worker_clients)
        had_server = server is not None
        for stale in worker_clients.values():
            stale.close()
        if server:
            server.close()
        worker_clients = {}
        server = None
        if had_workers:
            for node in nodes:
                try: worker_clients[node.host] = base.connect(node.host, password)
                except Exception as exc: cleanup_errors.append(f"worker {node.host} reconnect: {exc}")
        if had_server:
            try: server = base.connect(base.SERVER_HOST, password)
            except Exception as exc: cleanup_errors.append(f"server reconnect: {exc}")
        for node in nodes:
            client = worker_clients.get(node.host)
            if not client:
                continue
            if fleet_supervisor_pids[node.host]:
                try:
                    base.stop_owned(
                        client,
                        fleet_supervisor_pids[node.host],
                        "/usr/bin/python3.12",
                        f"{worker_run}/worker_fleet_supervisor.py",
                    )
                except Exception as exc:
                    cleanup_errors.append(str(exc))
            else:
                for pid, config in reversed(worker_pids_by_node[node.host]):
                    try: base.stop_owned(client, pid, f"{worker_run}/bundle/uenv-worker", config)
                    except Exception as exc: cleanup_errors.append(str(exc))
            try: base.stop_owned(client, model_pids[node.host], "/usr/bin/python3.12", f"{worker_run}/{model_script}")
            except Exception as exc: cleanup_errors.append(str(exc))
            try:
                indexes = node_worker_indexes[node.host]
                node_ports = [ports[i] for i in indexes]
                node_obs_ports = [obs_ports[i] for i in indexes]
                base.assert_ports_free(client, node_ports + node_obs_ports + [base.MODEL_PORT], node.host)
            except Exception as exc:
                cleanup_errors.append(str(exc))
        if server:
            try: base.stop_owned(server, server_pid, base.SERVER_BIN, base.SERVER_BIN)
            except Exception as exc: cleanup_errors.append(str(exc))
            try:
                if before: base.assert_protected_unchanged(server, before)
                base.assert_port_free(server, base.SERVER_PORT, base.SERVER_HOST)
            except Exception as exc: cleanup_errors.append(str(exc))
        if cleanup_errors:
            (local_run / "cleanup-errors.txt").write_text("\n".join(cleanup_errors))
            print(f"[dscodebench_pressure:{workers_count}x{capacity}] cleanup ERRORS={cleanup_errors}", flush=True)
            outcome = {
                "run_id": run_id,
                "scale": f"{workers_count}x{capacity}",
                "status": "failed",
                "error": "cleanup failed: " + " | ".join(cleanup_errors),
            }
        else:
            print(f"[dscodebench_pressure:{workers_count}x{capacity}] cleanup complete", flush=True)
        for client in (*worker_clients.values(), server):
            if client: client.close()
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
    """DSCodeBench pressure command entry for real-Worker DSCodeBench pressure and scale pressure runs."""
    parser = argparse.ArgumentParser()
    parser.add_argument("--duration", type=int, default=30)
    parser.add_argument("--workers", type=int, default=1)
    parser.add_argument("--capacity", type=int, default=1)
    parser.add_argument(
        "--private-worker-port-range",
        default="",
        help="Explicit already-open START-END private port range; required for more than one Worker.",
    )
    parser.add_argument(
        "--mode",
        action="append",
        choices=MODES,
        help="Mode to run; repeat as needed. Defaults to all three modes.",
    )
    parser.add_argument("--max-steps", type=int, default=10)
    parser.add_argument(
        "--model-mode",
        choices=("real", "simulator"),
        default="real",
        help="DSCodeBench pressure defaults to the real Ark LLM; simulator must be explicitly selected.",
    )
    parser.add_argument(
        "--llm-config",
        default="",
        help="Mode-0600 OpenHands LLM JSON path on the Worker; required for --model-mode real.",
    )
    parser.add_argument(
        "--min-steps",
        type=int,
        default=3,
        help="Minimum real model/environment interactions before a passing Code episode can terminate.",
    )
    parser.add_argument(
        "--code-wrong-steps", type=int, default=2,
        help="Code model simulator returns wrong code this many times per task before returning the correct solution.",
    )
    parser.add_argument("--dataset-jsonl", required=True, help="Absolute DSCodeBench JSONL path on the Server source host.")
    parser.add_argument("--dataset-limit", type=int, default=8)
    parser.add_argument("--dataset-offset", type=int, default=0)
    parser.add_argument("--exact-batches", type=int, default=0)
    parser.add_argument(
        "--episode-batch-size", type=int, default=0,
        help="Episodes per ExecuteBatch; defaults to the Worker count.",
    )
    parser.add_argument(
        "--concurrent-batches", type=int, default=0,
        help="ExecuteBatch RPCs allowed in flight; defaults to Worker capacity.",
    )
    parser.add_argument("--registration-timeout", type=int, default=180)
    parser.add_argument("--batch-timeout", type=int, default=180)
    parser.add_argument("--fleet-supervisor-threshold", type=int, default=16)
    parser.add_argument("--simulator-seed", type=int, default=20260720)
    parser.add_argument("--simulator-mode", choices=("template", "trace_replay"), default="template")
    parser.add_argument("--trace-corpus-path", default="")
    parser.add_argument("--trace-sampling-strategy", choices=("round_robin_episode",), default="round_robin_episode")
    parser.add_argument("--min-scale-episode-waves", type=int, default=10)
    parser.add_argument("--plugin-ready-timeout-seconds", type=int, default=2)
    parser.add_argument("--worker-register-max-attempts", type=int, default=5)
    parser.add_argument("--worker-register-retry-backoff-ms", type=int, default=200)
    parser.add_argument(
        "--code-python",
        default="",
        help="Explicit Python interpreter for the Code plugin evaluator on the Worker host.",
    )
    parser.add_argument(
        "--acceptance-purpose",
        choices=("dscodebench_pressure-real-llm", "worker-scale", "single-worker-diagnostic"),
        default="dscodebench_pressure-real-llm",
    )
    parser.add_argument("--artifacts", type=Path, default=Path.cwd() / "distributed-gate-artifacts")
    base.add_runtime_arguments(parser, require_code_plugin=True)
    args = parser.parse_args()
    base.configure_from_args(args)
    if not 1 <= args.worker_register_max_attempts <= 100:
        parser.error("--worker-register-max-attempts must be between 1 and 100")
    if not 10 <= args.worker_register_retry_backoff_ms <= 10_000:
        parser.error("--worker-register-retry-backoff-ms must be between 10 and 10000")
    if args.episode_batch_size < 0 or args.concurrent_batches < 0:
        parser.error("batch size/concurrency must be zero (auto) or positive")
    allowed_exposed_ports = {5432, 6379, 8000, 8077, 8088, 8099, 8777, 8888}
    exposed_ports = {
        "isolated server": base.SERVER_PORT,
        "model endpoint": base.MODEL_PORT,
    }
    if args.workers == 1:
        exposed_ports["single Worker"] = base.WORKER_PORT
    for label, port in exposed_ports.items():
        if port not in allowed_exposed_ports:
            raise SystemExit(f"{label} port {port} is outside the explicitly allowed cloud ports")
    if base.SERVER_PORT in base.PROTECTED_PORTS:
        raise SystemExit("isolated server port must not overlap a protected production port")
    if args.workers <= 0 or args.capacity <= 0:
        raise SystemExit("--workers and --capacity must be positive")
    if args.max_steps <= 0:
        raise SystemExit("--max-steps must be positive")
    if args.min_steps <= 0 or args.min_steps > args.max_steps:
        raise SystemExit("--min-steps must be positive and no greater than --max-steps")
    if args.model_mode == "real" and not args.llm_config:
        raise SystemExit("--llm-config is required for --model-mode real")
    if not args.dataset_jsonl.startswith("/"):
        raise SystemExit("--dataset-jsonl must be an absolute path on the Server host")
    if args.dataset_limit <= 0 or args.dataset_offset < 0:
        raise SystemExit("--dataset-limit must be positive and --dataset-offset non-negative")
    if args.exact_batches < 0 or args.registration_timeout <= 0 or args.batch_timeout <= 0:
        raise SystemExit("batch and timeout arguments must be non-negative/positive")
    if args.fleet_supervisor_threshold < 2:
        raise SystemExit("invalid fleet supervisor threshold")
    if args.model_mode == "simulator" and args.simulator_mode == "trace_replay" and not args.trace_corpus_path.strip():
        raise SystemExit("--simulator-mode trace_replay requires --trace-corpus-path")
    if args.model_mode == "real" and args.trace_corpus_path.strip():
        raise SystemExit("--trace-corpus-path is only used by simulator trace_replay runs")
    if not 1 <= args.plugin_ready_timeout_seconds <= 300:
        raise SystemExit("--plugin-ready-timeout-seconds must be between 1 and 300")
    modes = tuple(args.mode or MODES)
    if args.acceptance_purpose == "dscodebench_pressure-real-llm" and args.model_mode != "real":
        raise SystemExit("DSCodeBench pressure acceptance requires --model-mode real")
    if args.acceptance_purpose == "worker-scale":
        if args.model_mode != "simulator" or args.exact_batches <= 0:
            raise SystemExit("worker-scale requires simulator and --exact-batches > 0")
        if args.simulator_mode != "trace_replay":
            raise SystemExit("worker-scale requires DSCodeBench --simulator-mode trace_replay")
        if args.workers < 1024:
            raise SystemExit("worker-scale requires at least 1024 Workers")
        resolved_batch_size = args.episode_batch_size or args.workers
        required_episodes = args.workers * args.capacity * args.min_scale_episode_waves
        if resolved_batch_size * args.exact_batches < required_episodes:
            raise SystemExit(
                "worker-scale total episodes must be at least "
                f"workers * capacity * {args.min_scale_episode_waves} = {required_episodes}"
            )
        resolved_concurrent_batches = args.concurrent_batches or args.exact_batches
        if resolved_concurrent_batches < args.exact_batches:
            raise SystemExit(
                "worker-scale requires --concurrent-batches >= --exact-batches "
                "so all planned batches are submitted before collecting results"
            )
    if args.acceptance_purpose == "single-worker-diagnostic":
        if args.workers != 1 or args.capacity <= 1:
            raise SystemExit("single-worker-diagnostic requires --workers 1 and --capacity > 1")
        if args.model_mode != "simulator" or args.exact_batches <= 0:
            raise SystemExit("single-worker-diagnostic requires simulator and --exact-batches > 0")
        resolved_batch_size = args.episode_batch_size or args.workers
        required_episodes = args.capacity * args.min_scale_episode_waves
        if resolved_batch_size * args.exact_batches < required_episodes:
            raise SystemExit(
                "single-worker-diagnostic total episodes must be at least "
                f"capacity * {args.min_scale_episode_waves} = {required_episodes}"
            )
        resolved_concurrent_batches = args.concurrent_batches or args.exact_batches
        if resolved_concurrent_batches < args.exact_batches:
            raise SystemExit("single-worker-diagnostic requires --concurrent-batches >= --exact-batches")
    if args.code_wrong_steps < 0 or args.code_wrong_steps >= args.max_steps:
        raise SystemExit("--code-wrong-steps must be >=0 and smaller than --max-steps")
    password = os.environ.get("UENV_PASS")
    if not password: raise SystemExit("UENV_PASS is required")
    args.artifacts.mkdir(parents=True, exist_ok=True)
    worker_ports = _parse_private_port_range(args.private_worker_port_range, args.workers)
    summary = run_scale(
        args.workers,
        args.capacity,
        worker_ports,
        modes,
        args.duration,
        args.artifacts,
        password,
        args.max_steps,
        args.code_wrong_steps,
        args.min_steps,
        args.model_mode,
        args.llm_config,
        args.dataset_jsonl,
        args.dataset_limit,
        args.dataset_offset,
        args.exact_batches,
        args.registration_timeout,
        args.batch_timeout,
        args.fleet_supervisor_threshold,
        args.simulator_seed,
        args.simulator_mode,
        args.trace_corpus_path,
        args.trace_sampling_strategy,
        args.plugin_ready_timeout_seconds,
        args.worker_register_max_attempts,
        args.worker_register_retry_backoff_ms,
        args.episode_batch_size or args.workers,
        args.concurrent_batches or (
            args.exact_batches
            if args.acceptance_purpose in {"worker-scale", "single-worker-diagnostic"} and args.exact_batches > 0
            else args.capacity
        ),
        args.acceptance_purpose,
        args.code_python,
    )
    summaries = [summary]
    summary_path = args.artifacts / f"dscodebench-pressure-summary-{time.strftime('%Y%m%d-%H%M%S')}.json"
    summary_path.write_text(json.dumps(summaries, indent=2, sort_keys=True))
    print(f"[dscodebench_pressure] summary={summary_path}", flush=True)
    return 0 if summary["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
