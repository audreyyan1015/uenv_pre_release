#!/usr/bin/env bash
# Experimental UEnv SWE-bench-Pro evaluation with direct vLLM serving.
#
# This bypasses scripts/benchmark/common/run_model_gateway.py and sends the
# vLLM OpenAI-compatible /v1 endpoint directly to UEnv Worker/Agent.
# Gateway-side request logging and reasoning stripping are intentionally not
# used in this experiment. Thinking is enabled by the evaluator request payload.

set -xeuo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO_DIR=${REPO_DIR:-$(cd "${SCRIPT_DIR}/../../../.." && pwd)}

# ---- user-adjustable benchmark config ----
IMAGE=${IMAGE:-localhost/uenv-bridge-verl:layer4-build}
DATA_PATH=${DATA_PATH:-${REPO_DIR}/data/benchmarks/swebenchpro/test.jsonl}
OUTPUT_DIR=${OUTPUT_DIR:-${REPO_DIR}/temp/benchmarks/swebenchpro/experimental/direct_vllm_dp}
RUN_ID=${RUN_ID:-${UENV_TRAINING_RUN_ID:-swebenchpro-direct-vllm-dp-$(date +%Y%m%d_%H%M%S)}}

UENV_ADAPTER_CORE_ENDPOINT=${UENV_ADAPTER_CORE_ENDPOINT:-8.130.75.157:8088}
UENV_ROLLOUT_MODEL_ENDPOINT=${UENV_ROLLOUT_MODEL_ENDPOINT:-${MODEL_ENDPOINT:-}}
UENV_ROLLOUT_MODEL_NAME=${UENV_ROLLOUT_MODEL_NAME:-Qwen/Qwen3.6-35B-A3B}
UENV_OBS_URL=${UENV_OBS_URL:-}
UENV_OBS_TOKEN=${UENV_OBS_TOKEN:-}

LIMIT=${LIMIT:-}
INSTANCE_ID=${INSTANCE_ID:-}
BATCH_SIZE=${BATCH_SIZE:-4}
MAX_TOKENS=${MAX_TOKENS:-8192}
ENABLE_THINKING=${ENABLE_THINKING:-1}
THINKING_TOKEN_BUDGET=${THINKING_TOKEN_BUDGET:-4096}
TEMPERATURE=${TEMPERATURE:-0.0}
TOP_P=${TOP_P:-1.0}
TIMEOUT_SECONDS=${TIMEOUT_SECONDS:-7200}
CLIENT_TIMEOUT_SECONDS=${CLIENT_TIMEOUT_SECONDS:-7600}
RESUME=${RESUME:-0}

BENCHMARK_VARIANT=${BENCHMARK_VARIANT:-pro}
COMMAND_MODE=${COMMAND_MODE:-full_shell}
ENV_PACKAGE_ID=${ENV_PACKAGE_ID:-swe-bench-pro}
ENV_PACKAGE_VERSION=${ENV_PACKAGE_VERSION:-0.3.4}
AGENT_BRIDGE_ID=${AGENT_BRIDGE_ID:-uenv-agent-openhands}
AGENT_BRIDGE_VERSION=${AGENT_BRIDGE_VERSION:-1.0.0}
AGENT_POOL_ID=${AGENT_POOL_ID:-openhands-default}
DRIVER_ENTRYPOINT=${DRIVER_ENTRYPOINT:-run_swebenchpro_official.py}
WORKSPACE_DIR=${WORKSPACE_DIR:-/app}
LLM_CONFIG_PATH=${LLM_CONFIG_PATH:-/root/UEnv/config/openhands-llm-qwen3-thinking-max-token-8192.json}
MAX_ITERATIONS=${MAX_ITERATIONS:-60}
AGENT_MODE=${AGENT_MODE:-llm}
POOL_SELECTOR_JSON=${POOL_SELECTOR_JSON:-}
# ---- end benchmark config ----

