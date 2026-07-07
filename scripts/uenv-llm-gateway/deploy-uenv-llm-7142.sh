#!/usr/bin/env bash
# 7142 DeepSeek-V3-0324-AWQ + uenv-llm-gateway 一键部署
#
# 用法（开发机）:
#   bash scripts/uenv-llm-gateway/deploy-uenv-llm-7142.sh
#   bash scripts/uenv-llm-gateway/deploy-uenv-llm-7142.sh smoke
#   bash scripts/uenv-llm-gateway/deploy-uenv-llm-7142.sh gateway-only   # 仅网关（vLLM 已运行）
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PKG_DIR="$SCRIPT_DIR"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

ADAPTER_HOST="${UENV_ADAPTER_HOST:-219.147.100.43}"
ADAPTER_PORT="${UENV_ADAPTER_SSH_PORT:-7142}"
REMOTE_UENV="${UENV_REMOTE_DIR:-/root/UEnv}"
REMOTE_PKG="${UENV_LLM_PKG_DIR:-$REMOTE_UENV/scripts/uenv-llm-gateway}"

VLLM_VENV="${UENV_VLLM_VENV:-/opt/vllm-dsv3-awq}"
GATEWAY_VENV="${UENV_GATEWAY_VENV:-/opt/uenv-llm-gateway}"
MODEL_DIR="${UENV_MODEL_DIR:-/data/models/DeepSeek-V3-0324-AWQ}"
HF_HOME="${UENV_HF_HOME:-/data/huggingface}"
GATEWAY_ENV="${UENV_GATEWAY_ENV_FILE:-/root/.uenv-llm-gateway.env}"
GATEWAY_CONFIG="${UENV_GATEWAY_CONFIG:-$REMOTE_UENV/config/uenv-llm-gateway-7142.yaml}"

resolve_key() {
  if [[ -n "${UENV_SSH_KEY:-}" && -f "${UENV_SSH_KEY}" ]]; then echo "${UENV_SSH_KEY}"; return; fi
  for k in "$REPO_ROOT/secrets/2a9f778a35e7d08c738c79493ba643ef_65c3b455afbe3c81a8a757c01b0faae8_8.142" \
           "$HOME/Documents/142key"; do
    [[ -f "$k" ]] && { echo "$k"; return; }
  done
  echo "ERROR: set UENV_SSH_KEY to 7142 private key" >&2
  exit 1
}

KEY="$(resolve_key)"
chmod 600 "$KEY" 2>/dev/null || true
SSH7142=(ssh -o BatchMode=yes -o ConnectTimeout=20 -o StrictHostKeyChecking=accept-new -i "$KEY" -p "$ADAPTER_PORT" root@"$ADAPTER_HOST")
RSYNC_SSH="ssh -i $KEY -p $ADAPTER_PORT -o BatchMode=yes -o StrictHostKeyChecking=accept-new"

cmd="${1:-deploy}"

echo "== rsync uenv-llm-gateway + config -> 7142 =="
rsync -az \
  -e "$RSYNC_SSH" \
  "$PKG_DIR/" root@"$ADAPTER_HOST":"$REMOTE_PKG/"

rsync -az \
  -e "$RSYNC_SSH" \
  "$REPO_ROOT/config/uenv-llm-gateway-7142.yaml.example" \
  root@"$ADAPTER_HOST":"$REMOTE_UENV/config/uenv-llm-gateway-7142.yaml.example"

"${SSH7142[@]}" bash -s <<REMOTE
set -euo pipefail
REMOTE_UENV="$REMOTE_UENV"
REMOTE_PKG="$REMOTE_PKG"
VLLM_VENV="$VLLM_VENV"
GATEWAY_VENV="$GATEWAY_VENV"
MODEL_DIR="$MODEL_DIR"
HF_HOME="$HF_HOME"
GATEWAY_ENV="$GATEWAY_ENV"
GATEWAY_CONFIG="$GATEWAY_CONFIG"
DEPLOY_MODE="${cmd}"

export DEBIAN_FRONTEND=noninteractive
mkdir -p /var/log/uenv "\$HF_HOME" "\$(dirname "\$MODEL_DIR")"
command -v python3 >/dev/null || (apt-get update -qq && apt-get install -y -qq python3 python3-venv python3-pip curl)

if [[ ! -f "\$GATEWAY_CONFIG" ]]; then
  cp "\$REMOTE_UENV/config/uenv-llm-gateway-7142.yaml.example" "\$GATEWAY_CONFIG"
fi

if [[ ! -f "\$GATEWAY_ENV" ]]; then
  KEY="\$(openssl rand -hex 24 2>/dev/null || head -c 32 /dev/urandom | xxd -p | tr -d '\n')"
  printf 'UENV_LLM_GATEWAY_API_KEY=%s\n' "\$KEY" > "\$GATEWAY_ENV"
  chmod 600 "\$GATEWAY_ENV"
  echo "created \$GATEWAY_ENV (save this key for clients)"
