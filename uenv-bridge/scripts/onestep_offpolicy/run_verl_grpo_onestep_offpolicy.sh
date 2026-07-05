#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Run native VeRL GRPO with one-step off-policy scheduling inside the VeRL podman image.

This script is for comparing VeRL's native one-step off-policy training path
against the current synchronous Layer 4 baseline. It does not connect to
UEnv Server/Worker, and it does not enable UEnvAgentLoop.

Most data/model/training/vLLM defaults intentionally match
scripts/run_layer4_distributed.sh. The main intentional difference is resource
layout: one-step off-policy needs separate training and rollout GPU pools so it
can overlap rollout for the next step with actor update for the current step.

Usage:
  ./scripts/onestep_offpolicy/run_verl_grpo_onestep_offpolicy.sh

Common overrides:
  IMAGE                         Default: localhost/uenv-bridge-verl:layer4-build
  VERL_WORKSPACE                Default: /data/podman/verl/workspace
  MODEL_PATH                    Default: /data/ronghao/models/modelscope/Qwen/Qwen2___5-0___5B-Instruct
  DATA_DIR                      Default: <repo>/data
  TRAINING_STEPS                Default: 1. Override to match Layer4 comparison runs.
  TRAIN_BATCH_SIZE              Default: 256
  PPO_MINI_BATCH_SIZE           Default: 64
  PPO_MICRO_BATCH_SIZE_PER_GPU  Default: 2
  ROLLOUT_N                     Default: 5
  ROLLOUT_TP                    Default: 1
  DATA_MAX_RESPONSE_LENGTH      Default: 1024
  AGENT_NUM_WORKERS             AgentLoop workers for async generation. Default: 1
  NGPUS_PER_NODE                Container-visible GPU count. Default: 2
  TRAINING_GPUS_PER_NODE        GPUs used by trainer/actor. Default: NGPUS_PER_NODE - ROLLOUT_GPUS_PER_NODE
  ROLLOUT_GPUS_PER_NODE         GPUs used by async rollout server. Default: 1, or 2 when NGPUS_PER_NODE >= 8
  CHECKPOINT_ENGINE_BACKEND     Weight sync backend. Default: nccl
  PODMAN_GPU_ARGS               Default: nvidia.com/gpu=all
  PODMAN_EXTRA_ARGS             Extra podman run args. Default maps hostname to host IP.
  CUDA_VISIBLE_DEVICES_IN_CONTAINER Default: 0,1
  RAY_NUM_CPUS                  Default: 10 + NGPUS_PER_NODE * 4
  RAY_NOSET_CUDA_VISIBLE_DEVICES Default: empty; leave empty for Ray per-actor GPU isolation.
  LOG_ROOT                      Default: <repo>/temp/logs
  EXTRA_VERL_ARGS               Extra Hydra args appended to main_ppo.

Layer4-aligned 4-GPU comparison example:
  TRAINING_STEPS=10 \
  TRAIN_BATCH_SIZE=4 \
  PPO_MINI_BATCH_SIZE=4 \
  PPO_MICRO_BATCH_SIZE_PER_GPU=1 \
  ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=1 \
  REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=1 \
  TEST_FREQ=-1 \
  PODMAN_GPU_ARGS="nvidia.com/gpu=0,1,2,3" \
  CUDA_VISIBLE_DEVICES_IN_CONTAINER=0,1,2,3 \
  NGPUS_PER_NODE=4 \
  TRAINING_GPUS_PER_NODE=3 \
  ROLLOUT_GPUS_PER_NODE=1 \
  ROLLOUT_TP=1 \
  AGENT_NUM_WORKERS=1 \
  ./scripts/onestep_offpolicy/run_verl_grpo_onestep_offpolicy.sh

