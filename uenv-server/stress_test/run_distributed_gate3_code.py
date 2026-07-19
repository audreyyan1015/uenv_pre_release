#!/usr/bin/env python3
"""分布式 Gate3：真实 Code worker 多轮与扩容压测。

运行位置说明：
- 这个脚本从 8.130.75.157 的 stress_test 目录启动。
- 隔离 server 启动在 8.130.75.157:8099。
- 真实 Code worker 启动在 8.130.86.71。
- 已经在线的正式 adapter-core 只做保护检查，不复用、不停止。

默认只启动一个 Worker，使用明确获准的 8099/8000/8888 端口完成多轮 smoke。
多 Worker 必须通过 --private-worker-port-range 显式提供已开放的私网端口范围。
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import tempfile
import time
import uuid

import distributed_stress_runtime as base


# 三种并行模式都要测，避免只验证同步路径。
MODES = ("sync", "one_step_off_policy", "fully_async")

# load_client.py 会被写入远端 /tmp/uenv-<run_id>/ 运行目录。
# 它需要导入 stress_test_common.py，所以这里提前读取文件内容，后面一并下发。
COMMON_SOURCE = Path(__file__).with_name("stress_test_common.py").read_text(encoding="utf-8")
REAL_LLM_PROXY_SOURCE = Path(__file__).with_name("ark_real_llm_proxy.py").read_text(encoding="utf-8")
REAL_LLM_PREFLIGHT_SOURCE = Path(__file__).with_name("ark_real_llm_preflight.py").read_text(encoding="utf-8")
FLEET_SUPERVISOR_SOURCE = Path(__file__).with_name("worker_fleet_supervisor.py").read_text(encoding="utf-8")


MODEL_SIMULATOR = r'''#!/usr/bin/env python3
# 这个脚本会临时写到 worker 机器上运行。
# 它提供一个最小 OpenAI-compatible HTTP 接口，默认前几轮返回错误代码，
# 后续返回正确代码。这样 Gate3 可以真实经过多 step，而不是第一步就结束。
import argparse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import re
import threading
import time
from urllib.parse import parse_qs, urlparse

parser = argparse.ArgumentParser()
parser.add_argument("--port", type=int, required=True)
parser.add_argument("--latency-ms", type=int, default=1000)
parser.add_argument("--wrong-steps", type=int, default=2)
parser.add_argument("--dataset-jsonl", default="")
args = parser.parse_args()
attempts_by_task = {}
attempts_lock = threading.Lock()
dataset_oracle = {}
if args.dataset_jsonl:
    with open(args.dataset_jsonl, "r", encoding="utf-8") as source:
        for line in source:
            if line.strip():
                row = json.loads(line)
                dataset_oracle[str(row["problem_id"])] = str(row["ground_truth_code"])

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
                if not prefix or task_id.startswith(prefix)
            }
        histogram = {}
        for count in counts.values():
            histogram[str(count)] = histogram.get(str(count), 0) + 1
        ordered = sorted(counts.values())
        body = json.dumps({
            "prefix": prefix,
            "task_count": len(ordered),
            "total_model_calls": sum(ordered),
            "min_steps": min(ordered) if ordered else 0,
            "max_steps": max(ordered) if ordered else 0,
            "step_histogram": histogram,
            "examples": dict(list(sorted(counts.items()))[:20]),
        }).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        size = int(self.headers.get("content-length", "0"))
        raw = b""
        if size:
            raw = self.rfile.read(size)
        time.sleep(args.latency_ms / 1000)
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
        if attempt <= args.wrong_steps:
            content = "```python\nraise NotImplementedError('deterministic scale warmup')\n```"
        elif dataset_problem_id in dataset_oracle:
            content = "```python\n" + dataset_oracle[dataset_problem_id] + "\n```"
        else:
            content = "```python\ndef add(a, b):\n    return a + b\n```"
        body = json.dumps({
            "choices": [{
                "message": {"role": "assistant", "content": content},
                "finish_reason": "stop",
                "logprobs": {"content": [{"token": content, "token_id": 42, "logprob": -0.1}]}
            }],
            "usage": {"prompt_tokens": 8, "completion_tokens": 8},
            "uenv_model_version": {
                "rollout_param_version": 1,
                "rollout_policy_version": "gate3-policy-1"
            },
            "uenv_response_ids": [42]
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
parser.add_argument("--dataset-jsonl", required=True)
parser.add_argument("--dataset-limit", type=int, default=8)
parser.add_argument("--dataset-offset", type=int, default=0)
parser.add_argument("--registration-timeout", type=int, default=180)
parser.add_argument("--batch-timeout", type=int, default=180)
parser.add_argument("--exact-batches", type=int, default=0)
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

    submitted = completed = failed = rpc_errors = protocol_errors = 0
    rewards = []
    latencies = []
    actual_step_counts = []
    training_trace_token_counts = []
    lock = asyncio.Lock()
    semaphore = asyncio.Semaphore(args.slots)
    tasks = set()

    batch_sequence = 0

    def make_sample(batch_id, index, sample_ordinal):
        task_id = f"gate3-{args.run_id}-{args.mode}-{batch_id}-{index}"
        mode_offset = {
            "sync": 0,
            "one_step_off_policy": args.workers,
            "fully_async": args.workers * 2,
        }.get(args.mode, 0)
        row = dataset_rows[(mode_offset + sample_ordinal) % len(dataset_rows)]
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
                "dataset_problem_id": row["problem_id"],
            },
            timeout_seconds=args.batch_timeout,
            max_steps=args.max_steps,
            model_url=args.model_url,
            model_name=args.model_name,
        )

    async def send_batch():
        # 一次 ExecuteBatch 发送 args.workers 个样本。外层 semaphore 控制同时在飞的
        # batch 数，args.slots 表示每个 worker 的并发容量。
        nonlocal submitted, completed, failed, rpc_errors, protocol_errors, batch_sequence
        batch_id = str(uuid.uuid4())
        current_batch = batch_sequence
        batch_sequence += 1
        samples = [
            make_sample(batch_id, index, current_batch * args.workers + index)
            for index in range(args.workers)
        ]
        async with lock:
            submitted += len(samples)
        started = time.monotonic()
        try:
            response = await execute(
                adapter_core_pb2.ExecuteBatchRequest(
                    request_id=batch_id, batch_id=batch_id, samples=samples
                ), timeout=args.batch_timeout,
            )
            elapsed = (time.monotonic() - started) * 1000
            async with lock:
                latencies.append(elapsed)
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
        except grpc.RpcError:
            async with lock:
                rpc_errors += len(samples)
        finally:
            semaphore.release()

    # 在 duration 时间窗口内持续补充 batch。时间到后等待已经发出的 batch 收尾。
    started = time.monotonic()
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
    if tasks:
        await asyncio.gather(*tasks)
    elapsed = time.monotonic() - started
    # 统一用公共模块生成结果，保证 Gate3 和其它脚本的统计字段含义一致。
    document = stress_common.gate3_result_document(
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
    document["dataset"] = {
        "name": "DSCodeBench",
        "path": args.dataset_jsonl,
        "loaded_rows": len(dataset_rows),
        "offset": args.dataset_offset,
        "problem_ids": [str(row["problem_id"]) for row in dataset_rows],
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
import argparse, socket
p = argparse.ArgumentParser(); p.add_argument("host"); p.add_argument("port", type=int)
a = p.parse_args(); socket.create_connection((a.host, a.port), 5).close(); print("tcp_probe=ok")
'''


def server_config(workers: int, capacity: int) -> str:
    """生成隔离 server 的配置。

    这里的 server 只服务本次 Gate3 压测，绑定 8099，不使用正式 server。
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


