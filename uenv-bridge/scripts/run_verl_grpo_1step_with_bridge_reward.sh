#!/usr/bin/env bash
set -euo pipefail

REPO_DIR=${REPO_DIR:-"$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"}
IMAGE=${IMAGE:-docker.io/verlai/verl:vllm011.latest}
VERL_WORKSPACE=${VERL_WORKSPACE:-/data/podman/verl/workspace}
MODEL_CACHE=${MODEL_CACHE:-/data/ronghao/models}
MODEL_ID=${MODEL_ID:-Qwen/Qwen2.5-0.5B-Instruct}
HOST_MODEL_PATH=${HOST_MODEL_PATH:-${MODEL_CACHE}/modelscope/Qwen/Qwen2___5-0___5B-Instruct}
MODEL_PATH=${MODEL_PATH:-/models/modelscope/Qwen/Qwen2___5-0___5B-Instruct}
TRAINING_STEPS=${TRAINING_STEPS:-1}
DATA_DIR=${DATA_DIR:-${REPO_DIR}/tmp/verl_grpo_${TRAINING_STEPS}step_data}
CONTAINER_DATA_DIR=/tmp/uenv-bridge/tmp/verl_grpo_${TRAINING_STEPS}step_data
LOG_DIR=${LOG_DIR:-${REPO_DIR}/tmp/verl_grpo_${TRAINING_STEPS}step_bridge_reward_logs}
RECORD_ROOT=${RECORD_ROOT:-${REPO_DIR}/tmp/verl_bridge_reward_records}
SAMPLE_COUNT=${SAMPLE_COUNT:-2}
TRAIN_BATCH_SIZE=${TRAIN_BATCH_SIZE:-2}
ROLLOUT_N=${ROLLOUT_N:-2}
AGENT_NUM_WORKERS=${AGENT_NUM_WORKERS:-1}
RAY_NUM_CPUS=${RAY_NUM_CPUS:-4}
CUDA_VISIBLE_DEVICES_IN_CONTAINER=${CUDA_VISIBLE_DEVICES_IN_CONTAINER:-0}
EXPERIMENT_NAME=${EXPERIMENT_NAME:-qwen25_05b_gsm8k_grpo_${TRAINING_STEPS}step_bridge_reward}
RUN_ID=${RUN_ID:-$(date +%Y%m%d_%H%M%S)}
LOG_FILE=${LOG_FILE:-${LOG_DIR}/${RUN_ID}.log}
ADAPTER_CORE_ENDPOINT=${ADAPTER_CORE_ENDPOINT:-127.0.0.1:55101}
ADAPTER_CORE_FAKE_REWARD=${ADAPTER_CORE_FAKE_REWARD:-0.37}
ADAPTER_CORE_REWARD_MODE=${ADAPTER_CORE_REWARD_MODE:-fixed}
ADAPTER_CORE_FORMAT_REWARD=${ADAPTER_CORE_FORMAT_REWARD:-0.2}
ADAPTER_CORE_NONEMPTY_REWARD=${ADAPTER_CORE_NONEMPTY_REWARD:-0.05}
ADAPTER_CORE_DEFAULT_REWARD=${ADAPTER_CORE_DEFAULT_REWARD:-0.0}

mkdir -p "${MODEL_CACHE}" "${DATA_DIR}" "${LOG_DIR}" "${RECORD_ROOT}/${RUN_ID}"

if [ ! -f "${HOST_MODEL_PATH}/config.json" ] || [ ! -f "${HOST_MODEL_PATH}/model.safetensors" ]; then
  echo "Model not found at ${HOST_MODEL_PATH}; downloading ${MODEL_ID} from ModelScope..."
  podman run --rm --network host --entrypoint bash \
    -v "${MODEL_CACHE}:/models" \
    "${IMAGE}" \
    -lc "python - <<'PY'
from modelscope import snapshot_download
path = snapshot_download('${MODEL_ID}', cache_dir='/models/modelscope')
print(path)
PY"
fi

