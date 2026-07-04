#!/usr/bin/env bash
set -euo pipefail

# 打印脚本用途、跨主机角色分配以及常用运行参数。
usage() {
  cat <<'EOF'
Run the distributed Layer 4 pre-rollout smoke test for the shared test hosts.

This script is intended to run on the adapter login host. It starts:
  1. optional mock OpenAI-compatible model endpoint on the adapter host
  2. real VeRL trainer container with UEnvAgentLoop enabled

It does not start Rust adapter core, uenv-server, uenv-worker, or hub.
For this distributed integration shape, Rust adapter core is owned and started
by the server side. This script only connects VeRL/Python to that server-side
adapter core endpoint.

Usage:
  SERVER_ADAPTER_CORE_ENDPOINT=<server-core-host:port> ./scripts/run_layer4_distributed_smoke.sh

Common environment overrides:
  IMAGE                         VeRL image. Default: localhost/uenv-bridge-verl:layer4-build
  TRAINING_STEPS                Default: 1
  SAMPLE_COUNT                  Default: 2
  TRAIN_BATCH_SIZE              Default: 2
  ROLLOUT_N                     Default: 2
  DATA_MAX_RESPONSE_LENGTH      Default: 256; GSM8K answers need enough room for final #### answer.
  SERVER_ADAPTER_CORE_ENDPOINT  Server-side Rust adapter core gRPC endpoint. Default: 8.130.89.198:50053
  MODEL_NAME                    Default: mock-policy
  START_MOCK_MODEL              Start adapter-host mock model endpoint. Default: 1
  UENV_ROLLOUT_MODEL_ENDPOINT   Required only when START_MOCK_MODEL=0
  AGENT_LOOP_REQUEST_RECORD_PATH Request JSONL path inside the container.
  LOG_ROOT                      Directory for run logs. Default: <repo>/logs
  KEEP_SERVICES                 Do not stop local mock model after run. Default: 0

Example:
  SERVER_ADAPTER_CORE_ENDPOINT=8.130.89.198:50053 \
  IMAGE=localhost/uenv-bridge-verl:layer4-build \
  ./scripts/run_layer4_distributed_smoke.sh
EOF
}

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
  usage
  exit 0
fi

# 解析仓库路径。REPO_DIR 指向 uenv-bridge，WORKSPACE_ROOT 指向上一级
# uenv 工作区。
REPO_DIR=${REPO_DIR:-"$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"}
WORKSPACE_ROOT=${WORKSPACE_ROOT:-"$(cd "${REPO_DIR}/.." && pwd)"}

# 配置 server 侧已经启动的 Rust adapter core 地址。Python/VeRL 只连接
# 这个 endpoint，不在 adapter 侧启动 core。
SERVER_ADAPTER_CORE_ENDPOINT=${SERVER_ADAPTER_CORE_ENDPOINT:-${UENV_AGENT_LOOP_ENDPOINT:-8.130.75.157:8088}}
if [ -z "${SERVER_ADAPTER_CORE_ENDPOINT}" ]; then
  echo "SERVER_ADAPTER_CORE_ENDPOINT is required." >&2
  exit 1
fi

# 配置将在容器内运行的 VeRL 训练任务参数。
IMAGE=${IMAGE:-localhost/uenv-bridge-verl:layer4-build}
TRAINING_STEPS=${TRAINING_STEPS:-1}
SAMPLE_COUNT=${SAMPLE_COUNT:-2}
TRAIN_BATCH_SIZE=${TRAIN_BATCH_SIZE:-2}
ROLLOUT_N=${ROLLOUT_N:-2}
DATA_MAX_RESPONSE_LENGTH=${DATA_MAX_RESPONSE_LENGTH:-256}
ROLLOUT_FREE_CACHE_ENGINE=${ROLLOUT_FREE_CACHE_ENGINE:-False}
ROLLOUT_ENABLE_SLEEP_MODE=${ROLLOUT_ENABLE_SLEEP_MODE:-False}
ROLLOUT_GPU_MEMORY_UTILIZATION=${ROLLOUT_GPU_MEMORY_UTILIZATION:-0.25}
AGENT_NUM_WORKERS=${AGENT_NUM_WORKERS:-1}
RAY_NUM_CPUS=${RAY_NUM_CPUS:-4}
CUDA_VISIBLE_DEVICES_IN_CONTAINER=${CUDA_VISIBLE_DEVICES_IN_CONTAINER:-7}

