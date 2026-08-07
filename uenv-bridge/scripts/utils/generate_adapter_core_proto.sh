#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
WORKSPACE_ROOT="$(cd "$ROOT/.." && pwd)"
PROTO_DIR="$WORKSPACE_ROOT/proto/uenv/v1"
PYTHON_BIN="${PYTHON:-$(command -v python3 || command -v python)}"

mkdir -p "$ROOT/src/uenv/bridge/gen"
"$PYTHON_BIN" -m grpc_tools.protoc \
  -I="$PROTO_DIR" \
  "$PROTO_DIR/adapter_core.proto" \
  --python_out="$ROOT/src/uenv/bridge/gen" \
  --grpc_python_out="$ROOT/src/uenv/bridge/gen"

# grpc_tools emits absolute imports and may add runtime version hard checks.
# Keep imports package-qualified and compatible with the VeRL training image.
"$PYTHON_BIN" - "$ROOT/src/uenv/bridge/gen/adapter_core_pb2_grpc.py" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
text = path.read_text(encoding="utf-8")
text = text.replace(
    "\nimport adapter_core_pb2 as adapter__core__pb2\n",
    "\nfrom . import adapter_core_pb2 as adapter__core__pb2\n",
)
lines = []
skip_runtime_error_block = False
for line in text.splitlines():
    if line == "import warnings":
        continue
    if line.startswith("GRPC_GENERATED_VERSION = "):
        skip_runtime_error_block = True
        continue
    if skip_runtime_error_block and line.startswith("class "):
        skip_runtime_error_block = False
    if skip_runtime_error_block:
        continue
    lines.append(line)
text = "\n".join(lines) + "\n"
path.write_text(text, encoding="utf-8")
PY

"$PYTHON_BIN" - "$ROOT/src/uenv/bridge/gen/adapter_core_pb2.py" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
text = path.read_text(encoding="utf-8")
lines = []
skip_runtime_check = False
for line in text.splitlines():
    if line == "from google.protobuf import runtime_version as _runtime_version":
        continue
    if line.startswith("_runtime_version.ValidateProtobufRuntimeVersion("):
        skip_runtime_check = True
        continue
    if skip_runtime_check and line == ")":
        skip_runtime_check = False
        continue
    if skip_runtime_check:
        continue
    lines.append(line)
path.write_text("\n".join(lines) + "\n", encoding="utf-8")
PY
