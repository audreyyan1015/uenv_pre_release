#!/usr/bin/env bash
# 机器 B：启动 Worker（math + Hub）
set -euo pipefail

UENV_ROOT="${UENV_ROOT:-/root/UEnv}"
cd "$UENV_ROOT"
source "${HOME}/.cargo/env" 2>/dev/null || true

WORKER_BIN="${UENV_ROOT}/target/release/uenv-worker"
CONFIG="${UENV_ROOT}/Docs/discussions/a100-server-worker-e2e/config/uenv-worker.e2e.yaml"

if [[ ! -x "$WORKER_BIN" ]]; then
  WORKER_BIN="${UENV_ROOT}/uenv-worker/target/release/uenv-worker"
fi
PLUGIN_BIN="${UENV_ROOT}/target/release/uenv-math-plugin"
if [[ ! -x "$PLUGIN_BIN" ]]; then
  PLUGIN_BIN="${UENV_ROOT}/uenv-worker/target/release/uenv-math-plugin"
fi
export UENV_MATH_PLUGIN_BIN="$PLUGIN_BIN"
export UENV_ENV_TYPES=math

mkdir -p /var/log/uenv /tmp/uenv/wal

if [[ ! -x "$WORKER_BIN" ]]; then
  echo "missing $WORKER_BIN (run remote-build.sh first)" >&2
  exit 1
fi
if [[ ! -x "$UENV_MATH_PLUGIN_BIN" ]]; then
  echo "missing $UENV_MATH_PLUGIN_BIN" >&2
  exit 1
fi

nohup "$WORKER_BIN" --config "$CONFIG" serve > /var/log/uenv/worker.log 2>&1 &
echo "worker pid=$! log=/var/log/uenv/worker.log"

sleep 3
tail -20 /var/log/uenv/worker.log
ss -tlnp | grep -E '50052|19090' || true
