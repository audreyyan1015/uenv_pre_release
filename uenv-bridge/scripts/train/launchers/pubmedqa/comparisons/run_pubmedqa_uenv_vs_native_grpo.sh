#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Run a serial PubMedQA comparison:
  1. UEnv + VeRL GRPO training
  2. Native VeRL GRPO training
  3. Merge each final VeRL FSDP actor checkpoint to HuggingFace format
  4. Evaluate both merged models with the same PubMedQA evaluator

Default is a 2-step smoke run. Full run example:
  DATASET_MODE=full TRAINING_STEPS=500 EVAL_LIMIT= ./scripts/train/launchers/pubmedqa/comparisons/run_pubmedqa_uenv_vs_native_grpo.sh

Useful overrides:
  DATASET_MODE=smoke|full   Default: smoke
  TRAINING_STEPS=2          Use 500 for the planned full comparison
  EVAL_LIMIT=16             Empty value means evaluate all rows in DATA_PATH
  RUN_TS=20260823_220000    Reuse a stable timestamp/run namespace
  SKIP_MERGE=1              Only train, do not merge checkpoints
  SKIP_EVAL=1               Only train and merge, do not evaluate
EOF
}

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
  usage
  exit 0
fi

REPO_DIR=${REPO_DIR:-"$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../../.." && pwd)"}
cd "${REPO_DIR}"

IMAGE=${IMAGE:-localhost/uenv-bridge-verl:qwen35-torch210-vllm019-tf514-kernelfix}
VERL_WORKSPACE=${VERL_WORKSPACE:-/data/podman/verl/workspace}
MODEL_PATH=${MODEL_PATH:-/data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B}
CONTAINER_MODEL_PATH=${CONTAINER_MODEL_PATH:-/models/modelscope/Qwen/Qwen3___6-35B-A3B}

RUN_TS=${RUN_TS:-$(date +%Y%m%d_%H%M%S)}
RUN_PREFIX=${RUN_PREFIX:-pubmedqa_grpo_compare}
UENV_RUN_ID=${UENV_RUN_ID:-${RUN_PREFIX}_uenv_${RUN_TS}}
NATIVE_RUN_ID=${NATIVE_RUN_ID:-${RUN_PREFIX}_native_${RUN_TS}}

DATASET_MODE=${DATASET_MODE:-smoke}
FULL_DATA_DIR=${FULL_DATA_DIR:-${REPO_DIR}/data/benchmarks/pubmedqa_verl_90_10}
SMOKE_DATA_DIR=${SMOKE_DATA_DIR:-${REPO_DIR}/temp/training_data/pubmedqa_smoke}
RAW_PUBMEDQA_JSON=${RAW_PUBMEDQA_JSON:-${REPO_DIR}/data/benchmarks/pubmedqa/ori_pqal.json}
CONTAINER_DATA_DIR=${CONTAINER_DATA_DIR:-/data/pubmedqa}

