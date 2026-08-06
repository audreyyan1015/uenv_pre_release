#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Run native VeRL GRPO on GSM8K train/test data.

This script does not connect to UEnv Server/Worker, does not start the adapter
model gateway, and does not enable UEnvAgentLoop. It runs VeRL's native
main_ppo trainer with GSM8K rule reward.

Usage:
  ./scripts/train/run_verl_gsm8k_grpo.sh

Quick smoke example:
  TRAINING_STEPS=2 \
  TRAIN_BATCH_SIZE=2 \
  PPO_MINI_BATCH_SIZE=2 \
  ROLLOUT_N=4 \
  TEST_FREQ=1 \
  ./scripts/train/run_verl_gsm8k_grpo.sh

One-epoch full GSM8K example:
  TRAINING_STEPS=null \
  TOTAL_EPOCHS=1 \
  TRAIN_MAX_SAMPLES=-1 \
  VAL_MAX_SAMPLES=-1 \
  ./scripts/train/run_verl_gsm8k_grpo.sh

Common environment overrides:
  IMAGE                         VeRL image.
  VERL_WORKSPACE                Host VeRL workspace. Default: /data/podman/verl/workspace
  MODEL_PATH                    Host policy model path.
  CONTAINER_MODEL_PATH          Container policy model path.
  DATA_DIR                      Host GSM8K VeRL-format data dir. Default: <repo>/data/gsm8k
  CONTAINER_DATA_DIR            Container GSM8K data dir. Default: /data/gsm8k
  TRAINING_STEPS                Positive integer, or null for total_epochs. Default: 10
  TOTAL_EPOCHS                  Used when TRAINING_STEPS=null. Default: 1
  TRAIN_MAX_SAMPLES             Limit train rows; -1 means all. Default: -1
  VAL_MAX_SAMPLES               Limit eval rows; -1 means all. Default: -1
  VAL_BEFORE_TRAIN              Evaluate before the first training step. Default: True
  VAL_ONLY                      Evaluate and exit. Default: False
  TEST_FREQ                     Eval every N training steps; -1 disables periodic eval. Default: 5
  TRAIN_BATCH_SIZE              Prompt batch size per GRPO step. Default: 2
  PPO_MINI_BATCH_SIZE           PPO mini batch size before rollout expansion. Default: 2
  ROLLOUT_N                     Number of sampled responses per prompt. Default: 4
  ROLLOUT_TP                    vLLM tensor parallel size. Default: 8
  DATA_MAX_RESPONSE_LENGTH      Max generated response tokens. Default: 1024
  ROLLOUT_MAX_MODEL_LEN         vLLM max model length. Default: 2048
  TRAINER_LOGGER                Example: "['console','wandb']". Default: "['console']"
  WANDB_ENV_FILE                Optional env file. Default: <repo>/../secrets/wandb.env
  EXTRA_VERL_ARGS               Extra Hydra overrides appended to VeRL command.
EOF
}

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
  usage
  exit 0
fi

# 路径与镜像。
REPO_DIR=${REPO_DIR:-"$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"}
source "${REPO_DIR}/scripts/lib/common.sh"
VERL_WORKSPACE=${VERL_WORKSPACE:-/data/podman/verl/workspace}
IMAGE=${IMAGE:-localhost/uenv-bridge-verl:qwen35-torch210-vllm019-tf514-kernelfix}

# 模型与数据。
DEFAULT_HOST_MODEL_PATH=/data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B
DEFAULT_CONTAINER_MODEL_PATH=/models/modelscope/Qwen/Qwen3___6-35B-A3B
MODEL_PATH=${MODEL_PATH:-${DEFAULT_HOST_MODEL_PATH}}
CONTAINER_MODEL_PATH=${CONTAINER_MODEL_PATH:-${DEFAULT_CONTAINER_MODEL_PATH}}
DATA_DIR=${DATA_DIR:-${REPO_DIR}/data/gsm8k}
CONTAINER_DATA_DIR=${CONTAINER_DATA_DIR:-/data/gsm8k}

