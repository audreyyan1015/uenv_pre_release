#!/usr/bin/env bash
# UEnv benchmark | SWE-bench-Pro | OpenHands agent | optional checkpoint serving

set -xeuo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO_DIR=${REPO_DIR:-$(cd "${SCRIPT_DIR}/../../.." && pwd)}

# ---- user-adjustable ----
IMAGE=${IMAGE:-localhost/uenv-bridge-verl:layer4-build}
DATA_PATH=${DATA_PATH:-${REPO_DIR}/data/benchmarks/swebenchpro/test.jsonl}
OUTPUT_DIR=${OUTPUT_DIR:-${REPO_DIR}/temp/benchmarks/swebenchpro/qwen3_6_35b_a3b_uenv_agent_full}
# Logical run id used to group benchmark episodes in Obs/frontend.
RUN_ID=${RUN_ID:-${UENV_TRAINING_RUN_ID:-}}

# UEnv routing:
# - AdapterCore receives EpisodeRequest batches and forwards them to Server/Worker.
# - The rollout model endpoint is an OpenAI-compatible /v1 URL used by remote agents.
# - Obs is optional progress/event reporting for the frontend.
UENV_ADAPTER_CORE_ENDPOINT=${UENV_ADAPTER_CORE_ENDPOINT:-8.130.75.157:8088}
UENV_ROLLOUT_MODEL_ENDPOINT=${UENV_ROLLOUT_MODEL_ENDPOINT:-}
UENV_ROLLOUT_MODEL_NAME=${UENV_ROLLOUT_MODEL_NAME:-Qwen/Qwen3.6-35B-A3B}
UENV_OBS_URL=${UENV_OBS_URL:-}
UENV_OBS_TOKEN=${UENV_OBS_TOKEN:-}

# Dataset slicing. INSTANCE_ID runs one SWE-bench-Pro instance; RESUME skips
# completed instances already present in OUTPUT_DIR.
LIMIT=${LIMIT:-}
INSTANCE_ID=${INSTANCE_ID:-}
BATCH_SIZE=${BATCH_SIZE:-1}
MAX_TOKENS=${MAX_TOKENS:-8192}
THINKING_TOKEN_BUDGET=${THINKING_TOKEN_BUDGET:-4096}
TEMPERATURE=${TEMPERATURE:-0.0}
TOP_P=${TOP_P:-1.0}
TIMEOUT_SECONDS=${TIMEOUT_SECONDS:-7200}
CLIENT_TIMEOUT_SECONDS=${CLIENT_TIMEOUT_SECONDS:-7600}
RESUME=${RESUME:-0}

# SWE Worker/Agent contract:
# - EnvPackage identifies the Worker-side runtime package and version.
# - Agent bridge/pool selects a registered OpenHands agent execution pool.
# - LLM_CONFIG_PATH is read on the agent host; model URL still comes from request payload.
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
MAX_ITERATIONS=${MAX_ITERATIONS:-50}
AGENT_MODE=${AGENT_MODE:-llm}
# Optional JSON selector for Worker/Agent pools when multiple pools are registered.
POOL_SELECTOR_JSON=${POOL_SELECTOR_JSON:-}

# Evaluator container and optional checkpoint serving. If CHECKPOINT_DIR, HF_DIR,
# or MODEL_DIR is set, common/checkpoint_model.sh starts local vLLM + gateway.
PODMAN_GPU_ARGS=${PODMAN_GPU_ARGS:-}
PODMAN_EXTRA_ARGS=${PODMAN_EXTRA_ARGS:-}
VLLM_PORT=${VLLM_PORT:-18081}
GATEWAY_PORT=${GATEWAY_PORT:-18094}
MAX_MODEL_LEN=${MAX_MODEL_LEN:-131072}
GPU_MEMORY_UTILIZATION=${GPU_MEMORY_UTILIZATION:-0.95}
ENABLE_AUTO_TOOL_CHOICE=${ENABLE_AUTO_TOOL_CHOICE:-1}
VLLM_MAX_NUM_SEQS=${VLLM_MAX_NUM_SEQS:-1}
VLLM_MAX_NUM_BATCHED_TOKENS=${VLLM_MAX_NUM_BATCHED_TOKENS:-131072}
GATEWAY_ENABLE_THINKING=${GATEWAY_ENABLE_THINKING:-1}
GATEWAY_STRIP_REASONING=${GATEWAY_STRIP_REASONING:-1}
GATEWAY_THINKING_BUDGET=${GATEWAY_THINKING_BUDGET:-${THINKING_TOKEN_BUDGET}}
# ---- end user-adjustable ----

########################### model endpoint ###########################
if [ -n "${CHECKPOINT_DIR:-}${HF_DIR:-}${MODEL_DIR:-}" ]; then
    # Override this when remote agents cannot reach the local loopback address.
    MODEL_GATEWAY_PUBLIC_URL=${MODEL_GATEWAY_PUBLIC_URL:-http://127.0.0.1:18194/v1}
fi

source "${REPO_DIR}/scripts/benchmark/common/checkpoint_model.sh"
uenv_benchmark_prepare_model_endpoint "${OUTPUT_DIR}"

if [ -z "${UENV_ROLLOUT_MODEL_ENDPOINT}" ]; then
    echo "UENV_ROLLOUT_MODEL_ENDPOINT is required, or set CHECKPOINT_DIR / HF_DIR / MODEL_DIR to start one." >&2
    exit 2
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
if [ -n "${PODMAN_GPU_ARGS}" ]; then
    PODMAN_ARGS+=(--device "${PODMAN_GPU_ARGS}")
fi

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
