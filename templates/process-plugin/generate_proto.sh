#!/usr/bin/env bash
# Protocol maintenance helper. Environment authors normally do not run or edit
# this file; task behavior belongs in environment.py.
set -euo pipefail

PLUGIN_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROTO_DIR="$(cd "${PLUGIN_DIR}/../../plugin_proto/uenv/plugin/v1" && pwd)"
PYTHON_BIN="${UENV_PLUGIN_PYTHON:-${PLUGIN_DIR}/.venv/bin/python}"

if [[ ! -x "${PYTHON_BIN}" ]]; then
  echo "Python virtual environment not found: ${PYTHON_BIN}" >&2
  echo "Create it and install requirements-dev.txt first." >&2
  exit 1
fi

"${PYTHON_BIN}" -m grpc_tools.protoc \
  -I "${PROTO_DIR}" \
  --python_out="${PLUGIN_DIR}/generated" \
  --grpc_python_out="${PLUGIN_DIR}/generated" \
  "${PROTO_DIR}/plugin.proto"
