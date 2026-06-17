#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Run the distributed Layer 4 pre-rollout smoke test.

This script is the single Layer 4 distributed entrypoint on the adapter host.
It does three things:
  1. check the server-side Rust adapter core endpoint
  2. prepare the local VeRL policy model and GSM8K sample data
  3. run VeRL GRPO with UEnvAgentLoop enabled

It does not start Rust adapter core, uenv-server, uenv-worker, or hub. In the
distributed shape, Rust adapter core is owned by the server side; this script
only connects Python/VeRL to that endpoint.

Usage:
  SERVER_ADAPTER_CORE_ENDPOINT=<server-core-host:port> ./scripts/run_layer4_distributed_smoke.sh

Common environment overrides:
  IMAGE                         VeRL image. Default: localhost/uenv-bridge-verl:layer4-build
  VERL_WORKSPACE                Host VeRL workspace. Default: /data/podman/verl/workspace
  MODEL_PATH                    Host policy model path. Default: /data/ronghao/models/modelscope/Qwen/Qwen2___5-0___5B-Instruct
  HOST_MODEL_PATH               Host policy model path; preferred when MODEL_PATH is used as an old container-path alias.
  DATA_DIR                      Host generated VeRL-format GSM8K dir. Default: <repo>/data/
  CONTAINER_MODEL_PATH          Container policy model path. Default: /models/modelscope/Qwen/Qwen2___5-0___5B-Instruct
  CONTAINER_DATA_DIR            Container generated VeRL-format GSM8K dir. Default: /uenv/uenv-bridge/tmp/verl_layer4_agent_loop_data
  INFER_BACKEND                 VeRL rollout backend. Default: vllm
  SAMPLE_COUNT                  Default: 3840
  DATA_SAMPLE_OFFSET            Default: 0
  TRAINING_STEPS                Optional positive integer for smoke runs. Default: null.
  TRAIN_BATCH_SIZE              Default: 256
  PPO_MINI_BATCH_SIZE           Default: 64
  ROLLOUT_N                     Default: 5
  ROLLOUT_TP                    Default: 1
  DATA_MAX_RESPONSE_LENGTH      Default: 1024
  UENV_AGENT_LOOP_BATCH         Batch episodes before Python -> Rust core RPC. Default: 1
  UENV_AGENT_LOOP_BATCH_SIZE    Python -> Rust core micro-batch size. Default: 1
  UENV_AGENT_LOOP_TIMEOUT_SECONDS Default: 1800
  RAY_NUM_CPUS                  Default: NGPUS_PER_NODE * 4
  SERVER_ADAPTER_CORE_ENDPOINT  Server-side Rust adapter core gRPC endpoint. Default: 8.130.86.71:8088
  LOG_ROOT                      Host directory for run logs. Default: <repo>/temp/logs
  CONTAINER_LOG_ROOT            Container directory for run logs. Default: /uenv/uenv-bridge/temp/logs

Example:

最小可运行配置：
  TRAINING_STEPS=10 \
  PPO_MINI_BATCH_SIZE=4 \
  PPO_MICRO_BATCH_SIZE_PER_GPU=1 \
  ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=1 \
  REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=1 \
  TRAIN_BATCH_SIZE=4 \
  TEST_FREQ=-1 \
  PODMAN_GPU_ARGS="nvidia.com/gpu=2,5,6,7" \
  CUDA_VISIBLE_DEVICES_IN_CONTAINER=0,1,2,3 \
  NGPUS_PER_NODE=4 \
  ./scripts/run_layer4_distributed.sh

  TRAINING_STEPS=10 \
  PPO_MINI_BATCH_SIZE=4 \
  PPO_MICRO_BATCH_SIZE_PER_GPU=1 \
  ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=1 \
  REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=1 \
  TRAIN_BATCH_SIZE=4 \
  TEST_FREQ=-1 \
  PODMAN_GPU_ARGS="nvidia.com/gpu=2,5,6,7" \
  CUDA_VISIBLE_DEVICES_IN_CONTAINER=0,1,2,3 \
  NGPUS_PER_NODE=4 \
  ./scripts/run_layer4_distributed.sh
EOF
}

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
  usage
  exit 0
fi