# 训练与数据参数。
TRAINING_STEPS=${TRAINING_STEPS:-10}
TOTAL_EPOCHS=${TOTAL_EPOCHS:-1}
TRAIN_MAX_SAMPLES=${TRAIN_MAX_SAMPLES:--1}
VAL_MAX_SAMPLES=${VAL_MAX_SAMPLES:--1}
TRAIN_BATCH_SIZE=${TRAIN_BATCH_SIZE:-2}
PPO_MINI_BATCH_SIZE=${PPO_MINI_BATCH_SIZE:-2}
PPO_MICRO_BATCH_SIZE_PER_GPU=${PPO_MICRO_BATCH_SIZE_PER_GPU:-1}
ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=${ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU:-1}
REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=${REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU:-1}
MAX_PROMPT_LENGTH=${MAX_PROMPT_LENGTH:-512}
DATA_MAX_RESPONSE_LENGTH=${DATA_MAX_RESPONSE_LENGTH:-2048}
ROLLOUT_N=${ROLLOUT_N:-4}
ROLLOUT_CALCULATE_LOG_PROBS=${ROLLOUT_CALCULATE_LOG_PROBS:-False}
ACTOR_LR=${ACTOR_LR:-1e-6}
KL_LOSS_COEF=${KL_LOSS_COEF:-0.001}

# VeRL rollout/runtime 资源参数。
INFER_BACKEND=${INFER_BACKEND:-vllm}
ROLLOUT_TP=${ROLLOUT_TP:-8}
NGPUS_PER_NODE=${NGPUS_PER_NODE:-8}
PODMAN_GPU_ARGS=${PODMAN_GPU_ARGS:-nvidia.com/gpu=all}
CUDA_VISIBLE_DEVICES_IN_CONTAINER=${CUDA_VISIBLE_DEVICES_IN_CONTAINER:-0,1,2,3,4,5,6,7}
PODMAN_NETWORK_ARGS=${PODMAN_NETWORK_ARGS:---network host}
RAY_NUM_CPUS=${RAY_NUM_CPUS:-$((NGPUS_PER_NODE * 4))}
RAY_NOSET_CUDA_VISIBLE_DEVICES=${RAY_NOSET_CUDA_VISIBLE_DEVICES:-$([ "${NGPUS_PER_NODE}" -gt 1 ] && printf 1 || printf 0)}
ROLLOUT_GPU_MEMORY_UTILIZATION=${ROLLOUT_GPU_MEMORY_UTILIZATION:-0.50}
ROLLOUT_FREE_CACHE_ENGINE=${ROLLOUT_FREE_CACHE_ENGINE:-True}
ROLLOUT_ENABLE_SLEEP_MODE=${ROLLOUT_ENABLE_SLEEP_MODE:-True}
ROLLOUT_ENFORCE_EAGER=${ROLLOUT_ENFORCE_EAGER:-False}
ROLLOUT_ENABLE_CHUNKED_PREFILL=${ROLLOUT_ENABLE_CHUNKED_PREFILL:-True}
ROLLOUT_MAX_NUM_SEQS=${ROLLOUT_MAX_NUM_SEQS:-4}
ROLLOUT_MAX_NUM_BATCHED_TOKENS=${ROLLOUT_MAX_NUM_BATCHED_TOKENS:-16384}
ROLLOUT_MAX_MODEL_LEN=${ROLLOUT_MAX_MODEL_LEN:-16384}

# Eval / checkpoint / 日志参数。
VAL_BEFORE_TRAIN=${VAL_BEFORE_TRAIN:-True}
VAL_ONLY=${VAL_ONLY:-False}
TEST_FREQ=${TEST_FREQ:-5}
SAVE_FREQ=${SAVE_FREQ:--1}
RUN_TS=${RUN_TS:-$(date +%Y%m%d_%H%M%S)}
RUN_ID=${RUN_ID:-verl_gsm8k_grpo_native_${RUN_TS}}

WANDB_ENV_FILE=${WANDB_ENV_FILE:-${REPO_DIR}/../secrets/wandb.env}
TRAINER_LOGGER=${TRAINER_LOGGER:-"['console','wandb']"}
TRAINER_PROJECT_NAME=${TRAINER_PROJECT_NAME:-verl_gsm8k_grpo_train}
WANDB_MODE=${WANDB_MODE:-online}
EXPERIMENT_NAME=${EXPERIMENT_NAME:-qwen3_6_35b_a3b_gsm8k_grpo_${RUN_TS}}

