#!/usr/bin/env bash
set -euo pipefail

mode="${1:-gold}"
case "$mode" in
  gold|llm) ;;
  *) echo "unsupported OpenHands mode: $mode" >&2; exit 2 ;;
esac

: "${UENV_AGENT_JOB_FILE:?UENV_AGENT_JOB_FILE is required}"
: "${OPENHANDS_OUT_DIR:?OPENHANDS_OUT_DIR is required}"
: "${OPENHANDS_SDK_DIR:?OPENHANDS_SDK_DIR is required}"
: "${OPENHANDS_BENCHMARKS_DIR:?OPENHANDS_BENCHMARKS_DIR is required}"
: "${UENV_REPO:?UENV_REPO is required}"
: "${UENV_SWE_INSTANCES:?UENV_SWE_INSTANCES is required}"

driver="${UENV_REPO}/integrations/openhands/run_swebenchpro_official.py"
python="${OPENHANDS_PYTHON:-${OPENHANDS_BENCHMARKS_DIR}/.venv/bin/python}"
test -f "$driver"
test -f "$UENV_AGENT_JOB_FILE"
test -f "$UENV_SWE_INSTANCES"
test -d "$OPENHANDS_SDK_DIR"
test -d "$OPENHANDS_BENCHMARKS_DIR"
test -x "$python"
command -v "${UENV_SWE_RUNTIME:-docker}" >/dev/null

if [[ "$mode" == "llm" ]]; then
  : "${OPENHANDS_LLM_CONFIG:?OPENHANDS_LLM_CONFIG is required for llm mode}"
  test -f "$OPENHANDS_LLM_CONFIG"
fi

mkdir -p "$OPENHANDS_OUT_DIR"
export PYTHONPATH="${UENV_REPO}/integrations/openhands:${PYTHONPATH:-}"

# A stress run must never inherit a production trajectory endpoint or fixed gateway.
unset UENV_TRAJECTORY_ENDPOINT UENV_TRAJECTORY_TOKEN UENV_GATEWAY UENV_GATEWAY_LOCAL

args=(
  --instances "$UENV_SWE_INSTANCES"
  --output-dir "$OPENHANDS_OUT_DIR"
  --max-iterations "${MAX_ITERATIONS:-30}"
  --mode "$mode"
  --agent-job-file "$UENV_AGENT_JOB_FILE"
)
if [[ "$mode" == "llm" ]]; then
  args+=(--llm-config "$OPENHANDS_LLM_CONFIG")
fi

# The environment must be installed and frozen during host provisioning.  Running
# `uv run` from the nested SDK project with UV_PROJECT_ENVIRONMENT pointing at
# the benchmarks venv replaces that venv and downloads hundreds of packages in
# the hot path.  Stress execution is deliberately offline with respect to Python
# package resolution and fails fast when the provisioned environment is missing.
cd "$OPENHANDS_BENCHMARKS_DIR"
exec "$python" -u "$driver" "${args[@]}"
