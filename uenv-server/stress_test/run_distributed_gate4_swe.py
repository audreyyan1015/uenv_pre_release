#!/usr/bin/env python3
"""分布式 Gate4：真实 SWE/OpenHands 容器压测。

运行位置说明：
- 隔离 server 启动在 8.130.75.157:8099。
- SWE worker、OpenHands agent 和 Docker 容器启动在 8.130.86.71。
- 默认顺序跑 1 个容器并发、2 个容器并发。
- 不复用正式 server，也不停止正式 server。
"""

from __future__ import annotations

import argparse
import io
import json
import os
from pathlib import Path
import random
import re
import sys
import tarfile
import tempfile
import time
import uuid

import distributed_stress_runtime as base


# SWE worker 会打开 runtime gateway，OpenHands agent 通过这个 gateway 执行任务。
GATEWAY_PORT = 8777

# OpenHands runner 自己的 API 和健康检查端口，只监听 worker 本机。
AGENT_API_PORT = 18004
AGENT_HEALTH_PORT = 18005

DEFAULT_INSTANCE_ID = "astropy__astropy-7166"
DEFAULT_KNOWN_IMAGE = "swebench/sweb.eval.x86_64.astropy_1776_astropy-7166:latest"
DEFAULT_KNOWN_IMAGE_ID = "sha256:6909381901b865b904d9cfce69e412f659de0dc1e0454abb052c88b116654a83"
OPENHANDS_PYTHON = "/usr/bin/python3.12"
COMMON_SOURCE = Path(__file__).with_name("stress_test_common.py").read_text(encoding="utf-8")
FLEET_SUPERVISOR_SOURCE = Path(__file__).with_name("worker_fleet_supervisor.py").read_text(encoding="utf-8")


def put_worker_config_archive(worker, worker_run: str, documents: dict[str, tuple[str, int]], run_id: str) -> None:
    """Upload many Gate4 Worker YAMLs as one archive for 1024+ Worker scale runs."""
    with tempfile.NamedTemporaryFile(prefix=f"{run_id}-gate4-worker-configs-", suffix=".tgz", delete=False) as tmp:
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


RESOURCE_MONITOR = r'''#!/usr/bin/env python3
# 这个脚本会临时写到 worker 机器上运行。
# 它每 0.1 秒记录一次内存、load、Docker 正在运行的容器数量。
# Gate4 用它确认真实容器并发确实达到了 requested concurrency。
import argparse
import json
from pathlib import Path
import subprocess
import time

p = argparse.ArgumentParser(); p.add_argument("--output", required=True)
args = p.parse_args()
target = Path(args.output)
while True:
    mem = {}
    for line in Path("/proc/meminfo").read_text().splitlines():
        key, value = line.split(":", 1)
        if key in {"MemTotal", "MemAvailable"}:
            mem[key] = int(value.strip().split()[0])
    containers = subprocess.run(
        ["docker", "ps", "-q"], text=True, capture_output=True, check=False
    ).stdout.splitlines()
    with target.open("a", encoding="utf-8") as out:
        out.write(json.dumps({
            "ts": time.time(), "load": list(Path("/proc/loadavg").read_text().split()[:3]),
            "mem_total_kib": mem.get("MemTotal", 0),
            "mem_available_kib": mem.get("MemAvailable", 0),
            "running_containers": len(containers),
        }) + "\n")
    time.sleep(.1)
'''


LLM_PREFLIGHT = r'''#!/usr/bin/env python3
import argparse
import json
from pathlib import Path
import stat

parser = argparse.ArgumentParser()
parser.add_argument("--config", required=True)
args = parser.parse_args()
path = Path(args.config)
mode = stat.S_IMODE(path.stat().st_mode)
if mode & 0o077:
    raise SystemExit(f"LLM config permissions must be owner-only, got {mode:o}")

from benchmarks.utils.llm_config import load_llm_config
llm = load_llm_config(path)
raw = json.loads(path.read_text(encoding="utf-8"))
base_url = str(raw.get("base_url") or "").rstrip("/")
api_key = str(raw.get("api_key") or "")
model = str(raw.get("model") or getattr(llm, "model", "") or "")
if not base_url or not api_key or not model:
    raise SystemExit("LLM config must contain non-empty base_url, api_key and model")
placeholder_keys = {
    "replace_me", "changeme", "change_me", "your_api_key", "api_key",
    "placeholder", "dummy", "test",
}
if api_key.strip().lower() in placeholder_keys:
    raise SystemExit("LLM config contains a placeholder api_key")

# Exercise the exact OpenHands SDK transport used by the real agent.  A raw HTTP
# request can pass while LiteLLM/OpenHands model routing still rejects the config.
from openhands.sdk import Message, TextContent
response = llm.completion([
    Message(role="user", content=[TextContent(text="Reply with OK.")]),
], temperature=0, max_tokens=4)
content = getattr(getattr(response, "message", None), "content", ())
if not any(str(getattr(item, "text", "")).strip() for item in content):
    raise SystemExit("minimal authenticated OpenHands LLM call returned no text")
raw_response = getattr(response, "raw_response", None)
print(json.dumps({
    "schema_valid": True,
    "auth_and_minimal_call_valid": True,
    "transport": "openhands.sdk.LLM.completion",
    "model": model,
    "base_url": base_url,
    "response_id_present": bool(getattr(raw_response, "id", None)),
}, sort_keys=True))
'''


LLM_SIMULATOR = r'''#!/usr/bin/env python3
import argparse
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import random
import re
import threading
import time
from urllib.parse import urlparse

parser = argparse.ArgumentParser()
parser.add_argument("--port", type=int, required=True)
parser.add_argument("--latency-ms", type=float, default=250)
parser.add_argument("--latency-mean-ms", type=float, default=250)
parser.add_argument("--latency-std-ms", type=float, default=75)
parser.add_argument("--latency-min-ms", type=float, default=0)
parser.add_argument("--latency-max-ms", type=float, default=2000)
parser.add_argument("--zero-latency", action="store_true")
parser.add_argument("--model", default="openai/uenv-swe-simulator")
parser.add_argument("--simulator-mode", choices=("template", "trace_replay"), default="template")
parser.add_argument("--trace-corpus-path", default="")
parser.add_argument("--trace-sampling-strategy", choices=("instance_then_turn", "turn_only"), default="instance_then_turn")
parser.add_argument("--wrong-steps-mean", type=float, default=2)
parser.add_argument("--wrong-steps-std", type=float, default=1)
parser.add_argument("--wrong-steps-min", type=int, default=0)
parser.add_argument("--wrong-steps-max", type=int, default=5)
parser.add_argument("--repair-success-rate", type=float, default=0.35)
parser.add_argument("--repair-style", choices=("plausible_patch", "noisy_patch", "noop"), default="plausible_patch")
parser.add_argument("--seed", type=int, default=20260720)
args = parser.parse_args()
calls = 0
tokenization_calls = 0
trace_replay_hits = 0
trace_replay_misses = 0
profiles_by_task = {}
attempts_by_task = {}
observed_latencies_ms = []
lock = threading.Lock()


def load_trace_corpus(path_text):
    if not path_text:
        return []
    path = Path(path_text)
    files = []
    if path.is_dir():
        files.extend(sorted(path.rglob("llm_trace_corpus_episode.json")))
        files.extend(sorted(path.rglob("*.jsonl")))
    elif path.is_file():
        files.append(path)
    episodes = []
    for file in files:
        try:
            if file.suffix == ".jsonl":
                for line in file.read_text(encoding="utf-8").splitlines():
                    if line.strip():
                        episodes.append(json.loads(line))
            else:
                document = json.loads(file.read_text(encoding="utf-8"))
                if isinstance(document, list):
                    episodes.extend(document)
                else:
                    episodes.append(document)
        except Exception as exc:
            episodes.append({"corpus_load_error": str(exc), "path": str(file), "turns": []})
    return episodes


TRACE_CORPUS = load_trace_corpus(args.trace_corpus_path)


def clamp(value, lower, upper):
    return max(lower, min(upper, value))


def deterministic_rng(label):
    digest = hashlib.sha256(f"{args.seed}:{label}".encode("utf-8")).hexdigest()
    return random.Random(int(digest[:16], 16))


def extract_text(value):
    if isinstance(value, str):
        return value
    if isinstance(value, list):
        return "\n".join(extract_text(item) for item in value)
    if isinstance(value, dict):
        if "text" in value:
            return str(value["text"])
        if "content" in value:
            return extract_text(value["content"])
        return "\n".join(extract_text(item) for item in value.values())
    return ""


def request_fingerprint(document):
    messages = document.get("messages", [])
    user_texts = [
        extract_text(message.get("content", ""))
        for message in messages
        if isinstance(message, dict) and message.get("role") in {"user", "system"}
    ]
    seed_text = "\n".join(user_texts[:2]) or json.dumps(document, sort_keys=True)[:4096]
    instance_match = re.search(r"([A-Za-z0-9_.-]+__[A-Za-z0-9_.-]+-\d+)", seed_text)
    instance_id = instance_match.group(1) if instance_match else "unknown-instance"
    prompt_hash = hashlib.sha256(seed_text.encode("utf-8", errors="ignore")).hexdigest()[:12]
    return f"{instance_id}:{prompt_hash}", instance_id


def profile_for(task_key, instance_id):
    rng = deterministic_rng(task_key)
    wrong = int(round(rng.normalvariate(args.wrong_steps_mean, args.wrong_steps_std)))
    wrong = clamp(wrong, args.wrong_steps_min, args.wrong_steps_max)
    success = rng.random() < args.repair_success_rate
    return {
        "instance_id": instance_id,
        "wrong_steps": wrong,
        "repair_success": success,
        "repair_style": args.repair_style,
    }


def sample_latency(label):
    if args.zero_latency:
        return 0.0
    rng = deterministic_rng(f"{label}:latency")
    latency = rng.normalvariate(args.latency_mean_ms, args.latency_std_ms)
    return float(clamp(latency, args.latency_min_ms, args.latency_max_ms))


def choose_trace_turn(instance_id, attempt):
    candidates = [
        episode for episode in TRACE_CORPUS
        if isinstance(episode, dict) and isinstance(episode.get("turns"), list)
    ]
    if args.trace_sampling_strategy == "instance_then_turn" and instance_id != "unknown-instance":
        matched = [episode for episode in candidates if str(episode.get("instance_id")) == instance_id]
        if matched:
            candidates = matched
    if not candidates:
        return None, None
    episode = candidates[(attempt - 1) % len(candidates)]
    turns = episode.get("turns") or []
    if not turns:
        return episode, None
    turn = turns[min(max(attempt - 1, 0), len(turns) - 1)]
    return episode, turn


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


class Handler(BaseHTTPRequestHandler):
    def send_json(self, status, document):
        body = json.dumps(document, separators=(",", ":")).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        parsed = urlparse(self.path)
        if parsed.path == "/health":
            self.send_json(200, {"ok": True, "model": args.model})
            return
        if parsed.path == "/stats":
            with lock:
                profiles = dict(profiles_by_task)
                attempts = dict(attempts_by_task)
            wrong_steps = [int(profile["wrong_steps"]) for profile in profiles.values()]
            latencies = list(observed_latencies_ms)
            self.send_json(200, {
                "calls": calls,
                "tokenization_calls": tokenization_calls,
                "trace_replay_hits": trace_replay_hits,
                "trace_replay_misses": trace_replay_misses,
                "model": args.model,
                "latency_ms": args.latency_ms,
                "observed_latency_ms": numeric_stats(latencies),
                "simulator": {
                    "kind": "swebench-trace-replay" if args.simulator_mode == "trace_replay" else "swebench-wrong-steps-repair-quality",
                    "mode": args.simulator_mode,
                    "seed": args.seed,
                    "trace_corpus_path": args.trace_corpus_path,
                    "trace_episode_count": len(TRACE_CORPUS),
                    "trace_sampling_strategy": args.trace_sampling_strategy,
                    "zero_latency": args.zero_latency,
                    "latency_distribution_ms": {
                        "mean": args.latency_mean_ms,
                        "std": args.latency_std_ms,
                        "min": args.latency_min_ms,
                        "max": args.latency_max_ms,
                    },
                    "wrong_steps": {
                        "mean": args.wrong_steps_mean,
                        "std": args.wrong_steps_std,
                        "min": args.wrong_steps_min,
                        "max": args.wrong_steps_max,
                        "sampled": numeric_stats(wrong_steps),
                    },
                    "repair_success_rate": args.repair_success_rate,
                    "repair_style": args.repair_style,
                },
                "profiles": profiles,
                "attempts": attempts,
            })
            return
        self.send_error(404)

    def do_POST(self):
        global calls, tokenization_calls, trace_replay_hits, trace_replay_misses
        parsed_path = urlparse(self.path).path
        size = int(self.headers.get("content-length", "0"))
        raw_body = b""
        if size:
            raw_body = self.rfile.read(size)
        if parsed_path.endswith("/tokenization"):
            tokenization_calls += 1
            try:
                document = json.loads(raw_body.decode() or "{}")
            except Exception:
                document = {}
            texts = document.get("text", [])
            if isinstance(texts, str):
                texts = [texts]
            if not isinstance(texts, list):
                self.send_json(400, {"error": {"message": "text must be a string or list"}})
                return
            self.send_json(200, {
                "data": [
                    {"index": index, "token_ids": [1001 + index]}
                    for index, _text in enumerate(texts)
                ],
                "model": args.model,
            })
            return
        if not parsed_path.endswith("/chat/completions"):
            self.send_error(404)
            return
        try:
            document = json.loads(raw_body.decode() or "{}")
        except Exception:
            document = {}
        task_key, instance_id = request_fingerprint(document)
        with lock:
            profile = profiles_by_task.setdefault(task_key, profile_for(task_key, instance_id))
            attempt = attempts_by_task.get(task_key, 0) + 1
            attempts_by_task[task_key] = attempt
        calls += 1
        trace_episode = trace_turn = None
        if args.simulator_mode == "trace_replay":
            trace_episode, trace_turn = choose_trace_turn(instance_id, attempt)
        if trace_turn:
            trace_replay_hits += 1
            content = str(trace_turn.get("assistant_output") or trace_turn.get("text") or "")
            response_ids = [int(value) for value in trace_turn.get("response_ids", []) if isinstance(value, int)]
            logprobs = [
                float(value) for value in trace_turn.get("logprobs", [])
                if isinstance(value, (int, float))
            ]
            response_id = response_ids[0] if response_ids else 1000 + calls
            trace_latency = float(trace_turn.get("latency_ms") or 0.0)
            sleep_ms = 0.0 if args.zero_latency else (
                trace_latency if trace_latency > 0 else sample_latency(f"{task_key}:{attempt}:trace")
            )
            phase = "trace_replay"
            simulated_reward = float((trace_episode or {}).get("result", {}).get("reward", 0.0) or 0.0)
            trace_source = {
                "corpus_run_id": (trace_episode or {}).get("run_id", ""),
                "corpus_instance_id": (trace_episode or {}).get("instance_id", ""),
                "turn_index": trace_turn.get("turn_index"),
            }
        else:
            if args.simulator_mode == "trace_replay":
                trace_replay_misses += 1
            response_ids = []
            logprobs = []
            sleep_ms = sample_latency(f"{task_key}:{attempt}:miss")
            trace_source = {}
            if attempt <= int(profile["wrong_steps"]):
                content = (
                    "I will first inspect the repository and reproduce the failure before "
                    "editing. Do not guess a patch yet; gather the relevant files, tests, "
                    "and stack traces, then continue."
                )
                phase = "wrong_step"
                simulated_reward = 0.0
            elif profile["repair_success"] and args.repair_style != "noop":
                content = (
                    "Apply the smallest repository-local fix for this SWE-bench instance. "
                    "Focus on the failing behavior described in the issue, update only the "
                    "minimal source file, run the most relevant regression test, and finish "
                    "with the exact files changed and test result."
                )
                phase = "repair_success"
                simulated_reward = 1.0
            elif args.repair_style == "noisy_patch":
                content = (
                    "Make a plausible but conservative change near the suspected failure "
                    "site, then run the relevant test. If the test still fails, summarize "
                    "the remaining gap instead of broadening the patch."
                )
                phase = "repair_low_quality"
                simulated_reward = 0.25
            else:
                content = (
                    "The current bounded simulator iteration cannot produce a confident "
                    "repair. Leave the repository unchanged after inspection and report "
                    "the unresolved failure mode."
                )
                phase = "repair_failure"
                simulated_reward = 0.0
        with lock:
            observed_latencies_ms.append(float(sleep_ms))
        time.sleep(sleep_ms / 1000)
        if not response_ids:
            response_ids = [1000 + calls]
        if not logprobs:
            logprobs = [-0.1] * len(response_ids)
        self.send_json(200, {
            "id": f"chatcmpl-uenv-swe-sim-{calls}",
            "object": "chat.completion",
            "created": int(time.time()),
            "model": args.model,
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": content},
                "finish_reason": "stop",
                "logprobs": {"content": [
                    {"token": content, "token_id": token_id, "logprob": logprob}
                    for token_id, logprob in zip(response_ids, logprobs)
                ]},
            }],
            "usage": {"prompt_tokens": 16, "completion_tokens": 32, "total_tokens": 48},
            "uenv_response_ids": response_ids,
            "uenv_model_version": {
                "rollout_param_version": 0,
                "rollout_policy_version": args.model,
            },
            "uenv_simulator_profile": {
                "task_key": task_key,
                "instance_id": instance_id,
                "attempt": attempt,
                "wrong_steps": profile["wrong_steps"],
                "phase": phase,
                "repair_success": profile["repair_success"],
                "simulated_reward": simulated_reward,
                "simulator_mode": args.simulator_mode,
                "trace_source": trace_source,
            },
        })

    def log_message(self, *_args):
        pass


ThreadingHTTPServer(("127.0.0.1", args.port), Handler).serve_forever()
'''


