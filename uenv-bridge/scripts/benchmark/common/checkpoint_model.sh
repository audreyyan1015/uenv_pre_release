#!/usr/bin/env bash

# Optional model-serving helper for UEnv benchmark scripts.
#
# If UENV_ROLLOUT_MODEL_ENDPOINT or MODEL_ENDPOINT is set, benchmark scripts keep
# using that endpoint. If CHECKPOINT_DIR, HF_DIR, or MODEL_DIR is set, this helper
# starts vLLM plus the adapter model gateway and exports UENV_ROLLOUT_MODEL_ENDPOINT.

UENV_BENCHMARK_MODEL_VLLM_STARTED=0
UENV_BENCHMARK_MODEL_GATEWAY_STARTED=0
UENV_BENCHMARK_MODEL_GATEWAY_PID=""
UENV_BENCHMARK_MODEL_VLLM_CONTAINER=""
UENV_BENCHMARK_MODEL_CLEANUP_REGISTERED=0

uenv_benchmark_model_log() {
  printf '[uenv-benchmark-model] %s\n' "$*" >&2
}

uenv_benchmark_has_hf_weights() {
  local model_dir="$1"
  [ -f "${model_dir}/model.safetensors.index.json" ] \
    || compgen -G "${model_dir}/*.safetensors" >/dev/null \
    || compgen -G "${model_dir}/pytorch_model*.bin" >/dev/null
}

uenv_benchmark_resolve_actor_checkpoint_dir() {
  local checkpoint_dir="$1"
  if [ -d "${checkpoint_dir}/actor" ]; then
    printf '%s\n' "${checkpoint_dir}/actor"
  else
    printf '%s\n' "${checkpoint_dir}"
  fi
}

uenv_benchmark_wait_for_http() {
  local url="$1"
  local label="$2"
  local attempts="${3:-120}"
  local sleep_seconds="${4:-5}"

  for _ in $(seq 1 "${attempts}"); do
    if curl -sf --noproxy '*' "${url}" >/dev/null 2>&1; then
      uenv_benchmark_model_log "${label} ready: ${url}"
      return 0
    fi
    sleep "${sleep_seconds}"
  done
  printf 'Timed out waiting for %s: %s\n' "${label}" "${url}" >&2
  return 1
}

uenv_benchmark_merge_checkpoint() {
  local checkpoint_dir="$1"
  local hf_dir="$2"

  if [ "${SKIP_MERGE:-0}" = "1" ]; then
    uenv_benchmark_model_log "SKIP_MERGE=1, using ${hf_dir}"
    return 0
  fi
  if uenv_benchmark_has_hf_weights "${hf_dir}"; then
    uenv_benchmark_model_log "HF weights already exist at ${hf_dir}, skipping merge."
    return 0
  fi
  if [ ! -d "${checkpoint_dir}" ]; then
    printf 'CHECKPOINT_DIR does not exist: %s\n' "${checkpoint_dir}" >&2
    return 2
  fi

  mkdir -p "${hf_dir}"
  uenv_benchmark_model_log "Merging checkpoint ${checkpoint_dir} -> ${hf_dir}"
  podman run --rm \
    --entrypoint bash \
    --network host \
    --pids-limit=-1 \
    --shm-size="${CHECKPOINT_MERGE_SHM_SIZE:-64g}" \
    -v /data/ronghao:/data/ronghao \
    -v "${VERL_WORKSPACE:-/data/podman/verl/workspace}:/workspace" \
    -w /workspace/verl \
    -e CHECKPOINT_DIR="${checkpoint_dir}" \
    -e HF_DIR="${hf_dir}" \
    "${CHECKPOINT_MERGE_IMAGE:-localhost/uenv-bridge-verl:qwen35-torch210-vllm019-tf514-kernelfix}" \
    -lc 'set -euo pipefail
export PYTHONPATH=/workspace/verl:/data/ronghao/uenv/uenv-bridge/src:${PYTHONPATH:-}
export UENV_PATCH_VERL_TEXT_ONLY_PROCESSOR=1
python3 -m verl.model_merger merge \
  --backend fsdp \
  --use_cpu_initialization \
  --local_dir "${CHECKPOINT_DIR}" \
  --target_dir "${HF_DIR}"'
}

