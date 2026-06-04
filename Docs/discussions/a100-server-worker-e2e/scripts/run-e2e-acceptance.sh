#!/usr/bin/env bash
# Worker-Server 实机联调验收（机器 A 执行）
# 用法: bash run-e2e-acceptance.sh [SERVER_HOST:PORT]
set -euo pipefail

SERVER="${1:-127.0.0.1:50051}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
PROTO_ROOT="$REPO_ROOT/proto"
WORKER_IP="${WORKER_IP:-10.10.20.142}"

pass=0
fail=0

check() {
  local name="$1"
  shift
  if "$@"; then
    echo "[PASS] $name"
    pass=$((pass + 1))
  else
    echo "[FAIL] $name"
    fail=$((fail + 1))
  fi
}

submit_episode() {
  local eid="$1"
  local cid="$2"
  local payload_b64 reward_b64
  payload_b64=$(printf '%s' '{"question":"If 3 books cost $12, what is the cost of 5 books?","dataset":"gsm8k"}' | base64 -w0)
  reward_b64=$(printf '%s' '{"type":"rule_reward","target":"20"}' | base64 -w0)
  grpcurl -plaintext \
    -import-path "$PROTO_ROOT" \
    -proto uenv/v1/server.proto \
    -import-path "$PROTO_ROOT" \
    -proto uenv/v1/episode.proto \
    -d "{
    \"episode_id\": \"${eid}\",
    \"attempt_id\": 1,
    \"env_type\": \"math\",
    \"payload\": \"${payload_b64}\",
    \"mode\": \"MODE_SINGLE\",
    \"max_steps\": 1,
    \"correlation_id\": \"${cid}\",
    \"timeout_seconds\": 120,
    \"reward_config\": \"${reward_b64}\"
  }" "$SERVER" uenv.v1.UEnvService/SubmitEpisode
}

check_episode() {
  local name="$1"
  local out="$2"
  local field="$3"
  if echo "$out" | grep -q "$field"; then
    echo "[PASS] $name"
    pass=$((pass + 1))
  else
    echo "[FAIL] $name"
    fail=$((fail + 1))
  fi
}

echo "=== Worker-Server E2E Acceptance ==="
echo "server=$SERVER worker=$WORKER_IP"

check "server :50051 listening" ss -tlnp | grep -q ':50051'
check "worker :50052 reachable" timeout 3 bash -c "echo > /dev/tcp/${WORKER_IP}/50052"

echo "--- Episode #1 (cold / first dispatch) ---"
OUT1=$(submit_episode "math-e2e-run-$(date +%s)-001" "e2e-run-001" 2>&1) || true
echo "$OUT1"
check_episode "episode1 status=completed" "$OUT1" '"status": "completed"'
check_episode "episode1 total_reward=1" "$OUT1" '"totalReward": 1'
check_episode "episode1 integrity_verified" "$OUT1" '"integrityVerified": true'

echo "--- Episode #2 (warm pool reuse) ---"
OUT2=$(submit_episode "math-e2e-run-$(date +%s)-002" "e2e-run-002" 2>&1) || true
echo "$OUT2"
check_episode "episode2 status=completed" "$OUT2" '"status": "completed"'
check_episode "episode2 total_reward=1" "$OUT2" '"totalReward": 1'

echo "--- Log cross-check ---"
check "server register log" grep -q 'control_plane_register.*10.10.20.142:50052' /var/log/uenv/server.log 2>/dev/null || \
  grep -q 'worker registered.*10.10.20.142:50052' /var/log/uenv/server.log 2>/dev/null || \
  grep -q 'control_plane_register' /tmp/uenv-server.log 2>/dev/null
check "server report_result log" grep -q 'control_plane_report_result' /var/log/uenv/server.log 2>/dev/null || \
  grep -q 'control_plane_report_result' /tmp/uenv-server.log 2>/dev/null

echo "--- Summary ---"
echo "PASS=$pass FAIL=$fail"
if [[ "$fail" -gt 0 ]]; then
  exit 1
fi
echo "ALL CHECKS PASSED"
