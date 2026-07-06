#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Run ROLL step-parallel reproduction modes inside the VeRL podman image.

Modes:
  ROLL_MODE=sync                   RLVR synchronous baseline
  ROLL_MODE=async_training         RLVR async_generation_ratio pipeline
  ROLL_MODE=agentic_async_rollout  ROLL Agentic Atropos/GSM8K async rollout smoke; requires extra Atropos deps
  ROLL_MODE=agentic_async_rollout_frozenlake
                                  ROLL Agentic async rollout smoke with built-in FrozenLake env

Examples:
  ROLL_MODE=sync ROLL_MAX_STEPS=1 ./scripts/roll_step_parallel/run_roll_reproduction.sh
  ROLL_MODE=async_training ROLL_MAX_STEPS=1 ./scripts/roll_step_parallel/run_roll_reproduction.sh
  ROLL_MODE=agentic_async_rollout ROLL_MAX_STEPS=1 PODMAN_GPU_ARGS="nvidia.com/gpu=0,1" CUDA_VISIBLE_DEVICES_IN_CONTAINER=0,1 ./scripts/roll_step_parallel/run_roll_reproduction.sh
  ROLL_MODE=agentic_async_rollout_frozenlake ROLL_MAX_STEPS=1 PODMAN_GPU_ARGS="nvidia.com/gpu=0,1" CUDA_VISIBLE_DEVICES_IN_CONTAINER=0,1 ./scripts/roll_step_parallel/run_roll_reproduction.sh
EOF
}

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
  usage
  exit 0
fi

REPO_DIR=${REPO_DIR:-"$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"}
source "${REPO_DIR}/scripts/lib/common.sh"
ROLL_SRC=${ROLL_SRC:-/data/zhangzhiyuan/codes/ROLL-main}
IMAGE=${IMAGE:-localhost/uenv-bridge-verl:layer4-build}
ROLL_MODE=${ROLL_MODE:-sync}

DEFAULT_HOST_MODEL_PATH=/data/ronghao/models/modelscope/Qwen/Qwen2___5-0___5B-Instruct
DEFAULT_CONTAINER_MODEL_PATH=/models/modelscope/Qwen/Qwen2___5-0___5B-Instruct
MODEL_PATH=${MODEL_PATH:-${DEFAULT_HOST_MODEL_PATH}}
CONTAINER_MODEL_PATH=${CONTAINER_MODEL_PATH:-${DEFAULT_CONTAINER_MODEL_PATH}}
ROLL_MODEL_PATH=${ROLL_MODEL_PATH:-${CONTAINER_MODEL_PATH}}

RUN_ID=${RUN_ID:-roll_${ROLL_MODE}_$(date +%Y%m%d_%H%M%S)}
LOG_ROOT=${LOG_ROOT:-${REPO_DIR}/temp/logs/roll_step_parallel}
LOG_DIR=${LOG_DIR:-${LOG_ROOT}/${ROLL_MODE}}
LOG_FILE=${LOG_FILE:-${LOG_DIR}/${RUN_ID}.log}
ROLL_OUTPUT_DIR=${ROLL_OUTPUT_DIR:-/uenv/uenv-bridge/temp/logs/roll_step_parallel/output/${RUN_ID}}
ROLL_LOGGING_DIR=${ROLL_LOGGING_DIR:-/uenv/uenv-bridge/temp/logs/roll_step_parallel/roll_logs/${RUN_ID}}
ROLL_CHECKPOINT_DIR=${ROLL_CHECKPOINT_DIR:-/uenv/uenv-bridge/temp/logs/roll_step_parallel/checkpoints/${RUN_ID}}
ROLL_EXP_NAME=${ROLL_EXP_NAME:-${RUN_ID}}

