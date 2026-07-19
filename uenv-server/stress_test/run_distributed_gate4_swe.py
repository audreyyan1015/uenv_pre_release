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
import json
import os
from pathlib import Path
import sys
import tempfile
import time
import uuid

import distributed_stress_runtime as base


# SWE worker 会打开 runtime gateway，OpenHands agent 通过这个 gateway 执行任务。
GATEWAY_PORT = 8777

# OpenHands runner 自己的 API 和健康检查端口，只监听 worker 本机。
AGENT_API_PORT = 18004
AGENT_HEALTH_PORT = 18005

# Gate4 使用一个固定的 SWEBench verified 实例，便于每次结果可对比。
INSTANCE_ID = "astropy__astropy-7166"
IMAGE = "swebench/sweb.eval.x86_64.astropy_1776_astropy-7166:latest"
IMAGE_ID = "sha256:6909381901b865b904d9cfce69e412f659de0dc1e0454abb052c88b116654a83"
OPENHANDS_PYTHON = "/usr/bin/python3.12"
COMMON_SOURCE = Path(__file__).with_name("stress_test_common.py").read_text(encoding="utf-8")


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
import urllib.request

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
url = base_url if base_url.endswith("/chat/completions") else base_url + "/chat/completions"
payload = json.dumps({
    "model": model,
    "messages": [{"role": "user", "content": "Reply with OK."}],
    "temperature": 0,
    "max_tokens": 4,
}).encode()
request = urllib.request.Request(url, data=payload, method="POST")
request.add_header("Content-Type", "application/json")
request.add_header("Authorization", f"Bearer {api_key}")
with urllib.request.urlopen(request, timeout=90) as response:
    document = json.loads(response.read().decode())
if not isinstance(document.get("choices"), list) or not document["choices"]:
    raise SystemExit("minimal authenticated LLM call returned no choices")
print(json.dumps({
    "schema_valid": True,
    "auth_and_minimal_call_valid": True,
    "model": model,
    "base_url": base_url,
    "response_id_present": bool(document.get("id")),
}, sort_keys=True))
'''


SWE_CLIENT = r'''#!/usr/bin/env python3
# 这个脚本会临时写到 server 机器上运行。
# 它等待 SWE worker 注册后，向 AdapterCore ExecuteBatch 提交真实 SWE episode。
import argparse
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
parser.add_argument("--run-id", required=True)
parser.add_argument("--driver", required=True)
parser.add_argument("--catalog", required=True)
parser.add_argument("--output", required=True)
parser.add_argument("--concurrency", type=int, choices=(1, 2), required=True)
parser.add_argument("--mode", choices=("gold", "llm"), required=True)
parser.add_argument("--max-steps", type=int, required=True)
parser.add_argument("--openhands-max-iterations", type=int, required=True)
parser.add_argument("--llm-config", default="")
args = parser.parse_args()

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

# 只等待本次启动的 worker_id。这样不会误把其它 worker 当成本次压测 worker。
deadline = time.monotonic() + 120
registered = []
while time.monotonic() < deadline:
    try:
        if not health(adapter_core_pb2.HealthCheckRequest(), timeout=3).ok:
            time.sleep(1)
            continue
        response = list_workers(scheduler_pb2.ListWorkersRequest(), timeout=3)
        registered = [worker.worker_id for worker in response.workers]
        if args.worker_id in registered:
            break
    except grpc.RpcError:
        pass
    time.sleep(1)
else:
    raise SystemExit(f"worker did not register: expected={args.worker_id} registered={registered}")

