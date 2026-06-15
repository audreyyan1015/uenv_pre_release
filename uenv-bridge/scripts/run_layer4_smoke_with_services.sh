#!/usr/bin/env bash
set -euo pipefail

# 打印脚本用途、依赖服务布局以及常用运行参数。
usage() {
  cat <<'EOF'
Run the real pre-rollout Layer 4 smoke test end to end.

This wrapper starts the local dependencies needed by the real Layer 4 path:
  1. mock OpenAI-compatible model endpoint
  2. Rust adapter core with server backend
  3. uenv-worker registered to the adapter core
Then it runs scripts/run_verl_grpo_1step_with_uenv_agent_loop.sh.

Usage:
  ./scripts/run_layer4_smoke_with_services.sh

Common environment overrides:
  IMAGE                         VeRL image. Default: localhost/uenv-bridge-verl:latest
  TRAINING_STEPS                Default: 1
  SAMPLE_COUNT                  Default: 1
  TRAIN_BATCH_SIZE              Default: 1
  ROLLOUT_N                     Default: 1
  ROLLOUT_FREE_CACHE_ENGINE     Default: False
  ROLLOUT_ENABLE_SLEEP_MODE     Default: False
  CORE_ADDR                     Default: 127.0.0.1:50053
  WORKER_LISTEN                 Default: 127.0.0.1:50054
  MOCK_MODEL_ADDR               Default: 127.0.0.1:18080
  MODEL_NAME                    Default: mock-policy
  BUILD_RUST                    Build host Rust binaries before running. Default: 0
  START_MOCK_MODEL              Start built-in mock model endpoint. Default: 1
  UENV_ROLLOUT_MODEL_ENDPOINT   Required only when START_MOCK_MODEL=0
  KEEP_SERVICES                 Do not stop mock/core/worker after run. Default: 0

Example:
  IMAGE=localhost/uenv-bridge-verl:layer4-build ./scripts/run_layer4_smoke_with_services.sh
EOF
}

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
  usage
  exit 0
fi

# 解析仓库路径。REPO_DIR 指向 uenv-bridge，WORKSPACE_ROOT 指向上一级
# uenv 工作区，其中包含共享 proto、worker、server 和 plugins。
REPO_DIR=${REPO_DIR:-"$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"}
WORKSPACE_ROOT=${WORKSPACE_ROOT:-"$(cd "${REPO_DIR}/.." && pwd)"}

# 配置将在容器内运行的 VeRL 训练任务参数。
IMAGE=${IMAGE:-localhost/uenv-bridge-verl:latest}
TRAINING_STEPS=${TRAINING_STEPS:-1}
SAMPLE_COUNT=${SAMPLE_COUNT:-1}
TRAIN_BATCH_SIZE=${TRAIN_BATCH_SIZE:-1}
ROLLOUT_N=${ROLLOUT_N:-1}
ROLLOUT_FREE_CACHE_ENGINE=${ROLLOUT_FREE_CACHE_ENGINE:-False}
ROLLOUT_ENABLE_SLEEP_MODE=${ROLLOUT_ENABLE_SLEEP_MODE:-False}
ROLLOUT_GPU_MEMORY_UTILIZATION=${ROLLOUT_GPU_MEMORY_UTILIZATION:-0.25}
AGENT_NUM_WORKERS=${AGENT_NUM_WORKERS:-1}
RAY_NUM_CPUS=${RAY_NUM_CPUS:-4}
CUDA_VISIBLE_DEVICES_IN_CONTAINER=${CUDA_VISIBLE_DEVICES_IN_CONTAINER:-0}

# 配置本地服务地址。这些服务运行在宿主机上，默认通过 host network
# 被 VeRL 容器访问。
RUN_ID=${RUN_ID:-layer4_smoke_$(date +%Y%m%d_%H%M%S)}
CORE_ADDR=${CORE_ADDR:-127.0.0.1:50053}
WORKER_LISTEN=${WORKER_LISTEN:-127.0.0.1:50054}
WORKER_ID=${WORKER_ID:-layer4-worker}
MOCK_MODEL_ADDR=${MOCK_MODEL_ADDR:-127.0.0.1:18080}
MODEL_NAME=${MODEL_NAME:-mock-policy}
METRICS_LISTEN=${METRICS_LISTEN:-127.0.0.1:19091}
HEALTH_LISTEN=${HEALTH_LISTEN:-${METRICS_LISTEN}}

