#!/usr/bin/env bash
set -euo pipefail

PLUGIN_BIN="${UENV_MATH_PLUGIN_BIN:-}"
if [[ -z "${PLUGIN_BIN}" ]]; then
  echo "UENV_MATH_PLUGIN_BIN is required" >&2
  exit 1
fi

exec "${PLUGIN_BIN}" "$@"
