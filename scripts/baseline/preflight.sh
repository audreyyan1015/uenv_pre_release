#!/usr/bin/env bash
set -u

SERVICE_NAME="${SERVICE_NAME:-uenv-server.service}"
REPO_ROOT="${REPO_ROOT:-$(git rev-parse --show-toplevel 2>/dev/null || pwd)}"
REPO_UNIT="${REPO_UNIT:-${REPO_ROOT}/deploy/systemd/uenv-server.service}"
INSTALLED_UNIT="${INSTALLED_UNIT:-/etc/systemd/system/uenv-server.service}"
BINARY="${BINARY:-/usr/local/bin/uenv-adapter-core}"
TARGET_BINARY="${TARGET_BINARY:-${REPO_ROOT}/target/release/uenv-adapter-core}"
CONFIG="${CONFIG:-${REPO_ROOT}/config/server.yaml}"
ADMIN_URL="${ADMIN_URL:-http://127.0.0.1:50052}"
REQUIRED_PORTS="${REQUIRED_PORTS:-8088 8077 50052}"
MIN_NOFILE="${MIN_NOFILE:-1048576}"

pass_count=0
warn_count=0
fail_count=0

pass() {
  pass_count=$((pass_count + 1))
  printf 'PASS %s\n' "$1"
}

warn() {
  warn_count=$((warn_count + 1))
  printf 'WARN %s\n' "$1"
}

fail() {
  fail_count=$((fail_count + 1))
  printf 'FAIL %s\n' "$1"
}

check_file() {
  local path="$1"
  local label="$2"
  if [ -e "$path" ]; then
    pass "$label exists: $path"
  else
    fail "$label missing: $path"
  fi
}

json_field_number() {
  local json="$1"
  local field="$2"
  jq -r "$field // empty" 2>/dev/null <<<"$json"
}

check_git_status() {
  local disallowed
  disallowed="$(
    git -C "$REPO_ROOT" status --porcelain=v1 | while IFS= read -r line; do
      case "$line" in
        '?? Docs/server/10000-worker-pre-change-baseline-implementation.md') ;;
        '?? Docs/server/10000-worker-scale-gap-list.md') ;;
        '?? scripts/openhands/__pycache__/') ;;
        '?? trajectory-data/') ;;
        '?? baseline-artifacts/'*) ;;
        '?? scripts/baseline/'*) [ "${ALLOW_UNCOMMITTED_BASELINE_SCRIPTS:-0}" = "1" ] || printf '%s\n' "$line" ;;
        ' M scripts/baseline/'*) [ "${ALLOW_UNCOMMITTED_BASELINE_SCRIPTS:-0}" = "1" ] || printf '%s\n' "$line" ;;
        *) printf '%s\n' "$line" ;;
      esac
    done
  )"
  if [ -z "$disallowed" ]; then
    pass "git worktree has no disallowed dirty entries"
  else
    fail "git worktree has disallowed dirty entries:"
    printf '%s\n' "$disallowed"
  fi
}

check_service() {
  local active substate restarts main_pid
  active="$(systemctl show "$SERVICE_NAME" -p ActiveState --value 2>/dev/null || true)"
  substate="$(systemctl show "$SERVICE_NAME" -p SubState --value 2>/dev/null || true)"
  restarts="$(systemctl show "$SERVICE_NAME" -p NRestarts --value 2>/dev/null || echo unknown)"
  main_pid="$(systemctl show "$SERVICE_NAME" -p MainPID --value 2>/dev/null || echo 0)"

  [ "$active" = "active" ] && pass "$SERVICE_NAME ActiveState=active" || fail "$SERVICE_NAME ActiveState=$active"
  [ "$substate" = "running" ] && pass "$SERVICE_NAME SubState=running" || fail "$SERVICE_NAME SubState=$substate"
  [ "$restarts" = "0" ] && pass "$SERVICE_NAME NRestarts=0" || fail "$SERVICE_NAME NRestarts=$restarts"
  [ "$main_pid" != "0" ] && [ -n "$main_pid" ] && pass "$SERVICE_NAME MainPID=$main_pid" || fail "$SERVICE_NAME has no MainPID"

  if [ "$main_pid" != "0" ] && [ -r "/proc/${main_pid}/limits" ]; then
    local nofile_soft nofile_hard
    nofile_soft="$(awk '/Max open files/ {print $4}' "/proc/${main_pid}/limits")"
    nofile_hard="$(awk '/Max open files/ {print $5}' "/proc/${main_pid}/limits")"
    if [ "${nofile_soft:-0}" -ge "$MIN_NOFILE" ] && [ "${nofile_hard:-0}" -ge "$MIN_NOFILE" ]; then
      pass "service nofile soft/hard=${nofile_soft}/${nofile_hard}"
    else
      fail "service nofile soft/hard=${nofile_soft:-unknown}/${nofile_hard:-unknown}, required >= ${MIN_NOFILE}"
    fi
  else
    fail "cannot read service process limits for pid=${main_pid}"
  fi
}

