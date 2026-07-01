#!/usr/bin/env bash
# 7143：将 Runtime Gateway 切换至 :28097 并重启 Worker（在 7143 本机执行）
set -euo pipefail
cd /root/UEnv

echo "== rebuild worker =="
source ~/.cargo/env 2>/dev/null || true
bash scripts/gen-worker-proto.sh
cargo build -p uenv-worker --release 2>&1 | tail -5

sudo mkdir -p /var/lib/uenv/swe-artifacts/spool/pending /var/lib/uenv/swe-artifacts/bodies /var/lib/uenv/swe-artifacts/index/by-id

pkill -f 'uenv-worker.*serve' || true
sleep 2
source /root/.uenv-worker.env 2>/dev/null || true
source /root/.uenv-trajectory.env 2>/dev/null || true
export UENV_WORKER_ALLOW_DEGRADED_START=1
export UENV_SWE_GATEWAY_PUBLIC_URL=http://219.147.100.43:28097
export UENV_SWE_ARTIFACT_DIR="${UENV_SWE_ARTIFACT_DIR:-/var/lib/uenv/swe-artifacts}"
export UENV_TRAJECTORY_ENDPOINT="${UENV_TRAJECTORY_ENDPOINT:-http://8.130.75.157:8077}"
export UENV_SWE_INSTANCES="${UENV_SWE_INSTANCES:-/root/UEnv/config/swe/pro.json}"
export UENV_SWE_RUNTIME=docker
export UENV_SWE_IMAGE_PULL=1

nohup ./target/release/uenv-worker --config config/uenv-worker.deploy-7143-swe-pro.yaml serve \
  >> /var/log/uenv/worker-swe-pro.log 2>&1 &
sleep 6

echo "== local gateway =="
curl -sS -H 'X-API-Key: swe-pro-secret' http://127.0.0.1:28097/health; echo
ss -tlnp | grep 28097 || true
echo "Ensure A100 NAT maps public 219.147.100.43:28097 -> 10.10.20.143:28097"
