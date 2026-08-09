#!/usr/bin/env bash
# User-facing VeRL entry. verl_runner.sh is the environment-neutral low-level
# runner; this file automates the stages while requiring the caller to name the
# task, data, model, and (for SWE) benchmark cases explicitly.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RELEASE_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
LOW_LEVEL="$SCRIPT_DIR/verl_runner.sh"
PREPARE_DATA="$SCRIPT_DIR/prepare_episode_data.py"
CLIENT_KIT="$SCRIPT_DIR/create_client_kit.sh"
PREPARE_SWE_RUNTIME="$RELEASE_ROOT/libexec/uenv/evaluation/prepare_swe.sh"

fail() {
  echo "错误：$*" >&2
  exit 1
}

usage() {
  cat <<'EOF'
UEnv + VeRL 训练入口

通用 process-plugin 任务（任务参数必须显式）：
  train_verl.sh run-task \
    --env-type NAME --dataset NAME --input FILE --max-steps N \
    --model /absolute/model/path --work-dir /absolute/output \
    --uenv-endpoint HOST:50051 --gpus N --steps N --rollouts N \
    --train-batch-size N --runtime docker|podman --image IMAGE

SWE 任务（catalog 和实例选择必须显式）：
  train_verl.sh run-swe \
    --catalog FILE --benchmark-variant smith --instance ID \
    --model /absolute/model/path \
    --max-iterations N --work-dir /absolute/output \
    --uenv-endpoint HOST:50051 --gpus N --steps N --rollouts N \
    --train-batch-size N --runtime docker|podman --image IMAGE

仅在完整 UEnv release 的服务主机上，可一次完成 SWE Runtime、OpenHands
与 Agent 准备（不会选择或下载任务实例镜像）：
  sudo train_verl.sh prepare-swe \
    --bundle ./uenv-linux-x86_64.tar.gz --profile single-node \
    --runtime docker --image-policy allow_public \
    --gateway 127.0.0.1:28999

公共选项：
  --model DIR                Hugging Face 模型目录（必填）
  --work-dir DIR             工作目录（run-task/run-swe 必填）
  --uenv-release DIR         已安装 release 或训练客户端包根目录
  --bundle FILE              兼容旧流程：从完整 release bundle 读取客户端资产
  --bridge-wheel FILE        仅提供 UEnv Bridge wheel
  --uenv-endpoint HOST:PORT  Adapter gRPC 地址（run-task/run-swe 必填）
  --gateway-public-url URL   双机时 UEnv 主机访问 VeRL 模型 API 的 URL
  --gateway-port PORT        VeRL 模型 API 监听端口（默认 18080）
  --gateway-bind HOST        监听地址；单机默认 127.0.0.1，双机默认 0.0.0.0
  --gpus N                   GPU 数（必填）
  --steps N                  训练步数（必填）
  --rollouts N               每个问题的轨迹数（必填）
  --train-batch-size N       每批问题数（必填）
  --runtime docker|podman    容器运行时（必填）
  --image IMAGE              VeRL CUDA 镜像或 digest（必填）
  --verl-config FILE         每行一个 Hydra KEY=VALUE；适合版本管理
  --set KEY=VALUE            追加 VeRL/Hydra 配置，可重复
  --print-effective-config   打印最终 Hydra 配置并退出，不启动训练
  --dry-run                  只准备并打印训练命令

run-task 必填任务选项：
  --input FILE               Episode JSONL
  --env-type NAME            环境类型
  --dataset NAME             数据集路由键
  --max-steps N              每个 Episode 的最大环境步数
  --limit N                  只取前 N 行

run-swe 必填任务选项：
  --catalog FILE             SWE catalog
  --benchmark-variant smith  当前训练适配器支持的 benchmark variant（必填）
  --instance ID              明确选择实例；可重复
  --limit N                  或按 catalog 顺序选择前 N 条；0 表示全部
  --max-iterations N         每条轨迹的 Agent 步数（必填）

底层命令（排障或高级配置时使用）：
  prepare-gpu / prepare-data / run
  prepare-swe / prepare-swe-uenv / prepare-swe-data
  export-client（仅完整 release：为远程 GPU 主机生成小型客户端包）

双机训练不要求 GPU 主机持有完整 release bundle。可先在已部署 UEnv 的
CPU 主机运行 `uenv train export-client`，再把生成的小型客户端包复制到 GPU 主机。
EOF
}