check_deploy_files() {
  check_file "$REPO_UNIT" "repo systemd unit"
  check_file "$INSTALLED_UNIT" "installed systemd unit"
  check_file "$BINARY" "installed adapter binary"
  check_file "$TARGET_BINARY" "target release adapter binary"
  check_file "$CONFIG" "server config"

  if [ -e "$REPO_UNIT" ] && [ -e "$INSTALLED_UNIT" ]; then
    cmp -s "$REPO_UNIT" "$INSTALLED_UNIT" && pass "repo unit matches installed unit" || fail "repo unit differs from installed unit"
  fi
  if [ -e "$BINARY" ] && [ -e "$TARGET_BINARY" ]; then
    cmp -s "$BINARY" "$TARGET_BINARY" && pass "installed binary matches target/release binary" || fail "installed binary differs from target/release binary"
  fi
}

check_ports() {
  local port
  for port in $REQUIRED_PORTS; do
    if ss -ltnH "sport = :${port}" 2>/dev/null | grep -q .; then
      pass "port ${port} is listening"
    else
      fail "port ${port} is not listening"
    fi
  done
}

check_orphan_adapter() {
  local main_pid adapter_pids listener_pids unexpected_pids
  main_pid="$(systemctl show "$SERVICE_NAME" -p MainPID --value 2>/dev/null || echo 0)"
  adapter_pids="$(pgrep -f '/usr/local/bin/uenv-adapter-core' 2>/dev/null | sort -n || true)"
  listener_pids="$(
    for port in $REQUIRED_PORTS; do
      ss -ltnpH "sport = :${port}" 2>/dev/null \
        | sed -n 's/.*pid=\([0-9][0-9]*\).*/\1/p'
    done | sort -n | uniq
  )"

  unexpected_pids="$(
    printf '%s\n' "$listener_pids" | while IFS= read -r pid; do
      [ -n "$pid" ] || continue
      if [ "$main_pid" = "0" ] || [ "$pid" != "$main_pid" ]; then
        printf '%s\n' "$pid"
      fi
    done
  )"

  if [ -z "$adapter_pids" ]; then
    warn "no /usr/local/bin/uenv-adapter-core process found"
  else
    pass "adapter process ids: $(printf '%s' "$adapter_pids" | tr '\n' ' ')"
  fi

  if [ -z "$unexpected_pids" ]; then
    pass "adapter listeners are owned by systemd MainPID"
  else
    fail "orphan_adapter_detected: listener pid(s) not owned by $SERVICE_NAME MainPID=${main_pid}: $(printf '%s' "$unexpected_pids" | tr '\n' ' ')"
  fi
}

check_admin_state() {
  local status agents
  status="$(curl -fsS --max-time 3 "${ADMIN_URL}/status" 2>/dev/null || true)"
  if [ -n "$status" ] && jq -e . >/dev/null 2>&1 <<<"$status"; then
    pass "admin /status returns valid JSON"
    [ "$(json_field_number "$status" '.active_episodes')" = "0" ] && pass "active_episodes=0" || fail "active_episodes=$(json_field_number "$status" '.active_episodes')"
    [ "$(json_field_number "$status" '.pending_results')" = "0" ] && pass "pending_results=0" || fail "pending_results=$(json_field_number "$status" '.pending_results')"
  else
    fail "admin /status unavailable or invalid"
  fi

  agents="$(curl -fsS --max-time 3 "${ADMIN_URL}/agents" 2>/dev/null || true)"
  if [ -n "$agents" ] && jq -e . >/dev/null 2>&1 <<<"$agents"; then
    pass "admin /agents returns valid JSON"
    [ "$(json_field_number "$agents" '.pending_jobs')" = "0" ] && pass "pending_jobs=0" || fail "pending_jobs=$(json_field_number "$agents" '.pending_jobs')"
    [ "$(json_field_number "$agents" '.running_jobs')" = "0" ] && pass "running_jobs=0" || fail "running_jobs=$(json_field_number "$agents" '.running_jobs')"
    [ "$(json_field_number "$agents" '.outstanding_jobs')" = "0" ] && pass "outstanding_jobs=0" || fail "outstanding_jobs=$(json_field_number "$agents" '.outstanding_jobs')"
  else
    warn "admin /agents unavailable or invalid"
  fi
}

main() {
  cd "$REPO_ROOT"
  printf 'Gate 0 preflight for %s\n' "$SERVICE_NAME"
  printf 'repo=%s\n' "$REPO_ROOT"
  check_git_status
  check_service
  check_deploy_files
  check_ports
  check_orphan_adapter
  check_admin_state
  printf 'summary: pass=%s warn=%s fail=%s\n' "$pass_count" "$warn_count" "$fail_count"
  [ "$fail_count" -eq 0 ]
}

main "$@"
