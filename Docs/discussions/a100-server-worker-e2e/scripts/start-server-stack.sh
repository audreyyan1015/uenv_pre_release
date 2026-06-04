#!/usr/bin/env bash
# 机器 A：启动 Hub + Server
set -euo pipefail

UENV_ROOT="${UENV_ROOT:-/root/UEnv}"
cd "$UENV_ROOT"
source "${HOME}/.cargo/env" 2>/dev/null || true

mkdir -p /var/log/uenv

SERVER_BIN="${UENV_ROOT}/uenv-server/target/release/uenv-server"
if [[ ! -x "$SERVER_BIN" ]]; then
  SERVER_BIN="${UENV_ROOT}/target/release/uenv-server"
fi
HUB_BIN="${UENV_ROOT}/uenv-hub/target/release/uenv-hub-server"

for bin in "$SERVER_BIN" "$HUB_BIN"; do
  if [[ ! -x "$bin" ]]; then
    echo "missing binary: $bin (run remote-build.sh first)" >&2
    exit 1
  fi
done

nohup env UENV_HUB_AUTH__REQUIRE_TOKEN=false "$HUB_BIN" --bind 0.0.0.0:8080 > /var/log/uenv/hub.log 2>&1 &
echo "hub pid=$! log=/var/log/uenv/hub.log"

nohup "$SERVER_BIN" -b 0.0.0.0:50051 > /var/log/uenv/server.log 2>&1 &
echo "server pid=$! log=/var/log/uenv/server.log"

sleep 2
curl -sf "http://127.0.0.1:8080/api/v1/envs/math" >/dev/null && echo "hub math env OK" || echo "WARN: hub math check failed"
ss -tlnp | grep -E '8080|50051' || true
