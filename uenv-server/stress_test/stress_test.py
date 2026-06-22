#!/usr/bin/env python3
"""
uenv 压力测试：50 个 mock worker + 1 个 adapter-core + 并发批量请求
模拟千卡 VeRL 场景下 episode 调度吞吐量和延迟

用法：python3 /tmp/stress_test/stress_test.py
"""

import asyncio
import subprocess
import sys
import os
import time
import uuid
import json
import statistics
import signal
import threading
import traceback
from dataclasses import dataclass, field
from typing import List

# ── 配置 ─────────────────────────────────────────────────────────────────────
N_WORKERS          = 50        # mock worker 数量
WORKER_BASE_PORT   = 51000     # worker 端口从此开始，worker-i 用 51000+i
SERVER_ADDR        = "localhost:50051"
WORKER_CAPACITY    = 16        # 每个 worker 最大并发 episode 数
EPISODE_DURATION_MS = 80       # 模拟 episode 执行时间（毫秒）
HEARTBEAT_INTERVAL = 5.0       # 心跳间隔（秒）
BATCH_SIZE         = 64        # 每次 ExecuteBatch 的 episode 数（模拟 VeRL rollout batch）
N_CONCURRENT_BATCHES = 100       # 并发 batch 请求数（模拟多个 rollout worker）
TEST_DURATION      = 60        # 测试时长（秒）
ENV_TYPE           = "mock"    # episode 的 env_type

# ── 路径 ─────────────────────────────────────────────────────────────────────
STRESS_DIR   = "/home/uenv/uenv-server/stress_test"
ADAPTER_BIN  = "/home/uenv/target/release/uenv-adapter-core"
SERVER_YAML  = "/home/uenv/config/server.yaml"
LOG_DIR      = "/home/uenv/uenv-server/stress_test/logs"

os.makedirs(LOG_DIR, exist_ok=True)
sys.path.insert(0, STRESS_DIR)

import grpc
from grpc import aio as grpc_aio

from uenv.v1 import scheduler_pb2, scheduler_pb2_grpc
from uenv.v1 import episode_pb2
from uenv.v1 import adapter_core_pb2, adapter_core_pb2_grpc
from uenv.worker.v1 import worker_service_pb2, worker_service_pb2_grpc

# ── 指标收集 ──────────────────────────────────────────────────────────────────
@dataclass
class Metrics:
    latencies: List[float] = field(default_factory=list)
    completed: int = 0
    failed: int = 0
    errors: int = 0
    lock: threading.Lock = field(default_factory=threading.Lock)

    def record(self, latency_ms: float, status: str):
        with self.lock:
            self.latencies.append(latency_ms)
            if status == "completed":
                self.completed += 1
            else:
                self.failed += 1

    def record_error(self):
        with self.lock:
            self.errors += 1

    def summary(self, elapsed: float) -> str:
        with self.lock:
            total = self.completed + self.failed + self.errors
            lat = sorted(self.latencies)
            if not lat:
                return "no data"
            p50  = lat[int(len(lat) * 0.50)]
            p95  = lat[int(len(lat) * 0.95)]
            p99  = lat[int(len(lat) * 0.99)]
            tput = total / elapsed if elapsed > 0 else 0
            return (
                f"\n{'='*60}\n"
                f"  测试时长:      {elapsed:.1f}s\n"
                f"  总 episode:    {total}\n"
                f"  完成:          {self.completed} ({100*self.completed/max(total,1):.1f}%)\n"
                f"  失败:          {self.failed}\n"
                f"  gRPC 错误:     {self.errors}\n"
                f"  吞吐量:        {tput:.1f} ep/s\n"
                f"  延迟 p50:      {p50:.0f}ms\n"
                f"  延迟 p95:      {p95:.0f}ms\n"
                f"  延迟 p99:      {p99:.0f}ms\n"
                f"  理论最大并发:  {N_WORKERS * WORKER_CAPACITY} ep\n"
                f"  理论最大吞吐:  {N_WORKERS * WORKER_CAPACITY * 1000 / EPISODE_DURATION_MS:.0f} ep/s\n"
                f"{'='*60}"
            )

METRICS = Metrics()

