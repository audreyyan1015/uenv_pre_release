#!/usr/bin/env bash
set -euo pipefail

REPO_DIR=${REPO_DIR:-"$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"}
GEN_IMAGE=${GEN_IMAGE:-localhost/vllm-openai:v0.19.0-cu130}
EVAL_IMAGE=${EVAL_IMAGE:-localhost/uenv-bridge-verl:layer4-build}
MODEL_ID=${MODEL_ID:-Qwen/Qwen3.6-35B-A3B}
MODEL_DIR=${MODEL_DIR:-/data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B}
DATA_DIR=${DATA_DIR:-${REPO_DIR}/data/benchmarks/swebenchpro}
DATA_FILE=${DATA_FILE:-${DATA_DIR}/test.jsonl}
DATA_CSV=${DATA_CSV:-${DATA_DIR}/swe_bench_pro_full.csv}
OUTPUT_DIR=${OUTPUT_DIR:-${REPO_DIR}/temp/benchmarks/swebenchpro/qwen3_6_35b_a3b_full}
OFFICIAL_EVAL_DIR=${OFFICIAL_EVAL_DIR:-/data/ronghao/third_party/SWE-bench_Pro-os}

HF_ENDPOINT=${HF_ENDPOINT:-https://hf-mirror.com}
TENSOR_PARALLEL_SIZE=${TENSOR_PARALLEL_SIZE:-8}
MAX_MODEL_LEN=${MAX_MODEL_LEN:-16384}
MAX_TOKENS=${MAX_TOKENS:-4096}
GPU_MEMORY_UTILIZATION=${GPU_MEMORY_UTILIZATION:-0.9}
TEMPERATURE=${TEMPERATURE:-0.2}
TOP_P=${TOP_P:-1.0}
DISABLE_THINKING=${DISABLE_THINKING:-1}
PREFIX=${PREFIX:-qwen3_6_35b_a3b}
LIMIT=${LIMIT-}

RUN_PREPARE=${RUN_PREPARE:-1}
RUN_GENERATE=${RUN_GENERATE:-1}
RUN_SUMMARIZE=${RUN_SUMMARIZE:-1}
RUN_DOWNLOAD_OFFICIAL_ASSETS=${RUN_DOWNLOAD_OFFICIAL_ASSETS:-0}
RUN_OFFICIAL_EVALUATE=${RUN_OFFICIAL_EVALUATE:-0}

DOCKERHUB_USERNAME=${DOCKERHUB_USERNAME:-jefzda}
OFFICIAL_ASSET_BASE_URL=${OFFICIAL_ASSET_BASE_URL:-https://cdn.jsdelivr.net/gh/scaleapi/SWE-bench_Pro-os@main}
OFFICIAL_ASSET_TIMEOUT_SECONDS=${OFFICIAL_ASSET_TIMEOUT_SECONDS:-60}
OFFICIAL_ASSET_RETRIES=${OFFICIAL_ASSET_RETRIES:-3}
OFFICIAL_ASSET_WORKERS=${OFFICIAL_ASSET_WORKERS:-16}
OFFICIAL_NUM_WORKERS=${OFFICIAL_NUM_WORKERS:-1}
OFFICIAL_REDO=${OFFICIAL_REDO:-0}
INSTALL_OFFICIAL_DEPS=${INSTALL_OFFICIAL_DEPS:-0}
OFFICIAL_VENV=${OFFICIAL_VENV:-${REPO_DIR}/temp/venvs/swebenchpro_eval}
PIP_INDEX_URL=${PIP_INDEX_URL:-https://pypi.tuna.tsinghua.edu.cn/simple}

mkdir -p "${DATA_DIR}" "${OUTPUT_DIR}"

LIMIT_ARG=()
if [ -n "${LIMIT}" ]; then
  LIMIT_ARG=(--limit "${LIMIT}")
fi

DISABLE_THINKING_ARG=()
if [ "${DISABLE_THINKING}" = "1" ]; then
  DISABLE_THINKING_ARG=(--disable-thinking)
fi

if [ "${RUN_PREPARE}" = "1" ]; then
  podman run --rm \
    --entrypoint bash \
    --network host \
    --pids-limit=-1 \
    --shm-size=16g \
    -v /data/ronghao:/data/ronghao \
    -w "${REPO_DIR}" \
    -e HF_ENDPOINT="${HF_ENDPOINT}" \
    "${EVAL_IMAGE}" \
    -lc "
      set -euo pipefail
      python3 scripts/benchmark/evaluate_swebenchpro.py prepare \
        --output-dir '${DATA_DIR}' \
        ${LIMIT_ARG[*]}
    "
fi

if [ "${RUN_GENERATE}" = "1" ]; then
  podman run --rm \
    --entrypoint bash \
    --network host \
    --device nvidia.com/gpu=all \
    --pids-limit=-1 \
    --shm-size=64g \
    -v /data/ronghao:/data/ronghao \
    -w "${REPO_DIR}" \
    -e MODELSCOPE_CACHE=/data/ronghao/models/modelscope \
    "${GEN_IMAGE}" \
    -lc "
      set -euo pipefail
      python3 scripts/benchmark/evaluate_swebenchpro.py generate \
        --data '${DATA_FILE}' \
        --model '${MODEL_DIR}' \
        --output-dir '${OUTPUT_DIR}' \
        --prefix '${PREFIX}' \
        --tensor-parallel-size '${TENSOR_PARALLEL_SIZE}' \
        --max-model-len '${MAX_MODEL_LEN}' \
        --max-tokens '${MAX_TOKENS}' \
        --gpu-memory-utilization '${GPU_MEMORY_UTILIZATION}' \
        --temperature '${TEMPERATURE}' \
        --top-p '${TOP_P}' \
        ${DISABLE_THINKING_ARG[*]} \
        ${LIMIT_ARG[*]}
    "
fi

if [ "${RUN_DOWNLOAD_OFFICIAL_ASSETS}" = "1" ]; then
  podman run --rm \
    --entrypoint bash \
    --network host \
    --pids-limit=-1 \
    --shm-size=16g \
    -v /data/ronghao:/data/ronghao \
    -w "${REPO_DIR}" \
    "${EVAL_IMAGE}" \
    -lc "
      set -euo pipefail
      python3 scripts/benchmark/evaluate_swebenchpro.py download-official-assets \
        --data '${DATA_FILE}' \
        --official-eval-dir '${OFFICIAL_EVAL_DIR}' \
        --base-url '${OFFICIAL_ASSET_BASE_URL}' \
        --timeout-seconds '${OFFICIAL_ASSET_TIMEOUT_SECONDS}' \
        --retries '${OFFICIAL_ASSET_RETRIES}' \
        --workers '${OFFICIAL_ASSET_WORKERS}' \
        ${LIMIT_ARG[*]}
    "
fi

if [ "${RUN_OFFICIAL_EVALUATE}" = "1" ]; then
  if [ ! -x "${OFFICIAL_VENV}/bin/python" ]; then
    python3 -m venv "${OFFICIAL_VENV}"
    INSTALL_OFFICIAL_DEPS=1
  fi
  if [ "${INSTALL_OFFICIAL_DEPS}" = "1" ] || ! "${OFFICIAL_VENV}/bin/python" -c "import docker, pandas, tqdm" >/dev/null 2>&1; then
    "${OFFICIAL_VENV}/bin/python" -m pip install -q -i "${PIP_INDEX_URL}" docker pandas tqdm
  fi

  REDO_ARG=()
  if [ "${OFFICIAL_REDO}" = "1" ]; then
    REDO_ARG=(--redo)
  fi

  (
    cd "${OFFICIAL_EVAL_DIR}"
    "${OFFICIAL_VENV}/bin/python" swe_bench_pro_eval.py \
      --raw_sample_path "${DATA_CSV}" \
      --patch_path "${OUTPUT_DIR}/patches.json" \
      --output_dir "${OUTPUT_DIR}/official_eval" \
      --scripts_dir "${OFFICIAL_EVAL_DIR}/run_scripts" \
      --dockerhub_username "${DOCKERHUB_USERNAME}" \
      --num_workers "${OFFICIAL_NUM_WORKERS}" \
      --use_local_docker \
      "${REDO_ARG[@]}"
  )
fi

if [ "${RUN_SUMMARIZE}" = "1" ]; then
  OFFICIAL_RESULTS="${OUTPUT_DIR}/official_eval/eval_results.json"
  OFFICIAL_RESULTS_ARG=()
  if [ -f "${OFFICIAL_RESULTS}" ]; then
    OFFICIAL_RESULTS_ARG=(--official-results "${OFFICIAL_RESULTS}")
  fi
  podman run --rm \
    --entrypoint bash \
    --network host \
    --pids-limit=-1 \
    --shm-size=16g \
    -v /data/ronghao:/data/ronghao \
    -w "${REPO_DIR}" \
    "${EVAL_IMAGE}" \
    -lc "
      set -euo pipefail
      python3 scripts/benchmark/evaluate_swebenchpro.py summarize \
        --data '${DATA_FILE}' \
        --output-dir '${OUTPUT_DIR}' \
        ${OFFICIAL_RESULTS_ARG[*]}
    "
fi