# ---- direct vLLM serving config ----
VLLM_IMAGE=${VLLM_IMAGE:-localhost/vllm-openai:v0.19.0-cu130}
VLLM_PORT=${VLLM_PORT:-18199}
VLLM_PUBLIC_HOST=${VLLM_PUBLIC_HOST:-127.0.0.1}
VLLM_PUBLIC_URL=${VLLM_PUBLIC_URL:-http://${VLLM_PUBLIC_HOST}:${VLLM_PORT}/v1}
VLLM_CONTAINER_NAME=${VLLM_CONTAINER_NAME:-uenv-swebenchpro-direct-vllm-dp-${VLLM_PORT}}
VLLM_PODMAN_GPU_ARGS=${VLLM_PODMAN_GPU_ARGS:-nvidia.com/gpu=all}
VLLM_SHM_SIZE=${VLLM_SHM_SIZE:-64g}
MAX_MODEL_LEN=${MAX_MODEL_LEN:-131072}
GPU_MEMORY_UTILIZATION=${GPU_MEMORY_UTILIZATION:-0.95}
TENSOR_PARALLEL_SIZE=${TENSOR_PARALLEL_SIZE:-4}
VLLM_DATA_PARALLEL_SIZE=${VLLM_DATA_PARALLEL_SIZE:-2}
VLLM_DATA_PARALLEL_BACKEND=${VLLM_DATA_PARALLEL_BACKEND:-mp}
VLLM_DATA_PARALLEL_SIZE_LOCAL=${VLLM_DATA_PARALLEL_SIZE_LOCAL:-}
VLLM_DATA_PARALLEL_RPC_PORT=${VLLM_DATA_PARALLEL_RPC_PORT:-}
VLLM_DATA_PARALLEL_ADDRESS=${VLLM_DATA_PARALLEL_ADDRESS:-}
VLLM_DATA_PARALLEL_HYBRID_LB=${VLLM_DATA_PARALLEL_HYBRID_LB:-0}
VLLM_DATA_PARALLEL_EXTERNAL_LB=${VLLM_DATA_PARALLEL_EXTERNAL_LB:-0}
VLLM_MAX_NUM_SEQS=${VLLM_MAX_NUM_SEQS:-}
VLLM_MAX_NUM_BATCHED_TOKENS=${VLLM_MAX_NUM_BATCHED_TOKENS:-131072}
ENABLE_VLLM_REASONING=${ENABLE_VLLM_REASONING:-1}
VLLM_REASONING_PARSER=${VLLM_REASONING_PARSER:-qwen3}
VLLM_REASONING_CONFIG=${VLLM_REASONING_CONFIG:-}
ENABLE_AUTO_TOOL_CHOICE=${ENABLE_AUTO_TOOL_CHOICE:-1}
TOOL_CALL_PARSER=${TOOL_CALL_PARSER:-qwen3_xml}
PODMAN_EXTRA_ARGS=${PODMAN_EXTRA_ARGS:-}
KEEP_SERVE=${KEEP_SERVE:-0}
START_ONLY=${START_ONLY:-0}
START_ONLY_SLEEP_SECONDS=${START_ONLY_SLEEP_SECONDS:-0}
# ---- end direct vLLM serving config ----

source "${REPO_DIR}/scripts/benchmark/common/checkpoint_model.sh"

uenv_experimental_resolve_model_dir() {
  local checkpoint_dir=""
  local model_dir=""

  if [ -n "${MODEL_DIR:-}" ]; then
    model_dir="${MODEL_DIR}"
  elif [ -n "${CHECKPOINT_DIR:-}" ]; then
    checkpoint_dir="$(uenv_benchmark_resolve_actor_checkpoint_dir "${CHECKPOINT_DIR}")"
    if uenv_benchmark_has_hf_weights "${checkpoint_dir}"; then
      model_dir="${checkpoint_dir}"
    else
      HF_DIR="${HF_DIR:-${checkpoint_dir}/huggingface}"
      uenv_benchmark_merge_checkpoint "${checkpoint_dir}" "${HF_DIR}"
      uenv_benchmark_ensure_hf_metadata "${HF_DIR}"
      model_dir="${HF_DIR}"
    fi
    export CHECKPOINT_DIR="${checkpoint_dir}"
    export HF_DIR="${model_dir}"
  elif [ -n "${HF_DIR:-}" ]; then
    model_dir="${HF_DIR}"
  else
    return 1
  fi

  if ! uenv_benchmark_has_hf_weights "${model_dir}"; then
    printf 'No HuggingFace weights found in model dir: %s\n' "${model_dir}" >&2
    return 2
  fi

  printf '%s\n' "${model_dir}"
}

uenv_experimental_start_direct_vllm() {
  local model_dir="$1"
  local output_dir="$2"
  local upstream_url="http://127.0.0.1:${VLLM_PORT}/v1"
  local vllm_gpu_args=()
  local vllm_reasoning_args=()
  local vllm_tool_args=()
  local vllm_extra_args=()
  local vllm_dp_args=()

  mkdir -p "${output_dir}"
  export UENV_ROLLOUT_MODEL_ENDPOINT="${VLLM_PUBLIC_URL}"
  export MODEL_ENDPOINT="${VLLM_PUBLIC_URL}"
  export UENV_ROLLOUT_MODEL_NAME="${UENV_ROLLOUT_MODEL_NAME}"

  if [ -n "${VLLM_PODMAN_GPU_ARGS}" ]; then
    vllm_gpu_args=(--device "${VLLM_PODMAN_GPU_ARGS}")
  fi
  if [ "${ENABLE_VLLM_REASONING}" = "1" ]; then
    vllm_reasoning_args=(--reasoning-parser "${VLLM_REASONING_PARSER}")
    if [ -n "${VLLM_REASONING_CONFIG}" ]; then
      vllm_reasoning_args+=(--reasoning-config "${VLLM_REASONING_CONFIG}")
    else
      vllm_reasoning_args+=(--reasoning-config '{"reasoning_start_str":"<think>","reasoning_end_str":"</think>"}')
    fi
  fi
  if [ "${ENABLE_AUTO_TOOL_CHOICE}" = "1" ]; then
    vllm_tool_args=(--enable-auto-tool-choice --tool-call-parser "${TOOL_CALL_PARSER}")
  fi
  if [ -n "${VLLM_MAX_NUM_SEQS}" ]; then
    vllm_extra_args+=(--max-num-seqs "${VLLM_MAX_NUM_SEQS}")
  fi
  if [ -n "${VLLM_MAX_NUM_BATCHED_TOKENS}" ]; then
    vllm_extra_args+=(--max-num-batched-tokens "${VLLM_MAX_NUM_BATCHED_TOKENS}")
  fi

  vllm_dp_args=(--data-parallel-size "${VLLM_DATA_PARALLEL_SIZE}")
  if [ -n "${VLLM_DATA_PARALLEL_BACKEND}" ]; then
    vllm_dp_args+=(--data-parallel-backend "${VLLM_DATA_PARALLEL_BACKEND}")
  fi
  if [ -n "${VLLM_DATA_PARALLEL_SIZE_LOCAL}" ]; then
    vllm_dp_args+=(--data-parallel-size-local "${VLLM_DATA_PARALLEL_SIZE_LOCAL}")
  fi
  if [ -n "${VLLM_DATA_PARALLEL_RPC_PORT}" ]; then
    vllm_dp_args+=(--data-parallel-rpc-port "${VLLM_DATA_PARALLEL_RPC_PORT}")
  fi
  if [ -n "${VLLM_DATA_PARALLEL_ADDRESS}" ]; then
    vllm_dp_args+=(--data-parallel-address "${VLLM_DATA_PARALLEL_ADDRESS}")
  fi
  if [ "${VLLM_DATA_PARALLEL_HYBRID_LB}" = "1" ]; then
    vllm_dp_args+=(--data-parallel-hybrid-lb)
  fi
  if [ "${VLLM_DATA_PARALLEL_EXTERNAL_LB}" = "1" ]; then
    vllm_dp_args+=(--data-parallel-external-lb)
  fi

  uenv_benchmark_model_log "Starting direct vLLM container ${VLLM_CONTAINER_NAME}; endpoint=${VLLM_PUBLIC_URL}; tp=${TENSOR_PARALLEL_SIZE}; dp=${VLLM_DATA_PARALLEL_SIZE}"
  podman rm -f "${VLLM_CONTAINER_NAME}" >/dev/null 2>&1 || true
  podman run -d --name "${VLLM_CONTAINER_NAME}" \
    --entrypoint python3 \
    --network host \
    --pids-limit=-1 \
    --shm-size="${VLLM_SHM_SIZE}" \
    "${vllm_gpu_args[@]}" \
    -v /data/ronghao:/data/ronghao \
    -w "${REPO_DIR}" \
    -e PYTHONPATH="${REPO_DIR}/src" \
    -e UENV_PATCH_VERL_TEXT_ONLY_PROCESSOR=1 \
    "${VLLM_IMAGE}" \
    -m vllm.entrypoints.openai.api_server \
    --model "${model_dir}" \
    --served-model-name "${UENV_ROLLOUT_MODEL_NAME}" \
    --host 0.0.0.0 \
    --port "${VLLM_PORT}" \
    --tensor-parallel-size "${TENSOR_PARALLEL_SIZE}" \
    "${vllm_dp_args[@]}" \
    --max-model-len "${MAX_MODEL_LEN}" \
    --gpu-memory-utilization "${GPU_MEMORY_UTILIZATION}" \
    "${vllm_reasoning_args[@]}" \
    "${vllm_tool_args[@]}" \
    "${vllm_extra_args[@]}" \
    --trust-remote-code

  UENV_BENCHMARK_MODEL_VLLM_CONTAINER="${VLLM_CONTAINER_NAME}"
  UENV_BENCHMARK_MODEL_VLLM_STARTED=1
  uenv_benchmark_register_model_cleanup
  if ! uenv_benchmark_wait_for_http "${upstream_url}/models" "direct vLLM" "${VLLM_READY_ATTEMPTS:-120}" "${VLLM_READY_SLEEP_SECONDS:-5}"; then
    podman logs --tail 200 "${VLLM_CONTAINER_NAME}" >&2 || true
    return 1
  fi
}

########################### model endpoint ###########################
if [ -z "${UENV_ROLLOUT_MODEL_ENDPOINT}" ]; then
  if ! MODEL_DIR_RESOLVED="$(uenv_experimental_resolve_model_dir)"; then
    echo "UENV_ROLLOUT_MODEL_ENDPOINT is required, or set CHECKPOINT_DIR / HF_DIR / MODEL_DIR to start direct vLLM." >&2
    exit 2
  fi
  uenv_experimental_start_direct_vllm "${MODEL_DIR_RESOLVED}" "${OUTPUT_DIR}"
else
  uenv_benchmark_model_log "Using existing direct model endpoint: ${UENV_ROLLOUT_MODEL_ENDPOINT}"
fi

if [ -z "${UENV_ROLLOUT_MODEL_ENDPOINT}" ]; then
  echo "UENV_ROLLOUT_MODEL_ENDPOINT is required, or set CHECKPOINT_DIR / HF_DIR / MODEL_DIR to start direct vLLM." >&2
  exit 2
fi

if [ "${START_ONLY}" = "1" ]; then
  uenv_benchmark_model_log "START_ONLY=1, direct vLLM endpoint is ready: ${UENV_ROLLOUT_MODEL_ENDPOINT}"
  if [ "${START_ONLY_SLEEP_SECONDS}" != "0" ]; then
    sleep "${START_ONLY_SLEEP_SECONDS}"
  fi
  exit 0
fi

mkdir -p "${OUTPUT_DIR}"

########################### parameter arrays ###########################
EVAL_ARGS=(
  --data "${DATA_PATH}"
  --output-dir "${OUTPUT_DIR}"
  --endpoint "${UENV_ADAPTER_CORE_ENDPOINT}"
  --model-endpoint "${UENV_ROLLOUT_MODEL_ENDPOINT}"
  --model-name "${UENV_ROLLOUT_MODEL_NAME}"
  --batch-size "${BATCH_SIZE}"
  --max-tokens "${MAX_TOKENS}"
  --thinking-token-budget "${THINKING_TOKEN_BUDGET}"
  --temperature "${TEMPERATURE}"
  --top-p "${TOP_P}"
  --timeout-seconds "${TIMEOUT_SECONDS}"
  --client-timeout-seconds "${CLIENT_TIMEOUT_SECONDS}"
)
if [ "${ENABLE_THINKING}" = "1" ]; then
  EVAL_ARGS+=(--enable-thinking)
fi

AGENT=(
  --benchmark-variant "${BENCHMARK_VARIANT}"
  --command-mode "${COMMAND_MODE}"
  --env-package-id "${ENV_PACKAGE_ID}"
  --env-package-version "${ENV_PACKAGE_VERSION}"
  --agent-bridge-id "${AGENT_BRIDGE_ID}"
  --agent-bridge-version "${AGENT_BRIDGE_VERSION}"
  --agent-pool-id "${AGENT_POOL_ID}"
  --driver-entrypoint "${DRIVER_ENTRYPOINT}"
  --workspace-dir "${WORKSPACE_DIR}"
  --llm-config-path "${LLM_CONFIG_PATH}"
  --max-iterations "${MAX_ITERATIONS}"
  --agent-mode "${AGENT_MODE}"
)

EXTRA=()
if [ -n "${LIMIT}" ]; then
  EXTRA+=(--limit "${LIMIT}")
fi
if [ -n "${INSTANCE_ID}" ]; then
  EXTRA+=(--instance-id "${INSTANCE_ID}")
fi
if [ -n "${POOL_SELECTOR_JSON}" ]; then
  EXTRA+=(--pool-selector-json "${POOL_SELECTOR_JSON}")
fi
if [ -n "${RUN_ID}" ]; then
  EXTRA+=(--run-id "${RUN_ID}")
fi
if [ -n "${UENV_OBS_URL}" ]; then
  EXTRA+=(--obs-url "${UENV_OBS_URL}")
fi
if [ -n "${UENV_OBS_TOKEN}" ]; then
  EXTRA+=(--obs-token "${UENV_OBS_TOKEN}")
fi
if [ "${RESUME}" = "1" ]; then
  EXTRA+=(--resume)
fi

PODMAN_ARGS=(
  --rm
  --entrypoint python3
  --network host
  --pids-limit=-1
  --shm-size=16g
  -v /data/ronghao:/data/ronghao
  -w "${REPO_DIR}"
  -e PYTHONPATH=src
  -e PROTOCOL_BUFFERS_PYTHON_IMPLEMENTATION=python
)

########################### launch ###########################
podman run \
  "${PODMAN_ARGS[@]}" \
  ${PODMAN_EXTRA_ARGS} \
  "${IMAGE}" \
  scripts/benchmark/swe/evaluate_swebenchpro_uenv.py \
  "${EVAL_ARGS[@]}" \
  "${AGENT[@]}" \
  "${EXTRA[@]}" \
  "$@"
