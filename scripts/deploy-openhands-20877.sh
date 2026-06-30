#!/usr/bin/env bash
# Deploy OpenHands/benchmarks + runner on 阿里云 8C32G (8.130.208.77).
# Migrates from A100 7142; Worker 7143 Gateway must listen on :28097 (public NAT).
#
# Usage (dev machine with A100 SSH key, jumps via 7142):
#   UENV_SSH_KEY=secrets/... bash scripts/deploy-openhands-20877.sh
#   UENV_SSH_KEY=secrets/... bash scripts/deploy-openhands-20877.sh run-smoke
#
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
JUMP_HOST="${UENV_JUMP_HOST:-219.147.100.43}"
JUMP_PORT="${UENV_JUMP_SSH_PORT:-7142}"
TARGET_HOST="${UENV_OPENHANDS_HOST:-8.130.208.77}"
TARGET_PASS="${UENV_20877_PASS:-dev@BDW2026}"
REMOTE_UENV="${UENV_REMOTE_DIR:-/root/UEnv}"
OPENHANDS_DIR="${OPENHANDS_BENCHMARKS_DIR:-/opt/openhands/benchmarks}"
BENCHMARKS_SHA="${OPENHANDS_BENCHMARKS_SHA:-82687c83dfcc193989336f41d235612c02f2c044}"
RUNS_DIR="${OPENHANDS_RUNS_DIR:-/var/log/uenv/openhands-runs}"
WORKER_GW="${UENV_GATEWAY:-http://219.147.100.43:28097}"

resolve_key() {
  if [[ -n "${UENV_SSH_KEY:-}" && -f "${UENV_SSH_KEY}" ]]; then echo "${UENV_SSH_KEY}"; return; fi
  for k in "$REPO_ROOT/secrets/2a9f778a35e7d08c738c79493ba643ef_65c3b455afbe3c81a8a757c01b0faae8_8.142"; do
    [[ -f "$k" ]] && { echo "$k"; return; }
  done
  echo "ERROR: set UENV_SSH_KEY" >&2
  exit 1
}

KEY="$(resolve_key)"
chmod 600 "$KEY" 2>/dev/null || true
SSH_JUMP=(ssh -o BatchMode=yes -o ConnectTimeout=20 -o StrictHostKeyChecking=accept-new -i "$KEY" -p "$JUMP_PORT" root@"$JUMP_HOST")

ssh_20877() {
  "${SSH_JUMP[@]}" "sshpass -p '$TARGET_PASS' ssh -o StrictHostKeyChecking=no -o ConnectTimeout=20 root@$TARGET_HOST $*"
}

scp_to_20877() {
  local src="$1" dst="$2"
  "${SSH_JUMP[@]}" "sshpass -p '$TARGET_PASS' scp -o StrictHostKeyChecking=no '$src' root@$TARGET_HOST:'$dst'"
}

cmd="${1:-deploy}"

echo "== rsync UEnv -> 7142 staging =="
STAGE="/tmp/uenv-staging-$$"
"${SSH_JUMP[@]}" "rm -rf '$STAGE' && mkdir -p '$STAGE'"
rsync -az \
  --exclude 'target/' --exclude '.git/' --exclude 'frontend/' --exclude 'node_modules/' \
  -e "ssh -i $KEY -p $JUMP_PORT -o BatchMode=yes -o StrictHostKeyChecking=accept-new" \
  "$REPO_ROOT/" root@"$JUMP_HOST":"$STAGE/"

echo "== rsync UEnv 7142 -> 208.77:$REMOTE_UENV =="
"${SSH_JUMP[@]}" "sshpass -p '$TARGET_PASS' rsync -az --delete \
  --exclude 'target/' --exclude '.git/' \
  -e 'ssh -o StrictHostKeyChecking=no' \
  '$STAGE/' root@$TARGET_HOST:'$REMOTE_UENV/'"
"${SSH_JUMP[@]}" "rm -rf '$STAGE'"

echo "== migrate OpenHands benchmarks 7142 -> 208.77 (if missing) =="
"${SSH_JUMP[@]}" bash -s <<'MIGRATE'
set -euo pipefail
TARGET_PASS="${UENV_20877_PASS:-dev@BDW2026}"
TARGET_HOST="${UENV_OPENHANDS_HOST:-8.130.208.77}"
OPENHANDS_DIR="/opt/openhands/benchmarks"
if sshpass -p "$TARGET_PASS" ssh -o StrictHostKeyChecking=no root@$TARGET_HOST "test -d $OPENHANDS_DIR/vendor/software-agent-sdk"; then
  echo "benchmarks already on 208.77"
  exit 0