# 配置模型 endpoint。默认在 adapter login host 起一个 mock endpoint；
# worker 能访问到该地址时，server/worker 侧可以使用它完成 smoke test。
# 若使用真实模型服务，设置 START_MOCK_MODEL=0 并传入
# UENV_ROLLOUT_MODEL_ENDPOINT。
DEFAULT_ADAPTER_ADVERTISE_HOST=$(hostname -I 2>/dev/null | awk '{print $1}')
ADAPTER_ADVERTISE_HOST=${ADAPTER_ADVERTISE_HOST:-${DEFAULT_ADAPTER_ADVERTISE_HOST:-127.0.0.1}}
MODEL_NAME=${MODEL_NAME:-mock-policy}
START_MOCK_MODEL=${START_MOCK_MODEL:-1}
MOCK_MODEL_BIND=${MOCK_MODEL_BIND:-0.0.0.0:18080}
MOCK_MODEL_LOCAL_CHECK_ADDR=${MOCK_MODEL_LOCAL_CHECK_ADDR:-127.0.0.1:18080}
MOCK_MODEL_PUBLIC_ENDPOINT=${MOCK_MODEL_PUBLIC_ENDPOINT:-http://${ADAPTER_ADVERTISE_HOST}:18080/v1}
if [ "${START_MOCK_MODEL}" = "1" ]; then
  ROLLOUT_ENDPOINT="${MOCK_MODEL_PUBLIC_ENDPOINT}"
else
  ROLLOUT_ENDPOINT=${UENV_ROLLOUT_MODEL_ENDPOINT:-}
  if [ -z "${ROLLOUT_ENDPOINT}" ]; then
    echo "UENV_ROLLOUT_MODEL_ENDPOINT is required when START_MOCK_MODEL=0" >&2
    exit 1
  fi
fi

# 配置可选的服务保留行为以及容器网络。Layer4 容器默认使用 host network，
# 这样它可以访问 server 侧 adapter core endpoint。
KEEP_SERVICES=${KEEP_SERVICES:-0}
PODMAN_NETWORK_ARGS=${PODMAN_NETWORK_ARGS:---network host}

# 为本次运行创建独立目录，用于保存 mock model 和 VeRL 日志。
RUN_ID=${RUN_ID:-layer4_distributed_$(date +%Y%m%d_%H%M%S)}
LOG_ROOT=${LOG_ROOT:-${REPO_DIR}/logs}
SERVICE_DIR=${SERVICE_DIR:-${LOG_ROOT}/layer4_distributed/${RUN_ID}}
CONTAINER_SERVICE_DIR=/tmp/uenv-bridge/logs/layer4_distributed/${RUN_ID}
MOCK_MODEL_LOG=${MOCK_MODEL_LOG:-${SERVICE_DIR}/mock-model.log}
AGENT_LOOP_REQUEST_RECORD_PATH=${AGENT_LOOP_REQUEST_RECORD_PATH:-${CONTAINER_SERVICE_DIR}/agent-loop-requests.jsonl}
AGENT_LOOP_RESULT_RECORD_PATH=${AGENT_LOOP_RESULT_RECORD_PATH:-${CONTAINER_SERVICE_DIR}/agent-loop-results.jsonl}
mkdir -p "${SERVICE_DIR}"

# 记录本脚本启动的本地服务进程 id，退出时统一清理。
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
  python3 - "$host" "$port" >/dev/null 2>&1 <<'PYNET'
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
PYNET
}

# 如果服务地址已经被占用，则提前失败。
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

# 等待服务开始监听端口。
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

