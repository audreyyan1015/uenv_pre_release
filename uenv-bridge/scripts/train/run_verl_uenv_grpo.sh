#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Run VeRL GRPO with the UEnv AgentLoop bridge.

This script is the generic VeRL + UEnv training entrypoint on the adapter host.
It does three things:
  1. check the server-side Rust adapter core endpoint
  2. mount the local VeRL policy model and VeRL-format train/val data
  3. run VeRL GRPO with UEnvAgentLoop enabled

It does not start Rust adapter core, uenv-server, uenv-worker, or hub. In the
distributed shape, Rust adapter core is owned by the server side; this script
only connects Python/VeRL to that endpoint.

Usage:
  SERVER_ADAPTER_CORE_ENDPOINT=<server-core-host:port> ./scripts/train/run_verl_uenv_grpo.sh

Common environment overrides:
  UENV_DEVICE_BACKEND           Device backend for the local training container.
                                Default: cuda. Use ascend on 910C hosts.
  IMAGE                         VeRL image. Default: localhost/uenv-bridge-verl:qwen35-torch210-vllm019-tf514-kernelfix
  VERL_WORKSPACE                Host VeRL workspace. Default: /data/podman/verl/workspace
  MODEL_PATH                    Host policy model path. Default: /data/ronghao/models/modelscope/Qwen/Qwen2___5-0___5B-Instruct
  DATA_DIR                      Host VeRL-format train/val data dir. Default: <repo>/data/gsm8k
  CONTAINER_MODEL_PATH          Container policy model path. Default: /models/modelscope/Qwen/Qwen2___5-0___5B-Instruct
  CONTAINER_DATA_DIR            Container VeRL-format train/val data dir. Default: /data/gsm8k
  INFER_BACKEND                 VeRL rollout backend. Default: vllm
  TRAINING_STEPS                Optional positive integer for smoke runs. Default: null.
  TRAIN_BATCH_SIZE              Default: 256
  PPO_MINI_BATCH_SIZE           Default: 64
  ROLLOUT_N                     Default: 5
  ROLLOUT_TEMPERATURE           Rollout sampling temperature. Default: 1.0
  ROLLOUT_TP                    Default: 1
  ROLLOUT_CALCULATE_LOG_PROBS   Ask rollout engine to return token logprobs. Default: False for sync.
  DATA_MAX_RESPONSE_LENGTH      Default: 1024
  UENV_AGENT_LOOP_BATCH         Batch episodes before Python -> Rust core RPC. Default: 1
  UENV_AGENT_LOOP_BATCH_SIZE    Python -> Rust core micro-batch size; 0 means whole VeRL batch. Default: 0
  UENV_AGENT_LOOP_PARALLEL_MODE Adapter metadata parallel mode. Default: sync
  UENV_AGENT_LOOP_TIMEOUT_SECONDS Default: 1800
  UENV_ADAPTER_CORE_GRPC_MAX_MESSAGE_BYTES
                                  Python Adapter <-> Rust AdapterCore gRPC max message bytes. Default: 16777216
  UENV_EPISODE_MAX_STEPS_OVERRIDE Runtime max_steps/max_iterations override. Default: empty
  TRAINER_LOGGER                VeRL logger backends. Use "['console','wandb']" to enable wandb. Default: "['console']"
  TRAINER_PROJECT_NAME          VeRL/wandb project name. Default: uenv_bridge_layer4
  CHECKPOINT_ROOT               Host checkpoint root. Default: <repo>/checkpoints/uenv_grpo
  CHECKPOINT_RUN_DIR            Host checkpoint dir for this run. Default: <CHECKPOINT_ROOT>/<RUN_ID>
  CONTAINER_CHECKPOINT_RUN_DIR  Container checkpoint dir passed to VeRL. Default: /uenv/uenv-bridge/checkpoints/uenv_grpo/<RUN_ID>
  WANDB_ENV_FILE                Optional host env file loaded before wandb setup. Default: <repo>/../secrets/wandb.env
  WANDB_API_KEY                 Optional wandb API key; passed through to the container when set.
  WANDB_MODE                    Optional wandb mode, for example online or offline.
  WANDB_ENTITY                  Optional wandb entity.
  WANDB_DIR                     Optional wandb run directory inside the container.
  WANDB_BASE_URL                Optional wandb server URL for private deployments.
  UENV_OBS_URL                   Server Obs base URL for frontend visualization. Default: empty.
  UENV_OBS_TOKEN                 Optional Server Obs auth token. Default: empty.
  UENV_OBS_HEARTBEAT_INTERVAL_SECONDS
                                  Run heartbeat interval for Obs. Set <=0 to disable. Default: 30
  UENV_TRAINING_RUN_ID           Frontend/Obs run id. Default: RUN_ID.
  UENV_MODEL_GATEWAY_ENABLED    Start adapter-side model gateway and send its URL to Worker. Default: 0
  UENV_MODEL_GATEWAY_PORT       Adapter-side model gateway port. Default: 18080
  UENV_MODEL_GATEWAY_PUBLIC_URL Worker-visible gateway URL. Default: http://10.10.20.142:<port>/v1
  UENV_MODEL_GATEWAY_DISABLE_THINKING
                                  Inject chat_template_kwargs.enable_thinking=false for OpenAI chat requests. Default: 0
  UENV_MODEL_GATEWAY_MAX_TOKENS   Clamp OpenAI chat output token budget before forwarding to vLLM. Default: empty
  UENV_MODEL_GATEWAY_STOP_ON_CLOSE
                                  Stop adapter gateway when each AgentLoop instance closes. Default: 0 for multi-worker, 1 otherwise
  UENV_REQUIRE_SWE_RESPONSE_TRACE
                                  Refuse SWE training results without typed response_ids. Default: 1
  UENV_AGENT_LOOP_FAILED_EPISODE_POLICY
                                  Failed episode handling: raise or zero_reward. Default: raise
  UENV_PATCH_VERL_TEXT_ONLY_PROCESSOR
                                  Treat known text-only MoE checkpoints as having no multimodal processor. Default: 0
  UENV_PATCH_VERL_EMPTY_RESPONSE_BATCH
                                  Treat empty-response rollout batches as no-op instead of aborting training. Default: 1
  EXTRA_VERL_ARGS               Extra Hydra overrides appended to the VeRL command. Default: empty
  RAY_NUM_CPUS                  Default: NGPUS_PER_NODE * 4
  SERVER_ADAPTER_CORE_ENDPOINT  Server-side Rust adapter core gRPC endpoint. Default: 8.130.75.157:8088
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
  PODMAN_GPU_ARGS="nvidia.com/gpu=4,5,6,7" \
  CUDA_VISIBLE_DEVICES_IN_CONTAINER=0,1,2,3 \
  NGPUS_PER_NODE=4 \
  ./scripts/train/run_verl_uenv_grpo.sh