if [ ! -f "${DATA_DIR}/train.parquet" ] || [ ! -f "${DATA_DIR}/test.parquet" ]; then
  echo "Preparing VeRL-format GSM8K samples under ${DATA_DIR}..."
  podman run --rm --entrypoint bash \
    --workdir /tmp/uenv-bridge \
    -e SAMPLE_COUNT="${SAMPLE_COUNT}" \
    -e CONTAINER_DATA_DIR="${CONTAINER_DATA_DIR}" \
    -v "${VERL_WORKSPACE}:/workspace" \
    -v "${REPO_DIR}:/tmp/uenv-bridge" \
    "${IMAGE}" \
    -lc 'python scripts/prepare_verl_gsm8k_sample.py \
        --input /workspace/data/gsm8k/train.parquet \
        --output "${CONTAINER_DATA_DIR}/train.parquet" \
        --n "${SAMPLE_COUNT}" && \
      python scripts/prepare_verl_gsm8k_sample.py \
        --input /workspace/data/gsm8k/test.parquet \
        --output "${CONTAINER_DATA_DIR}/test.parquet" \
        --n "${SAMPLE_COUNT}"'
fi

echo "Running ${TRAINING_STEPS}-step GRPO with UEnv Bridge reward; log: ${LOG_FILE}"
echo "Bridge request records: ${RECORD_ROOT}/${RUN_ID}"
podman run --rm \
  --device nvidia.com/gpu=all \
  --shm-size=64g \
  --entrypoint bash \
  --workdir /workspace/verl \
  -v "${VERL_WORKSPACE}:/workspace" \
  -v "${REPO_DIR}:/tmp/uenv-bridge" \
  -v "${MODEL_CACHE}:/models" \
  "${IMAGE}" \
  -lc "set -euo pipefail
