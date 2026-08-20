#!/usr/bin/env bash
# 208.77：同步 UEnv 集成代码并确保 OpenHands runner / SSH 隧道 / Agent poll 就绪
# 开发机：UENV_SSH_KEY=secrets/... bash scripts/deploy-openhands-20877.sh
# 启用 Server 编排：OPENHANDS_ENABLE_POLL=1 bash scripts/deploy-openhands-20877.sh
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OH_HOST="${OPENHANDS_HOST:-8.130.208.77}"
JUMP_HOST="${UENV_JUMP_HOST:-219.147.100.43}"
JUMP_PORT="${UENV_JUMP_PORT:-7142}"
REMOTE_UENV="${UENV_REMOTE_UENV:-/root/UENV}"
ENABLE_POLL="${OPENHANDS_ENABLE_POLL:-0}"
AGENT_REPLICAS="${OPENHANDS_AGENT_REPLICAS:-1}"
ENABLE_AUTOSCALE="${OPENHANDS_ENABLE_AUTOSCALE:-0}"

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
# 仅打包存在的路径，避免缺文件导致 tar 失败
TAR_LIST=(
  integrations/openhands
  config/openhands-20877.env.example
  scripts/run-openhands-pro-20877.sh
  scripts/openhands
  scripts/gen-openhands-llm-config.py
  proto/uenv/v1/agent.proto
)
for opt in \
  config/openhands-llm-20877.json.example \
  config/uenv-trajectory.env.example \
  config/swe/pro-python-smoke.json \
  scripts/verify-openhands-trajectory-e2e-20877.sh \
  scripts/verify-swe-agent-orchestration-e2e.sh \
  scripts/swe_agent_orchestration_e2e.py
do
  [[ -e "$REPO_ROOT/$opt" ]] && TAR_LIST+=("$opt")
done
COPYFILE_DISABLE=1 tar -C "$REPO_ROOT" --exclude='__pycache__' --exclude='._*' \
  -czf /tmp/uenv-oh-sync.tgz "${TAR_LIST[@]}"

"${SSH_OH[@]}" "mkdir -p $REMOTE_UENV /root/UEnv"
cat /tmp/uenv-oh-sync.tgz | "${SSH_OH[@]}" "tar -xzf - -C /root/UEnv && cp -a /root/UEnv/. $REMOTE_UENV/ 2>/dev/null || true"