fi
if [[ ! -d /opt/openhands/benchmarks ]]; then
  echo "ERROR: no benchmarks on 7142 at /opt/openhands/benchmarks" >&2
  exit 1
fi
tar czf /tmp/openhands-benchmarks.tgz -C /opt/openhands/benchmarks .
sshpass -p "$TARGET_PASS" ssh -o StrictHostKeyChecking=no root@$TARGET_HOST \
  "mkdir -p $OPENHANDS_DIR && tar xzf - -C $OPENHANDS_DIR" < /tmp/openhands-benchmarks.tgz
rm -f /tmp/openhands-benchmarks.tgz
echo "benchmarks tarball migrated"
MIGRATE

echo "== setup 208.77 =="
ssh_20877 bash -s <<REMOTE
set -euo pipefail
OPENHANDS_DIR="$OPENHANDS_DIR"
BENCHMARKS_SHA="$BENCHMARKS_SHA"
REMOTE_UENV="$REMOTE_UENV"
RUNS_DIR="$RUNS_DIR"
WORKER_GW="$WORKER_GW"

export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq git curl sshpass rsync python3 python3-venv 2>/dev/null || true
command -v uv >/dev/null || curl -LsSf https://astral.sh/uv/install.sh | sh
export PATH="\$HOME/.local/bin:\$PATH"

mkdir -p "\$RUNS_DIR" "\$(dirname "\$OPENHANDS_DIR")"
if [[ -d "\$OPENHANDS_DIR/.git" ]]; then
  cd "\$OPENHANDS_DIR"
  git fetch origin 2>/dev/null || true
  git checkout "\$BENCHMARKS_SHA" 2>/dev/null || true
fi
if [[ -d "\$OPENHANDS_DIR/vendor/software-agent-sdk" ]]; then
  cd "\$OPENHANDS_DIR/vendor/software-agent-sdk"
  uv sync
fi

if [[ ! -f /root/.openhands-20877.env ]]; then
  cp "\$REMOTE_UENV/config/openhands-20877.env.example" /root/.openhands-20877.env
  chmod 600 /root/.openhands-20877.env
fi

chmod +x "\$REMOTE_UENV/scripts/run-openhands-pro-20877.sh"

cp "\$REMOTE_UENV/scripts/openhands/uenv-gateway-tunnel.service" /etc/systemd/system/
systemctl daemon-reload
systemctl enable uenv-gateway-tunnel
systemctl restart uenv-gateway-tunnel 2>/dev/null || echo "WARN: uenv-gateway-tunnel failed (check /root/.ssh/uenv-7142-jump)"

cat >/etc/systemd/system/openhands-runner.service <<UNIT
[Unit]
Description=OpenHands benchmark runner (208.77 :8888 / :8777)
After=network.target

[Service]
Type=simple
EnvironmentFile=/root/.openhands-20877.env
ExecStart=/usr/bin/python3 \$REMOTE_UENV/scripts/openhands/openhands_runner.py
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
UNIT

systemctl daemon-reload
systemctl enable openhands-runner
systemctl restart openhands-runner
sleep 1
systemctl is-active openhands-runner
curl -sS http://127.0.0.1:8777/health; echo
curl -sS http://127.0.0.1:8888/health; echo

echo "gateway probe -> \$WORKER_GW/health"
curl -sS -m 10 -H "X-API-Key: swe-pro-secret" "\$WORKER_GW/health" || echo "WARN: worker gateway not reachable yet (open 28097 on A100 NAT)"
REMOTE

if [[ "$cmd" == "deploy" ]]; then
  echo "deploy complete on $TARGET_HOST"
  exit 0
fi

if [[ "$cmd" == "run-smoke" || "$cmd" == "run-llm" ]]; then
  MODE=llm
  [[ "$cmd" == "run-smoke" ]] && MODE=gold
  ssh_20877 "bash $REMOTE_UENV/scripts/run-openhands-pro-20877.sh $MODE"
  exit 0
fi

echo "usage: $0 [deploy|run-smoke|run-llm]" >&2
exit 1