LOG_ROOT=${LOG_ROOT:-${REPO_DIR}/temp/logs}
LOG_DIR=${LOG_DIR:-${LOG_ROOT}/verl_gsm8k_native_grpo}
LOG_FILE=${LOG_FILE:-${LOG_DIR}/${RUN_ID}.log}
CONTAINER_LOG_ROOT=${CONTAINER_LOG_ROOT:-/uenv/uenv-bridge/temp/logs}
EXTRA_VERL_ARGS=${EXTRA_VERL_ARGS:-}
EXTRA_VERL_ARGS=${EXTRA_VERL_ARGS//$'\n'/ }

# 兼容补丁只修本地 VeRL/Transformers 与 Qwen3.6 text-only MoE 的适配问题，
# 不会接入 UEnv Server/Worker 或 model gateway。
UENV_PATCH_RESOURCE_TRACKER=${UENV_PATCH_RESOURCE_TRACKER:-1}
UENV_PATCH_VERL_VLLM_SHUTDOWN=${UENV_PATCH_VERL_VLLM_SHUTDOWN:-1}
UENV_PATCH_VERL_TEXT_ONLY_PROCESSOR=${UENV_PATCH_VERL_TEXT_ONLY_PROCESSOR:-1}
case "${UENV_PATCH_VERL_TEXT_ONLY_PROCESSOR}" in
  1|true|True|enabled|yes|on)
    UENV_PATCH_VERL_TEXT_ONLY_PROCESSOR_RAY=enabled
    ;;
  *)
    UENV_PATCH_VERL_TEXT_ONLY_PROCESSOR_RAY=disabled
    ;;
esac

WANDB_ENV_ARGS=()
if [ -f "${WANDB_ENV_FILE}" ]; then
  # shellcheck disable=SC1090
  source "${WANDB_ENV_FILE}"
fi
for wandb_var in WANDB_API_KEY WANDB_MODE WANDB_ENTITY WANDB_DIR WANDB_BASE_URL; do
  wandb_value=${!wandb_var:-}
  if [ -n "${wandb_value}" ]; then
    export "${wandb_var}=${wandb_value}"
    WANDB_ENV_ARGS+=("-e" "${wandb_var}")
  else
    unset "${wandb_var}" || true
  fi
done

ensure_valid_config() {
  ensure_positive_int NGPUS_PER_NODE "${NGPUS_PER_NODE}"
  ensure_positive_int ROLLOUT_TP "${ROLLOUT_TP}"
  ensure_positive_int RAY_NUM_CPUS "${RAY_NUM_CPUS}"
  ensure_positive_int TRAIN_BATCH_SIZE "${TRAIN_BATCH_SIZE}"
  ensure_positive_int PPO_MINI_BATCH_SIZE "${PPO_MINI_BATCH_SIZE}"
  ensure_positive_int ROLLOUT_N "${ROLLOUT_N}"

  if [ "${ROLLOUT_TP}" -gt "${NGPUS_PER_NODE}" ]; then
    echo "Invalid rollout tensor parallelism: ROLLOUT_TP=${ROLLOUT_TP} exceeds NGPUS_PER_NODE=${NGPUS_PER_NODE}." >&2
    exit 1
  fi

  if [ $((NGPUS_PER_NODE % ROLLOUT_TP)) -ne 0 ]; then
    echo "Invalid rollout split: NGPUS_PER_NODE must be divisible by ROLLOUT_TP." >&2
    exit 1
  fi

  local real_train_batch_size=$((TRAIN_BATCH_SIZE * ROLLOUT_N))
  if [ $((real_train_batch_size % NGPUS_PER_NODE)) -ne 0 ]; then
    echo "Invalid batch: TRAIN_BATCH_SIZE * ROLLOUT_N = ${real_train_batch_size}, must be divisible by NGPUS_PER_NODE=${NGPUS_PER_NODE}." >&2
    exit 1
  fi

  local real_ppo_mini_batch_size=$((PPO_MINI_BATCH_SIZE * ROLLOUT_N))
  if [ $((real_ppo_mini_batch_size % NGPUS_PER_NODE)) -ne 0 ]; then
    echo "Invalid PPO mini batch: PPO_MINI_BATCH_SIZE * ROLLOUT_N = ${real_ppo_mini_batch_size}, must be divisible by NGPUS_PER_NODE=${NGPUS_PER_NODE}." >&2
    exit 1
  fi
}

PODMAN_GPU_RUN_ARGS=$(build_podman_gpu_args "${PODMAN_GPU_ARGS}")

ensure_policy_model_exists
ensure_file_exists "${DATA_DIR}/train.parquet" "Missing GSM8K train parquet"
ensure_file_exists "${DATA_DIR}/test.parquet" "Missing GSM8K eval parquet"
ensure_valid_config
mkdir -p "${LOG_DIR}"

echo "Running native VeRL GSM8K GRPO; log: ${LOG_FILE}"
echo "Data: train=${DATA_DIR}/train.parquet eval=${DATA_DIR}/test.parquet"
echo "No UEnv Server/Worker/gateway/AgentLoop will be used."
echo "Metrics to watch: val/test_score/openai/gsm8k"

