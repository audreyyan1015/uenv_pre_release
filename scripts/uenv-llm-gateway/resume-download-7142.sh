#!/usr/bin/env bash
# 7142: DeepSeek-V3-0324-AWQ 断点续传 + 断线自动重试
#
# 7142 上:
#   bash /root/UEnv/scripts/uenv-llm-gateway/resume-download-7142.sh start
#   bash /root/UEnv/scripts/uenv-llm-gateway/resume-download-7142.sh status
#   tail -f /var/log/uenv/model-download.log
#
set -euo pipefail

MODEL_REPO="${UENV_MODEL_REPO:-cognitivecomputations/DeepSeek-V3-0324-AWQ}"
MODEL_DIR="${UENV_MODEL_DIR:-/data/models/DeepSeek-V3-0324-AWQ}"
HF_HOME="${UENV_HF_HOME:-/data/huggingface}"
HF_BIN="${HF_BIN:-/opt/hf-download/bin/hf}"
HF_PYTHON="${HF_PYTHON:-/opt/hf-download/bin/python}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
DOWNLOAD_PY="${UENV_DOWNLOAD_PY:-$SCRIPT_DIR/download-awq-resumable.py}"
LOG="${UENV_MODEL_DOWNLOAD_LOG:-/var/log/uenv/model-download.log}"
PIDFILE="${UENV_MODEL_DOWNLOAD_PIDFILE:-/var/run/uenv-model-download.pid}"

EXPECTED_SHARDS="${UENV_MODEL_EXPECTED_SHARDS:-36}"
MIN_SIZE_GB="${UENV_MODEL_MIN_GB:-300}"
RETRY_MIN_SEC="${UENV_MODEL_DOWNLOAD_RETRY_MIN:-30}"
RETRY_MAX_SEC="${UENV_MODEL_DOWNLOAD_RETRY_MAX:-600}"

export HF_ENDPOINT="${HF_ENDPOINT:-https://hf-mirror.com}"
export HF_HOME
# 关闭 xet/hf_transfer，避免 cas-bridge 预签名 URL 超时/403
export HF_HUB_ENABLE_HF_TRANSFER="${HF_HUB_ENABLE_HF_TRANSFER:-0}"
export HF_HUB_DISABLE_XET="${HF_HUB_DISABLE_XET:-1}"
export HF_HUB_DOWNLOAD_TIMEOUT="${HF_HUB_DOWNLOAD_TIMEOUT:-600}"
export HF_HUB_ETAG_TIMEOUT="${HF_HUB_ETAG_TIMEOUT:-60}"

log() {
  local msg="[$(date '+%F %T')] $*"
  printf '%s\n' "$msg"
  printf '%s\n' "$msg" >>"$LOG"
}

shard_count() {
  find "$MODEL_DIR" -maxdepth 1 -name 'model-*.safetensors' 2>/dev/null | wc -l | tr -d ' '
}

size_bytes() {
  du -sb "$MODEL_DIR" 2>/dev/null | awk '{print $1}'
}

size_gb() {
  awk -v b="$(size_bytes 2>/dev/null || echo 0)" 'BEGIN {printf "%.1f", b/1024/1024/1024}'
}

is_complete() {
  local shards bytes min_bytes
  shards="$(shard_count)"
  bytes="$(size_bytes 2>/dev/null || echo 0)"
  min_bytes=$((MIN_SIZE_GB * 1024 * 1024 * 1024))
  [[ "$bytes" -ge "$min_bytes" && "$shards" -ge $((EXPECTED_SHARDS - 1)) ]]
}

ensure_dirs() {
  mkdir -p "$MODEL_DIR" "$HF_HOME" /var/log/uenv "$(dirname "$PIDFILE")"
}

ensure_hf() {
  if [[ ! -x "$HF_PYTHON" ]] && [[ ! -x "$HF_BIN" ]]; then
    log "ERROR: neither HF_PYTHON=$HF_PYTHON nor HF_BIN=$HF_BIN found"
    exit 1
  fi
  ensure_dirs
}