加入“中转站”后的最小配置：
  UENV_MODEL_GATEWAY_ENABLED=1 \
  UENV_MODEL_GATEWAY_PORT=18088 \
  UENV_MODEL_GATEWAY_PUBLIC_URL=http://10.10.20.142:18088/v1 \
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
  ./scripts/train/run_verl_uenv_grpo.sh

完整运行配置：
  UENV_MODEL_GATEWAY_ENABLED=1 \
  UENV_MODEL_GATEWAY_PORT=18088 \
  UENV_MODEL_GATEWAY_PUBLIC_URL=http://10.10.20.142:18088/v1 \
  TRAIN_BATCH_SIZE=32 \
  PPO_MINI_BATCH_SIZE=32 \
  TEST_FREQ=-1 \
  PODMAN_GPU_ARGS="nvidia.com/gpu=all" \
  CUDA_VISIBLE_DEVICES_IN_CONTAINER=0,1,2,3,4,5,6,7 \
  NGPUS_PER_NODE=8 \
  ./scripts/train/run_verl_uenv_grpo.sh
EOF
}

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
  usage
  exit 0
fi

# 路径配置。REPO_DIR 指向 uenv-bridge，VERL_WORKSPACE 指向挂载进容器的 VeRL 工作区。
REPO_DIR=${REPO_DIR:-"$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"}
source "${REPO_DIR}/scripts/lib/common.sh"
VERL_WORKSPACE=${VERL_WORKSPACE:-/data/podman/verl/workspace}
UENV_DEVICE_BACKEND=${UENV_DEVICE_BACKEND:-cuda}
UENV_DEVICE_BACKEND=$(normalize_device_backend "${UENV_DEVICE_BACKEND}")

# Server 侧已经启动的 Rust adapter core 地址
SERVER_ADAPTER_CORE_ENDPOINT=${SERVER_ADAPTER_CORE_ENDPOINT:-8.130.75.157:8088}
if [ -z "${SERVER_ADAPTER_CORE_ENDPOINT}" ]; then
  echo "SERVER_ADAPTER_CORE_ENDPOINT is required." >&2
  exit 1
fi

# VeRL policy model
IMAGE=${IMAGE:-localhost/uenv-bridge-verl:qwen35-torch210-vllm019-tf514-kernelfix}
DEFAULT_HOST_MODEL_PATH=/data/ronghao/models/modelscope/Qwen/Qwen2___5-0___5B-Instruct
DEFAULT_CONTAINER_MODEL_PATH=/models/modelscope/Qwen/Qwen2___5-0___5B-Instruct

MODEL_PATH=${MODEL_PATH:-${DEFAULT_HOST_MODEL_PATH}}
CONTAINER_MODEL_PATH=${CONTAINER_MODEL_PATH:-${DEFAULT_CONTAINER_MODEL_PATH}}

