#!/usr/bin/env bash
set -euo pipefail

if [[ -z "${UENV_GSM8K_PLUGIN_BIN:-}" ]]; then
  echo "UENV_GSM8K_PLUGIN_BIN is required" >&2
  exit 1
fi

exec "${UENV_GSM8K_PLUGIN_BIN}" "$@"