TRAINING_STEPS=${TRAINING_STEPS:-2}
TRAIN_BATCH_SIZE=${TRAIN_BATCH_SIZE:-2}
PPO_MINI_BATCH_SIZE=${PPO_MINI_BATCH_SIZE:-2}
PPO_MICRO_BATCH_SIZE_PER_GPU=${PPO_MICRO_BATCH_SIZE_PER_GPU:-1}
ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=${ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU:-1}
REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=${REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU:-1}
ROLLOUT_N=${ROLLOUT_N:-4}
ROLLOUT_TP=${ROLLOUT_TP:-8}
NGPUS_PER_NODE=${NGPUS_PER_NODE:-8}
PODMAN_GPU_ARGS=${PODMAN_GPU_ARGS:-nvidia.com/gpu=all}
CUDA_VISIBLE_DEVICES_IN_CONTAINER=${CUDA_VISIBLE_DEVICES_IN_CONTAINER:-0,1,2,3,4,5,6,7}
RAY_NUM_CPUS=${RAY_NUM_CPUS:-$((NGPUS_PER_NODE * 4))}
MAX_PROMPT_LENGTH=${MAX_PROMPT_LENGTH:-4096}
DATA_MAX_RESPONSE_LENGTH=${DATA_MAX_RESPONSE_LENGTH:-128}
ROLLOUT_MAX_MODEL_LEN=${ROLLOUT_MAX_MODEL_LEN:-8192}
ROLLOUT_MAX_NUM_SEQS=${ROLLOUT_MAX_NUM_SEQS:-8}
ROLLOUT_MAX_NUM_BATCHED_TOKENS=${ROLLOUT_MAX_NUM_BATCHED_TOKENS:-16384}
ROLLOUT_GPU_MEMORY_UTILIZATION=${ROLLOUT_GPU_MEMORY_UTILIZATION:-0.50}
ROLLOUT_FREE_CACHE_ENGINE=${ROLLOUT_FREE_CACHE_ENGINE:-True}
ROLLOUT_ENABLE_SLEEP_MODE=${ROLLOUT_ENABLE_SLEEP_MODE:-True}
ROLLOUT_CALCULATE_LOG_PROBS=${ROLLOUT_CALCULATE_LOG_PROBS:-False}
ROLLOUT_TEMPERATURE=${ROLLOUT_TEMPERATURE:-1.0}

SAVE_FREQ=${SAVE_FREQ:-${TRAINING_STEPS}}
TEST_FREQ=${TEST_FREQ:--1}
VAL_BEFORE_TRAIN=${VAL_BEFORE_TRAIN:-False}
TRAINER_LOGGER=${TRAINER_LOGGER:-"['console']"}

