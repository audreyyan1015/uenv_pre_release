#!/usr/bin/env bash
set -euo pipefail

SERVICE_NAME="${SERVICE_NAME:-uenv-server.service}"
REPO_ROOT="${REPO_ROOT:-$(git rev-parse --show-toplevel 2>/dev/null || pwd)}"
BASELINE_ID="${BASELINE_ID:-10k-pre-$(date +%Y%m%d-%H%M%S)}"
OUT_DIR="${BASELINE_OUT_DIR:-/home/uenv/baseline-artifacts/${BASELINE_ID}}"
MANIFEST_PATH="${MANIFEST_PATH:-${OUT_DIR}/manifest.json}"
REPO_UNIT="${REPO_UNIT:-${REPO_ROOT}/deploy/systemd/uenv-server.service}"
INSTALLED_UNIT="${INSTALLED_UNIT:-/etc/systemd/system/uenv-server.service}"
BINARY="${BINARY:-/usr/local/bin/uenv-adapter-core}"
TARGET_BINARY="${TARGET_BINARY:-${REPO_ROOT}/target/release/uenv-adapter-core}"
CONFIG="${CONFIG:-${REPO_ROOT}/config/server.yaml}"
ADMIN_URL="${ADMIN_URL:-http://127.0.0.1:50052}"

sha_or_null() {
  local path="$1"
  if [ -e "$path" ]; then
    sha256sum "$path" | awk '{print $1}'
  else
    printf 'null'
  fi
}

json_file_or_null() {
  local path="$1"
  if [ -s "$path" ] && jq -e . "$path" >/dev/null 2>&1; then
    jq -c . "$path"
  else
    printf 'null'
  fi
}