asset_args() {
  ASSET_ARGS=()
  if [[ -n "$BUNDLE" ]]; then
    ASSET_ARGS+=(--bundle "$BUNDLE")
  elif [[ -n "$UENV_RELEASE" ]]; then
    ASSET_ARGS+=(--uenv-release "$UENV_RELEASE")
  elif [[ -n "$BRIDGE_WHEEL" ]]; then
    ASSET_ARGS+=(--bridge-wheel "$BRIDGE_WHEEL")
  elif [[ -f "$RELEASE_ROOT/manifest.json" ]]; then
    ASSET_ARGS+=(--uenv-release "$RELEASE_ROOT")
  else
    fail "找不到训练客户端资产；请传 --uenv-release、--bundle 或 --bridge-wheel"
  fi
}

data_python() {
  local work_dir="$1" venv="$1/data-tools-venv"
  if python3 -c 'import pandas, pyarrow' >/dev/null 2>&1; then
    printf 'python3\n'
    return
  fi
  if [[ ! -x "$venv/bin/python" ]]; then
    echo "==> 首次准备训练数据工具" >&2
    if ! python3 -m venv "$venv"; then
      fail "无法创建 Python venv；Ubuntu/Debian 请先安装 python3-venv"
    fi
  fi
  if ! "$venv/bin/python" -c 'import pandas, pyarrow' >/dev/null 2>&1; then
    echo "==> 安装训练数据转换依赖" >&2
    "$venv/bin/python" -m pip install --disable-pip-version-check pandas pyarrow >&2 \
      || fail "训练数据依赖安装失败；修复网络后可直接重试同一命令"
  fi
  printf '%s\n' "$venv/bin/python"
}

parse_run_common() {
  MODEL=""
  WORK_DIR=""
  BUNDLE=""
  UENV_RELEASE=""
  BRIDGE_WHEEL=""
  UENV_ENDPOINT=""
  GATEWAY_URL=""
  GPUS=""
  STEPS=""
  ROLLOUTS=""
  TRAIN_BATCH=""
  RUNTIME=""
  IMAGE=""
  GATEWAY_PORT=""
  GATEWAY_BIND=""
  DRY_RUN=0
  VERL_CONFIG=""
  PRINT_EFFECTIVE_CONFIG=0
  EXTRA_HYDRA=()
}

validate_run_common() {
  [[ -n "$MODEL" ]] || fail "$1 需要 --model"
  [[ -n "$WORK_DIR" ]] || fail "$1 需要 --work-dir，以明确训练资产和结果位置"
  [[ -n "$UENV_ENDPOINT" ]] || fail "$1 需要 --uenv-endpoint HOST:PORT"
  [[ "$GPUS" =~ ^[1-9][0-9]*$ ]] || fail "$1 需要正整数 --gpus"
  [[ "$STEPS" =~ ^[1-9][0-9]*$ ]] || fail "$1 需要正整数 --steps"
  [[ "$ROLLOUTS" =~ ^[1-9][0-9]*$ ]] || fail "$1 需要正整数 --rollouts"
  [[ "$TRAIN_BATCH" =~ ^[1-9][0-9]*$ ]] || fail "$1 需要正整数 --train-batch-size"
  [[ "$RUNTIME" == docker || "$RUNTIME" == podman ]] \
    || fail "$1 需要 --runtime docker|podman"
  [[ -n "$IMAGE" ]] || fail "$1 需要 --image，生产使用建议填写不可变 digest"
}