def worker_config(run_dir: str, worker_id: str, port: int, obs_port: int, capacity: int) -> str:
    """生成单个 Code worker 的配置。

    worker 监听 0.0.0.0:<port>，并把内网地址 advertise 给 server。
    env.types 只启用 code，plugin_dir 指向本次压测解包出来的 bundle。
    """
    return f'''server:
  endpoint: "{base.SERVER_PRIVATE_IP}:{base.SERVER_PORT}"
worker:
  id: "{worker_id}"
  listen: "0.0.0.0:{port}"
  advertise_endpoint: "{base.WORKER_PRIVATE_IP}:{port}"
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


def _model_step_stats(worker, prefix: str) -> dict:
    code = (
        "import urllib.parse,urllib.request; "
        f"prefix={prefix!r}; "
        f"url='http://127.0.0.1:{base.MODEL_PORT}/stats?prefix='+urllib.parse.quote(prefix); "
        "print(urllib.request.urlopen(url, timeout=10).read().decode())"
    )
    _, output, _ = base.run(worker, f"python3 -c {base.q(code)}", timeout=20)
    return json.loads(output)


def _completed_worker_coverage(server, log_path: str, worker_prefix: str) -> dict:
    worker_ids = set()
    for line in base.get_text(server, log_path).splitlines():
        if "episode_completed" not in line:
            continue
        match = re.search(r"worker_id=([^\s]+)", line)
        worker_id = match.group(1).strip('"') if match else ""
        if worker_id.startswith(worker_prefix):
            worker_ids.add(worker_id)
    return {
        "unique_completed_workers": len(worker_ids),
        "worker_ids": sorted(worker_ids),
    }


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
    simulator_latency_ms: int,
    acceptance_purpose: str,
) -> dict:
    """执行一组 Gate3 规模。

    workers_count 是 worker 数，capacity 是每个 worker 的并发 slot 数。
    这个函数完成以下步骤：
    1. 连接 server/worker 两台机器；
    2. 检查正式 server 未被影响、端口空闲；
    3. 打包 worker 和 code plugin，从 server 机器传到 worker 机器；
    4. 启动隔离 server、模型模拟器和真实 Code worker；
    5. 依次跑三种 parallel_mode；
    6. 清理本次启动的进程并复查端口。
    """
    run_id = f"gate3-code-{workers_count}x{capacity}-{time.strftime('%Y%m%d-%H%M%S')}-{uuid.uuid4().hex[:6]}"
    server_run = f"/tmp/uenv-{run_id}"
    worker_run = f"/opt/uenv-stress/runs/{run_id}"
    local_run = artifacts / run_id
    local_run.mkdir(parents=True)
    server = worker = None
    server_pid = model_pid = fleet_supervisor_pid = None
    worker_pids: list[tuple[int, str]] = []
    fleet_pid_document: dict = {}
    fleet_metrics_path = ""
    fleet_resource_metrics: dict = {}
    before = None
    cleanup_errors: list[str] = []
    results: list[dict] = []
    error: str | None = None
    outcome: dict | None = None
    ports = worker_ports
    obs_ports = [base.OBS_PORT + i for i in range(workers_count)]
    model_script = "model_simulator.py"
    llm_config_sha256 = ""
    try:
        # 连接两台机器后，第一件事是记录正式 server 快照。
        server = base.connect(base.SERVER_HOST, password)
        worker = base.connect(base.WORKER_HOST, password)
        before = base.protected_snapshot(server)
        base.assert_port_free(server, base.SERVER_PORT, base.SERVER_HOST)
        base.assert_ports_free(worker, ports + obs_ports + [base.MODEL_PORT], base.WORKER_HOST)
        print(f"[gate3:{workers_count}x{capacity}] preflight ports={ports[0]}-{ports[-1]}", flush=True)
        build = base.source_and_binary_manifest(server, include_code_plugin=True)
        base.run(server, f"test -f {base.q(dataset_jsonl)}", timeout=20)
        _, dataset_hash_out, _ = base.run(server, f"sha256sum {base.q(dataset_jsonl)}", timeout=30)
        dataset_sha256 = dataset_hash_out.split()[0]

        # bundle 先在 server 机器上制作，再通过本地临时文件转传到 worker 机器。
        # 这样 worker 机器不需要直接访问 /home/uenv 源码目录。
        base.run(server, f"install -d -m 0755 {base.q(server_run)}/bundle/plugins/code/scripts {base.q(server_run)}/generated/uenv/v1")
        base.run(worker, f"install -d -m 0755 {base.q(worker_run)}/logs {base.q(worker_run)}/wal")
        base.run(server, " && ".join([
            f"install -m 0755 {base.q(base.SOURCE_WORKER_BIN)} {base.q(server_run)}/bundle/uenv-worker",
            f"install -m 0755 {base.q(base.SOURCE_CODE_BIN)} {base.q(server_run)}/bundle/uenv-code-plugin",
            f"strip {base.q(server_run)}/bundle/uenv-worker {base.q(server_run)}/bundle/uenv-code-plugin",
            f"cp -a {base.q(base.SOURCE_REPO)}/plugins/code/. {base.q(server_run)}/bundle/plugins/code/",
            f"tar -C {base.q(server_run)}/bundle -czf {base.q(server_run)}/bundle.tgz .",
        ]), timeout=180)
        with tempfile.NamedTemporaryFile(prefix=run_id, suffix=".tgz", delete=False) as tmp:
            local_bundle = Path(tmp.name)
        try:
            with server.open_sftp() as sftp: sftp.get(f"{server_run}/bundle.tgz", str(local_bundle))
            with worker.open_sftp() as sftp: sftp.put(str(local_bundle), f"{worker_run}/bundle.tgz")
        finally:
            local_bundle.unlink(missing_ok=True)
        base.run(worker, f"install -d -m 0755 {base.q(worker_run)}/bundle && tar -C {base.q(worker_run)}/bundle -xzf {base.q(worker_run)}/bundle.tgz")
        base.put_text(server, f"{server_run}/server.yaml", server_config(workers_count, capacity))
        base.put_text(server, f"{server_run}/load_client.py", LOAD_CLIENT, 0o755)
        base.put_text(server, f"{server_run}/stress_test_common.py", COMMON_SOURCE)
        base.put_text(server, f"{server_run}/tcp_probe.py", TCP_PROBE, 0o755)
        base.put_text(worker, f"{worker_run}/model_simulator.py", MODEL_SIMULATOR, 0o755)
        base.put_text(worker, f"{worker_run}/ark_real_llm_proxy.py", REAL_LLM_PROXY_SOURCE, 0o700)
        base.put_text(worker, f"{worker_run}/ark_real_llm_preflight.py", REAL_LLM_PREFLIGHT_SOURCE, 0o700)
        base.put_text(worker, f"{worker_run}/worker_fleet_supervisor.py", FLEET_SUPERVISOR_SOURCE, 0o700)
        worker_dataset = f"{worker_run}/DSCodeBench.json"
        if model_mode == "simulator":
            with tempfile.NamedTemporaryFile(prefix=run_id, suffix="-dscodebench.json", delete=False) as tmp:
                local_dataset = Path(tmp.name)
            try:
                with server.open_sftp() as sftp:
                    sftp.get(dataset_jsonl, str(local_dataset))
                with worker.open_sftp() as sftp:
                    sftp.put(str(local_dataset), worker_dataset)
            finally:
                local_dataset.unlink(missing_ok=True)
        worker_documents = {}
        for i, (port, obs_port) in enumerate(zip(ports, obs_ports)):
            worker_id = f"stress-{run_id}-worker-{i:04d}"
            worker_documents[f"{worker_run}/worker-{i:04d}.yaml"] = (
                worker_config(worker_run, worker_id, port, obs_port, capacity),
                0o600,
            )
        base.put_texts(worker, worker_documents)
        # load_client.py 需要 Python 版 protobuf message，所以每次用当前 proto 生成。
        proto_root = f"{base.SOURCE_REPO}/proto"
        proto = " ".join([
            "/usr/bin/protoc", "-I", base.q(proto_root), f"--python_out={base.q(server_run)}/generated",
            f"{base.q(proto_root)}/uenv/v1/common.proto", f"{base.q(proto_root)}/uenv/v1/episode.proto",
            f"{base.q(proto_root)}/uenv/v1/scheduler.proto", f"{base.q(proto_root)}/uenv/v1/adapter_core.proto",
        ])
        base.run(server, proto)
        base.run(server, f"touch {base.q(server_run)}/generated/uenv/__init__.py {base.q(server_run)}/generated/uenv/v1/__init__.py")
        # 关闭 trajectory/obs，减少压测以外的写入和背景工作。
        # Scale acceptance needs the assignment.worker_id on every completion
        # to prove that all real Workers, rather than only N registry entries,
        # actually executed an episode.
        server_log_filter = "info" if acceptance_purpose == "worker-scale" else "warn"
        server_cmd = " ".join([
            "env", "UENV_SERVER_CONFIG_STRICT=1", "UENV_TRAJECTORY_ENABLED=0", "UENV_OBS_ENABLED=0", "UENV_LOG_ANSI=0",
            f"UENV_ADDR={base.SERVER_PRIVATE_IP}:{base.SERVER_PORT}", f"UENV_CONFIG_PATH={server_run}/server.yaml",
            f"RUST_LOG={server_log_filter}", base.SERVER_BIN,
        ])
        server_pid = base.start_owned(server, server_cmd, f"{server_run}/server.log", base.SERVER_BIN, base.SERVER_BIN)
        if model_mode == "real":
            _, mode_out, _ = base.run(worker, f"stat -c %a {base.q(llm_config)}")
            if mode_out.strip() != "600":
                raise RuntimeError("real LLM config must have mode 0600")
            _, hash_out, _ = base.run(worker, f"sha256sum {base.q(llm_config)}")
            llm_config_sha256 = hash_out.split()[0]
            model_script = "ark_real_llm_proxy.py"
            model_command = (
                f"python3 -B {worker_run}/{model_script} --port {base.MODEL_PORT} "
                f"--config {base.q(llm_config)}"
            )
        else:
            model_script = "model_simulator.py"
            model_command = (
                f"python3 -B {worker_run}/model_simulator.py --port {base.MODEL_PORT} "
                f"--latency-ms {simulator_latency_ms} --wrong-steps {code_wrong_steps} "
                f"--dataset-jsonl {base.q(worker_dataset)}"
            )
        model_pid = base.start_owned(
            worker,
            model_command,
            f"{worker_run}/model.log",
            "/usr/bin/python3.12",
            f"{worker_run}/{model_script}",
        )
        if model_mode == "real":
            _, preflight_out, _ = base.run(
                worker,
                f"python3 -B {worker_run}/ark_real_llm_preflight.py "
                f"--url http://127.0.0.1:{base.MODEL_PORT}/v1",
                timeout=240,
            )
            print(f"[gate3:{workers_count}x{capacity}] {preflight_out.strip()}", flush=True)
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
            "RUST_LOG": "error",
        }
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
                    for i in range(workers_count)
                ]
            }
            fleet_spec_path = f"{worker_run}/fleet.json"
            fleet_pid_path = f"{worker_run}/fleet-pids.json"
            fleet_metrics_path = f"{worker_run}/fleet-metrics.json"
            base.put_text(worker, fleet_spec_path, json.dumps(fleet_spec, sort_keys=True), 0o600)
            fleet_command = (
                f"python3 -B {worker_run}/worker_fleet_supervisor.py "
                f"--spec {fleet_spec_path} --pid-file {fleet_pid_path} "
                f"--metrics-file {fleet_metrics_path}"
            )
            fleet_supervisor_pid = base.start_owned(
                worker,
                fleet_command,
                f"{worker_run}/fleet-supervisor.log",
                "/usr/bin/python3.12",
                f"{worker_run}/worker_fleet_supervisor.py",
            )
            fleet_deadline = time.monotonic() + max(60, workers_count // 2)
            while time.monotonic() < fleet_deadline:
                status, _, _ = base.run(worker, f"test -s {base.q(fleet_pid_path)}", check=False)
                if status == 0:
                    fleet_pid_document = json.loads(base.get_text(worker, fleet_pid_path))
                    break
                time.sleep(0.5)
            else:
                raise RuntimeError("real Worker fleet supervisor did not publish its PID manifest")
            if fleet_pid_document.get("worker_count") != workers_count:
                raise RuntimeError(f"fleet PID manifest count mismatch: {fleet_pid_document}")
            worker_pids = [
                (int(item["pid"]), str(item["config"]))
                for item in fleet_pid_document["workers"]
            ]
        else:
            for i in range(workers_count):
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
                    worker,
                    cmd,
                    f"{worker_run}/logs/worker-{i:04d}.log",
                    f"{worker_run}/bundle/uenv-worker",
                    config,
                )
                worker_pids.append((pid, config))
        for port in sorted({ports[0], ports[-1]}):
            base.run(
                server,
                f"python3 -B {server_run}/tcp_probe.py {base.WORKER_PRIVATE_IP} {port}",
                timeout=15,
            )
        print(f"[gate3:{workers_count}x{capacity}] private Worker range endpoints reachable", flush=True)
        base.assert_protected_unchanged(server, before)
        # manifest 记录本轮实际启动的端口、PID 和受保护 server 快照。
        # 后续排查时先看 manifest，再看 result/log。
        manifest = {
            "run_id": run_id, "gate": 3, "acceptance_purpose": acceptance_purpose,
            "environment": "code", "real_workers": workers_count,
            "worker_capacity": capacity, "worker_slots": workers_count * capacity,
            "requested_episode_concurrency": workers_count * capacity, "modes": list(modes),
            "duration_per_mode_seconds": duration, "max_steps": max_steps,
            "code_wrong_steps": code_wrong_steps, "min_steps": min_steps, "worker_ports": ports,
            "model_mode": model_mode,
            "model_kind": (
                "real-ark-chat-completions+ark-tokenization"
                if model_mode == "real"
                else "deterministic-dataset-oracle-for-control-plane-scale"
            ),
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
            "owned_pids": {
                "server": server_pid,
                "model": model_pid,
                "fleet_supervisor": fleet_supervisor_pid,
                "workers": [p for p, _ in worker_pids],
            },
            "fleet_metrics_path": fleet_metrics_path,
        }
        base.put_text(server, f"{server_run}/manifest.json", json.dumps(manifest, indent=2, sort_keys=True))
        base.put_text(worker, f"{worker_run}/manifest.json", json.dumps(manifest, indent=2, sort_keys=True))
        (local_run / "manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True))
        # 同一组 worker 不重启，连续跑三种模式，减少启动成本。
        for mode in modes:
            remote_result = f"{server_run}/result-{mode}.json"
            command = " ".join([
                f"PYTHONPATH={server_run}:{server_run}/generated", "python3", "-B", f"{server_run}/load_client.py",
                "--server", f"{base.SERVER_PRIVATE_IP}:{base.SERVER_PORT}", "--workers", str(workers_count),
                "--slots", str(capacity), "--mode", mode, "--duration", str(duration),
                "--model-url", f"http://127.0.0.1:{base.MODEL_PORT}/v1", "--run-id", run_id, "--output", remote_result,
                "--max-steps", str(max_steps), "--code-wrong-steps", str(code_wrong_steps),
                "--min-steps", str(min_steps), "--model-mode", model_mode,
                "--dataset-jsonl", dataset_jsonl,
                "--dataset-limit", str(dataset_limit),
                "--dataset-offset", str(dataset_offset),
                "--registration-timeout", str(registration_timeout),
                "--batch-timeout", str(batch_timeout),
                "--exact-batches", str(exact_batches),
                "--model-name", ("proxy-selected-versioned-model" if model_mode == "real" else "gate3-code-model"),
            ])
            print(f"[gate3:{workers_count}x{capacity}] mode={mode} start", flush=True)
            command_timeout = registration_timeout + max(batch_timeout, duration + 240)
            _, out, err = base.run(server, command, timeout=command_timeout)
            if err: print(err, flush=True)
            result = json.loads(base.get_text(server, remote_result))
            step_stats = _model_step_stats(worker, f"gate3-{run_id}-{mode}-")
            expected_min_steps = min_steps if model_mode == "real" else code_wrong_steps + 1
            step_validation = {
                "expected_min_steps": expected_min_steps,
                "max_allowed_steps": max_steps,
                "completed_episodes": result["completed"],
                "stats": step_stats,
                "passed": (
                    step_stats["task_count"] == result["completed"]
                    and step_stats["min_steps"] >= expected_min_steps
                    and step_stats["max_steps"] <= max_steps
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
            if acceptance_purpose == "worker-scale":
                coverage = _completed_worker_coverage(
                    server,
                    f"{server_run}/server.log",
                    f"stress-{run_id}-worker-",
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
            print(f"[gate3:{workers_count}x{capacity}] mode={mode} PASS throughput={result['throughput_eps']:.2f} ep/s", flush=True)
        (local_run / "results.json").write_text(json.dumps(results, indent=2, sort_keys=True))
        if fleet_metrics_path:
            fleet_resource_metrics = json.loads(base.get_text(worker, fleet_metrics_path))
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
        # 重新连接可以提高 cleanup 成功率。
        had_worker, had_server = worker is not None, server is not None
        for stale in (worker, server):
            if stale: stale.close()
        worker = server = None
        if had_worker:
            try: worker = base.connect(base.WORKER_HOST, password)
            except Exception as exc: cleanup_errors.append(f"worker reconnect: {exc}")
        if had_server:
            try: server = base.connect(base.SERVER_HOST, password)
            except Exception as exc: cleanup_errors.append(f"server reconnect: {exc}")
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
                    try: base.stop_owned(worker, pid, f"{worker_run}/bundle/uenv-worker", config)
                    except Exception as exc: cleanup_errors.append(str(exc))
            try: base.stop_owned(worker, model_pid, "/usr/bin/python3.12", f"{worker_run}/{model_script}")
            except Exception as exc: cleanup_errors.append(str(exc))
            try:
                base.assert_ports_free(worker, ports + obs_ports + [base.MODEL_PORT], base.WORKER_HOST)
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
            print(f"[gate3:{workers_count}x{capacity}] cleanup ERRORS={cleanup_errors}", flush=True)
            outcome = {
                "run_id": run_id,
                "scale": f"{workers_count}x{capacity}",
                "status": "failed",
                "error": "cleanup failed: " + " | ".join(cleanup_errors),
            }
        else:
            print(f"[gate3:{workers_count}x{capacity}] cleanup complete", flush=True)
        for client in (worker, server):
            if client: client.close()
    assert outcome is not None
    return outcome


def main() -> int:
    """Gate3 command entry. Defaults to an isolated one-Worker smoke."""
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
        help="Gate3 defaults to the real Ark LLM; simulator must be explicitly selected.",
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
    parser.add_argument("--registration-timeout", type=int, default=180)
    parser.add_argument("--batch-timeout", type=int, default=180)
    parser.add_argument("--fleet-supervisor-threshold", type=int, default=16)
    parser.add_argument("--simulator-latency-ms", type=int, default=10)
    parser.add_argument(
        "--acceptance-purpose",
        choices=("gate3-real-llm", "worker-scale"),
        default="gate3-real-llm",
    )
    parser.add_argument("--artifacts", type=Path, default=Path.cwd() / "distributed-gate-artifacts")
    base.add_runtime_arguments(parser, require_code_plugin=True)
    args = parser.parse_args()
    base.configure_from_args(args)
    allowed_exposed_ports = {5432, 6379, 8000, 8077, 8088, 8099, 8777, 8888}
    for label, port in {
        "isolated server": base.SERVER_PORT,
        "single Worker": base.WORKER_PORT,
        "model endpoint": base.MODEL_PORT,
    }.items():
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
    if args.fleet_supervisor_threshold < 2 or args.simulator_latency_ms < 0:
        raise SystemExit("invalid fleet supervisor threshold or simulator latency")
    modes = tuple(args.mode or MODES)
    if args.acceptance_purpose == "gate3-real-llm" and args.model_mode != "real":
        raise SystemExit("Gate3 acceptance requires --model-mode real")
    if args.acceptance_purpose == "worker-scale":
        if args.model_mode != "simulator" or modes != ("sync",) or args.exact_batches <= 0:
            raise SystemExit("worker-scale requires simulator, exactly --mode sync, and --exact-batches > 0")
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
        args.simulator_latency_ms,
        args.acceptance_purpose,
    )
    summaries = [summary]
    summary_path = args.artifacts / f"gate3-summary-{time.strftime('%Y%m%d-%H%M%S')}.json"
    summary_path.write_text(json.dumps(summaries, indent=2, sort_keys=True))
    print(f"[gate3] summary={summary_path}", flush=True)
    return 0 if summary["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