"${SSH_OH[@]}" bash -s <<REMOTE
set -euo pipefail
cd /root/UEnv
chmod +x scripts/run-openhands-pro-20877.sh scripts/openhands/*.py 2>/dev/null || true
chmod +x scripts/verify-openhands-trajectory-e2e-20877.sh \
  scripts/verify-swe-agent-orchestration-e2e.sh scripts/swe_agent_orchestration_e2e.py 2>/dev/null || true

# Quarantine stale Agent-host /app checkout (openlibrary) so LocalWorkspace fallback cannot pollute runs.
if [[ -d /app/.git ]] && git -C /app remote get-url origin 2>/dev/null | grep -qi openlibrary; then
  BAK="/app.openlibrary-host-backup-\$(date +%Y%m%d%H%M%S)"
  echo "== moving stale Agent-host /app (openlibrary) to \$BAK =="
  mv /app "\$BAK"
  mkdir -p /app
  echo "Agent-host /app quarantined; gateway workspace is remote-only." > /app/README.uenv-quarantine
fi

if [[ ! -f /root/.openhands-20877.env ]]; then
  cp config/openhands-20877.env.example /root/.openhands-20877.env
  chmod 600 /root/.openhands-20877.env
fi
if [[ -f config/uenv-trajectory.env.example && ! -f /root/.uenv-trajectory.env ]]; then
  cp config/uenv-trajectory.env.example /root/.uenv-trajectory.env
  chmod 600 /root/.uenv-trajectory.env
fi

# Agent poller 使用独立 venv（系统 protobuf 过旧，无法生成/导入含 optional 的 agent stub）
if [[ ! -x /root/uenv-agent-venv/bin/python ]]; then
  python3 -m venv /root/uenv-agent-venv
  /root/uenv-agent-venv/bin/pip -q install -U pip
  /root/uenv-agent-venv/bin/pip -q install "grpcio>=1.60" "grpcio-tools>=1.60" "protobuf>=4.25"
fi
mkdir -p integrations/openhands/uenv_runtime/gen
/root/uenv-agent-venv/bin/python -m grpc_tools.protoc \
  -I=proto proto/uenv/v1/agent.proto \
  --python_out=integrations/openhands/uenv_runtime/gen \
  --grpc_python_out=integrations/openhands/uenv_runtime/gen
touch integrations/openhands/uenv_runtime/gen/__init__.py \
  integrations/openhands/uenv_runtime/gen/uenv/__init__.py \
  integrations/openhands/uenv_runtime/gen/uenv/v1/__init__.py

ENABLE_POLL="${ENABLE_POLL}"
AGENT_REPLICAS="${AGENT_REPLICAS}"
ENABLE_AUTOSCALE="${ENABLE_AUTOSCALE}"
if [[ "\$ENABLE_POLL" == "1" ]]; then
  echo "== enable Server poll mode =="
  grep -q '^OPENHANDS_AGENT_POLL=' /root/.openhands-20877.env 2>/dev/null && \
    sed -i 's/^OPENHANDS_AGENT_POLL=.*/OPENHANDS_AGENT_POLL=1/' /root/.openhands-20877.env || \
    echo 'OPENHANDS_AGENT_POLL=1' >> /root/.openhands-20877.env
  grep -q '^UENV_SERVER_ENDPOINT=' /root/.openhands-20877.env || \
    echo 'UENV_SERVER_ENDPOINT=8.130.75.157:8088' >> /root/.openhands-20877.env
  grep -q '^OPENHANDS_AGENT_POOL_ID=' /root/.openhands-20877.env || \
    echo 'OPENHANDS_AGENT_POOL_ID=openhands-default' >> /root/.openhands-20877.env
  grep -q '^UENV_GATEWAY_LOCAL=' /root/.openhands-20877.env || \
    echo 'UENV_GATEWAY_LOCAL=http://127.0.0.1:28097' >> /root/.openhands-20877.env
  grep -q '^UENV_GATEWAY_API_KEY=' /root/.openhands-20877.env || \
    echo 'UENV_GATEWAY_API_KEY=REPLACE_WITH_RANDOM_GATEWAY_API_KEY' >> /root/.openhands-20877.env
  grep -q '^OPENHANDS_RUN_TIMEOUT_SEC=' /root/.openhands-20877.env || \
    echo 'OPENHANDS_RUN_TIMEOUT_SEC=7200' >> /root/.openhands-20877.env
  grep -q '^OPENHANDS_MAX_OUTPUT_TOKENS=' /root/.openhands-20877.env || \
    echo 'OPENHANDS_MAX_OUTPUT_TOKENS=32768' >> /root/.openhands-20877.env
  grep -q '^OPENHANDS_AGENT_MAX_CONCURRENT=' /root/.openhands-20877.env || \
    echo 'OPENHANDS_AGENT_MAX_CONCURRENT=1' >> /root/.openhands-20877.env
  grep -q '^OPENHANDS_AGENT_ID=' /root/.openhands-20877.env || \
    echo 'OPENHANDS_AGENT_ID=openhands-20877-main' >> /root/.openhands-20877.env
  cp scripts/openhands/uenv-agent-poller.service /etc/systemd/system/uenv-agent-poller.service
  sed -i 's|^ExecStart=.*|ExecStart=/root/uenv-agent-venv/bin/python /root/UEnv/scripts/openhands/openhands_runner.py|' \
    /etc/systemd/system/uenv-agent-poller.service
  systemctl daemon-reload
  systemctl stop openhands-runner.service 2>/dev/null || true
  systemctl disable openhands-runner.service 2>/dev/null || true
  systemctl enable uenv-agent-poller.service
  systemctl restart uenv-agent-poller.service

  if [[ "\$AGENT_REPLICAS" =~ ^[0-9]+$ && "\$AGENT_REPLICAS" -gt 1 ]]; then
    echo "== enable additional OpenHands agent pollers replicas=\$AGENT_REPLICAS =="
    for idx in \$(seq 1 \$((AGENT_REPLICAS - 1))); do
      slot=\$(printf '%02d' "\$idx")
      api=\$((8888 + idx))
      health=\$((8777 + idx))
      runs_dir="/var/log/uenv/openhands-extra-\$slot"
      mkdir -p "\$runs_dir"
      cat > "/etc/systemd/system/uenv-agent-poller-extra-\$slot.service" <<EXTRA
[Unit]
Description=UEnv OpenHands Agent poller extra \$slot (208.77 -> Server AgentControlService)
After=network-online.target uenv-gateway-tunnel.service
Wants=network-online.target

