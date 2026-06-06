#!/usr/bin/env bash
# 停止本机所有 uenv 相关进程
set -euo pipefail

echo "==> stopping uenv processes"
pkill -f 'uenv-server' 2>/dev/null || true
pkill -f 'uenv-worker' 2>/dev/null || true
pkill -f 'uenv-hub-server' 2>/dev/null || true
sleep 1
if pgrep -af 'uenv-(server|worker|hub)' 2>/dev/null; then
  echo "WARN: some uenv processes still running"
  pgrep -af 'uenv-(server|worker|hub)' || true
else
  echo "==> all uenv processes stopped"
fi
