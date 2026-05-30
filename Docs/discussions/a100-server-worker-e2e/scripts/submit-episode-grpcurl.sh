#!/usr/bin/env bash
# Bridge Mock：向 uenv-server 提交 GSM8K 单轮 Episode（grpcurl）
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

grpcurl -plaintext \
  -import-path "$PROTO_ROOT" \
  -proto uenv/v1/server.proto \
  -import-path "$PROTO_ROOT" \
  -proto uenv/v1/episode.proto \
  -d @ "$SERVER" uenv.v1.UEnvService/SubmitEpisode <<'EOF'
{
  "episode_id": "gsm8k-e2e-001",
  "attempt_id": 1,
  "env_type": "gsm8k",
  "payload": "{\"question\":\"If 3 books cost $12, what is the cost of 5 books?\"}",
  "mode": "MODE_SINGLE",
  "max_steps": 1,
  "correlation_id": "e2e-trace-001",
  "timeout_seconds": 120,
  "reward_config": "{\"type\":\"rule_reward\",\"target\":\"20\"}"
}
EOF
