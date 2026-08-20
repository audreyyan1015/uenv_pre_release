#!/usr/bin/env bash
# Phase-2: gold grader check → E2/E4 until resolved>0 (7142, ronghao).
set -euo pipefail

REPO_DIR="${REPO_DIR:-/data/ronghao/uenv/uenv-bridge}"
BASE_ROOT="${BASE_ROOT:-${REPO_DIR}/temp/benchmarks/swebenchpro/phase2_$(date +%Y%m%d_%H%M%S)}"
UENV_ROLLOUT_MODEL_ENDPOINT="${UENV_ROLLOUT_MODEL_ENDPOINT:-http://10.10.20.142:18097/v1}"
GATEWAY_PORT="${GATEWAY_PORT:-18097}"

S1="instance_qutebrowser__qutebrowser-f91ace96223cac8161c16dd061907e138fe85111-v059c6fdc75567943479b23ebca7c07b5e9a7f34c"
S2="instance_flipt-io__flipt-c12967bc73fdf02054cf3ef8498c05e25f0a18c0"

# LLM configs on 208.77 (absolute paths used by AgentJob)
LLM_BASE="/root/UEnv/config/openhands-llm-qwen3-thinking-max-token-8192.json"
LLM_TEMP1="/root/UEnv/config/openhands-llm-qwen3-temp1-topp095.json"

mkdir -p "$BASE_ROOT"
ORCH_LOG="${BASE_ROOT}/orchestrator.log"

log() { echo "[$(date '+%Y-%m-%d %H:%M:%S')] $*" | tee -a "$ORCH_LOG"; }

wait_agent_idle() {
  local max_wait="${1:-600}" elapsed=0
  log "wait_agent_idle max=${max_wait}s"
  while [ "$elapsed" -lt "$max_wait" ]; do
    local state
    state=$(sshpass -p "${UENV_PASS:?set UENV_PASS}" ssh -o StrictHostKeyChecking=no -o ConnectTimeout=8 root@8.130.208.77 bash <<'OH' 2>/dev/null || echo idle
d=$(ls -td /var/log/uenv/openhands-runs/agent-job-* 2>/dev/null | head -1)
if [ -z "$d" ]; then echo idle; exit 0; fi
if [ -f "$d/submit_result.json" ]; then echo idle; else echo busy; fi
OH
)
    [ "$state" = "idle" ] && { log "agent idle (${elapsed}s)"; sleep 3; return 0; }
    log "agent busy (${elapsed}s)"
    sleep 15
    elapsed=$((elapsed + 15))
  done
  log "WARN wait_agent_idle timeout — proceeding"
}

summarize_dir() {
  python3 <<PY
import json, collections
from pathlib import Path
p = Path("$1/uenv_results.jsonl")
if not p.exists():
    print("no_results"); raise SystemExit(0)
rows = [json.loads(l) for l in p.read_text().splitlines() if l.strip()]
c = collections.Counter(r.get('uenv_status') for r in rows)
resolved = sum(1 for r in rows if r.get('resolved') is True)
diff_gt0 = sum(1 for r in rows if int(r.get('git_diff_bytes') or 0) > 0)
ctx = sum(1 for r in rows if 'ContextWindow' in str(r.get('uenv_error_message','')))
print(f"n={len(rows)} completed={c.get('completed',0)} failed={c.get('failed',0)} resolved={resolved} diff_gt0={diff_gt0} ctx_fail={ctx}")
PY
}

resolved_count() {
  python3 -c "import json; from pathlib import Path; p=Path('$1/uenv_results.jsonl');
print(sum(1 for l in p.read_text().splitlines() if l.strip() and json.loads(l).get('resolved')) if p.exists() else 0)" 2>/dev/null || echo 0
}

run_eval() {
  local out_dir="$1"; shift
  mkdir -p "$out_dir"
  rm -f "${out_dir}/.monitor_stop"
  wait_agent_idle 600
  log "=== START eval output=$out_dir extra=$* ==="
  (
    cd "$REPO_DIR"
    env REPO_DIR="$REPO_DIR" \
      DATA_PATH="${REPO_DIR}/data/benchmarks/swebenchpro/test.jsonl" \
      OUTPUT_DIR="$out_dir" \
      UENV_ADAPTER_CORE_ENDPOINT=8.130.75.157:8088 \
      UENV_ROLLOUT_MODEL_ENDPOINT="$UENV_ROLLOUT_MODEL_ENDPOINT" \
      UENV_ROLLOUT_MODEL_NAME=Qwen/Qwen3.6-35B-A3B \
      BATCH_SIZE=1 RESUME=0 \
      TIMEOUT_SECONDS=7200 CLIENT_TIMEOUT_SECONDS=7600 \
      BENCHMARK_VARIANT=pro COMMAND_MODE=full_shell \
      ENV_PACKAGE_ID=swe-bench-pro ENV_PACKAGE_VERSION=0.3.4 \
      AGENT_BRIDGE_ID=uenv-agent-openhands AGENT_BRIDGE_VERSION=1.0.0 \
      AGENT_POOL_ID=openhands-default \
      DRIVER_ENTRYPOINT=run_swebenchpro_official.py WORKSPACE_DIR=/app \
      "$@" \
      bash ./scripts/benchmark/run_swebenchpro_uenv_baseline.sh
  ) >> "${out_dir}/run.log" 2>&1 &
  local eval_pid=$!
  echo "$eval_pid" > "${out_dir}/eval.pid"
  bash "${REPO_DIR}/scripts/benchmark/swe_pro_monitor.sh" "$out_dir" 30 &
  local mon_pid=$!
  echo "$mon_pid" > "${out_dir}/monitor.pid"
  wait "$eval_pid" || true
  touch "${out_dir}/.monitor_stop"
  wait "$mon_pid" 2>/dev/null || true
  log "=== DONE eval output=$out_dir $(summarize_dir "$out_dir") ==="
}

