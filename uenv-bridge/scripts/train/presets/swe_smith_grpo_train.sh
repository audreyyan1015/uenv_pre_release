#!/usr/bin/env bash
set -euo pipefail

# SWE-smith + VeRL GRPO 正式训练预设。
# 本脚本负责准备 SWE-smith VeRL 格式数据并设置任务级默认参数，
# 实际训练逻辑由 scripts/train/run_verl_uenv_grpo.sh 统一执行。
#
# 常用示例：
#   LIMIT=1000 ./scripts/train/presets/swe_smith_grpo_train.sh
#   ./scripts/train/presets/swe_smith_grpo_train.sh --limit 1000
#   LIMIT=4096 TRAINING_STEPS=512 ./scripts/train/presets/swe_smith_grpo_train.sh
#   ./scripts/train/presets/swe_smith_grpo_train.sh --limit 1000 --prepare-only
#
# 数据选择参数：
#   LIMIT             读取多少条非空 SWE-smith 训练样本；0 表示全量非空样本。默认 1000。
#   OFFSET            跳过多少条非空样本后再读取。默认 0。
#   SWE_PREPARE_DATA  是否重新生成 VeRL parquet。默认 1。

usage() {
  cat <<'EOF'
Run SWE-smith GRPO training with VeRL + UEnv.

Usage:
  ./scripts/train/presets/swe_smith_grpo_train.sh [--limit N] [--offset N] [--prepare-only]

Options:
  --limit N       Number of non-empty SWE-smith training rows to read. 0 means all.
  --offset N      Skip this many non-empty rows before selecting.
  --prepare-only  Generate VeRL parquet data and exit without launching VeRL.
  -h, --help      Show this help.

Environment overrides are also supported. CLI options override LIMIT/OFFSET.
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --limit)
      if [ "$#" -lt 2 ]; then
        echo "--limit requires a value" >&2
        exit 1
      fi
      export LIMIT="$2"
      shift 2
      ;;
    --offset)
      if [ "$#" -lt 2 ]; then
        echo "--offset requires a value" >&2
        exit 1
      fi
      export OFFSET="$2"
      shift 2
      ;;
    --prepare-only)
      export PREPARE_ONLY=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

REPO_DIR=${REPO_DIR:-"$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"}
cd "${REPO_DIR}"

# 运行 ID 与日志。
RUN_TS=${RUN_TS:-$(date +%Y%m%d_%H%M%S)}
export RUN_ID=${RUN_ID:-verl_swesmith_grpo_train_${RUN_TS}}
export LOG_FILE=${LOG_FILE:-${REPO_DIR}/temp/logs/verl_layer4_agent_loop/${RUN_ID}.log}
export WANDB_ENV_FILE=${WANDB_ENV_FILE:-${REPO_DIR}/../../secrets/wandb.env}


# VeRL/wandb 日志。默认只输出 console；开启 wandb 时传入：
#   TRAINER_LOGGER="['console','wandb']" WANDB_MODE=online WANDB_API_KEY=...
# 也可以写入 /data/ronghao/uenv/secrets/wandb.env，由通用入口自动读取。
export TRAINER_LOGGER=${TRAINER_LOGGER:-"['console','wandb']"}
export TRAINER_PROJECT_NAME=${TRAINER_PROJECT_NAME:-uenv_swesmith_grpo_train}
export WANDB_MODE=${WANDB_MODE:-online}
export EXPERIMENT_NAME=${EXPERIMENT_NAME:-qwen3_6_35b_a3b_swesmith_grpo_limit${LIMIT}_${RUN_TS}}