# 训练与数据参数。
TRAINING_STEPS=${TRAINING_STEPS:-null}
TRAIN_BATCH_SIZE=${TRAIN_BATCH_SIZE:-256}
PPO_MINI_BATCH_SIZE=${PPO_MINI_BATCH_SIZE:-64}
PPO_MICRO_BATCH_SIZE_PER_GPU=${PPO_MICRO_BATCH_SIZE_PER_GPU:-2}
ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=${ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU:-4}
REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=${REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU:-${ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU}}
MAX_PROMPT_LENGTH=${MAX_PROMPT_LENGTH:-512}
ROLLOUT_N=${ROLLOUT_N:-5}
ROLLOUT_TEMPERATURE=${ROLLOUT_TEMPERATURE:-1.0}
ROLLOUT_TP=${ROLLOUT_TP:-1}
ROLLOUT_CALCULATE_LOG_PROBS=${ROLLOUT_CALCULATE_LOG_PROBS:-False}
DATA_MAX_RESPONSE_LENGTH=${DATA_MAX_RESPONSE_LENGTH:-1024}
DATA_DIR=${DATA_DIR:-/data/ronghao/uenv/uenv-bridge/data/gsm8k}
CONTAINER_DATA_DIR=${CONTAINER_DATA_DIR:-/data/gsm8k}
INFER_BACKEND=${INFER_BACKEND:-vllm}


# VeRL rollout/runtime 资源参数。
ROLLOUT_FREE_CACHE_ENGINE=${ROLLOUT_FREE_CACHE_ENGINE:-False}
ROLLOUT_ENABLE_SLEEP_MODE=${ROLLOUT_ENABLE_SLEEP_MODE:-False}
ROLLOUT_GPU_MEMORY_UTILIZATION=${ROLLOUT_GPU_MEMORY_UTILIZATION:-0.8}
AGENT_NUM_WORKERS=${AGENT_NUM_WORKERS:-1}
CUDA_VISIBLE_DEVICES_IN_CONTAINER=${CUDA_VISIBLE_DEVICES_IN_CONTAINER:-"7"}
PODMAN_GPU_ARGS=${PODMAN_GPU_ARGS:-nvidia.com/gpu=all}
PODMAN_ASCEND_ARGS=${PODMAN_ASCEND_ARGS:-}
NGPUS_PER_NODE=${NGPUS_PER_NODE:-1}
RAY_NUM_CPUS=${RAY_NUM_CPUS:-$((NGPUS_PER_NODE * 4))}
RAY_NOSET_CUDA_VISIBLE_DEVICES=${RAY_NOSET_CUDA_VISIBLE_DEVICES:-$([ "${NGPUS_PER_NODE}" -gt 1 ] && printf 1 || printf 0)}
ASCEND_VISIBLE_DEVICES=${ASCEND_VISIBLE_DEVICES:-0}
ASCEND_RT_VISIBLE_DEVICES=${ASCEND_RT_VISIBLE_DEVICES:-${ASCEND_VISIBLE_DEVICES}}
TORCH_DEVICE_BACKEND_AUTOLOAD=${TORCH_DEVICE_BACKEND_AUTOLOAD:-0}
RAY_EXPERIMENTAL_NOSET_ASCEND_RT_VISIBLE_DEVICES=${RAY_EXPERIMENTAL_NOSET_ASCEND_RT_VISIBLE_DEVICES:-1}
PODMAN_NETWORK_ARGS=${PODMAN_NETWORK_ARGS:---network host}
UENV_PATCH_RESOURCE_TRACKER=${UENV_PATCH_RESOURCE_TRACKER:-1}
UENV_PATCH_VERL_VLLM_SHUTDOWN=${UENV_PATCH_VERL_VLLM_SHUTDOWN:-1}
UENV_PATCH_VERL_MODEL_VERSION_RESPONSE=${UENV_PATCH_VERL_MODEL_VERSION_RESPONSE:-1}
UENV_PATCH_VERL_TEXT_ONLY_PROCESSOR=${UENV_PATCH_VERL_TEXT_ONLY_PROCESSOR:-0}
UENV_PATCH_VERL_EMPTY_RESPONSE_BATCH=${UENV_PATCH_VERL_EMPTY_RESPONSE_BATCH:-1}
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
UENV_OBS_HEARTBEAT_INTERVAL_SECONDS=${UENV_OBS_HEARTBEAT_INTERVAL_SECONDS:-30}
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
EXPERIMENT_NAME=${EXPERIMENT_NAME:-uenv_layer4_grpo_$(date +%Y%m%d_%H%M)}
WANDB_ENV_ARGS=()
for wandb_var in WANDB_API_KEY WANDB_MODE WANDB_ENTITY WANDB_DIR WANDB_BASE_URL; do
  wandb_value=${!wandb_var:-}
  if [ -n "${wandb_value}" ]; then
    export "${wandb_var}=${wandb_value}"
    WANDB_ENV_ARGS+=("-e" "${wandb_var}")
  else
    unset "${wandb_var}" || true
  fi
done