# 路径配置。REPO_DIR 指向 uenv-bridge，VERL_WORKSPACE 指向挂载进容器的 VeRL 工作区。
REPO_DIR=${REPO_DIR:-"$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"}
VERL_WORKSPACE=${VERL_WORKSPACE:-/data/podman/verl/workspace}

# Server 侧已经启动的 Rust adapter core 地址
SERVER_ADAPTER_CORE_ENDPOINT=${SERVER_ADAPTER_CORE_ENDPOINT:-8.130.86.71:8088}
if [ -z "${SERVER_ADAPTER_CORE_ENDPOINT}" ]; then
  echo "SERVER_ADAPTER_CORE_ENDPOINT is required." >&2
  exit 1
fi

# VeRL policy model
IMAGE=${IMAGE:-localhost/uenv-bridge-verl:layer4-build}
DEFAULT_HOST_MODEL_PATH=/data/ronghao/models/modelscope/Qwen/Qwen2___5-0___5B-Instruct
DEFAULT_CONTAINER_MODEL_PATH=/models/modelscope/Qwen/Qwen2___5-0___5B-Instruct

MODEL_PATH=${MODEL_PATH:-${DEFAULT_HOST_MODEL_PATH}}
CONTAINER_MODEL_PATH=${CONTAINER_MODEL_PATH:-${DEFAULT_CONTAINER_MODEL_PATH}}

# 训练与数据参数。
TRAINING_STEPS=${TRAINING_STEPS:-null}
SAMPLE_COUNT=${SAMPLE_COUNT:-3840}
DATA_SAMPLE_OFFSET=${DATA_SAMPLE_OFFSET:-0}
TRAIN_BATCH_SIZE=${TRAIN_BATCH_SIZE:-256}
PPO_MINI_BATCH_SIZE=${PPO_MINI_BATCH_SIZE:-64}
PPO_MICRO_BATCH_SIZE_PER_GPU=${PPO_MICRO_BATCH_SIZE_PER_GPU:-2}
ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=${ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU:-4}
REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=${REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU:-${ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU}}
MAX_PROMPT_LENGTH=${MAX_PROMPT_LENGTH:-512}
ROLLOUT_N=${ROLLOUT_N:-5}
ROLLOUT_TP=${ROLLOUT_TP:-1}
DATA_MAX_RESPONSE_LENGTH=${DATA_MAX_RESPONSE_LENGTH:-1024}
DATA_DIR=${DATA_DIR:-/data/ronghao/uenv/uenv-bridge/data}
CONTAINER_DATA_DIR=${CONTAINER_DATA_DIR:-/data/gsm8k}
INFER_BACKEND=${INFER_BACKEND:-vllm}


# VeRL rollout/runtime 资源参数。
ROLLOUT_FREE_CACHE_ENGINE=${ROLLOUT_FREE_CACHE_ENGINE:-False}
ROLLOUT_ENABLE_SLEEP_MODE=${ROLLOUT_ENABLE_SLEEP_MODE:-False}
ROLLOUT_GPU_MEMORY_UTILIZATION=${ROLLOUT_GPU_MEMORY_UTILIZATION:-0.9}
AGENT_NUM_WORKERS=${AGENT_NUM_WORKERS:-1}
CUDA_VISIBLE_DEVICES_IN_CONTAINER=${CUDA_VISIBLE_DEVICES_IN_CONTAINER:-"7"}
PODMAN_GPU_ARGS=${PODMAN_GPU_ARGS:-nvidia.com/gpu=all}
NGPUS_PER_NODE=${NGPUS_PER_NODE:-1}
RAY_NUM_CPUS=${RAY_NUM_CPUS:-$((NGPUS_PER_NODE * 4))}
RAY_NOSET_CUDA_VISIBLE_DEVICES=${RAY_NOSET_CUDA_VISIBLE_DEVICES:-$([ "${NGPUS_PER_NODE}" -gt 1 ] && printf 1 || printf 0)}
PODMAN_NETWORK_ARGS=${PODMAN_NETWORK_ARGS:---network host}
UENV_PATCH_RESOURCE_TRACKER=${UENV_PATCH_RESOURCE_TRACKER:-1}
UENV_PATCH_VERL_VLLM_SHUTDOWN=${UENV_PATCH_VERL_VLLM_SHUTDOWN:-1}
UENV_AGENT_LOOP_BATCH=${UENV_AGENT_LOOP_BATCH:-1}
UENV_AGENT_LOOP_BATCH_SIZE=${UENV_AGENT_LOOP_BATCH_SIZE:-1}
UENV_AGENT_LOOP_BATCH_RETRY_ATTEMPTS=${UENV_AGENT_LOOP_BATCH_RETRY_ATTEMPTS:-3}
UENV_AGENT_LOOP_BATCH_RETRY_DELAY_SECONDS=${UENV_AGENT_LOOP_BATCH_RETRY_DELAY_SECONDS:-5}
UENV_AGENT_LOOP_TIMEOUT_SECONDS=${UENV_AGENT_LOOP_TIMEOUT_SECONDS:-1800}
ACTOR_LR=${ACTOR_LR:-1e-6}
KL_LOSS_COEF=${KL_LOSS_COEF:-0.001}
TOTAL_EPOCHS=${TOTAL_EPOCHS:-15}
SAVE_FREQ=${SAVE_FREQ:--1}
TEST_FREQ=${TEST_FREQ:-5}
EXPERIMENT_NAME=${EXPERIMENT_NAME:-uenv_layer4_grpo_$(date +%Y%m%d_%H%M)}

