#!/usr/bin/env bash
set -euo pipefail

# 原生 VeRL baseline：自定义 SweAgentLoop + OpenHands runtime。
# 该脚本复用 SWE-smith 训练参数，只把 VeRL AgentLoop 从 UEnv episode
# 提交路径切到 native_swe_agent，不经过 Adapter Core / Server。
#
# 必填：
#   NATIVE_SWE_RUNTIME_GATEWAY_URL  SWE runtime gateway 地址，例如 http://host:28097
#
# 常用示例：
#   NATIVE_SWE_RUNTIME_GATEWAY_URL=http://219.147.100.43:28097 \
#     ./scripts/train/launchers/swe/native/swe_smith_native_verl_grpo_train.sh --limit 20
#
# gRPC 后端需要先启动远端 OpenHands poller。208.77 当前不能直连训练机
# 10.10.20.142:19051/18088，建议先在训练机建立反向隧道：
#   SSHPASS=dev@BDW2026 sshpass -e ssh -N \
#     -R 19051:127.0.0.1:19051 \
#     -R 18088:127.0.0.1:18088 \
#     root@8.130.208.77
# 再在 208.77 启动：
#   UENV_SERVER_ENDPOINT=127.0.0.1:19051 \
#     /root/uenv-native-swe-agentloop-20260823_231433/start-native-agent-poller.sh

REPO_DIR=${REPO_DIR:-"$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../../.." && pwd)"}

export RUN_TS=${RUN_TS:-$(date +%Y%m%d_%H%M%S)}
export RUN_ID=${RUN_ID:-verl_native_swesmith_grpo_train_${RUN_TS}}
export VERL_AGENT_LOOP_NAME=native_swe_agent
export VERL_AGENT_LOOP_CONFIG_PATH=/uenv/uenv-bridge/configs/native-swe-agent-loop.yaml
export UENV_MONOREPO_MOUNT_ENABLED=${UENV_MONOREPO_MOUNT_ENABLED:-1}
export UENV_MONOREPO_DIR=${UENV_MONOREPO_DIR:-"$(cd "${REPO_DIR}/.." && pwd)"}
export NATIVE_SWE_DRIVER_PATH=${NATIVE_SWE_DRIVER_PATH:-/uenv/integrations/openhands/run_swebenchpro_official.py}
export NATIVE_SWE_OUTPUT_ROOT=${NATIVE_SWE_OUTPUT_ROOT:-/uenv/uenv-bridge/temp/logs/native_swe_agent_loop}
export NATIVE_SWE_LLM_CONFIG_PATH=${NATIVE_SWE_LLM_CONFIG_PATH:-${SWE_LLM_CONFIG_PATH:-/root/UEnv/config/openhands-llm-qwen3-thinking-max-token-8192.json}}
export NATIVE_SWE_OPENHANDS_MODE=${NATIVE_SWE_OPENHANDS_MODE:-llm}
export NATIVE_SWE_ROLLOUT_TRACE=${NATIVE_SWE_ROLLOUT_TRACE:-required}
export NATIVE_SWE_RUN_TIMEOUT_SECONDS=${NATIVE_SWE_RUN_TIMEOUT_SECONDS:-7200}
export NATIVE_SWE_MAX_CONCURRENCY=${NATIVE_SWE_MAX_CONCURRENCY:-1}
export NATIVE_SWE_EXECUTION_BACKEND=${NATIVE_SWE_EXECUTION_BACKEND:-grpc}
export NATIVE_SWE_REMOTE_HOST=${NATIVE_SWE_REMOTE_HOST:-8.130.208.77}
export NATIVE_SWE_REMOTE_USER=${NATIVE_SWE_REMOTE_USER:-root}
export NATIVE_SWE_REMOTE_PORT=${NATIVE_SWE_REMOTE_PORT:-22}
export NATIVE_SWE_REMOTE_WORK_ROOT=${NATIVE_SWE_REMOTE_WORK_ROOT:-/tmp/uenv-native-swe-agent-loop}
export NATIVE_SWE_REMOTE_RUNNER=${NATIVE_SWE_REMOTE_RUNNER:-/root/UEnv/scripts/run-openhands-pro-20877.sh}
export NATIVE_SWE_REMOTE_BRIDGE_DIR=${NATIVE_SWE_REMOTE_BRIDGE_DIR:-/root/uenv-native-swe-agentloop-20260823_231433/integrations/openhands}
export NATIVE_SWE_REMOTE_LLM_CONFIG_PATH=${NATIVE_SWE_REMOTE_LLM_CONFIG_PATH:-/root/UEnv/config/openhands-llm-20877.json}
export NATIVE_SWE_AGENT_CONTROL_HOST=${NATIVE_SWE_AGENT_CONTROL_HOST:-0.0.0.0}
export NATIVE_SWE_AGENT_CONTROL_PORT=${NATIVE_SWE_AGENT_CONTROL_PORT:-19051}
export NATIVE_SWE_AGENT_CONTROL_PUBLIC_ENDPOINT=${NATIVE_SWE_AGENT_CONTROL_PUBLIC_ENDPOINT:-127.0.0.1:${NATIVE_SWE_AGENT_CONTROL_PORT}}
export UENV_MODEL_GATEWAY_PORT=${UENV_MODEL_GATEWAY_PORT:-18088}
export UENV_MODEL_GATEWAY_PUBLIC_URL=${UENV_MODEL_GATEWAY_PUBLIC_URL:-http://127.0.0.1:${UENV_MODEL_GATEWAY_PORT}/v1}
export UENV_AGENT_LOOP_FAILED_EPISODE_POLICY=${UENV_AGENT_LOOP_FAILED_EPISODE_POLICY:-zero_reward}
export TRAIN_SEED=${TRAIN_SEED:-42}
export DATA_SHUFFLE=${DATA_SHUFFLE:-false}
export DATA_SEED=${DATA_SEED:-${TRAIN_SEED}}
export ACTOR_DATA_LOADER_SEED=${ACTOR_DATA_LOADER_SEED:-${TRAIN_SEED}}
export CRITIC_DATA_LOADER_SEED=${CRITIC_DATA_LOADER_SEED:-${TRAIN_SEED}}
export ROLLOUT_SEED=${ROLLOUT_SEED:-${TRAIN_SEED}}

if [ -z "${NATIVE_SWE_RUNTIME_GATEWAY_URL:-}" ]; then
  echo "NATIVE_SWE_RUNTIME_GATEWAY_URL is required for native SWE AgentLoop baseline" >&2
  exit 1
fi
if [ "${NATIVE_SWE_EXECUTION_BACKEND}" = "ssh" ] && [ -z "${NATIVE_SWE_REMOTE_IDENTITY_FILE_HOST:-}" ] && [ -z "${NATIVE_SWE_REMOTE_IDENTITY_FILE:-}" ] && [ -z "${NATIVE_SWE_REMOTE_PASSWORD:-}" ]; then
  echo "ssh backend requires NATIVE_SWE_REMOTE_PASSWORD or NATIVE_SWE_REMOTE_IDENTITY_FILE_HOST" >&2
  exit 1
fi

exec "${REPO_DIR}/scripts/train/launchers/swe/swe_smith_grpo_train.sh" "$@"