# 配置可选的构建行为和服务生命周期行为。
BUILD_RUST=${BUILD_RUST:-0}
START_MOCK_MODEL=${START_MOCK_MODEL:-1}
KEEP_SERVICES=${KEEP_SERVICES:-0}
PODMAN_NETWORK_ARGS=${PODMAN_NETWORK_ARGS:---network host}

# 定位 smoke test 所需的宿主机侧二进制文件和插件资源。
ADAPTER_CORE_BIN=${ADAPTER_CORE_BIN:-${WORKSPACE_ROOT}/target/debug/uenv-adapter-core}
WORKER_BIN=${WORKER_BIN:-${WORKSPACE_ROOT}/target/debug/uenv-worker}
MATH_PLUGIN_BIN=${MATH_PLUGIN_BIN:-${WORKSPACE_ROOT}/target/debug/uenv-math-plugin}
PLUGIN_DIR=${PLUGIN_DIR:-${WORKSPACE_ROOT}/plugins}

# 为本次运行创建独立目录，用于保存服务日志、worker WAL 文件以及
# 可选的 worker 注册快照。
SERVICE_DIR=${SERVICE_DIR:-${REPO_DIR}/tmp/layer4_smoke/${RUN_ID}}
CONTAINER_SERVICE_DIR=/tmp/uenv-bridge/tmp/layer4_smoke/${RUN_ID}
WAL_DIR=${WAL_DIR:-${SERVICE_DIR}/wal}
MOCK_MODEL_LOG=${MOCK_MODEL_LOG:-${SERVICE_DIR}/mock-model.log}
CORE_LOG=${CORE_LOG:-${SERVICE_DIR}/adapter-core.log}
WORKER_STDOUT_LOG=${WORKER_STDOUT_LOG:-${SERVICE_DIR}/worker-stdout.log}
WORKER_LOG=${WORKER_LOG:-${SERVICE_DIR}/worker.log}
WORKERS_JSON=${WORKERS_JSON:-${SERVICE_DIR}/workers.json}
AGENT_LOOP_RESULT_RECORD_PATH=${AGENT_LOOP_RESULT_RECORD_PATH:-${CONTAINER_SERVICE_DIR}/agent-loop-results.jsonl}
mkdir -p "${SERVICE_DIR}" "${WAL_DIR}"

# 记录本脚本启动的服务进程 id，退出时由 cleanup trap 统一清理。
PIDS=()

# 从 host:port 地址中取出 host 部分。
split_host() {
  local addr="$1"
  printf '%s\n' "${addr%:*}"
}

# 从 host:port 地址中取出 port 部分。
split_port() {
  local addr="$1"
  printf '%s\n' "${addr##*:}"
}

# 检查 TCP 地址是否已经可以建立连接；可以连接则返回成功。
port_open() {
  local host="$1"
  local port="$2"
  python3 - "$host" "$port" >/dev/null 2>&1 <<'PY'
import socket
import sys

host = sys.argv[1]
port = int(sys.argv[2])
sock = socket.socket()
sock.settimeout(0.5)
try:
    sock.connect((host, port))
except OSError:
    sys.exit(1)
else:
    sys.exit(0)
finally:
    sock.close()
PY
}

# 如果服务地址已经被占用，则提前失败，避免后续启动服务时才报错。
require_free_addr() {
  local name="$1"
  local addr="$2"
  local host
  local port
  host="$(split_host "$addr")"
  port="$(split_port "$addr")"
  if port_open "$host" "$port"; then
    echo "${name} address ${addr} is already in use" >&2
    echo "Stop the process on ${addr}, or override the address before running this script." >&2
    exit 1
  fi
}