set +e
podman run --rm \
  ${PODMAN_NETWORK_ARGS} \
  ${PODMAN_GPU_RUN_ARGS} \
  --shm-size=64g \
  --entrypoint bash \
  --pids-limit=65536 \
  --workdir /workspace/verl \
  "${WANDB_ENV_ARGS[@]}" \
  -v "${VERL_WORKSPACE}:/workspace" \
  -v "${REPO_DIR}:/uenv/uenv-bridge" \
  -v "${MODEL_PATH}:${CONTAINER_MODEL_PATH}:ro" \
  -v "${DATA_DIR}:${CONTAINER_DATA_DIR}:ro" \
  "${IMAGE}" \
  -lc "set -euo pipefail
cd /workspace/verl
export PYTHONUNBUFFERED=1
export PYTHONPATH=/workspace/verl:/uenv/uenv-bridge/src
export CUDA_VISIBLE_DEVICES=${CUDA_VISIBLE_DEVICES_IN_CONTAINER}
export VLLM_USE_V1=1
export VLLM_ALLREDUCE_USE_SYMM_MEM=0
export VLLM_NO_USAGE_STATS=1
export TOKENIZERS_PARALLELISM=false
export HYDRA_FULL_ERROR=1
export RAY_DEDUP_LOGS=0
export RAY_EXPERIMENTAL_NOSET_CUDA_VISIBLE_DEVICES=${RAY_NOSET_CUDA_VISIBLE_DEVICES}
export OMP_NUM_THREADS=1
export MKL_NUM_THREADS=1
export TORCHINDUCTOR_COMPILE_THREADS=1
export UENV_PATCH_RESOURCE_TRACKER=${UENV_PATCH_RESOURCE_TRACKER}
export UENV_PATCH_VERL_VLLM_SHUTDOWN=${UENV_PATCH_VERL_VLLM_SHUTDOWN}
export UENV_PATCH_VERL_TEXT_ONLY_PROCESSOR=${UENV_PATCH_VERL_TEXT_ONLY_PROCESSOR}
python3 -m verl.trainer.main_ppo \\
  hydra.run.dir=${CONTAINER_LOG_ROOT}/verl_gsm8k_native_grpo/hydra_${RUN_ID} \\
  algorithm.adv_estimator=grpo \\
  algorithm.use_kl_in_reward=False \\
  data.train_files=${CONTAINER_DATA_DIR}/train.parquet \\
  data.val_files=${CONTAINER_DATA_DIR}/test.parquet \\
  data.train_batch_size=${TRAIN_BATCH_SIZE} \\
  data.train_max_samples=${TRAIN_MAX_SAMPLES} \\
  data.val_max_samples=${VAL_MAX_SAMPLES} \\
  data.max_prompt_length=${MAX_PROMPT_LENGTH} \\
  data.max_response_length=${DATA_MAX_RESPONSE_LENGTH} \\
  data.filter_overlong_prompts=True \\
  \"data.truncation='error'\" \\
  data.return_raw_chat=True \\
  data.dataloader_num_workers=0 \\
  data.trust_remote_code=True \\
  actor_rollout_ref.model.path=${CONTAINER_MODEL_PATH} \\
  actor_rollout_ref.model.trust_remote_code=True \\
  actor_rollout_ref.model.use_remove_padding=True \\
  actor_rollout_ref.model.enable_gradient_checkpointing=True \\
  actor_rollout_ref.model.enable_activation_offload=True \\
  actor_rollout_ref.actor.strategy=fsdp \\
  actor_rollout_ref.actor.optim.lr=${ACTOR_LR} \\
  actor_rollout_ref.actor.ppo_mini_batch_size=${PPO_MINI_BATCH_SIZE} \\
  actor_rollout_ref.actor.ppo_micro_batch_size_per_gpu=${PPO_MICRO_BATCH_SIZE_PER_GPU} \\
  actor_rollout_ref.actor.use_dynamic_bsz=False \\
  actor_rollout_ref.actor.use_kl_loss=True \\
  actor_rollout_ref.actor.kl_loss_coef=${KL_LOSS_COEF} \\
  actor_rollout_ref.actor.kl_loss_type=low_var_kl \\
  actor_rollout_ref.actor.entropy_coeff=0 \\
  actor_rollout_ref.actor.use_torch_compile=False \\
  actor_rollout_ref.actor.fsdp_config.param_offload=True \\
  actor_rollout_ref.actor.fsdp_config.optimizer_offload=True \\
  actor_rollout_ref.actor.fsdp_config.use_torch_compile=False \\
  actor_rollout_ref.actor.fsdp_config.model_dtype=bf16 \\
  actor_rollout_ref.rollout.name=${INFER_BACKEND} \\
  actor_rollout_ref.rollout.disable_log_stats=False \\
  actor_rollout_ref.rollout.tensor_model_parallel_size=${ROLLOUT_TP} \\
  actor_rollout_ref.rollout.gpu_memory_utilization=${ROLLOUT_GPU_MEMORY_UTILIZATION} \\
  actor_rollout_ref.rollout.n=${ROLLOUT_N} \\
  actor_rollout_ref.rollout.log_prob_micro_batch_size_per_gpu=${ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU} \\
  actor_rollout_ref.rollout.enforce_eager=${ROLLOUT_ENFORCE_EAGER} \\
  actor_rollout_ref.rollout.enable_chunked_prefill=${ROLLOUT_ENABLE_CHUNKED_PREFILL} \\
  actor_rollout_ref.rollout.free_cache_engine=${ROLLOUT_FREE_CACHE_ENGINE} \\
  +actor_rollout_ref.rollout.enable_sleep_mode=${ROLLOUT_ENABLE_SLEEP_MODE} \\
  actor_rollout_ref.rollout.max_num_seqs=${ROLLOUT_MAX_NUM_SEQS} \\
  actor_rollout_ref.rollout.max_num_batched_tokens=${ROLLOUT_MAX_NUM_BATCHED_TOKENS} \\
  +actor_rollout_ref.rollout.max_model_len=${ROLLOUT_MAX_MODEL_LEN} \\
  actor_rollout_ref.rollout.calculate_log_probs=${ROLLOUT_CALCULATE_LOG_PROBS} \\
  actor_rollout_ref.rollout.val_kwargs.n=1 \\
  actor_rollout_ref.rollout.val_kwargs.do_sample=False \\
  actor_rollout_ref.rollout.val_kwargs.temperature=0 \\
  actor_rollout_ref.ref.log_prob_micro_batch_size_per_gpu=${REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU} \\
  actor_rollout_ref.ref.fsdp_config.param_offload=True \\
  actor_rollout_ref.ref.fsdp_config.use_torch_compile=False \\
  actor_rollout_ref.ref.use_torch_compile=False \\
  reward.reward_manager.name=naive \\
  reward.num_workers=1 \\
  trainer.critic_warmup=0 \\
  trainer.balance_batch=True \\
  \"trainer.logger=${TRAINER_LOGGER}\" \\
  trainer.project_name=${TRAINER_PROJECT_NAME} \\
  trainer.experiment_name=${EXPERIMENT_NAME} \\
  trainer.n_gpus_per_node=${NGPUS_PER_NODE} \\
  trainer.nnodes=1 \\
  trainer.save_freq=${SAVE_FREQ} \\
  trainer.test_freq=${TEST_FREQ} \\
  trainer.val_before_train=${VAL_BEFORE_TRAIN} \\
  trainer.val_only=${VAL_ONLY} \\
  trainer.total_training_steps=${TRAINING_STEPS} \\
  trainer.total_epochs=${TOTAL_EPOCHS} \\
  trainer.resume_mode=disable \\
  trainer.default_local_dir=/uenv/uenv-bridge/tmp/verl_gsm8k_native_grpo_ckpt \\
  ray_kwargs.ray_init.num_cpus=${RAY_NUM_CPUS} \\
  +ray_kwargs.ray_init.num_gpus=${NGPUS_PER_NODE} \\
  +ray_kwargs.ray_init.runtime_env.env_vars.PYTHONPATH=/workspace/verl:/uenv/uenv-bridge/src \\
  +ray_kwargs.ray_init.runtime_env.env_vars.UENV_PATCH_VERL_TEXT_ONLY_PROCESSOR=${UENV_PATCH_VERL_TEXT_ONLY_PROCESSOR_RAY} \\
  +ray_kwargs.ray_init.include_dashboard=False \\
  +actor_rollout_ref.model.override_config.attn_implementation=sdpa \\
  actor_rollout_ref.rollout.checkpoint_engine.update_weights_bucket_megabytes=1024 \\
  ${EXTRA_VERL_ARGS}" 2>&1 | tee "${LOG_FILE}"
run_status=${PIPESTATUS[0]}
set -e

if [ "${run_status}" -ne 0 ]; then
  echo "Native VeRL GSM8K GRPO failed. Log: ${LOG_FILE}" >&2
  tail -120 "${LOG_FILE}" >&2 2>/dev/null || true
  exit "${run_status}"
fi

echo "Native VeRL GSM8K GRPO completed."
echo "Log: ${LOG_FILE}"
grep -E "val/test_score/openai/gsm8k|critic/rewards/mean|actor/loss|response_length/clip_ratio|Training Progress|total time:" "${LOG_FILE}" | tail -40 || true
