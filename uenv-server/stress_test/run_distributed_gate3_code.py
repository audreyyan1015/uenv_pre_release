#!/usr/bin/env python3
"""分布式 Gate3：真实 Code worker 扩容压测。

运行位置说明：
- 这个脚本从 8.130.75.157 的 stress_test 目录启动。
- 隔离 server 启动在 8.130.75.157:8099。
- 真实 Code worker 启动在 8.130.86.71。
- 已经在线的正式 adapter-core 只做保护检查，不复用、不停止。

Gate3 会跑两组规模：8 个 worker * 2 slot、32 个 worker * 4 slot。
每组都会依次跑 sync、one_step_off_policy、fully_async 三种模式。
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import tempfile
import time
import uuid

import distributed_stress_runtime as base


# 三种并行模式都要测，避免只验证同步路径。
MODES = ("sync", "one_step_off_policy", "fully_async")

# worker 和 observability 端口按顺序分配。启动前会逐个确认端口空闲。
WORKER_PORT_BASE = 20000
OBS_PORT_BASE = 21000
MODEL_PORT = 18001

# load_client.py 会被写入远端 /tmp/uenv-<run_id>/ 运行目录。
# 它需要导入 stress_test_common.py，所以这里提前读取文件内容，后面一并下发。
COMMON_SOURCE = Path(__file__).with_name("stress_test_common.py").read_text()


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

parser = argparse.ArgumentParser()
parser.add_argument("--port", type=int, required=True)
parser.add_argument("--latency-ms", type=int, default=1000)
parser.add_argument("--wrong-steps", type=int, default=2)
args = parser.parse_args()
attempts_by_task = {}
attempts_lock = threading.Lock()

class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        size = int(self.headers.get("content-length", "0"))
        raw = b""
        if size:
            raw = self.rfile.read(size)
        time.sleep(args.latency_ms / 1000)
        task_id = "unknown"
        try:
            request = json.loads(raw.decode() if raw else "{}")
            message = request.get("messages", [{}])[0].get("content", "")
            found = re.search(r"Task ID: ([A-Za-z0-9_.:-]+)", message)
            if found:
                task_id = found.group(1)
        except Exception:
            pass
        with attempts_lock:
            attempt = attempts_by_task.get(task_id, 0) + 1
            attempts_by_task[task_id] = attempt
        if attempt <= args.wrong_steps:
            content = "def add(a, b):\n    return a - b"
        else:
            content = "def add(a, b):\n    return a + b"
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
args = parser.parse_args()

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
    deadline = time.monotonic() + 180
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
    lock = asyncio.Lock()
    semaphore = asyncio.Semaphore(args.slots)
    tasks = set()

    def make_sample(batch_id, index):
        # 每个样本都是同一个小 Code 题，但 task_id 不同，便于日志排查。
        task_id = f"gate3-{args.run_id}-{args.mode}-{batch_id}-{index}"
        env_config = stress_common.code_env_payload(task_id)
        env_config["question"] = f"{env_config['question']}\nTask ID: {task_id}"
        return stress_common.make_sample_envelope(
            adapter_core_pb2,
            batch_id=batch_id,
            sample_index=index,
            env_type="code",
            parallel_mode=args.mode,
            env_config=env_config,
            reward_config=stress_common.code_reward_config(),
            sample_context={"stress_run_id": args.run_id, "gate": 3, "max_steps": args.max_steps},
            timeout_seconds=120,
            max_steps=args.max_steps,
            model_url=args.model_url,
            model_name="gate3-code-model",
        )

    async def send_batch():
        # 一次 ExecuteBatch 发送 args.workers 个样本。外层 semaphore 控制同时在飞的
        # batch 数，args.slots 表示每个 worker 的并发容量。
        nonlocal submitted, completed, failed, rpc_errors, protocol_errors
        batch_id = str(uuid.uuid4())
        samples = [make_sample(batch_id, index) for index in range(args.workers)]
        async with lock:
            submitted += len(samples)
        started = time.monotonic()
        try:
            response = await execute(
                adapter_core_pb2.ExecuteBatchRequest(
                    request_id=batch_id, batch_id=batch_id, samples=samples
                ), timeout=180,
            )
            elapsed = (time.monotonic() - started) * 1000
            async with lock:
                latencies.append(elapsed)
                for result in response.results:
                    rewards.append(result.reward)
                    if result.status in {"completed", "success"}:
                        completed += 1
                        if args.mode != "sync" and (
                            not result.rollout_policy_version or not result.rollout_log_probs
                        ):
                            protocol_errors += 1
                    else:
                        failed += 1
        except grpc.RpcError:
            async with lock:
                rpc_errors += len(samples)
        finally:
            semaphore.release()

    # 在 duration 时间窗口内持续补充 batch。时间到后等待已经发出的 batch 收尾。
    started = time.monotonic()
    while time.monotonic() - started < args.duration:
        await semaphore.acquire()
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
    with open(args.output, "w", encoding="utf-8") as target:
        json.dump(document, target, indent=2, sort_keys=True)
    print(json.dumps(document, indent=2, sort_keys=True), flush=True)
    await channel.close()
    if completed != submitted or failed or rpc_errors or protocol_errors:
        raise SystemExit(1)
    if document["average_reward"] < .999:
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


def run_scale(
    workers_count: int,
    capacity: int,
    duration: int,
    artifacts: Path,
    password: str,
    max_steps: int,
    code_wrong_steps: int,
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
    server_pid = model_pid = None
    worker_pids: list[tuple[int, str]] = []
    before = None
    cleanup_errors: list[str] = []
    results: list[dict] = []
    error: str | None = None
    outcome: dict | None = None
    try:
        # 连接两台机器后，第一件事是记录正式 server 快照。
        server = base.connect(base.SERVER_HOST, password)
        worker = base.connect(base.WORKER_HOST, password)
        before = base.protected_snapshot(server)
        base.assert_port_free(server, base.SERVER_PORT, base.SERVER_HOST)
        # Gate3 会启动多 worker，所以端口按连续区间分配。
        ports = [WORKER_PORT_BASE + i for i in range(workers_count)]
        obs_ports = [OBS_PORT_BASE + i for i in range(workers_count)]
        for port in ports + obs_ports + [MODEL_PORT]:
            base.assert_port_free(worker, port, base.WORKER_HOST)
        print(f"[gate3:{workers_count}x{capacity}] preflight ports={ports[0]}-{ports[-1]}", flush=True)

        # bundle 先在 server 机器上制作，再通过本地临时文件转传到 worker 机器。
        # 这样 worker 机器不需要直接访问 /home/uenv 源码目录。
        base.run(server, f"install -d -m 0755 {base.q(server_run)}/bundle/plugins/code/scripts {base.q(server_run)}/generated/uenv/v1")
        base.run(worker, f"install -d -m 0755 {base.q(worker_run)}/logs {base.q(worker_run)}/wal")
        base.run(server, " && ".join([
            f"install -m 0755 {base.q(base.SOURCE_WORKER_BIN)} {base.q(server_run)}/bundle/uenv-worker",
            f"install -m 0755 {base.q(base.SOURCE_CODE_BIN)} {base.q(server_run)}/bundle/uenv-code-plugin",
            f"strip {base.q(server_run)}/bundle/uenv-worker {base.q(server_run)}/bundle/uenv-code-plugin",
            f"cp -a /home/uenv/plugins/code/. {base.q(server_run)}/bundle/plugins/code/",
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
        for i, (port, obs_port) in enumerate(zip(ports, obs_ports)):
            worker_id = f"stress-{run_id}-worker-{i:04d}"
            base.put_text(worker, f"{worker_run}/worker-{i:04d}.yaml", worker_config(worker_run, worker_id, port, obs_port, capacity))
        # load_client.py 需要 Python 版 protobuf message，所以每次用当前 proto 生成。
        proto = " ".join([
            "/usr/bin/protoc", "-I", "/home/uenv/proto", f"--python_out={base.q(server_run)}/generated",
            "/home/uenv/proto/uenv/v1/common.proto", "/home/uenv/proto/uenv/v1/episode.proto",
            "/home/uenv/proto/uenv/v1/scheduler.proto", "/home/uenv/proto/uenv/v1/adapter_core.proto",
        ])
        base.run(server, proto)
        base.run(server, f"touch {base.q(server_run)}/generated/uenv/__init__.py {base.q(server_run)}/generated/uenv/v1/__init__.py")
        # 关闭 trajectory/obs，减少压测以外的写入和背景工作。
        server_cmd = " ".join([
            "env", "UENV_SERVER_CONFIG_STRICT=1", "UENV_TRAJECTORY_ENABLED=0", "UENV_OBS_ENABLED=0", "UENV_LOG_ANSI=0",
            f"UENV_ADDR={base.SERVER_PRIVATE_IP}:{base.SERVER_PORT}", f"UENV_CONFIG_PATH={server_run}/server.yaml", "RUST_LOG=warn", base.SERVER_BIN,
        ])
        server_pid = base.start_owned(server, server_cmd, f"{server_run}/server.log", base.SERVER_BIN, base.SERVER_BIN)
        model_pid = base.start_owned(
            worker,
            f"python3 -B {worker_run}/model_simulator.py --port {MODEL_PORT} "
            f"--latency-ms 1000 --wrong-steps {code_wrong_steps}",
            f"{worker_run}/model.log",
            "/usr/bin/python3.12",
            f"{worker_run}/model_simulator.py",
        )
        # 每个 worker 都有独立 yaml、独立 WAL 和独立日志，方便定位某个 worker 的问题。
        for i, port in enumerate(ports):
            config = f"{worker_run}/worker-{i:04d}.yaml"
            cmd = " ".join([
                "env", "UENV_SERVER_CONFIG_STRICT=1", "UENV_TRAJECTORY_ENABLED=0", "UENV_OBS_ENABLED=0", "UENV_LOG_ANSI=0",
                "UENV_WORKER_EPISODE_TIMEOUT_SECS=180", "UENV_LLM_HTTP_TIMEOUT_SECS=180",
                f"UENV_CODE_PLUGIN_BIN={worker_run}/bundle/uenv-code-plugin",
                f"UENV_CODE_EVAL_SCRIPT={worker_run}/bundle/plugins/code/scripts/evaluate_code.py",
                "RUST_LOG=error", f"{worker_run}/bundle/uenv-worker", "--config", config, "serve",
            ])
            pid = base.start_owned(worker, cmd, f"{worker_run}/logs/worker-{i:04d}.log", f"{worker_run}/bundle/uenv-worker", config)
            worker_pids.append((pid, config))
            if i == 0:
                base.run(server, f"python3 -B {server_run}/tcp_probe.py {base.WORKER_PRIVATE_IP} {port}", timeout=15)
                print(f"[gate3:{workers_count}x{capacity}] private worker TCP reachable", flush=True)
        base.assert_protected_unchanged(server, before)
        # manifest 记录本轮实际启动的端口、PID 和受保护 server 快照。
        # 后续排查时先看 manifest，再看 result/log。
        manifest = {
            "run_id": run_id, "gate": 3, "environment": "code", "real_workers": workers_count,
            "worker_capacity": capacity, "worker_slots": workers_count * capacity,
            "requested_episode_concurrency": workers_count * capacity, "modes": list(MODES),
            "duration_per_mode_seconds": duration, "max_steps": max_steps,
            "code_wrong_steps": code_wrong_steps, "worker_ports": ports,
            "model_kind": "deterministic-openai-compatible; workers/plugins/evaluator are real",
            "protected_server": before, "owned_pids": {"server": server_pid, "model": model_pid, "workers": [p for p, _ in worker_pids]},
        }
        base.put_text(server, f"{server_run}/manifest.json", json.dumps(manifest, indent=2, sort_keys=True))
        base.put_text(worker, f"{worker_run}/manifest.json", json.dumps(manifest, indent=2, sort_keys=True))
        (local_run / "manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True))
        # 同一组 worker 不重启，连续跑三种模式，减少启动成本。
        for mode in MODES:
            remote_result = f"{server_run}/result-{mode}.json"
            command = " ".join([
                f"PYTHONPATH={server_run}:{server_run}/generated", "python3", "-B", f"{server_run}/load_client.py",
                "--server", f"{base.SERVER_PRIVATE_IP}:{base.SERVER_PORT}", "--workers", str(workers_count),
                "--slots", str(capacity), "--mode", mode, "--duration", str(duration),
                "--model-url", f"http://127.0.0.1:{MODEL_PORT}/v1", "--run-id", run_id, "--output", remote_result,
                "--max-steps", str(max_steps), "--code-wrong-steps", str(code_wrong_steps),
            ])
            print(f"[gate3:{workers_count}x{capacity}] mode={mode} start", flush=True)
            _, out, err = base.run(server, command, timeout=duration + 240)
            if err: print(err, flush=True)
            result = json.loads(base.get_text(server, remote_result))
            results.append(result)
            (local_run / f"result-{mode}.json").write_text(json.dumps(result, indent=2, sort_keys=True))
            print(f"[gate3:{workers_count}x{capacity}] mode={mode} PASS throughput={result['throughput_eps']:.2f} ep/s", flush=True)
        (local_run / "results.json").write_text(json.dumps(results, indent=2, sort_keys=True))
        outcome = {"run_id": run_id, "scale": f"{workers_count}x{capacity}", "results": results, "status": "passed"}
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
            for pid, config in reversed(worker_pids):
                try: base.stop_owned(worker, pid, f"{worker_run}/bundle/uenv-worker", config)
                except Exception as exc: cleanup_errors.append(str(exc))
            try: base.stop_owned(worker, model_pid, "/usr/bin/python3.12", f"{worker_run}/model_simulator.py")
            except Exception as exc: cleanup_errors.append(str(exc))
            for port in [WORKER_PORT_BASE + i for i in range(workers_count)] + [OBS_PORT_BASE + i for i in range(workers_count)] + [MODEL_PORT]:
                try: base.assert_port_free(worker, port, base.WORKER_HOST)
                except Exception as exc: cleanup_errors.append(str(exc))
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
    """Gate3 命令行入口。默认每种模式跑 30 秒。"""
    parser = argparse.ArgumentParser()
    parser.add_argument("--duration", type=int, default=30)
    parser.add_argument("--max-steps", type=int, default=10)
    parser.add_argument(
        "--code-wrong-steps", type=int, default=2,
        help="Code model simulator returns wrong code this many times per task before returning the correct solution.",
    )
    parser.add_argument("--artifacts", type=Path, default=Path.cwd() / "distributed-gate-artifacts")
    args = parser.parse_args()
    if args.max_steps <= 0:
        raise SystemExit("--max-steps must be positive")
    if args.code_wrong_steps < 0 or args.code_wrong_steps >= args.max_steps:
        raise SystemExit("--code-wrong-steps must be >=0 and smaller than --max-steps")
    password = os.environ.get("UENV_PASS")
    if not password: raise SystemExit("UENV_PASS is required")
    args.artifacts.mkdir(parents=True, exist_ok=True)
    summaries = []
    for workers, capacity in ((8, 2), (32, 4)):
        summary = run_scale(
            workers, capacity, args.duration, args.artifacts, password,
            args.max_steps, args.code_wrong_steps,
        )
        summaries.append(summary)
        if summary["status"] != "passed":
            break
    summary_path = args.artifacts / f"gate3-summary-{time.strftime('%Y%m%d-%H%M%S')}.json"
    summary_path.write_text(json.dumps(summaries, indent=2, sort_keys=True))
    print(f"[gate3] summary={summary_path}", flush=True)
    return 0 if len(summaries) == 2 and all(item["status"] == "passed" for item in summaries) else 1


if __name__ == "__main__":
    raise SystemExit(main())
