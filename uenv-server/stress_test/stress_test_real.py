#!/usr/bin/env python3
"""
uenv 压力测试（真实 worker + mock LLM 版本）

架构：
  mock LLM server（本机 asyncio HTTP）
      ↑ HTTP /v1/chat/completions
  50 × uenv-worker (Rust, debug)
      ↑ gRPC DispatchEpisode / ReportResult
  uenv-adapter-core (Rust, release)
      ↑ gRPC ExecuteBatch
  load_generator (本脚本 asyncio)

Mock LLM 特性：
  - 延迟：LognNormal(μ, σ)，可配置 mean / p95 目标
  - 正确率：LLM_CORRECT_RATE（正确答案 "#### 42"，错误随机整数）
  - 完全 asyncio，可轻松支撑 400+ 并发连接

用法：
  python3 /home/uenv/uenv-server/stress_test/stress_test_real.py
"""

import asyncio
import random
import subprocess
import sys
import os
import time
import uuid
import json
import threading
from dataclasses import dataclass, field
from typing import List

# ── 全局配置 ─────────────────────────────────────────────────────────────────
N_WORKERS            = 50
WORKER_BASE_PORT     = 51000
SERVER_ADDR          = "localhost:50051"
WORKER_CAPACITY      = 8        # 每 worker 最大并发 episode 数
BATCH_SIZE           = 64       # 每次 ExecuteBatch 的 episode 数
N_CONCURRENT_BATCHES = 32       # 并发 batch 数（semaphore 限流）
TEST_DURATION        = 1800     # 测试时长（秒）——延迟最长 10min，至少跑 30min
ENV_TYPE             = "math"

# ── Mock LLM 参数 ─────────────────────────────────────────────────────────────
LLM_PORT         = 18080
LLM_CORRECT_RATE = 0.70   # 70% 返回正确答案
LLM_QUESTION     = "What is 6×7?"   # 传给 worker 的问题
LLM_TARGET       = "42"             # 正确答案（math plugin 比对用）
# 延迟分布：截断正态分布 Normal(mean, std)，区间 [10s, 600s]
LLM_LATENCY_MEAN_MS = 300_000   # 均值 300s（5min）
LLM_LATENCY_STD_MS  = 100_000   # 标准差 100s（覆盖 10s~600s 约 ±3σ）
LLM_LATENCY_MIN_MS  = 10_000    # 最小 10s
LLM_LATENCY_MAX_MS  = 600_000   # 最大 600s（10min）

# ── 路径 ─────────────────────────────────────────────────────────────────────
STRESS_DIR      = "/home/uenv/uenv-server/stress_test"
LOG_DIR         = "/home/uenv/uenv-server/stress_test/logs"
CONFIG_DIR      = "/tmp/uenv-stress/configs"
WAL_BASE        = "/tmp/uenv-stress/wal"
SERVER_YAML     = "/home/uenv/config/server.yaml"
ADAPTER_BIN     = "/home/uenv/target/release/uenv-adapter-core"
WORKER_BIN      = "/home/uenv/target/debug/uenv-worker"
MATH_PLUGIN_BIN = "/home/uenv/target/debug/uenv-math-plugin"
PLUGIN_DIR      = "/home/uenv/plugins"


