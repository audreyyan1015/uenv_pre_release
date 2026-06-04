#!/usr/bin/env bash
# 实机 release 编译（Server / Worker / Hub）— 仅 E2E 所需 proto 目标
set -euo pipefail

UENV_ROOT="${UENV_ROOT:-/root/UEnv}"
cd "$UENV_ROOT"

source "${HOME}/.cargo/env" 2>/dev/null || true
export PATH="${HOME}/.cargo/bin:${PATH}"

if ! command -v protoc >/dev/null 2>&1; then
  echo "==> installing protobuf-compiler"
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -qq
  apt-get install -y -qq protobuf-compiler
fi

if ! command -v protoc-gen-prost >/dev/null 2>&1; then
  echo "==> installing protoc-gen-prost / protoc-gen-tonic"
  cargo install protoc-gen-prost protoc-gen-tonic
fi

echo "==> make proto (e2e subset)"
make proto-server proto-worker proto-plugin proto-hub

echo "==> cargo build --release"
(cd uenv-server && cargo build --release)
(cd uenv-worker && cargo build --release)
(cd uenv-hub && cargo build -p uenv-hub-server --release)

echo "==> verify binaries"
ls -la target/release/uenv-worker target/release/uenv-math-plugin 2>/dev/null || ls -la uenv-worker/target/release/uenv-worker uenv-worker/target/release/uenv-math-plugin
ls -la uenv-server/target/release/uenv-server
ls -la uenv-hub/target/release/uenv-hub-server
test -f plugins/math/manifest.yaml && echo "plugins/math OK"

echo "==> build done"
