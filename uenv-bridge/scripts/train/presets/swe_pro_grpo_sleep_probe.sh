#!/usr/bin/env bash
set -euo pipefail

REPO_DIR=${REPO_DIR:-"$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"}
cd "${REPO_DIR}"

RUN_TS=${RUN_TS:-$(date +%Y%m%d_%H%M%S)}

export RUN_ID=${RUN_ID:-verl_sleep_reuse_probe_${RUN_TS}}
export LOG_FILE=${LOG_FILE:-${REPO_DIR}/temp/logs/verl_layer4_agent_loop/${RUN_ID}.log}

export UENV_MODEL_GATEWAY_ENABLED=${UENV_MODEL_GATEWAY_ENABLED:-1}
export UENV_MODEL_GATEWAY_PORT=${UENV_MODEL_GATEWAY_PORT:-18088}
export UENV_MODEL_GATEWAY_PUBLIC_URL=${UENV_MODEL_GATEWAY_PUBLIC_URL:-http://10.10.20.142:${UENV_MODEL_GATEWAY_PORT}/v1}

export MODEL_PATH=${MODEL_PATH:-/data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B}
export CONTAINER_MODEL_PATH=${CONTAINER_MODEL_PATH:-/models/modelscope/Qwen/Qwen3___6-35B-A3B}
export DATA_DIR=${DATA_DIR:-${REPO_DIR}/data/benchmarks/swebenchpro_train_smoke_10}
export CONTAINER_DATA_DIR=${CONTAINER_DATA_DIR:-/data/swebenchpro_train_smoke_10}

export TRAINING_STEPS=${TRAINING_STEPS:-1}
export TOTAL_EPOCHS=${TOTAL_EPOCHS:-1}
export TRAIN_BATCH_SIZE=${TRAIN_BATCH_SIZE:-4}
export PPO_MINI_BATCH_SIZE=${PPO_MINI_BATCH_SIZE:-4}
export PPO_MICRO_BATCH_SIZE_PER_GPU=${PPO_MICRO_BATCH_SIZE_PER_GPU:-1}
export ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=${ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU:-1}
export REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=${REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU:-1}

export ROLLOUT_N=${ROLLOUT_N:-2}
export ROLLOUT_TP=${ROLLOUT_TP:-8}
export NGPUS_PER_NODE=${NGPUS_PER_NODE:-8}
export PODMAN_GPU_ARGS=${PODMAN_GPU_ARGS:-nvidia.com/gpu=all}
export CUDA_VISIBLE_DEVICES_IN_CONTAINER=${CUDA_VISIBLE_DEVICES_IN_CONTAINER:-0,1,2,3,4,5,6,7}

export MAX_PROMPT_LENGTH=${MAX_PROMPT_LENGTH:-4096}
export DATA_MAX_RESPONSE_LENGTH=${DATA_MAX_RESPONSE_LENGTH:-6144}
export ROLLOUT_GPU_MEMORY_UTILIZATION=${ROLLOUT_GPU_MEMORY_UTILIZATION:-0.60}
export ROLLOUT_FREE_CACHE_ENGINE=${ROLLOUT_FREE_CACHE_ENGINE:-True}
export ROLLOUT_ENABLE_SLEEP_MODE=${ROLLOUT_ENABLE_SLEEP_MODE:-True}
export ROLLOUT_CALCULATE_LOG_PROBS=${ROLLOUT_CALCULATE_LOG_PROBS:-False}

export TEST_FREQ=${TEST_FREQ:--1}
export SAVE_FREQ=${SAVE_FREQ:--1}

if [ -z "${EXTRA_VERL_ARGS:-}" ]; then
  EXTRA_VERL_ARG_LIST=(
    "+ray_kwargs.ray_init.runtime_env.env_vars.VERL_LOGGING_LEVEL=DEBUG"
    "+actor_rollout_ref.rollout.max_model_len=16384"
    "actor_rollout_ref.rollout.max_num_batched_tokens=8192"
    "actor_rollout_ref.actor.fsdp_config.optimizer_offload=True"
    "+actor_rollout_ref.model.override_config.attn_implementation=sdpa"
    "+actor_rollout_ref.rollout.engine_kwargs.vllm.enable_auto_tool_choice=True"
    "+actor_rollout_ref.rollout.engine_kwargs.vllm.tool_call_parser=qwen3_coder"
    "actor_rollout_ref.rollout.checkpoint_engine.update_weights_bucket_megabytes=2048"
  )
  EXTRA_VERL_ARGS="${EXTRA_VERL_ARG_LIST[*]}"
fi
export EXTRA_VERL_ARGS

exec "${REPO_DIR}/scripts/train/run_verl_uenv_grpo.sh"