# ── Mock Worker gRPC 服务端（接收 DispatchEpisode）────────────────────────────
class MockWorkerServicer(worker_service_pb2_grpc.WorkerGrpcServiceServicer):
    def __init__(self, worker_id: str, cp_stub, server_epoch_ref: list):
        self.worker_id = worker_id
        self.cp_stub = cp_stub
        self.server_epoch = server_epoch_ref
        self._load = 0
        self._lock = asyncio.Lock()

    async def DispatchEpisode(self, request, context):
        ep = request.episode
        episode_id = ep.episode_id
        attempt_id = ep.attempt_id

        async with self._lock:
            self._load += 1

        try:
            # 模拟 episode 执行
            await asyncio.sleep(EPISODE_DURATION_MS / 1000.0)

            # 构造假的完成结果
            result = episode_pb2.EpisodeResult(
                episode_id=episode_id,
                attempt_id=attempt_id,
                status="completed",
                trajectory=episode_pb2.Trajectory(
                    steps=[episode_pb2.StepRecord(
                        step_index=1,
                        action=b"mock_action",
                        reward=1.0,
                        terminated=True,
                    )],
                    total_reward=1.0,
                    total_steps=1,
                ),
                summary=episode_pb2.EpisodeResult.Summary(
                    total_reward=1.0,
                    total_steps=1,
                    total_duration_ms=EPISODE_DURATION_MS,
                    terminate_reason="terminated",
                ),
                integrity_verified=True,
            )

            # 调用 report_result 通知 server
            idem_key = f"{episode_id}:{attempt_id}:{self.worker_id}"
            await self.cp_stub.ReportResult(scheduler_pb2.ReportResultRequest(
                idempotency_key=idem_key,
                worker_id=self.worker_id,
                server_epoch=self.server_epoch[0],
                result=result,
            ))

            # 发一条进度报告后关流
            yield episode_pb2.StreamReport(
                episode_id=episode_id,
                attempt_id=attempt_id,
                current_step=1,
                total_steps=1,
                phase="episode_complete",
                worker_id=self.worker_id,
            )
        except Exception as e:
            print(f"[{self.worker_id}] DispatchEpisode error: {e}")
        finally:
            async with self._lock:
                self._load = max(0, self._load - 1)

    async def HealthCheck(self, request, context):
        return worker_service_pb2.HealthCheckResponse(ok=True, status="mock_worker")

    def current_load(self):
        return self._load

# ── 启动单个 Mock Worker ───────────────────────────────────────────────────────
async def run_worker(worker_id: str, port: int, stop_event: asyncio.Event):
    listen_addr = f"0.0.0.0:{port}"
    advertise_ep = f"127.0.0.1:{port}"

    # 连接到 server 的控制平面
    cp_channel = grpc_aio.insecure_channel(SERVER_ADDR)
    cp_stub = scheduler_pb2_grpc.ControlPlaneServiceStub(cp_channel)

    # 注册 worker
    server_epoch = [0]
    try:
        resp = await cp_stub.RegisterWorker(scheduler_pb2.RegisterWorkerRequest(
            worker_id=worker_id,
            supported_env_types=[ENV_TYPE],
            endpoint=advertise_ep,
            max_concurrent=WORKER_CAPACITY,
        ))
        server_epoch[0] = resp.server_epoch
    except Exception as e:
        print(f"[{worker_id}] register failed: {e}")
        return

    # 启动 gRPC 服务端
    servicer = MockWorkerServicer(worker_id, cp_stub, server_epoch)
    server = grpc_aio.server()
    worker_service_pb2_grpc.add_WorkerGrpcServiceServicer_to_server(servicer, server)
    server.add_insecure_port(listen_addr)
    await server.start()

    # 心跳循环（双向流）
    async def heartbeat_loop():
        async def gen_requests():
            while not stop_event.is_set():
                yield scheduler_pb2.HeartbeatRequest(
                    worker_id=worker_id,
                    load=servicer.current_load(),
                    max_load=WORKER_CAPACITY,
                    timestamp_ms=int(time.time() * 1000),
                    server_epoch=server_epoch[0],
                )
                await asyncio.sleep(HEARTBEAT_INTERVAL)

        try:
            async for resp in cp_stub.WorkerHeartbeat(gen_requests()):
                if resp.server_epoch:
                    server_epoch[0] = resp.server_epoch
                if resp.drain and resp.drain.drain:
                    break
        except Exception:
            pass  # 测试结束时 server 关闭会断连

    hb_task = asyncio.create_task(heartbeat_loop())

    await stop_event.wait()
    hb_task.cancel()
    await server.stop(grace=0)
    await cp_channel.close()

