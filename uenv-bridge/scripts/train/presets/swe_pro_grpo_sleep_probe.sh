#!/usr/bin/env bash
set -euo pipefail

# SWE-Pro + VeRL GRPO 的 sleep/free-cache 验证预设
# 本脚本只设置任务级默认参数，实际训练逻辑由通用入口负责
REPO_DIR=${REPO_DIR:-"$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"}
cd "${REPO_DIR}"

# 同步模式
export UENV_AGENT_LOOP_PARALLEL_MODE=${UENV_AGENT_LOOP_PARALLEL_MODE:-sync}

# 运行 ID 与日志
RUN_TS=${RUN_TS:-$(date +%Y%m%d_%H%M%S)}
export RUN_ID=${RUN_ID:-verl_sleep_reuse_probe_${RUN_TS}}
export LOG_FILE=${LOG_FILE:-${REPO_DIR}/temp/logs/verl_layer4_agent_loop/${RUN_ID}.log}
export WANDB_ENV_FILE=${WANDB_ENV_FILE:-${REPO_DIR}/../secrets/wandb.env}

# VeRL/wandb 日志。默认只输出 console；开启 wandb 时传入：
#   TRAINER_LOGGER="['console','wandb']" WANDB_MODE=online WANDB_API_KEY=...
# 也可以写入 /data/ronghao/uenv/secrets/wandb.env，由通用入口自动读取。
export TRAINER_LOGGER=${TRAINER_LOGGER:-"['console','wandb']"}
export TRAINER_PROJECT_NAME=${TRAINER_PROJECT_NAME:-uenv_swepro_grpo_train}
export WANDB_MODE=${WANDB_MODE:-online}
export EXPERIMENT_NAME=${EXPERIMENT_NAME:-qwen3_6_35b_a3b_swepro_train_${RUN_TS}}

