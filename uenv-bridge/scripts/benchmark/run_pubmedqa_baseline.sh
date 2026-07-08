#!/usr/bin/env bash
set -euo pipefail

IMAGE=${IMAGE:-localhost/uenv-bridge-verl:layer4-build}
MODEL_ID=${MODEL_ID:-Qwen/Qwen3.6-35B-A3B}
MODEL_DIR=${MODEL_DIR:-/data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B}
DATA_PATH=${DATA_PATH:-/data/ronghao/uenv/uenv-bridge/data/benchmarks/pubmedqa/ori_pqal.json}
OUTPUT_DIR=${OUTPUT_DIR:-/data/ronghao/uenv/uenv-bridge/temp/benchmarks/pubmedqa/qwen3_6_35b_a3b}
TENSOR_PARALLEL_SIZE=${TENSOR_PARALLEL_SIZE:-8}
MAX_MODEL_LEN=${MAX_MODEL_LEN:-4096}
LIMIT=${LIMIT:-}
PYTHON_BIN=${PYTHON_BIN:-python3}
BACKEND=${BACKEND:-vllm}
INFERENCE_MODE=${INFERENCE_MODE:-generate}
PROMPT_STYLE=${PROMPT_STYLE:-default}
MAX_TOKENS=${MAX_TOKENS:-256}
TRANSFORMERS_DEVICE_MAP=${TRANSFORMERS_DEVICE_MAP:-auto}
TRANSFORMERS_BATCH_SIZE=${TRANSFORMERS_BATCH_SIZE:-1}
VLLM_LABEL_BATCH_SIZE=${VLLM_LABEL_BATCH_SIZE:-64}
LABEL_SCORE_NORMALIZATION=${LABEL_SCORE_NORMALIZATION:-mean}
PODMAN_EXTRA_ARGS=${PODMAN_EXTRA_ARGS:-}

mkdir -p "$(dirname "$DATA_PATH")" "$OUTPUT_DIR"

if [ ! -f "$DATA_PATH" ]; then
  python3 - <<'PY'
from pathlib import Path
import urllib.request

out = Path("/data/ronghao/uenv/uenv-bridge/data/benchmarks/pubmedqa/ori_pqal.json")
url = "https://cdn.jsdelivr.net/gh/pubmedqa/pubmedqa@master/data/ori_pqal.json"
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
    '$PYTHON_BIN' scripts/benchmark/evaluate_pubmedqa.py \
      --data '$DATA_PATH' \
      --model '$MODEL_DIR' \
      --output-dir '$OUTPUT_DIR' \
      --backend '$BACKEND' \
      --inference-mode '$INFERENCE_MODE' \
      --tensor-parallel-size '$TENSOR_PARALLEL_SIZE' \
      --max-model-len '$MAX_MODEL_LEN' \
      --max-tokens '$MAX_TOKENS' \
      --prompt-style '$PROMPT_STYLE' \
      --transformers-device-map '$TRANSFORMERS_DEVICE_MAP' \
      --transformers-batch-size '$TRANSFORMERS_BATCH_SIZE' \
      --vllm-label-batch-size '$VLLM_LABEL_BATCH_SIZE' \
      --label-score-normalization '$LABEL_SCORE_NORMALIZATION' \
      ${LIMIT:+--limit '$LIMIT'}
  "