# 日志目录。
RUN_ID=${RUN_ID:-layer4_distributed_$(date +%Y%m%d_%H%M%S)}
UENV_TRAINING_RUN_ID=${UENV_TRAINING_RUN_ID:-${RUN_ID}}
LOG_ROOT=${LOG_ROOT:-${REPO_DIR}/temp/logs}
SERVICE_DIR=${SERVICE_DIR:-${LOG_ROOT}/layer4_distributed/${RUN_ID}}
LOG_DIR=${LOG_DIR:-${LOG_ROOT}/verl_layer4_agent_loop}
LOG_FILE=${LOG_FILE:-${LOG_DIR}/${RUN_ID}.log}
CONTAINER_LOG_ROOT=${CONTAINER_LOG_ROOT:-/uenv/uenv-bridge/temp/logs}
CONTAINER_SERVICE_DIR=${CONTAINER_LOG_ROOT}/layer4_distributed/${RUN_ID}
AGENT_LOOP_RESULT_RECORD_PATH=${AGENT_LOOP_RESULT_RECORD_PATH:-${CONTAINER_SERVICE_DIR}/agent-loop-results.jsonl}
AGENT_LOOP_REQUEST_RECORD_PATH=${AGENT_LOOP_REQUEST_RECORD_PATH:-${CONTAINER_SERVICE_DIR}/agent-loop-requests.jsonl}
MODEL_GATEWAY_LOG_PATH=${MODEL_GATEWAY_LOG_PATH:-${CONTAINER_SERVICE_DIR}/model-gateway.jsonl}
CHECKPOINT_ROOT=${CHECKPOINT_ROOT:-${REPO_DIR}/checkpoints/uenv_grpo}
CHECKPOINT_RUN_DIR=${CHECKPOINT_RUN_DIR:-${CHECKPOINT_ROOT}/${RUN_ID}}
CONTAINER_CHECKPOINT_RUN_DIR=${CONTAINER_CHECKPOINT_RUN_DIR:-/uenv/uenv-bridge/checkpoints/uenv_grpo/${RUN_ID}}

infer_parquet_rows() {
  local parquet_path="$1"
  if [ ! -f "${parquet_path}" ]; then
    return 0
  fi
  python3 - "${parquet_path}" <<'PY' 2>/dev/null || true
import sys
try:
    import pyarrow.parquet as pq
    print(pq.ParquetFile(sys.argv[1]).metadata.num_rows)
except Exception:
    pass
PY
}

is_positive_int() {
  case "$1" in
    ''|*[!0-9]*)
      return 1
      ;;
    *)
      [ "$1" -gt 0 ]
      ;;
  esac
}

if [ -z "${UENV_OBS_PLANNED_STEP_TOTAL:-}" ]; then
  if is_positive_int "${TRAINING_STEPS}"; then
    UENV_OBS_PLANNED_STEP_TOTAL=${TRAINING_STEPS}
  else
    TRAIN_ROWS=$(infer_parquet_rows "${DATA_DIR}/train.parquet")
    if is_positive_int "${TRAIN_ROWS}" && is_positive_int "${TRAIN_BATCH_SIZE}" && is_positive_int "${TOTAL_EPOCHS}"; then
      UENV_OBS_PLANNED_STEP_TOTAL=$(( ((TRAIN_ROWS + TRAIN_BATCH_SIZE - 1) / TRAIN_BATCH_SIZE) * TOTAL_EPOCHS ))
    else
      UENV_OBS_PLANNED_STEP_TOTAL=
    fi
  fi
fi

if [ -z "${UENV_OBS_PLANNED_EPISODE_TOTAL:-}" ]; then
  TRAIN_ROWS_FOR_PLAN=$(infer_parquet_rows "${DATA_DIR}/train.parquet")
  if is_positive_int "${TRAINING_STEPS}" && is_positive_int "${TRAIN_BATCH_SIZE}" && is_positive_int "${ROLLOUT_N}"; then
    UENV_OBS_PLANNED_EPISODE_TOTAL=$(( TRAINING_STEPS * TRAIN_BATCH_SIZE * ROLLOUT_N ))
  elif is_positive_int "${TRAIN_ROWS_FOR_PLAN}" && is_positive_int "${ROLLOUT_N}" && is_positive_int "${TOTAL_EPOCHS}"; then
    UENV_OBS_PLANNED_EPISODE_TOTAL=$(( TRAIN_ROWS_FOR_PLAN * ROLLOUT_N * TOTAL_EPOCHS ))
  else
    UENV_OBS_PLANNED_EPISODE_TOTAL=
  fi
fi

