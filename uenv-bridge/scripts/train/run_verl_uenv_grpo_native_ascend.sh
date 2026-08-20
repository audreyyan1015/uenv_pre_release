#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Run VeRL GRPO with the UEnv AgentLoop bridge on the host Ascend environment.

This is the native-host counterpart of run_verl_uenv_grpo.sh. It does not use
Podman or an IMAGE. It expects torch-npu/vLLM-Ascend to be installed in the
host Python environment and connects to an already running server-side Rust
adapter core.

Usage:
  SERVER_ADAPTER_CORE_ENDPOINT=<server-core-host:port> ./scripts/train/run_verl_uenv_grpo_native_ascend.sh

Quick dry check without NPU or adapter core:
  CHECK_ONLY=1 \
  DATA_DIR=/data/zhongsiqi/uenv-workspace/verl-data/smoke_gsm8k \
  SERVER_ADAPTER_CORE_ENDPOINT=127.0.0.1:50051 \
  ./scripts/train/run_verl_uenv_grpo_native_ascend.sh

Common environment overrides:
  VERL_ROOT                    Host VeRL source root. Default: /data/zhongsiqi/uenv-workspace/verl
  VERL_ENV                     Host Python virtualenv. Default: /data/zhongsiqi/uenv-workspace/verl-env
  MODEL_PATH                   Host policy model path. Default: /data/zhongsiqi/models/modelscope/Qwen/Qwen3___6-35B-A3B
  DATA_DIR                     Host VeRL-format train/val data dir. Default: /data/zhongsiqi/uenv-workspace/verl-data/smoke_gsm8k
  CHECKPOINT_ROOT              Host checkpoint root. Default: /data/zhongsiqi/uenv-workspace/verl-ckpts/uenv_grpo
  LOG_ROOT                     Host run log root. Default: /data/zhongsiqi/uenv-workspace/verl-logs
  ASCEND_VISIBLE_DEVICES       Visible NPU ids. Default: 0
  ASCEND_RT_VISIBLE_DEVICES    Runtime visible NPU ids. Default: ASCEND_VISIBLE_DEVICES
  NGPUS_PER_NODE               VeRL device count field. Default: number of visible NPU ids.
  CHECK_ONLY                   1 prints and validates the command without requiring NPU/core. Default: 0
  EXTRA_VERL_ARGS              Extra Hydra overrides appended to the VeRL command.
EOF
}

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
  usage
  exit 0
fi

REPO_DIR=${REPO_DIR:-"$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"}
source "${REPO_DIR}/scripts/lib/common.sh"

count_visible_devices() {
  local visible="$1"
  local count=0
  local item
  local old_ifs="${IFS}"
  IFS=","
  for item in ${visible}; do
    item="$(printf '%s' "${item}" | tr -d '[:space:]')"
    if [ -n "${item}" ] && [ "${item}" != "NoDevFiles" ]; then
      count=$((count + 1))
    fi
  done
  IFS="${old_ifs}"
  if [ "${count}" -lt 1 ]; then
    count=1
  fi
  printf '%s\n' "${count}"
}

source_env_file() {
  local path="$1"
  local description="$2"
  if [ ! -f "${path}" ]; then
    echo "Skipping missing ${description}: ${path}"
    return 0
  fi

  local shell_opts="$-"
  set +e
  set +u
  # shellcheck disable=SC1090
  source "${path}"
  local status=$?
  case "${shell_opts}" in
    *e*) set -e ;;
    *) set +e ;;
  esac
  case "${shell_opts}" in
    *u*) set -u ;;
    *) set +u ;;
  esac
  if [ "${status}" -ne 0 ]; then
    echo "Failed to source ${description}: ${path}" >&2
    return "${status}"
  fi
}

validate_parquet_data() {
  ensure_file_exists "${DATA_DIR}/train.parquet" "Training parquet not found"
  ensure_file_exists "${DATA_DIR}/test.parquet" "Validation parquet not found"
  "${PYTHON_BIN}" - "${DATA_DIR}" <<'PY'
import sys
from pathlib import Path

import pyarrow.parquet as pq

data_dir = Path(sys.argv[1])
for name in ("train.parquet", "test.parquet"):
    path = data_dir / name
    metadata = pq.ParquetFile(path).metadata
    print(f"{path}: rows={metadata.num_rows} row_groups={metadata.num_row_groups}")
    if metadata.num_rows <= 0:
        raise SystemExit(f"{path} has no rows")
PY
}