def env_config():
    # 这里生成的 env_config_json 会交给 SWE worker。
    # driver 是 official runner，catalog 是 verified.json。
    # mode=llm 表示真实 OpenHands agent 多轮执行；mode=gold 只用于必要的基线排查。
    return stress_common.swe_openhands_env_payload(
        instance_id="astropy__astropy-7166",
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

batch_id = str(uuid.uuid4())
# concurrency=1 或 2 时，在同一个 batch 里提交对应数量的样本。
samples = [
    stress_common.make_sample_envelope(
        adapter_core_pb2,
        batch_id=batch_id,
        sample_index=index,
        env_type="swe",
        parallel_mode="sync",
        env_config=env_config(),
        reward_config=stress_common.swe_reward_config(),
        sample_context={
            "stress_run_id": args.run_id,
            "environment": "swe_openhands",
            "gate4_concurrency": args.concurrency,
            "mode": args.mode,
            "max_steps": args.max_steps,
            "openhands_max_iterations": args.openhands_max_iterations,
        },
        timeout_seconds=900,
        max_steps=args.max_steps,
    )
    for index in range(args.concurrency)
]
started = time.monotonic()
response = execute(
    adapter_core_pb2.ExecuteBatchRequest(
        request_id=batch_id, batch_id=batch_id, samples=samples
    ),
    timeout=960,
)
elapsed = time.monotonic() - started
if len(response.results) != args.concurrency:
    raise SystemExit(f"unexpected result count: {len(response.results)}")
# 把 proto result 转成 JSON 友好的 dict，便于后续保存和人工查看。
results = [stress_common.sample_result_dict(result) for result in response.results]
document = stress_common.gate4_swe_result_document(
    run_id=args.run_id,
    server=args.server,
    worker_id=args.worker_id,
    registered_workers=registered,
    instance_id="astropy__astropy-7166",
    mode=args.mode,
    concurrency=args.concurrency,
    max_steps=args.max_steps,
    openhands_max_iterations=args.openhands_max_iterations,
    elapsed_seconds=elapsed,
    results=results,
)
with open(args.output, "w", encoding="utf-8") as destination:
    json.dump(document, destination, indent=2, sort_keys=True)
print(json.dumps(document, indent=2, sort_keys=True))
if not document["infrastructure"]["passed"]:
    raise SystemExit(1)
'''


def server_config() -> str:
    """生成 Gate4 隔离 server 配置。

    SWE episode 比 Code episode 慢很多，所以 default_timeout_secs 和
    worker_degraded_threshold_secs 都设置得更长。
    """
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
  broadcast_capacity: 1024
  completed_async_ttl_secs: 3600
  completed_async_max_entries: 10000
'''


def worker_config(run_dir: str, worker_id: str, run_id: str, concurrency: int) -> str:
    """生成 SWE worker 配置。

    runtime_gateway 是 SWE/OpenHands 的关键：OpenHands agent 通过它访问
    worker 管理的容器运行环境。capacity 和本轮容器并发保持一致。
    """
    return f'''server:
  endpoint: "{base.SERVER_PRIVATE_IP}:{base.SERVER_PORT}"
worker:
  id: "{worker_id}"
  listen: "0.0.0.0:{base.WORKER_PORT}"
  advertise_endpoint: "{base.WORKER_PRIVATE_IP}:{base.WORKER_PORT}"
  max_concurrent: {concurrency}
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
  file: "{run_dir}/worker.runtime.log"
wal:
  dir: "{run_dir}/wal"
observability:
  metrics_listen: "127.0.0.1:{base.OBS_PORT}"
  health_listen: "127.0.0.1:{base.OBS_PORT}"
hub:
  enabled: false
runtime_gateway:
  enabled: true
  listen: "0.0.0.0:{GATEWAY_PORT}"
  capacity: {concurrency}
  api_key: "stress-gateway-{run_id}"
swe:
  variants: ["verified"]
  prewarm: []
  warm_tag: false
'''


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


class RunArgs:
    """给 run_one 传参的小对象，避免把 argparse 结果传进内部逻辑。"""
    def __init__(
        self,
        *,
        concurrency: int,
        artifacts: Path,
        mode: str,
        max_steps: int,
        openhands_max_iterations: int,
        llm_config: str,
    ) -> None:
        self.concurrency = concurrency
        self.artifacts = artifacts
        self.mode = mode
        self.max_steps = max_steps
        self.openhands_max_iterations = openhands_max_iterations
        self.llm_config = llm_config


def run_one(
    concurrency: int,
    artifacts: Path,
    mode: str,
    max_steps: int,
    openhands_max_iterations: int,
    llm_config: str,
) -> int:
    """执行一轮 Gate4。

    concurrency 表示真实容器任务并发数，目前只允许 1 或 2。
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
        max_steps=max_steps,
        openhands_max_iterations=openhands_max_iterations,
        llm_config=llm_config,
    )
    password = os.environ.get("UENV_PASS")
    if not password:
        raise SystemExit("UENV_PASS is required")

    run_id = f"gate4-swe-c{args.concurrency}-{time.strftime('%Y%m%d-%H%M%S')}-{uuid.uuid4().hex[:8]}"
    server_run = f"/tmp/uenv-{run_id}"
    worker_run = f"/opt/uenv-stress/runs/{run_id}"
    worker_id = f"stress-{run_id}-worker-0000"
    agent_id = f"stress-{run_id}-agent-0000"
    args.artifacts.mkdir(parents=True, exist_ok=True)
    local_run = args.artifacts / run_id
    local_run.mkdir()

    server = worker = None
    server_pid = worker_pid = agent_pid = monitor_pid = None
    before_protected = None
    before_containers: set[str] = set()
    error: str | None = None
    result_code = 1
    cleanup_errors: list[str] = []
    try:
        # 先连接两台机器，并记录正式 server 和容器集合的基线。
        server = base.connect(base.SERVER_HOST, password)
        worker = base.connect(base.WORKER_HOST, password)
        before_protected = base.protected_snapshot(server)
        build = base.source_and_binary_manifest(server, include_code_plugin=False)
        before_containers = container_ids(worker)
        if before_containers:
            raise RuntimeError(f"worker host is not container-empty: {sorted(before_containers)}")
        # Gate4 涉及 server、worker、gateway、agent API、agent health 多个端口。
        # 全部确认空闲后再启动。
        for port, host, client in (
            (base.SERVER_PORT, base.SERVER_HOST, server),
            (base.WORKER_PORT, base.WORKER_HOST, worker),
            (base.OBS_PORT, base.WORKER_HOST, worker),
            (GATEWAY_PORT, base.WORKER_HOST, worker),
            (AGENT_API_PORT, base.WORKER_HOST, worker),
            (AGENT_HEALTH_PORT, base.WORKER_HOST, worker),
        ):
            base.assert_port_free(client, port, host)
        # 镜像 ID 必须匹配预期值，避免使用同名但内容不同的镜像。
        _, image_id, _ = base.run(worker, f"docker image inspect {base.q(IMAGE)} --format '{{{{.Id}}}}'")
        if image_id.strip() != IMAGE_ID:
            raise RuntimeError(f"SWE image mismatch actual={image_id.strip()} expected={IMAGE_ID}")
        print(f"[preflight] protected={json.dumps(before_protected, sort_keys=True)}")
        print(f"[preflight] image={IMAGE} id={image_id.strip()}")
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
        if args.mode == "llm":
            if not args.llm_config:
                raise ValueError("Gate4 llm mode requires --llm-config or OPENHANDS_LLM_CONFIG")
            base.run(worker, f"test -f {base.q(args.llm_config)} && test $(stat -c %a {base.q(args.llm_config)}) = 600")
            base.put_text(worker, f"{worker_run}/llm_preflight.py", LLM_PREFLIGHT, 0o700)
            _, llm_output, _ = base.run(
                worker,
                "cd /opt/openhands/benchmarks && "
                f".venv/bin/python {base.q(worker_run)}/llm_preflight.py --config {base.q(args.llm_config)}",
                timeout=120,
            )
            llm_preflight = json.loads(llm_output)
            _, llm_config_hash, _ = base.run(worker, f"sha256sum {base.q(args.llm_config)}")
            llm_config_sha256 = llm_config_hash.split()[0]
            print(f"[preflight] OpenHands LLM schema/auth/minimal call verified: {llm_preflight}")
        else:
            llm_preflight = {}
            llm_config_sha256 = ""

        # 在 server 机器打包需要的 worker、plugins、integrations 和 SWE 配置，
        # 再传到 worker 机器解包运行。
        base.run(server, f"install -d -m 0755 {base.q(server_run)}/bundle {base.q(server_run)}/generated/uenv/v1")
        base.run(worker, f"install -d -m 0755 {base.q(worker_run)} {base.q(worker_run)}/wal {base.q(worker_run)}/openhands")
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

        base.put_text(server, f"{server_run}/server.yaml", server_config())
        base.put_text(server, f"{server_run}/smoke_client.py", SWE_CLIENT, 0o755)
        base.put_text(server, f"{server_run}/stress_test_common.py", COMMON_SOURCE)
        base.put_text(worker, f"{worker_run}/worker.yaml", worker_config(worker_run, worker_id, run_id, args.concurrency))
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

        # 启动隔离 server。关闭 trajectory/obs，减少与压测目标无关的额外工作。
        server_command = " ".join([
            "env", "UENV_SERVER_CONFIG_STRICT=1", "UENV_TRAJECTORY_ENABLED=0",
            "UENV_OBS_ENABLED=0", "UENV_LOG_ANSI=0",
            f"UENV_ADDR={base.SERVER_PRIVATE_IP}:{base.SERVER_PORT}",
            f"UENV_SWE_GATEWAY_API_KEY=stress-gateway-{run_id}",
            f"UENV_CONFIG_PATH={server_run}/server.yaml", "RUST_LOG=warn", base.SERVER_BIN,
        ])
        server_pid = base.start_owned(
            server, server_command, f"{server_run}/server.log", base.SERVER_BIN, base.SERVER_BIN
        )
        # 启动 SWE worker。UENV_SWE_GATEWAY_PUBLIC_URL 是 OpenHands agent
        # 访问 runtime gateway 的地址。
        worker_command = " ".join([
            "env", "UENV_SERVER_CONFIG_STRICT=1", "UENV_TRAJECTORY_ENABLED=0",
            "UENV_OBS_ENABLED=0", "UENV_LOG_ANSI=0",
            "UENV_WORKER_EPISODE_TIMEOUT_SECS=900", "UENV_LLM_HTTP_TIMEOUT_SECS=900",
            f"UENV_SWE_INSTANCES={worker_run}/bundle/config/swe/verified.json",
            "UENV_SWE_RUNTIME=docker",
            f"UENV_SWE_GATEWAY_API_KEY=stress-gateway-{run_id}",
            f"UENV_SWE_GATEWAY_PUBLIC_URL=http://{base.WORKER_PRIVATE_IP}:{GATEWAY_PORT}",
            "RUST_LOG=info", f"{worker_run}/bundle/uenv-worker",
            "--config", f"{worker_run}/worker.yaml", "serve",
        ])
        worker_pid = base.start_owned(
            worker, worker_command, f"{worker_run}/worker.log",
            f"{worker_run}/bundle/uenv-worker", f"{worker_run}/worker.yaml",
        )
        # worker 进程启动成功不代表 runtime gateway 已经监听，所以单独等端口。
        deadline = time.monotonic() + 90
        while time.monotonic() < deadline:
            gateway_lines = [line for line in base.listeners(worker).splitlines() if f":{GATEWAY_PORT} " in line]
            if len(gateway_lines) == 1 and f"pid={worker_pid}," in gateway_lines[0]:
                break
            time.sleep(1)
        else:
            raise TimeoutError("runtime gateway did not bind")

        # OpenHands agent 通过轮询 server 获取任务。下面这些环境变量告诉它：
        # 去哪个 server 拉任务、使用哪个 agent_id、运行目录在哪里、如何访问 gateway。
        agent_env = {
            "PYTHONPATH": f"{worker_run}/generated:{worker_run}/bundle/integrations/openhands",
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
            agent_env["OPENHANDS_LLM_CONFIG"] = args.llm_config
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
            "gate": 4,
            "container_concurrency": args.concurrency,
            "max_steps": args.max_steps,
            "openhands_max_iterations": args.openhands_max_iterations,
            "llm_config": args.llm_config if args.mode == "llm" else "",
            "llm_config_sha256": llm_config_sha256,
            "llm_preflight": llm_preflight,
            "server_addr": f"{base.SERVER_PRIVATE_IP}:{base.SERVER_PORT}",
            "worker_addr": f"{base.WORKER_PRIVATE_IP}:{base.WORKER_PORT}",
            "worker_id": worker_id,
            "agent_id": agent_id,
            "instance_id": INSTANCE_ID,
            "image": IMAGE,
            "image_id": IMAGE_ID,
            "source_and_binaries": build,
            "protected_server": before_protected,
            "owned_pids": {"server": server_pid, "worker": worker_pid, "agent": agent_pid, "monitor": monitor_pid},
        }
        base.put_text(server, f"{server_run}/manifest.json", json.dumps(manifest, indent=2, sort_keys=True))
        base.put_text(worker, f"{worker_run}/manifest.json", json.dumps(manifest, indent=2, sort_keys=True))

        # 真正提交 SWE episode 的动作发生在 server 机器上的 smoke_client.py 中。
        client_command = " ".join([
            f"PYTHONPATH={server_run}:{server_run}/generated", "python3", "-B", f"{server_run}/smoke_client.py",
            "--server", f"{base.SERVER_PRIVATE_IP}:{base.SERVER_PORT}",
            "--worker-id", worker_id,
            "--run-id", run_id,
            "--driver", f"{worker_run}/bundle/integrations/openhands/run_swebenchpro_official.py",
            "--catalog", f"{worker_run}/bundle/config/swe/verified.json",
            "--output", f"{server_run}/result.json",
            "--concurrency", str(args.concurrency),
            "--mode", args.mode,
            "--max-steps", str(args.max_steps),
            "--openhands-max-iterations", str(args.openhands_max_iterations),
            "--llm-config", args.llm_config if args.mode == "llm" else "",
        ])
        client_status, client_out, client_err = base.run(
            server, client_command, timeout=1100, check=False
        )
        print("[smoke] client output")
        print(client_out)
        if client_err:
            print(client_err)
        result_text = base.get_text(server, f"{server_run}/result.json")
        base.stop_owned(worker, monitor_pid, "/usr/bin/python3.12", f"{worker_run}/resource_monitor.py")
        monitor_pid = None
        resources_text = base.get_text(worker, f"{worker_run}/resources.jsonl")
        # 用资源监控数据确认真实容器数量确实达到本轮 concurrency。
        resource_rows = [json.loads(line) for line in resources_text.splitlines() if line.strip()]
        peak_containers = max((row["running_containers"] for row in resource_rows), default=0)
        min_available_kib = min((row["mem_available_kib"] for row in resource_rows), default=0)
        resource_summary = {
            "peak_running_containers": peak_containers,
            "min_mem_available_kib": min_available_kib,
            "samples": len(resource_rows),
        }
        if peak_containers < args.concurrency:
            raise RuntimeError(
                f"did not observe requested real container concurrency: requested={args.concurrency} peak={peak_containers}"
            )
        (local_run / "result.json").write_text(result_text)
        (local_run / "resources.jsonl").write_text(resources_text)
        (local_run / "resource-summary.json").write_text(json.dumps(resource_summary, indent=2, sort_keys=True))
        (local_run / "manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True))
        for client, remote_path, local_name in (
            (server, f"{server_run}/server.log", "server.log"),
            (worker, f"{worker_run}/worker.log", "worker.log"),
            (worker, f"{worker_run}/worker.runtime.log", "worker.runtime.log"),
            (worker, f"{worker_run}/agent.log", "agent.log"),
        ):
            try:
                (local_run / local_name).write_text(base.get_text(client, remote_path))
            except OSError:
                pass
        if client_status != 0:
            result_document = json.loads(result_text)
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
            for pid, exe, fragment in (
                (monitor_pid, "/usr/bin/python3.12", f"{worker_run}/resource_monitor.py"),
                (agent_pid, OPENHANDS_PYTHON, f"{worker_run}/bundle/scripts/openhands/openhands_runner.py"),
                (worker_pid, f"{worker_run}/bundle/uenv-worker", f"{worker_run}/worker.yaml"),
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
            for port in (
                base.WORKER_PORT, base.OBS_PORT, GATEWAY_PORT,
                AGENT_API_PORT, AGENT_HEALTH_PORT,
            ):
                try:
                    base.assert_port_free(worker, port, base.WORKER_HOST)
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
        choices=(1, 2),
        action="append",
        help="Run only the selected container concurrency. Omit to run Gate4's 1 then 2 sequence.",
    )
    parser.add_argument("--mode", choices=("gold", "llm"), default="llm")
    parser.add_argument("--max-steps", type=int, default=10)
    parser.add_argument("--openhands-max-iterations", type=int, default=10)
    parser.add_argument(
        "--llm-config",
        default=os.environ.get("OPENHANDS_LLM_CONFIG", ""),
        help="Path on the worker host to a real OpenHands LLM config. Required when --mode llm.",
    )
    parser.add_argument("--gateway-port", type=int, default=8777)
    parser.add_argument("--agent-api-port", type=int, default=18004)
    parser.add_argument("--agent-health-port", type=int, default=18005)
    base.add_runtime_arguments(parser, require_code_plugin=False)
    args = parser.parse_args()
    base.configure_from_args(args)
    GATEWAY_PORT = args.gateway_port
    AGENT_API_PORT = args.agent_api_port
    AGENT_HEALTH_PORT = args.agent_health_port
    allowed_exposed_ports = {5432, 6379, 8000, 8077, 8088, 8099, 8777, 8888}
    exposed = {
        "isolated server": base.SERVER_PORT,
        "SWE worker": base.WORKER_PORT,
        "runtime gateway": GATEWAY_PORT,
    }
    for label, port in exposed.items():
        if port not in allowed_exposed_ports:
            raise SystemExit(f"{label} port {port} is outside the explicitly allowed cloud ports")
    if base.SERVER_PORT in base.PROTECTED_PORTS:
        raise SystemExit("isolated server port must not overlap a protected production port")
    if args.max_steps <= 0:
        raise SystemExit("--max-steps must be positive")
    if args.openhands_max_iterations <= 0:
        raise SystemExit("--openhands-max-iterations must be positive")
    if args.mode == "llm" and not args.llm_config:
        raise SystemExit("--mode llm requires --llm-config or OPENHANDS_LLM_CONFIG")
    args.artifacts.mkdir(parents=True, exist_ok=True)

    concurrencies = args.concurrency or [1, 2]
    summary = []
    final_code = 0
    for concurrency in concurrencies:
        print(f"[gate4] concurrency={concurrency} start", flush=True)
        started = time.monotonic()
        returncode = run_one(
            concurrency,
            args.artifacts,
            args.mode,
            args.max_steps,
            args.openhands_max_iterations,
            args.llm_config,
        )
        item = {
            "container_concurrency": concurrency,
            "returncode": returncode,
            "wall_seconds": time.monotonic() - started,
        }
        summary.append(item)
        print(f"[gate4] concurrency={concurrency} done returncode={returncode}", flush=True)
        if returncode != 0:
            final_code = returncode
            break

    summary_path = args.artifacts / f"gate4-summary-{time.strftime('%Y%m%d-%H%M%S')}.json"
    summary_path.write_text(json.dumps(summary, indent=2, sort_keys=True))
    print(f"[gate4] summary={summary_path}")
    return final_code


if __name__ == "__main__":
    raise SystemExit(main())