mkdir -p "${DATA_DIR}" "${LOG_DIR}" "${SERVICE_DIR}" "${CHECKPOINT_RUN_DIR}"
write_json_metadata "${CHECKPOINT_RUN_DIR}/metadata.json" \
  "run_id=${RUN_ID}" \
  "training_run_id=${UENV_TRAINING_RUN_ID}" \
  "script=run_verl_uenv_grpo.sh" \
  "device_backend=${UENV_DEVICE_BACKEND}" \
  "image=${IMAGE}" \
  "model_path=${MODEL_PATH}" \
  "container_model_path=${CONTAINER_MODEL_PATH}" \
  "data_dir=${DATA_DIR}" \
  "container_data_dir=${CONTAINER_DATA_DIR}" \
  "infer_backend=${INFER_BACKEND}" \
  "train_batch_size=${TRAIN_BATCH_SIZE}" \
  "ppo_mini_batch_size=${PPO_MINI_BATCH_SIZE}" \
  "rollout_n=${ROLLOUT_N}" \
  "rollout_temperature=${ROLLOUT_TEMPERATURE}" \
  "rollout_tp=${ROLLOUT_TP}" \
  "ngpus_per_node=${NGPUS_PER_NODE}" \
  "max_prompt_length=${MAX_PROMPT_LENGTH}" \
  "data_max_response_length=${DATA_MAX_RESPONSE_LENGTH}" \
  "training_steps=${TRAINING_STEPS}" \
  "total_epochs=${TOTAL_EPOCHS}" \
  "obs_planned_step_total=${UENV_OBS_PLANNED_STEP_TOTAL}" \
  "obs_planned_episode_total=${UENV_OBS_PLANNED_EPISODE_TOTAL}" \
  "save_freq=${SAVE_FREQ}" \
  "test_freq=${TEST_FREQ}" \
  "checkpoint_run_dir=${CHECKPOINT_RUN_DIR}" \
  "container_checkpoint_run_dir=${CONTAINER_CHECKPOINT_RUN_DIR}"

PODMAN_DEVICE_RUN_ARGS=$(build_podman_accelerator_args "${UENV_DEVICE_BACKEND}" "${PODMAN_GPU_ARGS}" "${PODMAN_ASCEND_ARGS}")