# 等待刚启动的服务开始监听端口。
wait_for_addr() {
  local name="$1"
  local addr="$2"
  local timeout_seconds="$3"
  local host
  local port
  host="$(split_host "$addr")"
  port="$(split_port "$addr")"
  for _ in $(seq 1 "$timeout_seconds"); do
    if port_open "$host" "$port"; then
      echo "${name} is listening on ${addr}"
      return 0
    fi
    sleep 1
  done
  echo "Timed out waiting for ${name} on ${addr}" >&2
  return 1
}

# 除非设置 KEEP_SERVICES=1 用于调试，否则退出时停止本 wrapper 启动的
# 所有服务进程，并保留原始命令退出状态。
cleanup() {
  local status=$?
  if [ "${KEEP_SERVICES}" = "1" ]; then
    echo "KEEP_SERVICES=1; leaving service logs under ${SERVICE_DIR}"
    return "${status}"
  fi
  for pid in "${PIDS[@]:-}"; do
    kill "${pid}" >/dev/null 2>&1 || true
  done
  sleep 1
  for pid in "${PIDS[@]:-}"; do
    kill -9 "${pid}" >/dev/null 2>&1 || true
  done
  return "${status}"
}
trap cleanup EXIT INT TERM

# 当显式要求构建或预期二进制不存在时，构建宿主机侧 Rust binaries。
# VeRL trainer 仍然运行在容器内；这些二进制负责提供容器要连接的
# 本地 adapter core 和 worker 服务。
if [ "${BUILD_RUST}" = "1" ] ||
   [ ! -x "${ADAPTER_CORE_BIN}" ] ||
   [ ! -x "${WORKER_BIN}" ] ||
   [ ! -x "${MATH_PLUGIN_BIN}" ]; then
  echo "Building host Rust binaries..."
  cargo build \
    --manifest-path "${WORKSPACE_ROOT}/Cargo.toml" \
    -p uenv-adapter-core \
    -p uenv-worker \
    --bin uenv-adapter-core \
    --bin uenv-worker \
    --bin uenv-math-plugin
fi

# 在占用端口之前，检查所有运行时二进制文件和插件路径是否存在。
for path in "${ADAPTER_CORE_BIN}" "${WORKER_BIN}" "${MATH_PLUGIN_BIN}" "${PLUGIN_DIR}"; do
  if [ ! -e "${path}" ]; then
    echo "Required path does not exist: ${path}" >&2
    exit 1
  fi
done

# 检查需要监听的地址是否空闲，并确定 worker 在 rollout 时要调用的
# 模型 endpoint。
require_free_addr "adapter core" "${CORE_ADDR}"
require_free_addr "worker" "${WORKER_LISTEN}"
if [ "${START_MOCK_MODEL}" = "1" ]; then
  require_free_addr "mock model" "${MOCK_MODEL_ADDR}"
  ROLLOUT_ENDPOINT="http://${MOCK_MODEL_ADDR}/v1"
else
  ROLLOUT_ENDPOINT=${UENV_ROLLOUT_MODEL_ENDPOINT:-}
  if [ -z "${ROLLOUT_ENDPOINT}" ]; then
    echo "UENV_ROLLOUT_MODEL_ENDPOINT is required when START_MOCK_MODEL=0" >&2
    exit 1
  fi
fi
WORKER_LLM_PROVIDER=${UENV_LLM_PROVIDER:-openai_compatible}
WORKER_LLM_ENDPOINT=${UENV_LLM_ENDPOINT:-${ROLLOUT_ENDPOINT}}
WORKER_LLM_MODEL_NAME=${UENV_LLM_MODEL_NAME:-${MODEL_NAME}}

# 按需启动一个最小 OpenAI-compatible model endpoint。对于只需要
# chat/completions API、不需要真实模型服务的 smoke test，这已经足够。
if [ "${START_MOCK_MODEL}" = "1" ]; then
  echo "Starting mock OpenAI-compatible model endpoint on ${MOCK_MODEL_ADDR}"
  LAYER4_MODEL_ADDR="${MOCK_MODEL_ADDR}" LAYER4_MODEL_NAME="${MODEL_NAME}" \
    python3 -u - >"${MOCK_MODEL_LOG}" 2>&1 <<'PY' &
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os

