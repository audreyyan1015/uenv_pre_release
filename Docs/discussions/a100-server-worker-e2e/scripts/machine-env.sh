#!/usr/bin/env bash
# 实机联调环境变量 — 在对应机器 source 此文件
# 用法: source scripts/machine-env.sh server|worker

set -euo pipefail

MACHINE_A_IP="${MACHINE_A_IP:-10.10.20.143}"
MACHINE_B_IP="${MACHINE_B_IP:-10.10.20.142}"
UENV_ROOT="${UENV_ROOT:-/root/UEnv}"

case "${1:-}" in
  server|a|A)
    export UENV_ROLE=server
    export UENV_LISTEN="0.0.0.0:50051"
    export UENV_LOG_FILE="/var/log/uenv/server.log"
    ;;
  worker|b|B)
    export UENV_ROLE=worker
    export UENV_SCHEDULER_MODE=remote
    export UENV_SERVER_ENDPOINT="${MACHINE_A_IP}:50051"
    # 注册 endpoint 须为 Server 可达的内网 IP（同时用于 bind）
    export UENV_WORKER_LISTEN="${MACHINE_B_IP}:50052"
    export UENV_ENV_TYPES=math
    export UENV_PLUGIN_DIR="${UENV_ROOT}/plugins"
    export UENV_WARMUP_POOL_SIZE=2
    export UENV_MAX_CONCURRENT=4
    export UENV_METRICS_LISTEN="0.0.0.0:19090"
    export UENV_LOG_FILE="/var/log/uenv/worker.log"
    export UENV_WAL_DIR="/tmp/uenv/wal"
    export UENV_MATH_PLUGIN_BIN="${UENV_ROOT}/target/release/uenv-math-plugin"
    ;;
  *)
    echo "用法: source $0 server|worker"
    return 1 2>/dev/null || exit 1
    ;;
esac

export UENV_ROOT
echo "UENV_ROLE=$UENV_ROLE UENV_ROOT=$UENV_ROOT"
