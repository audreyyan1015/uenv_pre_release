#!/usr/bin/env bash
# 仅为 uenv-worker 生成 prost/tonic 代码（隔离构建用）。需 protoc + protoc-gen-prost + protoc-gen-tonic。
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/uenv-worker/src/gen"
mkdir -p "$OUT"

protoc -I="$ROOT/proto" -I="$ROOT/uenv-worker/proto" \
  "$ROOT/uenv-worker/proto/worker_service.proto" \
  "$ROOT/proto/uenv/v1/scheduler.proto" \
  "$ROOT/proto/uenv/v1/episode.proto" \
  "$ROOT/proto/uenv/v1/common.proto" \
  "$ROOT/proto/uenv/v1/wal.proto" \
  --prost_out="$OUT" --tonic_out="$OUT"

protoc -I="$ROOT/plugin_proto" \
  "$ROOT/plugin_proto/uenv/plugin/v1/plugin.proto" \
  --prost_out="$OUT" --tonic_out="$OUT"

echo "worker proto generated -> $OUT"