host, port = os.environ["LAYER4_MODEL_ADDR"].rsplit(":", 1)
model_name = os.environ["LAYER4_MODEL_NAME"]

class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path.rstrip("/") == "/v1/models":
            self.send_json({"object": "list", "data": [{"id": model_name, "object": "model"}]})
        else:
            self.send_error(404)

    def do_POST(self):
        length = int(self.headers.get("content-length", "0") or "0")
        body = self.rfile.read(length)
        print("POST", self.path, body.decode("utf-8", errors="replace")[:300], flush=True)
        if self.path.rstrip("/") == "/v1/chat/completions":
            self.send_json({
                "id": "mock-chatcmpl",
                "object": "chat.completion",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "20"},
                    "finish_reason": "stop",
                }],
            })
        else:
            self.send_error(404)

    def log_message(self, fmt, *args):
        print(fmt % args, flush=True)

    def send_json(self, data):
        payload = json.dumps(data).encode("utf-8")
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

print(f"mock model listening {host}:{port}", flush=True)
ThreadingHTTPServer((host, int(port)), Handler).serve_forever()
PY
  PIDS+=("$!")
  wait_for_addr "mock model" "${MOCK_MODEL_ADDR}" 20
fi

# 以 server backend 模式启动 Rust adapter core。Python 侧通过
# adapter_core.proto 定义的 gRPC 连接该进程。
echo "Starting adapter core on ${CORE_ADDR}"
UENV_ADDR="${CORE_ADDR}" \
UENV_ADAPTER_CORE_BACKEND=server \
  "${ADAPTER_CORE_BIN}" >"${CORE_LOG}" 2>&1 &
PIDS+=("$!")
wait_for_addr "adapter core" "${CORE_ADDR}" 20

# 启动一个 uenv-worker，并将其注册到 adapter core。worker 负责 math
# episode 执行、模型调用、reward 计算以及 trajectory 返回。
echo "Starting worker on ${WORKER_LISTEN}"
UENV_SERVER_ENDPOINT="${CORE_ADDR}" \
UENV_WORKER_LISTEN="${WORKER_LISTEN}" \
UENV_WORKER_ID="${WORKER_ID}" \
UENV_ENV_TYPES=math \
UENV_PLUGIN_DIR="${PLUGIN_DIR}" \
UENV_MATH_PLUGIN_BIN="${MATH_PLUGIN_BIN}" \
UENV_LOG_LEVEL=INFO \
UENV_LOG_FILE="${WORKER_LOG}" \
UENV_WAL_DIR="${WAL_DIR}" \
UENV_HUB_ENABLED=false \
UENV_METRICS_LISTEN="${METRICS_LISTEN}" \
UENV_HEALTH_LISTEN="${HEALTH_LISTEN}" \
UENV_LLM_PROVIDER="${WORKER_LLM_PROVIDER}" \
UENV_LLM_ENDPOINT="${WORKER_LLM_ENDPOINT}" \
UENV_LLM_MODEL_NAME="${WORKER_LLM_MODEL_NAME}" \
  "${WORKER_BIN}" serve >"${WORKER_STDOUT_LOG}" 2>&1 &
PIDS+=("$!")
wait_for_addr "worker" "${WORKER_LISTEN}" 20

