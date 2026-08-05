#!/usr/bin/env bash
set -euo pipefail

PLUGIN_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RELEASE_BIN="$(cd "${PLUGIN_DIR}/../.." && pwd)/bin/uenv-code-plugin"
PLUGIN_BIN="${UENV_CODE_PLUGIN_BIN:-}"
if [[ -z "${PLUGIN_BIN}" && -x "${RELEASE_BIN}" ]]; then
  PLUGIN_BIN="${RELEASE_BIN}"
fi
if [[ -z "${PLUGIN_BIN}" ]]; then
  echo "UENV_CODE_PLUGIN_BIN is required" >&2
  exit 1
fi

# Keep release upgrades usable even when an existing /etc/uenv/worker.env is
# preserved by the installer and predates these variables.
export UENV_CODE_EVAL_SCRIPT="${UENV_CODE_EVAL_SCRIPT:-${PLUGIN_DIR}/scripts/evaluate_code.py}"
export UENV_CODE_PYTHON="${UENV_CODE_PYTHON:-python3}"

exec "${PLUGIN_BIN}" "$@"