fi

# Gateway venv
if [[ ! -x "\$GATEWAY_VENV/bin/python" ]]; then
  python3 -m venv "\$GATEWAY_VENV"
fi
"\$GATEWAY_VENV/bin/pip" install -q -U pip
"\$GATEWAY_VENV/bin/pip" install -q -r "\$REMOTE_PKG/requirements.txt"

install_vllm() {
  if [[ ! -x "\$VLLM_VENV/bin/vllm" ]]; then
    python3 -m venv "\$VLLM_VENV"
    "\$VLLM_VENV/bin/pip" install -q -U pip
    "\$VLLM_VENV/bin/pip" install -q -U "vllm>=0.8.3"
    "\$VLLM_VENV/bin/pip" install -q -U "transformers>=4.48"
  fi
}

chat_template_path() {
  "\$VLLM_VENV/bin/python" -c "import vllm, pathlib; print(pathlib.Path(vllm.__file__).parent / 'examples/tool_chat_template_deepseekv3.jinja')"
}

write_systemd_units() {
  local CHAT_TEMPLATE
  CHAT_TEMPLATE="\$(chat_template_path)"

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

  cat > /etc/systemd/system/uenv-llm-gateway.service <<UNIT
[Unit]
Description=UEnv LLM Gateway (7142 :18888 -> vLLM :8000)
After=network.target vllm-dsv3-awq.service
Wants=vllm-dsv3-awq.service

[Service]
Type=simple
User=root
EnvironmentFile=-$GATEWAY_ENV
ExecStart=$GATEWAY_VENV/bin/python $REMOTE_PKG/uenv_llm_gateway.py --config $GATEWAY_CONFIG
Restart=on-failure
RestartSec=10

[Install]
WantedBy=multi-user.target
UNIT

  cat > /etc/systemd/system/uenv-llm.target <<UNIT
[Unit]
Description=UEnv 7142 Local LLM Stack (vLLM + Gateway)
Requires=vllm-dsv3-awq.service uenv-llm-gateway.service
After=vllm-dsv3-awq.service uenv-llm-gateway.service
UNIT

  systemctl daemon-reload
}

preflight() {
  echo "== preflight =="
  nvidia-smi -L || { echo "ERROR: nvidia-smi failed"; exit 1; }
  df -h /data || df -h /
  if [[ ! -d "\$MODEL_DIR" ]] || [[ -z "\$(ls -A "\$MODEL_DIR" 2>/dev/null | head -1)" ]]; then
    echo "WARN: model not found at \$MODEL_DIR"
    echo "  Download: huggingface-cli download cognitivecomputations/DeepSeek-V3-0324-AWQ --local-dir \$MODEL_DIR"
    if [[ "\$DEPLOY_MODE" != "gateway-only" && "\$DEPLOY_MODE" != "smoke" ]]; then
      exit 1
    fi
  fi
}

start_stack() {
  if [[ "\$DEPLOY_MODE" == "gateway-only" ]]; then
    systemctl enable uenv-llm-gateway
    systemctl restart uenv-llm-gateway
  else
    install_vllm
    write_systemd_units
    systemctl enable vllm-dsv3-awq uenv-llm-gateway
    systemctl restart vllm-dsv3-awq
    systemctl restart uenv-llm-gateway
  fi
}

wait_ready() {
  echo "== waiting for backend + gateway =="
  for i in \$(seq 1 180); do
    if curl -sf http://127.0.0.1:8000/v1/models >/dev/null 2>&1; then
      echo "vLLM ready (\${i}x5s)"
      break
    fi
    sleep 5
  done
  for i in \$(seq 1 60); do
    if curl -sf http://127.0.0.1:18777/health | grep -q '"status":"ok"'; then
      echo "gateway health ok"
      return 0
    fi
    sleep 5
  done
  echo "WARN: gateway not ready yet; check journalctl -u vllm-dsv3-awq -u uenv-llm-gateway"
  return 1
}

preflight
if [[ "\$DEPLOY_MODE" != "smoke" ]]; then
  start_stack
  wait_ready || true
fi

echo "== status =="
systemctl is-active vllm-dsv3-awq 2>/dev/null || echo "vllm: inactive"
systemctl is-active uenv-llm-gateway 2>/dev/null || echo "gateway: inactive"
ss -tlnp | grep -E ':8000|:18777|:18888' || true
REMOTE

if [[ "$cmd" == "smoke" || "$cmd" == "deploy" ]]; then
  echo "== remote smoke test =="
  "${SSH7142[@]}" "source $GATEWAY_ENV && bash $REMOTE_PKG/smoke-test-7142.sh"
fi

echo "done. Gateway key: ssh 7142 'cat $GATEWAY_ENV'"