uenv_benchmark_ensure_hf_metadata() {
  local model_dir="$1"
  local base_model_dir="${BASE_MODEL_DIR:-/data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B}"
  local filename

  if [ ! -d "${base_model_dir}" ]; then
    return 0
  fi
  for filename in \
    config.json \
    generation_config.json \
    tokenizer.json \
    tokenizer_config.json \
    special_tokens_map.json \
    vocab.json \
    merges.txt \
    added_tokens.json \
    preprocessor_config.json \
    video_preprocessor_config.json \
    configuration.json
  do
    if [ ! -f "${model_dir}/${filename}" ] && [ -f "${base_model_dir}/${filename}" ]; then
      cp "${base_model_dir}/${filename}" "${model_dir}/${filename}"
    fi
  done
}

uenv_benchmark_model_cleanup() {
  if [ "${KEEP_SERVE:-0}" = "1" ]; then
    return 0
  fi
  if [ "${UENV_BENCHMARK_MODEL_GATEWAY_STARTED}" = "1" ] && [ -n "${UENV_BENCHMARK_MODEL_GATEWAY_PID}" ]; then
    kill "${UENV_BENCHMARK_MODEL_GATEWAY_PID}" >/dev/null 2>&1 || true
  fi
  if [ "${UENV_BENCHMARK_MODEL_VLLM_STARTED}" = "1" ] && [ -n "${UENV_BENCHMARK_MODEL_VLLM_CONTAINER}" ]; then
    podman rm -f "${UENV_BENCHMARK_MODEL_VLLM_CONTAINER}" >/dev/null 2>&1 || true
  fi
}

uenv_benchmark_register_model_cleanup() {
  if [ "${UENV_BENCHMARK_MODEL_CLEANUP_REGISTERED}" = "1" ]; then
    return 0
  fi
  trap uenv_benchmark_model_cleanup EXIT INT TERM
  UENV_BENCHMARK_MODEL_CLEANUP_REGISTERED=1
}

