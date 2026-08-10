#!/usr/bin/env bash
set -euo pipefail

REPO_DIR=${REPO_DIR:-"$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"}
GEN_IMAGE=${GEN_IMAGE:-localhost/vllm-openai:v0.19.0-cu130}
EVAL_IMAGE=${EVAL_IMAGE:-localhost/uenv-bridge-verl:layer4-build}
MODEL_ID=${MODEL_ID:-Qwen/Qwen3.6-35B-A3B}
MODEL_DIR=${MODEL_DIR:-/data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B}
DATA_FILE=${DATA_FILE:-${REPO_DIR}/data/benchmarks/dscodebench/DSCodeBench.json}
OFFICIAL_EVAL_DIR=${OFFICIAL_EVAL_DIR:-/data/ronghao/third_party/DSCodeBench/benchmark_construction_evaluation}
OUTPUT_DIR=${OUTPUT_DIR:-${REPO_DIR}/temp/benchmarks/dscodebench/qwen3_6_35b_a3b_limit100_numpy}
LIMIT=${LIMIT-100}
LIBRARY=${LIBRARY:-}
MAX_PER_LIBRARY=${MAX_PER_LIBRARY:-}
TENSOR_PARALLEL_SIZE=${TENSOR_PARALLEL_SIZE:-8}
MAX_MODEL_LEN=${MAX_MODEL_LEN:-8192}
MAX_TOKENS=${MAX_TOKENS:-2048}
GPU_MEMORY_UTILIZATION=${GPU_MEMORY_UTILIZATION:-0.9}
TEMPERATURE=${TEMPERATURE:-0.2}
TOP_P=${TOP_P:-1.0}
PROMPT_STYLE=${PROMPT_STYLE:-official_fenced}
DISABLE_THINKING=${DISABLE_THINKING:-1}
TEST_CASE_NUMBER=${TEST_CASE_NUMBER:-20}
PER_PROBLEM_TIMEOUT=${PER_PROBLEM_TIMEOUT:-120}
RUN_GENERATE=${RUN_GENERATE:-1}
RUN_EVALUATE=${RUN_EVALUATE:-1}
INSTALL_EVAL_DEPS=${INSTALL_EVAL_DEPS:-0}
PIP_INDEX_URL=${PIP_INDEX_URL:-https://pypi.tuna.tsinghua.edu.cn/simple}

mkdir -p "${OUTPUT_DIR}"

LIMIT_ARG=()
if [ -n "${LIMIT}" ]; then
  LIMIT_ARG=(--limit "${LIMIT}")
fi

LIBRARY_ARG=()
if [ -n "${LIBRARY}" ]; then
  LIBRARY_ARG=(--library "${LIBRARY}")
fi

MAX_PER_LIBRARY_ARG=()
if [ -n "${MAX_PER_LIBRARY}" ]; then
  MAX_PER_LIBRARY_ARG=(--max-per-library "${MAX_PER_LIBRARY}")
fi

DISABLE_THINKING_ARG=()
if [ "${DISABLE_THINKING}" = "1" ]; then
  DISABLE_THINKING_ARG=(--disable-thinking)
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
      python3 scripts/benchmark/evaluate_dscodebench.py generate \
        --data '${DATA_FILE}' \
        --model '${MODEL_DIR}' \
        --output-dir '${OUTPUT_DIR}' \
        --tensor-parallel-size '${TENSOR_PARALLEL_SIZE}' \
        --max-model-len '${MAX_MODEL_LEN}' \
        --max-tokens '${MAX_TOKENS}' \
        --gpu-memory-utilization '${GPU_MEMORY_UTILIZATION}' \
        --temperature '${TEMPERATURE}' \
        --top-p '${TOP_P}' \
        --prompt-style '${PROMPT_STYLE}' \
        ${DISABLE_THINKING_ARG[*]} \
        ${LIMIT_ARG[*]} \
        ${LIBRARY_ARG[*]} \
        ${MAX_PER_LIBRARY_ARG[*]}
    "
fi

if [ "${RUN_EVALUATE}" = "1" ]; then
  podman run --rm \
    --entrypoint bash \
    --network host \
    --pids-limit=-1 \
    --shm-size=32g \
    -v /data/ronghao:/data/ronghao \
    -w "${REPO_DIR}" \
    "${EVAL_IMAGE}" \
    -lc "
      set -euo pipefail
      if [ '${INSTALL_EVAL_DEPS}' = '1' ]; then
        python3 -m pip install -q -i '${PIP_INDEX_URL}' \
          aiofiles scikit-learn seaborn scikit-image lightgbm tensorflow keras
      fi
      python3 scripts/benchmark/evaluate_dscodebench.py evaluate \
        --data '${DATA_FILE}' \
        --generations '${OUTPUT_DIR}/generations.json' \
        --output-dir '${OUTPUT_DIR}' \
        --official-eval-dir '${OFFICIAL_EVAL_DIR}' \
        --test-case-number '${TEST_CASE_NUMBER}' \
        --per-problem-timeout '${PER_PROBLEM_TIMEOUT}' \
        ${LIMIT_ARG[*]} \
        ${LIBRARY_ARG[*]} \
        ${MAX_PER_LIBRARY_ARG[*]}
    "
fi
