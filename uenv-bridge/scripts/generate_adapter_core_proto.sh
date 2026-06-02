#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORKSPACE_ROOT="$(cd "$ROOT/.." && pwd)"
PYTHON_BIN="${PYTHON:-$(command -v python3 || command -v python)}"

mkdir -p "$ROOT/src/uenv/bridge/gen"
"$PYTHON_BIN" -m grpc_tools.protoc \
  -I="$WORKSPACE_ROOT/proto" \
  "$WORKSPACE_ROOT/proto/uenv/v1/adapter_core.proto" \
  --python_out="$ROOT/src/uenv/bridge/gen" \
  --grpc_python_out="$ROOT/src/uenv/bridge/gen"

# grpc_tools emits absolute imports in *_pb2_grpc.py. Keep a package-qualified
# import so uenv.bridge.gen can be imported without mutating sys.path.
"$PYTHON_BIN" - "$ROOT/src/uenv/bridge/gen/adapter_core_pb2_grpc.py" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
text = path.read_text(encoding="utf-8")
text = text.replace("import adapter_core_pb2 as adapter__core__pb2", "from . import adapter_core_pb2 as adapter__core__pb2")
path.write_text(text, encoding="utf-8")
PY