download_once() {
  log "download: repo=$MODEL_REPO dir=$MODEL_DIR endpoint=$HF_ENDPOINT hf_transfer=$HF_HUB_ENABLE_HF_TRANSFER disable_xet=$HF_HUB_DISABLE_XET"
  if [[ -f "$DOWNLOAD_PY" ]] && [[ -x "$HF_PYTHON" ]]; then
    UENV_MODEL_REPO="$MODEL_REPO" UENV_MODEL_DIR="$MODEL_DIR" \
      "$HF_PYTHON" "$DOWNLOAD_PY" 2>&1 | tee -a "$LOG"
    return "${PIPESTATUS[0]}"
  fi
  "$HF_BIN" download "$MODEL_REPO" --local-dir "$MODEL_DIR" 2>&1 | tee -a "$LOG"
  return "${PIPESTATUS[0]}"
}

run_loop() {
  ensure_hf
  local retry="$RETRY_MIN_SEC"
  local prev_bytes
  prev_bytes="$(size_bytes 2>/dev/null || echo 0)"

  while ! is_complete; do
    log "progress: $(size_gb)GB shards=$(shard_count)/$EXPECTED_SHARDS retry_in=${retry}s"

    set +e
    download_once
    local rc=$?
    set -e

    if is_complete; then
      log "DOWNLOAD_COMPLETE size=$(size_gb)GB shards=$(shard_count)/$EXPECTED_SHARDS dir=$MODEL_DIR"
      rm -f "$PIDFILE"
      exit 0
    fi

    local now_bytes
    now_bytes="$(size_bytes 2>/dev/null || echo 0)"
    if [[ "$now_bytes" -gt "$prev_bytes" ]]; then
      log "bytes grew ${prev_bytes} -> ${now_bytes}; reset retry backoff"
      retry="$RETRY_MIN_SEC"
      prev_bytes="$now_bytes"
    else
      if [[ "$rc" -eq 0 ]]; then
        log "hf exited 0 but incomplete; retry in ${retry}s"
      else
        log "hf failed rc=$rc (timeout/403/network); retry in ${retry}s"
      fi
      sleep "$retry"
      retry=$((retry * 2))
      [[ "$retry" -gt "$RETRY_MAX_SEC" ]] && retry="$RETRY_MAX_SEC"
    fi
  done
}

cmd_start() {
  ensure_hf
  if [[ -f "$PIDFILE" ]]; then
    local pid
    pid="$(cat "$PIDFILE")"
    if kill -0 "$pid" 2>/dev/null; then
      echo "already running pid=$pid ($(size_gb)GB shards=$(shard_count)/$EXPECTED_SHARDS)"
      return 0
    fi
    rm -f "$PIDFILE"
  fi
  nohup bash "$0" run >/dev/null 2>&1 &
  echo $! >"$PIDFILE"
  echo "started pid=$(cat "$PIDFILE") log=$LOG"
}

cmd_stop() {
  if [[ -f "$PIDFILE" ]]; then
    kill "$(cat "$PIDFILE")" 2>/dev/null || true
  fi
  pkill -f "resume-download-7142.sh run" 2>/dev/null || true
  pkill -f "download-awq-resumable.py" 2>/dev/null || true
  pkill -f "hf download.*DeepSeek-V3-0324-AWQ" 2>/dev/null || true
  rm -f "$PIDFILE"
  echo "stopped"
}

cmd_status() {
  local running=no pid=-
  if [[ -f "$PIDFILE" ]]; then
    pid="$(cat "$PIDFILE")"
    if kill -0 "$pid" 2>/dev/null; then running=yes; fi
  fi
  echo "running=$running pid=$pid size=$(size_gb)GB shards=$(shard_count)/$EXPECTED_SHARDS complete=$(is_complete && echo yes || echo no)"
}

case "${1:-start}" in
  run) run_loop ;;
  start) cmd_start ;;
  stop) cmd_stop ;;
  status) cmd_status ;;
  *)
    echo "usage: $0 {start|stop|status|run}" >&2
    exit 1
    ;;
esac
