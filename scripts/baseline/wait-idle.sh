#!/usr/bin/env bash
set -euo pipefail

ADMIN_URL="${ADMIN_URL:-http://127.0.0.1:50052}"
INTERVAL_SECS="${INTERVAL_SECS:-5}"
TIMEOUT_SECS="${TIMEOUT_SECS:-0}"
REQUIRE_SYSTEMD_ACTIVE="${REQUIRE_SYSTEMD_ACTIVE:-0}"
SERVICE_NAME="${SERVICE_NAME:-uenv-server.service}"

started_at="$(date +%s)"

field() {
  local json="$1"
  local expr="$2"
  jq -r "$expr // 0" <<<"$json"
}

while true; do
  now="$(date +%s)"
  if [ "$TIMEOUT_SECS" -gt 0 ] && [ $((now - started_at)) -ge "$TIMEOUT_SECS" ]; then
    echo "timeout waiting for idle state after ${TIMEOUT_SECS}s" >&2
    exit 124
  fi

  status="$(curl -fsS --max-time 3 "${ADMIN_URL}/status")"
  agents="$(curl -fsS --max-time 3 "${ADMIN_URL}/agents" 2>/dev/null || echo '{}')"

  active_episodes="$(field "$status" '.active_episodes')"
  pending_results="$(field "$status" '.pending_results')"
  pending_jobs="$(field "$agents" '.pending_jobs')"
  running_jobs="$(field "$agents" '.running_jobs')"
  outstanding_jobs="$(field "$agents" '.outstanding_jobs')"

  systemd_ok=1
  if [ "$REQUIRE_SYSTEMD_ACTIVE" = "1" ]; then
    active="$(systemctl show "$SERVICE_NAME" -p ActiveState --value 2>/dev/null || true)"
    substate="$(systemctl show "$SERVICE_NAME" -p SubState --value 2>/dev/null || true)"
    [ "$active" = "active" ] && [ "$substate" = "running" ] || systemd_ok=0
  fi

  printf 'active_episodes=%s pending_results=%s pending_jobs=%s running_jobs=%s outstanding_jobs=%s systemd_ok=%s\n' \
    "$active_episodes" "$pending_results" "$pending_jobs" "$running_jobs" "$outstanding_jobs" "$systemd_ok"

  if [ "$active_episodes" = "0" ] \
    && [ "$pending_results" = "0" ] \
    && [ "$pending_jobs" = "0" ] \
    && [ "$running_jobs" = "0" ] \
    && [ "$outstanding_jobs" = "0" ] \
    && [ "$systemd_ok" = "1" ]; then
    echo "idle"
    exit 0
  fi

  sleep "$INTERVAL_SECS"
done
