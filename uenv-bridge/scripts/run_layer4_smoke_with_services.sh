#!/usr/bin/env bash
set -euo pipefail

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

REPO_DIR=${REPO_DIR:-"$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"}
WORKSPACE_ROOT=${WORKSPACE_ROOT:-"$(cd "${REPO_DIR}/.." && pwd)"}

IMAGE=${IMAGE:-localhost/uenv-bridge-verl:latest}
TRAINING_STEPS=${TRAINING_STEPS:-1}
SAMPLE_COUNT=${SAMPLE_COUNT:-1}
TRAIN_BATCH_SIZE=${TRAIN_BATCH_SIZE:-1}
ROLLOUT_N=${ROLLOUT_N:-1}
ROLLOUT_FREE_CACHE_ENGINE=${ROLLOUT_FREE_CACHE_ENGINE:-False}
ROLLOUT_ENABLE_SLEEP_MODE=${ROLLOUT_ENABLE_SLEEP_MODE:-False}
AGENT_NUM_WORKERS=${AGENT_NUM_WORKERS:-1}
RAY_NUM_CPUS=${RAY_NUM_CPUS:-4}
CUDA_VISIBLE_DEVICES_IN_CONTAINER=${CUDA_VISIBLE_DEVICES_IN_CONTAINER:-0}

RUN_ID=${RUN_ID:-layer4_smoke_$(date +%Y%m%d_%H%M%S)}
CORE_ADDR=${CORE_ADDR:-127.0.0.1:50053}
WORKER_LISTEN=${WORKER_LISTEN:-127.0.0.1:50054}
WORKER_ID=${WORKER_ID:-layer4-worker}
MOCK_MODEL_ADDR=${MOCK_MODEL_ADDR:-127.0.0.1:18080}
MODEL_NAME=${MODEL_NAME:-mock-policy}
METRICS_LISTEN=${METRICS_LISTEN:-127.0.0.1:19091}
HEALTH_LISTEN=${HEALTH_LISTEN:-${METRICS_LISTEN}}

BUILD_RUST=${BUILD_RUST:-0}
START_MOCK_MODEL=${START_MOCK_MODEL:-1}
KEEP_SERVICES=${KEEP_SERVICES:-0}
PODMAN_NETWORK_ARGS=${PODMAN_NETWORK_ARGS:---network host}

ADAPTER_CORE_BIN=${ADAPTER_CORE_BIN:-${WORKSPACE_ROOT}/target/debug/uenv-adapter-core}
WORKER_BIN=${WORKER_BIN:-${WORKSPACE_ROOT}/target/debug/uenv-worker}
MATH_PLUGIN_BIN=${MATH_PLUGIN_BIN:-${WORKSPACE_ROOT}/target/debug/uenv-math-plugin}
PLUGIN_DIR=${PLUGIN_DIR:-${WORKSPACE_ROOT}/plugins}

SERVICE_DIR=${SERVICE_DIR:-${REPO_DIR}/tmp/layer4_smoke/${RUN_ID}}
WAL_DIR=${WAL_DIR:-${SERVICE_DIR}/wal}
MOCK_MODEL_LOG=${MOCK_MODEL_LOG:-${SERVICE_DIR}/mock-model.log}
CORE_LOG=${CORE_LOG:-${SERVICE_DIR}/adapter-core.log}
WORKER_STDOUT_LOG=${WORKER_STDOUT_LOG:-${SERVICE_DIR}/worker-stdout.log}
WORKER_LOG=${WORKER_LOG:-${SERVICE_DIR}/worker.log}
WORKERS_JSON=${WORKERS_JSON:-${SERVICE_DIR}/workers.json}
mkdir -p "${SERVICE_DIR}" "${WAL_DIR}"

PIDS=()

split_host() {
  local addr="$1"
  printf '%s\n' "${addr%:*}"
}

split_port() {
  local addr="$1"
  printf '%s\n' "${addr##*:}"
}

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

for path in "${ADAPTER_CORE_BIN}" "${WORKER_BIN}" "${MATH_PLUGIN_BIN}" "${PLUGIN_DIR}"; do
  if [ ! -e "${path}" ]; then
    echo "Required path does not exist: ${path}" >&2
    exit 1
  fi
done

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

echo "Starting adapter core on ${CORE_ADDR}"
UENV_ADDR="${CORE_ADDR}" \
UENV_ADAPTER_CORE_BACKEND=server \
  "${ADAPTER_CORE_BIN}" >"${CORE_LOG}" 2>&1 &
PIDS+=("$!")
wait_for_addr "adapter core" "${CORE_ADDR}" 20

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
  "${WORKER_BIN}" serve >"${WORKER_STDOUT_LOG}" 2>&1 &
PIDS+=("$!")
wait_for_addr "worker" "${WORKER_LISTEN}" 20

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

echo "Running VeRL Layer 4 smoke test; service logs: ${SERVICE_DIR}"
set +e
IMAGE="${IMAGE}" \
TRAINING_STEPS="${TRAINING_STEPS}" \
SAMPLE_COUNT="${SAMPLE_COUNT}" \
TRAIN_BATCH_SIZE="${TRAIN_BATCH_SIZE}" \
ROLLOUT_N="${ROLLOUT_N}" \
ROLLOUT_FREE_CACHE_ENGINE="${ROLLOUT_FREE_CACHE_ENGINE}" \
ROLLOUT_ENABLE_SLEEP_MODE="${ROLLOUT_ENABLE_SLEEP_MODE}" \
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
PODMAN_NETWORK_ARGS="${PODMAN_NETWORK_ARGS}" \
RUN_ID="${RUN_ID}" \
  "${REPO_DIR}/scripts/run_verl_grpo_1step_with_uenv_agent_loop.sh"
run_status=$?
set -e

VERL_LOG="${REPO_DIR}/tmp/verl_grpo_${TRAINING_STEPS}step_agent_loop_logs/${RUN_ID}.log"
if [ "${run_status}" -ne 0 ]; then
  echo "Layer 4 smoke test failed. VeRL log: ${VERL_LOG}" >&2
  tail -120 "${VERL_LOG}" >&2 2>/dev/null || true
  exit "${run_status}"
fi

echo "Layer 4 smoke test completed."
echo "VeRL log: ${VERL_LOG}"
grep -E "Training Progress: 100%|critic/score/mean|critic/rewards/mean" "${VERL_LOG}" | tail -5 || true
echo "Worker dispatch evidence:"
grep -E "verl-agent-loop|dispatch_completed|reward=" "${WORKER_LOG}" | tail -20 || true