run_verl_training() {
  if [ "${TRAINING_STEPS}" != "null" ] && [ "${TRAINING_STEPS}" -gt 0 ]; then
    echo "Running ${TRAINING_STEPS}-step GRPO with UEnv pre-rollout AgentLoop; log: ${LOG_FILE}"
  else
    echo "Running GRPO with UEnv pre-rollout AgentLoop; log: ${LOG_FILE}"
  fi
  echo "AgentLoop request records: ${SERVICE_DIR}/agent-loop-requests.jsonl"
  echo "AgentLoop result records: ${SERVICE_DIR}/agent-loop-results.jsonl"
  echo "Checkpoint dir: ${CHECKPOINT_RUN_DIR}"
  if [ -n "${UENV_OBS_URL}" ]; then
    echo "Frontend run: ${UENV_OBS_URL%/obs}/?run=${UENV_TRAINING_RUN_ID}"
  fi
  podman run --rm \
    ${PODMAN_NETWORK_ARGS} \
    ${PODMAN_DEVICE_RUN_ARGS} \
    --shm-size=64g \
    --entrypoint bash \
    --pids-limit=65536 \
    --workdir /workspace/verl \
    "${WANDB_ENV_ARGS[@]}" \
    -v "${VERL_WORKSPACE}:/workspace" \
    -v "${REPO_DIR}:/uenv/uenv-bridge" \
    -v "${CHECKPOINT_RUN_DIR}:${CONTAINER_CHECKPOINT_RUN_DIR}" \
    -v "${MODEL_PATH}:${CONTAINER_MODEL_PATH}:ro" \
    -v "${DATA_DIR}:${CONTAINER_DATA_DIR}:ro" \
    "${IMAGE}" \
    -lc "set -euo pipefail
cd /workspace/verl
export PYTHONPATH=/workspace/verl:/uenv/uenv-bridge/src
export PROTOCOL_BUFFERS_PYTHON_IMPLEMENTATION=python
if [ "${UENV_DEVICE_BACKEND}" = "ascend" ]; then
  if [ -f /usr/local/Ascend/ascend-toolkit/set_env.sh ]; then
    source /usr/local/Ascend/ascend-toolkit/set_env.sh
  fi
  if [ -f /usr/local/Ascend/nnal/atb/set_env.sh ]; then
    source /usr/local/Ascend/nnal/atb/set_env.sh
  fi
  if [ -d /usr/local/Ascend/driver/lib64/driver ]; then
    export LD_LIBRARY_PATH=/usr/local/Ascend/driver/lib64/driver:\${LD_LIBRARY_PATH:-}
  fi
  export ASCEND_VISIBLE_DEVICES=${ASCEND_VISIBLE_DEVICES}
  export ASCEND_RT_VISIBLE_DEVICES=${ASCEND_RT_VISIBLE_DEVICES}
  export TORCH_DEVICE_BACKEND_AUTOLOAD=${TORCH_DEVICE_BACKEND_AUTOLOAD}
  export RAY_EXPERIMENTAL_NOSET_ASCEND_RT_VISIBLE_DEVICES=${RAY_EXPERIMENTAL_NOSET_ASCEND_RT_VISIBLE_DEVICES}
  unset CUDA_VISIBLE_DEVICES
  export UENV_PATCH_TORCH_CUDA_IS_AVAILABLE_NO_DEVICES=0
  export UENV_PATCH_VERL_DEVICE_CAPABILITY_FALLBACK=0
else
  export CUDA_VISIBLE_DEVICES=${CUDA_VISIBLE_DEVICES_IN_CONTAINER}
fi
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
export UENV_PATCH_VERL_MODEL_VERSION_RESPONSE=${UENV_PATCH_VERL_MODEL_VERSION_RESPONSE}
export UENV_PATCH_VERL_TEXT_ONLY_PROCESSOR=${UENV_PATCH_VERL_TEXT_ONLY_PROCESSOR}
export UENV_PATCH_VERL_EMPTY_RESPONSE_BATCH=${UENV_PATCH_VERL_EMPTY_RESPONSE_BATCH}
export UENV_DEVICE_BACKEND=${UENV_DEVICE_BACKEND}
export UENV_AGENT_LOOP_BATCH=${UENV_AGENT_LOOP_BATCH}
export UENV_AGENT_LOOP_BATCH_SIZE=${UENV_AGENT_LOOP_BATCH_SIZE}
export UENV_AGENT_LOOP_BATCH_RETRY_ATTEMPTS=${UENV_AGENT_LOOP_BATCH_RETRY_ATTEMPTS}
export UENV_AGENT_LOOP_BATCH_RETRY_DELAY_SECONDS=${UENV_AGENT_LOOP_BATCH_RETRY_DELAY_SECONDS}
export UENV_AGENT_LOOP_PARALLEL_MODE=${UENV_AGENT_LOOP_PARALLEL_MODE}
export UENV_AGENT_LOOP_TIMEOUT_SECONDS=${UENV_AGENT_LOOP_TIMEOUT_SECONDS}
export UENV_EPISODE_MAX_STEPS_OVERRIDE=${UENV_EPISODE_MAX_STEPS_OVERRIDE}
export UENV_OBS_URL=\"${UENV_OBS_URL}\"
export UENV_OBS_TOKEN=\"${UENV_OBS_TOKEN}\"
export UENV_OBS_HEARTBEAT_INTERVAL_SECONDS=${UENV_OBS_HEARTBEAT_INTERVAL_SECONDS}
export UENV_TRAINING_RUN_ID=\"${UENV_TRAINING_RUN_ID}\"
export UENV_OBS_PLANNED_STEP_TOTAL=\"${UENV_OBS_PLANNED_STEP_TOTAL}\"
export UENV_OBS_PLANNED_EPISODE_TOTAL=\"${UENV_OBS_PLANNED_EPISODE_TOTAL}\"
export UENV_MODEL_GATEWAY_ENABLED=${UENV_MODEL_GATEWAY_ENABLED}
export UENV_MODEL_GATEWAY_BIND_HOST=${UENV_MODEL_GATEWAY_BIND_HOST}
export UENV_MODEL_GATEWAY_PORT=${UENV_MODEL_GATEWAY_PORT}
export UENV_MODEL_GATEWAY_PUBLIC_URL=${UENV_MODEL_GATEWAY_PUBLIC_URL}
export UENV_MODEL_GATEWAY_LOG_PATH=\"${MODEL_GATEWAY_LOG_PATH}\"
export UENV_MODEL_GATEWAY_DISABLE_THINKING=${UENV_MODEL_GATEWAY_DISABLE_THINKING}
export UENV_MODEL_GATEWAY_MAX_TOKENS=${UENV_MODEL_GATEWAY_MAX_TOKENS}
export UENV_MODEL_GATEWAY_STOP_ON_CLOSE=${UENV_MODEL_GATEWAY_STOP_ON_CLOSE}
export UENV_REQUIRE_SWE_RESPONSE_TRACE=${UENV_REQUIRE_SWE_RESPONSE_TRACE}
export UENV_AGENT_LOOP_FAILED_EPISODE_POLICY=${UENV_AGENT_LOOP_FAILED_EPISODE_POLICY}
pip install -q 'grpcio>=1.80' --break-system-packages 2>/dev/null || pip install -q 'grpcio>=1.80'
export UENV_AGENT_LOOP_CLIENT=rust_core
export UENV_ADAPTER_CORE_ENDPOINT=${SERVER_ADAPTER_CORE_ENDPOINT}
export UENV_ADAPTER_CORE_AUTO_START=0
export UENV_ADAPTER_CORE_BINARY=/uenv/uenv-bridge/core/target/debug/uenv-adapter-core
export UENV_ADAPTER_CORE_STARTUP_TIMEOUT_SECONDS=60
export UENV_ADAPTER_CORE_BACKEND=server
export UENV_ADAPTER_CORE_GRPC_MAX_MESSAGE_BYTES=${UENV_ADAPTER_CORE_GRPC_MAX_MESSAGE_BYTES}
export UENV_AGENT_LOOP_REQUEST_RECORD_PATH=\"${AGENT_LOOP_REQUEST_RECORD_PATH}\"
export UENV_AGENT_LOOP_RESULT_RECORD_PATH=\"${AGENT_LOOP_RESULT_RECORD_PATH}\"
python3 /uenv/uenv-bridge/scripts/run_verl_main_ppo.py \\
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
  actor_rollout_ref.rollout.temperature=${ROLLOUT_TEMPERATURE} \\
  actor_rollout_ref.rollout.gpu_memory_utilization=${ROLLOUT_GPU_MEMORY_UTILIZATION} \\
  actor_rollout_ref.rollout.n=${ROLLOUT_N} \\
  actor_rollout_ref.rollout.agent.num_workers=${AGENT_NUM_WORKERS} \\
  actor_rollout_ref.rollout.agent.default_agent_loop=uenv_agent \\
  actor_rollout_ref.rollout.agent.agent_loop_config_path=/uenv/uenv-bridge/configs/uenv-agent-loop.yaml \\
  actor_rollout_ref.rollout.log_prob_micro_batch_size_per_gpu=${ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU} \\
  actor_rollout_ref.rollout.enforce_eager=False \\
  actor_rollout_ref.rollout.enable_chunked_prefill=True \\
  actor_rollout_ref.rollout.free_cache_engine=${ROLLOUT_FREE_CACHE_ENGINE} \\
  +actor_rollout_ref.rollout.enable_sleep_mode=${ROLLOUT_ENABLE_SLEEP_MODE} \\
  actor_rollout_ref.rollout.max_num_seqs=8 \\
  actor_rollout_ref.rollout.max_num_batched_tokens=2048 \\
  actor_rollout_ref.rollout.calculate_log_probs=${ROLLOUT_CALCULATE_LOG_PROBS} \\
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
  trainer.val_before_train=False \\
  trainer.total_training_steps=${TRAINING_STEPS} \\
  trainer.total_epochs=${TOTAL_EPOCHS} \\
  trainer.resume_mode=disable \\
  trainer.default_local_dir=${CONTAINER_CHECKPOINT_RUN_DIR} \\
  ray_kwargs.ray_init.num_cpus=${RAY_NUM_CPUS} \\
  +ray_kwargs.ray_init.num_gpus=${NGPUS_PER_NODE} \\
  +ray_kwargs.ray_init.runtime_env.env_vars.PYTHONPATH=/workspace/verl:/uenv/uenv-bridge/src \\
  +ray_kwargs.ray_init.runtime_env.env_vars.PROTOCOL_BUFFERS_PYTHON_IMPLEMENTATION=python \\
  +ray_kwargs.ray_init.runtime_env.env_vars.UENV_DEVICE_BACKEND=${UENV_DEVICE_BACKEND} \\
  +ray_kwargs.ray_init.runtime_env.env_vars.UENV_OBS_URL=\\\"${UENV_OBS_URL}\\\" \\
  +ray_kwargs.ray_init.runtime_env.env_vars.UENV_OBS_TOKEN=\\\"${UENV_OBS_TOKEN}\\\" \\
  +ray_kwargs.ray_init.runtime_env.env_vars.UENV_OBS_HEARTBEAT_INTERVAL_SECONDS=\\\"${UENV_OBS_HEARTBEAT_INTERVAL_SECONDS}\\\" \\
  +ray_kwargs.ray_init.runtime_env.env_vars.UENV_TRAINING_RUN_ID=\\\"${UENV_TRAINING_RUN_ID}\\\" \\
  +ray_kwargs.ray_init.runtime_env.env_vars.UENV_OBS_PLANNED_STEP_TOTAL=\\\"${UENV_OBS_PLANNED_STEP_TOTAL}\\\" \\
  +ray_kwargs.ray_init.runtime_env.env_vars.UENV_OBS_PLANNED_EPISODE_TOTAL=\\\"${UENV_OBS_PLANNED_EPISODE_TOTAL}\\\" \\
  +ray_kwargs.ray_init.runtime_env.env_vars.ASCEND_VISIBLE_DEVICES=\\\"${ASCEND_VISIBLE_DEVICES}\\\" \\
  +ray_kwargs.ray_init.runtime_env.env_vars.ASCEND_RT_VISIBLE_DEVICES=\\\"${ASCEND_RT_VISIBLE_DEVICES}\\\" \\
  +ray_kwargs.ray_init.runtime_env.env_vars.TORCH_DEVICE_BACKEND_AUTOLOAD=\\\"${TORCH_DEVICE_BACKEND_AUTOLOAD}\\\" \\
  +ray_kwargs.ray_init.runtime_env.env_vars.RAY_EXPERIMENTAL_NOSET_ASCEND_RT_VISIBLE_DEVICES=\\\"${RAY_EXPERIMENTAL_NOSET_ASCEND_RT_VISIBLE_DEVICES}\\\" \\
  +ray_kwargs.ray_init.runtime_env.env_vars.UENV_PATCH_VERL_MODEL_VERSION_RESPONSE=enabled \\
  +ray_kwargs.ray_init.runtime_env.env_vars.UENV_PATCH_VERL_TEXT_ONLY_PROCESSOR=${UENV_PATCH_VERL_TEXT_ONLY_PROCESSOR_RAY} \\
  +ray_kwargs.ray_init.runtime_env.env_vars.UENV_PATCH_VERL_EMPTY_RESPONSE_BATCH=enabled \\
  +ray_kwargs.ray_init.include_dashboard=False \\
  ${EXTRA_VERL_ARGS}" 2>&1 | tee "${LOG_FILE}"
}

