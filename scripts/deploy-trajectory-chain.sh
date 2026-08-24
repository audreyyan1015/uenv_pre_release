#!/usr/bin/env bash
# 从开发机一键：7143 Worker + 75.157 Server + 208.77 OpenHands 轨迹链部署与 gold 验收
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORKER_HOST="${UENV_WORKER_HOST:-219.147.100.43}"
WORKER_PORT="${UENV_WORKER_SSH_PORT:-7143}"
SERVER_HOST="${UENV_SERVER_HOST:-8.130.75.157}"
OH_HOST="${OPENHANDS_HOST:-8.130.208.77}"
SERVER_PASS="${UENV_SERVER_PASS:-dev@BDW2026}"

resolve_worker_key() {
  if [[ -n "${UENV_SSH_KEY:-}" && -f "${UENV_SSH_KEY}" ]]; then echo "${UENV_SSH_KEY}"; return; fi
  for k in "$REPO_ROOT/secrets/9aa460dab6678381f86a1022b8a54c9f_32e42d1c7902ce68ba6719d551645e02_8.143"; do
    [[ -f "$k" ]] && { echo "$k"; return; }
  done
  echo "ERROR: set UENV_SSH_KEY" >&2; exit 1
}
WKEY="$(resolve_worker_key)"
chmod 600 "$WKEY" 2>/dev/null || true
SSH_W=(ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new -i "$WKEY" -p "$WORKER_PORT" root@"$WORKER_HOST")

sync_worker() {
  echo "== sync UEnv -> 7143 /root/UEnv =="
  tar -C "$REPO_ROOT" -czf /tmp/uenv-worker-sync.tgz \
    --exclude=target --exclude=.git --exclude=frontend --exclude='**/src/gen' \
    Cargo.toml Cargo.lock uenv-worker uenv-common uenv-server uenv-bridge \
    proto plugin_proto config scripts integrations/openhands
  cat /tmp/uenv-worker-sync.tgz | "${SSH_W[@]}" 'mkdir -p /root/UEnv && tar -xzf - -C /root/UEnv'
}

deploy_worker() {
  echo "== deploy 7143 swe-pro worker =="
  "${SSH_W[@]}" bash -s <<'REMOTE'
set -euo pipefail
cd /root/UEnv
if [[ ! -f /root/.uenv-trajectory.env ]]; then
  cp config/uenv-trajectory.env.example /root/.uenv-trajectory.env
  chmod 600 /root/.uenv-trajectory.env
fi
bash scripts/restart-worker-gateway-28097-7143.sh
REMOTE
}

deploy_server() {
  echo "== deploy Server $SERVER_HOST =="
  tar -C "$REPO_ROOT" -czf /tmp/uenv-server-sync.tgz \
    --exclude=target --exclude=.git \
    Cargo.toml Cargo.lock uenv-server uenv-bridge uenv-common config proto
  if command -v sshpass >/dev/null 2>&1; then
    sshpass -p "$SERVER_PASS" scp -o StrictHostKeyChecking=accept-new /tmp/uenv-server-sync.tgz "root@${SERVER_HOST}:/tmp/"
    sshpass -p "$SERVER_PASS" ssh -o StrictHostKeyChecking=accept-new "root@${SERVER_HOST}" bash -s <<'REMOTE'
set -euo pipefail
mkdir -p /home/uenv/UEnv
tar -xzf /tmp/uenv-server-sync.tgz -C /home/uenv/UEnv
cd /home/uenv/UEnv
bash scripts/deploy-adapter-core-75157.sh
REMOTE
  else
    echo "WARN: sshpass not found; run scripts/deploy-adapter-core-75157.sh manually on $SERVER_HOST"
  fi
}

deploy_openhands() {
  bash "$REPO_ROOT/scripts/deploy-openhands-20877.sh"
}

run_acceptance() {
  echo "== OpenHands gold + Server trajectory verify on 208.77 =="
  JKEY="$REPO_ROOT/secrets/2a9f778a35e7d08c738c79493ba643ef_65c3b455afbe3c81a8a757c01b0faae8_8.142"
  [[ -f "$JKEY" ]] || JKEY="${UENV_SSH_KEY:-}"
  ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new \
    -o ProxyCommand="ssh -i $JKEY -p 7142 -W %h:22 root@219.147.100.43" \
    root@"$OH_HOST" 'bash /root/UEnv/scripts/verify-openhands-trajectory-e2e-20877.sh'
}

case "${1:-all}" in
  worker) sync_worker; deploy_worker ;;
  server) deploy_server ;;
  openhands) deploy_openhands ;;
  accept) run_acceptance ;;
  all) sync_worker; deploy_worker; deploy_server; deploy_openhands; run_acceptance ;;
  *) echo "usage: $0 {all|worker|server|openhands|accept}"; exit 1 ;;
esac
