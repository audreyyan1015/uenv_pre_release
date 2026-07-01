#!/usr/bin/env bash
# 208.77：同步 UEnv 集成代码并确保 OpenHands runner / SSH 隧道就绪
# 开发机：UENV_SSH_KEY=secrets/... bash scripts/deploy-openhands-20877.sh
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OH_HOST="${OPENHANDS_HOST:-8.130.208.77}"
JUMP_HOST="${UENV_JUMP_HOST:-219.147.100.43}"
JUMP_PORT="${UENV_JUMP_PORT:-7142}"
REMOTE_UENV="${UENV_REMOTE_UENV:-/root/UEnv}"

resolve_key() {
  if [[ -n "${UENV_SSH_KEY:-}" && -f "${UENV_SSH_KEY}" ]]; then echo "${UENV_SSH_KEY}"; return; fi
  for k in "$REPO_ROOT/secrets/2a9f778a35e7d08c738c79493ba643ef_65c3b455afbe3c81a8a757c01b0faae8_8.142" \
           "$HOME/Documents/142key"; do
    [[ -f "$k" ]] && { echo "$k"; return; }
  done
  echo "ERROR: set UENV_SSH_KEY to 7142 jump key" >&2; exit 1
}
KEY="$(resolve_key)"
chmod 600 "$KEY" 2>/dev/null || true

SSH_JUMP=(ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new -i "$KEY" -p "$JUMP_PORT" root@"$JUMP_HOST")
SSH_OH=(ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new -o ProxyCommand="ssh -i $KEY -p $JUMP_PORT -W %h:22 root@$JUMP_HOST" root@"$OH_HOST")

echo "== tar sync integrations + config + scripts -> 208.77:$REMOTE_UENV =="
tar -C "$REPO_ROOT" -czf /tmp/uenv-oh-sync.tgz \
  integrations/openhands \
  config/openhands-20877.env.example \
  config/openhands-llm-20877.json.example \
  config/uenv-trajectory.env.example \
  config/swe/pro-python-smoke.json \
  scripts/run-openhands-pro-20877.sh \
  scripts/verify-openhands-trajectory-e2e-20877.sh \
  scripts/openhands \
  scripts/gen-openhands-llm-config.py

"${SSH_OH[@]}" "mkdir -p $REMOTE_UENV"
cat /tmp/uenv-oh-sync.tgz | "${SSH_OH[@]}" "tar -xzf - -C $REMOTE_UENV"

"${SSH_OH[@]}" bash -s <<'REMOTE'
set -euo pipefail
cd /root/UEnv
chmod +x scripts/run-openhands-pro-20877.sh scripts/verify-openhands-trajectory-e2e-20877.sh scripts/openhands/*.py 2>/dev/null || true
if [[ ! -f /root/.openhands-20877.env ]]; then
  cp config/openhands-20877.env.example /root/.openhands-20877.env
  chmod 600 /root/.openhands-20877.env
fi
if [[ ! -f /root/.uenv-trajectory.env ]]; then
  cp config/uenv-trajectory.env.example /root/.uenv-trajectory.env
  chmod 600 /root/.uenv-trajectory.env
fi
systemctl is-active uenv-gateway-tunnel.service >/dev/null 2>&1 && systemctl restart uenv-gateway-tunnel.service || true
systemctl is-active openhands-runner.service >/dev/null 2>&1 && systemctl restart openhands-runner.service || true
curl -sf http://127.0.0.1:8777/health && echo " runner_ok" || echo " runner_not_ready"
curl -sf -H 'X-API-Key: swe-pro-secret' http://127.0.0.1:28097/health && echo " tunnel_gateway_ok" || echo " tunnel_gateway_fail"
REMOTE

echo "208.77 sync done."
