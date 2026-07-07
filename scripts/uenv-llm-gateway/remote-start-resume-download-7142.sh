#!/usr/bin/env bash
# 开发机：同步 resume-download 脚本到 7142 并启动断点续传
#
#   bash scripts/uenv-llm-gateway/remote-start-resume-download-7142.sh
#   bash scripts/uenv-llm-gateway/remote-start-resume-download-7142.sh status
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

ADAPTER_HOST="${UENV_ADAPTER_HOST:-219.147.100.43}"
ADAPTER_PORT="${UENV_ADAPTER_SSH_PORT:-7142}"
REMOTE_PKG="${UENV_LLM_PKG_DIR:-/root/UEnv/scripts/uenv-llm-gateway}"

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

cmd="${1:-start}"

echo "== rsync uenv-llm-gateway -> 7142:$REMOTE_PKG =="
rsync -az -e "$RSYNC_SSH" "$SCRIPT_DIR/" root@"$ADAPTER_HOST":"$REMOTE_PKG/"

"${SSH7142[@]}" bash -s <<REMOTE
set -euo pipefail
chmod +x "$REMOTE_PKG/resume-download-7142.sh"
"$REMOTE_PKG/resume-download-7142.sh" ${cmd}
REMOTE

echo "log: ssh -p $ADAPTER_PORT -i <key> root@$ADAPTER_HOST 'tail -f /var/log/uenv/model-download.log'"
echo "monitor (dev): python scripts/uenv-llm-gateway/monitor-download-7142.py"
