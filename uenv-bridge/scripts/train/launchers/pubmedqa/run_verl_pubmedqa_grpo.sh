#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Run native VeRL GRPO on PubMedQA VeRL-format train/test parquet data.

This path does not use UEnv Server/Worker.  PubMedQA reward is computed locally
through scripts/train/rewards/pubmedqa_label_reward.py.

Quick smoke:
  ./scripts/train/launchers/pubmedqa/run_verl_pubmedqa_grpo.sh

Useful overrides:
  TRAINING_STEPS=2
  TRAIN_BATCH_SIZE=2
  PPO_MINI_BATCH_SIZE=2
  ROLLOUT_N=4
  DATA_DIR=/data/ronghao/uenv/uenv-bridge/temp/training_data/pubmedqa_smoke
EOF
}

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
  usage
  exit 0
fi

REPO_DIR=${REPO_DIR:-"$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../.." && pwd)"}
RUN_TS=${RUN_TS:-$(date +%Y%m%d_%H%M%S)}

export RUN_ID=${RUN_ID:-verl_pubmedqa_grpo_native_${RUN_TS}}
export DATASET_NAME=${DATASET_NAME:-PubMedQA}
export TRAINING_RUN_LABEL=${TRAINING_RUN_LABEL:-"native VeRL PubMedQA GRPO"}
export METRICS_HINT=${METRICS_HINT:-"critic/score/mean, critic/rewards/mean"}
export METRICS_GREP_PATTERN=${METRICS_GREP_PATTERN:-"critic/score/mean|critic/rewards/mean|actor/loss|response_length/clip_ratio|Training Progress|total time:"}
export MODEL_PATH=${MODEL_PATH:-/data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B}
export CONTAINER_MODEL_PATH=${CONTAINER_MODEL_PATH:-/models/modelscope/Qwen/Qwen3___6-35B-A3B}
export DATA_DIR=${DATA_DIR:-${REPO_DIR}/temp/training_data/pubmedqa_smoke}
export CONTAINER_DATA_DIR=${CONTAINER_DATA_DIR:-/data/pubmedqa}

export TRAINING_STEPS=${TRAINING_STEPS:-2}
export TOTAL_EPOCHS=${TOTAL_EPOCHS:-1}
export TRAIN_MAX_SAMPLES=${TRAIN_MAX_SAMPLES:--1}
export VAL_MAX_SAMPLES=${VAL_MAX_SAMPLES:--1}
export TRAIN_BATCH_SIZE=${TRAIN_BATCH_SIZE:-2}
export PPO_MINI_BATCH_SIZE=${PPO_MINI_BATCH_SIZE:-2}
export PPO_MICRO_BATCH_SIZE_PER_GPU=${PPO_MICRO_BATCH_SIZE_PER_GPU:-1}
export ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=${ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU:-1}
export REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=${REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU:-1}
export MAX_PROMPT_LENGTH=${MAX_PROMPT_LENGTH:-4096}
export DATA_MAX_RESPONSE_LENGTH=${DATA_MAX_RESPONSE_LENGTH:-128}
export ROLLOUT_N=${ROLLOUT_N:-4}
export ROLLOUT_TP=${ROLLOUT_TP:-8}
export NGPUS_PER_NODE=${NGPUS_PER_NODE:-8}
export PODMAN_GPU_ARGS=${PODMAN_GPU_ARGS:-nvidia.com/gpu=all}
export CUDA_VISIBLE_DEVICES_IN_CONTAINER=${CUDA_VISIBLE_DEVICES_IN_CONTAINER:-0,1,2,3,4,5,6,7}
export RAY_NUM_CPUS=${RAY_NUM_CPUS:-$((NGPUS_PER_NODE * 4))}

export ROLLOUT_GPU_MEMORY_UTILIZATION=${ROLLOUT_GPU_MEMORY_UTILIZATION:-0.50}
export ROLLOUT_FREE_CACHE_ENGINE=${ROLLOUT_FREE_CACHE_ENGINE:-True}
export ROLLOUT_ENABLE_SLEEP_MODE=${ROLLOUT_ENABLE_SLEEP_MODE:-True}
export ROLLOUT_ENFORCE_EAGER=${ROLLOUT_ENFORCE_EAGER:-False}
export ROLLOUT_ENABLE_CHUNKED_PREFILL=${ROLLOUT_ENABLE_CHUNKED_PREFILL:-True}
export ROLLOUT_MAX_NUM_SEQS=${ROLLOUT_MAX_NUM_SEQS:-8}
export ROLLOUT_MAX_NUM_BATCHED_TOKENS=${ROLLOUT_MAX_NUM_BATCHED_TOKENS:-16384}
export ROLLOUT_MAX_MODEL_LEN=${ROLLOUT_MAX_MODEL_LEN:-8192}
export ROLLOUT_CALCULATE_LOG_PROBS=${ROLLOUT_CALCULATE_LOG_PROBS:-False}

export VAL_BEFORE_TRAIN=${VAL_BEFORE_TRAIN:-False}
export VAL_ONLY=${VAL_ONLY:-False}
export TEST_FREQ=${TEST_FREQ:--1}
export SAVE_FREQ=${SAVE_FREQ:--1}
export TRAINER_LOGGER=${TRAINER_LOGGER:-"['console']"}
export TRAINER_PROJECT_NAME=${TRAINER_PROJECT_NAME:-verl_pubmedqa_grpo_train}
export EXPERIMENT_NAME=${EXPERIMENT_NAME:-qwen3_6_35b_a3b_pubmedqa_native_grpo_${RUN_TS}}
export LOG_DIR=${LOG_DIR:-${REPO_DIR}/temp/logs/verl_pubmedqa_native_grpo}
export LOG_FILE=${LOG_FILE:-${LOG_DIR}/${RUN_ID}.log}
export CHECKPOINT_ROOT=${CHECKPOINT_ROOT:-${REPO_DIR}/checkpoints/pubmedqa_native_grpo}
export CONTAINER_CHECKPOINT_RUN_DIR=${CONTAINER_CHECKPOINT_RUN_DIR:-/uenv/uenv-bridge/checkpoints/pubmedqa_native_grpo/${RUN_ID}}

REWARD_ARGS=(
  "+data.apply_chat_template_kwargs.enable_thinking=False"
  "reward.custom_reward_function.path=/uenv/uenv-bridge/scripts/train/rewards/pubmedqa_label_reward.py"
  "reward.custom_reward_function.name=compute_score"
)
if [ -n "${EXTRA_VERL_ARGS:-}" ]; then
  export EXTRA_VERL_ARGS="${EXTRA_VERL_ARGS} ${REWARD_ARGS[*]}"
else
  export EXTRA_VERL_ARGS="${REWARD_ARGS[*]}"
fi

exec "${REPO_DIR}/scripts/train/launchers/gsm8k/run_verl_gsm8k_grpo.sh"
