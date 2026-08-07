#!/usr/bin/env bash
# Execute one Server-assigned SWE AgentJob with the OpenHands version pinned by UEnv.
set -euo pipefail

MODE="${1:-llm}"
case "$MODE" in
  llm|gold) ;;
  *) echo "mode must be llm or gold" >&2; exit 2 ;;
esac

: "${UENV_AGENT_JOB_FILE:?UENV_AGENT_JOB_FILE is required}"
: "${OPENHANDS_OUT_DIR:?OPENHANDS_OUT_DIR is required}"

OPENHANDS_DIR="${OPENHANDS_BENCHMARKS_DIR:-/opt/uenv/agent/openhands-benchmarks}"
OPENHANDS_PYTHON="${OPENHANDS_PYTHON:-$OPENHANDS_DIR/.venv/bin/python}"
UENV_RELEASE="${UENV_RELEASE:-/opt/uenv/current}"
DRIVER="${UENV_OPENHANDS_DRIVER:-$UENV_RELEASE/share/swe/openhands/run_swebenchpro_official.py}"
LLM_TEMPLATE="${OPENHANDS_LLM_TEMPLATE:-/etc/uenv/openhands-llm.json}"

[[ -x "$OPENHANDS_PYTHON" ]] || {
  echo "OpenHands is not installed: $OPENHANDS_PYTHON" >&2
  exit 1
}
[[ -f "$DRIVER" ]] || { echo "UEnv OpenHands driver not found: $DRIVER" >&2; exit 1; }
[[ "$MODE" == gold || -r "$LLM_TEMPLATE" ]] || {
  echo "OpenHands LLM template is not readable: $LLM_TEMPLATE" >&2
  exit 1
}

mkdir -p "$OPENHANDS_OUT_DIR"
export OPENHANDS_BENCHMARKS_DIR="$OPENHANDS_DIR"
export UENV_ROLLOUT_TRACE=required
export UENV_REQUIRE_SWE_RESPONSE_TRACE=1

ARGS=(
  --agent-job-file "$UENV_AGENT_JOB_FILE"
  --output-dir "$OPENHANDS_OUT_DIR"
  --mode "$MODE"
  --rollout-trace required
)
if [[ "$MODE" == llm ]]; then
  ARGS+=(--llm-config "$LLM_TEMPLATE")
fi

exec "$OPENHANDS_PYTHON" "$DRIVER" "${ARGS[@]}"
