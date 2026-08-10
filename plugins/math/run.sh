#!/usr/bin/env bash
set -euo pipefail

PLUGIN_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RELEASE_BIN="$(cd "${PLUGIN_DIR}/../.." && pwd)/bin/uenv-math-plugin"
PLUGIN_BIN="${UENV_MATH_PLUGIN_BIN:-}"
if [[ -z "${PLUGIN_BIN}" && -x "${RELEASE_BIN}" ]]; then
  PLUGIN_BIN="${RELEASE_BIN}"
fi
if [[ -z "${PLUGIN_BIN}" ]]; then
  echo "UENV_MATH_PLUGIN_BIN is required" >&2
  exit 1
fi

exec "${PLUGIN_BIN}" "$@"