uenv_benchmark_start_vllm_and_gateway() {
  local model_dir="$1"
  local output_dir="$2"
  local model_name="${MODEL_NAME:-${UENV_ROLLOUT_MODEL_NAME:-Qwen/Qwen3.6-35B-A3B}}"
  local vllm_port="${VLLM_PORT:-18081}"
  local gateway_port="${GATEWAY_PORT:-18094}"
  local gateway_public_url="${MODEL_GATEWAY_PUBLIC_URL:-http://${MODEL_GATEWAY_PUBLIC_HOST:-10.10.20.142}:${gateway_port}/v1}"
  local upstream_url="http://127.0.0.1:${vllm_port}/v1"
  local gateway_log_path="${MODEL_GATEWAY_LOG_PATH:-${output_dir}/model-gateway.jsonl}"
  local gateway_stdout_path="${MODEL_GATEWAY_STDOUT_PATH:-${output_dir}/model-gateway.log}"
  local vllm_gpu_args=()
  local vllm_reasoning_args=()
  local vllm_tool_args=()
  local vllm_extra_args=()
  local gateway_args=()
  local gateway_thinking_budget="${GATEWAY_THINKING_BUDGET:-${THINKING_TOKEN_BUDGET:-}}"

  mkdir -p "${output_dir}"
  export UENV_ROLLOUT_MODEL_NAME="${model_name}"
  export UENV_ROLLOUT_MODEL_ENDPOINT="${gateway_public_url}"
  export MODEL_ENDPOINT="${gateway_public_url}"

  UENV_BENCHMARK_MODEL_VLLM_CONTAINER="${VLLM_CONTAINER_NAME:-uenv-benchmark-vllm-${vllm_port}}"

  if [ -n "${VLLM_PODMAN_GPU_ARGS:-nvidia.com/gpu=all}" ]; then
    vllm_gpu_args=(--device "${VLLM_PODMAN_GPU_ARGS:-nvidia.com/gpu=all}")
  fi
  if [ "${ENABLE_VLLM_REASONING:-1}" = "1" ]; then
    vllm_reasoning_args=(--reasoning-parser "${VLLM_REASONING_PARSER:-qwen3}")
    if [ -n "${VLLM_REASONING_CONFIG:-}" ]; then
      vllm_reasoning_args+=(--reasoning-config "${VLLM_REASONING_CONFIG}")
    else
      vllm_reasoning_args+=(--reasoning-config '{"reasoning_start_str":"<think>","reasoning_end_str":"</think>"}')
    fi
  fi
  if [ "${ENABLE_AUTO_TOOL_CHOICE:-0}" = "1" ]; then
    vllm_tool_args=(--enable-auto-tool-choice --tool-call-parser "${TOOL_CALL_PARSER:-qwen3_xml}")
  fi
  if [ -n "${VLLM_MAX_NUM_SEQS:-}" ]; then
    vllm_extra_args+=(--max-num-seqs "${VLLM_MAX_NUM_SEQS}")
  fi
  if [ -n "${VLLM_MAX_NUM_BATCHED_TOKENS:-}" ]; then
    vllm_extra_args+=(--max-num-batched-tokens "${VLLM_MAX_NUM_BATCHED_TOKENS}")
  fi

  uenv_benchmark_model_log "Starting vLLM container ${UENV_BENCHMARK_MODEL_VLLM_CONTAINER}"
  podman rm -f "${UENV_BENCHMARK_MODEL_VLLM_CONTAINER}" >/dev/null 2>&1 || true
  podman run -d --name "${UENV_BENCHMARK_MODEL_VLLM_CONTAINER}" \
    --entrypoint python3 \
    --network host \
    --pids-limit=-1 \
    --shm-size="${VLLM_SHM_SIZE:-64g}" \
    "${vllm_gpu_args[@]}" \
    -v /data/ronghao:/data/ronghao \
    -w "${REPO_DIR:-/data/ronghao/uenv/uenv-bridge}" \
    -e PYTHONPATH="${REPO_DIR:-/data/ronghao/uenv/uenv-bridge}/src" \
    -e UENV_PATCH_VERL_TEXT_ONLY_PROCESSOR=1 \
    "${VLLM_IMAGE:-localhost/vllm-openai:v0.19.0-cu130}" \
    -m vllm.entrypoints.openai.api_server \
    --model "${model_dir}" \
    --served-model-name "${model_name}" \
    --host 0.0.0.0 \
    --port "${vllm_port}" \
    --tensor-parallel-size "${TENSOR_PARALLEL_SIZE:-8}" \
    --max-model-len "${MAX_MODEL_LEN:-65536}" \
    --gpu-memory-utilization "${GPU_MEMORY_UTILIZATION:-0.90}" \
    "${vllm_reasoning_args[@]}" \
    "${vllm_tool_args[@]}" \
    "${vllm_extra_args[@]}" \
    --trust-remote-code
  UENV_BENCHMARK_MODEL_VLLM_STARTED=1
  uenv_benchmark_register_model_cleanup

  uenv_benchmark_wait_for_http "${upstream_url}/models" "vLLM" "${VLLM_READY_ATTEMPTS:-120}" "${VLLM_READY_SLEEP_SECONDS:-5}"

  gateway_args=(
    --upstream "${upstream_url}"
    --bind-host "${MODEL_GATEWAY_BIND_HOST:-0.0.0.0}"
    --port "${gateway_port}"
    --public-url "${gateway_public_url}"
    --request-timeout-seconds "${MODEL_GATEWAY_REQUEST_TIMEOUT_SECONDS:-7200}"
    --log-path "${gateway_log_path}"
  )
  if [ "${GATEWAY_ENABLE_THINKING:-${ENABLE_THINKING:-0}}" = "1" ]; then
    gateway_args+=(--enable-thinking)
  fi
  if [ "${GATEWAY_PRESERVE_THINKING:-${PRESERVE_THINKING:-0}}" = "1" ]; then
    gateway_args+=(--preserve-thinking)
  fi
  if [ "${GATEWAY_STRIP_REASONING:-0}" = "1" ]; then
    gateway_args+=(--strip-reasoning)
  fi
  if [ -n "${gateway_thinking_budget}" ]; then
    gateway_args+=(--thinking-token-budget "${gateway_thinking_budget}")
  fi
  if [ -n "${GATEWAY_MAX_TOKENS:-}" ]; then
    gateway_args+=(--max-tokens "${GATEWAY_MAX_TOKENS}")
  fi

  uenv_benchmark_model_log "Starting model gateway ${gateway_public_url} -> ${upstream_url}"
  nohup env PYTHONPATH="${REPO_DIR:-/data/ronghao/uenv/uenv-bridge}/src" \
    python3 "${REPO_DIR:-/data/ronghao/uenv/uenv-bridge}/scripts/benchmark/common/run_model_gateway.py" \
    "${gateway_args[@]}" \
    > "${gateway_stdout_path}" 2>&1 &
  UENV_BENCHMARK_MODEL_GATEWAY_PID=$!
  UENV_BENCHMARK_MODEL_GATEWAY_STARTED=1
  uenv_benchmark_register_model_cleanup

  uenv_benchmark_wait_for_http "http://127.0.0.1:${gateway_port}/v1/models" "model gateway" "${GATEWAY_READY_ATTEMPTS:-120}" "${GATEWAY_READY_SLEEP_SECONDS:-2}"
}