SERVER_ADAPTER_CORE_ENDPOINT=${SERVER_ADAPTER_CORE_ENDPOINT:-8.130.75.157:8088}
UENV_OBS_URL=${UENV_OBS_URL:-http://8.130.75.157:8888/obs}
UENV_OBS_TOKEN=${UENV_OBS_TOKEN:-}
UENV_MODEL_GATEWAY_ENABLED=${UENV_MODEL_GATEWAY_ENABLED:-1}
UENV_MODEL_GATEWAY_PORT=${UENV_MODEL_GATEWAY_PORT:-18088}
UENV_MODEL_GATEWAY_PUBLIC_URL=${UENV_MODEL_GATEWAY_PUBLIC_URL:-http://10.10.20.142:${UENV_MODEL_GATEWAY_PORT}/v1}
UENV_MODEL_GATEWAY_MAX_TOKENS=${UENV_MODEL_GATEWAY_MAX_TOKENS:-${DATA_MAX_RESPONSE_LENGTH}}
UENV_MODEL_GATEWAY_DISABLE_THINKING=${UENV_MODEL_GATEWAY_DISABLE_THINKING:-1}
UENV_AGENT_LOOP_PARALLEL_MODE=${UENV_AGENT_LOOP_PARALLEL_MODE:-sync}
UENV_AGENT_LOOP_FAILED_EPISODE_POLICY=${UENV_AGENT_LOOP_FAILED_EPISODE_POLICY:-raise}
UENV_REQUIRE_SWE_RESPONSE_TRACE=${UENV_REQUIRE_SWE_RESPONSE_TRACE:-0}
UENV_DEFAULT_ENV_TYPE=${UENV_DEFAULT_ENV_TYPE:-qa}
UENV_EPISODE_MAX_STEPS_OVERRIDE=${UENV_EPISODE_MAX_STEPS_OVERRIDE:-1}
AGENT_NUM_WORKERS=${AGENT_NUM_WORKERS:-1}

EVAL_BACKEND=${EVAL_BACKEND:-vllm}
EVAL_INFERENCE_MODE=${EVAL_INFERENCE_MODE:-generate}
EVAL_PROMPT_STYLE=${EVAL_PROMPT_STYLE:-official}
EVAL_MAX_TOKENS=${EVAL_MAX_TOKENS:-128}
EVAL_MAX_MODEL_LEN=${EVAL_MAX_MODEL_LEN:-4096}
EVAL_TENSOR_PARALLEL_SIZE=${EVAL_TENSOR_PARALLEL_SIZE:-8}
EVAL_GPU_MEMORY_UTILIZATION=${EVAL_GPU_MEMORY_UTILIZATION:-0.75}
EVAL_VLLM_LABEL_BATCH_SIZE=${EVAL_VLLM_LABEL_BATCH_SIZE:-64}
EVAL_LABEL_SCORE_NORMALIZATION=${EVAL_LABEL_SCORE_NORMALIZATION:-mean}
SKIP_MERGE=${SKIP_MERGE:-0}
SKIP_EVAL=${SKIP_EVAL:-0}

EXPERIMENT_ROOT=${EXPERIMENT_ROOT:-${REPO_DIR}/temp/experiments/pubmedqa_uenv_vs_native/${RUN_TS}}
LOG_ROOT=${LOG_ROOT:-${EXPERIMENT_ROOT}/logs}
RESULT_ROOT=${RESULT_ROOT:-${EXPERIMENT_ROOT}/eval}
CHECKPOINT_BASE=${CHECKPOINT_BASE:-${REPO_DIR}/checkpoints/pubmedqa_uenv_vs_native/${RUN_TS}}
mkdir -p "${LOG_ROOT}" "${RESULT_ROOT}" "${CHECKPOINT_BASE}"

case "${TRAINING_STEPS}" in
  ''|*[!0-9]*)
    echo "TRAINING_STEPS must be a positive integer for this comparison script." >&2
    exit 2
    ;;
esac
if [ "${TRAINING_STEPS}" -le 0 ]; then
  echo "TRAINING_STEPS must be positive." >&2
  exit 2
fi

case "${DATASET_MODE}" in
  smoke)
    TRAIN_DATA_DIR=${TRAIN_DATA_DIR:-${SMOKE_DATA_DIR}}
    EVAL_DATA_PATH=${EVAL_DATA_PATH:-${RAW_PUBMEDQA_JSON}}
    if [ -z "${EVAL_LIMIT+x}" ]; then
      EVAL_LIMIT=16
    fi
    ;;
  full)
    TRAIN_DATA_DIR=${TRAIN_DATA_DIR:-${FULL_DATA_DIR}}
    if [ ! -f "${TRAIN_DATA_DIR}/train.parquet" ] || [ ! -f "${TRAIN_DATA_DIR}/test.parquet" ]; then
      python3 scripts/utils/prepare_verl_pubmedqa_train.py \
        --input "${RAW_PUBMEDQA_JSON}" \
        --output-dir "${TRAIN_DATA_DIR}"
    fi
    EVAL_DATA_PATH=${EVAL_DATA_PATH:-${TRAIN_DATA_DIR}/eval_pqal.json}
    if [ -z "${EVAL_LIMIT+x}" ]; then
      EVAL_LIMIT=
    fi
    ;;
  *)
    echo "DATASET_MODE must be smoke or full." >&2
    exit 2
    ;;
esac

count_parquet_rows() {
  python3 - "$1" <<'PY'
import sys
import pandas as pd
print(len(pd.read_parquet(sys.argv[1])))
PY
}

TRAIN_ROWS=$(count_parquet_rows "${TRAIN_DATA_DIR}/train.parquet")
STEPS_PER_EPOCH=$(( (TRAIN_ROWS + TRAIN_BATCH_SIZE - 1) / TRAIN_BATCH_SIZE ))
if [ "${STEPS_PER_EPOCH}" -le 0 ]; then
  echo "No training rows found in ${TRAIN_DATA_DIR}/train.parquet." >&2
  exit 2
fi
if [ -z "${TOTAL_EPOCHS+x}" ]; then
  TOTAL_EPOCHS=$(( (TRAINING_STEPS + STEPS_PER_EPOCH - 1) / STEPS_PER_EPOCH ))