Previously verified 8-GPU smoke:
  TRAINING_STEPS=1 \
  TRAIN_BATCH_SIZE=6 \
  PPO_MINI_BATCH_SIZE=6 \
  PPO_MICRO_BATCH_SIZE_PER_GPU=1 \
  ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=1 \
  REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=1 \
  ROLLOUT_N=2 \
  DATA_MAX_RESPONSE_LENGTH=32 \
  TEST_FREQ=-1 \
  PODMAN_GPU_ARGS="nvidia.com/gpu=0,1,2,3,4,5,6,7" \
  CUDA_VISIBLE_DEVICES_IN_CONTAINER=0,1,2,3,4,5,6,7 \
  NGPUS_PER_NODE=8 \
  TRAINING_GPUS_PER_NODE=6 \
  ROLLOUT_GPUS_PER_NODE=2 \
  ROLLOUT_TP=2 \
  AGENT_NUM_WORKERS=1 \
  ./scripts/onestep_offpolicy/run_verl_grpo_onestep_offpolicy.sh
EOF
}

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
  usage
  exit 0
fi

REPO_DIR=${REPO_DIR:-"$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"}
source "${REPO_DIR}/scripts/lib/common.sh"
VERL_WORKSPACE=${VERL_WORKSPACE:-/data/podman/verl/workspace}
IMAGE=${IMAGE:-localhost/uenv-bridge-verl:layer4-build}

DEFAULT_HOST_MODEL_PATH=/data/ronghao/models/modelscope/Qwen/Qwen2___5-0___5B-Instruct
DEFAULT_CONTAINER_MODEL_PATH=/models/modelscope/Qwen/Qwen2___5-0___5B-Instruct
MODEL_PATH=${MODEL_PATH:-${DEFAULT_HOST_MODEL_PATH}}
CONTAINER_MODEL_PATH=${CONTAINER_MODEL_PATH:-${DEFAULT_CONTAINER_MODEL_PATH}}

DATA_DIR=${DATA_DIR:-${REPO_DIR}/data}
CONTAINER_DATA_DIR=${CONTAINER_DATA_DIR:-/data/gsm8k}

TRAINING_STEPS=${TRAINING_STEPS:-1}
TRAIN_BATCH_SIZE=${TRAIN_BATCH_SIZE:-256}
PPO_MINI_BATCH_SIZE=${PPO_MINI_BATCH_SIZE:-64}
PPO_MICRO_BATCH_SIZE_PER_GPU=${PPO_MICRO_BATCH_SIZE_PER_GPU:-2}
ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=${ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU:-4}
REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=${REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU:-${ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU}}
MAX_PROMPT_LENGTH=${MAX_PROMPT_LENGTH:-512}
DATA_MAX_RESPONSE_LENGTH=${DATA_MAX_RESPONSE_LENGTH:-1024}
ROLLOUT_N=${ROLLOUT_N:-5}
ROLLOUT_TP=${ROLLOUT_TP:-1}
AGENT_NUM_WORKERS=${AGENT_NUM_WORKERS:-1}
INFER_BACKEND=${INFER_BACKEND:-vllm}

NGPUS_PER_NODE=${NGPUS_PER_NODE:-2}
DEFAULT_ROLLOUT_GPUS_PER_NODE=1
if [ "${NGPUS_PER_NODE}" -ge 8 ]; then
  DEFAULT_ROLLOUT_GPUS_PER_NODE=2
