#!/usr/bin/env bash
# 单台 A100 机器环境 bootstrap（Server 或 Worker 均需执行一次）
# 用法: sudo bash prep-bootstrap.sh

set -euo pipefail

echo "==> 安装构建依赖 (Ubuntu/Debian)"
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq build-essential pkg-config libssl-dev protobuf-compiler git curl

echo "==> 安装 grpcurl"
if ! command -v grpcurl >/dev/null 2>&1; then
  if command -v go >/dev/null 2>&1; then
    go install github.com/fullstorydev/grpcurl/cmd/grpcurl@latest
    export PATH="${HOME}/go/bin:${PATH}"
  else
    GRPCURL_VER="1.9.3"
    ARCH="$(uname -m)"
    case "$ARCH" in
      x86_64) GRPCURL_ARCH="x86_64" ;;
      aarch64) GRPCURL_ARCH="arm64" ;;
      *) echo "unsupported arch: $ARCH"; exit 1 ;;
    esac
    TMP="$(mktemp -d)"
    curl -fsSL "https://github.com/fullstorydev/grpcurl/releases/download/v${GRPCURL_VER}/grpcurl_${GRPCURL_VER}_linux_${GRPCURL_ARCH}.tar.gz" \
      | tar -xz -C "$TMP"
    install -m 755 "$TMP/grpcurl" /usr/local/bin/grpcurl
    rm -rf "$TMP"
  fi
fi
grpcurl -version

echo "==> Rust toolchain"
if ! command -v cargo >/dev/null 2>&1; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
  # shellcheck disable=SC1091
  source "${HOME}/.cargo/env"
fi
rustc --version
cargo --version

echo "==> 日志与 WAL 目录"
mkdir -p /var/log/uenv /tmp/uenv/wal
chmod 755 /var/log/uenv /tmp/uenv

echo "==> 完成。下一步: 同步 UEnv 代码至 /root/UEnv 并 cargo build"