# ── 指标 ─────────────────────────────────────────────────────────────────────
@dataclass
class Metrics:
    latencies: List[float] = field(default_factory=list)
    rewards:   List[float] = field(default_factory=list)
    completed: int = 0
    failed:    int = 0
    errors:    int = 0
    lock: threading.Lock = field(default_factory=threading.Lock)

    def record(self, latency_ms: float, status: str, reward: float = 0.0):
        with self.lock:
            self.latencies.append(latency_ms)
            self.rewards.append(reward)
            if status in ("completed", "success"):
                self.completed += 1
            else:
                self.failed += 1

    def record_error(self):
        with self.lock:
            self.errors += 1

    def snapshot(self):
        with self.lock:
            return (self.completed, self.failed, self.errors,
                    list(self.latencies), list(self.rewards))

    def summary(self, elapsed: float) -> str:
        completed, failed, errors, latencies, rewards = self.snapshot()
        total = completed + failed + errors
        lat = sorted(latencies)
        if not lat:
            return "no data"
        p50 = lat[int(len(lat) * 0.50)]
        p95 = lat[int(len(lat) * 0.95)]
        p99 = lat[int(len(lat) * 0.99)]
        tput = total / elapsed if elapsed > 0 else 0
        avg_rw = sum(rewards) / len(rewards) if rewards else 0.0
        correct = sum(1 for r in rewards if r >= 0.99)
        correct_pct = 100 * correct / len(rewards) if rewards else 0.0
        return (
            f"\n{'='*60}\n"
            f"  测试时长:      {elapsed:.1f}s\n"
            f"  总 episode:    {total}\n"
            f"  完成:          {completed} ({100*completed/max(total,1):.1f}%)\n"
            f"  失败:          {failed}\n"
            f"  gRPC 错误:     {errors}\n"
            f"  吞吐量:        {tput:.1f} ep/s\n"
            f"  延迟 p50:      {p50:.0f}ms\n"
            f"  延迟 p95:      {p95:.0f}ms\n"
            f"  延迟 p99:      {p99:.0f}ms\n"
            f"  平均 reward:   {avg_rw:.3f}\n"
            f"  正确率:        {correct_pct:.1f}% (预期 {LLM_CORRECT_RATE*100:.0f}%)\n"
            f"  LLM 延迟分布:  Normal(mean={LLM_LATENCY_MEAN_MS//1000}s, std={LLM_LATENCY_STD_MS//1000}s) 截断 [{LLM_LATENCY_MIN_MS//1000}s,{LLM_LATENCY_MAX_MS//1000}s]\n"
            f"  并发上限:      {N_WORKERS * WORKER_CAPACITY} ep\n"
            f"{'='*60}"
        )


METRICS = Metrics()

def json_bytes(value: dict) -> bytes:
    return json.dumps(value, separators=(",", ":")).encode()


# ── Mock LLM HTTP Server ──────────────────────────────────────────────────────
class MockLLMServer:
    """
    模拟 OpenAI /v1/chat/completions 的 asyncio HTTP 服务器。

    延迟：LogNormal(mu_ln, sigma_ln) — 和真实 vLLM 分布形状相近（有重尾）。
    正确率：以 correct_rate 概率返回 "#### {target}"，否则返回随机错误整数。
    """

    def __init__(self, port: int, correct_rate: float,
                 latency_mean_ms: float, latency_std_ms: float,
                 latency_min_ms: float, latency_max_ms: float,
                 target: str):
        self.port = port
        self.correct_rate = correct_rate
        self.lat_mean = latency_mean_ms
        self.lat_std  = latency_std_ms
        self.lat_min  = latency_min_ms
        self.lat_max  = latency_max_ms
        self.target = target
        self._server = None
        self._req_count = 0

    def _sample_latency_ms(self) -> float:
        """从截断正态分布采样延迟（ms），区间 [lat_min, lat_max]。"""
        sample = random.normalvariate(self.lat_mean, self.lat_std)
        return max(self.lat_min, min(self.lat_max, sample))

    def _make_response(self) -> bytes:
        if random.random() < self.correct_rate:
            content = f"#### {self.target}"          # 正确答案
        else:
            wrong = random.randint(0, 999)
            while str(wrong) == self.target:
                wrong = random.randint(0, 999)
            content = f"#### {wrong}"                # 错误答案
        body = json.dumps({
            "choices": [{
                "message": {"role": "assistant", "content": content},
                "finish_reason": "stop",
            }],
            "usage": {"prompt_tokens": 8, "completion_tokens": 4},
        }).encode()
        return (
            b"HTTP/1.1 200 OK\r\n"
            b"Content-Type: application/json\r\n"
            b"Connection: close\r\n"
            + f"Content-Length: {len(body)}\r\n".encode()
            + b"\r\n"
            + body
        )

    async def _handle(self, reader: asyncio.StreamReader,
                      writer: asyncio.StreamWriter):
        try:
            # 读 HTTP 请求头
            buf = b""
            while b"\r\n\r\n" not in buf:
                chunk = await asyncio.wait_for(reader.read(8192), timeout=10.0)
                if not chunk:
                    return
                buf += chunk

            # 读剩余 body（按 Content-Length）
            cl = 0
            for line in buf.split(b"\r\n"):
                if line.lower().startswith(b"content-length:"):
                    cl = int(line.split(b":", 1)[1].strip())
            body_start = buf.index(b"\r\n\r\n") + 4
            body = buf[body_start:]
            while len(body) < cl:
                chunk = await asyncio.wait_for(reader.read(8192), timeout=10.0)
                if not chunk:
                    break
                body += chunk

            # 模拟 LLM 处理延迟
            latency = self._sample_latency_ms()
            await asyncio.sleep(latency / 1000.0)

            # 返回响应
            self._req_count += 1
            writer.write(self._make_response())
            await writer.drain()
        except (asyncio.TimeoutError, ConnectionResetError, BrokenPipeError):
            pass
        except Exception:
            pass
        finally:
            try:
                writer.close()
            except Exception:
                pass

    async def start(self):
        self._server = await asyncio.start_server(
            self._handle, "127.0.0.1", self.port,
            limit=2 ** 20,  # 1MB read buffer
        )
        print(f"[*] Mock LLM 启动 → http://127.0.0.1:{self.port}/v1/chat/completions")
        print(f"    正确率={self.correct_rate*100:.0f}%  "
              f"延迟 Normal(mean={self.lat_mean/1000:.0f}s, std={self.lat_std/1000:.0f}s) "
              f"截断至 [{self.lat_min/1000:.0f}s, {self.lat_max/1000:.0f}s]")

    async def stop(self):
        if self._server:
            self._server.close()
            await self._server.wait_closed()