ROLL_MAX_STEPS=${ROLL_MAX_STEPS:-1}
ROLL_ROLLOUT_BATCH_SIZE=${ROLL_ROLLOUT_BATCH_SIZE:-16}
ROLL_NUM_RETURN_SEQUENCES=${ROLL_NUM_RETURN_SEQUENCES:-2}
ROLL_PROMPT_LENGTH=${ROLL_PROMPT_LENGTH:-512}
ROLL_RESPONSE_LENGTH=${ROLL_RESPONSE_LENGTH:-512}
ROLL_TRAIN_MICRO_BATCH_SIZE=${ROLL_TRAIN_MICRO_BATCH_SIZE:-1}
ROLL_GRAD_ACCUM_STEPS=${ROLL_GRAD_ACCUM_STEPS:-4}
ROLL_ASYNC_GENERATION_RATIO=${ROLL_ASYNC_GENERATION_RATIO:-1}
ROLL_NUM_GPUS_PER_NODE=${ROLL_NUM_GPUS_PER_NODE:-8}
ROLL_ACTOR_TRAIN_WORLD_SIZE=${ROLL_ACTOR_TRAIN_WORLD_SIZE:-2}
ROLL_REFERENCE_WORLD_SIZE=${ROLL_REFERENCE_WORLD_SIZE:-2}
ROLL_ACTOR_INFER_START_GPU=${ROLL_ACTOR_INFER_START_GPU:-4}
ROLL_ACTOR_INFER_END_GPU=${ROLL_ACTOR_INFER_END_GPU:-8}
ROLL_ACTOR_TRAIN_START_GPU=${ROLL_ACTOR_TRAIN_START_GPU:-1}
ROLL_ACTOR_TRAIN_END_GPU=${ROLL_ACTOR_TRAIN_END_GPU:-2}
ROLL_REFERENCE_START_GPU=${ROLL_REFERENCE_START_GPU:-1}
ROLL_REFERENCE_END_GPU=${ROLL_REFERENCE_END_GPU:-2}
ROLL_REWARD_WORLD_SIZE=${ROLL_REWARD_WORLD_SIZE:-4}
ROLL_SEQUENCE_LENGTH=${ROLL_SEQUENCE_LENGTH:-1024}
ROLL_VLLM_GPU_MEMORY_UTILIZATION=${ROLL_VLLM_GPU_MEMORY_UTILIZATION:-0.6}
ROLL_MAX_ENV_NUM_PER_WORKER=${ROLL_MAX_ENV_NUM_PER_WORKER:-2}
ROLL_NUM_ENV_GROUPS=${ROLL_NUM_ENV_GROUPS:-2}
ROLL_ENV_GROUP_SIZE=${ROLL_ENV_GROUP_SIZE:-2}
ROLL_MAX_RUNNING_REQUESTS=${ROLL_MAX_RUNNING_REQUESTS:-128}

PODMAN_NETWORK_ARGS=${PODMAN_NETWORK_ARGS:---network host}
PODMAN_GPU_ARGS=${PODMAN_GPU_ARGS:-nvidia.com/gpu=all}
SHM_SIZE=${SHM_SIZE:-128g}
PIDS_LIMIT=${PIDS_LIMIT:-262144}
RAY_RUN_ID=${RUN_ID//[^A-Za-z0-9]/}
RAY_RUN_ID=${RAY_RUN_ID:0:12}
RAY_TMPDIR=${RAY_TMPDIR:-/tmp/roll-${RAY_RUN_ID}}
INSTALL_ROLL_DEPS=${INSTALL_ROLL_DEPS:-1}

case "${ROLL_MODE}" in
  sync)
    ROLL_CONFIG_NAME=${ROLL_CONFIG_NAME:-roll_rlvr_sync}
    ROLL_PIPELINE=${ROLL_PIPELINE:-rlvr}
    ;;
  async_training)
    ROLL_CONFIG_NAME=${ROLL_CONFIG_NAME:-roll_rlvr_async_training}
    ROLL_PIPELINE=${ROLL_PIPELINE:-rlvr}
    ;;
  agentic_async_rollout)
    ROLL_CONFIG_NAME=${ROLL_CONFIG_NAME:-roll_agentic_async_rollout}
    ROLL_PIPELINE=${ROLL_PIPELINE:-agentic}
    DEFAULT_CUDA_VISIBLE_DEVICES_IN_CONTAINER=0,1
    ;;
  agentic_async_rollout_frozenlake)
    ROLL_CONFIG_NAME=${ROLL_CONFIG_NAME:-roll_agentic_async_rollout_frozenlake}
    ROLL_PIPELINE=${ROLL_PIPELINE:-agentic}
    DEFAULT_CUDA_VISIBLE_DEVICES_IN_CONTAINER=0,1
    ;;
  *)
    echo "Unknown ROLL_MODE=${ROLL_MODE}" >&2
    usage >&2
    exit 1
    ;;
