#!/usr/bin/env bash
set -euo pipefail

PLUGIN_BIN="${UENV_CODE_PLUGIN_BIN:-}"
if [[ -z "${PLUGIN_BIN}" ]]; then
  echo "UENV_CODE_PLUGIN_BIN is required" >&2
  exit 1
fi

exec "${PLUGIN_BIN}" "$@"