# Server Obs / 前端可视化。默认通过前端机器的 /obs 反代上报事件。
export UENV_OBS_URL=${UENV_OBS_URL-http://8.130.75.157:8888/obs}
export UENV_OBS_TOKEN=${UENV_OBS_TOKEN:-}
export UENV_TRAINING_RUN_ID=${UENV_TRAINING_RUN_ID:-${RUN_ID}}

# Adapter model gateway。Worker/OpenHands 访问该地址，gateway 再转发到 VeRL 内部 vLLM
export UENV_MODEL_GATEWAY_ENABLED=${UENV_MODEL_GATEWAY_ENABLED:-1}
export UENV_MODEL_GATEWAY_PORT=${UENV_MODEL_GATEWAY_PORT:-18088}
export UENV_MODEL_GATEWAY_PUBLIC_URL=${UENV_MODEL_GATEWAY_PUBLIC_URL:-http://10.10.20.142:${UENV_MODEL_GATEWAY_PORT}/v1}
export UENV_MODEL_GATEWAY_MAX_TOKENS=${UENV_MODEL_GATEWAY_MAX_TOKENS:-4096}

# 模型与数据挂载。宿主机路径挂到容器内固定路径，供 VeRL Hydra 参数引用
export MODEL_PATH=${MODEL_PATH:-/data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B}
export CONTAINER_MODEL_PATH=${CONTAINER_MODEL_PATH:-/models/modelscope/Qwen/Qwen3___6-35B-A3B}
export DATA_DIR=${DATA_DIR:-${REPO_DIR}/data/benchmarks/swebenchpro}
export CONTAINER_DATA_DIR=${CONTAINER_DATA_DIR:-/data/swebenchpro}

# SWE/OpenHands 数据准备参数。
# SWE_TRAJECTORY_MAX_STEPS 既用于生成数据，也默认作为运行时 episode 步数覆盖值；
# 如需完全使用数据集 extra_info 中的 max_steps/max_iterations，可显式传入：
#   UENV_EPISODE_MAX_STEPS_OVERRIDE=
export SWE_PREPARE_DATA=${SWE_PREPARE_DATA:-1}
export SWE_SAMPLE_LIMIT=${SWE_SAMPLE_LIMIT:-731}
export SWE_SAMPLE_OFFSET=${SWE_SAMPLE_OFFSET:-0}
export SWE_WORKSPACE_DIR=${SWE_WORKSPACE_DIR:-/app}
export SWE_LLM_CONFIG_PATH=${SWE_LLM_CONFIG_PATH:-/root/UEnv/config/openhands-llm-qwen3-thinking-max-token-8192.json}
export SWE_TRAJECTORY_MAX_STEPS=${SWE_TRAJECTORY_MAX_STEPS:-30}
if [ -z "${UENV_EPISODE_MAX_STEPS_OVERRIDE+x}" ]; then
  export UENV_EPISODE_MAX_STEPS_OVERRIDE=${SWE_TRAJECTORY_MAX_STEPS}
fi
export SWE_ENV_PACKAGE_VERSION=${SWE_ENV_PACKAGE_VERSION:-0.3.4}
export SWE_AGENT_MODE=${SWE_AGENT_MODE:-llm}

# 训练与 batch 参数
export TRAINING_STEPS=${TRAINING_STEPS:-500}
export TOTAL_EPOCHS=${TOTAL_EPOCHS:-1}
export TRAIN_BATCH_SIZE=${TRAIN_BATCH_SIZE:-2}
export PPO_MINI_BATCH_SIZE=${PPO_MINI_BATCH_SIZE:-2}
export PPO_MICRO_BATCH_SIZE_PER_GPU=${PPO_MICRO_BATCH_SIZE_PER_GPU:-1}
export ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=${ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU:-1}
export REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=${REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU:-1}

# rollout 与 GPU 资源参数。默认使用 8 卡 TP 跑 Qwen3.6-35B-A3B
export ROLLOUT_N=${ROLLOUT_N:-4}
export ROLLOUT_TP=${ROLLOUT_TP:-8}
export NGPUS_PER_NODE=${NGPUS_PER_NODE:-8}
export AGENT_NUM_WORKERS=${AGENT_NUM_WORKERS:-1}
export UENV_EXPECTED_WORKER_PARALLELISM=${UENV_EXPECTED_WORKER_PARALLELISM:-8}
export UENV_MAX_EPISODE_CONCURRENCY=${UENV_MAX_EPISODE_CONCURRENCY:-${UENV_EXPECTED_WORKER_PARALLELISM}}
export UENV_MAX_IN_FLIGHT_BATCHES=${UENV_MAX_IN_FLIGHT_BATCHES:-1}
export UENV_TARGET_WORKER_SLOTS=${UENV_TARGET_WORKER_SLOTS:-${UENV_EXPECTED_WORKER_PARALLELISM}}
export UENV_POOL_WARMUP_TARGET=${UENV_POOL_WARMUP_TARGET:-${UENV_EXPECTED_WORKER_PARALLELISM}}
export UENV_MAX_PARALLEL_PER_WORKER=${UENV_MAX_PARALLEL_PER_WORKER:-4}
export UENV_AGENT_JOB_MAX_CONCURRENCY=${UENV_AGENT_JOB_MAX_CONCURRENCY:-${UENV_MAX_PARALLEL_PER_WORKER}}
export UENV_RUNTIME_GATEWAY_SESSION_LIMIT=${UENV_RUNTIME_GATEWAY_SESSION_LIMIT:-${UENV_MAX_PARALLEL_PER_WORKER}}
export UENV_REQUIRE_WARM_SLOT=${UENV_REQUIRE_WARM_SLOT:-false}
export PODMAN_GPU_ARGS=${PODMAN_GPU_ARGS:-nvidia.com/gpu=all}
export CUDA_VISIBLE_DEVICES_IN_CONTAINER=${CUDA_VISIBLE_DEVICES_IN_CONTAINER:-0,1,2,3,4,5,6,7}

# prompt/response 长度与 vLLM 显存复用参数
export MAX_PROMPT_LENGTH=${MAX_PROMPT_LENGTH:-8192}
export DATA_MAX_RESPONSE_LENGTH=${DATA_MAX_RESPONSE_LENGTH:-8192}
export ROLLOUT_GPU_MEMORY_UTILIZATION=${ROLLOUT_GPU_MEMORY_UTILIZATION:-0.50}
export ROLLOUT_FREE_CACHE_ENGINE=${ROLLOUT_FREE_CACHE_ENGINE:-True}
export ROLLOUT_ENABLE_SLEEP_MODE=${ROLLOUT_ENABLE_SLEEP_MODE:-True}

# SWE/OpenHands 训练需要 response token trace；当前 worker 通过 OpenAI logprobs
# 恢复 token_id，所以这里即使是同步 GRPO 也默认要求 rollout 返回 logprobs。
export ROLLOUT_CALCULATE_LOG_PROBS=${ROLLOUT_CALCULATE_LOG_PROBS:-True}
export UENV_REQUIRE_SWE_RESPONSE_TRACE=${UENV_REQUIRE_SWE_RESPONSE_TRACE:-1}
export UENV_AGENT_LOOP_FAILED_EPISODE_POLICY=${UENV_AGENT_LOOP_FAILED_EPISODE_POLICY:-zero_reward}
export UENV_PATCH_VERL_TEXT_ONLY_PROCESSOR=${UENV_PATCH_VERL_TEXT_ONLY_PROCESSOR:-1}

# smoke/probe 默认不做验证和保存 checkpoint
export TEST_FREQ=${TEST_FREQ:--1}
export SAVE_FREQ=${SAVE_FREQ:--1}

# 额外 VeRL Hydra overrides
if [ -z "${EXTRA_VERL_ARGS:-}" ]; then
  EXTRA_VERL_ARG_LIST=(
    "+ray_kwargs.ray_init.runtime_env.env_vars.VERL_LOGGING_LEVEL=INFO"
    "+ray_kwargs.ray_init.runtime_env.env_vars.VLLM_LOGGING_LEVEL=INFO"
    "+actor_rollout_ref.rollout.max_model_len=131072"
    "actor_rollout_ref.rollout.max_num_batched_tokens=65536"
    "actor_rollout_ref.rollout.multi_stage_wake_up=True"
    "actor_rollout_ref.actor.fsdp_config.optimizer_offload=True"
    "+actor_rollout_ref.model.override_config.attn_implementation=sdpa"
    "+actor_rollout_ref.rollout.engine_kwargs.vllm.enable_auto_tool_choice=True"
    "+actor_rollout_ref.rollout.engine_kwargs.vllm.tool_call_parser=qwen3_coder"
    "actor_rollout_ref.rollout.checkpoint_engine.update_weights_bucket_megabytes=1024"
  )
  EXTRA_VERL_ARGS="${EXTRA_VERL_ARG_LIST[*]}"
fi
export EXTRA_VERL_ARGS

if [ "${SWE_PREPARE_DATA}" != "0" ]; then
  python3 "${REPO_DIR}/scripts/utils/prepare_verl_swebenchpro_smoke.py" \
    --output-dir "${DATA_DIR}" \
    --limit "${SWE_SAMPLE_LIMIT}" \
    --offset "${SWE_SAMPLE_OFFSET}" \
    --workspace-dir "${SWE_WORKSPACE_DIR}" \
    --llm-config-path "${SWE_LLM_CONFIG_PATH}" \
    --max-steps "${SWE_TRAJECTORY_MAX_STEPS}" \
    --env-package-version "${SWE_ENV_PACKAGE_VERSION}" \
    --agent-mode "${SWE_AGENT_MODE}"
fi

# 进入通用 VeRL + UEnv GRPO 训练入口。
exec "${REPO_DIR}/scripts/train/run_verl_uenv_grpo.sh"