uenv_benchmark_prepare_model_endpoint() {
  local output_dir="$1"
  local checkpoint_dir=""
  local model_dir=""

  if [ -n "${MODEL_ENDPOINT:-}" ] && [ -z "${UENV_ROLLOUT_MODEL_ENDPOINT:-}" ]; then
    export UENV_ROLLOUT_MODEL_ENDPOINT="${MODEL_ENDPOINT}"
  fi
  if [ -n "${UENV_ROLLOUT_MODEL_ENDPOINT:-}" ]; then
    uenv_benchmark_model_log "Using existing model endpoint: ${UENV_ROLLOUT_MODEL_ENDPOINT}"
    return 0
  fi

  if [ -n "${MODEL_DIR:-}" ]; then
    model_dir="${MODEL_DIR}"
  elif [ -n "${CHECKPOINT_DIR:-}" ]; then
    checkpoint_dir="$(uenv_benchmark_resolve_actor_checkpoint_dir "${CHECKPOINT_DIR}")"
    if uenv_benchmark_has_hf_weights "${checkpoint_dir}"; then
      model_dir="${checkpoint_dir}"
    else
      HF_DIR="${HF_DIR:-${checkpoint_dir}/huggingface}"
      uenv_benchmark_merge_checkpoint "${checkpoint_dir}" "${HF_DIR}"
      uenv_benchmark_ensure_hf_metadata "${HF_DIR}"
      model_dir="${HF_DIR}"
    fi
    export CHECKPOINT_DIR="${checkpoint_dir}"
    export HF_DIR="${model_dir}"
  elif [ -n "${HF_DIR:-}" ]; then
    model_dir="${HF_DIR}"
  else
    return 0
  fi

  if ! uenv_benchmark_has_hf_weights "${model_dir}"; then
    printf 'No HuggingFace weights found in model dir: %s\n' "${model_dir}" >&2
    return 2
  fi

  uenv_benchmark_start_vllm_and_gateway "${model_dir}" "${output_dir}"
}