SWE_CLIENT = r'''#!/usr/bin/env python3
# 这个脚本会临时写到 server 机器上运行。
# 它等待 SWE worker 注册后，向 AdapterCore ExecuteBatch 提交真实 SWE episode。
import argparse
import concurrent.futures
import json
import sys
import time
import uuid
import grpc
from uenv.v1 import adapter_core_pb2, scheduler_pb2
import stress_test_common as stress_common

if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(line_buffering=True)

parser = argparse.ArgumentParser()
parser.add_argument("--server", required=True)
parser.add_argument("--worker-id", required=True)
parser.add_argument("--worker-prefix", default="")
parser.add_argument("--expected-workers", type=int, default=1)
parser.add_argument("--worker-capacity", type=int, default=1)
parser.add_argument("--total-episodes", type=int, default=0)
parser.add_argument("--episode-batch-size", type=int, default=0)
parser.add_argument("--episode-offset", type=int, default=0)
parser.add_argument("--registration-timeout", type=int, default=120)
parser.add_argument("--batch-timeout", type=int, default=960)
parser.add_argument("--run-id", required=True)
parser.add_argument("--driver", required=True)
parser.add_argument("--catalog", required=True)
parser.add_argument("--output", required=True)
parser.add_argument("--concurrency", type=int, required=True)
parser.add_argument("--mode", choices=("gold", "llm"), required=True)
parser.add_argument(
    "--parallel-mode",
    choices=("sync", "one_step_off_policy", "fully_async"),
    required=True,
)
parser.add_argument("--max-steps", type=int, required=True)
parser.add_argument("--openhands-max-iterations", type=int, required=True)
parser.add_argument("--llm-config", default="")
parser.add_argument("--instance-ids-json", required=True)
args = parser.parse_args()
instance_ids = json.loads(args.instance_ids_json)
if not isinstance(instance_ids, list) or not instance_ids:
    raise SystemExit("--instance-ids-json must be a non-empty JSON list")
if args.concurrency <= 0:
    raise SystemExit("--concurrency must be positive")
if args.expected_workers <= 0:
    raise SystemExit("--expected-workers must be positive")
if args.worker_capacity <= 0:
    raise SystemExit("--worker-capacity must be positive")
if args.total_episodes < 0 or args.episode_batch_size < 0:
    raise SystemExit("--total-episodes and --episode-batch-size must be non-negative")
if args.episode_offset < 0:
    raise SystemExit("--episode-offset must be non-negative")

channel = grpc.insecure_channel(args.server, options=[
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

# 只等待本次启动的 worker_id/prefix。这样不会误把其它 worker 当成本次压测 worker。
deadline = time.monotonic() + args.registration_timeout
registered = []
owned = []
while time.monotonic() < deadline:
    try:
        if not health(adapter_core_pb2.HealthCheckRequest(), timeout=3).ok:
            time.sleep(1)
            continue
        response = list_workers(scheduler_pb2.ListWorkersRequest(), timeout=3)
        registered = [worker.worker_id for worker in response.workers]
        if args.worker_prefix:
            owned = [worker_id for worker_id in registered if worker_id.startswith(args.worker_prefix)]
            if len(owned) == args.expected_workers:
                break
        elif args.worker_id in registered:
            owned = [args.worker_id]
            break
    except grpc.RpcError:
        pass
    time.sleep(1)
else:
    expected = args.worker_prefix or args.worker_id
    raise SystemExit(f"workers did not register: expected={expected} count={args.expected_workers} registered={registered}")

def env_config(instance_id):
    # 这里生成的 env_config_json 会交给 SWE worker。
    # driver 是 official runner，catalog 是 verified.json。
    # mode=llm 表示真实 OpenHands agent 多轮执行；mode=gold 只用于必要的基线排查。
    return stress_common.swe_openhands_env_payload(
        instance_id=instance_id,
        benchmark_variant="verified",
        command_mode="FullShell",
        mode=args.mode,
        agent_pool_id="openhands-distributed-smoke",
        driver_entrypoint=args.driver,
        workspace_dir="/opt/openhands/benchmarks",
        llm_config_path=args.llm_config,
        max_iterations=args.openhands_max_iterations,
        instances_catalog=args.catalog,
        pool_selector="openhands-distributed-smoke",
    )

target_episodes = args.total_episodes or args.concurrency
episode_batch_size = args.episode_batch_size or args.concurrency
if episode_batch_size <= 0:
    raise SystemExit("--episode-batch-size resolved to zero")
submitted = completed = failed = protocol_errors = rpc_error_episodes = 0
rpc_error_details = []
latencies = []
all_results = []
instance_usage = {}
started = time.monotonic()
batch_specs = []
planned = 0
while planned < target_episodes:
    current_size = min(episode_batch_size, target_episodes - planned)
    batch_id = str(uuid.uuid4())
    samples = []
    for index in range(current_size):
        ordinal = args.episode_offset + planned + index
        instance_id = instance_ids[ordinal % len(instance_ids)]
        instance_usage[instance_id] = instance_usage.get(instance_id, 0) + 1
        samples.append(stress_common.make_sample_envelope(
            adapter_core_pb2,
            batch_id=batch_id,
            sample_index=ordinal,
            env_type="swe",
            parallel_mode=args.parallel_mode,
            env_config=env_config(instance_id),
            reward_config=stress_common.swe_reward_config(),
            sample_context={
                "stress_run_id": args.run_id,
                "environment": "swe_openhands",
                "gate4_concurrency": args.concurrency,
                "mode": args.mode,
                "parallel_mode": args.parallel_mode,
                "max_steps": args.max_steps,
                "openhands_max_iterations": args.openhands_max_iterations,
                "dataset": "SWE-bench Verified",
                "instance_id": instance_id,
                "episode_ordinal": ordinal,
            },
            timeout_seconds=args.batch_timeout,
            max_steps=args.max_steps,
        ))
    batch_specs.append((len(batch_specs), batch_id, current_size, samples))
    planned += current_size


def execute_batch(batch_spec):
    batch_index, batch_id, current_size, samples = batch_spec
    batch_started = time.monotonic()
    try:
        response = execute(
            adapter_core_pb2.ExecuteBatchRequest(
                request_id=batch_id, batch_id=batch_id, samples=samples
            ),
            timeout=args.batch_timeout + 60,
        )
    except grpc.RpcError as exc:
        return {
            "batch_index": batch_index,
            "batch_id": batch_id,
            "submitted": current_size,
            "completed": 0,
            "failed": current_size,
            "protocol_errors": 0,
            "rpc_error_episodes": current_size,
            "rpc_error": {
                "code": exc.code().name if exc.code() else "UNKNOWN",
                "details": exc.details() or "",
            },
            "latency_ms": (time.monotonic() - batch_started) * 1000,
            "results": [],
        }
    latency_ms = (time.monotonic() - batch_started) * 1000
    parsed_results = [stress_common.sample_result_dict(result) for result in response.results]
    batch_completed = 0
    batch_failed = 0
    batch_protocol_errors = 0
    for result in response.results:
        if result.status in {"completed", "success"}:
            batch_completed += 1
        else:
            batch_failed += 1
    for parsed in parsed_results:
        if not parsed.get("training_trace_valid", True):
            batch_protocol_errors += 1
    missing_results = max(0, current_size - len(response.results))
    return {
        "batch_index": batch_index,
        "batch_id": batch_id,
        "submitted": current_size,
        "completed": batch_completed,
        "failed": batch_failed + missing_results,
        "protocol_errors": batch_protocol_errors + missing_results,
        "rpc_error_episodes": 0,
        "latency_ms": latency_ms,
        "results": parsed_results,
        "result_count": len(response.results),
        "expected_result_count": current_size,
    }


submit_started = time.monotonic()
max_in_flight_batches = len(batch_specs)
with concurrent.futures.ThreadPoolExecutor(max_workers=max_in_flight_batches) as executor:
    future_to_batch = {executor.submit(execute_batch, spec): spec for spec in batch_specs}
    client_submit_seconds = time.monotonic() - submit_started
    for future in concurrent.futures.as_completed(future_to_batch):
        _, batch_id, current_size, _ = future_to_batch[future]
        try:
            batch_result = future.result()
        except Exception as exc:
            batch_result = {
                "batch_id": batch_id,
                "submitted": current_size,
                "completed": 0,
                "failed": current_size,
                "protocol_errors": 0,
                "rpc_error_episodes": current_size,
                "rpc_error": {"code": type(exc).__name__, "details": str(exc)},
                "latency_ms": 0,
                "results": [],
            }
        submitted += int(batch_result.get("submitted", 0))
        completed += int(batch_result.get("completed", 0))
        failed += int(batch_result.get("failed", 0))
        protocol_errors += int(batch_result.get("protocol_errors", 0))
        rpc_error_episodes += int(batch_result.get("rpc_error_episodes", 0))
        latencies.append(float(batch_result.get("latency_ms", 0)))
        all_results.extend(batch_result.get("results", []))
        if "rpc_error" in batch_result and len(rpc_error_details) < 10:
            detail = dict(batch_result["rpc_error"])
            detail["batch_id"] = batch_result.get("batch_id", batch_id)
            detail["sample_count"] = current_size
            rpc_error_details.append(detail)
elapsed = time.monotonic() - started
resolved = completed + failed + rpc_error_episodes
document = stress_common.gate4_swe_result_document(
    run_id=args.run_id,
    server=args.server,
    worker_id=args.worker_id,
    registered_workers=registered,
    instance_id=instance_ids[0],
    mode=args.mode,
    parallel_mode=args.parallel_mode,
    concurrency=args.concurrency,
    max_steps=args.max_steps,
    openhands_max_iterations=args.openhands_max_iterations,
    elapsed_seconds=elapsed,
    results=all_results,
)
document["scale"] = {
    "registered_worker_count": len(owned),
    "expected_workers": args.expected_workers,
    "worker_prefix": args.worker_prefix,
    "parallel_mode": args.parallel_mode,
    "total_episodes": target_episodes,
    "episode_offset": args.episode_offset,
    "episode_ordinal_start": args.episode_offset,
    "episode_ordinal_end_exclusive": args.episode_offset + submitted,
    "episode_batch_size": episode_batch_size,
    "batch_count": len(batch_specs),
    "planned_batches": len(batch_specs),
    "submission_strategy": "submit_all_batches_then_collect",
    "submitted_to_uenv": submitted,
    "client_submit_seconds": client_submit_seconds,
    "worker_capacity": args.worker_capacity,
    "worker_slots": args.expected_workers * args.worker_capacity,
    "target_backlog_ratio": target_episodes / max(1, args.expected_workers * args.worker_capacity),
    "submitted": submitted,
    "completed": completed,
    "failed": failed,
    "protocol_errors": protocol_errors,
    "rpc_error_episodes": rpc_error_episodes,
    "resolved_throughput_eps": resolved / elapsed if elapsed > 0 else 0.0,
    "completed_throughput_eps": completed / elapsed if elapsed > 0 else 0.0,
    "submitted_throughput_eps": submitted / elapsed if elapsed > 0 else 0.0,
    "client_submit_rate_eps": submitted / client_submit_seconds if client_submit_seconds > 0 else 0.0,
    "rpc_error_details": rpc_error_details,
    "batch_latency_ms": {
        "count": len(latencies),
        "min": min(latencies) if latencies else 0,
        "max": max(latencies) if latencies else 0,
        "mean": sum(latencies) / len(latencies) if latencies else 0,
    },
}
document["dataset"] = {
    "name": "SWE-bench Verified",
    "selected_instance_ids": instance_ids,
    "unique_instance_count": len(set(instance_ids)),
    "submitted_episodes": submitted,
    "reuse_factor": submitted / len(set(instance_ids)) if instance_ids else 0.0,
    "instance_usage_top20": dict(sorted(instance_usage.items(), key=lambda item: (-item[1], item[0]))[:20]),
    "sampling_unit": "episode",
}
with open(args.output, "w", encoding="utf-8") as destination:
    json.dump(document, destination, indent=2, sort_keys=True)
print(json.dumps(document, indent=2, sort_keys=True))
if not document["infrastructure"]["passed"] or completed != submitted or protocol_errors or rpc_error_episodes:
    raise SystemExit(1)
'''