run_import_check() {
  "${PYTHON_BIN}" <<'PY'
from importlib import metadata

import torch
import torch_npu  # noqa: F401
import vllm
import vllm_ascend  # noqa: F401
import verl
from uenv.bridge.verl_agent_loop import UEnvAgentLoop

print("torch", torch.__version__)
print("torch-npu", metadata.version("torch-npu"))
print("vllm", vllm.__version__)
print("vllm-ascend", metadata.version("vllm-ascend"))
print("verl", metadata.version("verl"))
print("torch.npu.is_available", torch.npu.is_available())
print("torch.npu.device_count", torch.npu.device_count())
print("uenv_agent_loop", UEnvAgentLoop.__name__)
PY
}

require_npu_available() {
  "${PYTHON_BIN}" - "${NGPUS_PER_NODE}" <<'PY'
import sys

import torch
import torch_npu  # noqa: F401

requested = int(sys.argv[1])
available = torch.npu.is_available()
count = torch.npu.device_count()
print(f"torch.npu.is_available={available}")
print(f"torch.npu.device_count={count}")
if not available or count <= 0:
    print("No NPU is available; set CHECK_ONLY=1 for a dry validation on this host.", file=sys.stderr)
    sys.exit(1)
if count < requested:
    print(f"Configured {requested} NPU workers, but only {count} devices are visible.", file=sys.stderr)
    sys.exit(1)
PY
}

print_command() {
  printf '%q ' "$@"
  printf '\n'
}

CHECK_ONLY=${CHECK_ONLY:-0}
VERL_ROOT=${VERL_ROOT:-/data/zhongsiqi/uenv-workspace/verl}
VERL_ENV=${VERL_ENV:-/data/zhongsiqi/uenv-workspace/verl-env}
PYTHON_BIN=${PYTHON_BIN:-${VERL_ENV}/bin/python}
UENV_DEVICE_BACKEND=${UENV_DEVICE_BACKEND:-ascend}
UENV_DEVICE_BACKEND=$(normalize_device_backend "${UENV_DEVICE_BACKEND}")
if [ "${UENV_DEVICE_BACKEND}" != "ascend" ]; then
  echo "run_verl_uenv_grpo_native_ascend.sh only supports UENV_DEVICE_BACKEND=ascend." >&2
  exit 1
fi

SERVER_ADAPTER_CORE_ENDPOINT=${SERVER_ADAPTER_CORE_ENDPOINT:-8.130.75.157:8088}
if [ -z "${SERVER_ADAPTER_CORE_ENDPOINT}" ]; then
  echo "SERVER_ADAPTER_CORE_ENDPOINT is required." >&2
  exit 1
fi

MODEL_PATH=${MODEL_PATH:-/data/zhongsiqi/models/modelscope/Qwen/Qwen3___6-35B-A3B}
DATA_DIR=${DATA_DIR:-/data/zhongsiqi/uenv-workspace/verl-data/smoke_gsm8k}
INFER_BACKEND=${INFER_BACKEND:-vllm}

TRAINING_STEPS=${TRAINING_STEPS:-null}
TRAIN_BATCH_SIZE=${TRAIN_BATCH_SIZE:-256}
PPO_MINI_BATCH_SIZE=${PPO_MINI_BATCH_SIZE:-64}
PPO_MICRO_BATCH_SIZE_PER_GPU=${PPO_MICRO_BATCH_SIZE_PER_GPU:-2}
ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=${ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU:-4}
REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=${REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU:-${ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU}}
MAX_PROMPT_LENGTH=${MAX_PROMPT_LENGTH:-512}
ROLLOUT_N=${ROLLOUT_N:-5}
ROLLOUT_TP=${ROLLOUT_TP:-1}
ROLLOUT_CALCULATE_LOG_PROBS=${ROLLOUT_CALCULATE_LOG_PROBS:-False}
DATA_MAX_RESPONSE_LENGTH=${DATA_MAX_RESPONSE_LENGTH:-1024}

