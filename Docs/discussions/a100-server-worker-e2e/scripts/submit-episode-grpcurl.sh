#!/usr/bin/env bash
# Bridge Mock：向 uenv-server 提交 MathEnv 单轮 Episode（grpcurl，dataset=gsm8k）
# 用法: ./submit-episode-grpcurl.sh [SERVER_HOST:PORT]
# 默认: 127.0.0.1:50051（在机器 A 上执行）

set -euo pipefail

SERVER="${1:-127.0.0.1:50051}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
PROTO_ROOT="$REPO_ROOT/proto"

if ! command -v grpcurl >/dev/null 2>&1; then
  echo "grpcurl 未安装。见 prep-bootstrap.sh 或:"
  echo "  go install github.com/fullstorydev/grpcurl/cmd/grpcurl@latest"
  exit 1
fi

PAYLOAD_B64=$(printf '%s' '{"question":"If 3 books cost $12, what is the cost of 5 books?","dataset":"gsm8k"}' | base64 -w0)
REWARD_B64=$(printf '%s' '{"type":"rule_reward","target":"20"}' | base64 -w0)

grpcurl -plaintext \
  -import-path "$PROTO_ROOT" \
  -proto uenv/v1/server.proto \
  -import-path "$PROTO_ROOT" \
  -proto uenv/v1/episode.proto \
  -d "{
  \"episode_id\": \"math-e2e-001\",
  \"attempt_id\": 1,
  \"env_type\": \"math\",
  \"payload\": \"${PAYLOAD_B64}\",
  \"mode\": \"MODE_SINGLE\",
  \"max_steps\": 1,
  \"correlation_id\": \"e2e-trace-001\",
  \"timeout_seconds\": 120,
  \"reward_config\": \"${REWARD_B64}\"
}" "$SERVER" uenv.v1.UEnvService/SubmitEpisode
