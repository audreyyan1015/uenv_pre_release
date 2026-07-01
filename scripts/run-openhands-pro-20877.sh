#!/usr/bin/env bash
# Run OpenHands official Pro eval on 阿里云 8C32G (8.130.208.77).
set -euo pipefail

[[ -f /root/.openhands-20877.env ]] && source /root/.openhands-20877.env

MODE="${1:-llm}"
SDK="${OPENHANDS_SDK_DIR:-/opt/openhands/benchmarks/vendor/software-agent-sdk}"
BENCH="${OPENHANDS_BENCHMARKS_DIR:-/opt/openhands/benchmarks}"
UENV="${UENV_REPO:-/root/UEnv}"
GATEWAY="${UENV_GATEWAY:-http://219.147.100.43:28097}"
API_KEY="${UENV_GATEWAY_API_KEY:-swe-pro-secret}"
LLM_JSON="${OPENHANDS_LLM_CONFIG:-$UENV/config/openhands-llm-20877.json}"
INSTANCE="${UENV_PRO_INSTANCE:-instance_qutebrowser__qutebrowser-f91ace96223cac8161c16dd061907e138fe85111-v059c6fdc75567943479b23ebca7c07b5e9a7f34c}"
STAMP="$(date +%Y%m%d-%H%M%S)"
RUN_ID="${UENV_RUN_ID:-run-oh-${STAMP}-pro-${MODE}}"
OUT="${OPENHANDS_RUNS_DIR:-/var/log/uenv/openhands-runs}/pro-official-${MODE}-${STAMP}"

export PATH="$HOME/.local/bin:$PATH"
source /root/.uenv-trajectory.env 2>/dev/null || true

if [[ -f "$UENV/config/uenv-worker-llm.env" && ! -f "$LLM_JSON" ]]; then
  python3 "$UENV/scripts/gen-openhands-llm-config.py" \
    "$UENV/config/uenv-worker-llm.env" \
    "$LLM_JSON"
fi

mkdir -p "$OUT"
export OPENHANDS_BENCHMARKS_DIR="$BENCH"
export UENV_REPO="$UENV"

cd "$SDK"
exec uv run python "$UENV/integrations/openhands/run_swebenchpro_official.py" \
  --llm-config "$LLM_JSON" \
  --gateway "$GATEWAY" \
  --api-key "$API_KEY" \
  --run-id "$RUN_ID" \
  --instance "$INSTANCE" \
  --instances "$UENV/config/swe/pro-python-smoke.json" \
  --benchmark-variant pro \
  --mode "$MODE" \
  --max-iterations "${MAX_ITERATIONS:-30}" \
  --output-dir "$OUT"