# 如果本机有 grpcurl，则在启动成本较高的 VeRL 训练前，确认 worker
# 已经成功注册。
if command -v grpcurl >/dev/null 2>&1; then
  echo "Waiting for worker registration..."
  for _ in $(seq 1 30); do
    if grpcurl -plaintext \
      -import-path "${WORKSPACE_ROOT}/proto" \
      -proto uenv/v1/server.proto \
      -proto uenv/v1/scheduler.proto \
      -d '{}' \
      "${CORE_ADDR}" \
      uenv.v1.AdminService/ListWorkers >"${WORKERS_JSON}" 2>/dev/null &&
      grep -q "\"workerId\": \"${WORKER_ID}\"" "${WORKERS_JSON}"; then
      echo "worker registered: ${WORKER_ID}"
      break
    fi
    sleep 1
  done
  if ! grep -q "\"workerId\": \"${WORKER_ID}\"" "${WORKERS_JSON}" 2>/dev/null; then
    echo "Worker did not register to adapter core. Current worker list:" >&2
    cat "${WORKERS_JSON}" >&2 2>/dev/null || true
    exit 1
  fi
else
  echo "grpcurl not found; skipping worker registration check"
  sleep 3
fi

# 运行真实 VeRL GRPO smoke test。内部脚本会启动 VeRL 容器，启用
# UEnvAgentLoop，并将其指向上面启动的 adapter core。
echo "Running VeRL Layer 4 smoke test; service logs: ${SERVICE_DIR}"
set +e
IMAGE="${IMAGE}" \
TRAINING_STEPS="${TRAINING_STEPS}" \
SAMPLE_COUNT="${SAMPLE_COUNT}" \
TRAIN_BATCH_SIZE="${TRAIN_BATCH_SIZE}" \
ROLLOUT_N="${ROLLOUT_N}" \
ROLLOUT_FREE_CACHE_ENGINE="${ROLLOUT_FREE_CACHE_ENGINE}" \
ROLLOUT_ENABLE_SLEEP_MODE="${ROLLOUT_ENABLE_SLEEP_MODE}" \
ROLLOUT_GPU_MEMORY_UTILIZATION="${ROLLOUT_GPU_MEMORY_UTILIZATION}" \
AGENT_NUM_WORKERS="${AGENT_NUM_WORKERS}" \
RAY_NUM_CPUS="${RAY_NUM_CPUS}" \
CUDA_VISIBLE_DEVICES_IN_CONTAINER="${CUDA_VISIBLE_DEVICES_IN_CONTAINER}" \
UENV_AGENT_LOOP_CLIENT=rust_core \
UENV_AGENT_LOOP_ENDPOINT="${CORE_ADDR}" \
UENV_ADAPTER_CORE_AUTO_START=0 \
UENV_AGENT_LOOP_BUILD_CORE=0 \
UENV_ADAPTER_CORE_BACKEND=server \
UENV_ROLLOUT_MODEL_ENDPOINT="${ROLLOUT_ENDPOINT}" \
UENV_ROLLOUT_MODEL_NAME="${MODEL_NAME}" \
UENV_AGENT_LOOP_RESULT_RECORD_PATH="${AGENT_LOOP_RESULT_RECORD_PATH}" \
PODMAN_NETWORK_ARGS="${PODMAN_NETWORK_ARGS}" \
RUN_ID="${RUN_ID}" \
  "${REPO_DIR}/scripts/run_verl_grpo_1step_with_uenv_agent_loop.sh"
run_status=$?
set -e

# 如果 VeRL 运行失败，先打印训练日志末尾的关键信息，再返回原始失败码。
VERL_LOG="${REPO_DIR}/logs/verl_grpo_${TRAINING_STEPS}step_agent_loop/${RUN_ID}.log"
if [ "${run_status}" -ne 0 ]; then
  echo "Layer 4 smoke test failed. VeRL log: ${VERL_LOG}" >&2
  tail -120 "${VERL_LOG}" >&2 2>/dev/null || true
  exit "${run_status}"
fi

# 从 VeRL 和 worker 日志中打印简短的成功摘要。
echo "Layer 4 smoke test completed."
echo "VeRL log: ${VERL_LOG}"
grep -E "Training Progress: 100%|critic/score/mean|critic/rewards/mean" "${VERL_LOG}" | tail -5 || true
echo "Worker dispatch evidence:"
grep -E "verl-agent-loop|dispatch_completed|reward=" "${WORKER_LOG}" | tail -20 || true
