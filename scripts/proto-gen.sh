#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

echo "=== Generating Rust gRPC code (requires protoc + protoc-gen-prost + protoc-gen-tonic) ==="

mkdir -p "$ROOT/uenv-server/src/gen" "$ROOT/uenv-worker/src/gen" \
    "$ROOT/uenv-mock-scheduler/src/gen" "$ROOT/uenv-hub/src/gen" \
    "$ROOT/uenv-bridge/src/gen"

# uenv-server
protoc -I="$ROOT/proto" -I="$ROOT/uenv-server/proto" \
    "$ROOT/uenv-server/proto/server.proto" \
    "$ROOT/proto/uenv/v1/episode.proto" \
    "$ROOT/proto/uenv/v1/common.proto" \
    "$ROOT/proto/uenv/v1/wal.proto" \
    --prost_out="$ROOT/uenv-server/src/gen" \
    --tonic_out="$ROOT/uenv-server/src/gen"

# uenv-worker
protoc -I="$ROOT/proto" -I="$ROOT/uenv-worker/proto" \
    "$ROOT/uenv-worker/proto/worker_service.proto" \
    "$ROOT/proto/uenv/v1/episode.proto" \
    "$ROOT/proto/uenv/v1/common.proto" \
    "$ROOT/proto/uenv/v1/wal.proto" \
    --prost_out="$ROOT/uenv-worker/src/gen" \
    --tonic_out="$ROOT/uenv-worker/src/gen"

# uenv-mock-scheduler
protoc -I="$ROOT/proto" \
    "$ROOT/proto/uenv/v1/scheduler.proto" \
    "$ROOT/proto/uenv/v1/episode.proto" \
    "$ROOT/proto/uenv/v1/common.proto" \
    "$ROOT/proto/uenv/v1/wal.proto" \
    --prost_out="$ROOT/uenv-mock-scheduler/src/gen" \
    --tonic_out="$ROOT/uenv-mock-scheduler/src/gen"

# uenv-hub
protoc -I="$ROOT/proto" -I="$ROOT/uenv-hub/proto" \
    "$ROOT/uenv-hub/proto/hub.proto" \
    "$ROOT/proto/uenv/v1/episode.proto" \
    "$ROOT/proto/uenv/v1/common.proto" \
    --prost_out="$ROOT/uenv-hub/src/gen" \
    --tonic_out="$ROOT/uenv-hub/src/gen"

echo "=== Generating Python gRPC code ==="

# uenv-bridge (design 术语 uenv-adapter = uenv-bridge)
protoc -I="$ROOT/proto" -I="$ROOT/uenv-server/proto" \
    "$ROOT/uenv-server/proto/server.proto" \
    "$ROOT/proto/uenv/v1/episode.proto" \
    "$ROOT/proto/uenv/v1/common.proto" \
    --python_out="$ROOT/uenv-bridge/src/gen" \
    --grpc_python_out="$ROOT/uenv-bridge/src/gen"

echo "=== Generating L2 plugin proto (worker only) ==="

protoc -I="$ROOT/plugin_proto" \
    "$ROOT/plugin_proto/uenv/plugin/v1/plugin.proto" \
    --prost_out="$ROOT/uenv-worker/src/gen" \
    --tonic_out="$ROOT/uenv-worker/src/gen"

echo "=== Done ==="
