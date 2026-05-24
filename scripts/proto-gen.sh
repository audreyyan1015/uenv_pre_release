#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

echo "=== Generating Rust gRPC code ==="

# uenv-server
protoc -I="$ROOT/uenv-server/proto" -I="$ROOT/proto" \
    "$ROOT/uenv-server/proto/server.proto" \
    --rust_out="$ROOT/uenv-server/src/gen" \
    --tonic_out="$ROOT/uenv-server/src/gen"

# uenv-worker
protoc -I="$ROOT/uenv-worker/proto" -I="$ROOT/proto" \
    "$ROOT/uenv-worker/proto/worker.proto" \
    --rust_out="$ROOT/uenv-worker/src/gen" \
    --tonic_out="$ROOT/uenv-worker/src/gen"

# uenv-hub
protoc -I="$ROOT/uenv-hub/proto" -I="$ROOT/proto" \
    "$ROOT/uenv-hub/proto/hub.proto" \
    --rust_out="$ROOT/uenv-hub/src/gen" \
    --tonic_out="$ROOT/uenv-hub/src/gen"

echo "=== Generating Python gRPC code ==="

# uenv-adapter
protoc -I="$ROOT/uenv-server/proto" -I="$ROOT/proto" \
    "$ROOT/uenv-server/proto/server.proto" \
    --python_out="$ROOT/uenv-adapter/src/gen" \
    --grpc_python_out="$ROOT/uenv-adapter/src/gen"

echo "=== Done ==="
