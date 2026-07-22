#!/usr/bin/env bash
# SWE-bench-Pro smoke (S1/S2/S3) + phase-5 tune orchestrator (7142).
# Stops full run; runs controlled experiments until pass-rate improves.
set -euo pipefail

REPO_DIR="${REPO_DIR:-/data/ronghao/uenv/uenv-bridge}"
BASE_ROOT="${BASE_ROOT:-${REPO_DIR}/temp/benchmarks/swebenchpro/phase_tune_$(date +%Y%m%d_%H%M%S)}"
UENV_ROLLOUT_MODEL_ENDPOINT="${UENV_ROLLOUT_MODEL_ENDPOINT:-http://10.10.20.142:18097/v1}"
GATEWAY_PORT="${GATEWAY_PORT:-18097}"
VLLM_PORT="${VLLM_PORT:-18081}"
RUN_USER="${RUN_USER:-ronghao}"

# Smoke matrix (plan §5)
S1="instance_qutebrowser__qutebrowser-f91ace96223cac8161c16dd061907e138fe85111-v059c6fdc75567943479b23ebca7c07b5e9a7f34c"
S2="instance_flipt-io__flipt-c12967bc73fdf02054cf3ef8498c05e25f0a18c0"
S3="instance_NodeBB__NodeBB-04998908ba6721d64eba79ae3b65a351dcfbc5b5-vnan"

mkdir -p "$BASE_ROOT"
ORCH_LOG="${BASE_ROOT}/orchestrator.log"

log() {
  echo "[$(date '+%Y-%m-%d %H:%M:%S')] $*" | tee -a "$ORCH_LOG"
}

# Wait until 208.77 poller has no in-flight job (avoid pickup timeout 2003).
wait_agent_idle() {
  local max_wait="${1:-600}"
  local elapsed=0
  log "wait_agent_idle max=${max_wait}s"
  while [ "$elapsed" -lt "$max_wait" ]; do
    local state
    state=$(sshpass -p 'dev@BDW2026' ssh -o StrictHostKeyChecking=no -o ConnectTimeout=8 root@8.130.208.77 bash <<'OH' 2>/dev/null || echo unreachable
d=$(ls -td /var/log/uenv/openhands-runs/agent-job-* 2>/dev/null | head -1)
if [ -z "$d" ]; then echo idle; exit 0; fi
if [ -f "$d/submit_result.json" ]; then echo idle; else echo busy:$(basename "$d" | cut -c1-50); fi
OH
)
    if [ "$state" = "idle" ] || [ "$state" = "unreachable" ]; then
      log "agent idle (${elapsed}s)"
      sleep 5
      return 0
    fi
    log "agent busy: $state (${elapsed}s)"
    sleep 15
    elapsed=$((elapsed + 15))
  done
  log "WARN wait_agent_idle timeout ${max_wait}s — proceeding anyway"
}