# 日志目录。
RUN_ID=${RUN_ID:-layer4_distributed_$(date +%Y%m%d_%H%M%S)}
LOG_ROOT=${LOG_ROOT:-${REPO_DIR}/temp/logs}
SERVICE_DIR=${SERVICE_DIR:-${LOG_ROOT}/layer4_distributed/${RUN_ID}}
LOG_DIR=${LOG_DIR:-${LOG_ROOT}/verl_layer4_agent_loop}
LOG_FILE=${LOG_FILE:-${LOG_DIR}/${RUN_ID}.log}
CONTAINER_LOG_ROOT=${CONTAINER_LOG_ROOT:-/uenv/uenv-bridge/temp/logs}
CONTAINER_SERVICE_DIR=${CONTAINER_LOG_ROOT}/layer4_distributed/${RUN_ID}
AGENT_LOOP_RESULT_RECORD_PATH=${AGENT_LOOP_RESULT_RECORD_PATH:-${CONTAINER_SERVICE_DIR}/agent-loop-results.jsonl}
AGENT_LOOP_REQUEST_RECORD_PATH=${AGENT_LOOP_REQUEST_RECORD_PATH:-${CONTAINER_SERVICE_DIR}/agent-loop-requests.jsonl}

mkdir -p "${DATA_DIR}" "${LOG_DIR}" "${SERVICE_DIR}"

build_podman_gpu_args() {
  local value="$1"
  if [ -z "${value}" ]; then
    printf '%s\n' "--device nvidia.com/gpu=all"
    return 0
  fi

  case "${value}" in
    --device*|--gpus*)
      printf '%s\n' "${value}"
      return 0
      ;;
    all|nvidia.com/gpu=all)
      printf '%s\n' "--device nvidia.com/gpu=all"
      return 0
      ;;
    nvidia.com/gpu=*)
      value="${value#nvidia.com/gpu=}"
      ;;
  esac

  local output=""
  local old_ifs="${IFS}"
  IFS=','
  for gpu_id in ${value}; do
    gpu_id="$(printf '%s' "${gpu_id}" | tr -d '[:space:]')"
    if [ -n "${gpu_id}" ]; then
      output="${output} --device nvidia.com/gpu=${gpu_id}"
    fi
  done
  IFS="${old_ifs}"
  printf '%s\n' "${output# }"
}

PODMAN_GPU_RUN_ARGS=$(build_podman_gpu_args "${PODMAN_GPU_ARGS}")

split_host() {
  local addr="$1"
  printf '%s\n' "${addr%:*}"
}

split_port() {
  local addr="$1"
  printf '%s\n' "${addr##*:}"
}

port_open() {
  local host="$1"
  local port="$2"
  python3 - "$host" "$port" >/dev/null 2>&1 <<'PYNET'
import socket
import sys

host = sys.argv[1]
port = int(sys.argv[2])
sock = socket.socket()
sock.settimeout(0.5)
try:
    sock.connect((host, port))
except OSError:
    sys.exit(1)
else:
    sys.exit(0)
finally:
    sock.close()
PYNET
}

