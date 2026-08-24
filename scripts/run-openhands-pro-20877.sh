#!/usr/bin/env bash
# Run OpenHands official Pro/Smith eval on 阿里云 8C32G (8.130.208.77).
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
# 手动评测默认尽力采集但不因 trace 缺失而失败；训练 poller 会显式设为 required。
ROLLOUT_TRACE="${UENV_ROLLOUT_TRACE:-best-effort}"

# Server 编排：从 AgentJob 读取 variant，避免 .env 默认 pro / smoke fixture 抢占 Smith。
if [[ -n "${UENV_AGENT_JOB_FILE:-}" && -f "${UENV_AGENT_JOB_FILE}" ]]; then
  JOB_VARIANT="$(python3 - "$UENV_AGENT_JOB_FILE" <<'PY'
import json, sys
d = json.load(open(sys.argv[1], encoding="utf-8"))
print((d.get("benchmark_variant") or d.get("benchmarkVariant") or "").strip())
PY
)"
  if [[ -n "$JOB_VARIANT" ]]; then
    UENV_BENCHMARK_VARIANT="$JOB_VARIANT"
  fi
  JOB_LLM="$(python3 - "$UENV_AGENT_JOB_FILE" <<'PY'
import json, sys
d = json.load(open(sys.argv[1], encoding="utf-8"))
print((d.get("llm_config_path") or d.get("llmConfigPath") or "").strip())
PY
)"
  if [[ -n "$JOB_LLM" && -f "$JOB_LLM" ]]; then
    LLM_JSON="$JOB_LLM"
  fi
fi

VARIANT="${UENV_BENCHMARK_VARIANT:-pro}"
if [[ "$VARIANT" == "smith" || "$VARIANT" == "swe-smith" || "$VARIANT" == "swesmith" || "$VARIANT" == "swe-bench-smith" ]]; then
  VARIANT="smith"
  DEFAULT_INSTANCE="oauthlib__oauthlib.1fd52536.combine_file__0fceycuu"
  # AgentJob 路径不要绑死 smoke fixture；留给 driver 走 Gateway 单样本拉取。
  if [[ -n "${UENV_AGENT_JOB_FILE:-}" ]]; then
    DEFAULT_INSTANCES=""
  else
    DEFAULT_INSTANCES="$UENV/fixtures/swe/smith_catalog.json"
    [[ -f "$DEFAULT_INSTANCES" ]] || DEFAULT_INSTANCES="$UENV/config/swe/smith-sample-catalog.json"
  fi
else
  DEFAULT_INSTANCE="instance_qutebrowser__qutebrowser-f91ace96223cac8161c16dd061907e138fe85111-v059c6fdc75567943479b23ebca7c07b5e9a7f34c"
  DEFAULT_INSTANCES="$UENV/config/swe/pro-python-smoke.json"
fi
INSTANCE="${UENV_PRO_INSTANCE:-$DEFAULT_INSTANCE}"
STAMP="$(date +%Y%m%d-%H%M%S)"
RUN_ID="${UENV_RUN_ID:-run-oh-${STAMP}-${VARIANT}-${MODE}}"
# 输出目录：poller 通过 OPENHANDS_OUT_DIR 指定可预测路径以读取 submit_result.json；
# 未指定时沿用原带时间戳的默认目录。
OUT="${OPENHANDS_OUT_DIR:-${OPENHANDS_RUNS_DIR:-/var/log/uenv/openhands-runs}/${VARIANT}-official-${MODE}-${STAMP}}"

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
if [[ "$VARIANT" == "smith" && -f "$BRIDGE_DIR/run_swesmith_official.py" && -z "${UENV_AGENT_JOB_FILE:-}" ]]; then
  DRIVER="$BRIDGE_DIR/run_swesmith_official.py"
fi
# AgentJob + smith：显式清空 Pro 默认 catalog；driver 会 Gateway 拉取单样本。
if [[ "$VARIANT" == "smith" && -n "${UENV_AGENT_JOB_FILE:-}" ]]; then
  case "${UENV_SWE_INSTANCES:-}" in
    *smith*) INSTANCES="$UENV_SWE_INSTANCES" ;;
    *)
      INSTANCES=""
      unset UENV_SWE_INSTANCES || true
      ;;
  esac
elif [[ "$VARIANT" == "smith" ]]; then
  case "${UENV_SWE_INSTANCES:-}" in
    *smith*) INSTANCES="$UENV_SWE_INSTANCES" ;;
    *) INSTANCES="${DEFAULT_INSTANCES}" ;;
  esac
else
  INSTANCES="${UENV_SWE_INSTANCES:-$DEFAULT_INSTANCES}"
fi

cd "$SDK"

# 组装 driver 参数：Server 编排模式下 gateway 由 AgentJob 注入，仅在显式指定
# UENV_GATEWAY 时才传 --gateway；UENV_AGENT_JOB_FILE 存在时显式传 --agent-job-file。
# AgentJob 会覆盖 --benchmark-variant / --instance 等字段。
DRIVER_ARGS=(
  --llm-config "$LLM_JSON"
  --api-key "$API_KEY"
  --run-id "$RUN_ID"
  --instance "$INSTANCE"
  --benchmark-variant "$VARIANT"
  --mode "$MODE"
  --rollout-trace "$ROLLOUT_TRACE"
  --max-iterations "${MAX_ITERATIONS:-30}"
  --output-dir "$OUT"
)
[[ -n "$INSTANCES" ]] && DRIVER_ARGS+=(--instances "$INSTANCES")
[[ -n "$GATEWAY" ]] && DRIVER_ARGS+=(--gateway "$GATEWAY")
[[ -n "${UENV_AGENT_JOB_FILE:-}" ]] && DRIVER_ARGS+=(--agent-job-file "$UENV_AGENT_JOB_FILE")

exec uv run python "$DRIVER" "${DRIVER_ARGS[@]}"