fi
ROLLOUT_GPUS_PER_NODE=${ROLLOUT_GPUS_PER_NODE:-${DEFAULT_ROLLOUT_GPUS_PER_NODE}}
DEFAULT_TRAINING_GPUS_PER_NODE=$((NGPUS_PER_NODE - ROLLOUT_GPUS_PER_NODE))
TRAINING_GPUS_PER_NODE=${TRAINING_GPUS_PER_NODE:-${DEFAULT_TRAINING_GPUS_PER_NODE}}
PODMAN_GPU_ARGS=${PODMAN_GPU_ARGS:-nvidia.com/gpu=all}
CUDA_VISIBLE_DEVICES_IN_CONTAINER=${CUDA_VISIBLE_DEVICES_IN_CONTAINER:-0,1}
PODMAN_NETWORK_ARGS=${PODMAN_NETWORK_ARGS:---network host}
HOST_IP=${HOST_IP:-$(hostname -I 2>/dev/null | awk '{print $1}')}
PODMAN_EXTRA_ARGS=${PODMAN_EXTRA_ARGS:-"--add-host user:${HOST_IP}"}
DEFAULT_RAY_NUM_CPUS=$((10 + NGPUS_PER_NODE * 4))
RAY_NUM_CPUS=${RAY_NUM_CPUS:-${DEFAULT_RAY_NUM_CPUS}}
# Do not set RAY_EXPERIMENTAL_NOSET_CUDA_VISIBLE_DEVICES by default.
# In this environment Ray's NCCL collective fails if every GPU actor keeps the
# full CUDA_VISIBLE_DEVICES list and therefore binds to logical cuda:0.
RAY_NOSET_CUDA_VISIBLE_DEVICES=${RAY_NOSET_CUDA_VISIBLE_DEVICES:-}

ROLLOUT_GPU_MEMORY_UTILIZATION=${ROLLOUT_GPU_MEMORY_UTILIZATION:-0.8}
ROLLOUT_MAX_NUM_SEQS=${ROLLOUT_MAX_NUM_SEQS:-4}
ROLLOUT_MAX_NUM_BATCHED_TOKENS=${ROLLOUT_MAX_NUM_BATCHED_TOKENS:-512}
ROLLOUT_ENFORCE_EAGER=${ROLLOUT_ENFORCE_EAGER:-True}
ROLLOUT_ENABLE_CHUNKED_PREFILL=${ROLLOUT_ENABLE_CHUNKED_PREFILL:-False}
ROLLOUT_FREE_CACHE_ENGINE=${ROLLOUT_FREE_CACHE_ENGINE:-False}
ROLLOUT_ENABLE_SLEEP_MODE=${ROLLOUT_ENABLE_SLEEP_MODE:-False}
ROLLOUT_LAYERED_SUMMON=${ROLLOUT_LAYERED_SUMMON:-True}
CHECKPOINT_ENGINE_BACKEND=${CHECKPOINT_ENGINE_BACKEND:-nccl}

ACTOR_LR=${ACTOR_LR:-1e-6}
KL_LOSS_COEF=${KL_LOSS_COEF:-0.001}
TOTAL_EPOCHS=${TOTAL_EPOCHS:-15}
SAVE_FREQ=${SAVE_FREQ:--1}
TEST_FREQ=${TEST_FREQ:-5}
VAL_BEFORE_TRAIN=${VAL_BEFORE_TRAIN:-False}
EXPERIMENT_NAME=${EXPERIMENT_NAME:-uenv_onestep_offpolicy_$(date +%Y%m%d_%H%M)}
RUN_ID=${RUN_ID:-onestep_offpolicy_$(date +%Y%m%d_%H%M%S)}

LOG_ROOT=${LOG_ROOT:-${REPO_DIR}/temp/logs}
CONTAINER_LOG_ROOT=${CONTAINER_LOG_ROOT:-/uenv/uenv-bridge/temp/logs}
LOG_DIR=${LOG_DIR:-${LOG_ROOT}/verl_onestep_offpolicy}
LOG_FILE=${LOG_FILE:-${LOG_DIR}/${RUN_ID}.log}

UENV_PATCH_RESOURCE_TRACKER=${UENV_PATCH_RESOURCE_TRACKER:-1}
UENV_PATCH_VERL_VLLM_SHUTDOWN=${UENV_PATCH_VERL_VLLM_SHUTDOWN:-1}
EXTRA_VERL_ARGS=${EXTRA_VERL_ARGS:-}

mkdir -p "${LOG_DIR}"

