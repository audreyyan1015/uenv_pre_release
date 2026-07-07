#!/usr/bin/env bash
# Run OpenHands official Pro eval on 阿里云 8C32G (8.130.208.77).
set -euo pipefail

[[ -f /root/.openhands-20877.env ]] && source /root/.openhands-20877.env

MODE="${1:-llm}"
SDK="${OPENHANDS_SDK_DIR:-/opt/openhands/benchmarks/vendor/software-agent-sdk}"
BENCH="${OPENHANDS_BENCHMARKS_DIR:-/opt/openhands/benchmarks}"
UENV="${UENV_REPO:-/root/UEnv}"
# Gateway：Server 编排模式下由 AgentJob（UENV_AGENT_JOB_FILE）注入，driver 会覆盖此值，
# 故此处默认留空；旁路/手动模式仍可用 UENV_GATEWAY 显式指定。driver 在两者皆空时报错。
GATEWAY="${UENV_GATEWAY:-}"
API_KEY="${UENV_GATEWAY_API_KEY:-swe-pro-secret}"
LLM_JSON="${OPENHANDS_LLM_CONFIG:-$UENV/config/openhands-llm-20877.json}"
INSTANCE="${UENV_PRO_INSTANCE:-instance_qutebrowser__qutebrowser-f91ace96223cac8161c16dd061907e138fe85111-v059c6fdc75567943479b23ebca7c07b5e9a7f34c}"
STAMP="$(date +%Y%m%d-%H%M%S)"
RUN_ID="${UENV_RUN_ID:-run-oh-${STAMP}-pro-${MODE}}"
# 输出目录：poller 通过 OPENHANDS_OUT_DIR 指定可预测路径以读取 submit_result.json；
# 未指定时沿用原带时间戳的默认目录。
OUT="${OPENHANDS_OUT_DIR:-${OPENHANDS_RUNS_DIR:-/var/log/uenv/openhands-runs}/pro-official-${MODE}-${STAMP}}"

export PATH="${HOME:-/root}/.local/bin:$PATH"
source /root/.uenv-trajectory.env 2>/dev/null || true

if [[ -f "$UENV/config/uenv-worker-llm.env" && ! -f "$LLM_JSON" ]]; then
  python3 "$UENV/scripts/gen-openhands-llm-config.py" \
    "$UENV/config/uenv-worker-llm.env" \
    "$LLM_JSON"
fi

mkdir -p "$OUT"
export OPENHANDS_BENCHMARKS_DIR="$BENCH"
export UENV_REPO="$UENV"
BRIDGE_DIR="${UENV_AGENT_BRIDGE_DIR:-$UENV/integrations/openhands}"
export PYTHONPATH="$BRIDGE_DIR:${PYTHONPATH:-}"
DRIVER="$BRIDGE_DIR/drivers/run_swebenchpro_official.py"
[[ -f "$DRIVER" ]] || DRIVER="$BRIDGE_DIR/run_swebenchpro_official.py"
INSTANCES="${UENV_SWE_INSTANCES:-$UENV/config/swe/pro-python-smoke.json}"

cd "$SDK"

# 组装 driver 参数：Server 编排模式下 gateway 由 AgentJob 注入，仅在显式指定
# UENV_GATEWAY 时才传 --gateway；UENV_AGENT_JOB_FILE 存在时显式传 --agent-job-file。
DRIVER_ARGS=(
  --llm-config "$LLM_JSON"
  --api-key "$API_KEY"
  --run-id "$RUN_ID"
  --instance "$INSTANCE"
  --instances "$INSTANCES"
  --benchmark-variant pro
  --mode "$MODE"
  --max-iterations "${MAX_ITERATIONS:-30}"
  --output-dir "$OUT"
)
[[ -n "$GATEWAY" ]] && DRIVER_ARGS+=(--gateway "$GATEWAY")
[[ -n "${UENV_AGENT_JOB_FILE:-}" ]] && DRIVER_ARGS+=(--agent-job-file "$UENV_AGENT_JOB_FILE")

exec uv run python "$DRIVER" "${DRIVER_ARGS[@]}"