ROLLOUT_FREE_CACHE_ENGINE=${ROLLOUT_FREE_CACHE_ENGINE:-False}
ROLLOUT_ENABLE_SLEEP_MODE=${ROLLOUT_ENABLE_SLEEP_MODE:-False}
ROLLOUT_GPU_MEMORY_UTILIZATION=${ROLLOUT_GPU_MEMORY_UTILIZATION:-0.8}
AGENT_NUM_WORKERS=${AGENT_NUM_WORKERS:-1}
ASCEND_VISIBLE_DEVICES=${ASCEND_VISIBLE_DEVICES:-0}
ASCEND_RT_VISIBLE_DEVICES=${ASCEND_RT_VISIBLE_DEVICES:-${ASCEND_VISIBLE_DEVICES}}
TORCH_DEVICE_BACKEND_AUTOLOAD=${TORCH_DEVICE_BACKEND_AUTOLOAD:-0}
RAY_EXPERIMENTAL_NOSET_ASCEND_RT_VISIBLE_DEVICES=${RAY_EXPERIMENTAL_NOSET_ASCEND_RT_VISIBLE_DEVICES:-1}
NGPUS_PER_NODE=${NGPUS_PER_NODE:-$(count_visible_devices "${ASCEND_VISIBLE_DEVICES}")}
RAY_NUM_CPUS=${RAY_NUM_CPUS:-$((NGPUS_PER_NODE * 4))}

UENV_PATCH_RESOURCE_TRACKER=${UENV_PATCH_RESOURCE_TRACKER:-1}
UENV_PATCH_VERL_VLLM_SHUTDOWN=${UENV_PATCH_VERL_VLLM_SHUTDOWN:-1}
UENV_PATCH_VERL_MODEL_VERSION_RESPONSE=${UENV_PATCH_VERL_MODEL_VERSION_RESPONSE:-1}
UENV_PATCH_VERL_TEXT_ONLY_PROCESSOR=${UENV_PATCH_VERL_TEXT_ONLY_PROCESSOR:-0}
case "${UENV_PATCH_VERL_TEXT_ONLY_PROCESSOR}" in
  1|true|True|enabled|yes|on)
    UENV_PATCH_VERL_TEXT_ONLY_PROCESSOR_RAY=enabled
    ;;
  *)
    UENV_PATCH_VERL_TEXT_ONLY_PROCESSOR_RAY=disabled
    ;;