# 除非设置 KEEP_SERVICES=1 用于调试，否则退出时停止本脚本启动的
# mock model。
cleanup() {
  local status=$?
  if [ "${KEEP_SERVICES}" = "1" ]; then
    echo "KEEP_SERVICES=1; leaving local services and logs under ${SERVICE_DIR}"
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


# 先检查 server 侧 adapter core endpoint 是否已可连接。这里不启动 core。
wait_for_addr "server-side adapter core" "${SERVER_ADAPTER_CORE_ENDPOINT}" 20

# 按需启动一个 adapter host 上的 mock OpenAI-compatible model endpoint。
if [ "${START_MOCK_MODEL}" = "1" ]; then
  require_free_addr "mock model" "${MOCK_MODEL_LOCAL_CHECK_ADDR}"
  echo "Starting mock OpenAI-compatible model endpoint on ${MOCK_MODEL_BIND}"
  LAYER4_MODEL_ADDR="${MOCK_MODEL_BIND}" LAYER4_MODEL_NAME="${MODEL_NAME}" \
    python3 -u - >"${MOCK_MODEL_LOG}" 2>&1 <<'PYMODEL' &
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
PYMODEL
  PIDS+=("$!")
  wait_for_addr "mock model" "${MOCK_MODEL_LOCAL_CHECK_ADDR}" 20
fi

# 运行真实 VeRL GRPO smoke test。内部脚本会启动 VeRL 容器，启用
# UEnvAgentLoop，并将其指向 server 侧已经启动的 Rust adapter core。
echo "Running distributed VeRL Layer 4 smoke test; service logs: ${SERVICE_DIR}"
set +e
IMAGE="${IMAGE}" \
TRAINING_STEPS="${TRAINING_STEPS}" \
SAMPLE_COUNT="${SAMPLE_COUNT}" \
TRAIN_BATCH_SIZE="${TRAIN_BATCH_SIZE}" \
ROLLOUT_N="${ROLLOUT_N}" \
DATA_MAX_RESPONSE_LENGTH="${DATA_MAX_RESPONSE_LENGTH}" \
ROLLOUT_FREE_CACHE_ENGINE="${ROLLOUT_FREE_CACHE_ENGINE}" \
ROLLOUT_ENABLE_SLEEP_MODE="${ROLLOUT_ENABLE_SLEEP_MODE}" \
ROLLOUT_GPU_MEMORY_UTILIZATION="${ROLLOUT_GPU_MEMORY_UTILIZATION}" \
AGENT_NUM_WORKERS="${AGENT_NUM_WORKERS}" \
RAY_NUM_CPUS="${RAY_NUM_CPUS}" \
CUDA_VISIBLE_DEVICES_IN_CONTAINER="${CUDA_VISIBLE_DEVICES_IN_CONTAINER}" \
UENV_AGENT_LOOP_CLIENT=rust_core \
UENV_AGENT_LOOP_ENDPOINT="${SERVER_ADAPTER_CORE_ENDPOINT}" \
UENV_ADAPTER_CORE_AUTO_START=0 \
UENV_AGENT_LOOP_BUILD_CORE=0 \
UENV_ADAPTER_CORE_BACKEND=server \
UENV_ROLLOUT_MODEL_ENDPOINT="${ROLLOUT_ENDPOINT}" \
UENV_ROLLOUT_MODEL_NAME="${MODEL_NAME}" \
UENV_AGENT_LOOP_REQUEST_RECORD_PATH="${AGENT_LOOP_REQUEST_RECORD_PATH}" \
UENV_AGENT_LOOP_RESULT_RECORD_PATH="${AGENT_LOOP_RESULT_RECORD_PATH}" \
PODMAN_NETWORK_ARGS="${PODMAN_NETWORK_ARGS}" \
RUN_ID="${RUN_ID}" \
LOG_DIR="${LOG_ROOT}/verl_grpo_${TRAINING_STEPS}step_agent_loop" \
  "${REPO_DIR}/scripts/run_verl_grpo_1step_with_uenv_agent_loop.sh"
run_status=$?
set -e

# 如果 VeRL 运行失败，打印训练日志末尾。
VERL_LOG="${LOG_ROOT}/verl_grpo_${TRAINING_STEPS}step_agent_loop/${RUN_ID}.log"
if [ "${run_status}" -ne 0 ]; then
  echo "Distributed Layer 4 smoke test failed. VeRL log: ${VERL_LOG}" >&2
  tail -120 "${VERL_LOG}" >&2 2>/dev/null || true
  exit "${run_status}"
fi

# 从 VeRL 日志中打印简短的成功摘要。
echo "Distributed Layer 4 smoke test completed."
echo "VeRL log: ${VERL_LOG}"
grep -E "Training Progress: 100%|critic/score/mean|critic/rewards/mean" "${VERL_LOG}" | tail -5 || true