wait_for_addr() {
  local name="$1"
  local addr="$2"
  local timeout_seconds="$3"
  local host
  local port
  host="$(split_host "$addr")"
  port="$(split_port "$addr")"
  for _ in $(seq 1 "$timeout_seconds"); do
    if port_open "$host" "$port"; then
      echo "${name} is listening on ${addr}"
      return 0
    fi
    sleep 1
  done
  echo "Timed out waiting for ${name} on ${addr}" >&2
  return 1
}

ensure_policy_model_exists() {
  if [ -f "${MODEL_PATH}/config.json" ] && compgen -G "${MODEL_PATH}/*.safetensors" >/dev/null; then
    return 0
  fi

  echo "Policy model not found at ${MODEL_PATH}." >&2
  echo "Prepare the policy model there, or override MODEL_PATH/CONTAINER_MODEL_PATH." >&2
  exit 1
}

run_verl_training() {
  if [ "${TRAINING_STEPS}" != "null" ] && [ "${TRAINING_STEPS}" -gt 0 ]; then
    echo "Running ${TRAINING_STEPS}-step GRPO with UEnv pre-rollout AgentLoop; log: ${LOG_FILE}"
  else
    echo "Running GRPO with UEnv pre-rollout AgentLoop; log: ${LOG_FILE}"
  fi
  podman run --rm \
    ${PODMAN_NETWORK_ARGS} \
    ${PODMAN_GPU_RUN_ARGS} \
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
export RAY_EXPERIMENTAL_NOSET_CUDA_VISIBLE_DEVICES=${RAY_NOSET_CUDA_VISIBLE_DEVICES}
export OMP_NUM_THREADS=1
export MKL_NUM_THREADS=1
export TORCHINDUCTOR_COMPILE_THREADS=1
export UENV_PATCH_RESOURCE_TRACKER=${UENV_PATCH_RESOURCE_TRACKER}
export UENV_PATCH_VERL_VLLM_SHUTDOWN=${UENV_PATCH_VERL_VLLM_SHUTDOWN}
export UENV_AGENT_LOOP_BATCH=${UENV_AGENT_LOOP_BATCH}
export UENV_AGENT_LOOP_BATCH_SIZE=${UENV_AGENT_LOOP_BATCH_SIZE}
export UENV_AGENT_LOOP_BATCH_RETRY_ATTEMPTS=${UENV_AGENT_LOOP_BATCH_RETRY_ATTEMPTS}
export UENV_AGENT_LOOP_BATCH_RETRY_DELAY_SECONDS=${UENV_AGENT_LOOP_BATCH_RETRY_DELAY_SECONDS}
export UENV_AGENT_LOOP_TIMEOUT_SECONDS=${UENV_AGENT_LOOP_TIMEOUT_SECONDS}
pip install -q 'grpcio>=1.80' --break-system-packages 2>/dev/null || pip install -q 'grpcio>=1.80'
export UENV_AGENT_LOOP_CLIENT=rust_core
export UENV_ADAPTER_CORE_ENDPOINT=${SERVER_ADAPTER_CORE_ENDPOINT}
export UENV_ADAPTER_CORE_AUTO_START=0
export UENV_ADAPTER_CORE_BINARY=/uenv/uenv-bridge/core/target/debug/uenv-adapter-core
export UENV_ADAPTER_CORE_STARTUP_TIMEOUT_SECONDS=60
export UENV_ADAPTER_CORE_BACKEND=server
export UENV_AGENT_LOOP_REQUEST_RECORD_PATH=\"${AGENT_LOOP_REQUEST_RECORD_PATH}\"
export UENV_AGENT_LOOP_RESULT_RECORD_PATH=\"${AGENT_LOOP_RESULT_RECORD_PATH}\"
python3 -m verl.trainer.main_ppo \\
  hydra.run.dir=${CONTAINER_LOG_ROOT}/verl_layer4_agent_loop/hydra_${RUN_ID} \\
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
  actor_rollout_ref.model.path=${CONTAINER_MODEL_PATH} \\
  actor_rollout_ref.model.use_remove_padding=True \\
  actor_rollout_ref.model.enable_gradient_checkpointing=True \\
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
  actor_rollout_ref.actor.fsdp_config.param_offload=False \\
  actor_rollout_ref.actor.fsdp_config.optimizer_offload=False \\
  actor_rollout_ref.actor.fsdp_config.use_torch_compile=False \\
  actor_rollout_ref.actor.fsdp_config.model_dtype=bf16 \\
  actor_rollout_ref.rollout.name=${INFER_BACKEND} \\
  actor_rollout_ref.rollout.tensor_model_parallel_size=${ROLLOUT_TP} \\
  actor_rollout_ref.rollout.gpu_memory_utilization=${ROLLOUT_GPU_MEMORY_UTILIZATION} \\
  actor_rollout_ref.rollout.n=${ROLLOUT_N} \\
  actor_rollout_ref.rollout.agent.num_workers=${AGENT_NUM_WORKERS} \\
  actor_rollout_ref.rollout.agent.default_agent_loop=uenv_agent \\
  actor_rollout_ref.rollout.agent.agent_loop_config_path=/uenv/uenv-bridge/configs/uenv-agent-loop.yaml \\
  actor_rollout_ref.rollout.log_prob_micro_batch_size_per_gpu=${ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU} \\
  actor_rollout_ref.rollout.enforce_eager=True \\
  actor_rollout_ref.rollout.enable_chunked_prefill=False \\
  actor_rollout_ref.rollout.free_cache_engine=${ROLLOUT_FREE_CACHE_ENGINE} \\
  +actor_rollout_ref.rollout.enable_sleep_mode=${ROLLOUT_ENABLE_SLEEP_MODE} \\
  actor_rollout_ref.rollout.max_num_seqs=4 \\
  actor_rollout_ref.rollout.max_num_batched_tokens=512 \\
  actor_rollout_ref.rollout.calculate_log_probs=True \\
  actor_rollout_ref.ref.log_prob_micro_batch_size_per_gpu=${REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU} \\
  actor_rollout_ref.ref.fsdp_config.param_offload=False \\
  actor_rollout_ref.ref.fsdp_config.use_torch_compile=False \\
  actor_rollout_ref.ref.use_torch_compile=False \\
  reward.reward_manager.name=naive \\
  reward.num_workers=1 \\
  trainer.critic_warmup=0 \\
  trainer.balance_batch=True \\
  \"trainer.logger=['console']\" \\
  trainer.project_name=uenv_bridge_layer4 \\
  trainer.experiment_name=${EXPERIMENT_NAME} \\
  trainer.n_gpus_per_node=${NGPUS_PER_NODE} \\
  trainer.nnodes=1 \\
  trainer.save_freq=${SAVE_FREQ} \\
  trainer.test_freq=${TEST_FREQ} \\
  trainer.val_before_train=False \\
  trainer.total_training_steps=${TRAINING_STEPS} \\
  trainer.total_epochs=${TOTAL_EPOCHS} \\
  trainer.resume_mode=disable \\
  trainer.default_local_dir=/uenv/uenv-bridge/tmp/verl_layer4_agent_loop_ckpt \\
  ray_kwargs.ray_init.num_cpus=${RAY_NUM_CPUS} \\
  +ray_kwargs.ray_init.num_gpus=${NGPUS_PER_NODE} \\
  +ray_kwargs.ray_init.include_dashboard=False" 2>&1 | tee "${LOG_FILE}"
}

wait_for_addr "server-side adapter core" "${SERVER_ADAPTER_CORE_ENDPOINT}" 20
ensure_policy_model_exists

set +e
run_verl_training
run_status=$?
set -e

if [ "${run_status}" -ne 0 ]; then
  echo "Distributed Layer 4 smoke test failed. VeRL log: ${LOG_FILE}" >&2
  tail -120 "${LOG_FILE}" >&2 2>/dev/null || true
  exit "${run_status}"
fi

echo "Distributed Layer 4 smoke test completed."
echo "VeRL log: ${LOG_FILE}"
grep -E "Training Progress: 100%|critic/score/mean|critic/rewards/mean" "${LOG_FILE}" | tail -5 || true
