#!/usr/bin/env bash
set -euo pipefail

IMAGE=${IMAGE:-localhost/uenv-bridge-verl:layer4-build}
REPO_DIR=${REPO_DIR:-/data/ronghao/uenv/uenv-bridge}
DATA_DIR=${DATA_DIR:-${REPO_DIR}/data/benchmarks/olymmath}
DATASETS=${DATASETS:-EN-EASY,EN-HARD,ZH-EASY,ZH-HARD}
OUTPUT_DIR=${OUTPUT_DIR:-${REPO_DIR}/temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_generate}
UENV_ADAPTER_CORE_ENDPOINT=${UENV_ADAPTER_CORE_ENDPOINT:-8.130.75.157:8088}
UENV_ROLLOUT_MODEL_ENDPOINT=${UENV_ROLLOUT_MODEL_ENDPOINT:-}
UENV_ROLLOUT_MODEL_NAME=${UENV_ROLLOUT_MODEL_NAME:-Qwen/Qwen3.6-35B-A3B}
LIMIT=${LIMIT:-}
BATCH_SIZE=${BATCH_SIZE:-1}
PROMPT_STYLE=${PROMPT_STYLE:-official_no_think}
MAX_TOKENS=${MAX_TOKENS:-2048}
ENABLE_THINKING=${ENABLE_THINKING:-0}
PRESERVE_THINKING=${PRESERVE_THINKING:-0}
THINKING_TOKEN_BUDGET=${THINKING_TOKEN_BUDGET:-}
TEMPERATURE=${TEMPERATURE:-0.0}
TOP_P=${TOP_P:-1.0}
TIMEOUT_SECONDS=${TIMEOUT_SECONDS:-1800}
CLIENT_TIMEOUT_SECONDS=${CLIENT_TIMEOUT_SECONDS:-2400}
RESUME=${RESUME:-0}
PODMAN_GPU_ARGS=${PODMAN_GPU_ARGS:-}
PODMAN_EXTRA_ARGS=${PODMAN_EXTRA_ARGS:-}

if [ -z "$UENV_ROLLOUT_MODEL_ENDPOINT" ]; then
  echo "UENV_ROLLOUT_MODEL_ENDPOINT is required, for example http://10.10.20.142:18088/v1" >&2
  exit 2
fi

mkdir -p "$OUTPUT_DIR"

GPU_ARGS=()
if [ -n "$PODMAN_GPU_ARGS" ]; then
  GPU_ARGS+=(--device "$PODMAN_GPU_ARGS")
fi

ARGS=(
  --data-dir "$DATA_DIR"
  --datasets "$DATASETS"
  --output-dir "$OUTPUT_DIR"
  --endpoint "$UENV_ADAPTER_CORE_ENDPOINT"
  --model-endpoint "$UENV_ROLLOUT_MODEL_ENDPOINT"
  --model-name "$UENV_ROLLOUT_MODEL_NAME"
  --batch-size "$BATCH_SIZE"
  --prompt-style "$PROMPT_STYLE"
  --max-tokens "$MAX_TOKENS"
  --temperature "$TEMPERATURE"
  --top-p "$TOP_P"
  --timeout-seconds "$TIMEOUT_SECONDS"
  --client-timeout-seconds "$CLIENT_TIMEOUT_SECONDS"
)
if [ -n "$LIMIT" ]; then
  ARGS+=(--limit "$LIMIT")
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
  --shm-size=16g \
  "${GPU_ARGS[@]}" \
  -v /data/ronghao:/data/ronghao \
  -w "$REPO_DIR" \
  -e PYTHONPATH=src \
  ${PODMAN_EXTRA_ARGS} \
  "$IMAGE" \
  -lc "python3 scripts/benchmark/evaluate_olymmath_uenv.py ${ARGS[*]@Q}"