esac

DEFAULT_CUDA_VISIBLE_DEVICES_IN_CONTAINER=${DEFAULT_CUDA_VISIBLE_DEVICES_IN_CONTAINER:-0,1,2,3,4,5,6,7}
CUDA_VISIBLE_DEVICES_IN_CONTAINER=${CUDA_VISIBLE_DEVICES_IN_CONTAINER:-${DEFAULT_CUDA_VISIBLE_DEVICES_IN_CONTAINER}}

ensure_path "${ROLL_SRC}/roll" "ROLL source not found"
ensure_path "${MODEL_PATH}/config.json" "Model config not found"
ensure_path "${REPO_DIR}/scripts/roll_step_parallel/configs/${ROLL_CONFIG_NAME}.yaml" "ROLL config not found"

mkdir -p "${LOG_DIR}" "${LOG_ROOT}/output" "${LOG_ROOT}/roll_logs" "${LOG_ROOT}/checkpoints"

PODMAN_GPU_RUN_ARGS=$(build_podman_gpu_args "${PODMAN_GPU_ARGS}")

echo "Running ROLL ${ROLL_MODE}; log: ${LOG_FILE}"
echo "ROLL source: ${ROLL_SRC}"
echo "Config: ${ROLL_CONFIG_NAME}, pipeline: ${ROLL_PIPELINE}"
echo "Visible GPUs in container: ${CUDA_VISIBLE_DEVICES_IN_CONTAINER}"

set +e
podman run --rm \
  ${PODMAN_NETWORK_ARGS} \
  ${PODMAN_GPU_RUN_ARGS} \
  --shm-size="${SHM_SIZE}" \
  --pids-limit="${PIDS_LIMIT}" \
  --entrypoint /bin/bash \
  -v "${ROLL_SRC}:/workspace/ROLL-main:ro" \
  -v "${REPO_DIR}:/uenv/uenv-bridge" \
  -v "${MODEL_PATH}:${CONTAINER_MODEL_PATH}:ro" \
  -w /uenv/uenv-bridge \
  "${IMAGE}" \
  -lc "set -euo pipefail
