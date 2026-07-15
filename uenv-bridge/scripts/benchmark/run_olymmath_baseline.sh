#!/usr/bin/env bash
set -euo pipefail

IMAGE=${IMAGE:-localhost/vllm-openai:v0.19.0-cu130}
MODEL_ID=${MODEL_ID:-Qwen/Qwen3.6-35B-A3B}
MODEL_DIR=${MODEL_DIR:-/data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B}
DATA_DIR=${DATA_DIR:-/data/ronghao/uenv/uenv-bridge/data/benchmarks/olymmath}
DATASETS=${DATASETS:-EN-EASY,EN-HARD}
OUTPUT_DIR=${OUTPUT_DIR:-/data/ronghao/uenv/uenv-bridge/temp/benchmarks/olymmath/qwen3_6_35b_a3b_en_easy_hard}
TENSOR_PARALLEL_SIZE=${TENSOR_PARALLEL_SIZE:-8}
MAX_MODEL_LEN=${MAX_MODEL_LEN:-16384}
MAX_TOKENS=${MAX_TOKENS:-8192}
GPU_MEMORY_UTILIZATION=${GPU_MEMORY_UTILIZATION:-0.9}
TEMPERATURE=${TEMPERATURE:-0.0}
TOP_P=${TOP_P:-1.0}
MIN_P=${MIN_P:-}
SAMPLE=${SAMPLE:-1}
PROMPT_STYLE=${PROMPT_STYLE:-official}
ENFORCE_EAGER=${ENFORCE_EAGER:-0}
LIMIT=${LIMIT:-}
PYTHON_BIN=${PYTHON_BIN:-python3}
INSTALL_MATH_VERIFY=${INSTALL_MATH_VERIFY:-1}
PIP_INDEX_URL=${PIP_INDEX_URL:-https://pypi.tuna.tsinghua.edu.cn/simple}
PODMAN_EXTRA_ARGS=${PODMAN_EXTRA_ARGS:-}
export DATA_DIR

ENFORCE_EAGER_ARG=
if [ "$ENFORCE_EAGER" = "1" ]; then
  ENFORCE_EAGER_ARG=--enforce-eager
fi
MIN_P_ARG=
if [ -n "$MIN_P" ]; then
  MIN_P_ARG="--min-p '$MIN_P'"
fi

mkdir -p "$DATA_DIR" "$OUTPUT_DIR"

python3 - <<'PY'
from pathlib import Path
import os
import urllib.request

data_dir = Path(os.environ.get("DATA_DIR", "/data/ronghao/uenv/uenv-bridge/data/benchmarks/olymmath"))
data_dir.mkdir(parents=True, exist_ok=True)
base = "https://cdn.jsdelivr.net/gh/RUCAIBox/OlymMATH@main/data"
files = [
    "OlymMATH-EN-EASY.jsonl",
    "OlymMATH-EN-HARD.jsonl",
    "OlymMATH-ZH-EASY.jsonl",
    "OlymMATH-ZH-HARD.jsonl",
]
for name in files:
    out = data_dir / name
    if out.exists():
        continue
    req = urllib.request.Request(f"{base}/{name}", headers={"User-Agent": "Mozilla/5.0"})
    with urllib.request.urlopen(req, timeout=120) as response:
        out.write_bytes(response.read())
    print(out)
PY

DATA_ARGS=()
IFS=',' read -ra DATASET_ITEMS <<< "$DATASETS"
for dataset in "${DATASET_ITEMS[@]}"; do
  dataset=$(echo "$dataset" | xargs)
  case "$dataset" in
    EN-EASY|EN-HARD|ZH-EASY|ZH-HARD)
      DATA_ARGS+=("$DATA_DIR/OlymMATH-$dataset.jsonl")
      ;;
    *)
      DATA_ARGS+=("$dataset")
      ;;
  esac
done

podman run --rm \
  --entrypoint bash \
  --network host \
  --device nvidia.com/gpu=all \
  --pids-limit=-1 \
  --shm-size=64g \
  -v /data/ronghao:/data/ronghao \
  -w /data/ronghao/uenv/uenv-bridge \
  -e MODELSCOPE_CACHE=/data/ronghao/models/modelscope \
  ${PODMAN_EXTRA_ARGS} \
  "$IMAGE" \
  -lc "
    set -euo pipefail
    if [ '$INSTALL_MATH_VERIFY' = '1' ]; then
      '$PYTHON_BIN' -m pip install -q math-verify -i '$PIP_INDEX_URL'
    fi
    if [ ! -f '$MODEL_DIR/model.safetensors.index.json' ]; then
      '$PYTHON_BIN' - <<'PY'
from modelscope import snapshot_download
snapshot_download('$MODEL_ID', cache_dir='/data/ronghao/models/modelscope', max_workers=8)
PY
    fi
    '$PYTHON_BIN' scripts/benchmark/evaluate_olymmath.py \
      --data ${DATA_ARGS[*]} \
      --model '$MODEL_DIR' \
      --output-dir '$OUTPUT_DIR' \
      --tensor-parallel-size '$TENSOR_PARALLEL_SIZE' \
      --max-model-len '$MAX_MODEL_LEN' \
      --max-tokens '$MAX_TOKENS' \
      --sample '$SAMPLE' \
      --prompt-style '$PROMPT_STYLE' \
      --gpu-memory-utilization '$GPU_MEMORY_UTILIZATION' \
      --temperature '$TEMPERATURE' \
      --top-p '$TOP_P' \
      $MIN_P_ARG \
      $ENFORCE_EAGER_ARG \
      ${LIMIT:+--limit '$LIMIT'}
  "
