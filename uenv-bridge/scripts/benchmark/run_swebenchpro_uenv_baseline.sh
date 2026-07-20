#!/usr/bin/env bash
set -euo pipefail

IMAGE=${IMAGE:-localhost/uenv-bridge-verl:layer4-build}
REPO_DIR=${REPO_DIR:-/data/ronghao/uenv/uenv-bridge}
DATA_PATH=${DATA_PATH:-${REPO_DIR}/data/benchmarks/swebenchpro/test.jsonl}
OUTPUT_DIR=${OUTPUT_DIR:-${REPO_DIR}/temp/benchmarks/swebenchpro/qwen3_6_35b_a3b_uenv_agent_full}
UENV_ADAPTER_CORE_ENDPOINT=${UENV_ADAPTER_CORE_ENDPOINT:-8.130.75.157:8088}
UENV_ROLLOUT_MODEL_NAME=${UENV_ROLLOUT_MODEL_NAME:-Qwen/Qwen3.6-35B-A3B}
UENV_ROLLOUT_MODEL_ENDPOINT=${UENV_ROLLOUT_MODEL_ENDPOINT:-}
LIMIT=${LIMIT:-}
INSTANCE_ID=${INSTANCE_ID:-}
BATCH_SIZE=${BATCH_SIZE:-1}
MAX_TOKENS=${MAX_TOKENS:-8192}
THINKING_TOKEN_BUDGET=${THINKING_TOKEN_BUDGET:-4096}
TEMPERATURE=${TEMPERATURE:-0.0}
TOP_P=${TOP_P:-1.0}
TIMEOUT_SECONDS=${TIMEOUT_SECONDS:-7200}
CLIENT_TIMEOUT_SECONDS=${CLIENT_TIMEOUT_SECONDS:-7600}
BENCHMARK_VARIANT=${BENCHMARK_VARIANT:-pro}
COMMAND_MODE=${COMMAND_MODE:-full_shell}
ENV_PACKAGE_ID=${ENV_PACKAGE_ID:-swe-bench-pro}
ENV_PACKAGE_VERSION=${ENV_PACKAGE_VERSION:-0.3.4}
AGENT_BRIDGE_ID=${AGENT_BRIDGE_ID:-uenv-agent-openhands}
AGENT_BRIDGE_VERSION=${AGENT_BRIDGE_VERSION:-1.0.0}
AGENT_POOL_ID=${AGENT_POOL_ID:-openhands-default}
DRIVER_ENTRYPOINT=${DRIVER_ENTRYPOINT:-run_swebenchpro_official.py}
WORKSPACE_DIR=${WORKSPACE_DIR:-/app}
LLM_CONFIG_PATH=${LLM_CONFIG_PATH:-/root/UEnv/config/openhands-llm-uenv-gateway-max-token-8192.json}
MAX_ITERATIONS=${MAX_ITERATIONS:-50}
POOL_SELECTOR_JSON=${POOL_SELECTOR_JSON:-}
RESUME=${RESUME:-0}
PODMAN_GPU_ARGS=${PODMAN_GPU_ARGS:-}
PODMAN_EXTRA_ARGS=${PODMAN_EXTRA_ARGS:-}

if [ -z "$UENV_ROLLOUT_MODEL_ENDPOINT" ]; then
  echo "UENV_ROLLOUT_MODEL_ENDPOINT is required, for example http://10.10.20.142:18097/v1" >&2
  exit 2
fi

mkdir -p "$OUTPUT_DIR"

GPU_ARGS=()
if [ -n "$PODMAN_GPU_ARGS" ]; then
  GPU_ARGS+=(--device "$PODMAN_GPU_ARGS")
fi

ARGS=(
  --data "$DATA_PATH"
  --output-dir "$OUTPUT_DIR"
  --endpoint "$UENV_ADAPTER_CORE_ENDPOINT"
  --model-endpoint "$UENV_ROLLOUT_MODEL_ENDPOINT"
  --model-name "$UENV_ROLLOUT_MODEL_NAME"
  --batch-size "$BATCH_SIZE"
  --max-tokens "$MAX_TOKENS"
  --thinking-token-budget "$THINKING_TOKEN_BUDGET"
  --temperature "$TEMPERATURE"
  --top-p "$TOP_P"
  --timeout-seconds "$TIMEOUT_SECONDS"
  --client-timeout-seconds "$CLIENT_TIMEOUT_SECONDS"
  --benchmark-variant "$BENCHMARK_VARIANT"
  --command-mode "$COMMAND_MODE"
  --env-package-id "$ENV_PACKAGE_ID"
  --env-package-version "$ENV_PACKAGE_VERSION"
  --agent-bridge-id "$AGENT_BRIDGE_ID"
  --agent-bridge-version "$AGENT_BRIDGE_VERSION"
  --agent-pool-id "$AGENT_POOL_ID"
  --driver-entrypoint "$DRIVER_ENTRYPOINT"
  --workspace-dir "$WORKSPACE_DIR"
  --llm-config-path "$LLM_CONFIG_PATH"
  --max-iterations "$MAX_ITERATIONS"
)
if [ -n "$LIMIT" ]; then
  ARGS+=(--limit "$LIMIT")
fi
if [ -n "$INSTANCE_ID" ]; then
  ARGS+=(--instance-id "$INSTANCE_ID")
fi
if [ -n "$POOL_SELECTOR_JSON" ]; then
  ARGS+=(--pool-selector-json "$POOL_SELECTOR_JSON")
fi
if [ "$RESUME" = "1" ]; then
  ARGS+=(--resume)
fi

podman run --rm \
  --entrypoint bash \
  --network host \
  --pids-limit=-1 \
  --shm-size=16g \
  "${GPU_ARGS[@]}" \
  -v /data/ronghao:/data/ronghao \
  -w "$REPO_DIR" \
  -e PYTHONPATH=src \
  -e PROTOCOL_BUFFERS_PYTHON_IMPLEMENTATION=python \
  ${PODMAN_EXTRA_ARGS} \
  "$IMAGE" \
  -lc "python3 scripts/benchmark/evaluate_swebenchpro_uenv.py ${ARGS[*]@Q}"