summarize_agent_loop_records() {
  python3 - "${SERVICE_DIR}" <<'PY'
import json
import sys
from collections import Counter
from pathlib import Path

service_dir = Path(sys.argv[1])
for filename in ("agent-loop-requests.jsonl", "agent-loop-results.jsonl", "model-gateway.jsonl"):
    path = service_dir / filename
    print(f"{filename}: {path}")
    if not path.exists():
        print("  missing")
        continue

    records = []
    with path.open(encoding="utf-8") as file:
        for line in file:
            line = line.strip()
            if line:
                records.append(json.loads(line))

    phases = Counter(record.get("phase") for record in records)
    batch_ids = Counter(record.get("batch_id") for record in records)
    status_codes = Counter(record.get("status_code") for record in records if "status_code" in record)
    upstreams = Counter(record.get("upstream_url") for record in records if "upstream_url" in record)
    sample_indexes = [
        record.get("sample_index")
        for record in records
        if isinstance(record.get("sample_index"), int)
    ]
    print(f"  lines: {len(records)}")
    if phases:
        print(f"  phases: {dict(phases)}")
    if batch_ids:
        print(f"  batch_ids: {dict(batch_ids)}")
    if status_codes:
        print(f"  status_codes: {dict(status_codes)}")
    if upstreams:
        print(f"  upstreams: {dict(upstreams)}")
    if sample_indexes:
        print(f"  sample_index_range: {min(sample_indexes)}..{max(sample_indexes)}")
PY
}

