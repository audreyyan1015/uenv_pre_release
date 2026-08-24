#!/usr/bin/env bash
# Small-batch LLM verification after local-ws isolation (7142 / ronghao).
set -euo pipefail

REPO_DIR="${REPO_DIR:-/data/ronghao/uenv/uenv-bridge}"
BASE_ROOT="${BASE_ROOT:-${REPO_DIR}/temp/benchmarks/swebenchpro/phase2b_$(date +%Y%m%d_%H%M%S)}"
UENV_ROLLOUT_MODEL_ENDPOINT="${UENV_ROLLOUT_MODEL_ENDPOINT:-http://10.10.20.142:18097/v1}"
LLM_TEMP1="/root/UEnv/config/openhands-llm-qwen3-temp1-topp095.json"
LIMIT="${LIMIT:-10}"

mkdir -p "$BASE_ROOT"
ORCH_LOG="${BASE_ROOT}/orchestrator.log"
log() { echo "[$(date '+%Y-%m-%d %H:%M:%S')] $*" | tee -a "$ORCH_LOG"; }

wait_agent_idle() {
  local max_wait="${1:-600}" elapsed=0
  while [ "$elapsed" -lt "$max_wait" ]; do
    local state
    state=$(sshpass -p 'dev@BDW2026' ssh -o StrictHostKeyChecking=no -o ConnectTimeout=8 root@8.130.208.77 bash <<'OH' 2>/dev/null || echo idle
d=$(ls -td /var/log/uenv/openhands-runs/agent-job-* 2>/dev/null | head -1)
if [ -z "$d" ] || [ -f "$d/submit_result.json" ]; then echo idle; else echo busy; fi
OH
)
    [ "$state" = "idle" ] && { log "agent idle (${elapsed}s)"; return 0; }
    log "agent busy (${elapsed}s)"
    sleep 15
    elapsed=$((elapsed + 15))
  done
  log "WARN wait_agent_idle timeout"
}

summarize() {
  python3 <<PY
import json, collections
from pathlib import Path
p = Path("$1/uenv_results.jsonl")
if not p.exists():
    print("no_results"); raise SystemExit(0)
rows = [json.loads(l) for l in p.read_text().splitlines() if l.strip()]
c = collections.Counter(r.get("uenv_status") for r in rows)
resolved = sum(1 for r in rows if r.get("resolved") is True)
diff = sum(1 for r in rows if int(r.get("git_diff_bytes") or 0) > 0)
ctx = sum(1 for r in rows if "ContextWindow" in str(r.get("uenv_error_message", "")))
print(f"n={len(rows)} completed={c.get('completed',0)} failed={c.get('failed',0)} resolved={resolved} diff_gt0={diff} ctx_fail={ctx}")
for r in rows:
    if r.get("resolved"):
        print("RESOLVED", (r.get("instance_id") or "")[:60])
PY
}

OUT="${BASE_ROOT}/exp_limit${LIMIT}_iter100_temp1_localws"
mkdir -p "$OUT"
log "phase2b BASE_ROOT=$BASE_ROOT LIMIT=$LIMIT"
wait_agent_idle 300

(
  cd "$REPO_DIR"
  env REPO_DIR="$REPO_DIR" \
    DATA_PATH="${REPO_DIR}/data/benchmarks/swebenchpro/test.jsonl" \
    OUTPUT_DIR="$OUT" \
    UENV_ADAPTER_CORE_ENDPOINT=8.130.75.157:8088 \
    UENV_ROLLOUT_MODEL_ENDPOINT="$UENV_ROLLOUT_MODEL_ENDPOINT" \
    UENV_ROLLOUT_MODEL_NAME=Qwen/Qwen3.6-35B-A3B \
    BATCH_SIZE=1 RESUME=0 \
    LIMIT="$LIMIT" \
    AGENT_MODE=llm \
    MAX_ITERATIONS=100 \
    MAX_TOKENS=2048 THINKING_TOKEN_BUDGET=1024 \
    TEMPERATURE=1.0 TOP_P=0.95 \
    TIMEOUT_SECONDS=7200 CLIENT_TIMEOUT_SECONDS=7600 \
    BENCHMARK_VARIANT=pro COMMAND_MODE=full_shell \
    ENV_PACKAGE_ID=swe-bench-pro ENV_PACKAGE_VERSION=0.3.4 \
    AGENT_BRIDGE_ID=uenv-agent-openhands AGENT_BRIDGE_VERSION=1.0.0 \
    AGENT_POOL_ID=openhands-default \
    DRIVER_ENTRYPOINT=run_swebenchpro_official.py WORKSPACE_DIR=/app \
    LLM_CONFIG_PATH="$LLM_TEMP1" \
    bash ./scripts/benchmark/run_swebenchpro_uenv_baseline.sh
) >> "${OUT}/run.log" 2>&1 &
EVAL_PID=$!
echo "$EVAL_PID" > "${OUT}/eval.pid"
bash "${REPO_DIR}/scripts/benchmark/swe_pro_monitor.sh" "$OUT" 30 &
MON_PID=$!
echo "$MON_PID" > "${OUT}/monitor.pid"
log "started eval_pid=$EVAL_PID mon_pid=$MON_PID out=$OUT"
wait "$EVAL_PID" || true
touch "${OUT}/.monitor_stop"
wait "$MON_PID" 2>/dev/null || true
log "DONE $(summarize "$OUT")"
summarize "$OUT" | tee -a "$ORCH_LOG"
log "phase2b finished BASE_ROOT=$BASE_ROOT"
