#!/usr/bin/env bash
# DSCodeBench Agent 轨道（Verifiers 风格 ToolEnv：run_python + submit_code）评测入口。
#
# 与 run_dscodebench_uenv_baseline.sh（官方单轮 pass@1）是**两条独立轨道**：
#   * 官方轨：Worker code env 单轮取 completion → 官方 harness → pass@1
#   * Agent 轨：Agent 多轮 run_python 自测/自修 → submit_code 定稿 → 同一官方 harness → agentic_pass@1
# 两者指标不可直接比较，输出目录也彼此隔离。
set -euo pipefail

REPO_DIR=${REPO_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)}
BRIDGE_DIR=${BRIDGE_DIR:-${REPO_DIR}/uenv-bridge}
DATA_FILE=${DATA_FILE:-${REPO_DIR}/data/benchmarks/dscodebench/DSCodeBench.json}

UENV_ADAPTER_CORE_ENDPOINT=${UENV_ADAPTER_CORE_ENDPOINT:-8.130.75.157:8088}
POLICY=${POLICY:-llm}
LLM_ENDPOINT=${LLM_ENDPOINT:-}
LLM_MODEL=${LLM_MODEL:-}
LLM_API_KEY=${LLM_API_KEY:-}

RUN_TAG=${RUN_TAG:-$(echo "${LLM_MODEL:-$POLICY}" | tr '/:' '__')}
RUN_NAME=${RUN_NAME:-toolenv_${RUN_TAG}_$(date +%Y%m%d_%H%M%S)}
# 固定输出根目录：Agent 轨道与官方轨道分离（官方轨在 temp/benchmarks/dscodebench/）
OUTPUT_ROOT=${OUTPUT_ROOT:-${REPO_DIR}/temp/benchmarks/dscodebench-agentic}
OUTPUT_DIR=${OUTPUT_DIR:-${OUTPUT_ROOT}/${RUN_NAME}}

LIMIT=${LIMIT:-}
LIBRARY=${LIBRARY:-}
MAX_PER_LIBRARY=${MAX_PER_LIBRARY:-}
MAX_TURNS=${MAX_TURNS:-4}
MAX_TOKENS=${MAX_TOKENS:-2048}
PROMPT_STYLE=${PROMPT_STYLE:-official}
NUM_TESTS=${NUM_TESTS:-200}
EVALUATION_MODE=${EVALUATION_MODE:-inline_harness}
CODE_TIMEOUT_SECS=${CODE_TIMEOUT_SECS:-300}
TIMEOUT_SECONDS=${TIMEOUT_SECONDS:-1800}
CLIENT_TIMEOUT_SECONDS=${CLIENT_TIMEOUT_SECONDS:-2400}
RUN_PYTHON_TIMEOUT=${RUN_PYTHON_TIMEOUT:-60}
# run_python 沙箱解释器：需含 numpy/pandas 等库，建议指向 DSCodeBench venv
PYTHON_BIN=${PYTHON_BIN:-/var/lib/uenv/envs/dscodebench/0.2.0/venv/bin/python}
SHIM_HOST=${SHIM_HOST:-127.0.0.1}
SHIM_PORT=${SHIM_PORT:-18899}
SHIM_PUBLIC_URL=${SHIM_PUBLIC_URL:-}
RESUME=${RESUME:-0}
AGENT_PYTHON=${AGENT_PYTHON:-python3}

if [ "$POLICY" = "llm" ] && { [ -z "$LLM_ENDPOINT" ] || [ -z "$LLM_MODEL" ]; }; then
  echo "POLICY=llm requires LLM_ENDPOINT and LLM_MODEL, e.g. LLM_ENDPOINT=http://10.10.20.142:18099/v1 LLM_MODEL=Qwen3-14B" >&2
  exit 2
fi

mkdir -p "$OUTPUT_DIR"

ARGS=(
  --endpoint "$UENV_ADAPTER_CORE_ENDPOINT"
  --data "$DATA_FILE"
  --policy "$POLICY"
  --prompt-style "$PROMPT_STYLE"
  --max-turns "$MAX_TURNS"
  --max-tokens "$MAX_TOKENS"
  --num-tests "$NUM_TESTS"
  --evaluation-mode "$EVALUATION_MODE"
  --code-timeout-secs "$CODE_TIMEOUT_SECS"
  --timeout-seconds "$TIMEOUT_SECONDS"
  --client-timeout-seconds "$CLIENT_TIMEOUT_SECONDS"
  --run-python-timeout "$RUN_PYTHON_TIMEOUT"
  --shim-host "$SHIM_HOST"
  --shim-port "$SHIM_PORT"
  --output-dir "$OUTPUT_DIR"
  --run-name "$RUN_NAME"
)
[ -n "$LIMIT" ] && ARGS+=(--limit "$LIMIT")
[ -n "$LIBRARY" ] && ARGS+=(--library "$LIBRARY")
[ -n "$MAX_PER_LIBRARY" ] && ARGS+=(--max-per-library "$MAX_PER_LIBRARY")
[ -n "$SHIM_PUBLIC_URL" ] && ARGS+=(--shim-public-url "$SHIM_PUBLIC_URL")
[ -n "$PYTHON_BIN" ] && ARGS+=(--python-bin "$PYTHON_BIN")
[ "$POLICY" = "llm" ] && ARGS+=(--llm-endpoint "$LLM_ENDPOINT" --llm-model "$LLM_MODEL")
[ -n "$LLM_API_KEY" ] && ARGS+=(--llm-api-key "$LLM_API_KEY")
[ "$RESUME" = "1" ] && ARGS+=(--resume)

echo "[agentic] output_dir=$OUTPUT_DIR"
cd "$REPO_DIR"
PROTOCOL_BUFFERS_PYTHON_IMPLEMENTATION=${PROTOCOL_BUFFERS_PYTHON_IMPLEMENTATION:-python} \
PYTHONPATH="${BRIDGE_DIR}/src${PYTHONPATH:+:$PYTHONPATH}" \
  "$AGENT_PYTHON" "${BRIDGE_DIR}/scripts/benchmark/dscode_toolenv_agent.py" "${ARGS[@]}" \
  2>&1 | tee -a "${OUTPUT_DIR}/run.log"

PROTOCOL_BUFFERS_PYTHON_IMPLEMENTATION=${PROTOCOL_BUFFERS_PYTHON_IMPLEMENTATION:-python} \
  "$AGENT_PYTHON" "${BRIDGE_DIR}/scripts/benchmark/report_dscode_agentic.py" \
  --output-dir "$OUTPUT_DIR" ${BASELINE_METRICS:+--baseline-metrics "$BASELINE_METRICS"}