append_run_common() {
  RUN_ARGS=(
    --work-dir "$WORK_DIR/.uenv-verl"
    --model "$MODEL"
    --data "$DATA_DIR"
    --uenv-endpoint "$UENV_ENDPOINT"
    --gpus "$GPUS"
    --steps "$STEPS"
    --rollouts "$ROLLOUTS"
    --train-batch-size "$TRAIN_BATCH"
  )
  [[ -n "$GATEWAY_URL" ]] && RUN_ARGS+=(--gateway-public-url "$GATEWAY_URL")
  [[ -n "$GATEWAY_PORT" ]] && RUN_ARGS+=(--gateway-port "$GATEWAY_PORT")
  [[ -n "$GATEWAY_BIND" ]] && RUN_ARGS+=(--gateway-bind "$GATEWAY_BIND")
  [[ -n "$RUNTIME" ]] && RUN_ARGS+=(--runtime "$RUNTIME")
  [[ -n "$IMAGE" ]] && RUN_ARGS+=(--image "$IMAGE")
  [[ -n "$VERL_CONFIG" ]] && RUN_ARGS+=(--verl-config "$VERL_CONFIG")
  [[ "$PRINT_EFFECTIVE_CONFIG" -eq 1 ]] && RUN_ARGS+=(--print-effective-config)
  [[ "$DRY_RUN" -eq 1 ]] && RUN_ARGS+=(--dry-run)
  local item
  for item in "${EXTRA_HYDRA[@]}"; do
    RUN_ARGS+=(--set "$item")
  done
}

run_task() {
  parse_run_common
  local input="" env_type="" dataset="" max_steps="" limit=""
  while (($#)); do
    case "$1" in
      --model) MODEL="${2:-}"; shift 2 ;;
      --work-dir) WORK_DIR="${2:-}"; shift 2 ;;
      --uenv-release) UENV_RELEASE="${2:-}"; shift 2 ;;
      --bundle) BUNDLE="${2:-}"; shift 2 ;;
      --bridge-wheel) BRIDGE_WHEEL="${2:-}"; shift 2 ;;
      --uenv-endpoint) UENV_ENDPOINT="${2:-}"; shift 2 ;;
      --gateway-public-url) GATEWAY_URL="${2:-}"; shift 2 ;;
      --gateway-port) GATEWAY_PORT="${2:-}"; shift 2 ;;
      --gateway-bind) GATEWAY_BIND="${2:-}"; shift 2 ;;
      --gpus) GPUS="${2:-}"; shift 2 ;;
      --steps) STEPS="${2:-}"; shift 2 ;;
      --rollouts) ROLLOUTS="${2:-}"; shift 2 ;;
      --train-batch-size) TRAIN_BATCH="${2:-}"; shift 2 ;;
      --runtime) RUNTIME="${2:-}"; shift 2 ;;
      --image) IMAGE="${2:-}"; shift 2 ;;
      --verl-config) VERL_CONFIG="${2:-}"; shift 2 ;;
      --set) EXTRA_HYDRA+=("${2:-}"); shift 2 ;;
      --print-effective-config) PRINT_EFFECTIVE_CONFIG=1; shift ;;
      --dry-run) DRY_RUN=1; shift ;;
      --input) input="${2:-}"; shift 2 ;;
      --env-type) env_type="${2:-}"; shift 2 ;;
      --dataset) dataset="${2:-}"; shift 2 ;;
      --max-steps) max_steps="${2:-}"; shift 2 ;;
      --limit) limit="${2:-}"; shift 2 ;;
      -h|--help) usage; exit 0 ;;
      *) fail "run-task 未知参数：$1" ;;
    esac
  done
  validate_run_common run-task
  [[ -n "$input" ]] || fail "run-task 需要 --input；UEnv 不会选择内置任务数据"
  [[ -n "$env_type" ]] || fail "run-task 需要 --env-type"
  [[ -n "$dataset" ]] || fail "run-task 需要 --dataset"
  [[ "$max_steps" =~ ^[1-9][0-9]*$ ]] || fail "run-task 需要正整数 --max-steps"
  [[ "$max_steps" -eq 1 ]] \
    || fail "run-task 当前只支持单步 process plugin 训练；多步环境需要专用 token-trace Bridge"
  mkdir -p "$WORK_DIR"
  WORK_DIR="$(cd "$WORK_DIR" && pwd)"
  DATA_DIR="$WORK_DIR/episode-data"
  asset_args

  echo "==> 准备固定提交的 VeRL 源码与 UEnv Bridge"
  bash "$LOW_LEVEL" prepare-gpu "${ASSET_ARGS[@]}" --work-dir "$WORK_DIR/.uenv-verl"

  echo "==> 将 Episode JSONL 转为 VeRL Parquet"
  local python_bin
  python_bin="$(data_python "$WORK_DIR")"
  local -a data_args=(
    --input "$input" --output-dir "$DATA_DIR"
    --env-type "$env_type" --dataset "$dataset" --max-steps "$max_steps"
  )
  [[ -n "$limit" ]] && data_args+=(--limit "$limit")
  "$python_bin" "$PREPARE_DATA" "${data_args[@]}"

  echo "==> 启动 VeRL 训练"
  append_run_common
  bash "$LOW_LEVEL" run \
    --env-type "$env_type" \
    "${RUN_ARGS[@]}"
}