emit_obs_run_terminal() {
  local status=$1
  local reason=$2
  local exit_code=${3:-0}
  if [ -z "${UENV_OBS_URL}" ]; then
    return 0
  fi
  UENV_OBS_URL="${UENV_OBS_URL}" \
  UENV_OBS_TOKEN="${UENV_OBS_TOKEN}" \
  python3 - "${UENV_TRAINING_RUN_ID}" "${status}" "${reason}" "${exit_code}" <<'PY' || true
import json
import os
import sys
import time
import uuid
from urllib import error, request

run_id, status, reason, exit_code = sys.argv[1:5]
event_type = {
    "completed": "RUN_COMPLETED",
    "terminated": "RUN_TERMINATED",
    "failed": "RUN_FAILED",
}[status]
base = os.environ.get("UENV_OBS_URL", "").rstrip("/")
token = os.environ.get("UENV_OBS_TOKEN", "").strip()
if not base:
    raise SystemExit(0)
now_ms = int(time.time() * 1000)
event = {
    "event_id": str(uuid.uuid4()),
    "schema_version": "1",
    "correlation_id": f"run:{run_id}:terminal",
    "training_run_id": run_id,
    "source_id": f"train-script:{os.getpid()}",
    "module": "adapter",
    "entity_type": "training_run",
    "entity_id": run_id,
    "event_type": event_type,
    "seq": 1,
    "source_ts": now_ms,
    "payload": {
        "run_status": status,
        "terminal_reason": reason,
        "exit_code": int(exit_code),
    },
}
headers = {"Content-Type": "application/json"}
if token:
    headers["Authorization"] = f"Bearer {token}"
    headers["X-Obs-Token"] = token
req = request.Request(
    f"{base}/api/v1/events",
    data=json.dumps(event).encode("utf-8"),
    headers=headers,
    method="POST",
)
try:
    with request.urlopen(req, timeout=5.0) as resp:
        resp.read()
except error.URLError as exc:
    print(f"obs terminal event failed: {exc}", file=sys.stderr)
PY
}

wait_for_addr "server-side adapter core" "${SERVER_ADAPTER_CORE_ENDPOINT}" 20
ensure_policy_model_exists

set +e
run_verl_training
run_status=$?
set -e

if [ "${run_status}" -ne 0 ]; then
  if [ "${run_status}" -eq 130 ] || [ "${run_status}" -eq 143 ]; then
    emit_obs_run_terminal "terminated" "terminated" "${run_status}"
  else
    emit_obs_run_terminal "failed" "failed" "${run_status}"
  fi
  echo "VeRL UEnv GRPO training failed. VeRL log: ${LOG_FILE}" >&2
  tail -120 "${LOG_FILE}" >&2 2>/dev/null || true
  exit "${run_status}"
fi

emit_obs_run_terminal "completed" "completed" "0"
echo "VeRL UEnv GRPO training completed."
echo "VeRL log: ${LOG_FILE}"
grep -E "Training Progress: 100%|critic/score/mean|critic/rewards/mean" "${LOG_FILE}" | tail -5 || true
summarize_agent_loop_records
