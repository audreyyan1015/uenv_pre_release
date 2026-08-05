#!/usr/bin/env bash
set -euo pipefail

IMAGE=${IMAGE:-localhost/vllm-openai:v0.19.0-cu130}
MODEL_ID=${MODEL_ID:-Qwen/Qwen3.6-35B-A3B}
MODEL_DIR=${MODEL_DIR:-/data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B}
DATA_PATH=${DATA_PATH:-/data/ronghao/uenv/uenv-bridge/data/benchmarks/scitab/sci_tab.json}
OUTPUT_DIR=${OUTPUT_DIR:-/data/ronghao/uenv/uenv-bridge/temp/benchmarks/scitab/qwen3_6_35b_a3b}
TENSOR_PARALLEL_SIZE=${TENSOR_PARALLEL_SIZE:-8}
MAX_MODEL_LEN=${MAX_MODEL_LEN:-4096}
GPU_MEMORY_UTILIZATION=${GPU_MEMORY_UTILIZATION:-0.8}
ENFORCE_EAGER=${ENFORCE_EAGER:-0}
LIMIT=${LIMIT:-}
PYTHON_BIN=${PYTHON_BIN:-python3}
BACKEND=${BACKEND:-vllm}
INFERENCE_MODE=${INFERENCE_MODE:-label_logprob}
PROMPT_STYLE=${PROMPT_STYLE:-default}
MAX_TOKENS=${MAX_TOKENS:-512}
VLLM_LABEL_BATCH_SIZE=${VLLM_LABEL_BATCH_SIZE:-32}
LABEL_SCORE_NORMALIZATION=${LABEL_SCORE_NORMALIZATION:-mean}
PODMAN_EXTRA_ARGS=${PODMAN_EXTRA_ARGS:-}
ENFORCE_EAGER_ARG=
if [ "$ENFORCE_EAGER" = "1" ]; then
  ENFORCE_EAGER_ARG=--enforce-eager
fi

mkdir -p "$(dirname "$DATA_PATH")" "$OUTPUT_DIR"

if [ ! -f "$DATA_PATH" ]; then
  python3 - <<'PY'
from pathlib import Path
import urllib.request

out = Path("/data/ronghao/uenv/uenv-bridge/data/benchmarks/scitab/sci_tab.json")
url = "https://cdn.jsdelivr.net/gh/XinyuanLu00/SciTab@main/dataset/sci_tab.json"
out.parent.mkdir(parents=True, exist_ok=True)
req = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0"})
with urllib.request.urlopen(req, timeout=120) as response:
    out.write_bytes(response.read())
print(out)
PY
fi

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
    if [ ! -f '$MODEL_DIR/model.safetensors.index.json' ]; then
      python3 - <<'PY'
from modelscope import snapshot_download
snapshot_download('$MODEL_ID', cache_dir='/data/ronghao/models/modelscope', max_workers=8)
PY
    fi
    '$PYTHON_BIN' scripts/benchmark/evaluate_scitab.py \
      --data '$DATA_PATH' \
      --model '$MODEL_DIR' \
      --output-dir '$OUTPUT_DIR' \
      --backend '$BACKEND' \
      --inference-mode '$INFERENCE_MODE' \
      --tensor-parallel-size '$TENSOR_PARALLEL_SIZE' \
      --max-model-len '$MAX_MODEL_LEN' \
      --gpu-memory-utilization '$GPU_MEMORY_UTILIZATION' \
      --max-tokens '$MAX_TOKENS' \
      --prompt-style '$PROMPT_STYLE' \
      --vllm-label-batch-size '$VLLM_LABEL_BATCH_SIZE' \
      --label-score-normalization '$LABEL_SCORE_NORMALIZATION' \
      $ENFORCE_EAGER_ARG \
      ${LIMIT:+--limit '$LIMIT'}
  "