fi

write_experiment_metadata() {
  python3 - "$EXPERIMENT_ROOT/metadata.json" <<PY
import json
import sys

metadata = {
    "run_ts": "${RUN_TS}",
    "dataset_mode": "${DATASET_MODE}",
    "train_data_dir": "${TRAIN_DATA_DIR}",
    "eval_data_path": "${EVAL_DATA_PATH}",
    "eval_limit": "${EVAL_LIMIT}",
    "training_steps": int("${TRAINING_STEPS}"),
    "total_epochs": int("${TOTAL_EPOCHS}"),
    "train_rows": int("${TRAIN_ROWS}"),
    "train_batch_size": int("${TRAIN_BATCH_SIZE}"),
    "rollout_n": int("${ROLLOUT_N}"),
    "uenv_run_id": "${UENV_RUN_ID}",
    "native_run_id": "${NATIVE_RUN_ID}",
    "checkpoint_base": "${CHECKPOINT_BASE}",
}
with open(sys.argv[1], "w", encoding="utf-8") as f:
    json.dump(metadata, f, ensure_ascii=False, indent=2)
    f.write("\\n")
PY
}

latest_actor_dir() {
  python3 - "$1" <<'PY'
import re
import sys
from pathlib import Path

run_dir = Path(sys.argv[1])
latest_file = run_dir / "latest_checkpointed_iteration.txt"
if latest_file.exists():
    value = latest_file.read_text(encoding="utf-8").strip()
    if value:
        actor = run_dir / f"global_step_{value}" / "actor"
        if actor.exists():
            print(actor)
            raise SystemExit(0)

candidates = []
for path in run_dir.glob("global_step_*/actor"):
    match = re.search(r"global_step_(\\d+)$", path.parent.name)
    if match:
        candidates.append((int(match.group(1)), path))
if not candidates:
    raise SystemExit(f"no actor checkpoint found under {run_dir}")
print(max(candidates, key=lambda item: item[0])[1])
PY
}

has_hf_weights() {
  python3 - "$1" <<'PY'
import sys
from pathlib import Path
p = Path(sys.argv[1])
exists = (p / "model.safetensors.index.json").exists() or any(p.glob("*.safetensors")) or any(p.glob("pytorch_model*.bin"))
raise SystemExit(0 if exists else 1)
PY
}

merge_actor_checkpoint() {
  local label="$1"
  local actor_dir="$2"
  local target_dir="$3"
  local merge_log="${LOG_ROOT}/${label}_merge.log"

  if [ "${SKIP_MERGE}" = "1" ]; then
    return 0
  fi
  if has_hf_weights "${target_dir}"; then
    return 0
  fi

  echo "Merging ${label} checkpoint: ${actor_dir}"
  podman run --rm \
    --entrypoint bash \
    --network host \
    --pids-limit=-1 \
    --shm-size=64g \
    -v "${VERL_WORKSPACE}:/workspace" \
    -v "${REPO_DIR}:/uenv/uenv-bridge" \
    -v /data/ronghao:/data/ronghao \
    -w /workspace/verl \
    "${IMAGE}" \
    -lc "set -euo pipefail
export PYTHONPATH=/workspace/verl:/uenv/uenv-bridge/src
export UENV_PATCH_VERL_TEXT_ONLY_PROCESSOR=1
python3 -m verl.model_merger merge \
  --backend fsdp \
  --use_cpu_initialization \
  --local_dir '${actor_dir}' \
  --target_dir '${target_dir}'" 2>&1 | tee "${merge_log}"
}

