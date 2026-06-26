#!/usr/bin/env bash
# Run OpenHands official Pro eval on 7142 (requires deploy-openhands assets on 7142).
set -euo pipefail

MODE="${1:-llm}"
SDK="${OPENHANDS_SDK_DIR:-/opt/openhands/benchmarks/vendor/software-agent-sdk}"
BENCH="${OPENHANDS_BENCHMARKS_DIR:-/opt/openhands/benchmarks}"
UENV="${UENV_REPO:-/root/UEnv}"
GATEWAY="${UENV_GATEWAY:-http://10.10.20.143:28999}"
API_KEY="${UENV_GATEWAY_API_KEY:-swe-pro-secret}"
INSTANCE="${UENV_PRO_INSTANCE:-instance_qutebrowser__qutebrowser-f91ace96223cac8161c16dd061907e138fe85111-v059c6fdc75567943479b23ebca7c07b5e9a7f34c}"
STAMP="$(date +%Y%m%d-%H%M%S)"
OUT="${OPENHANDS_RUNS_DIR:-/var/log/uenv/openhands-runs}/pro-official-${MODE}-${STAMP}"

export PATH="$HOME/.local/bin:$PATH"
python3 "$UENV/scripts/gen-openhands-llm-config.py" \
  "$UENV/config/uenv-worker-llm.env" \
  "$UENV/config/openhands-llm-7142.json"

mkdir -p "$OUT"
export OPENHANDS_BENCHMARKS_DIR="$BENCH"
export UENV_REPO="$UENV"

cd "$SDK"
exec uv run python "$UENV/integrations/openhands/run_swebenchpro_official.py" \
  --llm-config "$UENV/config/openhands-llm-7142.json" \
  --gateway "$GATEWAY" \
  --api-key "$API_KEY" \
  --instance "$INSTANCE" \
  --instances "$UENV/config/swe/pro-python-smoke.json" \
  --benchmark-variant pro \
  --mode "$MODE" \
  --max-iterations "${MAX_ITERATIONS:-30}" \
  --output-dir "$OUT"