esac
UENV_AGENT_LOOP_BATCH=${UENV_AGENT_LOOP_BATCH:-1}
UENV_AGENT_LOOP_BATCH_SIZE=${UENV_AGENT_LOOP_BATCH_SIZE:-0}
UENV_AGENT_LOOP_BATCH_RETRY_ATTEMPTS=${UENV_AGENT_LOOP_BATCH_RETRY_ATTEMPTS:-3}
UENV_AGENT_LOOP_BATCH_RETRY_DELAY_SECONDS=${UENV_AGENT_LOOP_BATCH_RETRY_DELAY_SECONDS:-5}
UENV_AGENT_LOOP_PARALLEL_MODE=${UENV_AGENT_LOOP_PARALLEL_MODE:-sync}
UENV_AGENT_LOOP_TIMEOUT_SECONDS=${UENV_AGENT_LOOP_TIMEOUT_SECONDS:-3600}
UENV_ADAPTER_CORE_GRPC_MAX_MESSAGE_BYTES=${UENV_ADAPTER_CORE_GRPC_MAX_MESSAGE_BYTES:-16777216}
UENV_EPISODE_MAX_STEPS_OVERRIDE=${UENV_EPISODE_MAX_STEPS_OVERRIDE:-}
UENV_OBS_URL=${UENV_OBS_URL:-}
UENV_OBS_TOKEN=${UENV_OBS_TOKEN:-}
UENV_MODEL_GATEWAY_ENABLED=${UENV_MODEL_GATEWAY_ENABLED:-0}
UENV_MODEL_GATEWAY_BIND_HOST=${UENV_MODEL_GATEWAY_BIND_HOST:-0.0.0.0}
UENV_MODEL_GATEWAY_PORT=${UENV_MODEL_GATEWAY_PORT:-18080}
UENV_MODEL_GATEWAY_PUBLIC_URL=${UENV_MODEL_GATEWAY_PUBLIC_URL:-http://10.10.20.142:${UENV_MODEL_GATEWAY_PORT}/v1}
UENV_MODEL_GATEWAY_DISABLE_THINKING=${UENV_MODEL_GATEWAY_DISABLE_THINKING:-0}
UENV_MODEL_GATEWAY_MAX_TOKENS=${UENV_MODEL_GATEWAY_MAX_TOKENS:-}
UENV_REQUIRE_SWE_RESPONSE_TRACE=${UENV_REQUIRE_SWE_RESPONSE_TRACE:-1}
UENV_AGENT_LOOP_FAILED_EPISODE_POLICY=${UENV_AGENT_LOOP_FAILED_EPISODE_POLICY:-raise}
if [ -z "${UENV_MODEL_GATEWAY_STOP_ON_CLOSE+x}" ]; then
  if [ "${AGENT_NUM_WORKERS}" -gt 1 ]; then
    UENV_MODEL_GATEWAY_STOP_ON_CLOSE=0
  else
    UENV_MODEL_GATEWAY_STOP_ON_CLOSE=1
  fi
fi
EXTRA_VERL_ARGS=${EXTRA_VERL_ARGS:-}
EXTRA_VERL_ARGS=${EXTRA_VERL_ARGS//$'\n'/ }
ACTOR_LR=${ACTOR_LR:-1e-6}
KL_LOSS_COEF=${KL_LOSS_COEF:-0.001}
TOTAL_EPOCHS=${TOTAL_EPOCHS:-15}
SAVE_FREQ=${SAVE_FREQ:--1}
TEST_FREQ=${TEST_FREQ:-5}

WANDB_ENV_FILE=${WANDB_ENV_FILE:-${REPO_DIR}/../secrets/wandb.env}
if [ -f "${WANDB_ENV_FILE}" ]; then
  # shellcheck disable=SC1090
  source "${WANDB_ENV_FILE}"
fi
TRAINER_LOGGER=${TRAINER_LOGGER:-"['console']"}
TRAINER_PROJECT_NAME=${TRAINER_PROJECT_NAME:-uenv_bridge_layer4}
EXPERIMENT_NAME=${EXPERIMENT_NAME:-uenv_layer4_grpo_native_ascend_$(date +%Y%m%d_%H%M)}
for wandb_var in WANDB_API_KEY WANDB_MODE WANDB_ENTITY WANDB_DIR WANDB_BASE_URL; do
  wandb_value=${!wandb_var:-}
  if [ -n "${wandb_value}" ]; then
    export "${wandb_var}=${wandb_value}"
  else
    unset "${wandb_var}" || true
  fi
done

RUN_ID=${RUN_ID:-layer4_native_ascend_$(date +%Y%m%d_%H%M%S)}
UENV_TRAINING_RUN_ID=${UENV_TRAINING_RUN_ID:-${RUN_ID}}
LOG_ROOT=${LOG_ROOT:-/data/zhongsiqi/uenv-workspace/verl-logs}
SERVICE_DIR=${SERVICE_DIR:-${LOG_ROOT}/layer4_distributed/${RUN_ID}}
LOG_DIR=${LOG_DIR:-${LOG_ROOT}/verl_layer4_agent_loop}
LOG_FILE=${LOG_FILE:-${LOG_DIR}/${RUN_ID}.log}
CHECKPOINT_ROOT=${CHECKPOINT_ROOT:-/data/zhongsiqi/uenv-workspace/verl-ckpts/uenv_grpo}
CHECKPOINT_RUN_DIR=${CHECKPOINT_RUN_DIR:-${CHECKPOINT_ROOT}/${RUN_ID}}
AGENT_LOOP_RESULT_RECORD_PATH=${AGENT_LOOP_RESULT_RECORD_PATH:-${SERVICE_DIR}/agent-loop-results.jsonl}
AGENT_LOOP_REQUEST_RECORD_PATH=${AGENT_LOOP_REQUEST_RECORD_PATH:-${SERVICE_DIR}/agent-loop-requests.jsonl}
MODEL_GATEWAY_LOG_PATH=${MODEL_GATEWAY_LOG_PATH:-${SERVICE_DIR}/model-gateway.jsonl}
HYDRA_RUN_DIR=${HYDRA_RUN_DIR:-${LOG_DIR}/hydra_${RUN_ID}}

ensure_path "${VERL_ROOT}" "VeRL root not found"
ensure_file_exists "${PYTHON_BIN}" "Python interpreter not found"
ensure_file_exists "${REPO_DIR}/scripts/run_verl_main_ppo.py" "VeRL PPO wrapper not found"
ensure_file_exists "${REPO_DIR}/configs/uenv-agent-loop.yaml" "UEnv AgentLoop config not found"
ensure_policy_model_exists "${MODEL_PATH}"
validate_parquet_data

mkdir -p "${LOG_DIR}" "${SERVICE_DIR}" "${CHECKPOINT_RUN_DIR}" "${HYDRA_RUN_DIR}"
write_json_metadata "${CHECKPOINT_RUN_DIR}/metadata.json" \
  "run_id=${RUN_ID}" \
  "training_run_id=${UENV_TRAINING_RUN_ID}" \
  "script=run_verl_uenv_grpo_native_ascend.sh" \
  "device_backend=${UENV_DEVICE_BACKEND}" \
  "model_path=${MODEL_PATH}" \
  "data_dir=${DATA_DIR}" \
  "infer_backend=${INFER_BACKEND}" \
  "train_batch_size=${TRAIN_BATCH_SIZE}" \
  "ppo_mini_batch_size=${PPO_MINI_BATCH_SIZE}" \
  "rollout_n=${ROLLOUT_N}" \
  "rollout_tp=${ROLLOUT_TP}" \
  "ngpus_per_node=${NGPUS_PER_NODE}" \
  "max_prompt_length=${MAX_PROMPT_LENGTH}" \
  "data_max_response_length=${DATA_MAX_RESPONSE_LENGTH}" \
  "training_steps=${TRAINING_STEPS}" \
  "total_epochs=${TOTAL_EPOCHS}" \
  "save_freq=${SAVE_FREQ}" \
  "test_freq=${TEST_FREQ}" \
  "checkpoint_run_dir=${CHECKPOINT_RUN_DIR}"

export TORCH_DEVICE_BACKEND_AUTOLOAD
source_env_file "/usr/local/Ascend/ascend-toolkit/set_env.sh" "Ascend Toolkit environment"
source_env_file "${VERL_ENV}/bin/activate" "VeRL Python environment"
source_env_file "/usr/local/Ascend/nnal/atb/set_env.sh" "Ascend ATB environment"
if [ -d /usr/local/Ascend/driver/lib64/driver ]; then
  export LD_LIBRARY_PATH=/usr/local/Ascend/driver/lib64/driver:${LD_LIBRARY_PATH:-}
fi

export PYTHONPATH="${VERL_ROOT}:${REPO_DIR}/src${PYTHONPATH:+:${PYTHONPATH}}"
export PROTOCOL_BUFFERS_PYTHON_IMPLEMENTATION=python
export VERL_PLATFORM=${VERL_PLATFORM:-huawei}
export VLLM_USE_V1=${VLLM_USE_V1:-1}
export VLLM_ALLREDUCE_USE_SYMM_MEM=${VLLM_ALLREDUCE_USE_SYMM_MEM:-0}
export VLLM_NO_USAGE_STATS=${VLLM_NO_USAGE_STATS:-1}
export TOKENIZERS_PARALLELISM=${TOKENIZERS_PARALLELISM:-false}
export HYDRA_FULL_ERROR=${HYDRA_FULL_ERROR:-1}
export RAY_DEDUP_LOGS=${RAY_DEDUP_LOGS:-0}
export RAY_EXPERIMENTAL_NOSET_ASCEND_RT_VISIBLE_DEVICES
export RAY_EXPERIMENTAL_NOSET_CUDA_VISIBLE_DEVICES=${RAY_EXPERIMENTAL_NOSET_CUDA_VISIBLE_DEVICES:-1}
export OMP_NUM_THREADS=${OMP_NUM_THREADS:-1}
export MKL_NUM_THREADS=${MKL_NUM_THREADS:-1}
export TORCHINDUCTOR_COMPILE_THREADS=${TORCHINDUCTOR_COMPILE_THREADS:-1}
export ASCEND_VISIBLE_DEVICES
export ASCEND_RT_VISIBLE_DEVICES
unset CUDA_VISIBLE_DEVICES || true

export UENV_PATCH_RESOURCE_TRACKER
export UENV_PATCH_VERL_VLLM_SHUTDOWN
export UENV_PATCH_VERL_MODEL_VERSION_RESPONSE
export UENV_PATCH_VERL_TEXT_ONLY_PROCESSOR
export UENV_DEVICE_BACKEND
export UENV_AGENT_LOOP_BATCH
export UENV_AGENT_LOOP_BATCH_SIZE
export UENV_AGENT_LOOP_BATCH_RETRY_ATTEMPTS
export UENV_AGENT_LOOP_BATCH_RETRY_DELAY_SECONDS
export UENV_AGENT_LOOP_PARALLEL_MODE
export UENV_AGENT_LOOP_TIMEOUT_SECONDS
export UENV_EPISODE_MAX_STEPS_OVERRIDE
export UENV_OBS_URL
export UENV_OBS_TOKEN
export UENV_TRAINING_RUN_ID
export UENV_MODEL_GATEWAY_ENABLED
export UENV_MODEL_GATEWAY_BIND_HOST
export UENV_MODEL_GATEWAY_PORT
export UENV_MODEL_GATEWAY_PUBLIC_URL
export UENV_MODEL_GATEWAY_LOG_PATH
export UENV_MODEL_GATEWAY_DISABLE_THINKING
export UENV_MODEL_GATEWAY_MAX_TOKENS
export UENV_MODEL_GATEWAY_STOP_ON_CLOSE
export UENV_REQUIRE_SWE_RESPONSE_TRACE
export UENV_AGENT_LOOP_FAILED_EPISODE_POLICY
export UENV_AGENT_LOOP_CLIENT=rust_core
export UENV_ADAPTER_CORE_ENDPOINT=${SERVER_ADAPTER_CORE_ENDPOINT}
export UENV_ADAPTER_CORE_AUTO_START=0
export UENV_ADAPTER_CORE_BINARY=${UENV_ADAPTER_CORE_BINARY:-${REPO_DIR}/core/target/debug/uenv-adapter-core}
export UENV_ADAPTER_CORE_STARTUP_TIMEOUT_SECONDS=${UENV_ADAPTER_CORE_STARTUP_TIMEOUT_SECONDS:-60}
export UENV_ADAPTER_CORE_BACKEND=server
export UENV_ADAPTER_CORE_GRPC_MAX_MESSAGE_BYTES
export UENV_AGENT_LOOP_REQUEST_RECORD_PATH
export UENV_AGENT_LOOP_RESULT_RECORD_PATH

run_import_check

VERL_CMD=(
  "${PYTHON_BIN}" "${REPO_DIR}/scripts/run_verl_main_ppo.py"
  "hydra.run.dir=${HYDRA_RUN_DIR}"
  "algorithm.adv_estimator=grpo"
  "algorithm.use_kl_in_reward=False"
  "data.train_files=${DATA_DIR}/train.parquet"
  "data.val_files=${DATA_DIR}/test.parquet"
  "data.train_batch_size=${TRAIN_BATCH_SIZE}"
  "data.max_prompt_length=${MAX_PROMPT_LENGTH}"
  "data.max_response_length=${DATA_MAX_RESPONSE_LENGTH}"
  "data.filter_overlong_prompts=True"
  "data.truncation='error'"
  "data.return_raw_chat=True"
  "data.dataloader_num_workers=0"
  "actor_rollout_ref.model.path=${MODEL_PATH}"
  "actor_rollout_ref.model.use_remove_padding=True"
  "actor_rollout_ref.model.enable_gradient_checkpointing=True"
  "actor_rollout_ref.model.enable_activation_offload=True"
  "actor_rollout_ref.actor.strategy=fsdp"
  "actor_rollout_ref.actor.optim.lr=${ACTOR_LR}"
  "actor_rollout_ref.actor.ppo_mini_batch_size=${PPO_MINI_BATCH_SIZE}"
  "actor_rollout_ref.actor.ppo_micro_batch_size_per_gpu=${PPO_MICRO_BATCH_SIZE_PER_GPU}"
  "actor_rollout_ref.actor.use_dynamic_bsz=False"
  "actor_rollout_ref.actor.use_kl_loss=True"
  "actor_rollout_ref.actor.kl_loss_coef=${KL_LOSS_COEF}"
  "actor_rollout_ref.actor.kl_loss_type=low_var_kl"
  "actor_rollout_ref.actor.entropy_coeff=0"
  "actor_rollout_ref.actor.use_torch_compile=False"
  "actor_rollout_ref.actor.fsdp_config.param_offload=True"
  "actor_rollout_ref.actor.fsdp_config.optimizer_offload=True"
  "actor_rollout_ref.actor.fsdp_config.use_torch_compile=False"
  "actor_rollout_ref.actor.fsdp_config.model_dtype=bf16"
  "actor_rollout_ref.rollout.name=${INFER_BACKEND}"
  "actor_rollout_ref.rollout.disable_log_stats=False"
  "actor_rollout_ref.rollout.tensor_model_parallel_size=${ROLLOUT_TP}"
  "actor_rollout_ref.rollout.gpu_memory_utilization=${ROLLOUT_GPU_MEMORY_UTILIZATION}"
  "actor_rollout_ref.rollout.n=${ROLLOUT_N}"
  "actor_rollout_ref.rollout.agent.num_workers=${AGENT_NUM_WORKERS}"
  "actor_rollout_ref.rollout.agent.default_agent_loop=uenv_agent"
  "actor_rollout_ref.rollout.agent.agent_loop_config_path=${REPO_DIR}/configs/uenv-agent-loop.yaml"
  "actor_rollout_ref.rollout.log_prob_micro_batch_size_per_gpu=${ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU}"
  "actor_rollout_ref.rollout.enforce_eager=False"
  "actor_rollout_ref.rollout.enable_chunked_prefill=True"
  "actor_rollout_ref.rollout.free_cache_engine=${ROLLOUT_FREE_CACHE_ENGINE}"
  "+actor_rollout_ref.rollout.enable_sleep_mode=${ROLLOUT_ENABLE_SLEEP_MODE}"
  "actor_rollout_ref.rollout.max_num_seqs=8"
  "actor_rollout_ref.rollout.max_num_batched_tokens=2048"
  "actor_rollout_ref.rollout.calculate_log_probs=${ROLLOUT_CALCULATE_LOG_PROBS}"
  "actor_rollout_ref.ref.log_prob_micro_batch_size_per_gpu=${REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU}"
  "actor_rollout_ref.ref.fsdp_config.param_offload=True"
  "actor_rollout_ref.ref.fsdp_config.use_torch_compile=False"
  "actor_rollout_ref.ref.use_torch_compile=False"
  "reward.reward_manager.name=naive"
  "reward.num_workers=1"
  "trainer.critic_warmup=0"
  "trainer.balance_batch=True"
  "trainer.device=npu"
  "trainer.logger=${TRAINER_LOGGER}"
  "trainer.project_name=${TRAINER_PROJECT_NAME}"
  "trainer.experiment_name=${EXPERIMENT_NAME}"
  "trainer.n_gpus_per_node=${NGPUS_PER_NODE}"
  "trainer.nnodes=1"
  "trainer.save_freq=${SAVE_FREQ}"
  "trainer.test_freq=${TEST_FREQ}"
  "trainer.val_before_train=False"
  "trainer.total_training_steps=${TRAINING_STEPS}"
  "trainer.total_epochs=${TOTAL_EPOCHS}"
  "trainer.resume_mode=disable"
  "trainer.default_local_dir=${CHECKPOINT_RUN_DIR}"
  "ray_kwargs.ray_init.num_cpus=${RAY_NUM_CPUS}"
  "+ray_kwargs.ray_init.resources.NPU=${NGPUS_PER_NODE}"
  "+ray_kwargs.ray_init.runtime_env.env_vars.PYTHONPATH=${VERL_ROOT}:${REPO_DIR}/src"
  "+ray_kwargs.ray_init.runtime_env.env_vars.PROTOCOL_BUFFERS_PYTHON_IMPLEMENTATION=python"
  "+ray_kwargs.ray_init.runtime_env.env_vars.VERL_PLATFORM=${VERL_PLATFORM}"
  "+ray_kwargs.ray_init.runtime_env.env_vars.UENV_DEVICE_BACKEND=${UENV_DEVICE_BACKEND}"
  "+ray_kwargs.ray_init.runtime_env.env_vars.ASCEND_VISIBLE_DEVICES='${ASCEND_VISIBLE_DEVICES}'"
  "+ray_kwargs.ray_init.runtime_env.env_vars.ASCEND_RT_VISIBLE_DEVICES='${ASCEND_RT_VISIBLE_DEVICES}'"
  "+ray_kwargs.ray_init.runtime_env.env_vars.TORCH_DEVICE_BACKEND_AUTOLOAD=${TORCH_DEVICE_BACKEND_AUTOLOAD}"
  "+ray_kwargs.ray_init.runtime_env.env_vars.RAY_EXPERIMENTAL_NOSET_ASCEND_RT_VISIBLE_DEVICES=${RAY_EXPERIMENTAL_NOSET_ASCEND_RT_VISIBLE_DEVICES}"
  "+ray_kwargs.ray_init.runtime_env.env_vars.UENV_PATCH_VERL_MODEL_VERSION_RESPONSE=enabled"
  "+ray_kwargs.ray_init.runtime_env.env_vars.UENV_PATCH_VERL_TEXT_ONLY_PROCESSOR=${UENV_PATCH_VERL_TEXT_ONLY_PROCESSOR_RAY}"
  "+ray_kwargs.ray_init.include_dashboard=False"
)

if [ -n "${EXTRA_VERL_ARGS}" ]; then
  # Keep compatibility with the existing container script, where EXTRA_VERL_ARGS
  # is a whitespace-separated list of Hydra overrides.
  # shellcheck disable=SC2206
  EXTRA_VERL_ARG_ARRAY=(${EXTRA_VERL_ARGS})
  VERL_CMD+=("${EXTRA_VERL_ARG_ARRAY[@]}")
fi

echo "Native Ascend VeRL command:"
print_command "${VERL_CMD[@]}"
echo "VeRL log: ${LOG_FILE}"
echo "AgentLoop request records: ${AGENT_LOOP_REQUEST_RECORD_PATH}"
echo "AgentLoop result records: ${AGENT_LOOP_RESULT_RECORD_PATH}"
echo "Checkpoint dir: ${CHECKPOINT_RUN_DIR}"
if [ -n "${UENV_OBS_URL}" ]; then
  echo "Frontend run: ${UENV_OBS_URL%/obs}/?run=${UENV_TRAINING_RUN_ID}"
fi

if [ "${CHECK_ONLY}" = "1" ]; then
  echo "CHECK_ONLY=1: validating Hydra config composition without NPU/core."
  CHECK_CMD=("${VERL_CMD[@]:0:2}" "--cfg" "job" "${VERL_CMD[@]:2}")
  set +e
  (
    cd "${VERL_ROOT}"
    UENV_PATCH_VERL_MODEL_VERSION_RESPONSE=0 "${CHECK_CMD[@]}"
  ) >"${LOG_FILE}" 2>&1
  check_status=$?
  set -e
  if [ "${check_status}" -ne 0 ]; then
    echo "Hydra config check failed. Log: ${LOG_FILE}" >&2
    tail -120 "${LOG_FILE}" >&2 2>/dev/null || true
    exit "${check_status}"
  fi
  echo "Hydra config check passed. Log: ${LOG_FILE}"
  exit 0
fi

require_npu_available
wait_for_addr "server-side adapter core" "${SERVER_ADAPTER_CORE_ENDPOINT}" 20

set +e
(cd "${VERL_ROOT}" && "${VERL_CMD[@]}") 2>&1 | tee "${LOG_FILE}"
run_status=${PIPESTATUS[0]}
set -e

if [ "${run_status}" -ne 0 ]; then
  echo "VeRL UEnv GRPO native Ascend training failed. VeRL log: ${LOG_FILE}" >&2
  tail -120 "${LOG_FILE}" >&2 2>/dev/null || true
  exit "${run_status}"
fi

echo "VeRL UEnv GRPO native Ascend training completed."
echo "VeRL log: ${LOG_FILE}"
grep -E "Training Progress: 100%|critic/score/mean|critic/rewards/mean" "${LOG_FILE}" | tail -5 || true