def server_config(expected_workers: int = 1, worker_capacity: int = 1) -> str:
    """生成 Gate4 隔离 server 配置。

    SWE episode 比 Code episode 慢很多，所以 default_timeout_secs 和
    worker_degraded_threshold_secs 都设置得更长。
    """
    total_slots = max(1, expected_workers * worker_capacity)
    return f'''port: {base.SERVER_PORT}
admin_http_port: 0
admin_http_bind: "127.0.0.1"
scheduler:
  strategy: round_robin
  worker_degraded_threshold_secs: 1200
  schedule_retry_interval_ms: 50
  heartbeat_interval_ms: 5000
  heartbeat_timeout_secs: 30
episode:
  default_timeout_secs: 900
  stale_warning_secs: 450
  max_attempts: 3
  queue_dynamic: true
  queue_max_in_flight: 0
  broadcast_capacity: {max(1024, total_slots * 4)}
  completed_async_ttl_secs: 3600
  completed_async_max_entries: {max(10000, total_slots * 16)}
'''


def worker_config(
    run_dir: str,
    worker_id: str,
    run_id: str,
    worker_port: int,
    obs_port: int,
    gateway_port: int,
    capacity: int,
) -> str:
    """生成 SWE worker 配置。

    runtime_gateway 是 SWE/OpenHands 的关键：OpenHands agent 通过它访问
    worker 管理的容器运行环境。1024 Worker 规模压测时，每个 Worker 都有独立
    listen / obs / runtime gateway 端口，避免伪造注册数量。
    """
    return f'''server:
  endpoint: "{base.SERVER_PRIVATE_IP}:{base.SERVER_PORT}"
worker:
  id: "{worker_id}"
  listen: "0.0.0.0:{worker_port}"
  advertise_endpoint: "{base.WORKER_PRIVATE_IP}:{worker_port}"
  max_concurrent: {capacity}
scheduler:
  mode: "remote"
env:
  types: ["swe"]
  backend: "process"
  plugin_dir: "{run_dir}/bundle/plugins"
pool:
  warmup_size: 0
  prewarm_on_startup: false
  max_idle_time: 600
  cool_timeout: 60
  max_episode_count: 10
logging:
  level: "info"
  file: "{run_dir}/logs/{worker_id}.runtime.log"
wal:
  dir: "{run_dir}/wal/{worker_id}"
observability:
  metrics_listen: "127.0.0.1:{obs_port}"
  health_listen: "127.0.0.1:{obs_port}"
hub:
  enabled: false
runtime_gateway:
  enabled: true
  listen: "0.0.0.0:{gateway_port}"
  capacity: {capacity}
  api_key: "stress-gateway-{run_id}"
swe:
  variants: ["verified"]
  prewarm: []
  warm_tag: false
'''


def parse_private_port_range(value: str, count: int, *, single_port: int, label: str) -> list[int]:
    if count == 1:
        if value:
            raise ValueError(f"--{label} is only valid when --registered-workers > 1")
        return [single_port]
    if not value or "-" not in value:
        raise ValueError(
            f"--{label} START-END is required when --registered-workers > 1; "
            "the range must already be open between the Server and Worker hosts"
        )
    start_text, end_text = value.split("-", 1)
    start, end = int(start_text), int(end_text)
    if start <= 0 or end > 65535 or end < start:
        raise ValueError(f"invalid --{label}: {value}")
    ports = list(range(start, end + 1))
    if len(ports) < count:
        raise ValueError(f"--{label} has {len(ports)} ports, but {count} Workers were requested")
    return ports[:count]


def container_ids(client) -> set[str]:
    """读取 worker 机器上所有 Docker 容器 ID。

    Gate4 开始前要求容器集合为空；结束后要求恢复到开始前的集合。
    这样可以发现压测有没有遗留容器。
    """
    _, out, _ = base.run(client, "docker ps -aq --no-trunc", timeout=30)
    return {line.strip() for line in out.splitlines() if line.strip()}


def wait_for_log(client, path: str, needle: str, timeout: int) -> str:
    """等待远端日志里出现指定文本。

    OpenHands agent 注册成功后会写入 registered agent_id=...。
    没等到这行日志就不能提交 SWE episode。
    """
    deadline = time.monotonic() + timeout
    latest = ""
    while time.monotonic() < deadline:
        try:
            latest = base.get_text(client, path)
        except OSError:
            latest = ""
        if needle in latest:
            return latest
        time.sleep(1)
    raise TimeoutError(f"did not find {needle!r} in {path}; tail={latest[-4000:]}")


def _log_timestamp(line: str) -> str:
    match = re.search(
        r"\d{4}-\d{2}-\d{2}[ T]\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:?\d{2})?",
        line,
    )
    return match.group(0) if match else ""


def _line_worker_id(line: str) -> str:
    match = re.search(r"worker_id=([^\s,]+)", line)
    return match.group(1).strip('",') if match else ""


def completed_worker_coverage(server, log_path: str, worker_prefix: str) -> dict:
    """Summarize per-Worker episode load from server logs."""
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


def summarize_resource_rows(resource_rows: list[dict]) -> dict:
    if not resource_rows:
        return {
            "samples": 0,
            "peak_running_containers": 0,
            "min_mem_available_kib": 0,
            "observation_only": True,
            "note": "resource monitor produced no samples",
        }
    first = resource_rows[0]
    last = resource_rows[-1]

    def load_at(row: dict, index: int) -> float:
        try:
            return float(row.get("load", [0.0, 0.0, 0.0])[index])
        except (TypeError, ValueError, IndexError):
            return 0.0

    mem_total_kib = max((int(row.get("mem_total_kib", 0) or 0) for row in resource_rows), default=0)
    initial_available = int(first.get("mem_available_kib", 0) or 0)
    final_available = int(last.get("mem_available_kib", 0) or 0)
    min_available = min((int(row.get("mem_available_kib", 0) or 0) for row in resource_rows), default=0)
    peak_containers = max((int(row.get("running_containers", 0) or 0) for row in resource_rows), default=0)
    started = float(first.get("ts", 0.0) or 0.0)
    finished = float(last.get("ts", started) or started)
    return {
        "samples": len(resource_rows),
        "duration_seconds": max(0.0, finished - started),
        "sample_period_seconds": 0.1,
        "mem_total_kib": mem_total_kib,
        "initial_mem_available_kib": initial_available,
        "final_mem_available_kib": final_available,
        "min_mem_available_kib": min_available,
        "max_mem_available_drop_kib": max(0, initial_available - min_available),
        "peak_running_containers": peak_containers,
        "max_load1": max(load_at(row, 0) for row in resource_rows),
        "max_load5": max(load_at(row, 1) for row in resource_rows),
        "max_load15": max(load_at(row, 2) for row in resource_rows),
        "observation_only": True,
    }


def summarize_fleet_metrics(metrics: dict, registered_workers: int) -> dict:
    if not metrics:
        return {"available": False, "observation_only": True}
    initial_available = int(metrics.get("initial_mem_available_bytes", 0) or 0)
    min_available = int(metrics.get("min_mem_available_bytes", 0) or 0)
    peak_rss = int(metrics.get("peak_rss_bytes", 0) or 0)
    peak_open_fds = int(metrics.get("peak_open_fds", 0) or 0)
    workers = max(1, int(registered_workers))
    enriched = dict(metrics)
    enriched.update({
        "available": True,
        "observation_only": True,
        "registered_workers": registered_workers,
        "mem_available_drop_bytes": max(0, initial_available - min_available),
        "peak_rss_bytes_per_registered_worker": peak_rss / workers,
        "peak_open_fds_per_registered_worker": peak_open_fds / workers,
        "note": (
            "Summed RSS may double-count shared executable/library pages; "
            "host MemAvailable drop is the safer capacity observation."
        ),
    })
    return enriched


def newest_local_artifact(root: Path, pattern: str) -> Path | None:
    candidates = list(root.rglob(pattern))
    return max(candidates, key=lambda item: item.stat().st_mtime_ns) if candidates else None


def _first_text(*values) -> str:
    for value in values:
        if isinstance(value, str) and value:
            return value
    return ""


def _response_choice(response: dict) -> dict:
    choices = response.get("choices")
    if isinstance(choices, list) and choices and isinstance(choices[0], dict):
        return choices[0]
    return {}


def _response_content(response: dict) -> str:
    choice = _response_choice(response)
    message = choice.get("message")
    if isinstance(message, dict):
        content = message.get("content")
        if isinstance(content, str):
            return content
    return _first_text(response.get("content"), response.get("text"), response.get("output"))


def _response_token_data(response: dict) -> tuple[list[int], list[float]]:
    response_ids = response.get("uenv_response_ids") or response.get("response_ids") or []
    if not isinstance(response_ids, list):
        response_ids = []
    logprobs = response.get("rollout_log_probs") or response.get("logprobs") or []
    choice = _response_choice(response)
    choice_logprobs = choice.get("logprobs") if isinstance(choice, dict) else None
    if isinstance(choice_logprobs, dict) and isinstance(choice_logprobs.get("content"), list):
        tokens = choice_logprobs["content"]
        if not response_ids:
            response_ids = [
                item.get("token_id")
                for item in tokens
                if isinstance(item, dict) and isinstance(item.get("token_id"), int)
            ]
        logprobs = [
            item.get("logprob")
            for item in tokens
            if isinstance(item, dict) and isinstance(item.get("logprob"), (int, float))
        ]
    return (
        [int(value) for value in response_ids if isinstance(value, int)],
        [float(value) for value in logprobs if isinstance(value, (int, float))],
    )


def _turn_from_raw(item: dict, turn_index: int) -> dict | None:
    response = item.get("response") if isinstance(item.get("response"), dict) else {}
    rollout_trace = item.get("rollout_trace") if isinstance(item.get("rollout_trace"), dict) else {}
    content = _first_text(
        item.get("assistant_output"),
        item.get("text"),
        item.get("content"),
        item.get("output"),
        _response_content(response),
    )
    response_ids, logprobs = _response_token_data(response)
    if not response_ids:
        response_ids = item.get("response_ids") or item.get("uenv_response_ids") or rollout_trace.get("response_ids") or []
    if not logprobs:
        logprobs = item.get("logprobs") or item.get("rollout_log_probs") or []
    response_ids = [int(value) for value in response_ids if isinstance(value, int)]
    logprobs = [float(value) for value in logprobs if isinstance(value, (int, float))]
    if not (content or response_ids or logprobs):
        return None
    raw_turn_index = item.get("turn_index", turn_index)
    return {
        "turn_index": int(raw_turn_index) if isinstance(raw_turn_index, int) else turn_index,
        "assistant_output": content,
        "response_ids": response_ids,
        "logprobs": logprobs,
        "latency_ms": float(item.get("latency_ms", 0.0) or 0.0),
    }


