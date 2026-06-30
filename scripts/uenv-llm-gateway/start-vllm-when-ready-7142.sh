#!/usr/bin/env bash
# 7142：模型下载完成后启动 vLLM 并等待网关就绪
set -euo pipefail

MODEL_DIR="${UENV_MODEL_DIR:-/data/models/DeepSeek-V3-0324-AWQ}"
VLLM_VENV="${UENV_VLLM_VENV:-/opt/vllm-dsv3-awq}"
HF_HOME="${UENV_HF_HOME:-/data/huggingface}"
MIN_SIZE_GB="${UENV_MODEL_MIN_GB:-300}"

size_gb=$(du -sb "$MODEL_DIR" 2>/dev/null | awk '{printf "%.0f", $1/1024/1024/1024}')
if [[ ! -d "$MODEL_DIR" ]] || [[ "$size_gb" -lt "$MIN_SIZE_GB" ]]; then
  echo "model not ready: ${size_gb}GB / need ~${MIN_SIZE_GB}GB at $MODEL_DIR"
  exit 1
fi

CHAT_TEMPLATE="$("$VLLM_VENV/bin/python" -c "import vllm, pathlib; print(pathlib.Path(vllm.__file__).parent / 'examples/tool_chat_template_deepseekv3.jinja')")"

cat > /etc/systemd/system/vllm-dsv3-awq.service <<UNIT
[Unit]
Description=vLLM DeepSeek-V3-0324-AWQ (TP8, 8xA100)
After=network.target

[Service]
Type=simple
User=root
Environment=CUDA_VISIBLE_DEVICES=0,1,2,3,4,5,6,7
Environment=HF_HOME=$HF_HOME
Environment=VLLM_USE_V1=0
Environment=VLLM_WORKER_MULTIPROC_METHOD=spawn
Environment=VLLM_MARLIN_USE_ATOMIC_ADD=1
ExecStart=$VLLM_VENV/bin/vllm serve $MODEL_DIR \\
  --served-model-name deepseek-v3-0324-awq \\
  --host 127.0.0.1 --port 8000 --trust-remote-code \\
  --tensor-parallel-size 8 --gpu-memory-utilization 0.90 \\
  --max-model-len 32768 --max-num-seqs 4 \\
  --enable-chunked-prefill --enable-prefix-caching \\
  --enable-auto-tool-choice --tool-call-parser deepseek_v3 \\
  --chat-template $CHAT_TEMPLATE
Restart=on-failure
RestartSec=30

[Install]
WantedBy=multi-user.target
UNIT

systemctl daemon-reload
systemctl enable vllm-dsv3-awq
systemctl restart vllm-dsv3-awq

echo "vLLM started; waiting for /v1/models (up to 20min)..."
for i in $(seq 1 240); do
  if curl -sf http://127.0.0.1:8000/v1/models >/dev/null; then
    echo "vLLM ready after ${i}x5s"
    curl -s http://127.0.0.1:18777/health
    echo
    exit 0
  fi
  sleep 5
done
echo "timeout waiting for vLLM"
journalctl -u vllm-dsv3-awq -n 30 --no-pager
exit 1