# ── Worker 配置生成 ───────────────────────────────────────────────────────────
def make_worker_config_yaml(worker_id: str, port: int, index: int) -> str:
    return f"""server:
  endpoint: "127.0.0.1:50051"

worker:
  id: "{worker_id}"
  listen: "0.0.0.0:{port}"
  advertise_endpoint: "127.0.0.1:{port}"
  max_concurrent: {WORKER_CAPACITY}

scheduler:
  mode: "remote"

env:
  types: ["{ENV_TYPE}"]
  backend: "process"
  plugin_dir: "{PLUGIN_DIR}"

pool:
  warmup_size: 2
  prewarm_on_startup: true
  max_idle_time: 600
  cool_timeout: 60
  max_episode_count: 100000

logging:
  level: "error"
  file: "/dev/null"

wal:
  dir: "{WAL_BASE}/{worker_id}"

observability:
  metrics_listen: "127.0.0.1:{19090 + index}"
  health_listen: "127.0.0.1:{19090 + index}"

hub:
  enabled: false
"""


def write_worker_configs() -> list:
    os.makedirs(CONFIG_DIR, exist_ok=True)
    configs = []
    for i in range(N_WORKERS):
        wid = f"stress-worker-{i:02d}"
        port = WORKER_BASE_PORT + i
        path = f"{CONFIG_DIR}/{wid}.yaml"
        with open(path, "w") as f:
            f.write(make_worker_config_yaml(wid, port, i))
        configs.append((wid, path))
    return configs


def start_worker_proc(wid: str, config_path: str):
    env = os.environ.copy()
    env["UENV_MATH_PLUGIN_BIN"] = MATH_PLUGIN_BIN
    env["RUST_LOG"] = "error"
    env["UENV_WORKER_EPISODE_TIMEOUT_SECS"] = "700"   # 大于最大 LLM 延迟 600s
    env["UENV_LLM_HTTP_TIMEOUT_SECS"] = "700"          # reqwest 超时同步
    lf = open(f"{LOG_DIR}/{wid}.log", "w")
    proc = subprocess.Popen(
        [WORKER_BIN, "--config", config_path, "serve"],
        env=env, stdout=lf, stderr=lf,
    )
    return proc, lf


# ── 等待 Worker 注册 ──────────────────────────────────────────────────────────
async def wait_workers_ready(n: int, timeout: float = 120.0) -> int:
    from uenv.v1 import scheduler_pb2, scheduler_pb2_grpc
    from grpc import aio as grpc_aio

    ch = grpc_aio.insecure_channel(SERVER_ADDR)
    stub = scheduler_pb2_grpc.ControlPlaneServiceStub(ch)
    deadline = time.time() + timeout
    registered = 0
    while time.time() < deadline:
        try:
            resp = await stub.ListWorkers(scheduler_pb2.ListWorkersRequest())
            registered = len(resp.workers)
            print(f"\r[startup] 已注册 {registered}/{n} 个 worker", end="", flush=True)
            if registered >= n:
                break
        except Exception:
            pass
        await asyncio.sleep(1.0)
    print()
    await ch.close()
    return registered


