#!/usr/bin/env bash
# Generated transport launcher. Put environment behavior in environment.py;
# this file normally does not need edits.
set -euo pipefail

PLUGIN_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PYTHON_BIN="${UENV_PLUGIN_PYTHON:-}"
if [[ -z "${PYTHON_BIN}" && -x "${PLUGIN_DIR}/.venv/bin/python" ]]; then
  PYTHON_BIN="${PLUGIN_DIR}/.venv/bin/python"
fi
PYTHON_BIN="${PYTHON_BIN:-python3}"

exec "${PYTHON_BIN}" "${PLUGIN_DIR}/plugin.py" "$@"
