#!/usr/bin/env bash
# 7143：将 Runtime Gateway 切换至 :28097 并重启 Worker（在 7143 本机执行）
# SKIP_REBUILD=1（默认）跳过 cargo，避免长编导致 SSH 无输出超时；需要重编时 SKIP_REBUILD=0
set -euo pipefail
cd /root/UEnv
SKIP_REBUILD="${SKIP_REBUILD:-1}"

if [[ "$SKIP_REBUILD" != "1" ]]; then
  echo "== rebuild worker =="
  source ~/.cargo/env 2>/dev/null || true
  bash scripts/gen-worker-proto.sh
  cargo build -p uenv-worker --release 2>&1 | tail -20
else
  echo "== skip rebuild (SKIP_REBUILD=1); binary=$(ls -lh ./target/release/uenv-worker | awk '{print $5,$6,$7,$8,$9}') =="
fi

echo "== prepare dirs =="
sudo mkdir -p /var/lib/uenv/swe-artifacts/spool/pending /var/lib/uenv/swe-artifacts/bodies /var/lib/uenv/swe-artifacts/index/by-id

echo "== stop old worker (keep swe-pro image pull) =="
pkill -f 'uenv-worker.*serve' || true
sleep 2
fuser -k 28097/tcp 2>/dev/null || true
fuser -k 28888/tcp 2>/dev/null || true
sleep 2
source /root/.uenv-worker.env 2>/dev/null || true
source /root/.uenv-trajectory.env 2>/dev/null || true
export UENV_WORKER_ALLOW_DEGRADED_START=1
# VeRL math rollout：预热 math 插件池（与 deploy-7143-swe-pro.yaml pool 段一致）
export UENV_WARMUP_POOL_SIZE="${UENV_WARMUP_POOL_SIZE:-4}"
export UENV_PREWARM_ON_STARTUP="${UENV_PREWARM_ON_STARTUP:-true}"
# Server/Agent 均经本机 28097 隧道访问 Gateway；勿用公网 28097（NAT 未通）
export UENV_SWE_GATEWAY_PUBLIC_URL=http://127.0.0.1:28097
export UENV_SWE_ARTIFACT_DIR="${UENV_SWE_ARTIFACT_DIR:-/var/lib/uenv/swe-artifacts}"
export UENV_TRAJECTORY_ENDPOINT="${UENV_TRAJECTORY_ENDPOINT:-http://8.130.75.157:8077}"
# DSCodeBench（若已 sync）
if [[ -d /var/lib/uenv/envs/dscodebench/0.2.0/benchmark ]]; then
  export UENV_DSCODEBENCH_ROOT="${UENV_DSCODEBENCH_ROOT:-/var/lib/uenv/envs/dscodebench/0.2.0/benchmark}"
fi
if [[ -x /var/lib/uenv/envs/dscodebench/0.2.0/venv/bin/python ]]; then
  export UENV_CODE_PYTHON="${UENV_CODE_PYTHON:-/var/lib/uenv/envs/dscodebench/0.2.0/venv/bin/python}"
fi
# EnvPackage：优先 0.3.4（全量 catalog）；.env 已设则尊重
export UENV_SWE_ENV_PACKAGE="${UENV_SWE_ENV_PACKAGE:-}"
if [[ -z "$UENV_SWE_ENV_PACKAGE" ]]; then
  if [[ -d /var/lib/uenv/envs/swe-bench-pro/0.3.4 ]]; then
    export UENV_SWE_ENV_PACKAGE=/var/lib/uenv/envs/swe-bench-pro/0.3.4
  elif [[ -d /var/lib/uenv/envs/swe-bench-pro/0.2.0 ]]; then
    export UENV_SWE_ENV_PACKAGE=/var/lib/uenv/envs/swe-bench-pro/0.2.0
  fi
fi
# Legacy fallback when EnvPackage not synced
export UENV_SWE_INSTANCES="${UENV_SWE_INSTANCES:-/root/UEnv/config/swe/pro.json}"
export UENV_SWE_RUNTIME=docker
# 本机已预拉 / docker load 的镜像走 local；缺图时再由运维拉，不在重启脚本里强制公网 pull
export UENV_SWE_IMAGE_PULL_POLICY="${UENV_SWE_IMAGE_PULL_POLICY:-local_only}"

echo "== starting worker (SWE_ENV_PACKAGE=$UENV_SWE_ENV_PACKAGE DSCODE_ROOT=${UENV_DSCODEBENCH_ROOT:-unset}) =="
nohup ./target/release/uenv-worker --config config/uenv-worker.deploy-7143-swe-pro.yaml serve \
  >> /var/log/uenv/worker-swe-pro.log 2>&1 &
echo "worker_pid=$!"
for i in 1 2 3 4 5 6 7 8 9 10 11 12 15; do
  sleep 2
  if curl -fsS -m 2 http://127.0.0.1:28777/health >/dev/null 2>&1; then
    echo "[poll $i] health ok"
    break
  fi
  echo "[poll $i] waiting health..."
done

echo "== local health/gateway =="
curl -sS -m 3 http://127.0.0.1:28777/health; echo
curl -sS -m 3 -H 'X-API-Key: swe-pro-secret' http://127.0.0.1:28097/health; echo
ss -tlnp | grep -E '28097|28888|28777' || true
echo "Ensure A100 NAT maps public 219.147.100.43:28097 -> 10.10.20.143:28097"
