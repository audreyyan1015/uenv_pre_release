#!/usr/bin/env bash
set -euo pipefail

# SWE-smith + VeRL GRPO preset for Ascend 910C hosts.
#
# Default mode is native host execution. It uses the current Python environment
# instead of requiring a Podman image that already contains torch-npu/vLLM-Ascend.

usage() {
  cat <<'EOF'
Run SWE-smith GRPO training with VeRL + UEnv on Ascend.

Usage:
  ./scripts/train/presets/swe_smith_grpo_train_ascend.sh [--limit N] [--offset N] [--prepare-only]

Important environment overrides:
  UENV_EXECUTION_MODE      native or container. Default: native.
  VERL_ROOT                Host VeRL source root. Default: /data/zhongsiqi/uenv-workspace/verl
  VERL_ENV                 Host Python virtualenv. Default: /data/zhongsiqi/uenv-workspace/verl-env
  IMAGE                    Container image, only used with UENV_EXECUTION_MODE=container.
  ASCEND_VISIBLE_DEVICES   Visible NPU ids. Default: 0,1,2,3,4,5,6,7
  ASCEND_RT_VISIBLE_DEVICES Runtime visible NPU ids. Default: ASCEND_VISIBLE_DEVICES
  NGPUS_PER_NODE           VeRL device count field; keep name until VeRL config is
                           fully renamed upstream. Default: number of visible NPUs.
  EXTRA_VERL_ARGS          VeRL Ascend Hydra overrides.
  CHECK_ONLY               1 validates the native command/config without NPU/core.

Options:
  --limit N       Number of non-empty SWE-smith training rows to read. 0 means all.
  --offset N      Skip this many non-empty rows before selecting.
  --prepare-only  Generate VeRL parquet data and exit without launching VeRL.
  -h, --help      Show this help.
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

export UENV_EXECUTION_MODE=${UENV_EXECUTION_MODE:-native}
export VERL_ROOT=${VERL_ROOT:-/data/zhongsiqi/uenv-workspace/verl}
export VERL_ENV=${VERL_ENV:-/data/zhongsiqi/uenv-workspace/verl-env}
PREPARE_PYTHON=${PREPARE_PYTHON:-${VERL_ENV}/bin/python}
if [ ! -x "${PREPARE_PYTHON}" ]; then
  PREPARE_PYTHON=python3
fi

visible_devices=${ASCEND_VISIBLE_DEVICES:-0,1,2,3,4,5,6,7}
device_count=$(python3 - "${visible_devices}" <<'PY'
import sys

items = [item.strip() for item in sys.argv[1].split(",") if item.strip()]
print(max(1, len(items)))
PY
)

export UENV_DEVICE_BACKEND=ascend
export ASCEND_VISIBLE_DEVICES=${ASCEND_VISIBLE_DEVICES:-${visible_devices}}
export ASCEND_RT_VISIBLE_DEVICES=${ASCEND_RT_VISIBLE_DEVICES:-${ASCEND_VISIBLE_DEVICES}}
export TORCH_DEVICE_BACKEND_AUTOLOAD=${TORCH_DEVICE_BACKEND_AUTOLOAD:-0}
export RAY_EXPERIMENTAL_NOSET_ASCEND_RT_VISIBLE_DEVICES=${RAY_EXPERIMENTAL_NOSET_ASCEND_RT_VISIBLE_DEVICES:-1}
export NGPUS_PER_NODE=${NGPUS_PER_NODE:-${device_count}}
export PODMAN_ASCEND_ARGS=${PODMAN_ASCEND_ARGS:-}

export RUN_TS=${RUN_TS:-$(date +%Y%m%d_%H%M%S)}
export RUN_ID=${RUN_ID:-verl_swesmith_grpo_ascend_${RUN_TS}}
export LOG_ROOT=${LOG_ROOT:-/data/zhongsiqi/uenv-workspace/verl-logs}
export LOG_FILE=${LOG_FILE:-${LOG_ROOT}/verl_layer4_agent_loop/${RUN_ID}.log}
export CHECKPOINT_ROOT=${CHECKPOINT_ROOT:-/data/zhongsiqi/uenv-workspace/verl-ckpts/uenv_grpo}
export WANDB_ENV_FILE=${WANDB_ENV_FILE:-${REPO_DIR}/../secrets/wandb.env}

export TRAINER_LOGGER=${TRAINER_LOGGER:-"['console','wandb']"}
export TRAINER_PROJECT_NAME=${TRAINER_PROJECT_NAME:-uenv_swesmith_grpo_train_ascend}
export WANDB_MODE=${WANDB_MODE:-online}
export LIMIT=${LIMIT:-100}
export OFFSET=${OFFSET:-0}
export EXPERIMENT_NAME=${EXPERIMENT_NAME:-qwen3_6_35b_a3b_swesmith_grpo_ascend_limit${LIMIT}_${RUN_TS}}

export UENV_OBS_URL=${UENV_OBS_URL-http://8.130.75.157:8888/obs}
export UENV_OBS_TOKEN=${UENV_OBS_TOKEN:-}
export UENV_TRAINING_RUN_ID=${UENV_TRAINING_RUN_ID:-${RUN_ID}}

export UENV_MODEL_GATEWAY_ENABLED=${UENV_MODEL_GATEWAY_ENABLED:-1}
export UENV_MODEL_GATEWAY_PORT=${UENV_MODEL_GATEWAY_PORT:-18088}
export UENV_MODEL_GATEWAY_PUBLIC_URL=${UENV_MODEL_GATEWAY_PUBLIC_URL:-http://10.10.20.142:${UENV_MODEL_GATEWAY_PORT}/v1}
export UENV_MODEL_GATEWAY_MAX_TOKENS=${UENV_MODEL_GATEWAY_MAX_TOKENS:-4096}

export UENV_AGENT_LOOP_PARALLEL_MODE=${UENV_AGENT_LOOP_PARALLEL_MODE:-sync}
export UENV_AGENT_LOOP_FAILED_EPISODE_POLICY=${UENV_AGENT_LOOP_FAILED_EPISODE_POLICY:-zero_reward}

export MODEL_PATH=${MODEL_PATH:-/data/zhongsiqi/models/modelscope/Qwen/Qwen3___6-35B-A3B}
export SWE_RAW_DATA_DIR=${SWE_RAW_DATA_DIR:-${REPO_DIR}/data/benchmarks/swesmith/raw/data}
export DATA_DIR=${DATA_DIR:-${REPO_DIR}/data/benchmarks/swesmith_train_limit${LIMIT}_offset${OFFSET}}

export SWE_PREPARE_DATA=${SWE_PREPARE_DATA:-0}
export SWE_WORKSPACE_DIR=${SWE_WORKSPACE_DIR:-/testbed}
export SWE_LLM_CONFIG_PATH=${SWE_LLM_CONFIG_PATH:-/root/UEnv/config/openhands-llm-qwen3-thinking-max-token-8192.json}
export SWE_TRAJECTORY_MAX_STEPS=${SWE_TRAJECTORY_MAX_STEPS:-50}
if [ -z "${UENV_EPISODE_MAX_STEPS_OVERRIDE+x}" ]; then
  export UENV_EPISODE_MAX_STEPS_OVERRIDE=${SWE_TRAJECTORY_MAX_STEPS}
fi
export SWE_ENV_PACKAGE_VERSION=${SWE_ENV_PACKAGE_VERSION:-0.1.0-local}
export SWE_AGENT_MODE=${SWE_AGENT_MODE:-llm}

export TRAINING_STEPS=${TRAINING_STEPS:-null}
export TOTAL_EPOCHS=${TOTAL_EPOCHS:-1}
export TRAIN_BATCH_SIZE=${TRAIN_BATCH_SIZE:-2}
export PPO_MINI_BATCH_SIZE=${PPO_MINI_BATCH_SIZE:-2}
export PPO_MICRO_BATCH_SIZE_PER_GPU=${PPO_MICRO_BATCH_SIZE_PER_GPU:-1}
export ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=${ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU:-1}
export REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=${REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU:-1}

export ROLLOUT_N=${ROLLOUT_N:-4}
export ROLLOUT_TP=${ROLLOUT_TP:-${NGPUS_PER_NODE}}
export AGENT_NUM_WORKERS=${AGENT_NUM_WORKERS:-1}

export MAX_PROMPT_LENGTH=${MAX_PROMPT_LENGTH:-8192}
export DATA_MAX_RESPONSE_LENGTH=${DATA_MAX_RESPONSE_LENGTH:-8192}
export ROLLOUT_GPU_MEMORY_UTILIZATION=${ROLLOUT_GPU_MEMORY_UTILIZATION:-0.50}
export ROLLOUT_FREE_CACHE_ENGINE=${ROLLOUT_FREE_CACHE_ENGINE:-True}
export ROLLOUT_ENABLE_SLEEP_MODE=${ROLLOUT_ENABLE_SLEEP_MODE:-True}
export ROLLOUT_CALCULATE_LOG_PROBS=${ROLLOUT_CALCULATE_LOG_PROBS:-True}
export UENV_REQUIRE_SWE_RESPONSE_TRACE=${UENV_REQUIRE_SWE_RESPONSE_TRACE:-1}
export UENV_PATCH_VERL_TEXT_ONLY_PROCESSOR=${UENV_PATCH_VERL_TEXT_ONLY_PROCESSOR:-1}
export UENV_PATCH_TORCH_CUDA_IS_AVAILABLE_NO_DEVICES=0
export UENV_PATCH_VERL_DEVICE_CAPABILITY_FALLBACK=0

export TEST_FREQ=${TEST_FREQ:--1}
export SAVE_FREQ=${SAVE_FREQ:-5}

if [ -z "${EXTRA_VERL_ARGS:-}" ]; then
  EXTRA_VERL_ARG_LIST=(
    "+ray_kwargs.ray_init.runtime_env.env_vars.VERL_LOGGING_LEVEL=INFO"
    "+ray_kwargs.ray_init.runtime_env.env_vars.VLLM_LOGGING_LEVEL=INFO"
    "+actor_rollout_ref.rollout.max_model_len=262144"
    "actor_rollout_ref.rollout.max_num_batched_tokens=65536"
    "actor_rollout_ref.rollout.multi_stage_wake_up=True"
    "actor_rollout_ref.actor.fsdp_config.optimizer_offload=True"
    "+actor_rollout_ref.rollout.engine_kwargs.vllm.enable_auto_tool_choice=True"
    "+actor_rollout_ref.rollout.engine_kwargs.vllm.tool_call_parser=qwen3_coder"
    "actor_rollout_ref.rollout.checkpoint_engine.update_weights_bucket_megabytes=1024"
  )
  if [ "${UENV_EXECUTION_MODE}" = "container" ]; then
    EXTRA_VERL_ARG_LIST+=(
      "+ray_kwargs.ray_init.runtime_env.env_vars.UENV_DEVICE_BACKEND=ascend"
      "+ray_kwargs.ray_init.runtime_env.env_vars.ASCEND_VISIBLE_DEVICES='${ASCEND_VISIBLE_DEVICES}'"
      "+ray_kwargs.ray_init.runtime_env.env_vars.ASCEND_RT_VISIBLE_DEVICES='${ASCEND_RT_VISIBLE_DEVICES}'"
      "+ray_kwargs.ray_init.runtime_env.env_vars.TORCH_DEVICE_BACKEND_AUTOLOAD=${TORCH_DEVICE_BACKEND_AUTOLOAD}"
      "+ray_kwargs.ray_init.runtime_env.env_vars.RAY_EXPERIMENTAL_NOSET_ASCEND_RT_VISIBLE_DEVICES=${RAY_EXPERIMENTAL_NOSET_ASCEND_RT_VISIBLE_DEVICES}"
      "trainer.device=npu"
      "+ray_kwargs.ray_init.resources.NPU=${NGPUS_PER_NODE}"
    )
  fi
  EXTRA_VERL_ARGS="${EXTRA_VERL_ARG_LIST[*]}"
fi
export EXTRA_VERL_ARGS

if [ "${SWE_PREPARE_DATA}" != "0" ]; then
  "${PREPARE_PYTHON}" "${REPO_DIR}/scripts/utils/prepare_verl_swesmith_train.py" \
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

case "${UENV_EXECUTION_MODE}" in
  native)
    exec "${REPO_DIR}/scripts/train/run_verl_uenv_grpo_native_ascend.sh"
    ;;
  container)
    export IMAGE=${IMAGE:-localhost/uenv-bridge-verl:ascend910c}
    export CONTAINER_MODEL_PATH=${CONTAINER_MODEL_PATH:-/models/modelscope/Qwen/Qwen3___6-35B-A3B}
    export CONTAINER_DATA_DIR=${CONTAINER_DATA_DIR:-/data/swesmith_train}
    exec "${REPO_DIR}/scripts/train/run_verl_uenv_grpo.sh"
    ;;
  *)
    echo "Unsupported UENV_EXECUTION_MODE=${UENV_EXECUTION_MODE}; expected native or container." >&2
    exit 1
    ;;
esac
