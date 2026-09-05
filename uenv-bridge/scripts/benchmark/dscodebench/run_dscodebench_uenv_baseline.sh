#!/usr/bin/env bash
# UEnv benchmark | DSCodeBench | code env | optional checkpoint serving

set -xeuo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO_DIR=${REPO_DIR:-$(cd "${SCRIPT_DIR}/../../.." && pwd)}

# ---- user-adjustable ----
IMAGE=${IMAGE:-localhost/uenv-bridge-verl:layer4-build}
DATA_FILE=${DATA_FILE:-${REPO_DIR}/data/benchmarks/dscodebench/DSCodeBench.json}
OUTPUT_DIR=${OUTPUT_DIR:-${REPO_DIR}/temp/benchmarks/dscodebench/qwen3_6_35b_a3b_uenv_thinking_max32768_budget16384_full_$(date +%Y%m%d_%H%M%S)}
# Logical run id used to group benchmark episodes in Obs/frontend.
RUN_ID=${RUN_ID:-${UENV_TRAINING_RUN_ID:-}}

# UEnv routing:
# - AdapterCore receives EpisodeRequest batches and forwards them to Server/Worker.
# - The rollout model endpoint is an OpenAI-compatible /v1 URL used by remote envs.
# - Obs is optional progress/event reporting for the frontend.
UENV_ADAPTER_CORE_ENDPOINT=${UENV_ADAPTER_CORE_ENDPOINT:-8.130.75.157:8088}
UENV_ROLLOUT_MODEL_ENDPOINT=${UENV_ROLLOUT_MODEL_ENDPOINT:-}
UENV_ROLLOUT_MODEL_NAME=${UENV_ROLLOUT_MODEL_NAME:-Qwen/Qwen3.6-35B-A3B}
UENV_OBS_URL=${UENV_OBS_URL:-}
UENV_OBS_TOKEN=${UENV_OBS_TOKEN:-}

# Dataset slicing. LIMIT caps total rows; LIBRARY filters row["library"];
# MAX_PER_LIBRARY caps each library after filtering.
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
# inline_harness embeds the DSCodeBench test harness in the EpisodeRequest.
# path_harness asks the Worker to find a pre-staged test file by problem_id.
EVALUATION_MODE=${EVALUATION_MODE:-inline_harness}
# Resume appends logs and skips problem_ids already completed in OUTPUT_DIR.
RESUME=${RESUME:-0}

# Evaluator container and optional checkpoint serving. If CHECKPOINT_DIR, HF_DIR,
# or MODEL_DIR is set, common/checkpoint_model.sh starts local vLLM + gateway.
PODMAN_GPU_ARGS=${PODMAN_GPU_ARGS:-}
PODMAN_EXTRA_ARGS=${PODMAN_EXTRA_ARGS:-}
VLLM_PORT=${VLLM_PORT:-18081}
GATEWAY_PORT=${GATEWAY_PORT:-18094}
MAX_MODEL_LEN=${MAX_MODEL_LEN:-65536}
GATEWAY_ENABLE_THINKING=${GATEWAY_ENABLE_THINKING:-${ENABLE_THINKING}}
GATEWAY_STRIP_REASONING=${GATEWAY_STRIP_REASONING:-1}
GATEWAY_THINKING_BUDGET=${GATEWAY_THINKING_BUDGET:-${THINKING_TOKEN_BUDGET}}
# ---- end user-adjustable ----

########################### model endpoint ###########################
source "${REPO_DIR}/scripts/benchmark/common/checkpoint_model.sh"
uenv_benchmark_prepare_model_endpoint "${OUTPUT_DIR}"

if [ -z "${UENV_ROLLOUT_MODEL_ENDPOINT}" ]; then
    echo "UENV_ROLLOUT_MODEL_ENDPOINT is required, or set CHECKPOINT_DIR / HF_DIR / MODEL_DIR to start one." >&2
    exit 2
fi

mkdir -p "${OUTPUT_DIR}"

########################### parameter arrays ###########################
EVAL_ARGS=(
    --data "${DATA_FILE}"
    --output-dir "${OUTPUT_DIR}"
    --endpoint "${UENV_ADAPTER_CORE_ENDPOINT}"
    --model-endpoint "${UENV_ROLLOUT_MODEL_ENDPOINT}"
    --model-name "${UENV_ROLLOUT_MODEL_NAME}"
    --batch-size "${BATCH_SIZE}"
    --prompt-style "${PROMPT_STYLE}"
    --max-tokens "${MAX_TOKENS}"
    --temperature "${TEMPERATURE}"
    --top-p "${TOP_P}"
    --test-case-number "${TEST_CASE_NUMBER}"
    --timeout-seconds "${TIMEOUT_SECONDS}"
    --code-timeout-secs "${CODE_TIMEOUT_SECS}"
    --client-timeout-seconds "${CLIENT_TIMEOUT_SECONDS}"
    --evaluation-mode "${EVALUATION_MODE}"
)

EXTRA=()
if [ -n "${LIMIT}" ]; then
    EXTRA+=(--limit "${LIMIT}")
fi
if [ -n "${LIBRARY}" ]; then
    EXTRA+=(--library "${LIBRARY}")
fi
if [ -n "${MAX_PER_LIBRARY}" ]; then
    EXTRA+=(--max-per-library "${MAX_PER_LIBRARY}")
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
if [ "${ENABLE_THINKING}" = "1" ]; then
    EXTRA+=(--enable-thinking)
fi
if [ "${PRESERVE_THINKING}" = "1" ]; then
    EXTRA+=(--preserve-thinking)
fi
if [ -n "${THINKING_TOKEN_BUDGET}" ]; then
    EXTRA+=(--thinking-token-budget "${THINKING_TOKEN_BUDGET}")
fi
if [ "${RESUME}" = "1" ]; then
    EXTRA+=(--resume)
fi

PODMAN_ARGS=(
    --rm
    --entrypoint python3
    --network host
    --pids-limit=-1
    --shm-size=32g
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
    scripts/benchmark/dscodebench/evaluate_dscodebench_uenv.py \
    "${EVAL_ARGS[@]}" \
    "${EXTRA[@]}" \
    "$@"