ensure_valid_resource_split() {
  ensure_positive_int NGPUS_PER_NODE "${NGPUS_PER_NODE}"
  ensure_positive_int TRAINING_GPUS_PER_NODE "${TRAINING_GPUS_PER_NODE}"
  ensure_positive_int ROLLOUT_GPUS_PER_NODE "${ROLLOUT_GPUS_PER_NODE}"
  ensure_positive_int ROLLOUT_TP "${ROLLOUT_TP}"
  ensure_positive_int AGENT_NUM_WORKERS "${AGENT_NUM_WORKERS}"
  ensure_positive_int RAY_NUM_CPUS "${RAY_NUM_CPUS}"

  local used_gpus=$((TRAINING_GPUS_PER_NODE + ROLLOUT_GPUS_PER_NODE))
  if [ "${used_gpus}" -gt "${NGPUS_PER_NODE}" ]; then
    echo "Invalid GPU split: TRAINING_GPUS_PER_NODE + ROLLOUT_GPUS_PER_NODE = ${used_gpus}, but NGPUS_PER_NODE = ${NGPUS_PER_NODE}." >&2
    exit 1
  fi

  if [ "${ROLLOUT_TP}" -gt "${ROLLOUT_GPUS_PER_NODE}" ]; then
    echo "Invalid rollout tensor parallelism: ROLLOUT_TP=${ROLLOUT_TP} exceeds ROLLOUT_GPUS_PER_NODE=${ROLLOUT_GPUS_PER_NODE}." >&2
    exit 1
  fi

  if [ $((ROLLOUT_GPUS_PER_NODE % ROLLOUT_TP)) -ne 0 ]; then
    echo "Invalid rollout split: ROLLOUT_GPUS_PER_NODE must be divisible by ROLLOUT_TP." >&2
    exit 1
  fi

  local generated_batch_size=$((TRAIN_BATCH_SIZE * ROLLOUT_N))
  if [ $((generated_batch_size % AGENT_NUM_WORKERS)) -ne 0 ]; then
    echo "Invalid AgentLoop split: TRAIN_BATCH_SIZE * ROLLOUT_N = ${generated_batch_size}, but AGENT_NUM_WORKERS = ${AGENT_NUM_WORKERS}." >&2
    echo "VeRL AgentLoop requires the repeated generation batch to split evenly across agent workers." >&2
    exit 1
  fi

  local min_ray_cpus=$((10 + used_gpus * 3))
  if [ "${RAY_NUM_CPUS}" -lt "${min_ray_cpus}" ]; then
    echo "Invalid Ray CPU count: one-step off-policy needs at least ${min_ray_cpus} CPUs for TaskRunner and worker placement groups, got RAY_NUM_CPUS=${RAY_NUM_CPUS}." >&2
    exit 1
  fi
}

PODMAN_GPU_RUN_ARGS=$(build_podman_gpu_args "${PODMAN_GPU_ARGS}")

ensure_policy_model_exists
ensure_file_exists "${DATA_DIR}/train.parquet" "Missing train parquet"
ensure_file_exists "${DATA_DIR}/test.parquet" "Missing test parquet"
ensure_valid_resource_split

echo "Running VeRL one-step off-policy GRPO; log: ${LOG_FILE}"
echo "GPU split: training=${TRAINING_GPUS_PER_NODE}, rollout=${ROLLOUT_GPUS_PER_NODE}, visible=${NGPUS_PER_NODE}"

set +e
podman run --rm \
  ${PODMAN_NETWORK_ARGS} \
  ${PODMAN_GPU_RUN_ARGS} \
  ${PODMAN_EXTRA_ARGS} \
  --shm-size=64g \
  --entrypoint bash \
  --pids-limit=65536 \
  --workdir /workspace/verl \
  -v "${VERL_WORKSPACE}:/workspace" \
  -v "${REPO_DIR}:/uenv/uenv-bridge" \
  -v "${MODEL_PATH}:${CONTAINER_MODEL_PATH}:ro" \
  -v "${DATA_DIR}:${CONTAINER_DATA_DIR}:ro" \
  "${IMAGE}" \
  -lc "set -euo pipefail