# Server Obs / 前端可视化。默认通过前端机器的 /obs 反代上报事件。
export UENV_OBS_URL=${UENV_OBS_URL-http://8.130.75.157:8888/obs}
export UENV_OBS_TOKEN=${UENV_OBS_TOKEN:-}
export UENV_TRAINING_RUN_ID=${UENV_TRAINING_RUN_ID:-${RUN_ID}}

# Adapter model gateway。Worker/OpenHands 访问该地址，gateway 再转发到 VeRL 内部 vLLM。
export UENV_MODEL_GATEWAY_ENABLED=${UENV_MODEL_GATEWAY_ENABLED:-1}
export UENV_MODEL_GATEWAY_PORT=${UENV_MODEL_GATEWAY_PORT:-18088}
export UENV_MODEL_GATEWAY_PUBLIC_URL=${UENV_MODEL_GATEWAY_PUBLIC_URL:-http://10.10.20.142:${UENV_MODEL_GATEWAY_PORT}/v1}
export UENV_MODEL_GATEWAY_MAX_TOKENS=${UENV_MODEL_GATEWAY_MAX_TOKENS:-4096}

# SWE/OpenHands 训练需要 worker 回传完整 response trace，默认沿用当前 SWE 训练口径。
export UENV_AGENT_LOOP_PARALLEL_MODE=${UENV_AGENT_LOOP_PARALLEL_MODE:-sync}
export UENV_AGENT_LOOP_FAILED_EPISODE_POLICY=${UENV_AGENT_LOOP_FAILED_EPISODE_POLICY:-zero_reward}

# 模型与数据挂载。宿主机路径挂到容器内固定路径，供 VeRL Hydra 参数引用。
export MODEL_PATH=${MODEL_PATH:-/data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B}
export CONTAINER_MODEL_PATH=${CONTAINER_MODEL_PATH:-/models/modelscope/Qwen/Qwen3___6-35B-A3B}
export SWE_RAW_DATA_DIR=${SWE_RAW_DATA_DIR:-${REPO_DIR}/data/benchmarks/swesmith/raw/data}
export LIMIT=${LIMIT:-1000}
export OFFSET=${OFFSET:-0}
export DATA_DIR=${DATA_DIR:-${REPO_DIR}/data/benchmarks/swesmith_train_limit${LIMIT}_offset${OFFSET}}
export CONTAINER_DATA_DIR=${CONTAINER_DATA_DIR:-/data/swesmith_train}

# SWE/OpenHands 数据准备参数。SWE_TRAJECTORY_MAX_STEPS 会同时写入 max_steps 和 max_iterations。
export SWE_PREPARE_DATA=${SWE_PREPARE_DATA:-1}
export SWE_WORKSPACE_DIR=${SWE_WORKSPACE_DIR:-/testbed}
export SWE_LLM_CONFIG_PATH=${SWE_LLM_CONFIG_PATH:-/root/UEnv/config/openhands-llm-qwen3-thinking-max-token-8192.json}
export SWE_TRAJECTORY_MAX_STEPS=${SWE_TRAJECTORY_MAX_STEPS:-30}
export SWE_ENV_PACKAGE_VERSION=${SWE_ENV_PACKAGE_VERSION:-0.1.0-local}
export SWE_AGENT_MODE=${SWE_AGENT_MODE:-llm}

# 训练与数据参数。
export TRAINING_STEPS=${TRAINING_STEPS:-null}
export TOTAL_EPOCHS=${TOTAL_EPOCHS:-1}
export TRAIN_BATCH_SIZE=${TRAIN_BATCH_SIZE:-2}
export PPO_MINI_BATCH_SIZE=${PPO_MINI_BATCH_SIZE:-2}
export PPO_MICRO_BATCH_SIZE_PER_GPU=${PPO_MICRO_BATCH_SIZE_PER_GPU:-1}
export ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=${ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU:-1}
export REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=${REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU:-1}

# rollout 与 GPU 资源参数。默认使用 8 卡 TP 跑 Qwen3.6-35B-A3B。
export ROLLOUT_N=${ROLLOUT_N:-4}
export ROLLOUT_TP=${ROLLOUT_TP:-8}
export NGPUS_PER_NODE=${NGPUS_PER_NODE:-8}
export AGENT_NUM_WORKERS=${AGENT_NUM_WORKERS:-1}
export PODMAN_GPU_ARGS=${PODMAN_GPU_ARGS:-nvidia.com/gpu=all}
export CUDA_VISIBLE_DEVICES_IN_CONTAINER=${CUDA_VISIBLE_DEVICES_IN_CONTAINER:-0,1,2,3,4,5,6,7}

# prompt/response 长度与 vLLM 显存复用参数。
export MAX_PROMPT_LENGTH=${MAX_PROMPT_LENGTH:-8192}
export DATA_MAX_RESPONSE_LENGTH=${DATA_MAX_RESPONSE_LENGTH:-8192}
export ROLLOUT_GPU_MEMORY_UTILIZATION=${ROLLOUT_GPU_MEMORY_UTILIZATION:-0.50}
export ROLLOUT_FREE_CACHE_ENGINE=${ROLLOUT_FREE_CACHE_ENGINE:-True}
export ROLLOUT_ENABLE_SLEEP_MODE=${ROLLOUT_ENABLE_SLEEP_MODE:-True}

# SWE/OpenHands 训练需要 response token trace；当前 worker 通过 OpenAI logprobs
# 恢复 token_id，所以同步 GRPO 也默认要求 rollout 返回 logprobs。
export ROLLOUT_CALCULATE_LOG_PROBS=${ROLLOUT_CALCULATE_LOG_PROBS:-True}
export UENV_REQUIRE_SWE_RESPONSE_TRACE=${UENV_REQUIRE_SWE_RESPONSE_TRACE:-1}
export UENV_PATCH_VERL_TEXT_ONLY_PROCESSOR=${UENV_PATCH_VERL_TEXT_ONLY_PROCESSOR:-1}

# 正式训练默认不做验证；checkpoint 频率可由 SAVE_FREQ 覆盖。
export TEST_FREQ=${TEST_FREQ:--1}
export SAVE_FREQ=${SAVE_FREQ:-50}

# 额外 VeRL Hydra overrides。
if [ -z "${EXTRA_VERL_ARGS:-}" ]; then
  EXTRA_VERL_ARG_LIST=(
    "+ray_kwargs.ray_init.runtime_env.env_vars.VERL_LOGGING_LEVEL=INFO"
    "+ray_kwargs.ray_init.runtime_env.env_vars.VLLM_LOGGING_LEVEL=INFO"
    "+actor_rollout_ref.rollout.max_model_len=65536"
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
  python3 "${REPO_DIR}/scripts/utils/prepare_verl_swesmith_train.py" \
    --input-dir "${SWE_RAW_DATA_DIR}" \
    --output-dir "${DATA_DIR}" \
    --limit "${LIMIT}" \
    --offset "${OFFSET}" \
    --workspace-dir "${SWE_WORKSPACE_DIR}" \
    --llm-config-path "${SWE_LLM_CONFIG_PATH}" \
    --max-steps "${SWE_TRAJECTORY_MAX_STEPS}" \
    --env-package-version "${SWE_ENV_PACKAGE_VERSION}" \
    --agent-mode "${SWE_AGENT_MODE}"
fi

if [ "${PREPARE_ONLY:-0}" = "1" ]; then
  echo "Prepared SWE-smith VeRL data at: ${DATA_DIR}"
  exit 0
fi

# 进入通用 VeRL + UEnv GRPO 训练入口。
exec "${REPO_DIR}/scripts/train/run_verl_uenv_grpo.sh"