run_swe() {
  parse_run_common
  local catalog="" benchmark_variant="" limit="" max_iterations=""
  local -a instances=()
  while (($#)); do
    case "$1" in
      --model) MODEL="${2:-}"; shift 2 ;;
      --work-dir) WORK_DIR="${2:-}"; shift 2 ;;
      --uenv-release) UENV_RELEASE="${2:-}"; shift 2 ;;
      --bundle) BUNDLE="${2:-}"; shift 2 ;;
      --bridge-wheel) BRIDGE_WHEEL="${2:-}"; shift 2 ;;
      --uenv-endpoint) UENV_ENDPOINT="${2:-}"; shift 2 ;;
      --gateway-public-url) GATEWAY_URL="${2:-}"; shift 2 ;;
      --gateway-port) GATEWAY_PORT="${2:-}"; shift 2 ;;
      --gateway-bind) GATEWAY_BIND="${2:-}"; shift 2 ;;
      --gpus) GPUS="${2:-}"; shift 2 ;;
      --steps) STEPS="${2:-}"; shift 2 ;;
      --rollouts) ROLLOUTS="${2:-}"; shift 2 ;;
      --train-batch-size) TRAIN_BATCH="${2:-}"; shift 2 ;;
      --runtime) RUNTIME="${2:-}"; shift 2 ;;
      --image) IMAGE="${2:-}"; shift 2 ;;
      --verl-config) VERL_CONFIG="${2:-}"; shift 2 ;;
      --set) EXTRA_HYDRA+=("${2:-}"); shift 2 ;;
      --print-effective-config) PRINT_EFFECTIVE_CONFIG=1; shift ;;
      --dry-run) DRY_RUN=1; shift ;;
      --catalog) catalog="${2:-}"; shift 2 ;;
      --benchmark-variant) benchmark_variant="${2:-}"; shift 2 ;;
      --instance) instances+=("${2:-}"); shift 2 ;;
      --limit) limit="${2:-}"; shift 2 ;;
      --max-iterations) max_iterations="${2:-}"; shift 2 ;;
      -h|--help) usage; exit 0 ;;
      *) fail "run-swe 未知参数：$1" ;;
    esac
  done
  validate_run_common run-swe
  [[ -n "$catalog" ]] || fail "run-swe 需要 --catalog；UEnv 不会选择内置 benchmark"
  [[ "$benchmark_variant" == "smith" ]] \
    || fail "run-swe 需要 --benchmark-variant smith；当前训练适配器只支持 Smith"
  if [[ ${#instances[@]} -eq 0 && -z "$limit" ]]; then
    fail "run-swe 需要 --instance ID（可重复）或 --limit N；0 表示全部"
  fi
  if [[ ${#instances[@]} -gt 0 && -n "$limit" ]]; then
    fail "run-swe 的 --instance 与 --limit 二选一，不能同时使用"
  fi
  [[ -z "$limit" || "$limit" =~ ^[0-9]+$ ]] || fail "--limit 必须是非负整数"
  [[ "$max_iterations" =~ ^[1-9][0-9]*$ ]] \
    || fail "run-swe 需要正整数 --max-iterations"
  mkdir -p "$WORK_DIR"
  WORK_DIR="$(cd "$WORK_DIR" && pwd)"
  DATA_DIR="$WORK_DIR/swe-data"
  asset_args

  echo "==> 准备固定提交的 VeRL 源码与 UEnv Bridge"
  bash "$LOW_LEVEL" prepare-gpu "${ASSET_ARGS[@]}" --work-dir "$WORK_DIR/.uenv-verl"

  echo "==> 准备 SWE-smith 训练 Parquet"
  local -a swe_data_args=(
    --catalog "$catalog" --output-dir "$DATA_DIR"
    --benchmark-variant "$benchmark_variant"
    --max-iterations "$max_iterations"
  )
  local instance
  for instance in "${instances[@]}"; do
    swe_data_args+=(--instance "$instance")
  done
  [[ ${#instances[@]} -eq 0 ]] && swe_data_args+=(--limit "$limit")
  [[ -n "$RUNTIME" ]] && swe_data_args+=(--runtime "$RUNTIME")
  [[ -n "$IMAGE" ]] && swe_data_args+=(--image "$IMAGE")
  bash "$LOW_LEVEL" prepare-data "${swe_data_args[@]}"

  echo "==> 启动 VeRL 训练"
  append_run_common
  bash "$LOW_LEVEL" run --env-type swe "${RUN_ARGS[@]}"
}

prepare_swe() {
  local -a runtime_args=()
  local profile="" server="" trajectory_endpoint=""
  while (($#)); do
    case "$1" in
      --bundle|--installer|--runtime|--image-policy|--gateway|--gateway-public|--advertise|--shared-key-file)
        [[ $# -ge 2 && -n "${2:-}" ]] || fail "$1 缺少参数"
        runtime_args+=("$1" "$2")
        shift 2
        ;;
      --profile)
        [[ $# -ge 2 && -n "${2:-}" ]] || fail "$1 缺少参数"
        profile="$2"
        runtime_args+=("$1" "$2")
        shift 2
        ;;
      --server)
        [[ $# -ge 2 && -n "${2:-}" ]] || fail "$1 缺少参数"
        server="$2"
        runtime_args+=("$1" "$2")
        shift 2
        ;;
      --trajectory-endpoint)
        [[ $# -ge 2 && -n "${2:-}" ]] || fail "$1 缺少参数"
        trajectory_endpoint="$2"
        runtime_args+=("$1" "$2")
        shift 2
        ;;
      --force-swe-config|--reset-swe-key)
        runtime_args+=("$1")
        shift
        ;;
      -h|--help)
        bash "$PREPARE_SWE_RUNTIME" --help
        return
        ;;
      *) fail "prepare-swe 未知参数：$1" ;;
    esac
  done
  [[ -f "$PREPARE_SWE_RUNTIME" ]] || fail "找不到 SWE runtime 准备脚本：$PREPARE_SWE_RUNTIME"
  bash "$PREPARE_SWE_RUNTIME" "${runtime_args[@]}"
  [[ "$profile" != "control-plane" ]] || return 0
  local -a agent_args=(
    prepare-uenv --uenv-release /opt/uenv/current --skip-openhands
    --profile "$profile"
  )
  [[ -n "$server" ]] && agent_args+=(--server "$server")
  [[ -n "$trajectory_endpoint" ]] \
    && agent_args+=(--trajectory-endpoint "$trajectory_endpoint")
  bash "$LOW_LEVEL" "${agent_args[@]}"
}

command_name="${1:-help}"
if (($#)); then
  shift
fi
case "$command_name" in
  run-task) run_task "$@" ;;
  run-swe) run_swe "$@" ;;
  prepare-swe) prepare_swe "$@" ;;
  prepare-gpu) exec bash "$LOW_LEVEL" prepare-gpu "$@" ;;
  prepare-data) exec python3 "$PREPARE_DATA" "$@" ;;
  run)
    exec bash "$LOW_LEVEL" run "$@"
    ;;
  prepare-swe-uenv) exec bash "$LOW_LEVEL" prepare-uenv "$@" ;;
  prepare-swe-data) exec bash "$LOW_LEVEL" prepare-data "$@" ;;
  export-client)
    [[ -f "$CLIENT_KIT" ]] \
      || fail "export-client 只在完整 UEnv release 的服务主机上提供"
    exec bash "$CLIENT_KIT" "$@"
    ;;
  help|-h|--help) usage ;;
  *) fail "未知命令：$command_name" ;;
esac
