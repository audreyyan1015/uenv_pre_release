#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Run one ROLL RLVR 8GPU timing experiment.

This wrapper keeps the 8GPU RLVR comparison knobs aligned across sync and
async_training runs. Override any variable from the shell when needed. Set
ROLL_GPU_SPLIT to use all eight visible GPUs with a contiguous trainer/ref
range followed by a rollout/vLLM range.

Examples:
  ROLL_MODE=sync ./scripts/roll_step_parallel/run_roll_rlvr_8gpu_matrix.sh
  ROLL_MODE=async_training ROLL_ASYNC_GENERATION_RATIO=2 ./scripts/roll_step_parallel/run_roll_rlvr_8gpu_matrix.sh
  ROLL_MODE=async_training ROLL_ASYNC_GENERATION_RATIO=2 ROLL_GPU_SPLIT=2x6 ./scripts/roll_step_parallel/run_roll_rlvr_8gpu_matrix.sh
EOF
}

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
  usage
  exit 0
fi

ROLL_MODE=${ROLL_MODE:-sync}
case "${ROLL_MODE}" in
  sync|async_training) ;;
  *)
    echo "ROLL_MODE must be sync or async_training for RLVR 8GPU comparison: ${ROLL_MODE}" >&2
    exit 1
    ;;
esac

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROLL_GPU_SPLIT=${ROLL_GPU_SPLIT:-2x6}
case "${ROLL_GPU_SPLIT}" in
  1x7)
    DEFAULT_TRAIN_GPUS=1
    DEFAULT_ROLLOUT_START=1
    DEFAULT_ROLLOUT_END=8
    ;;
  2x6)
    DEFAULT_TRAIN_GPUS=2
    DEFAULT_ROLLOUT_START=2
    DEFAULT_ROLLOUT_END=8
    ;;
  4x4)
    DEFAULT_TRAIN_GPUS=4
    DEFAULT_ROLLOUT_START=4
    DEFAULT_ROLLOUT_END=8
    ;;
  6x2)
    DEFAULT_TRAIN_GPUS=6
    DEFAULT_ROLLOUT_START=6
    DEFAULT_ROLLOUT_END=8
    ;;
  *)
    echo "ROLL_GPU_SPLIT must be one of 1x7, 2x6, 4x4, 6x2: ${ROLL_GPU_SPLIT}" >&2
    exit 1
    ;;
esac

export ROLL_MODE
export ROLL_MAX_STEPS=${ROLL_MAX_STEPS:-5}
export ROLL_ROLLOUT_BATCH_SIZE=${ROLL_ROLLOUT_BATCH_SIZE:-16}
export ROLL_RESPONSE_LENGTH=${ROLL_RESPONSE_LENGTH:-512}
export ROLL_NUM_RETURN_SEQUENCES=${ROLL_NUM_RETURN_SEQUENCES:-1}
export ROLL_GRAD_ACCUM_STEPS=${ROLL_GRAD_ACCUM_STEPS:-4}
export ROLL_TRAIN_MICRO_BATCH_SIZE=${ROLL_TRAIN_MICRO_BATCH_SIZE:-1}
export ROLL_NUM_GPUS_PER_NODE=${ROLL_NUM_GPUS_PER_NODE:-8}
export ROLL_ACTOR_TRAIN_START_GPU=${ROLL_ACTOR_TRAIN_START_GPU:-0}
export ROLL_ACTOR_TRAIN_END_GPU=${ROLL_ACTOR_TRAIN_END_GPU:-${DEFAULT_TRAIN_GPUS}}
export ROLL_REFERENCE_START_GPU=${ROLL_REFERENCE_START_GPU:-0}
export ROLL_REFERENCE_END_GPU=${ROLL_REFERENCE_END_GPU:-${DEFAULT_TRAIN_GPUS}}
export ROLL_ACTOR_TRAIN_WORLD_SIZE=${ROLL_ACTOR_TRAIN_WORLD_SIZE:-${DEFAULT_TRAIN_GPUS}}
export ROLL_REFERENCE_WORLD_SIZE=${ROLL_REFERENCE_WORLD_SIZE:-${DEFAULT_TRAIN_GPUS}}
export ROLL_REWARD_WORLD_SIZE=${ROLL_REWARD_WORLD_SIZE:-4}
export ROLL_ACTOR_INFER_START_GPU=${ROLL_ACTOR_INFER_START_GPU:-${DEFAULT_ROLLOUT_START}}
export ROLL_ACTOR_INFER_END_GPU=${ROLL_ACTOR_INFER_END_GPU:-${DEFAULT_ROLLOUT_END}}
export ROLL_VLLM_GPU_MEMORY_UTILIZATION=${ROLL_VLLM_GPU_MEMORY_UTILIZATION:-0.6}
export ROLL_MAX_RUNNING_REQUESTS=${ROLL_MAX_RUNNING_REQUESTS:-128}
export PODMAN_GPU_ARGS=${PODMAN_GPU_ARGS:-nvidia.com/gpu=0,1,2,3,4,5,6,7}
export CUDA_VISIBLE_DEVICES_IN_CONTAINER=${CUDA_VISIBLE_DEVICES_IN_CONTAINER:-0,1,2,3,4,5,6,7}

RUN_SUFFIX=${RUN_SUFFIX:-8gpu_${ROLL_GPU_SPLIT}_b${ROLL_ROLLOUT_BATCH_SIZE}_r${ROLL_RESPONSE_LENGTH}_${ROLL_MAX_STEPS}step}
export RUN_ID=${RUN_ID:-roll_${ROLL_MODE}_${RUN_SUFFIX}_$(date +%Y%m%d_%H%M%S)}

exec "${SCRIPT_DIR}/run_roll_reproduction.sh"