run_eval() {
  local out_dir="$1"
  shift
  mkdir -p "$out_dir"
  rm -f "${out_dir}/.monitor_stop"

  wait_agent_idle 600
  log "=== START eval output=$out_dir extra_env=$* ==="
  (
    cd "$REPO_DIR"
    env REPO_DIR="$REPO_DIR" \
      DATA_PATH="${REPO_DIR}/data/benchmarks/swebenchpro/test.jsonl" \
      OUTPUT_DIR="$out_dir" \
      UENV_ADAPTER_CORE_ENDPOINT=8.130.75.157:8088 \
      UENV_ROLLOUT_MODEL_ENDPOINT="$UENV_ROLLOUT_MODEL_ENDPOINT" \
      UENV_ROLLOUT_MODEL_NAME=Qwen/Qwen3.6-35B-A3B \
      BATCH_SIZE=1 RESUME=0 \
      TEMPERATURE=0.0 TOP_P=1.0 \
      TIMEOUT_SECONDS=7200 CLIENT_TIMEOUT_SECONDS=7600 \
      BENCHMARK_VARIANT=pro COMMAND_MODE=full_shell \
      ENV_PACKAGE_ID=swe-bench-pro ENV_PACKAGE_VERSION=0.3.4 \
      AGENT_BRIDGE_ID=uenv-agent-openhands AGENT_BRIDGE_VERSION=1.0.0 \
      AGENT_POOL_ID=openhands-default \
      DRIVER_ENTRYPOINT=run_swebenchpro_official.py WORKSPACE_DIR=/app \
      LLM_CONFIG_PATH=/root/UEnv/config/openhands-llm-qwen3-thinking-max-token-8192.json \
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

verify_smoke_20877() {
  local tag="$1"
  local out_dir="$2"
  log "--- smoke verify $tag on 208.77 ---"
  sshpass -p 'dev@BDW2026' ssh -o StrictHostKeyChecking=no root@8.130.208.77 \
    "python3 -c \"import json,glob,pathlib; runs=sorted(pathlib.Path('/var/log/uenv/openhands-runs').glob('agent-job-*'), key=lambda p:p.stat().st_mtime, reverse=True); 
for d in runs[:15]:
  inst='$3'
  if inst not in d.name: continue
  patch=json.load(open(d/'tool_patch_status.json')) if (d/'tool_patch_status.json').exists() else {}
  probe=(d/'workspace_probe.json').exists()
  ol=open(d/'runner_stdout.log').read().lower().count('openlibrary') if (d/'runner_stdout.log').exists() else -1
  print('$tag', 'patch_ok', patch.get('patch_ok'), 'probe', probe, 'openlibrary', ol, d.name[:70]); break\"" \
    2>/dev/null | tee -a "$ORCH_LOG" || true
}

restart_vllm() {
  local max_len="$1"
  log "Restart vLLM max-model-len=$max_len (may take several minutes)..."
  podman rm -f uenv-swebenchpro-vllm-${VLLM_PORT} 2>/dev/null || true
  podman run -d --name uenv-swebenchpro-vllm-${VLLM_PORT} \
    --entrypoint python3 --network host --pids-limit=-1 --shm-size=64g \
    --device nvidia.com/gpu=all \
    -v /data/ronghao:/data/ronghao \
    -w /data/ronghao/uenv/uenv-bridge \
    localhost/vllm-openai:v0.19.0-cu130 \
    -m vllm.entrypoints.openai.api_server \
    --model /data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B \
    --served-model-name Qwen/Qwen3.6-35B-A3B \
    --host 0.0.0.0 --port "${VLLM_PORT}" \
    --tensor-parallel-size 8 \
    --max-model-len "$max_len" \
    --gpu-memory-utilization 0.90 \
    --reasoning-parser qwen3 \
    --reasoning-config '{"reasoning_start_str":"<think>","reasoning_end_str":"</think>"}' \
    --trust-remote-code

  for i in $(seq 1 60); do
    if curl -sf --noproxy '*' "http://127.0.0.1:${VLLM_PORT}/v1/models" >/dev/null 2>&1; then
      log "vLLM ready max_model_len=$max_len (${i}*30s)"
      return 0
    fi
    log "waiting vLLM... ${i}/60"
    sleep 30
  done
  log "ERROR vLLM failed to start"
  return 1
}

ensure_gateway() {
  local out_dir="$1"
  local budget="${2:-4096}"
  if curl -sf "http://127.0.0.1:${GATEWAY_PORT}/v1/models" >/dev/null 2>&1; then
    log "gateway :${GATEWAY_PORT} already up"
    return 0
  fi
  log "starting model gateway :${GATEWAY_PORT} thinking_budget=$budget"
  nohup env PYTHONPATH=src python3 scripts/benchmark/run_model_gateway.py \
    --upstream "http://127.0.0.1:${VLLM_PORT}/v1" \
    --bind-host 0.0.0.0 --port "${GATEWAY_PORT}" \
    --public-url "http://10.10.20.142:${GATEWAY_PORT}/v1" \
    --request-timeout-seconds 7200 \
    --enable-thinking --thinking-token-budget "$budget" --strip-reasoning \
    --log-path "${out_dir}/model-gateway.jsonl" \
    > "${out_dir}/model-gateway.log" 2>&1 &
  sleep 3
}

log "orchestrator BASE_ROOT=$BASE_ROOT"
ensure_gateway "$BASE_ROOT" 4096

# ── Phase: Smoke S1/S2/S3 ──
# S1 may have failed with pickup timeout if full-run job was still in flight; retry once at end.
SMOKE_RETRY_S1=0
for pair in "S1:${S1}" "S2:${S2}" "S3:${S3}"; do
  tag="${pair%%:*}"
  inst="${pair#*:}"
  out="${BASE_ROOT}/smoke_${tag}"
  run_eval "$out" \
    INSTANCE_ID="$inst" \
    LIMIT=1 \
    MAX_ITERATIONS=10 \
    MAX_TOKENS=8192 \
    THINKING_TOKEN_BUDGET=4096
  verify_smoke_20877 "$tag" "$out" "$inst"
  if [ "$tag" = "S1" ]; then
    SMOKE_RETRY_S1=$(python3 -c "import json; p='$out/uenv_results.jsonl';
rows=[json.loads(l) for l in open(p) if l.strip()] if __import__('pathlib').Path(p).exists() else [];
print(1 if rows and rows[0].get('uenv_error_code')==2003 else 0)" 2>/dev/null || echo 0)
  fi
done

if [ "${SMOKE_RETRY_S1:-0}" -eq 1 ]; then
  log "=== RETRY S1 (prior pickup timeout 2003) ==="
  out="${BASE_ROOT}/smoke_S1_retry"
  run_eval "$out" \
    INSTANCE_ID="$S1" \
    LIMIT=1 \
    MAX_ITERATIONS=10 \
    MAX_TOKENS=8192 \
    THINKING_TOKEN_BUDGET=4096
  verify_smoke_20877 "S1_retry" "$out" "$S1"
fi

# ── Phase 5: baseline 10 (control) ──
OUT_CTRL="${BASE_ROOT}/exp_control_limit10_iter30_t8192_ctx65536"
run_eval "$OUT_CTRL" \
  LIMIT=10 \
  MAX_ITERATIONS=30 \
  MAX_TOKENS=8192 \
  THINKING_TOKEN_BUDGET=4096

# ── Phase 5 E3: reduce output tokens ──
OUT_E3="${BASE_ROOT}/exp_e3_limit10_iter30_t4096"
ensure_gateway "$BASE_ROOT" 2048
run_eval "$OUT_E3" \
  LIMIT=10 \
  MAX_ITERATIONS=30 \
  MAX_TOKENS=4096 \
  THINKING_TOKEN_BUDGET=2048

# Compare control vs E3
log "=== COMPARE control vs E3 ==="
summarize_dir "$OUT_CTRL" | tee -a "$ORCH_LOG"
summarize_dir "$OUT_E3" | tee -a "$ORCH_LOG"

CTRL_RESOLVED=$(python3 -c "import json; p='$OUT_CTRL/uenv_results.jsonl'; print(sum(1 for l in open(p) if l.strip() and json.loads(l).get('resolved')))" 2>/dev/null || echo 0)
E3_RESOLVED=$(python3 -c "import json; p='$OUT_E3/uenv_results.jsonl'; print(sum(1 for l in open(p) if l.strip() and json.loads(l).get('resolved')))" 2>/dev/null || echo 0)

if [ "${CTRL_RESOLVED:-0}" -eq 0 ] && [ "${E3_RESOLVED:-0}" -eq 0 ]; then
  log "E3 no resolved gain — trying E1 max_model_len=131072"
  restart_vllm 131072
  ensure_gateway "$BASE_ROOT" 4096
  OUT_E1="${BASE_ROOT}/exp_e1_limit10_iter30_ctx131072"
  run_eval "$OUT_E1" \
    LIMIT=10 \
    MAX_ITERATIONS=30 \
    MAX_TOKENS=8192 \
    THINKING_TOKEN_BUDGET=4096
  log "=== COMPARE after E1 ==="
  summarize_dir "$OUT_E1" | tee -a "$ORCH_LOG"
fi

log "orchestrator finished BASE_ROOT=$BASE_ROOT"
python3 <<PY | tee -a "$ORCH_LOG"
import json, pathlib
root = pathlib.Path("$BASE_ROOT")
print("=== FINAL SUMMARY ===")
for d in sorted(root.glob("*/uenv_results.jsonl")):
    rows = [json.loads(l) for l in d.read_text().splitlines() if l.strip()]
    resolved = sum(1 for r in rows if r.get('resolved'))
    diff = sum(1 for r in rows if int(r.get('git_diff_bytes') or 0)>0)
    ctx = sum(1 for r in rows if 'ContextWindow' in str(r.get('uenv_error_message','')))
    print(f"{d.parent.name}: n={len(rows)} resolved={resolved} diff_gt0={diff} ctx_fail={ctx}")
PY
