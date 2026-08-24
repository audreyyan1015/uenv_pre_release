#!/usr/bin/env bash
# Real-time monitor for SWE-bench-Pro smoke/tune runs (7142 + 208.77).
# Usage: swe_pro_monitor.sh <OUTPUT_DIR> [poll_sec]
set -uo pipefail

OUTPUT_DIR="${1:?OUTPUT_DIR required}"
POLL_SEC="${2:-30}"
RESULTS="${OUTPUT_DIR}/uenv_results.jsonl"
LOG="${OUTPUT_DIR}/monitor.log"
STALL_SEC="${STALL_SEC:-1200}"  # 20 min no new result => warn

last_count=-1
last_change=$(date +%s)
start_ts=$(date +%s)

log() {
  local msg="[$(date '+%Y-%m-%d %H:%M:%S')] $*"
  echo "$msg" | tee -a "$LOG"
}

summarize_results() {
  python3 <<PY
import json, collections
from pathlib import Path
p = Path("$RESULTS")
if not p.exists():
    print("results=0")
    raise SystemExit(0)
rows = [json.loads(l) for l in p.read_text().splitlines() if l.strip()]
c = collections.Counter(r.get('uenv_status') for r in rows)
resolved = sum(1 for r in rows if r.get('resolved') is True)
diff_gt0 = sum(1 for r in rows if int(r.get('git_diff_bytes') or 0) > 0)
ctx = sum(1 for r in rows if 'ContextWindow' in str(r.get('uenv_error_message','')))
print(f"results={len(rows)} completed={c.get('completed',0)} failed={c.get('failed',0)} resolved={resolved} diff_gt0={diff_gt0} ctx_fail={ctx}")
if rows:
    r = rows[-1]
    inst = (r.get('instance_id') or '')[:48]
    print(f"last={inst} status={r.get('uenv_status')} diff={r.get('git_diff_bytes')} resolved={r.get('resolved')}")
PY
}

poll_20877() {
  if ! command -v sshpass >/dev/null 2>&1; then
    echo "208.77=sshpass_missing"
    return
  fi
  sshpass -p 'dev@BDW2026' ssh -o StrictHostKeyChecking=no -o ConnectTimeout=8 root@8.130.208.77 bash <<'OH' 2>/dev/null || echo "208.77=unreachable"
RUNS=/var/log/uenv/openhands-runs
d=$(ls -td "$RUNS"/agent-job-* 2>/dev/null | head -1)
if [ -z "$d" ]; then echo "208.77=no_runs"; exit 0; fi
base=$(basename "$d")
if [ -f "$d/tool_patch_status.json" ]; then
  ok=$(python3 -c "import json; print(json.load(open('$d/tool_patch_status.json')).get('patch_ok'))" 2>/dev/null)
else ok=na
fi
if [ -f "$d/submit_result.json" ]; then
  sub=$(python3 -c "import json; d=json.load(open('$d/submit_result.json')); print('resolved='+str(d.get('resolved'))+' tests='+str(d.get('tests_passed'))+'/'+str(d.get('tests_total')))" 2>/dev/null)
else sub=RUNNING
fi
ol=0
[ -f "$d/runner_stdout.log" ] && ol=$(grep -ci openlibrary "$d/runner_stdout.log" 2>/dev/null || echo 0)
echo "208.77 run=${base:0:55} patch_ok=$ok $sub openlibrary=$ol"
journalctl -u uenv-agent-poller -n 1 --no-pager 2>/dev/null | sed 's/^/  poller: /'
OH
}

log "monitor start OUTPUT_DIR=$OUTPUT_DIR poll=${POLL_SEC}s stall_warn=${STALL_SEC}s"

while true; do
  elapsed=$(( $(date +%s) - start_ts ))
  if [ -f "$RESULTS" ]; then
    count=$(wc -l < "$RESULTS" | tr -d ' ')
  else
    count=0
  fi

  if [ "$count" != "$last_count" ]; then
    last_count=$count
    last_change=$(date +%s)
    log "PROGRESS elapsed=${elapsed}s $(summarize_results)"
  else
    idle=$(( $(date +%s) - last_change ))
    log "HEARTBEAT elapsed=${elapsed}s idle=${idle}s $(summarize_results)"
    if [ "$idle" -ge "$STALL_SEC" ]; then
      log "WARN stall ${idle}s — check evaluate podman / 208.77 poller / gateway"
    fi
  fi

  poll_20877 | while read -r line; do log "$line"; done

  if [ -f "${OUTPUT_DIR}/.monitor_stop" ]; then
    log "monitor stop flag seen"
    break
  fi
  sleep "$POLL_SEC"
done
