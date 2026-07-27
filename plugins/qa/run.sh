#!/usr/bin/env bash
set -euo pipefail

# QaEnv reuses the (env_type-agnostic) math plugin binary; scoring is routed by
# `dataset`, not by env_type. Prefer a dedicated var, fall back to the math one
# so existing deployments work without new env vars.
PLUGIN_BIN="${UENV_QA_PLUGIN_BIN:-${UENV_MATH_PLUGIN_BIN:-}}"
if [[ -z "${PLUGIN_BIN}" ]]; then
  echo "UENV_QA_PLUGIN_BIN or UENV_MATH_PLUGIN_BIN is required" >&2
  exit 1
fi

exec "${PLUGIN_BIN}" "$@"