def normalize_trace_corpus_document(document: object, *, source_path: str, fallback_run_id: str) -> dict | None:
    if isinstance(document, list):
        turns = [
            turn
            for index, item in enumerate(document)
            if isinstance(item, dict)
            for turn in [_turn_from_raw(item, index)]
            if turn
        ]
        if not turns:
            return None
        return {
            "schema_version": 1,
            "corpus_kind": "openhands_real_llm_trace_rollout",
            "run_id": fallback_run_id,
            "source_path": source_path,
            "turns": turns,
        }
    if not isinstance(document, dict):
        return None
    if isinstance(document.get("turns"), list):
        normalized = dict(document)
        normalized.setdefault("schema_version", 1)
        normalized.setdefault("corpus_kind", "openhands_real_llm_trace_episode")
        normalized.setdefault("source_path", source_path)
        return normalized
    turns = []
    for candidate in (
        document.get("llm_calls"),
        document.get("calls"),
        document.get("events"),
        document.get("messages"),
        document.get("responses"),
    ):
        if not isinstance(candidate, list):
            continue
        for item in candidate:
            if isinstance(item, dict):
                turn = _turn_from_raw(item, len(turns))
                if turn:
                    turns.append(turn)
    if not turns:
        turn = _turn_from_raw(document, 0)
        if turn:
            turns.append(turn)
    if not turns:
        return None
    return {
        "schema_version": 1,
        "corpus_kind": "openhands_real_llm_trace_rollout",
        "run_id": str(document.get("run_id") or fallback_run_id),
        "instance_id": str(document.get("instance_id") or ""),
        "source_path": source_path,
        "turns": turns,
    }


def sample_swe_instances(catalog_path: str, catalog_text: str, count: int, seed: int) -> tuple[list[str], dict]:
    """Sample multiple SWE-bench Verified instances from the configured catalog."""
    data = json.loads(catalog_text)
    rows = list(data.values()) if isinstance(data, dict) else list(data)
    ids = [str(row["instance_id"]) for row in rows]
    if not ids:
        raise ValueError(f"empty SWE catalog: {catalog_path}")
    rng = random.Random(seed)
    shuffled = ids[:]
    rng.shuffle(shuffled)
    selected = [shuffled[index % len(shuffled)] for index in range(count)]
    return selected, {
        "catalog_path": str(catalog_path),
        "catalog_size": len(ids),
        "seed": seed,
        "selected_instance_ids": selected,
        "unique_instance_count": len(set(selected)),
        "requested_episodes": count,
        "reused_instances": count > len(set(selected)),
        "catalog_has_image_fields": any(
            bool(row.get("image") or row.get("docker_image")) for row in rows
            if isinstance(row, dict)
        ),
    }


def docker_image_inventory(client) -> dict:
    """Record cached Docker images without assuming every SWE instance has one image field."""
    _, out, _ = base.run(
        client,
        "docker images --format '{{.Repository}}:{{.Tag}} {{.ID}} {{.Size}}'",
        timeout=60,
    )
    images = [line.strip() for line in out.splitlines() if line.strip()]
    known_status, known_id, _ = base.run(
        client,
        f"docker image inspect {base.q(DEFAULT_KNOWN_IMAGE)} --format '{{{{.Id}}}}'",
        timeout=30,
        check=False,
    )
    return {
        "cached_images": images,
        "known_astropy_image": DEFAULT_KNOWN_IMAGE,
        "known_astropy_image_present": known_status == 0,
        "known_astropy_image_id": known_id.strip() if known_status == 0 else "",
        "known_astropy_image_id_matches_prior": known_id.strip() == DEFAULT_KNOWN_IMAGE_ID if known_status == 0 else False,
        "note": "Catalog entries do not carry docker image names here; the real OpenHands/official runner resolves environments at runtime.",
    }


def swebench_image_for_instance(instance_id: str) -> str:
    namespace, repo_issue = instance_id.split("__", 1)
    return f"swebench/sweb.eval.x86_64.{namespace}_1776_{repo_issue}:latest"


def docker_image_presence(client, instance_ids: list[str]) -> dict:
    records = []
    missing = []
    for instance_id in instance_ids:
        image = swebench_image_for_instance(instance_id)
        status, image_id, _ = base.run(
            client,
            f"docker image inspect {base.q(image)} --format '{{{{.Id}}}}'",
            timeout=30,
            check=False,
        )
        present = status == 0
        record = {
            "instance_id": instance_id,
            "image": image,
            "present": present,
            "image_id": image_id.strip() if present else "",
        }
        records.append(record)
        if not present:
            missing.append(record)
    return {
        "records": records,
        "missing": missing,
        "all_present": not missing,
        "unique_image_count": len({record["image"] for record in records}),
        "unique_images": sorted({record["image"] for record in records}),
        "image_pull_policy": "local_only",
    }


class RunArgs:
    """给 run_one 传参的小对象，避免把 argparse 结果传进内部逻辑。"""
    def __init__(
        self,
        *,
        concurrency: int,
        artifacts: Path,
        mode: str,
        parallel_mode: str,
        max_steps: int,
        openhands_max_iterations: int,
        llm_config: str,
        llm_kind: str,
        instance_count: int,
        instance_seed: int,
        simulator_latency_ms: float,
        simulator_latency_mean_ms: float,
        simulator_latency_std_ms: float,
        simulator_latency_min_ms: float,
        simulator_latency_max_ms: float,
        simulator_zero_latency: bool,
        simulator_wrong_steps_mean: float,
        simulator_wrong_steps_std: float,
        simulator_wrong_steps_min: int,
        simulator_wrong_steps_max: int,
        simulator_repair_success_rate: float,
        simulator_repair_style: str,
        simulator_seed: int,
        simulator_mode: str,
        trace_corpus_path: str,
        trace_sampling_strategy: str,
        registered_workers: int,
        worker_capacity: int,
        total_episodes: int,
        episode_batch_size: int,
        episode_offset: int,
        min_episode_waves: int,
        private_worker_port_range: str,
        private_gateway_port_range: str,
        fleet_supervisor_threshold: int,
        registration_timeout: int,
        batch_timeout: int,
    ) -> None:
        self.concurrency = concurrency
        self.artifacts = artifacts
        self.mode = mode
        self.parallel_mode = parallel_mode
        self.max_steps = max_steps
        self.openhands_max_iterations = openhands_max_iterations
        self.llm_config = llm_config
        self.llm_kind = llm_kind
        self.instance_count = instance_count
        self.instance_seed = instance_seed
        self.simulator_latency_ms = simulator_latency_ms
        self.simulator_latency_mean_ms = simulator_latency_mean_ms
        self.simulator_latency_std_ms = simulator_latency_std_ms
        self.simulator_latency_min_ms = simulator_latency_min_ms
        self.simulator_latency_max_ms = simulator_latency_max_ms
        self.simulator_zero_latency = simulator_zero_latency
        self.simulator_wrong_steps_mean = simulator_wrong_steps_mean
        self.simulator_wrong_steps_std = simulator_wrong_steps_std
        self.simulator_wrong_steps_min = simulator_wrong_steps_min
        self.simulator_wrong_steps_max = simulator_wrong_steps_max
        self.simulator_repair_success_rate = simulator_repair_success_rate
        self.simulator_repair_style = simulator_repair_style
        self.simulator_seed = simulator_seed
        self.simulator_mode = simulator_mode
        self.trace_corpus_path = trace_corpus_path
        self.trace_sampling_strategy = trace_sampling_strategy
        self.registered_workers = registered_workers
        self.worker_capacity = worker_capacity
        self.total_episodes = total_episodes
        self.episode_batch_size = episode_batch_size
        self.episode_offset = episode_offset
        self.min_episode_waves = min_episode_waves
        self.private_worker_port_range = private_worker_port_range
        self.private_gateway_port_range = private_gateway_port_range
        self.fleet_supervisor_threshold = fleet_supervisor_threshold
        self.registration_timeout = registration_timeout
        self.batch_timeout = batch_timeout