# ── 负载生成器 ────────────────────────────────────────────────────────────────
async def load_generator(stop_event: asyncio.Event):
    """
    固定 N_CONCURRENT_BATCHES 个并发 ExecuteBatch。
    Semaphore 先 acquire 再 create_task → in-flight task 数严格有界，不爆炸。
    """
    import grpc
    from grpc import aio as grpc_aio
    from uenv.v1 import adapter_core_pb2, adapter_core_pb2_grpc

    ch = grpc_aio.insecure_channel(SERVER_ADDR, options=[
        ("grpc.max_receive_message_length", 64 * 1024 * 1024),
        ("grpc.max_send_message_length",    64 * 1024 * 1024),
    ])
    stub = adapter_core_pb2_grpc.AdapterCoreServiceStub(ch)
    sem = asyncio.Semaphore(N_CONCURRENT_BATCHES)

    async def send_batch():
        batch_id = str(uuid.uuid4())
        samples = [
            adapter_core_pb2.SampleEnvelope(
                request_id=str(uuid.uuid4()),
                batch_id=batch_id,
                sample_index=i,
                env_type=ENV_TYPE,
                framework="verl",
                # typed SampleEnvelope 字段（adapter-core 解析）：
                #   env_config_json.question → worker payload.question（非空 → 触发真实 LLM 调用）
                #   model_endpoint.url       → EpisodeRequest.model_endpoint（指向 mock LLM）
                #   reward_config_json       → 透传给 worker，math plugin 用 target 比对答案
                env_config_json=json_bytes({
                    "question": LLM_QUESTION,
                    "dataset": "gsm8k",  # 使用 gsm8k answers_match，兼容 "#### 42" 格式
                }),
                episode_config_json=json_bytes({"max_steps": 1}),
                reward_config_json=json_bytes({
                    "type": "rule_reward",
                    "target": LLM_TARGET,
                }),
                model_endpoint=adapter_core_pb2.ModelEndpoint(
                    endpoint_type="http",
                    url=f"http://127.0.0.1:{LLM_PORT}/v1",
                    model_name="mock-llm",
                ),
                sample_context_json=json_bytes({}),
                timeout_seconds=700,  # 需大于最大 LLM 延迟 600s
            )
            for i in range(BATCH_SIZE)
        ]
        t0 = time.time()
        try:
            resp = await stub.ExecuteBatch(adapter_core_pb2.ExecuteBatchRequest(
                request_id=batch_id,
                batch_id=batch_id,
                samples=samples,
            ))
            elapsed_ms = (time.time() - t0) * 1000
            for r in resp.results:
                METRICS.record(elapsed_ms / BATCH_SIZE, r.status, r.reward)
        except grpc.RpcError as e:
            METRICS.record_error()
            if not stop_event.is_set():
                print(f"\n[load_gen] batch error: {e.code()} {e.details()[:100]}")

    tasks: set = set()

    async def run_and_release():
        try:
            await send_batch()
        finally:
            sem.release()

    while not stop_event.is_set():
        await sem.acquire()
        if stop_event.is_set():
            sem.release()
            break
        task = asyncio.create_task(run_and_release())
        tasks.add(task)
        task.add_done_callback(tasks.discard)

    if tasks:
        await asyncio.gather(*tasks, return_exceptions=True)
    await ch.close()


# ── 进度打印 ──────────────────────────────────────────────────────────────────
async def progress_printer(stop_event: asyncio.Event, start_time: float):
    while not stop_event.is_set():
        await asyncio.sleep(10)
        elapsed = time.time() - start_time
        completed, failed, errors, _, rewards = METRICS.snapshot()
        total = completed + failed + errors
        tput = total / elapsed if elapsed > 0 else 0
        avg_rw = sum(rewards) / len(rewards) if rewards else 0.0
        print(f"[{elapsed:.0f}s] {total} ep | {tput:.1f} ep/s | "
              f"完成={completed} 失败={failed} 错误={errors} | "
              f"avg_reward={avg_rw:.3f}")