main() {
  cd "$REPO_ROOT"
  mkdir -p "$OUT_DIR"

  local status_file service_file status_json_file agents_json_file listeners_file limits_file
  status_file="${OUT_DIR}/git-status.txt"
  service_file="${OUT_DIR}/systemd-show.txt"
  status_json_file="${OUT_DIR}/admin-status.json"
  agents_json_file="${OUT_DIR}/admin-agents.json"
  listeners_file="${OUT_DIR}/listeners.txt"
  limits_file="${OUT_DIR}/service-limits.txt"

  git status --short --branch >"$status_file"
  systemctl show "$SERVICE_NAME" >"$service_file"
  ss -ltnp >"$listeners_file"

  local main_pid
  main_pid="$(systemctl show "$SERVICE_NAME" -p MainPID --value 2>/dev/null || echo 0)"
  if [ "$main_pid" != "0" ] && [ -r "/proc/${main_pid}/limits" ]; then
    cp "/proc/${main_pid}/limits" "$limits_file"
  else
    : >"$limits_file"
  fi

  curl -fsS --max-time 3 "${ADMIN_URL}/status" >"$status_json_file" 2>/dev/null || : >"$status_json_file"
  curl -fsS --max-time 3 "${ADMIN_URL}/agents" >"$agents_json_file" 2>/dev/null || : >"$agents_json_file"

  local git_dirty unit_match binary_match active_state sub_state nrestarts nofile_soft nofile_hard
  git_dirty="$(git status --porcelain=v1 | wc -l | awk '{print $1}')"
  cmp -s "$REPO_UNIT" "$INSTALLED_UNIT" && unit_match=true || unit_match=false
  cmp -s "$BINARY" "$TARGET_BINARY" && binary_match=true || binary_match=false
  active_state="$(systemctl show "$SERVICE_NAME" -p ActiveState --value 2>/dev/null || true)"
  sub_state="$(systemctl show "$SERVICE_NAME" -p SubState --value 2>/dev/null || true)"
  nrestarts="$(systemctl show "$SERVICE_NAME" -p NRestarts --value 2>/dev/null || true)"
  nofile_soft="$(awk '/Max open files/ {print $4}' "$limits_file" 2>/dev/null || true)"
  nofile_hard="$(awk '/Max open files/ {print $5}' "$limits_file" 2>/dev/null || true)"

  jq -n \
    --arg baseline_id "$BASELINE_ID" \
    --arg generated_at "$(date -Is)" \
    --arg repo_root "$REPO_ROOT" \
    --arg branch "$(git branch --show-current)" \
    --arg git_sha "$(git rev-parse HEAD)" \
    --arg git_describe "$(git describe --always --dirty --tags 2>/dev/null || git rev-parse --short HEAD)" \
    --argjson git_dirty_count "$git_dirty" \
    --arg service_name "$SERVICE_NAME" \
    --arg active_state "$active_state" \
    --arg sub_state "$sub_state" \
    --arg main_pid "$main_pid" \
    --arg nrestarts "$nrestarts" \
    --arg binary "$BINARY" \
    --arg binary_sha256 "$(sha_or_null "$BINARY")" \
    --arg target_binary "$TARGET_BINARY" \
    --arg target_binary_sha256 "$(sha_or_null "$TARGET_BINARY")" \
    --argjson binary_matches_target "$binary_match" \
    --arg config "$CONFIG" \
    --arg config_sha256 "$(sha_or_null "$CONFIG")" \
    --arg repo_unit "$REPO_UNIT" \
    --arg repo_unit_sha256 "$(sha_or_null "$REPO_UNIT")" \
    --arg installed_unit "$INSTALLED_UNIT" \
    --arg installed_unit_sha256 "$(sha_or_null "$INSTALLED_UNIT")" \
    --argjson repo_unit_matches_installed "$unit_match" \
    --arg hostname "$(hostname)" \
    --arg kernel "$(uname -srmo)" \
    --arg cpu_count "$(nproc)" \
    --arg memory_bytes "$(awk '/MemTotal/ {printf "%d", $2 * 1024}' /proc/meminfo)" \
    --arg nofile_soft "${nofile_soft:-}" \
    --arg nofile_hard "${nofile_hard:-}" \
    --arg status_file "$status_file" \
    --arg service_file "$service_file" \
    --arg listeners_file "$listeners_file" \
    --arg limits_file "$limits_file" \
    --argjson admin_status "$(json_file_or_null "$status_json_file")" \
    --argjson admin_agents "$(json_file_or_null "$agents_json_file")" \
    '{
      baseline_id: $baseline_id,
      generated_at: $generated_at,
      repo: {
        root: $repo_root,
        branch: $branch,
        git_sha: $git_sha,
        git_describe: $git_describe,
        dirty_entry_count: $git_dirty_count,
        git_status_file: $status_file
      },
      service: {
        name: $service_name,
        active_state: $active_state,
        sub_state: $sub_state,
        main_pid: ($main_pid | tonumber? // 0),
        nrestarts: ($nrestarts | tonumber? // 0),
        systemd_show_file: $service_file,
        process_limits_file: $limits_file,
        nofile_soft: ($nofile_soft | tonumber? // null),
        nofile_hard: ($nofile_hard | tonumber? // null)
      },
      artifacts: {
        installed_binary: $binary,
        installed_binary_sha256: $binary_sha256,
        target_binary: $target_binary,
        target_binary_sha256: $target_binary_sha256,
        binary_matches_target: $binary_matches_target,
        server_config: $config,
        server_config_sha256: $config_sha256,
        repo_systemd_unit: $repo_unit,
        repo_systemd_unit_sha256: $repo_unit_sha256,
        installed_systemd_unit: $installed_unit,
        installed_systemd_unit_sha256: $installed_unit_sha256,
        repo_unit_matches_installed: $repo_unit_matches_installed
      },
      host: {
        hostname: $hostname,
        kernel: $kernel,
        cpu_count: ($cpu_count | tonumber),
        memory_bytes: ($memory_bytes | tonumber)
      },
      listeners_file: $listeners_file,
      admin_status: $admin_status,
      admin_agents: $admin_agents
    }' >"$MANIFEST_PATH"

  jq . "$MANIFEST_PATH" >/dev/null
  printf '%s\n' "$MANIFEST_PATH"
}

main "$@"