def run_one(
    concurrency: int,
    artifacts: Path,
    mode: str,
    parallel_mode: str,
    max_steps: int,
    openhands_max_iterations: int,
    llm_config: str,
    llm_kind: str,
    instance_count: int,
    instance_seed: int,
    simulator_latency_ms: float,
    simulator_latency_mean_ms: float,
    simulator_latency_std_ms: float,
    simulator_latency_min_ms: float,
    simulator_latency_max_ms: float,
    simulator_zero_latency: bool,
    simulator_wrong_steps_mean: float,
    simulator_wrong_steps_std: float,
    simulator_wrong_steps_min: int,
    simulator_wrong_steps_max: int,
    simulator_repair_success_rate: float,
    simulator_repair_style: str,
    simulator_seed: int,
    simulator_mode: str,
    trace_corpus_path: str,
    trace_sampling_strategy: str,
    registered_workers: int,
    worker_capacity: int,
    total_episodes: int,
    episode_batch_size: int,
    episode_offset: int,
    min_episode_waves: int,
    private_worker_port_range: str,
    private_gateway_port_range: str,
    fleet_supervisor_threshold: int,
    registration_timeout: int,
    batch_timeout: int,
) -> int:
    """执行一轮 Gate4。

    concurrency 表示 OpenHands/container 并发上限；registered_workers 表示
    真实 UEnv SWE Worker 注册数量。1024 Worker 规模压测通过多批 episode
    覆盖 Worker，而不是默认同时启动 1024 个 SWE 容器。
    函数步骤：
    1. 检查正式 server、端口、Docker 容器基线和 SWE 镜像；
    2. 打包 worker、plugins、integrations、OpenHands runner；
    3. 启动隔离 server、SWE worker、OpenHands agent、资源监控；
    4. 提交 SWE episode；
    5. 保存 result、resource summary、manifest 和日志；
    6. 清理本次进程和本次新增容器。
    """
    args = RunArgs(
        concurrency=concurrency,
        artifacts=artifacts,
        mode=mode,
        parallel_mode=parallel_mode,
        max_steps=max_steps,
        openhands_max_iterations=openhands_max_iterations,
        llm_config=llm_config,
        llm_kind=llm_kind,
        instance_count=instance_count,
        instance_seed=instance_seed,
        simulator_latency_ms=simulator_latency_ms,
        simulator_latency_mean_ms=simulator_latency_mean_ms,
        simulator_latency_std_ms=simulator_latency_std_ms,
        simulator_latency_min_ms=simulator_latency_min_ms,
        simulator_latency_max_ms=simulator_latency_max_ms,
        simulator_zero_latency=simulator_zero_latency,
        simulator_wrong_steps_mean=simulator_wrong_steps_mean,
        simulator_wrong_steps_std=simulator_wrong_steps_std,
        simulator_wrong_steps_min=simulator_wrong_steps_min,
        simulator_wrong_steps_max=simulator_wrong_steps_max,
        simulator_repair_success_rate=simulator_repair_success_rate,
        simulator_repair_style=simulator_repair_style,
        simulator_seed=simulator_seed,
        simulator_mode=simulator_mode,
        trace_corpus_path=trace_corpus_path,
        trace_sampling_strategy=trace_sampling_strategy,
        registered_workers=registered_workers,
        worker_capacity=worker_capacity,
        total_episodes=total_episodes,
        episode_batch_size=episode_batch_size,
        episode_offset=episode_offset,
        min_episode_waves=min_episode_waves,
        private_worker_port_range=private_worker_port_range,
        private_gateway_port_range=private_gateway_port_range,
        fleet_supervisor_threshold=fleet_supervisor_threshold,
        registration_timeout=registration_timeout,
        batch_timeout=batch_timeout,
    )
    password = os.environ.get("UENV_PASS")
    if not password:
        raise SystemExit("UENV_PASS is required")

    run_id = f"gate4-swe-{args.parallel_mode}-c{args.concurrency}-{time.strftime('%Y%m%d-%H%M%S')}-{uuid.uuid4().hex[:8]}"
    server_run = f"/tmp/uenv-{run_id}"
    worker_run = f"/opt/uenv-stress/runs/{run_id}"
    worker_prefix = f"stress-{run_id}-worker-"
    worker_id = f"{worker_prefix}0000"
    agent_id = f"stress-{run_id}-agent-0000"
    worker_ports = parse_private_port_range(
        args.private_worker_port_range,
        args.registered_workers,
        single_port=base.WORKER_PORT,
        label="private-worker-port-range",
    )
    gateway_ports = parse_private_port_range(
        args.private_gateway_port_range,
        args.registered_workers,
        single_port=GATEWAY_PORT,
        label="private-gateway-port-range",
    )
    obs_ports = [base.OBS_PORT + index for index in range(args.registered_workers)]
    fixed_worker_ports = [AGENT_API_PORT, AGENT_HEALTH_PORT, base.MODEL_PORT]
    port_groups = {
        "worker": set(worker_ports),
        "gateway": set(gateway_ports),
        "observability": set(obs_ports),
        "agent/model": set(fixed_worker_ports),
    }
    overlaps: list[dict[str, object]] = []
    group_items = list(port_groups.items())
    for left_index, (left_label, left_ports) in enumerate(group_items):
        for right_label, right_ports in group_items[left_index + 1:]:
            shared = sorted(left_ports & right_ports)
            if shared:
                overlaps.append({"left": left_label, "right": right_label, "ports": shared[:20]})
    if len(fixed_worker_ports) != len(set(fixed_worker_ports)):
        overlaps.append({"left": "agent/model", "right": "agent/model", "ports": sorted(fixed_worker_ports)})
    if overlaps:
        raise ValueError(f"Gate4 Worker/gateway/observability/agent port groups overlap: {overlaps}")
    target_episodes = args.total_episodes or (args.registered_workers * args.worker_capacity * args.min_episode_waves)
    resolved_episode_batch_size = args.episode_batch_size or args.concurrency
    if resolved_episode_batch_size <= 0:
        raise ValueError("episode batch size must resolve to a positive value")
    args.artifacts.mkdir(parents=True, exist_ok=True)
    local_run = args.artifacts / run_id
    local_run.mkdir()
    print(
        "[gate4][stage] run prepared "
        f"run_id={run_id} local_run={local_run} "
        f"server={base.SERVER_HOST}:{base.SERVER_PORT} "
        f"worker={base.WORKER_HOST} registered_workers={args.registered_workers} "
        f"worker_ports={worker_ports[0]}-{worker_ports[-1]} "
        f"gateway_ports={gateway_ports[0]}-{gateway_ports[-1]} "
        f"obs_ports={obs_ports[0]}-{obs_ports[-1]}",
        flush=True,
    )

    server = worker = None
    server_pid = agent_pid = monitor_pid = llm_simulator_pid = fleet_supervisor_pid = None
    worker_pid = None
    worker_pids: list[tuple[int, str]] = []
    fleet_metrics_path = ""
    fleet_pid_document: dict = {}
    before_protected = None
    before_containers: set[str] = set()
    error: str | None = None
    result_code = 1
    cleanup_errors: list[str] = []
    try:
        # 先连接两台机器，并记录正式 server 和容器集合的基线。
        server = base.connect(base.SERVER_HOST, password)
        print(f"[gate4][stage] connecting server {base.SERVER_HOST}", flush=True)
        server = base.connect(base.SERVER_HOST, password)
        print(f"[gate4][stage] connected server {base.SERVER_HOST}", flush=True)
        print(f"[gate4][stage] connecting worker {base.WORKER_HOST}", flush=True)
        worker = base.connect(base.WORKER_HOST, password)
        print(f"[gate4][stage] connected worker {base.WORKER_HOST}", flush=True)
        print("[gate4][stage] collecting protected/source/catalog preflight", flush=True)
        before_protected = base.protected_snapshot(server)
        build = base.source_and_binary_manifest(server, include_code_plugin=False)
        catalog_path = f"{base.SOURCE_REPO}/config/swe/verified.json"
        catalog_text = base.get_text(server, catalog_path)
        selected_instances, instance_sampling = sample_swe_instances(
            catalog_path,
            catalog_text,
            max(args.concurrency, args.instance_count),
            args.instance_seed,
        )
        before_containers = container_ids(worker)
        if before_containers:
            raise RuntimeError(f"worker host is not container-empty: {sorted(before_containers)}")
        # Gate4 涉及 server、worker、gateway、agent API、agent health 多个端口。
        # 全部确认空闲后再启动。
        base.assert_port_free(server, base.SERVER_PORT, base.SERVER_HOST)
        print("[gate4][stage] checking isolated ports and SWE images", flush=True)
        base.assert_port_free(server, base.SERVER_PORT, base.SERVER_HOST)
        base.assert_ports_free(worker, worker_ports + obs_ports + gateway_ports + fixed_worker_ports, base.WORKER_HOST)
        image_inventory = docker_image_inventory(worker)
        selected_image_presence = docker_image_presence(worker, selected_instances)
        if not selected_image_presence["all_present"]:
            raise RuntimeError(
                "selected SWE-bench images are not cached locally under local_only policy: "
                + json.dumps(selected_image_presence["missing"], sort_keys=True)
            )
        print(f"[preflight] protected={json.dumps(before_protected, sort_keys=True)}")
        print(f"[preflight] selected_instances={json.dumps(selected_instances, sort_keys=True)}")
        print(f"[preflight] docker_images={len(image_inventory['cached_images'])}")
        base.run(worker, f"install -d -m 0755 {base.q(worker_run)}")
        # OpenHands 依赖使用 frozen 环境。这里用 uv sync --check --frozen 确认环境没有漂移。
        base.run(
            worker,
            "cd /opt/openhands/benchmarks && "
            "uv sync --check --frozen && "
            ".venv/bin/python -c 'import openhands.sdk; import openhands.tools.file_editor'",
            timeout=60,
        )
        print("[preflight] OpenHands frozen environment and imports verified")
        effective_llm_config = args.llm_config
        effective_trace_corpus_path = args.trace_corpus_path
        if args.llm_kind == "simulator" and args.simulator_mode == "trace_replay":
            local_corpus = Path(args.trace_corpus_path)
            if local_corpus.exists():
                with tempfile.NamedTemporaryFile(prefix=f"{run_id}-trace-corpus-", suffix=".tgz", delete=False) as tmp:
                    local_archive = Path(tmp.name)
                try:
                    with tarfile.open(local_archive, "w:gz") as tar:
                        if local_corpus.is_dir():
                            tar.add(local_corpus, arcname="trace-corpus")
                        else:
                            tar.add(local_corpus, arcname=f"trace-corpus/{local_corpus.name}")
                    remote_archive = f"{worker_run}/trace-corpus.tgz"
                    with worker.open_sftp() as sftp:
                        sftp.put(str(local_archive), remote_archive)
                    base.run(worker, f"install -d -m 0755 {base.q(worker_run)}/trace-corpus && tar -C {base.q(worker_run)} -xzf {base.q(remote_archive)}", timeout=120)
                    effective_trace_corpus_path = f"{worker_run}/trace-corpus"
                finally:
                    local_archive.unlink(missing_ok=True)
        if args.mode == "llm" and args.llm_kind == "simulator":
            base.put_text(worker, f"{worker_run}/llm_simulator.py", LLM_SIMULATOR, 0o700)
            simulator_config = {
                "model": "openai/uenv-swe-simulator",
                "base_url": f"http://127.0.0.1:{base.MODEL_PORT}/v1",
                "api_key": f"uenv-simulator-{run_id}",
                "timeout": 900,
            }
            effective_llm_config = f"{worker_run}/openhands-llm-simulator.json"
            base.put_text(worker, effective_llm_config, json.dumps(simulator_config, sort_keys=True), 0o600)
            llm_simulator_pid = base.start_owned(
                worker,
                " ".join([
                    "python3", "-B", f"{worker_run}/llm_simulator.py",
                    "--port", str(base.MODEL_PORT),
                    "--latency-ms", str(args.simulator_latency_ms),
                    "--latency-mean-ms", str(args.simulator_latency_mean_ms),
                    "--latency-std-ms", str(args.simulator_latency_std_ms),
                    "--latency-min-ms", str(args.simulator_latency_min_ms),
                    "--latency-max-ms", str(args.simulator_latency_max_ms),
                    *(["--zero-latency"] if args.simulator_zero_latency else []),
                    "--wrong-steps-mean", str(args.simulator_wrong_steps_mean),
                    "--wrong-steps-std", str(args.simulator_wrong_steps_std),
                    "--wrong-steps-min", str(args.simulator_wrong_steps_min),
                    "--wrong-steps-max", str(args.simulator_wrong_steps_max),
                    "--repair-success-rate", str(args.simulator_repair_success_rate),
                    "--repair-style", args.simulator_repair_style,
                    "--seed", str(args.simulator_seed),
                    "--simulator-mode", args.simulator_mode,
                    "--trace-corpus-path", base.q(effective_trace_corpus_path),
                    "--trace-sampling-strategy", args.trace_sampling_strategy,
                ]),
                f"{worker_run}/llm-simulator.log",
                "/usr/bin/python3.12",
                f"{worker_run}/llm_simulator.py",
            )
        if args.mode == "llm":
            if not effective_llm_config:
                raise ValueError("Gate4 llm mode requires --llm-config or OPENHANDS_LLM_CONFIG")
            base.run(worker, f"test -f {base.q(effective_llm_config)} && test $(stat -c %a {base.q(effective_llm_config)}) = 600")
            base.put_text(worker, f"{worker_run}/llm_preflight.py", LLM_PREFLIGHT, 0o700)
            _, llm_output, _ = base.run(
                worker,
                "cd /opt/openhands/benchmarks && "
                f".venv/bin/python {base.q(worker_run)}/llm_preflight.py --config {base.q(effective_llm_config)}",
                timeout=120,
            )
            llm_preflight = json.loads(llm_output)
            _, llm_config_hash, _ = base.run(worker, f"sha256sum {base.q(effective_llm_config)}")
            llm_config_sha256 = llm_config_hash.split()[0]
            print(f"[preflight] OpenHands LLM schema/auth/minimal call verified: {llm_preflight}")
        else:
            llm_preflight = {}
            llm_config_sha256 = ""

        # 在 server 机器打包需要的 worker、plugins、integrations 和 SWE 配置，
        # 再传到 worker 机器解包运行。
        base.run(server, f"install -d -m 0755 {base.q(server_run)}/bundle {base.q(server_run)}/generated/uenv/v1")
        base.run(worker, f"install -d -m 0755 {base.q(worker_run)} {base.q(worker_run)}/logs {base.q(worker_run)}/wal {base.q(worker_run)}/openhands {base.q(worker_run)}/trajectory")
        base.run(
            server,
            " && ".join([
                f"install -m 0755 {base.q(base.SOURCE_WORKER_BIN)} {base.q(server_run)}/bundle/uenv-worker",
                f"strip {base.q(server_run)}/bundle/uenv-worker",
                f"cp -a {base.q(base.SOURCE_REPO)}/plugins {base.q(server_run)}/bundle/",
                f"cp -a {base.q(base.SOURCE_REPO)}/integrations {base.q(server_run)}/bundle/",
                f"install -d -m 0755 {base.q(server_run)}/bundle/scripts/openhands {base.q(server_run)}/bundle/uenv-server/stress_test {base.q(server_run)}/bundle/config/swe",
                f"install -m 0755 {base.q(base.SOURCE_REPO)}/scripts/openhands/openhands_runner.py {base.q(server_run)}/bundle/scripts/openhands/openhands_runner.py",
                f"install -m 0755 {base.q(base.SOURCE_REPO)}/uenv-server/stress_test/run_openhands_stress.sh {base.q(server_run)}/bundle/uenv-server/stress_test/run_openhands_stress.sh",
                f"install -m 0644 {base.q(base.SOURCE_REPO)}/config/swe/verified.json {base.q(server_run)}/bundle/config/swe/verified.json",
                f"tar -C {base.q(server_run)}/bundle -czf {base.q(server_run)}/bundle.tgz .",
                f"sha256sum {base.q(server_run)}/bundle.tgz > {base.q(server_run)}/bundle.tgz.sha256",
            ]),
            timeout=180,
        )
        _, bundle_size, _ = base.run(server, f"stat -c %s {base.q(server_run)}/bundle.tgz")
        print(f"[bundle] compressed_bytes={bundle_size.strip()}")
        with tempfile.NamedTemporaryFile(prefix=run_id, suffix=".tgz", delete=False) as temporary:
            local_bundle = Path(temporary.name)
        try:
            with server.open_sftp() as source_sftp:
                source_sftp.get(f"{server_run}/bundle.tgz", str(local_bundle))
            with worker.open_sftp() as target_sftp:
                target_sftp.put(str(local_bundle), f"{worker_run}/bundle.tgz")
        finally:
            local_bundle.unlink(missing_ok=True)
        base.run(worker, f"install -d -m 0755 {base.q(worker_run)}/bundle && tar -C {base.q(worker_run)}/bundle -xzf {base.q(worker_run)}/bundle.tgz")

        base.put_text(server, f"{server_run}/server.yaml", server_config(args.registered_workers, args.worker_capacity))
        base.put_text(server, f"{server_run}/smoke_client.py", SWE_CLIENT, 0o755)
        base.put_text(server, f"{server_run}/stress_test_common.py", COMMON_SOURCE)
        base.put_text(worker, f"{worker_run}/worker_fleet_supervisor.py", FLEET_SUPERVISOR_SOURCE, 0o700)
        worker_documents = {}
        for index, (worker_port, obs_port, gateway_port) in enumerate(zip(worker_ports, obs_ports, gateway_ports)):
            current_worker_id = f"{worker_prefix}{index:04d}"
            config_path = f"{worker_run}/worker-{index:04d}.yaml"
            worker_documents[config_path] = (
                worker_config(
                    worker_run,
                    current_worker_id,
                    run_id,
                    worker_port,
                    obs_port,
                    gateway_port,
                    args.worker_capacity,
                ),
                0o600,
            )
        put_worker_config_archive(worker, worker_run, worker_documents, run_id)
        base.put_text(worker, f"{worker_run}/resource_monitor.py", RESOURCE_MONITOR, 0o755)

        # server 侧 client 和 worker 侧 agent 都需要 Python protobuf 文件。
        # 这里从显式 source repo 的 proto 生成，避免使用旧生成物。
        proto_root = f"{base.SOURCE_REPO}/proto"
        proto_command = " ".join([
            "/usr/bin/protoc", "-I", base.q(proto_root),
            f"--python_out={base.q(server_run)}/generated",
            base.q(f"{proto_root}/uenv/v1/common.proto"),
            base.q(f"{proto_root}/uenv/v1/episode.proto"),
            base.q(f"{proto_root}/uenv/v1/scheduler.proto"),
            base.q(f"{proto_root}/uenv/v1/adapter_core.proto"),
            base.q(f"{proto_root}/uenv/v1/agent.proto"),
        ])
        base.run(server, proto_command)
        base.run(server, f"touch {base.q(server_run)}/generated/uenv/__init__.py {base.q(server_run)}/generated/uenv/v1/__init__.py")
        with server.open_sftp() as server_sftp, worker.open_sftp() as worker_sftp:
            with server_sftp.open(
                f"{base.SOURCE_REPO}/integrations/openhands/uenv_runtime/gen/uenv/v1/agent_pb2_grpc.py", "rb"
            ) as source:
                grpc_stub = source.read()
            with worker_sftp.open(f"{worker_run}/agent_pb2_grpc.py", "wb") as destination:
                destination.write(grpc_stub)
        base.run(worker, f"install -d -m 0755 {base.q(worker_run)}/generated/uenv/v1")
        for filename in ("common_pb2.py", "episode_pb2.py", "scheduler_pb2.py", "agent_pb2.py"):
            with server.open_sftp() as source_sftp, worker.open_sftp() as target_sftp:
                with source_sftp.open(f"{server_run}/generated/uenv/v1/{filename}", "rb") as source:
                    payload = source.read()
                with target_sftp.open(f"{worker_run}/generated/uenv/v1/{filename}", "wb") as target:
                    target.write(payload)
        base.run(
            worker,
            f"install -m 0644 {base.q(worker_run)}/agent_pb2_grpc.py {base.q(worker_run)}/generated/uenv/v1/agent_pb2_grpc.py && "
            f"touch {base.q(worker_run)}/generated/uenv/__init__.py {base.q(worker_run)}/generated/uenv/v1/__init__.py",
        )
        agent_pythonpath = f"{worker_run}/generated:{worker_run}/bundle/integrations/openhands"
        _, grpc_import_output, _ = base.run(
            worker,
            " ".join([
                f"PYTHONPATH={base.q(agent_pythonpath)}",
                OPENHANDS_PYTHON,
                "-c",
                base.q(
                    "from uenv_runtime.agent_client import _load_grpc_modules; "
                    "grpc, _, _ = _load_grpc_modules(); "
                    "print('agent_grpc_compatible=' + grpc.__version__)"
                ),
            ]),
        )
        print(f"[preflight] {grpc_import_output.strip()}")

        # 启动隔离 server。关闭 trajectory/obs，减少与压测目标无关的额外工作。
        scale_purpose = (
            "single_worker_multi_concurrency_diagnostic"
            if args.registered_workers == 1 and args.worker_capacity > 1
            else "worker_scale"
        )
        server_log_filter = "info" if args.registered_workers > 1 or scale_purpose.startswith("single_worker") else "warn"
        server_command = " ".join([
            "env", "UENV_SERVER_CONFIG_STRICT=1", "UENV_TRAJECTORY_ENABLED=0",
            "UENV_OBS_ENABLED=0", "UENV_LOG_ANSI=0",
            f"UENV_ADDR={base.SERVER_PRIVATE_IP}:{base.SERVER_PORT}",
            f"UENV_SWE_GATEWAY_API_KEY=stress-gateway-{run_id}",
            f"UENV_CONFIG_PATH={server_run}/server.yaml", f"RUST_LOG={server_log_filter}", base.SERVER_BIN,
        ])
        server_pid = base.start_owned(
            server, server_command, f"{server_run}/server.log", base.SERVER_BIN, base.SERVER_BIN
        )
        # 启动 SWE worker fleet。每个 worker 进程必须携带自己的
        # UENV_SWE_GATEWAY_PUBLIC_URL，否则多 Worker AgentJob 会退化到同一个 gateway。
        worker_env_base = {
            "UENV_SERVER_CONFIG_STRICT": "1",
            "UENV_TRAJECTORY_ENABLED": "0",
            "UENV_OBS_ENABLED": "0",
            "UENV_LOG_ANSI": "0",
            "UENV_WORKER_EPISODE_TIMEOUT_SECS": str(args.batch_timeout),
            "UENV_LLM_HTTP_TIMEOUT_SECS": str(args.batch_timeout),
            "UENV_SWE_ARTIFACT_DIR": f"{worker_run}/trajectory",
            "UENV_SWE_INSTANCES": f"{worker_run}/bundle/config/swe/verified.json",
            "UENV_SWE_RUNTIME": "docker",
            "UENV_SWE_GATEWAY_API_KEY": f"stress-gateway-{run_id}",
            "RUST_LOG": "info",
        }
        if args.registered_workers >= args.fleet_supervisor_threshold:
            fleet_spec = {
                "workers": [
                    {
                        "worker_id": f"{worker_prefix}{index:04d}",
                        "config": f"{worker_run}/worker-{index:04d}.yaml",
                        "argv": [
                            f"{worker_run}/bundle/uenv-worker",
                            "--config",
                            f"{worker_run}/worker-{index:04d}.yaml",
                            "serve",
                        ],
                        "env": {
                            **worker_env_base,
                            "UENV_SWE_GATEWAY_PUBLIC_URL": f"http://{base.WORKER_PRIVATE_IP}:{gateway_ports[index]}",
                        },
                        "log": f"{worker_run}/worker-{index:04d}.log",
                    }
                    for index in range(args.registered_workers)
                ]
            }
            fleet_spec_path = f"{worker_run}/fleet.json"
            fleet_pid_path = f"{worker_run}/fleet-pids.json"
            fleet_metrics_path = f"{worker_run}/fleet-metrics.json"
            base.put_text(worker, fleet_spec_path, json.dumps(fleet_spec, sort_keys=True), 0o600)
            fleet_supervisor_pid = base.start_owned(
                worker,
                (
                    f"python3 -B {worker_run}/worker_fleet_supervisor.py "
                    f"--spec {fleet_spec_path} --pid-file {fleet_pid_path} "
                    f"--metrics-file {fleet_metrics_path}"
                ),
                f"{worker_run}/fleet-supervisor.log",
                "/usr/bin/python3.12",
                f"{worker_run}/worker_fleet_supervisor.py",
            )
            fleet_deadline = time.monotonic() + max(60, args.registered_workers // 2)
            while time.monotonic() < fleet_deadline:
                status, _, _ = base.run(worker, f"test -s {base.q(fleet_pid_path)}", check=False)
                if status == 0:
                    fleet_pid_document = json.loads(base.get_text(worker, fleet_pid_path))
                    break
                time.sleep(0.5)
            else:
                raise RuntimeError("Gate4 Worker fleet supervisor did not publish its PID manifest")
            if fleet_pid_document.get("worker_count") != args.registered_workers:
                raise RuntimeError(f"Gate4 fleet PID manifest count mismatch: {fleet_pid_document}")
            worker_pids = [
                (int(item["pid"]), str(item["config"]))
                for item in fleet_pid_document["workers"]
            ]
            worker_pid = worker_pids[0][0] if worker_pids else None
        else:
            for index in range(args.registered_workers):
                config = f"{worker_run}/worker-{index:04d}.yaml"
                env = {
                    **worker_env_base,
                    "UENV_SWE_GATEWAY_PUBLIC_URL": f"http://{base.WORKER_PRIVATE_IP}:{gateway_ports[index]}",
                }
                command = " ".join([
                    "env",
                    *(f"{key}={base.q(value)}" for key, value in env.items()),
                    f"{worker_run}/bundle/uenv-worker",
                    "--config",
                    config,
                    "serve",
                ])
                pid = base.start_owned(
                    worker,
                    command,
                    f"{worker_run}/worker-{index:04d}.log",
                    f"{worker_run}/bundle/uenv-worker",
                    config,
                )
                worker_pids.append((pid, config))
            worker_pid = worker_pids[0][0] if worker_pids else None
        # worker 进程启动成功不代表 runtime gateway 已经监听，所以至少检查首尾 gateway。
        deadline = time.monotonic() + max(90, args.registered_workers // 4)
        pending_gateway_ports = {gateway_ports[0], gateway_ports[-1]}
        while time.monotonic() < deadline:
            listener_text = base.listeners(worker)
            ready = {
                port for port in pending_gateway_ports
                if any(f":{port} " in line for line in listener_text.splitlines())
            }
            pending_gateway_ports -= ready
            if not pending_gateway_ports:
                break
            time.sleep(1)
        if pending_gateway_ports:
            raise TimeoutError(f"runtime gateway did not bind for ports={sorted(pending_gateway_ports)}")

        # OpenHands agent 通过轮询 server 获取任务。下面这些环境变量告诉它：
        # 去哪个 server 拉任务、使用哪个 agent_id、运行目录在哪里、如何访问 gateway。
        agent_env = {
            "PYTHONPATH": agent_pythonpath,
            "UENV_SERVER_ENDPOINT": f"{base.SERVER_PRIVATE_IP}:{base.SERVER_PORT}",
            "OPENHANDS_AGENT_POLL": "1",
            "OPENHANDS_AGENT_ID": agent_id,
            "OPENHANDS_AGENT_POOL_ID": "openhands-distributed-smoke",
            "OPENHANDS_AGENT_BRIDGE_ID": "uenv-agent-openhands",
            "OPENHANDS_AGENT_BRIDGE_VERSION": "1.0.0",
            "OPENHANDS_AGENT_MAX_CONCURRENT": str(args.concurrency),
            "OPENHANDS_POLL_INTERVAL_SEC": "0.2",
            "OPENHANDS_HEARTBEAT_INTERVAL_SEC": "2",
            "OPENHANDS_RUN_SCRIPT": f"{worker_run}/bundle/uenv-server/stress_test/run_openhands_stress.sh",
            "OPENHANDS_RUNS_DIR": f"{worker_run}/openhands",
            "OPENHANDS_RUNNER_API_BIND": f"127.0.0.1:{AGENT_API_PORT}",
            "OPENHANDS_RUNNER_HEALTH_BIND": f"127.0.0.1:{AGENT_HEALTH_PORT}",
            "OPENHANDS_SDK_DIR": "/opt/openhands/benchmarks/vendor/software-agent-sdk",
            "OPENHANDS_BENCHMARKS_DIR": "/opt/openhands/benchmarks",
            "OPENHANDS_PYTHON": "/opt/openhands/benchmarks/.venv/bin/python",
            "OPENHANDS_SUPPRESS_BANNER": "1",
            "OPENHANDS_MODE": args.mode,
            "OPENHANDS_MAX_ITERATIONS": str(args.openhands_max_iterations),
            "OPENHANDS_RUN_TIMEOUT_SEC": "900",
            "UENV_REPO": f"{worker_run}/bundle",
            "UENV_SWE_INSTANCES": f"{worker_run}/bundle/config/swe/verified.json",
            "UENV_SWE_RUNTIME": "docker",
            "UENV_GATEWAY_API_KEY": f"stress-gateway-{run_id}",
            "UENV_AGENT_BRIDGE_DIR": f"{worker_run}/bundle/integrations/openhands",
            "RUST_LOG": "warn",
        }
        if args.mode == "llm":
            agent_env["OPENHANDS_LLM_CONFIG"] = effective_llm_config
        agent_command = "env " + " ".join(
            f"{key}={base.q(value)}" for key, value in agent_env.items()
        ) + f" /opt/openhands/benchmarks/.venv/bin/python -B {base.q(worker_run)}/bundle/scripts/openhands/openhands_runner.py"
        agent_pid = base.start_owned(
            worker, agent_command, f"{worker_run}/agent.log", OPENHANDS_PYTHON,
            f"{worker_run}/bundle/scripts/openhands/openhands_runner.py",
        )
        wait_for_log(worker, f"{worker_run}/agent.log", "registered agent_id=", 120)
        base.assert_protected_unchanged(server, before_protected)

        # 资源监控在提交 episode 前启动，确保能观测到容器并发峰值。
        monitor_pid = base.start_owned(
            worker,
            f"python3 -B {worker_run}/resource_monitor.py --output {worker_run}/resources.jsonl",
            f"{worker_run}/resource-monitor.log",
            "/usr/bin/python3.12",
            f"{worker_run}/resource_monitor.py",
        )

        # manifest 记录本轮固定参数、镜像、PID、server 快照，便于复盘。
        manifest = {
            "run_id": run_id,
            "environment": "swe_openhands",
            "mode": args.mode,
            "parallel_mode": args.parallel_mode,
            "gate": 4,
            "scale_purpose": scale_purpose,
            "container_concurrency": args.concurrency,
            "registered_workers": args.registered_workers,
            "worker_capacity": args.worker_capacity,
            "worker_slots": args.registered_workers * args.worker_capacity,
            "total_episodes": target_episodes,
            "episode_batch_size": resolved_episode_batch_size,
            "episode_offset": args.episode_offset,
            "episode_ordinal_start": args.episode_offset,
            "episode_ordinal_end_exclusive": args.episode_offset + target_episodes,
            "min_episode_waves": args.min_episode_waves,
            "max_steps": args.max_steps,
            "openhands_max_iterations": args.openhands_max_iterations,
            "llm_kind": args.llm_kind,
            "llm_config": effective_llm_config if args.mode == "llm" else "",
            "llm_config_sha256": llm_config_sha256,
            "llm_preflight": llm_preflight,
            "server_addr": f"{base.SERVER_PRIVATE_IP}:{base.SERVER_PORT}",
            "worker_addr": f"{base.WORKER_PRIVATE_IP}:{worker_ports[0]}",
            "worker_id": worker_id,
            "worker_prefix": worker_prefix,
            "worker_ports": worker_ports,
            "gateway_ports": gateway_ports,
            "obs_ports": obs_ports,
            "agent_id": agent_id,
            "dataset": {
                "name": "SWE-bench Verified",
                "sampling": instance_sampling,
            },
            "selected_instance_ids": selected_instances,
            "docker_image_inventory": image_inventory,
            "selected_image_presence": selected_image_presence,
            "execution_boundary": "real OpenHands runner + real UEnv AgentControl/runtime gateway/container path; only the LLM endpoint is simulated when llm_kind=simulator",
            "llm_simulator": {
                "enabled": args.llm_kind == "simulator",
                "latency_ms": {
                    "legacy": args.simulator_latency_ms,
                    "mean": args.simulator_latency_mean_ms,
                    "std": args.simulator_latency_std_ms,
                    "min": args.simulator_latency_min_ms,
                    "max": args.simulator_latency_max_ms,
                    "zero_latency": args.simulator_zero_latency,
                    "miss_policy": "normal_distribution_when_trace_replay_misses",
                },
                "wrong_steps": {
                    "mean": args.simulator_wrong_steps_mean,
                    "std": args.simulator_wrong_steps_std,
                    "min": args.simulator_wrong_steps_min,
                    "max": args.simulator_wrong_steps_max,
                },
                "repair_success_rate": args.simulator_repair_success_rate,
                "repair_style": args.simulator_repair_style,
                "seed": args.simulator_seed,
                "mode": args.simulator_mode,
                "trace_corpus_path": args.trace_corpus_path,
                "effective_trace_corpus_path": effective_trace_corpus_path,
                "trace_sampling_strategy": args.trace_sampling_strategy,
            },
            "source_and_binaries": build,
            "protected_server": before_protected,
            "owned_pids": {
                "server": server_pid,
                "worker": worker_pid,
                "workers": [pid for pid, _ in worker_pids],
                "fleet_supervisor": fleet_supervisor_pid,
                "agent": agent_pid,
                "monitor": monitor_pid,
                "llm_simulator": llm_simulator_pid,
            },
            "fleet_metrics_path": fleet_metrics_path,
        }
        base.put_text(server, f"{server_run}/manifest.json", json.dumps(manifest, indent=2, sort_keys=True))
        base.put_text(worker, f"{worker_run}/manifest.json", json.dumps(manifest, indent=2, sort_keys=True))

        # 真正提交 SWE episode 的动作发生在 server 机器上的 smoke_client.py 中。
        client_command = " ".join([
            f"PYTHONPATH={server_run}:{server_run}/generated", "python3", "-B", f"{server_run}/smoke_client.py",
            "--server", f"{base.SERVER_PRIVATE_IP}:{base.SERVER_PORT}",
            "--worker-id", worker_id,
            "--worker-prefix", worker_prefix,
            "--expected-workers", str(args.registered_workers),
            "--worker-capacity", str(args.worker_capacity),
            "--total-episodes", str(target_episodes),
            "--episode-batch-size", str(resolved_episode_batch_size),
            "--episode-offset", str(args.episode_offset),
            "--registration-timeout", str(args.registration_timeout),
            "--batch-timeout", str(args.batch_timeout),
            "--run-id", run_id,
            "--driver", f"{worker_run}/bundle/integrations/openhands/run_swebenchpro_official.py",
            "--catalog", f"{worker_run}/bundle/config/swe/verified.json",
            "--output", f"{server_run}/result.json",
            "--concurrency", str(args.concurrency),
            "--mode", args.mode,
            "--parallel-mode", args.parallel_mode,
            "--max-steps", str(args.max_steps),
            "--openhands-max-iterations", str(args.openhands_max_iterations),
            "--llm-config", effective_llm_config if args.mode == "llm" else "",
            "--instance-ids-json", base.q(json.dumps(selected_instances)),
        ])
        client_status, client_out, client_err = base.run(
            server,
            client_command,
            timeout=args.registration_timeout + max(args.batch_timeout * ((target_episodes // resolved_episode_batch_size) + 1), 1100),
            check=False,
        )
        print("[smoke] client output")
        print(client_out)
        if client_err:
            print(client_err)
        result_text = base.get_text(server, f"{server_run}/result.json")
        result_document = json.loads(result_text)
        if args.registered_workers > 1 or scale_purpose.startswith("single_worker"):
            coverage = completed_worker_coverage(server, f"{server_run}/server.log", worker_prefix)
            coverage["expected_workers"] = args.registered_workers
            coverage["passed"] = coverage["unique_completed_workers"] == args.registered_workers
            result_document["worker_dispatch_coverage"] = coverage
            result_text = json.dumps(result_document, indent=2, sort_keys=True)
            if args.registered_workers > 1 and not coverage["passed"]:
                raise RuntimeError(
                    "not every Gate4 SWE Worker completed an episode: "
                    f"expected={args.registered_workers} actual={coverage['unique_completed_workers']}"
                )
        if args.llm_kind == "simulator":
            _, llm_stats_text, _ = base.run(
                worker,
                f"curl -fsS http://127.0.0.1:{base.MODEL_PORT}/stats",
                timeout=30,
            )
        else:
            llm_stats_text = "{}"
        llm_stats = json.loads(llm_stats_text or "{}")
        if args.llm_kind == "simulator" and args.simulator_mode == "trace_replay":
            if int(llm_stats.get("trace_replay_misses", 0)) != 0:
                raise RuntimeError(f"Gate4 trace replay had misses: {llm_stats}")
            if int(llm_stats.get("trace_replay_hits", 0)) <= 0:
                raise RuntimeError(f"Gate4 trace replay had no hits: {llm_stats}")
        _, explicit_corpus_paths_text, _ = base.run(
            worker,
            f"find {base.q(worker_run)}/openhands -name llm_trace_corpus_episode.json -type f -print",
            timeout=30,
            check=False,
        )
        _, rollout_trace_paths_text, _ = base.run(
            worker,
            f"find {base.q(worker_run)}/openhands -name llm_rollout_trace.json -type f -print",
            timeout=30,
            check=False,
        )
        trace_corpus_docs = []
        trace_corpus_dir = local_run / "trace-corpus"
        trace_corpus_dir.mkdir(exist_ok=True)
        trace_sources = [
            ("explicit_episode", line.strip())
            for line in explicit_corpus_paths_text.splitlines()
            if line.strip()
        ] + [
            ("rollout_trace", line.strip())
            for line in rollout_trace_paths_text.splitlines()
            if line.strip()
        ]
        for index, (source_kind, remote_corpus_path) in enumerate(trace_sources):
            try:
                corpus_text = base.get_text(worker, remote_corpus_path)
                raw_corpus_doc = json.loads(corpus_text)
                corpus_doc = normalize_trace_corpus_document(
                    raw_corpus_doc,
                    source_path=remote_corpus_path,
                    fallback_run_id=run_id,
                )
                if not corpus_doc:
                    raise ValueError("trace document does not contain replayable turns")
                corpus_doc.setdefault("source_kind", source_kind)
                trace_corpus_docs.append(corpus_doc)
                instance = str(corpus_doc.get("instance_id") or f"episode-{index}")
                safe_instance = re.sub(r"[^A-Za-z0-9_.-]+", "-", instance)
                (trace_corpus_dir / f"{index:05d}-{safe_instance}.json").write_text(
                    json.dumps(corpus_doc, indent=2, ensure_ascii=False, sort_keys=True)
                )
            except Exception as exc:
                trace_corpus_docs.append({
                    "schema_version": 1,
                    "corpus_kind": "openhands_real_llm_episode_fetch_error",
                    "source_kind": source_kind,
                    "remote_path": remote_corpus_path,
                    "error": str(exc),
                })
        base.stop_owned(worker, monitor_pid, "/usr/bin/python3.12", f"{worker_run}/resource_monitor.py")
        monitor_pid = None
        resources_text = base.get_text(worker, f"{worker_run}/resources.jsonl")
        # Resource metrics are observational: they are reported for analysis,
        # while Gate4 pass/fail remains based on protocol, trace and coverage.
        resource_rows = [json.loads(line) for line in resources_text.splitlines() if line.strip()]
        resource_summary = summarize_resource_rows(resource_rows)
        peak_containers = int(resource_summary["peak_running_containers"])
        if peak_containers < args.concurrency:
            raise RuntimeError(
                f"did not observe requested real container concurrency: requested={args.concurrency} peak={peak_containers}"
            )
        fleet_metrics = {}
        if fleet_metrics_path:
            try:
                fleet_metrics = json.loads(base.get_text(worker, fleet_metrics_path))
            except (OSError, json.JSONDecodeError):
                fleet_metrics = {}
        resource_observations = {
            "observation_only": True,
            "container_concurrency": {
                "requested": args.concurrency,
                "peak_running_containers": peak_containers,
                "observed_requested_concurrency": peak_containers >= args.concurrency,
            },
            "worker_host": resource_summary,
            "worker_fleet": summarize_fleet_metrics(fleet_metrics, args.registered_workers),
        }
        result_document["resource_observations"] = resource_observations
        result_document["fleet_resource_metrics"] = resource_observations["worker_fleet"]
        result_document["host_resource_metrics"] = resource_observations["worker_host"]
        replayable_trace_docs = [
            item for item in trace_corpus_docs
            if isinstance(item, dict) and isinstance(item.get("turns"), list) and item.get("turns")
        ]
        valid_result_count = sum(1 for item in result_document.get("results", []) if item.get("training_trace_valid"))
        result_document["trace_corpus_collection"] = {
            "required_valid_episodes": args.instance_count,
            "explicit_episode_files": len([line for line in explicit_corpus_paths_text.splitlines() if line.strip()]),
            "rollout_trace_files": len([line for line in rollout_trace_paths_text.splitlines() if line.strip()]),
            "replayable_documents": len(replayable_trace_docs),
            "valid_training_trace_results": valid_result_count,
            "complete_for_configured_requirement": len(replayable_trace_docs) >= args.instance_count,
            "partial_corpus_preserved": len(replayable_trace_docs) > 0 and len(replayable_trace_docs) < args.instance_count,
            "note": "Real-LLM trace collection may preserve replayable documents even when the batch fails due to provider rate limits.",
        }
        result_text = json.dumps(result_document, indent=2, sort_keys=True)
        (local_run / "result.json").write_text(result_text)
        (local_run / "resources.jsonl").write_text(resources_text)
        (local_run / "resource-summary.json").write_text(json.dumps(resource_summary, indent=2, sort_keys=True))
        (local_run / "llm-simulator-stats.json").write_text(llm_stats_text)
        if fleet_metrics:
            (local_run / "fleet-metrics.json").write_text(json.dumps(fleet_metrics, indent=2, sort_keys=True))
        (local_run / "trace-corpus.jsonl").write_text(
            "\n".join(json.dumps(item, ensure_ascii=False, sort_keys=True) for item in trace_corpus_docs) + ("\n" if trace_corpus_docs else "")
        )
        (local_run / "manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True))
        for client, remote_path, local_name in (
            (server, f"{server_run}/server.log", "server.log"),
            (worker, f"{worker_run}/worker-0000.log", "worker-0000.log"),
            (worker, f"{worker_run}/worker-{args.registered_workers - 1:04d}.log", f"worker-{args.registered_workers - 1:04d}.log"),
            (worker, f"{worker_run}/logs/{worker_prefix}0000.runtime.log", "worker-0000.runtime.log"),
            (worker, f"{worker_run}/logs/{worker_prefix}{args.registered_workers - 1:04d}.runtime.log", f"worker-{args.registered_workers - 1:04d}.runtime.log"),
            (worker, f"{worker_run}/agent.log", "agent.log"),
        ):
            try:
                (local_run / local_name).write_text(base.get_text(client, remote_path))
            except OSError:
                pass
        if client_status != 0:
            raise RuntimeError(
                "Gate4 infrastructure/trace validation failed: "
                f"{result_document.get('infrastructure')}"
            )
        print(f"[smoke] PASS run_id={run_id} local_artifacts={local_run}")
        result_code = 0
    except Exception as exc:
        error = f"{type(exc).__name__}: {exc}"
        print(f"[smoke] FAIL {error}")
    finally:
        # 真实 SWE episode 可能运行较久，原来的 SSH 连接可能已经空闲断开。
        # 清理时重新建立带指纹校验的 SSH 连接，再停止本次拥有的进程。
        had_worker_connection = worker is not None
        had_server_connection = server is not None
        for stale in (worker, server):
            if stale:
                stale.close()
        worker = server = None
        if had_worker_connection:
            try:
                worker = base.connect(base.WORKER_HOST, password)
            except Exception as exc:
                cleanup_errors.append(f"worker cleanup reconnect failed: {exc}")
        if had_server_connection:
            try:
                server = base.connect(base.SERVER_HOST, password)
            except Exception as exc:
                cleanup_errors.append(f"server cleanup reconnect failed: {exc}")
        if worker:
            if fleet_supervisor_pid:
                try:
                    base.stop_owned(
                        worker,
                        fleet_supervisor_pid,
                        "/usr/bin/python3.12",
                        f"{worker_run}/worker_fleet_supervisor.py",
                    )
                except Exception as exc:
                    cleanup_errors.append(str(exc))
            else:
                for pid, config in reversed(worker_pids):
                    try:
                        base.stop_owned(worker, pid, f"{worker_run}/bundle/uenv-worker", config)
                    except Exception as exc:
                        cleanup_errors.append(str(exc))
            for pid, exe, fragment in (
                (monitor_pid, "/usr/bin/python3.12", f"{worker_run}/resource_monitor.py"),
                (agent_pid, OPENHANDS_PYTHON, f"{worker_run}/bundle/scripts/openhands/openhands_runner.py"),
                (llm_simulator_pid, "/usr/bin/python3.12", f"{worker_run}/llm_simulator.py"),
            ):
                try:
                    base.stop_owned(worker, pid, exe, fragment)
                except Exception as exc:
                    cleanup_errors.append(str(exc))
            try:
                # 只删除本次新增的容器，并检查容器集合是否恢复到开始前状态。
                new_containers = container_ids(worker) - before_containers
                if new_containers:
                    base.run(worker, "docker rm -f " + " ".join(sorted(new_containers)), timeout=120)
                    print(f"[cleanup] removed owned container IDs={sorted(new_containers)}")
                if container_ids(worker) != before_containers:
                    cleanup_errors.append("container set did not return to baseline")
            except Exception as exc:
                cleanup_errors.append(str(exc))
        if server:
            try:
                base.stop_owned(server, server_pid, base.SERVER_BIN, base.SERVER_BIN)
            except Exception as exc:
                cleanup_errors.append(str(exc))
            try:
                if before_protected:
                    base.assert_protected_unchanged(server, before_protected)
                base.assert_port_free(server, base.SERVER_PORT, base.SERVER_HOST)
            except Exception as exc:
                cleanup_errors.append(str(exc))
        if worker:
            try:
                base.assert_ports_free(worker, worker_ports + obs_ports + gateway_ports + fixed_worker_ports, base.WORKER_HOST)
            except Exception as exc:
                cleanup_errors.append(str(exc))
        if cleanup_errors:
            print("[cleanup] ERRORS " + " | ".join(cleanup_errors))
            result_code = 1
        else:
            print("[cleanup] owned processes/containers stopped; protected server unchanged")
        if error:
            (local_run / "error.txt").write_text(error)
        for client in (worker, server):
            if client:
                client.close()
    return result_code


def main() -> int:
    """Gate4 命令行入口。

    默认跑 1 和 2 两轮；也可以传 --concurrency 1 或 --concurrency 2
    只跑某一轮。每轮都会生成独立 run_id 和 artifact 目录。
    """
    global GATEWAY_PORT, AGENT_API_PORT, AGENT_HEALTH_PORT
    parser = argparse.ArgumentParser()
    parser.add_argument("--artifacts", type=Path, default=Path.cwd() / "distributed-gate-artifacts")
    parser.add_argument(
        "--concurrency",
        type=int,
        action="append",
        help="Run selected container concurrency. Repeat as needed. Omit to run the safe 1 then 2 sequence.",
    )
    parser.add_argument("--mode", choices=("gold", "llm"), default="llm")
    parser.add_argument(
        "--parallel-mode",
        action="append",
        choices=("sync", "one_step_off_policy", "fully_async"),
        help=(
            "UEnv SampleEnvelope parallel_mode to run. Repeat as needed; "
            "defaults to sync, one_step_off_policy and fully_async."
        ),
    )
    parser.add_argument("--llm-kind", choices=("real", "simulator"), default="simulator")
    parser.add_argument("--max-steps", type=int, default=10)
    parser.add_argument("--openhands-max-iterations", type=int, default=10)
    parser.add_argument("--instance-count", type=int, default=4)
    parser.add_argument("--instance-seed", type=int, default=20260720)
    parser.add_argument("--simulator-latency-ms", type=float, default=250)
    parser.add_argument("--simulator-latency-mean-ms", type=float, default=250)
    parser.add_argument("--simulator-latency-std-ms", type=float, default=75)
    parser.add_argument("--simulator-latency-min-ms", type=float, default=0)
    parser.add_argument("--simulator-latency-max-ms", type=float, default=2000)
    parser.add_argument("--simulator-zero-latency", action="store_true")
    parser.add_argument("--simulator-wrong-steps-mean", type=float, default=2)
    parser.add_argument("--simulator-wrong-steps-std", type=float, default=1)
    parser.add_argument("--simulator-wrong-steps-min", type=int, default=0)
    parser.add_argument("--simulator-wrong-steps-max", type=int, default=5)
    parser.add_argument("--simulator-repair-success-rate", type=float, default=0.35)
    parser.add_argument(
        "--simulator-repair-style",
        choices=("plausible_patch", "noisy_patch", "noop"),
        default="plausible_patch",
    )
    parser.add_argument("--simulator-seed", type=int, default=20260720)
    parser.add_argument("--simulator-mode", choices=("template", "trace_replay"), default="template")
    parser.add_argument("--trace-corpus-path", default="")
    parser.add_argument("--trace-sampling-strategy", choices=("instance_then_turn", "turn_only"), default="instance_then_turn")
    parser.add_argument(
        "--llm-config",
        default=os.environ.get("OPENHANDS_LLM_CONFIG", ""),
        help="Path on the worker host to a real OpenHands LLM config. Required when --mode llm.",
    )
    parser.add_argument("--gateway-port", type=int, default=8777)
    parser.add_argument("--agent-api-port", type=int, default=8077)
    parser.add_argument("--agent-health-port", type=int, default=8088)
    parser.add_argument("--registered-workers", type=int, default=1)
    parser.add_argument("--worker-capacity", type=int, default=0)
    parser.add_argument("--total-episodes", type=int, default=0)
    parser.add_argument("--episode-batch-size", type=int, default=0)
    parser.add_argument("--episode-offset", type=int, default=0)
    parser.add_argument("--min-scale-episode-waves", type=int, default=10)
    parser.add_argument("--private-worker-port-range", default="")
    parser.add_argument("--private-gateway-port-range", default="")
    parser.add_argument("--fleet-supervisor-threshold", type=int, default=16)
    parser.add_argument("--registration-timeout", type=int, default=900)
    parser.add_argument("--batch-timeout", type=int, default=1800)
    base.add_runtime_arguments(parser, require_code_plugin=False)
    args = parser.parse_args()
    base.configure_from_args(args)
    GATEWAY_PORT = args.gateway_port
    AGENT_API_PORT = args.agent_api_port
    AGENT_HEALTH_PORT = args.agent_health_port
    allowed_exposed_ports = {5432, 6379, 8000, 8077, 8088, 8099, 8777, 8888}
    exposed = {
        "isolated server": base.SERVER_PORT,
        "LLM simulator": base.MODEL_PORT,
        "agent API": AGENT_API_PORT,
        "agent health": AGENT_HEALTH_PORT,
    }
    if args.registered_workers == 1:
        exposed["SWE worker"] = base.WORKER_PORT
        exposed["runtime gateway"] = GATEWAY_PORT
    for label, port in exposed.items():
        if port not in allowed_exposed_ports:
            raise SystemExit(f"{label} port {port} is outside the explicitly allowed cloud ports")
    if base.SERVER_PORT in base.PROTECTED_PORTS:
        raise SystemExit("isolated server port must not overlap a protected production port")
    if args.max_steps <= 0:
        raise SystemExit("--max-steps must be positive")
    if args.openhands_max_iterations <= 0:
        raise SystemExit("--openhands-max-iterations must be positive")
    if args.mode == "llm" and args.llm_kind == "real" and not args.llm_config:
        raise SystemExit("--mode llm --llm-kind real requires --llm-config or OPENHANDS_LLM_CONFIG")
    if args.instance_count <= 0:
        raise SystemExit("--instance-count must be positive")
    if args.instance_count < 50:
        raise SystemExit("--instance-count must be at least 50 for SWE-bench Verified coverage")
    if args.registered_workers <= 0:
        raise SystemExit("--registered-workers must be positive")
    if args.worker_capacity < 0:
        raise SystemExit("--worker-capacity must be non-negative")
    if args.worker_capacity == 0:
        args.worker_capacity = args.concurrency[0] if args.registered_workers == 1 and args.concurrency else 1
    if args.worker_capacity <= 0:
        raise SystemExit("--worker-capacity must resolve to a positive value")
    if args.registered_workers > 1:
        if args.registered_workers < 1024:
            raise SystemExit("Gate4 scale mode requires at least 1024 registered Workers")
        if not args.private_worker_port_range or not args.private_gateway_port_range:
            raise SystemExit("Gate4 multi-Worker scale requires --private-worker-port-range and --private-gateway-port-range")
        if args.llm_kind != "simulator" or args.simulator_mode != "trace_replay":
            raise SystemExit("Gate4 1024 Worker scale requires simulator trace_replay LLM")
        required_episodes = args.registered_workers * args.worker_capacity * args.min_scale_episode_waves
        if args.total_episodes and args.total_episodes < required_episodes:
            raise SystemExit("Gate4 scale total episodes must be at least registered_workers * worker_capacity * min_scale_episode_waves")
    if args.total_episodes < 0 or args.episode_batch_size < 0 or args.episode_offset < 0:
        raise SystemExit("--total-episodes, --episode-batch-size and --episode-offset must be non-negative")
    if args.fleet_supervisor_threshold < 2:
        raise SystemExit("--fleet-supervisor-threshold must be at least 2")
    if args.registration_timeout <= 0 or args.batch_timeout <= 0:
        raise SystemExit("--registration-timeout and --batch-timeout must be positive")
    if args.simulator_latency_ms < 0:
        raise SystemExit("--simulator-latency-ms must be non-negative")
    if not (
        0
        <= args.simulator_latency_min_ms
        <= args.simulator_latency_mean_ms
        <= args.simulator_latency_max_ms
    ):
        raise SystemExit("simulator latency must satisfy 0 <= min <= mean <= max")
    if args.simulator_latency_std_ms < 0:
        raise SystemExit("simulator latency std must be non-negative")
    if args.simulator_zero_latency:
        args.simulator_latency_ms = 0.0
        args.simulator_latency_mean_ms = 0.0
        args.simulator_latency_std_ms = 0.0
        args.simulator_latency_min_ms = 0.0
        args.simulator_latency_max_ms = 0.0
    if args.simulator_wrong_steps_std < 0:
        raise SystemExit("--simulator-wrong-steps-std must be non-negative")
    if not (
        0
        <= args.simulator_wrong_steps_min
        <= args.simulator_wrong_steps_mean
        <= args.simulator_wrong_steps_max
    ):
        raise SystemExit("simulator wrong_steps must satisfy 0 <= min <= mean <= max")
    if args.simulator_wrong_steps_max >= args.max_steps:
        raise SystemExit("--simulator-wrong-steps-max must be smaller than --max-steps")
    if not 0 <= args.simulator_repair_success_rate <= 1:
        raise SystemExit("--simulator-repair-success-rate must be in [0, 1]")
    if args.llm_kind == "simulator" and args.simulator_mode == "trace_replay" and not args.trace_corpus_path:
        raise SystemExit("--simulator-mode trace_replay requires --trace-corpus-path")
    args.artifacts.mkdir(parents=True, exist_ok=True)

    concurrencies = args.concurrency or [1, 2]
    parallel_modes = args.parallel_mode or ["sync", "one_step_off_policy", "fully_async"]
    summary = []
    final_code = 0
    for parallel_mode in parallel_modes:
        for concurrency in concurrencies:
            print(f"[gate4] parallel_mode={parallel_mode} concurrency={concurrency} start", flush=True)
            started = time.monotonic()
            returncode = run_one(
                concurrency,
                args.artifacts,
                args.mode,
                parallel_mode,
                args.max_steps,
                args.openhands_max_iterations,
                args.llm_config,
                args.llm_kind,
                args.instance_count,
                args.instance_seed,
                args.simulator_latency_ms,
                args.simulator_latency_mean_ms,
                args.simulator_latency_std_ms,
                args.simulator_latency_min_ms,
                args.simulator_latency_max_ms,
                args.simulator_zero_latency,
                args.simulator_wrong_steps_mean,
                args.simulator_wrong_steps_std,
                args.simulator_wrong_steps_min,
                args.simulator_wrong_steps_max,
                args.simulator_repair_success_rate,
                args.simulator_repair_style,
                args.simulator_seed,
                args.simulator_mode,
                args.trace_corpus_path,
                args.trace_sampling_strategy,
                args.registered_workers,
                args.worker_capacity,
                args.total_episodes,
                args.episode_batch_size,
                args.episode_offset,
                args.min_scale_episode_waves,
                args.private_worker_port_range,
                args.private_gateway_port_range,
                args.fleet_supervisor_threshold,
                args.registration_timeout,
                args.batch_timeout,
            )
            item = {
                "parallel_mode": parallel_mode,
                "container_concurrency": concurrency,
                "registered_workers": args.registered_workers,
                "worker_capacity": args.worker_capacity,
                "total_episodes": args.total_episodes
                or (args.registered_workers * args.worker_capacity * args.min_scale_episode_waves),
                "episode_offset": args.episode_offset,
                "returncode": returncode,
                "wall_seconds": time.monotonic() - started,
            }
            latest_result = newest_local_artifact(args.artifacts, "result.json")
            if latest_result is not None:
                try:
                    result_document = json.loads(latest_result.read_text(encoding="utf-8"))
                    item["result_path"] = str(latest_result)
                    if "scale" in result_document:
                        item["scale"] = result_document["scale"]
                    for key in ("resource_observations", "host_resource_metrics", "fleet_resource_metrics"):
                        if key in result_document:
                            item[key] = result_document[key]
                except (OSError, json.JSONDecodeError):
                    item["result_path"] = str(latest_result)
                    item["resource_observations_error"] = "failed to parse latest result.json"
            summary.append(item)
            print(f"[gate4] parallel_mode={parallel_mode} concurrency={concurrency} done returncode={returncode}", flush=True)
            if returncode != 0:
                final_code = returncode
                break
        if final_code != 0:
            break

    summary_path = args.artifacts / f"gate4-summary-{time.strftime('%Y%m%d-%H%M%S')}.json"
    summary_path.write_text(json.dumps(summary, indent=2, sort_keys=True))
    print(f"[gate4] summary={summary_path}")
    return final_code


if __name__ == "__main__":
    raise SystemExit(main())