ensure_gateway() {
  if curl -sf "http://127.0.0.1:${GATEWAY_PORT}/v1/models" >/dev/null 2>&1; then
    log "gateway :${GATEWAY_PORT} up"
    return 0
  fi
  log "ERROR gateway :${GATEWAY_PORT} down"
  return 1
}

log "phase2 BASE_ROOT=$BASE_ROOT"
ensure_gateway

# ── Gold grader smoke (S1) ──
OUT_GOLD="${BASE_ROOT}/gold_S1"
run_eval "$OUT_GOLD" \
  INSTANCE_ID="$S1" LIMIT=1 \
  AGENT_MODE=gold \
  MAX_ITERATIONS=5 \
  MAX_TOKENS=8192 THINKING_TOKEN_BUDGET=4096 \
  TEMPERATURE=0.0 TOP_P=1.0 \
  LLM_CONFIG_PATH="$LLM_BASE"

GOLD_R=$(resolved_count "$OUT_GOLD")
log "GOLD resolved_count=$GOLD_R"
if [ "${GOLD_R:-0}" -lt 1 ]; then
  log "WARN gold did not resolve — grader/apply path suspect; continuing E2/E4 anyway"
fi

# ── E2+E4: iter=100 + temp=1.0 + top_p=0.95 (LIMIT=10) ──
OUT_E24="${BASE_ROOT}/exp_e2e4_limit10_iter100_temp1"
run_eval "$OUT_E24" \
  LIMIT=10 \
  AGENT_MODE=llm \
  MAX_ITERATIONS=100 \
  MAX_TOKENS=4096 THINKING_TOKEN_BUDGET=2048 \
  TEMPERATURE=1.0 TOP_P=0.95 \
  LLM_CONFIG_PATH="$LLM_TEMP1"

E24_R=$(resolved_count "$OUT_E24")
DIFF=$(python3 -c "import json; from pathlib import Path; p=Path('$OUT_E24/uenv_results.jsonl');
print(sum(1 for l in p.read_text().splitlines() if l.strip() and int(json.loads(l).get('git_diff_bytes') or 0)>0) if p.exists() else 0)" 2>/dev/null || echo 0)
log "E2E4 resolved=$E24_R diff_gt0=$DIFF"

# ── If still 0: E2-only longer on S1+S2 with temp1 ──
if [ "${E24_R:-0}" -lt 1 ]; then
  log "E2E4 no resolve — retry S1+S2 with iter=100 temp=1 (focused)"
  OUT_FOCUS="${BASE_ROOT}/exp_focus_S1S2_iter100_temp1"
  # run S1 then S2 via two LIMIT=1
  for pair in "S1:${S1}" "S2:${S2}"; do
    tag="${pair%%:*}"; inst="${pair#*:}"
    run_eval "${OUT_FOCUS}_${tag}" \
      INSTANCE_ID="$inst" LIMIT=1 \
      AGENT_MODE=llm \
      MAX_ITERATIONS=100 \
      MAX_TOKENS=4096 THINKING_TOKEN_BUDGET=2048 \
      TEMPERATURE=1.0 TOP_P=0.95 \
      LLM_CONFIG_PATH="$LLM_TEMP1"
  done
fi

log "=== FINAL SUMMARY ==="
python3 <<PY | tee -a "$ORCH_LOG"
import json, pathlib
root = pathlib.Path("$BASE_ROOT")
print("=== FINAL SUMMARY ===")
total_resolved = 0
for d in sorted(root.glob("*/uenv_results.jsonl")):
    rows = [json.loads(l) for l in d.read_text().splitlines() if l.strip()]
    resolved = sum(1 for r in rows if r.get('resolved'))
    diff = sum(1 for r in rows if int(r.get('git_diff_bytes') or 0)>0)
    ctx = sum(1 for r in rows if 'ContextWindow' in str(r.get('uenv_error_message','')))
    total_resolved += resolved
    print(f"{d.parent.name}: n={len(rows)} resolved={resolved} diff_gt0={diff} ctx_fail={ctx}")
print(f"TOTAL_RESOLVED={total_resolved}")
PY

log "phase2 finished BASE_ROOT=$BASE_ROOT"