cd /workspace/verl
export PYTHONPATH=/workspace/verl:/uenv/uenv-bridge/src
export CUDA_VISIBLE_DEVICES=${CUDA_VISIBLE_DEVICES_IN_CONTAINER}
export VLLM_USE_V1=1
export VLLM_ALLREDUCE_USE_SYMM_MEM=0
export VLLM_NO_USAGE_STATS=1
export TOKENIZERS_PARALLELISM=false
export HYDRA_FULL_ERROR=1
export RAY_DEDUP_LOGS=0
if [ -n \"${RAY_NOSET_CUDA_VISIBLE_DEVICES}\" ]; then
  export RAY_EXPERIMENTAL_NOSET_CUDA_VISIBLE_DEVICES=${RAY_NOSET_CUDA_VISIBLE_DEVICES}
else
  unset RAY_EXPERIMENTAL_NOSET_CUDA_VISIBLE_DEVICES
fi
export OMP_NUM_THREADS=1
export MKL_NUM_THREADS=1
export TORCHINDUCTOR_COMPILE_THREADS=1
export UENV_PATCH_RESOURCE_TRACKER=${UENV_PATCH_RESOURCE_TRACKER}
export UENV_PATCH_VERL_VLLM_SHUTDOWN=${UENV_PATCH_VERL_VLLM_SHUTDOWN}
export UENV_AGENT_LOOP_BATCH=0
python3 -m verl.experimental.one_step_off_policy.main_ppo \\
  hydra.run.dir=${CONTAINER_LOG_ROOT}/verl_onestep_offpolicy/hydra_${RUN_ID} \\
  algorithm.adv_estimator=grpo \\
  algorithm.use_kl_in_reward=False \\
  data.train_files=${CONTAINER_DATA_DIR}/train.parquet \\
  data.val_files=${CONTAINER_DATA_DIR}/test.parquet \\
  data.train_batch_size=${TRAIN_BATCH_SIZE} \\
  data.max_prompt_length=${MAX_PROMPT_LENGTH} \\
  data.max_response_length=${DATA_MAX_RESPONSE_LENGTH} \\
  data.filter_overlong_prompts=True \\
  \"data.truncation='error'\" \\
  data.return_raw_chat=True \\
  data.dataloader_num_workers=0 \\
  actor_rollout_ref.hybrid_engine=False \\
  actor_rollout_ref.model.path=${CONTAINER_MODEL_PATH} \\
  actor_rollout_ref.model.use_remove_padding=True \\
  actor_rollout_ref.model.enable_gradient_checkpointing=True \\
  actor_rollout_ref.actor.fsdp_config.strategy=fsdp2 \\
  actor_rollout_ref.actor.optim.lr=${ACTOR_LR} \\
  actor_rollout_ref.actor.ppo_mini_batch_size=${PPO_MINI_BATCH_SIZE} \\
  actor_rollout_ref.actor.ppo_micro_batch_size_per_gpu=${PPO_MICRO_BATCH_SIZE_PER_GPU} \\
  actor_rollout_ref.actor.use_dynamic_bsz=False \\
  actor_rollout_ref.actor.use_kl_loss=True \\
  actor_rollout_ref.actor.kl_loss_coef=${KL_LOSS_COEF} \\
  actor_rollout_ref.actor.kl_loss_type=low_var_kl \\
  actor_rollout_ref.actor.entropy_coeff=0 \\
  actor_rollout_ref.actor.use_torch_compile=False \\
  actor_rollout_ref.actor.fsdp_config.param_offload=False \\
  actor_rollout_ref.actor.fsdp_config.optimizer_offload=False \\
  actor_rollout_ref.actor.fsdp_config.use_torch_compile=False \\
  actor_rollout_ref.actor.fsdp_config.model_dtype=bf16 \\
  critic.strategy=fsdp2 \\
  actor_rollout_ref.rollout.name=${INFER_BACKEND} \\
  actor_rollout_ref.rollout.mode=async \\
  actor_rollout_ref.rollout.tensor_model_parallel_size=${ROLLOUT_TP} \\
  actor_rollout_ref.rollout.gpu_memory_utilization=${ROLLOUT_GPU_MEMORY_UTILIZATION} \\
  actor_rollout_ref.rollout.n=${ROLLOUT_N} \\
  actor_rollout_ref.rollout.agent.num_workers=${AGENT_NUM_WORKERS} \\
  actor_rollout_ref.rollout.load_format=safetensors \\
  actor_rollout_ref.rollout.layered_summon=${ROLLOUT_LAYERED_SUMMON} \\
  actor_rollout_ref.rollout.checkpoint_engine.backend=${CHECKPOINT_ENGINE_BACKEND} \\
  actor_rollout_ref.rollout.log_prob_micro_batch_size_per_gpu=${ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU} \\
  actor_rollout_ref.rollout.free_cache_engine=${ROLLOUT_FREE_CACHE_ENGINE} \\
  +actor_rollout_ref.rollout.enable_sleep_mode=${ROLLOUT_ENABLE_SLEEP_MODE} \\
  actor_rollout_ref.rollout.calculate_log_probs=True \\
  actor_rollout_ref.rollout.enforce_eager=${ROLLOUT_ENFORCE_EAGER} \\
  actor_rollout_ref.rollout.enable_chunked_prefill=${ROLLOUT_ENABLE_CHUNKED_PREFILL} \\
  actor_rollout_ref.rollout.max_num_seqs=${ROLLOUT_MAX_NUM_SEQS} \\
  actor_rollout_ref.rollout.max_num_batched_tokens=${ROLLOUT_MAX_NUM_BATCHED_TOKENS} \\
  actor_rollout_ref.ref.log_prob_micro_batch_size_per_gpu=${REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU} \\
  actor_rollout_ref.ref.fsdp_config.param_offload=False \\
  actor_rollout_ref.ref.fsdp_config.use_torch_compile=False \\
  actor_rollout_ref.ref.use_torch_compile=False \\
  reward.reward_manager.name=naive \\
  reward.num_workers=1 \\
  trainer.critic_warmup=0 \\
  trainer.balance_batch=True \\
  \"trainer.logger=['console']\" \\
  trainer.project_name=uenv_bridge_onestep_offpolicy \\
  trainer.experiment_name=${EXPERIMENT_NAME} \\
  trainer.nnodes=1 \\
  trainer.n_gpus_per_node=${TRAINING_GPUS_PER_NODE} \\
  trainer.save_freq=${SAVE_FREQ} \\
  trainer.test_freq=${TEST_FREQ} \\
  trainer.val_before_train=${VAL_BEFORE_TRAIN} \\
  trainer.total_training_steps=${TRAINING_STEPS} \\
  trainer.total_epochs=${TOTAL_EPOCHS} \\
  trainer.resume_mode=disable \\
  trainer.default_local_dir=/uenv/uenv-bridge/tmp/verl_onestep_offpolicy_ckpt \\
  rollout.nnodes=1 \\
  rollout.n_gpus_per_node=${ROLLOUT_GPUS_PER_NODE} \\
  ray_kwargs.ray_init.num_cpus=${RAY_NUM_CPUS} \\
  +ray_kwargs.ray_init.num_gpus=${NGPUS_PER_NODE} \\
  +ray_kwargs.ray_init.include_dashboard=False \\
  ${EXTRA_VERL_ARGS}" 2>&1 | tee "${LOG_FILE}"
run_status=${PIPESTATUS[0]}
set -e

if [ "${run_status}" -ne 0 ]; then
  echo "VeRL one-step off-policy run failed. Log: ${LOG_FILE}" >&2
  tail -120 "${LOG_FILE}" >&2 2>/dev/null || true
  exit "${run_status}"
fi

echo "VeRL one-step off-policy run completed."
echo "Log: ${LOG_FILE}"
grep -E "Training Progress: 100%|step:[0-9]+ -|critic/rewards/mean|actor/loss|total time:" "${LOG_FILE}" | tail -20 || true