# ── 负载生成器（模拟 VeRL adapter 调用 ExecuteBatch）──────────────────────────
async def load_generator(stop_event: asyncio.Event):
    channel = grpc_aio.insecure_channel(SERVER_ADDR, options=[
        ('grpc.max_receive_message_length', 64 * 1024 * 1024),
        ('grpc.max_send_message_length',    64 * 1024 * 1024),
    ])
    stub = adapter_core_pb2_grpc.AdapterCoreServiceStub(channel)

    # 等 workers 注册完成
    await asyncio.sleep(2.0)

    async def send_batch():
        batch_id = str(uuid.uuid4())
        samples = []
        for i in range(BATCH_SIZE):
            req_id = str(uuid.uuid4())
            samples.append(adapter_core_pb2.SampleEnvelope(
                request_id=req_id,
                batch_id=batch_id,
                sample_index=i,
                framework="verl",
            env_type=ENV_TYPE,
                payload_json=json.dumps({
                    "question": "2+2=?",
                    "timeout_seconds": 120,
                }).encode(),
                meta_json=b"{}",
            ))
        t0 = time.time()
        try:
            resp = await stub.ExecuteBatch(adapter_core_pb2.ExecuteBatchRequest(
                request_id=batch_id,
                batch_id=batch_id,
                samples=samples,
            ))
            elapsed_ms = (time.time() - t0) * 1000
            for r in resp.results:
                METRICS.record(elapsed_ms / BATCH_SIZE, r.status)
        except grpc.RpcError as e:
            METRICS.record_error()
            if not stop_event.is_set():
                print(f"[load_gen] batch error: {e.code()} {e.details()[:80]}")

    # 持续发批次，直到 stop_event
    sem = asyncio.Semaphore(N_CONCURRENT_BATCHES)
    tasks = []

    async def bounded_batch():
        async with sem:
            await send_batch()

    while not stop_event.is_set():
        task = asyncio.create_task(bounded_batch())
        tasks.append(task)
        await asyncio.sleep(0)  # 不限速，让 server 成为瓶颈

    # 等待所有 in-flight batch 完成
    if tasks:
        await asyncio.gather(*tasks, return_exceptions=True)
    await channel.close()

# ── 进度打印 ──────────────────────────────────────────────────────────────────
async def progress_printer(stop_event: asyncio.Event, start_time: float):
    while not stop_event.is_set():
        await asyncio.sleep(10)
        elapsed = time.time() - start_time
        with METRICS.lock:
            total = METRICS.completed + METRICS.failed + METRICS.errors
            tput = total / elapsed if elapsed > 0 else 0
        print(f"[{elapsed:.0f}s] 已完成 {total} ep, 吞吐 {tput:.1f} ep/s")

# ── 主流程 ────────────────────────────────────────────────────────────────────
async def main():
    print(f"启动 adapter-core ({ADAPTER_BIN})")
    env = os.environ.copy()
    env.update({
        "UENV_ADDR": f"[::]:{SERVER_ADDR.split(':')[1]}",
        "UENV_CONFIG_PATH": SERVER_YAML,
        "RUST_LOG": "warn",
    })

    adapter_log = open(f"{LOG_DIR}/adapter_core.log", "w")
    adapter_proc = subprocess.Popen(
        [ADAPTER_BIN],
        env=env,
        stdout=adapter_log,
        stderr=adapter_log,
    )
    print(f"adapter-core PID={adapter_proc.pid}, 等待启动...")
    await asyncio.sleep(1.5)

    stop_event = asyncio.Event()

    print(f"启动 {N_WORKERS} 个 mock worker (端口 {WORKER_BASE_PORT}-{WORKER_BASE_PORT+N_WORKERS-1})")
    worker_tasks = [
        asyncio.create_task(run_worker(f"mock-worker-{i:02d}", WORKER_BASE_PORT + i, stop_event))
        for i in range(N_WORKERS)
    ]

    # 等 workers 注册
    await asyncio.sleep(1.5)
    print(f"Workers 已注册，开始压测 (时长={TEST_DURATION}s, "
          f"batch={BATCH_SIZE}, 并发={N_CONCURRENT_BATCHES}, "
          f"episode_ms={EPISODE_DURATION_MS})")

    start_time = time.time()
    load_task     = asyncio.create_task(load_generator(stop_event))
    progress_task = asyncio.create_task(progress_printer(stop_event, start_time))

    await asyncio.sleep(TEST_DURATION)
    print("\n测试时间到，停止...")
    stop_event.set()

    await load_task
    progress_task.cancel()
    await asyncio.gather(*worker_tasks, return_exceptions=True)

    elapsed = time.time() - start_time
    print(METRICS.summary(elapsed))

    adapter_proc.terminate()
    adapter_proc.wait()
    adapter_log.close()
    print(f"adapter-core 日志: {LOG_DIR}/adapter_core.log")

if __name__ == "__main__":
    asyncio.run(main())
