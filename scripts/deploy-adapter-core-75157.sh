#!/usr/bin/env bash
# 8.130.75.157：部署 uenv-adapter-core + 轨迹 HTTP :8077
# 在 Server 本机执行，或经 ssh root@8.130.75.157 'bash -s' < scripts/deploy-adapter-core-75157.sh
set -euo pipefail

UENV_HOME="${UENV_HOME:-/home/uenv}"
cd "$UENV_HOME"

if [[ ! -f config/uenv-server.trajectory.env.example ]]; then
  echo "ERROR: run from UEnv repo root on server ($UENV_HOME)" >&2
  exit 1
fi

if [[ ! -f /root/.uenv-server.env ]]; then
  cp config/uenv-server.trajectory.env.example /root/.uenv-server.env
  chmod 600 /root/.uenv-server.env
  echo "created /root/.uenv-server.env from example — review token before prod"
fi
source /root/.uenv-server.env

mkdir -p "$UENV_HOME/trajectory-data/bodies" "$UENV_HOME/logs"

source "$HOME/.cargo/env" 2>/dev/null || true
cargo build -p uenv-adapter-core --release 2>&1 | tail -8

pkill -f 'uenv-adapter-core' || true
sleep 2
nohup "$UENV_HOME/target/release/uenv-adapter-core" >> "$UENV_HOME/logs/adapter-core.log" 2>&1 &
sleep 4

echo "== gRPC =="
ss -tlnp | grep 8088 || true
curl -sS --max-time 5 http://127.0.0.1:8077/control/v1/trajectories/health && echo " trajectory_ok" || echo " trajectory_health_failed"
ss -tlnp | grep 8077 || true
tail -20 "$UENV_HOME/logs/adapter-core.log" | grep -E 'trajectory|listening|8088' || tail -5 "$UENV_HOME/logs/adapter-core.log"