cd /workspace/verl
export PYTHONPATH=/workspace/verl:/tmp/uenv-bridge/src
export CUDA_VISIBLE_DEVICES=${CUDA_VISIBLE_DEVICES_IN_CONTAINER}
export VLLM_USE_V1=1
export VLLM_ALLREDUCE_USE_SYMM_MEM=0
export TOKENIZERS_PARALLELISM=false
export HYDRA_FULL_ERROR=1
export RAY_DEDUP_LOGS=0
export OMP_NUM_THREADS=1
export MKL_NUM_THREADS=1
export TORCHINDUCTOR_COMPILE_THREADS=1
export UENV_BRIDGE_RECORD_DIR=/tmp/uenv-bridge/tmp/verl_bridge_reward_records/${RUN_ID}
export UENV_BRIDGE_VERBOSE=${UENV_BRIDGE_VERBOSE:-1}
export UENV_BRIDGE_CLIENT=rust_core
export UENV_ADAPTER_CORE_ENDPOINT=${ADAPTER_CORE_ENDPOINT}
export UENV_ADAPTER_CORE_AUTO_START=1
export UENV_ADAPTER_CORE_BINARY=/tmp/uenv-bridge/core/target/debug/uenv-adapter-core
export UENV_ADAPTER_CORE_FAKE_REWARD=${ADAPTER_CORE_FAKE_REWARD}
export UENV_ADAPTER_CORE_REWARD_MODE=${ADAPTER_CORE_REWARD_MODE}
export UENV_ADAPTER_CORE_FORMAT_REWARD=${ADAPTER_CORE_FORMAT_REWARD}
export UENV_ADAPTER_CORE_NONEMPTY_REWARD=${ADAPTER_CORE_NONEMPTY_REWARD}
export UENV_ADAPTER_CORE_DEFAULT_REWARD=${ADAPTER_CORE_DEFAULT_REWARD}
export UENV_ADAPTER_CORE_STARTUP_TIMEOUT_SECONDS=60
cd /tmp/uenv-bridge
./scripts/generate_adapter_core_proto.sh
cd /tmp/uenv-bridge/core
cargo build
cd /workspace/verl
python3 -m verl.trainer.main_ppo \\
  hydra.run.dir=/tmp/uenv-bridge/tmp/verl_grpo_${TRAINING_STEPS}step_bridge_reward_logs/hydra_${RUN_ID} \\
  algorithm.adv_estimator=grpo \\
  algorithm.use_kl_in_reward=False \\
  algorithm.kl_ctrl.kl_coef=0.0 \\
  data.train_files=${CONTAINER_DATA_DIR}/train.parquet \\
  data.val_files=${CONTAINER_DATA_DIR}/test.parquet \\
  data.train_batch_size=${TRAIN_BATCH_SIZE} \\
  data.val_batch_size=${TRAIN_BATCH_SIZE} \\
  data.max_prompt_length=256 \\
  data.max_response_length=32 \\
  data.filter_overlong_prompts=False \\
  data.truncation=error \\
  data.return_raw_chat=True \\
  data.dataloader_num_workers=0 \\
  actor_rollout_ref.model.path=${MODEL_PATH} \\
  actor_rollout_ref.model.use_remove_padding=False \\
  actor_rollout_ref.model.enable_gradient_checkpointing=False \\
  actor_rollout_ref.actor.optim.lr=1e-6 \\
  actor_rollout_ref.actor.ppo_mini_batch_size=${TRAIN_BATCH_SIZE} \\
  actor_rollout_ref.actor.ppo_micro_batch_size_per_gpu=1 \\
  actor_rollout_ref.actor.use_dynamic_bsz=False \\
  actor_rollout_ref.actor.use_kl_loss=False \\
  actor_rollout_ref.actor.entropy_coeff=0 \\
  actor_rollout_ref.actor.use_torch_compile=False \\
  actor_rollout_ref.actor.fsdp_config.param_offload=False \\
  actor_rollout_ref.actor.fsdp_config.optimizer_offload=False \\
  actor_rollout_ref.actor.fsdp_config.use_torch_compile=False \\
  actor_rollout_ref.actor.fsdp_config.model_dtype=bfloat16 \\
  actor_rollout_ref.rollout.name=vllm \\
  actor_rollout_ref.rollout.tensor_model_parallel_size=1 \\
  actor_rollout_ref.rollout.gpu_memory_utilization=0.25 \\
  actor_rollout_ref.rollout.n=${ROLLOUT_N} \\
  actor_rollout_ref.rollout.agent.num_workers=${AGENT_NUM_WORKERS} \\
  actor_rollout_ref.rollout.log_prob_micro_batch_size_per_gpu=1 \\
  actor_rollout_ref.rollout.enforce_eager=True \\
  actor_rollout_ref.rollout.enable_chunked_prefill=False \\
  actor_rollout_ref.rollout.free_cache_engine=True \\
  actor_rollout_ref.rollout.max_num_seqs=4 \\
  actor_rollout_ref.rollout.max_num_batched_tokens=512 \\
  actor_rollout_ref.rollout.calculate_log_probs=True \\
  actor_rollout_ref.ref.log_prob_micro_batch_size_per_gpu=1 \\
  actor_rollout_ref.ref.fsdp_config.param_offload=True \\
  actor_rollout_ref.ref.fsdp_config.use_torch_compile=False \\
  actor_rollout_ref.ref.use_torch_compile=False \\
  reward.reward_manager.source=importlib \\
  reward.reward_manager.name=UEnvBridgeRewardManager \\
  reward.reward_manager.module.path=/tmp/uenv-bridge/src/uenv/bridge/verl_reward_manager.py \\
  reward.num_workers=1 \\
  trainer.critic_warmup=0 \\
  trainer.logger=console \\
  trainer.project_name=uenv_bridge \\
  trainer.experiment_name=${EXPERIMENT_NAME} \\
  trainer.n_gpus_per_node=1 \\
  trainer.nnodes=1 \\
  trainer.save_freq=-1 \\
  trainer.test_freq=-1 \\
  trainer.val_before_train=False \\
  trainer.total_training_steps=${TRAINING_STEPS} \\
  trainer.total_epochs=1 \\
  trainer.resume_mode=disable \\
  trainer.default_local_dir=/tmp/uenv-bridge/tmp/verl_grpo_${TRAINING_STEPS}step_bridge_reward_ckpt \\
  ray_kwargs.ray_init.num_cpus=${RAY_NUM_CPUS} \\
  +ray_kwargs.ray_init.num_gpus=1 \\
  +ray_kwargs.ray_init.include_dashboard=False" 2>&1 | tee "${LOG_FILE}"