export PYTHONPATH=/uenv/uenv-bridge/scripts/roll_step_parallel:/workspace/ROLL-main:/workspace/ROLL-main/mcore_adapter/src:\${PYTHONPATH:-}
export CUDA_VISIBLE_DEVICES=${CUDA_VISIBLE_DEVICES_IN_CONTAINER}
export VLLM_NO_USAGE_STATS=1
export TOKENIZERS_PARALLELISM=false
export HYDRA_FULL_ERROR=1
export RAY_DEDUP_LOGS=0
export RAY_TMPDIR=${RAY_TMPDIR}
export WANDB_MODE=disabled
export TQDM_DISABLE=1
export HF_HUB_OFFLINE=1
export TRANSFORMERS_OFFLINE=1
export ROLL_MODEL_PATH=${ROLL_MODEL_PATH}
export ROLL_MAX_STEPS=${ROLL_MAX_STEPS}
export ROLL_ROLLOUT_BATCH_SIZE=${ROLL_ROLLOUT_BATCH_SIZE}
export ROLL_NUM_RETURN_SEQUENCES=${ROLL_NUM_RETURN_SEQUENCES}
export ROLL_PROMPT_LENGTH=${ROLL_PROMPT_LENGTH}
export ROLL_RESPONSE_LENGTH=${ROLL_RESPONSE_LENGTH}
export ROLL_TRAIN_MICRO_BATCH_SIZE=${ROLL_TRAIN_MICRO_BATCH_SIZE}
export ROLL_GRAD_ACCUM_STEPS=${ROLL_GRAD_ACCUM_STEPS}
export ROLL_ASYNC_GENERATION_RATIO=${ROLL_ASYNC_GENERATION_RATIO}
export ROLL_NUM_GPUS_PER_NODE=${ROLL_NUM_GPUS_PER_NODE}
export ROLL_ACTOR_TRAIN_WORLD_SIZE=${ROLL_ACTOR_TRAIN_WORLD_SIZE}
export ROLL_REFERENCE_WORLD_SIZE=${ROLL_REFERENCE_WORLD_SIZE}
export ROLL_ACTOR_INFER_START_GPU=${ROLL_ACTOR_INFER_START_GPU}
export ROLL_ACTOR_INFER_END_GPU=${ROLL_ACTOR_INFER_END_GPU}
export ROLL_ACTOR_TRAIN_START_GPU=${ROLL_ACTOR_TRAIN_START_GPU}
export ROLL_ACTOR_TRAIN_END_GPU=${ROLL_ACTOR_TRAIN_END_GPU}
export ROLL_REFERENCE_START_GPU=${ROLL_REFERENCE_START_GPU}
export ROLL_REFERENCE_END_GPU=${ROLL_REFERENCE_END_GPU}
export ROLL_REWARD_WORLD_SIZE=${ROLL_REWARD_WORLD_SIZE}
export ROLL_SEQUENCE_LENGTH=${ROLL_SEQUENCE_LENGTH}
export ROLL_VLLM_GPU_MEMORY_UTILIZATION=${ROLL_VLLM_GPU_MEMORY_UTILIZATION}
export ROLL_MAX_ENV_NUM_PER_WORKER=${ROLL_MAX_ENV_NUM_PER_WORKER}
export ROLL_NUM_ENV_GROUPS=${ROLL_NUM_ENV_GROUPS}
export ROLL_ENV_GROUP_SIZE=${ROLL_ENV_GROUP_SIZE}
export ROLL_MAX_RUNNING_REQUESTS=${ROLL_MAX_RUNNING_REQUESTS}
export ROLL_OUTPUT_DIR=${ROLL_OUTPUT_DIR}
export ROLL_LOGGING_DIR=${ROLL_LOGGING_DIR}
export ROLL_CHECKPOINT_DIR=${ROLL_CHECKPOINT_DIR}
export ROLL_EXP_NAME=${ROLL_EXP_NAME}
mkdir -p \"${ROLL_OUTPUT_DIR}\" \"${ROLL_LOGGING_DIR}\" \"${ROLL_CHECKPOINT_DIR}\" \"${RAY_TMPDIR}\"
if [ \"${INSTALL_ROLL_DEPS}\" = \"1\" ]; then
  python3 -m pip install -q --root-user-action=ignore \
    dacite imageio math-verify latex2sympy2==1.5.4 latex2sympy2_extended==1.10.1 \
    jsonlines deprecated langdetect nltk aiohttp openai gymnasium 'gymnasium[toy-text]' \
    'antlr4-python3-runtime==4.9.3'
fi
python3 scripts/roll_step_parallel/start_roll_pipeline.py \
  --config-dir /uenv/uenv-bridge/scripts/roll_step_parallel/configs \
  --config-name ${ROLL_CONFIG_NAME} \
  --pipeline ${ROLL_PIPELINE}
" >"${LOG_FILE}" 2>&1
status=$?
set -e

if [ "${status}" -ne 0 ]; then
  echo "ROLL ${ROLL_MODE} failed. Log: ${LOG_FILE}" >&2
  tail -n 80 "${LOG_FILE}" >&2 || true
  exit "${status}"
fi

echo "ROLL ${ROLL_MODE} completed. Log: ${LOG_FILE}"
