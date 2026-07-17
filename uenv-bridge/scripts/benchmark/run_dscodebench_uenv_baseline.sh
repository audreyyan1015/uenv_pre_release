#!/usr/bin/env bash
set -euo pipefail

IMAGE=${IMAGE:-localhost/uenv-bridge-verl:layer4-build}
REPO_DIR=${REPO_DIR:-/data/ronghao/uenv/uenv-bridge}
DATA_FILE=${DATA_FILE:-${REPO_DIR}/data/benchmarks/dscodebench/DSCodeBench.json}
OUTPUT_DIR=${OUTPUT_DIR:-${REPO_DIR}/temp/benchmarks/dscodebench/qwen3_6_35b_a3b_uenv_thinking_max32768_budget16384_full_$(date +%Y%m%d_%H%M%S)}
UENV_ADAPTER_CORE_ENDPOINT=${UENV_ADAPTER_CORE_ENDPOINT:-8.130.75.157:8088}
UENV_ROLLOUT_MODEL_ENDPOINT=${UENV_ROLLOUT_MODEL_ENDPOINT:-}
UENV_ROLLOUT_MODEL_NAME=${UENV_ROLLOUT_MODEL_NAME:-Qwen/Qwen3.6-35B-A3B}
LIMIT=${LIMIT:-}
LIBRARY=${LIBRARY:-}
MAX_PER_LIBRARY=${MAX_PER_LIBRARY:-}
BATCH_SIZE=${BATCH_SIZE:-1}
PROMPT_STYLE=${PROMPT_STYLE:-official_fenced}
MAX_TOKENS=${MAX_TOKENS:-32768}
ENABLE_THINKING=${ENABLE_THINKING:-1}
PRESERVE_THINKING=${PRESERVE_THINKING:-0}
THINKING_TOKEN_BUDGET=${THINKING_TOKEN_BUDGET:-16384}
TEMPERATURE=${TEMPERATURE:-0.2}
TOP_P=${TOP_P:-1.0}
TEST_CASE_NUMBER=${TEST_CASE_NUMBER:-200}
TIMEOUT_SECONDS=${TIMEOUT_SECONDS:-7200}
CODE_TIMEOUT_SECS=${CODE_TIMEOUT_SECS:-300}
CLIENT_TIMEOUT_SECONDS=${CLIENT_TIMEOUT_SECONDS:-7800}
EVALUATION_MODE=${EVALUATION_MODE:-inline_harness}
RESUME=${RESUME:-0}
PODMAN_GPU_ARGS=${PODMAN_GPU_ARGS:-}
PODMAN_EXTRA_ARGS=${PODMAN_EXTRA_ARGS:-}

if [ -z "$UENV_ROLLOUT_MODEL_ENDPOINT" ]; then
  echo "UENV_ROLLOUT_MODEL_ENDPOINT is required, for example http://10.10.20.142:18094/v1" >&2
  exit 2
fi

mkdir -p "$OUTPUT_DIR"

GPU_ARGS=()
if [ -n "$PODMAN_GPU_ARGS" ]; then
  GPU_ARGS+=(--device "$PODMAN_GPU_ARGS")
fi

ARGS=(
  --data "$DATA_FILE"
  --output-dir "$OUTPUT_DIR"
  --endpoint "$UENV_ADAPTER_CORE_ENDPOINT"
  --model-endpoint "$UENV_ROLLOUT_MODEL_ENDPOINT"
  --model-name "$UENV_ROLLOUT_MODEL_NAME"
  --batch-size "$BATCH_SIZE"
  --prompt-style "$PROMPT_STYLE"
  --max-tokens "$MAX_TOKENS"
  --temperature "$TEMPERATURE"
  --top-p "$TOP_P"
  --test-case-number "$TEST_CASE_NUMBER"
  --timeout-seconds "$TIMEOUT_SECONDS"
  --code-timeout-secs "$CODE_TIMEOUT_SECS"
  --client-timeout-seconds "$CLIENT_TIMEOUT_SECONDS"
  --evaluation-mode "$EVALUATION_MODE"
)
if [ -n "$LIMIT" ]; then
  ARGS+=(--limit "$LIMIT")
fi
if [ -n "$LIBRARY" ]; then
  ARGS+=(--library "$LIBRARY")
fi
if [ -n "$MAX_PER_LIBRARY" ]; then
  ARGS+=(--max-per-library "$MAX_PER_LIBRARY")
fi
if [ "$ENABLE_THINKING" = "1" ]; then
  ARGS+=(--enable-thinking)
fi
if [ "$PRESERVE_THINKING" = "1" ]; then
  ARGS+=(--preserve-thinking)
fi
if [ -n "$THINKING_TOKEN_BUDGET" ]; then
  ARGS+=(--thinking-token-budget "$THINKING_TOKEN_BUDGET")
fi
if [ "$RESUME" = "1" ]; then
  ARGS+=(--resume)
fi

podman run --rm \
  --entrypoint bash \
  --network host \
  --pids-limit=-1 \
  --shm-size=32g \
  "${GPU_ARGS[@]}" \
  -v /data/ronghao:/data/ronghao \
  -w "$REPO_DIR" \
  -e PYTHONPATH=src \
  -e PROTOCOL_BUFFERS_PYTHON_IMPLEMENTATION=python \
  ${PODMAN_EXTRA_ARGS} \
  "$IMAGE" \
  -lc "python3 scripts/benchmark/evaluate_dscodebench_uenv.py ${ARGS[*]@Q}"