evaluate_hf_model() {
  local label="$1"
  local model_dir="$2"
  local output_dir="${RESULT_ROOT}/${label}"
  local eval_log="${LOG_ROOT}/${label}_eval.log"

  if [ "${SKIP_EVAL}" = "1" ]; then
    return 0
  fi

  echo "Evaluating ${label} model: ${model_dir}"
  (
    export IMAGE="${IMAGE}"
    export MODEL_DIR="${model_dir}"
    export DATA_PATH="${EVAL_DATA_PATH}"
    export OUTPUT_DIR="${output_dir}"
    export LIMIT="${EVAL_LIMIT}"
    export BACKEND="${EVAL_BACKEND}"
    export INFERENCE_MODE="${EVAL_INFERENCE_MODE}"
    export PROMPT_STYLE="${EVAL_PROMPT_STYLE}"
    export MAX_TOKENS="${EVAL_MAX_TOKENS}"
    export MAX_MODEL_LEN="${EVAL_MAX_MODEL_LEN}"
    export TENSOR_PARALLEL_SIZE="${EVAL_TENSOR_PARALLEL_SIZE}"
    export VLLM_LABEL_BATCH_SIZE="${EVAL_VLLM_LABEL_BATCH_SIZE}"
    export LABEL_SCORE_NORMALIZATION="${EVAL_LABEL_SCORE_NORMALIZATION}"
    export PODMAN_EXTRA_ARGS="-e UENV_PATCH_VERL_TEXT_ONLY_PROCESSOR=1 -e PYTHONPATH=/data/ronghao/uenv/uenv-bridge/src"
    ./scripts/benchmark/pubmedqa/run_pubmedqa_baseline.sh
  ) 2>&1 | tee "${eval_log}"
}

run_uenv_train() {
  echo "Starting UEnv + VeRL training: ${UENV_RUN_ID}"
  (
    export IMAGE="${IMAGE}"
    export RUN_ID="${UENV_RUN_ID}"
    export UENV_TRAINING_RUN_ID="${UENV_RUN_ID}"
    export MODEL_PATH="${MODEL_PATH}"
    export CONTAINER_MODEL_PATH="${CONTAINER_MODEL_PATH}"
    export DATA_DIR="${TRAIN_DATA_DIR}"
    export CONTAINER_DATA_DIR="${CONTAINER_DATA_DIR}"
    export CHECKPOINT_ROOT="${CHECKPOINT_BASE}/uenv"
    export LOG_ROOT="${LOG_ROOT}/uenv"
    export SERVER_ADAPTER_CORE_ENDPOINT="${SERVER_ADAPTER_CORE_ENDPOINT}"
    export UENV_OBS_URL="${UENV_OBS_URL}"
    export UENV_OBS_TOKEN="${UENV_OBS_TOKEN}"
    export UENV_MODEL_GATEWAY_ENABLED="${UENV_MODEL_GATEWAY_ENABLED}"
    export UENV_MODEL_GATEWAY_PORT="${UENV_MODEL_GATEWAY_PORT}"
    export UENV_MODEL_GATEWAY_PUBLIC_URL="${UENV_MODEL_GATEWAY_PUBLIC_URL}"
    export UENV_MODEL_GATEWAY_MAX_TOKENS="${UENV_MODEL_GATEWAY_MAX_TOKENS}"
    export UENV_MODEL_GATEWAY_DISABLE_THINKING="${UENV_MODEL_GATEWAY_DISABLE_THINKING}"
    export UENV_AGENT_LOOP_PARALLEL_MODE="${UENV_AGENT_LOOP_PARALLEL_MODE}"
    export UENV_AGENT_LOOP_FAILED_EPISODE_POLICY="${UENV_AGENT_LOOP_FAILED_EPISODE_POLICY}"
    export UENV_REQUIRE_SWE_RESPONSE_TRACE="${UENV_REQUIRE_SWE_RESPONSE_TRACE}"
    export UENV_DEFAULT_ENV_TYPE="${UENV_DEFAULT_ENV_TYPE}"
    export UENV_EPISODE_MAX_STEPS_OVERRIDE="${UENV_EPISODE_MAX_STEPS_OVERRIDE}"
    export TRAINING_STEPS="${TRAINING_STEPS}"
    export TOTAL_EPOCHS="${TOTAL_EPOCHS}"
    export TRAIN_BATCH_SIZE="${TRAIN_BATCH_SIZE}"
    export PPO_MINI_BATCH_SIZE="${PPO_MINI_BATCH_SIZE}"
    export PPO_MICRO_BATCH_SIZE_PER_GPU="${PPO_MICRO_BATCH_SIZE_PER_GPU}"
    export ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU="${ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU}"
    export REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU="${REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU}"
    export MAX_PROMPT_LENGTH="${MAX_PROMPT_LENGTH}"
    export DATA_MAX_RESPONSE_LENGTH="${DATA_MAX_RESPONSE_LENGTH}"
    export ROLLOUT_N="${ROLLOUT_N}"
    export ROLLOUT_TEMPERATURE="${ROLLOUT_TEMPERATURE}"
    export ROLLOUT_TP="${ROLLOUT_TP}"
    export NGPUS_PER_NODE="${NGPUS_PER_NODE}"
    export AGENT_NUM_WORKERS="${AGENT_NUM_WORKERS}"
    export PODMAN_GPU_ARGS="${PODMAN_GPU_ARGS}"
    export CUDA_VISIBLE_DEVICES_IN_CONTAINER="${CUDA_VISIBLE_DEVICES_IN_CONTAINER}"
    export RAY_NUM_CPUS="${RAY_NUM_CPUS}"
    export ROLLOUT_GPU_MEMORY_UTILIZATION="${ROLLOUT_GPU_MEMORY_UTILIZATION}"
    export ROLLOUT_FREE_CACHE_ENGINE="${ROLLOUT_FREE_CACHE_ENGINE}"
    export ROLLOUT_ENABLE_SLEEP_MODE="${ROLLOUT_ENABLE_SLEEP_MODE}"
    export ROLLOUT_CALCULATE_LOG_PROBS="${ROLLOUT_CALCULATE_LOG_PROBS}"
    export TEST_FREQ="${TEST_FREQ}"
    export SAVE_FREQ="${SAVE_FREQ}"
    export TRAINER_LOGGER="${TRAINER_LOGGER}"
    export TRAINER_PROJECT_NAME="uenv_pubmedqa_grpo_compare"
    export EXPERIMENT_NAME="${UENV_RUN_ID}"
    ./scripts/train/launchers/pubmedqa/pubmedqa_uenv_grpo_smoke.sh
  )
}

