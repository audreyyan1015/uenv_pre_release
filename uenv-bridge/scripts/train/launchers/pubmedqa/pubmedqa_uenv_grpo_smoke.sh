#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Run a PubMedQA smoke GRPO job through UEnv + VeRL.

This path uses UEnv AgentLoop and the server-side AdapterCore/Worker chain.
It is intended for experiment A smoke comparison against native VeRL.
EOF
}

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
  usage
  exit 0
fi

REPO_DIR=${REPO_DIR:-"$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../.." && pwd)"}
cd "${REPO_DIR}"

RUN_TS=${RUN_TS:-$(date +%Y%m%d_%H%M%S)}
export RUN_ID=${RUN_ID:-verl_pubmedqa_uenv_grpo_${RUN_TS}}
export UENV_TRAINING_RUN_ID=${UENV_TRAINING_RUN_ID:-${RUN_ID}}
export LOG_FILE=${LOG_FILE:-${REPO_DIR}/temp/logs/verl_layer4_agent_loop/${RUN_ID}.log}

export MODEL_PATH=${MODEL_PATH:-/data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B}
export CONTAINER_MODEL_PATH=${CONTAINER_MODEL_PATH:-/models/modelscope/Qwen/Qwen3___6-35B-A3B}
export DATA_DIR=${DATA_DIR:-${REPO_DIR}/temp/training_data/pubmedqa_smoke}
export CONTAINER_DATA_DIR=${CONTAINER_DATA_DIR:-/data/pubmedqa}

export SERVER_ADAPTER_CORE_ENDPOINT=${SERVER_ADAPTER_CORE_ENDPOINT:-8.130.75.157:8088}
export UENV_OBS_URL=${UENV_OBS_URL:-http://8.130.75.157:8888/obs}
export UENV_OBS_TOKEN=${UENV_OBS_TOKEN:-}

export UENV_MODEL_GATEWAY_ENABLED=${UENV_MODEL_GATEWAY_ENABLED:-1}
export UENV_MODEL_GATEWAY_PORT=${UENV_MODEL_GATEWAY_PORT:-18088}
export UENV_MODEL_GATEWAY_PUBLIC_URL=${UENV_MODEL_GATEWAY_PUBLIC_URL:-http://10.10.20.142:${UENV_MODEL_GATEWAY_PORT}/v1}
export UENV_MODEL_GATEWAY_MAX_TOKENS=${UENV_MODEL_GATEWAY_MAX_TOKENS:-128}
export UENV_MODEL_GATEWAY_DISABLE_THINKING=${UENV_MODEL_GATEWAY_DISABLE_THINKING:-1}

export UENV_AGENT_LOOP_PARALLEL_MODE=${UENV_AGENT_LOOP_PARALLEL_MODE:-sync}
export UENV_AGENT_LOOP_FAILED_EPISODE_POLICY=${UENV_AGENT_LOOP_FAILED_EPISODE_POLICY:-raise}
export UENV_REQUIRE_SWE_RESPONSE_TRACE=${UENV_REQUIRE_SWE_RESPONSE_TRACE:-0}
export UENV_DEFAULT_ENV_TYPE=${UENV_DEFAULT_ENV_TYPE:-qa}
export UENV_EPISODE_MAX_STEPS_OVERRIDE=${UENV_EPISODE_MAX_STEPS_OVERRIDE:-1}

export TRAINING_STEPS=${TRAINING_STEPS:-2}
export TOTAL_EPOCHS=${TOTAL_EPOCHS:-1}
export TRAIN_BATCH_SIZE=${TRAIN_BATCH_SIZE:-2}
export PPO_MINI_BATCH_SIZE=${PPO_MINI_BATCH_SIZE:-2}
export PPO_MICRO_BATCH_SIZE_PER_GPU=${PPO_MICRO_BATCH_SIZE_PER_GPU:-1}
export ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=${ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU:-1}
export REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=${REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU:-1}
export MAX_PROMPT_LENGTH=${MAX_PROMPT_LENGTH:-4096}
export DATA_MAX_RESPONSE_LENGTH=${DATA_MAX_RESPONSE_LENGTH:-128}
export ROLLOUT_N=${ROLLOUT_N:-4}
export ROLLOUT_TEMPERATURE=${ROLLOUT_TEMPERATURE:-1.0}
export ROLLOUT_TP=${ROLLOUT_TP:-8}
export NGPUS_PER_NODE=${NGPUS_PER_NODE:-8}
export AGENT_NUM_WORKERS=${AGENT_NUM_WORKERS:-1}
export PODMAN_GPU_ARGS=${PODMAN_GPU_ARGS:-nvidia.com/gpu=all}
export CUDA_VISIBLE_DEVICES_IN_CONTAINER=${CUDA_VISIBLE_DEVICES_IN_CONTAINER:-0,1,2,3,4,5,6,7}
export RAY_NUM_CPUS=${RAY_NUM_CPUS:-$((NGPUS_PER_NODE * 4))}

export ROLLOUT_GPU_MEMORY_UTILIZATION=${ROLLOUT_GPU_MEMORY_UTILIZATION:-0.50}
export ROLLOUT_FREE_CACHE_ENGINE=${ROLLOUT_FREE_CACHE_ENGINE:-True}
export ROLLOUT_ENABLE_SLEEP_MODE=${ROLLOUT_ENABLE_SLEEP_MODE:-True}
export ROLLOUT_CALCULATE_LOG_PROBS=${ROLLOUT_CALCULATE_LOG_PROBS:-False}

export TEST_FREQ=${TEST_FREQ:--1}
export SAVE_FREQ=${SAVE_FREQ:--1}
export TRAINER_LOGGER=${TRAINER_LOGGER:-"['console']"}
export TRAINER_PROJECT_NAME=${TRAINER_PROJECT_NAME:-uenv_pubmedqa_grpo_train}
export EXPERIMENT_NAME=${EXPERIMENT_NAME:-qwen3_6_35b_a3b_pubmedqa_uenv_grpo_${RUN_TS}}

if [ -z "${EXTRA_VERL_ARGS:-}" ]; then
  EXTRA_VERL_ARGS=$(
    printf '%s ' \
      "+ray_kwargs.ray_init.runtime_env.env_vars.VERL_LOGGING_LEVEL=INFO" \
      "+ray_kwargs.ray_init.runtime_env.env_vars.VLLM_LOGGING_LEVEL=INFO" \
      "+data.apply_chat_template_kwargs.enable_thinking=False" \
      "+actor_rollout_ref.rollout.max_model_len=8192" \
      "actor_rollout_ref.rollout.max_num_batched_tokens=16384" \
      "actor_rollout_ref.rollout.multi_stage_wake_up=True" \
      "+actor_rollout_ref.model.override_config.attn_implementation=sdpa" \
      "actor_rollout_ref.rollout.checkpoint_engine.update_weights_bucket_megabytes=1024"
  )
fi
export EXTRA_VERL_ARGS

exec "${REPO_DIR}/scripts/train/launchers/common/run_verl_uenv_grpo.sh"
