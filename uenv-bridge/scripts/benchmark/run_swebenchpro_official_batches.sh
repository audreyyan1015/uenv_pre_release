#!/usr/bin/env bash
set -euo pipefail

REPO_DIR=${REPO_DIR:-"$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"}
DATA_DIR=${DATA_DIR:-${REPO_DIR}/data/benchmarks/swebenchpro}
DATA_CSV=${DATA_CSV:-${DATA_DIR}/swe_bench_pro_full.csv}
OUTPUT_DIR=${OUTPUT_DIR:-${REPO_DIR}/temp/benchmarks/swebenchpro/qwen3_6_35b_a3b_full}
PATCHES=${PATCHES:-${OUTPUT_DIR}/patches.json}
BATCH_ROOT=${BATCH_ROOT:-${OUTPUT_DIR}/official_eval_batches}
OFFICIAL_EVAL_DIR=${OFFICIAL_EVAL_DIR:-/data/ronghao/third_party/SWE-bench_Pro-os}
OFFICIAL_VENV=${OFFICIAL_VENV:-${REPO_DIR}/temp/venvs/swebenchpro_eval}
PIP_INDEX_URL=${PIP_INDEX_URL:-https://pypi.tuna.tsinghua.edu.cn/simple}

DOCKERHUB_USERNAME=${DOCKERHUB_USERNAME:-docker.1panel.live/jefzda}
BATCH_SIZE=${BATCH_SIZE:-10}
OFFICIAL_NUM_WORKERS=${OFFICIAL_NUM_WORKERS:-1}
BATCH_TIMEOUT_SECONDS=${BATCH_TIMEOUT_SECONDS:-0}
LIMIT=${LIMIT-}
MAX_BATCHES=${MAX_BATCHES-}
START_BATCH=${START_BATCH:-0}
INSTANCE_ID_FILE=${INSTANCE_ID_FILE-}
EXTRA_MERGE_ROOTS=${EXTRA_MERGE_ROOTS-}
REDO_BATCHES=${REDO_BATCHES:-0}
SKIP_COMPLETED_INSTANCES=${SKIP_COMPLETED_INSTANCES:-1}
CLEAN_IMAGES_AFTER_BATCH=${CLEAN_IMAGES_AFTER_BATCH:-1}
INSTALL_OFFICIAL_DEPS=${INSTALL_OFFICIAL_DEPS:-0}

if [ ! -x "${OFFICIAL_VENV}/bin/python" ]; then
  python3 -m venv "${OFFICIAL_VENV}"
  INSTALL_OFFICIAL_DEPS=1
fi

if [ "${INSTALL_OFFICIAL_DEPS}" = "1" ] || ! "${OFFICIAL_VENV}/bin/python" -c "import docker, pandas, tqdm" >/dev/null 2>&1; then
  "${OFFICIAL_VENV}/bin/python" -m pip install -q -i "${PIP_INDEX_URL}" docker pandas tqdm
fi

ARGS=(
  --raw-sample-csv "${DATA_CSV}"
  --patches "${PATCHES}"
  --output-dir "${OUTPUT_DIR}"
  --batch-root "${BATCH_ROOT}"
  --official-eval-dir "${OFFICIAL_EVAL_DIR}"
  --python "${OFFICIAL_VENV}/bin/python"
  --dockerhub-username "${DOCKERHUB_USERNAME}"
  --batch-size "${BATCH_SIZE}"
  --official-num-workers "${OFFICIAL_NUM_WORKERS}"
  --batch-timeout-seconds "${BATCH_TIMEOUT_SECONDS}"
  --start-batch "${START_BATCH}"
)

if [ -n "${LIMIT}" ]; then
  ARGS+=(--limit "${LIMIT}")
fi

if [ -n "${MAX_BATCHES}" ]; then
  ARGS+=(--max-batches "${MAX_BATCHES}")
fi

if [ -n "${INSTANCE_ID_FILE}" ]; then
  ARGS+=(--instance-id-file "${INSTANCE_ID_FILE}")
fi

if [ -n "${EXTRA_MERGE_ROOTS}" ]; then
  IFS=':' read -r -a EXTRA_ROOT_ARRAY <<< "${EXTRA_MERGE_ROOTS}"
  for root in "${EXTRA_ROOT_ARRAY[@]}"; do
    if [ -n "${root}" ]; then
      ARGS+=(--extra-merge-root "${root}")
    fi
  done
fi

if [ "${REDO_BATCHES}" = "1" ]; then
  ARGS+=(--redo)
fi

if [ "${SKIP_COMPLETED_INSTANCES}" != "1" ]; then
  ARGS+=(--no-skip-completed-instances)
fi

if [ "${CLEAN_IMAGES_AFTER_BATCH}" != "1" ]; then
  ARGS+=(--no-clean-images-after-batch)
fi

cd "${REPO_DIR}"
exec "${OFFICIAL_VENV}/bin/python" scripts/benchmark/run_swebenchpro_official_batches.py "${ARGS[@]}"