# ── 主流程 ────────────────────────────────────────────────────────────────────
async def main():
    os.makedirs(LOG_DIR, exist_ok=True)
    os.makedirs(WAL_BASE, exist_ok=True)
    sys.path.insert(0, STRESS_DIR)

    # 检查二进制
    for path, name in [(ADAPTER_BIN, "uenv-adapter-core"),
                       (WORKER_BIN, "uenv-worker"),
                       (MATH_PLUGIN_BIN, "uenv-math-plugin")]:
        if not os.path.isfile(path):
            print(f"[ERROR] {name} 未找到: {path}")
            sys.exit(1)

    # 启动 mock LLM server
    llm = MockLLMServer(LLM_PORT, LLM_CORRECT_RATE,
                        LLM_LATENCY_MEAN_MS, LLM_LATENCY_STD_MS,
                        LLM_LATENCY_MIN_MS, LLM_LATENCY_MAX_MS, LLM_TARGET)
    await llm.start()

    # 生成 worker 配置
    configs = write_worker_configs()
    print(f"[*] 已生成 {N_WORKERS} 个 worker 配置 → {CONFIG_DIR}/")

    # 启动 adapter-core
    print(f"\n[*] 启动 adapter-core ...")
    adapter_env = os.environ.copy()
    adapter_env.update({
        "UENV_ADDR": f"[::]:{SERVER_ADDR.split(':')[1]}",
        "UENV_CONFIG_PATH": SERVER_YAML,
        "RUST_LOG": "warn",
    })
    adapter_log = open(f"{LOG_DIR}/adapter_core.log", "w")
    adapter_proc = subprocess.Popen(
        [ADAPTER_BIN], env=adapter_env,
        stdout=adapter_log, stderr=adapter_log,
    )
    print(f"    PID={adapter_proc.pid}, 等待 2s 启动...")
    await asyncio.sleep(2.0)

    # 启动 50 个 uenv-worker
    print(f"\n[*] 启动 {N_WORKERS} 个 uenv-worker (端口 {WORKER_BASE_PORT}~{WORKER_BASE_PORT+N_WORKERS-1})...")
    worker_procs, worker_logs = [], []
    for wid, cfg in configs:
        proc, lf = start_worker_proc(wid, cfg)
        worker_procs.append(proc)
        worker_logs.append(lf)

    # 等注册 + 插件预热
    print(f"\n[*] 等待 worker 注册 + 插件预热 (prewarm=true)...")
    registered = await wait_workers_ready(N_WORKERS, timeout=120.0)
    if registered == 0:
        print("[ERROR] 0 个 worker 注册，检查日志")
        adapter_proc.terminate()
        sys.exit(1)
    print(f"[*] {registered}/{N_WORKERS} 个 worker 已注册，等待 20s 让插件充分预热...")
    await asyncio.sleep(20.0)

    # 开始压测
    print(f"\n[*] 开始压测")
    print(f"    workers:          {registered} × {WORKER_CAPACITY} = {registered*WORKER_CAPACITY} ep 并发")
    print(f"    batch_size:       {BATCH_SIZE}   并发 batch: {N_CONCURRENT_BATCHES}")
    print(f"    时长:             {TEST_DURATION}s")
    print(f"    Mock LLM:         Normal(mean={LLM_LATENCY_MEAN_MS//1000}s, "
          f"std={LLM_LATENCY_STD_MS//1000}s) [{LLM_LATENCY_MIN_MS//1000}s,{LLM_LATENCY_MAX_MS//1000}s]  "
          f"correct_rate={LLM_CORRECT_RATE*100:.0f}%\n")

    stop_event = asyncio.Event()
    start_time = time.time()
    load_task     = asyncio.create_task(load_generator(stop_event))
    progress_task = asyncio.create_task(progress_printer(stop_event, start_time))

    await asyncio.sleep(TEST_DURATION)
    print("\n[*] 测试时间到，停止...")
    stop_event.set()

    await load_task
    progress_task.cancel()

    elapsed = time.time() - start_time
    print(METRICS.summary(elapsed))

    # 清理
    print(f"\n[*] 停止 {N_WORKERS} 个 worker ...")
    for p in worker_procs:
        p.terminate()
    for p in worker_procs:
        try:
            p.wait(timeout=3)
        except subprocess.TimeoutExpired:
            p.kill()
    for lf in worker_logs:
        lf.close()

    adapter_proc.terminate()
    adapter_proc.wait()
    adapter_log.close()
    await llm.stop()

    print(f"[*] 日志: {LOG_DIR}/")


if __name__ == "__main__":
    asyncio.run(main())