run_native_train() {
  echo "Starting native VeRL training: ${NATIVE_RUN_ID}"
  (
    export IMAGE="${IMAGE}"
    export RUN_ID="${NATIVE_RUN_ID}"
    export MODEL_PATH="${MODEL_PATH}"
    export CONTAINER_MODEL_PATH="${CONTAINER_MODEL_PATH}"
    export DATA_DIR="${TRAIN_DATA_DIR}"
    export CONTAINER_DATA_DIR="${CONTAINER_DATA_DIR}"
    export CHECKPOINT_ROOT="${CHECKPOINT_BASE}/native"
    export LOG_DIR="${LOG_ROOT}/native"
    export TRAINING_STEPS="${TRAINING_STEPS}"
    export TOTAL_EPOCHS="${TOTAL_EPOCHS}"
    export TRAIN_BATCH_SIZE="${TRAIN_BATCH_SIZE}"
    export PPO_MINI_BATCH_SIZE="${PPO_MINI_BATCH_SIZE}"
    export PPO_MICRO_BATCH_SIZE_PER_GPU="${PPO_MICRO_BATCH_SIZE_PER_GPU}"
    export ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU="${ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU}"
    export REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU="${REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU}"
    export MAX_PROMPT_LENGTH="${MAX_PROMPT_LENGTH}"
    export DATA_MAX_RESPONSE_LENGTH="${DATA_MAX_RESPONSE_LENGTH}"
    export ROLLOUT_N="${ROLLOUT_N}"
    export ROLLOUT_TP="${ROLLOUT_TP}"
    export NGPUS_PER_NODE="${NGPUS_PER_NODE}"
    export PODMAN_GPU_ARGS="${PODMAN_GPU_ARGS}"
    export CUDA_VISIBLE_DEVICES_IN_CONTAINER="${CUDA_VISIBLE_DEVICES_IN_CONTAINER}"
    export RAY_NUM_CPUS="${RAY_NUM_CPUS}"
    export ROLLOUT_GPU_MEMORY_UTILIZATION="${ROLLOUT_GPU_MEMORY_UTILIZATION}"
    export ROLLOUT_FREE_CACHE_ENGINE="${ROLLOUT_FREE_CACHE_ENGINE}"
    export ROLLOUT_ENABLE_SLEEP_MODE="${ROLLOUT_ENABLE_SLEEP_MODE}"
    export ROLLOUT_MAX_NUM_SEQS="${ROLLOUT_MAX_NUM_SEQS}"
    export ROLLOUT_MAX_NUM_BATCHED_TOKENS="${ROLLOUT_MAX_NUM_BATCHED_TOKENS}"
    export ROLLOUT_MAX_MODEL_LEN="${ROLLOUT_MAX_MODEL_LEN}"
    export ROLLOUT_CALCULATE_LOG_PROBS="${ROLLOUT_CALCULATE_LOG_PROBS}"
    export VAL_BEFORE_TRAIN="${VAL_BEFORE_TRAIN}"
    export TEST_FREQ="${TEST_FREQ}"
    export SAVE_FREQ="${SAVE_FREQ}"
    export TRAINER_LOGGER="${TRAINER_LOGGER}"
    export TRAINER_PROJECT_NAME="native_pubmedqa_grpo_compare"
    export EXPERIMENT_NAME="${NATIVE_RUN_ID}"
    ./scripts/train/launchers/pubmedqa/run_verl_pubmedqa_grpo.sh
  )
}

