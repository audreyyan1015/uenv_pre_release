#!/usr/bin/env bash
# 停止 7142 LLM 栈，释放 GPU
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

ADAPTER_HOST="${UENV_ADAPTER_HOST:-219.147.100.43}"
ADAPTER_PORT="${UENV_ADAPTER_SSH_PORT:-7142}"

resolve_key() {
  if [[ -n "${UENV_SSH_KEY:-}" && -f "${UENV_SSH_KEY}" ]]; then echo "${UENV_SSH_KEY}"; return; fi
  for k in "$REPO_ROOT/secrets/2a9f778a35e7d08c738c79493ba643ef_65c3b455afbe3c81a8a757c01b0faae8_8.142"; do
    [[ -f "$k" ]] && { echo "$k"; return; }
  done
  echo "ERROR: set UENV_SSH_KEY" >&2; exit 1
}

KEY="$(resolve_key)"
SSH7142=(ssh -o BatchMode=yes -o ConnectTimeout=15 -i "$KEY" -p "$ADAPTER_PORT" root@"$ADAPTER_HOST")

if [[ "${1:-}" == "--local" ]]; then
  systemctl stop uenv-llm-gateway 2>/dev/null || true
  systemctl stop vllm-dsv3-awq 2>/dev/null || true
  echo "stopped local uenv-llm stack"
  exit 0
fi

"${SSH7142[@]}" bash -s <<'REMOTE'
set -euo pipefail
systemctl stop uenv-llm-gateway 2>/dev/null || true
systemctl stop vllm-dsv3-awq 2>/dev/null || true
sleep 2
nvidia-smi --query-gpu=index,memory.used --format=csv,noheader || true
echo "7142 LLM stack stopped"
REMOTE
