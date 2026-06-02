#!/usr/bin/env bash
# 在两台 A100 上初始化 Server-Worker 联调目录布局
# 用法: bash init-e2e-layout.sh

set -euo pipefail

UENV_ROOT="${UENV_ROOT:-/root/UEnv}"

echo "==> 创建代码目录: $UENV_ROOT"
mkdir -p "$UENV_ROOT"

echo "==> 创建运行时目录"
mkdir -p /var/log/uenv /tmp/uenv/wal

echo "==> 目录布局"
printf '  UENV_ROOT (代码)  %s\n' "$UENV_ROOT"
printf '  日志              /var/log/uenv/{server,worker}.log\n'
printf '  WAL               /tmp/uenv/wal\n'

if [[ -d "$UENV_ROOT/.git" ]] || [[ -f "$UENV_ROOT/Cargo.toml" ]]; then
  echo "==> 检测到已有代码，跳过占位"
else
  echo "==> 等待代码同步（sync-from-dev.ps1 或 scp）"
fi

echo "==> 完成"