write_summary() {
  python3 - "$EXPERIMENT_ROOT/summary.json" "$RESULT_ROOT/uenv/metrics.json" "$RESULT_ROOT/native/metrics.json" <<'PY'
import json
import sys
from pathlib import Path

summary = {}
for label, path in (("uenv", Path(sys.argv[2])), ("native", Path(sys.argv[3]))):
    if path.exists():
        summary[label] = json.loads(path.read_text(encoding="utf-8"))
    else:
        summary[label] = None
Path(sys.argv[1]).write_text(json.dumps(summary, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
print(json.dumps(summary, ensure_ascii=False, indent=2))
PY
}

write_experiment_metadata

echo "Experiment root: ${EXPERIMENT_ROOT}"
echo "Checkpoint base: ${CHECKPOINT_BASE}"
echo "Dataset mode: ${DATASET_MODE}, train rows: ${TRAIN_ROWS}, total epochs: ${TOTAL_EPOCHS}, steps: ${TRAINING_STEPS}"
echo "Eval data: ${EVAL_DATA_PATH}, eval limit: ${EVAL_LIMIT:-all}"

run_uenv_train
UENV_ACTOR_DIR=$(latest_actor_dir "${CHECKPOINT_BASE}/uenv/${UENV_RUN_ID}")
UENV_HF_DIR="${UENV_ACTOR_DIR}/huggingface"
merge_actor_checkpoint uenv "${UENV_ACTOR_DIR}" "${UENV_HF_DIR}"
evaluate_hf_model uenv "${UENV_HF_DIR}"

run_native_train
NATIVE_ACTOR_DIR=$(latest_actor_dir "${CHECKPOINT_BASE}/native/${NATIVE_RUN_ID}")
NATIVE_HF_DIR="${NATIVE_ACTOR_DIR}/huggingface"
merge_actor_checkpoint native "${NATIVE_ACTOR_DIR}" "${NATIVE_HF_DIR}"
evaluate_hf_model native "${NATIVE_HF_DIR}"

write_summary
echo "Comparison completed: ${EXPERIMENT_ROOT}"