[Service]
Type=simple
ExecStart=/bin/bash -lc 'set -a; . /root/.openhands-20877.env; set +a; export OPENHANDS_AGENT_POLL=1 UENV_SERVER_ENDPOINT=8.130.75.157:8088 OPENHANDS_AGENT_POOL_ID=openhands-default OPENHANDS_AGENT_BRIDGE_ID=uenv-agent-openhands OPENHANDS_AGENT_BRIDGE_VERSION=1.0.0 OPENHANDS_AGENT_MAX_CONCURRENT=1 OPENHANDS_POLL_INTERVAL_SEC=1 OPENHANDS_HEARTBEAT_INTERVAL_SEC=5 UENV_AGENT_BRIDGE_DIR=/root/UEnv/integrations/openhands OPENHANDS_RUN_SCRIPT=/root/UEnv/scripts/run-openhands-pro-20877.sh UENV_GATEWAY_LOCAL=http://127.0.0.1:28097 UENV_GATEWAY_API_KEY=REPLACE_WITH_RANDOM_GATEWAY_API_KEY OPENHANDS_AGENT_ID=openhands-20877-extra-\$slot OPENHANDS_RUNS_DIR=\$runs_dir OPENHANDS_COMPLETION_SPOOL_DIR=\$runs_dir/completion-spool OPENHANDS_RUNNER_API_BIND=127.0.0.1:\$api OPENHANDS_RUNNER_HEALTH_BIND=127.0.0.1:\$health OPENHANDS_AGENT_LABELS=role=openhands,slot=extra-\$slot; exec /root/uenv-agent-venv/bin/python /root/UEnv/scripts/openhands/openhands_runner.py'
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
EXTRA
      systemctl enable "uenv-agent-poller-extra-\$slot.service"
      systemctl restart "uenv-agent-poller-extra-\$slot.service"
    done
  fi

  if [[ "\$ENABLE_AUTOSCALE" == "1" ]]; then
    echo "== enable OpenHands agent pool autoscaler =="
    cat > /etc/systemd/system/uenv-agent-pool-supervisor.service <<'SUPERVISOR'
[Unit]
Description=UEnv OpenHands Agent Pool Supervisor
After=network-online.target uenv-agent-poller.service
Wants=network-online.target

[Service]
Type=simple
Environment=UENV_AGENT_SUPERVISOR_ADMIN_BASE_URL=http://8.130.75.157:8099
Environment=OPENHANDS_AGENT_POOL_ID=openhands-default
Environment=OPENHANDS_AGENT_AUTOSCALE_MIN=1
Environment=OPENHANDS_AGENT_AUTOSCALE_MAX=4
Environment=OPENHANDS_AGENT_AUTOSCALE_INTERVAL_SEC=10
Environment=UENV_GATEWAY_LOCAL=http://127.0.0.1:28097
Environment=UENV_GATEWAY_API_KEY=REPLACE_WITH_RANDOM_GATEWAY_API_KEY
ExecStart=/root/uenv-agent-venv/bin/python /root/UEnv/scripts/openhands/agent_pool_supervisor.py
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
SUPERVISOR
    systemctl daemon-reload
    systemctl enable uenv-agent-pool-supervisor.service
    systemctl restart uenv-agent-pool-supervisor.service
  else
    systemctl stop uenv-agent-pool-supervisor.service 2>/dev/null || true
    systemctl disable uenv-agent-pool-supervisor.service 2>/dev/null || true
  fi
else
  systemctl is-active openhands-runner.service >/dev/null 2>&1 && systemctl restart openhands-runner.service || true
fi

systemctl is-active uenv-gateway-tunnel.service >/dev/null 2>&1 && systemctl restart uenv-gateway-tunnel.service || true
sleep 3
curl -sf http://127.0.0.1:8777/health && echo " runner_ok" || echo " runner_not_ready"
curl -sf -H 'X-API-Key: REPLACE_WITH_RANDOM_GATEWAY_API_KEY' http://127.0.0.1:28097/health && echo " tunnel_gateway_ok" || echo " tunnel_gateway_fail"
if [[ "\$ENABLE_POLL" == "1" ]]; then
  systemctl is-active uenv-agent-poller.service && echo " agent_poller_active" || echo " agent_poller_inactive"
  if [[ "\$AGENT_REPLICAS" =~ ^[0-9]+$ && "\$AGENT_REPLICAS" -gt 1 ]]; then
    for idx in \$(seq 1 \$((AGENT_REPLICAS - 1))); do
      slot=\$(printf '%02d' "\$idx")
      health=\$((8777 + idx))
      systemctl is-active "uenv-agent-poller-extra-\$slot.service" && echo " agent_poller_extra_\${slot}_active" || echo " agent_poller_extra_\${slot}_inactive"
      curl -sf "http://127.0.0.1:\${health}/health" && echo " agent_poller_extra_\${slot}_health_ok" || echo " agent_poller_extra_\${slot}_health_fail"
    done
  fi
  if [[ "\$ENABLE_AUTOSCALE" == "1" ]]; then
    systemctl is-active uenv-agent-pool-supervisor.service && echo " agent_pool_supervisor_active" || echo " agent_pool_supervisor_inactive"
    journalctl -u uenv-agent-pool-supervisor -n 8 --no-pager 2>/dev/null | tail -5 || true
  fi
  journalctl -u uenv-agent-poller -n 8 --no-pager 2>/dev/null | tail -5 || true
fi
REMOTE

echo "208.77 sync done (OPENHANDS_ENABLE_POLL=${ENABLE_POLL}, OPENHANDS_AGENT_REPLICAS=${AGENT_REPLICAS}, OPENHANDS_ENABLE_AUTOSCALE=${ENABLE_AUTOSCALE})."
