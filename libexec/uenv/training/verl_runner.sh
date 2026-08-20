#!/usr/bin/env bash
# Low-level VeRL GRPO runner shared by QA, process plugins, and SWE.
set -euo pipefail

VERL_REPOSITORY="https://github.com/verl-project/verl.git"
VERL_VERSION="v0.7.1"
VERL_COMMIT="bec9ef74768dd201881cd4e54cd0385e87caae27"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPT_RELEASE="$(cd "$SCRIPT_DIR/../../.." && pwd)"

usage() {
  cat <<'EOF'
UEnv + VeRL 底层训练入口

CPU/UEnv 主机（只需一次）：
  sudo bash train_verl.sh prepare-uenv --uenv-release /opt/uenv/current

训练设备主机（准备固定版本 VeRL 和 Bridge）：
  bash train_verl.sh prepare-gpu \
    --uenv-release /opt/uenv/current --work-dir /data/uenv-run/.uenv-verl

准备数据：
  bash train_verl.sh prepare-data \
    --catalog /data/tasks/swe-catalog.json --benchmark-variant smith \
    --instance INSTANCE_ID \
    --output-dir /data/uenv-run/swe-data --max-iterations 30 \
    --runtime docker --image docker.io/verlai/verl:vllm017.latest

运行（任务类型、连接和训练规模均须明确）：
  bash train_verl.sh run --uenv-release /opt/uenv/current \
    --env-type swe --model /data/models/MODEL --data /data/uenv-run/swe-data \
    --uenv-endpoint 127.0.0.1:50051 --gpus 1 --steps 1 \
    --rollouts 2 --train-batch-size 1 --runtime docker \
    --image docker.io/verlai/verl:vllm017.latest

双机运行时，在上述命令中把 --uenv-endpoint 改为 CPU/UEnv 地址，
并显式增加 --gateway-public-url 和 --gateway-bind。

prepare-uenv 选项：
  --uenv-release DIR       已安装的 UEnv release（默认 /opt/uenv/current）
  --profile PROFILE        single-node、full 或 worker（默认 single-node）
  --server HOST:PORT       OpenHands Agent 连接的 UEnv Server gRPC 地址（默认 127.0.0.1:50051）
  --trajectory-endpoint URL  OpenHands Agent 上传交互轨迹的 UEnv Server URL
  --openhands-dir DIR      OpenHands 安装目录
  --skip-openhands         使用已有 OpenHands，不执行安装器

prepare-gpu / run 公共选项：
  --bundle FILE            从 release 包提取 Bridge wheel、配置和启动器
  --uenv-release DIR       从已安装 release 读取上述文件
  --bridge-wheel FILE      仅提供 Bridge wheel；脚本生成最小配置
  --work-dir DIR           VeRL 与运行资产目录（默认 ./.uenv-verl）

prepare-data 选项：
  --catalog FILE           UEnv SWE catalog JSON（必填）
  --benchmark-variant smith  当前数据转换器支持的 variant（必填）
  --output-dir DIR         训练数据输出目录（必填）
  --limit N                最多选择 N 个实例；0 表示全部（与 --instance 二选一）
  --instance ID            只选择指定实例，可重复
  --max-iterations N       每条轨迹最大 Agent 步数（必填）
  --runtime docker|podman  主机缺 pyarrow 时使用的容器运行时
  --image IMAGE            数据准备容器；使用容器准备数据时必填

run 选项：
  --model DIR              Hugging Face 模型目录（必填）
  --data DIR               含 train.parquet/test.parquet 的目录（必填）
  --env-type NAME          本批训练数据对应的 UEnv 环境类型（必填）
  --uenv-endpoint HOST:PORT  UEnv Server 地址（必填）
  --gateway-public-url URL CPU/UEnv 主机可访问的 GPU 模型网关 URL
  --gateway-port PORT      GPU 模型网关监听端口（默认 18080）
  --gateway-bind HOST      监听地址；单机默认 127.0.0.1，双机默认 0.0.0.0
  --gpus N                 单节点训练设备数（必填；历史参数名保留）
  --steps N                训练步数（必填）
  --rollouts N             每个问题的轨迹数，至少为 2（必填）
  --train-batch-size N     每批问题数（必填）
  --runtime docker|podman  容器运行时（必填）
  --image IMAGE            VeRL 镜像或 digest（必填）
  --device-backend cuda|ascend
                           本机训练设备后端（默认 cuda）
  --ascend-devices LIST    Ascend 可见设备，如 0,1,2,3；默认取 ASCEND_VISIBLE_DEVICES 或 0
  --verl-config FILE       每行一个 Hydra KEY=VALUE；空行和 # 注释忽略
  --set KEY=VALUE          追加一个 Hydra 覆盖，可重复
  --print-effective-config 打印合并后的覆盖列表并退出
  --dry-run                完成校验并打印容器命令，不启动训练

可选调度环境变量（空值表示不发送提示）：
  UENV_MAX_EPISODE_CONCURRENCY、UENV_MAX_IN_FLIGHT_BATCHES
  UENV_TARGET_WORKER_SLOTS、UENV_POOL_WARMUP_TARGET
  UENV_MAX_PARALLEL_PER_WORKER、UENV_AGENT_JOB_MAX_CONCURRENCY
  UENV_RUNTIME_GATEWAY_SESSION_LIMIT、UENV_REQUIRE_WARM_SLOT
  UENV_EXPECTED_WORKER_PARALLELISM、UENV_ADAPTER_CORE_GRPC_MAX_MESSAGE_BYTES

此入口固定 VeRL 源码到 v0.7.1 / bec9ef74768dd201881cd4e54cd0385e87caae27，
不会修改 VeRL 源码。训练依赖本地 Worker catalog，不需要 UEnv Hub。
EOF
}

fail() {
  echo "错误：$*" >&2
  exit 1
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || fail "缺少命令：$1"
}

normalize_device_backend() {
  local value
  value="$(printf '%s' "${1:-cuda}" | tr '[:upper:]' '[:lower:]')"
  case "$value" in
    cuda|gpu|nvidia) printf 'cuda\n' ;;
    ascend|npu|910c) printf 'ascend\n' ;;
    *) fail "--device-backend 必须是 cuda 或 ascend" ;;
  esac
}

append_ascend_container_args() {
  local -n target="$1"
  local device
  for device in /dev/davinci_manager /dev/devmm_svm /dev/hisi_hdc; do
    [[ -e "$device" ]] && target+=(--device "$device")
  done
  for device in /dev/davinci[0-9]*; do
    [[ -e "$device" ]] && target+=(--device "$device")
  done
  [[ -d /usr/local/Ascend/driver ]] \
    && target+=(-v /usr/local/Ascend/driver:/usr/local/Ascend/driver:ro)
}

absolute_file() {
  local path="$1"
  [[ -f "$path" ]] || fail "文件不存在：$path"
  (cd "$(dirname "$path")" && printf '%s/%s\n' "$PWD" "$(basename "$path")")
}

absolute_dir() {
  local path="$1"
  [[ -d "$path" ]] || fail "目录不存在：$path"
  (cd "$path" && printf '%s\n' "$PWD")
}

write_agent_loop_config() {
  local target="$1"
  cat > "$target" <<'EOF'
- name: uenv_agent
  _target_: uenv.bridge.verl_agent_loop.UEnvAgentLoop
  mode: ${oc.env:UENV_AGENT_LOOP_CLIENT,rust_core}
  endpoint: ${oc.env:UENV_ADAPTER_CORE_ENDPOINT,127.0.0.1:50051}
  timeout_seconds: ${oc.env:UENV_AGENT_LOOP_TIMEOUT_SECONDS,3600}
  max_message_bytes: ${oc.env:UENV_ADAPTER_CORE_GRPC_MAX_MESSAGE_BYTES,16777216}
  startup_timeout_seconds: 60
  auto_start: false
  default_env_type: ${oc.env:UENV_DEFAULT_ENV_TYPE,""}
  default_model_endpoint: ""
  default_model_name: ""
  default_max_steps: 10
  default_max_turns: 1
  seed_base: 42
  request_record_path: ${oc.env:UENV_AGENT_LOOP_REQUEST_RECORD_PATH,""}
  result_record_path: ${oc.env:UENV_AGENT_LOOP_RESULT_RECORD_PATH,""}
  model_gateway_enabled: true
  model_gateway_bind_host: ${oc.env:UENV_MODEL_GATEWAY_BIND_HOST,0.0.0.0}
  model_gateway_port: ${oc.env:UENV_MODEL_GATEWAY_PORT,18080}
  model_gateway_public_url: ${oc.env:UENV_MODEL_GATEWAY_PUBLIC_URL,""}
  model_gateway_stop_on_close: true
  require_swe_response_trace: true
  failed_episode_policy: raise
  parallel_mode: sync
  expected_worker_parallelism: ${oc.env:UENV_EXPECTED_WORKER_PARALLELISM,""}
  max_episode_concurrency: ${oc.env:UENV_MAX_EPISODE_CONCURRENCY,""}
  max_in_flight_batches: ${oc.env:UENV_MAX_IN_FLIGHT_BATCHES,""}
  target_worker_slots: ${oc.env:UENV_TARGET_WORKER_SLOTS,""}
  pool_warmup_target: ${oc.env:UENV_POOL_WARMUP_TARGET,""}
  max_parallel_per_worker: ${oc.env:UENV_MAX_PARALLEL_PER_WORKER,""}
  agent_job_max_concurrency: ${oc.env:UENV_AGENT_JOB_MAX_CONCURRENCY,""}
  runtime_gateway_session_limit: ${oc.env:UENV_RUNTIME_GATEWAY_SESSION_LIMIT,""}
  require_warm_slot: ${oc.env:UENV_REQUIRE_WARM_SLOT,false}
  batch_size: 0
EOF
}

prepare_uenv() {
  local release="/opt/uenv/current"
  local profile="single-node"
  local server="127.0.0.1:50051"
  local trajectory_endpoint=""
  local openhands_dir="/opt/uenv/agent/openhands-benchmarks"
  local health_url="${UENV_SWE_AGENT_HEALTH_URL:-http://127.0.0.1:8777/health}"
  local skip_openhands=0
  while (($#)); do
    case "$1" in
      --uenv-release) release="${2:-}"; shift 2 ;;
      --profile) profile="${2:-}"; shift 2 ;;
      --server) server="${2:-}"; shift 2 ;;
      --trajectory-endpoint) trajectory_endpoint="${2:-}"; shift 2 ;;
      --openhands-dir) openhands_dir="${2:-}"; shift 2 ;;
      --skip-openhands) skip_openhands=1; shift ;;
      -h|--help) usage; exit 0 ;;
      *) fail "prepare-uenv 未知参数：$1" ;;
    esac
  done

  [[ "${EUID:-$(id -u)}" -eq 0 ]] || fail "prepare-uenv 需要 sudo"
  release="$(absolute_dir "$release")"
  case "$profile" in
    single-node|full|worker) ;;
    *) fail "prepare-uenv --profile 必须是 single-node、full 或 worker" ;;
  esac
  [[ "$server" =~ ^[A-Za-z0-9._:-]+$ ]] || fail "--server 应为 HOST:PORT"
  if [[ -z "$trajectory_endpoint" ]]; then
    if [[ "$profile" == "worker" ]]; then
      fail "worker 需要 --trajectory-endpoint http://<CONTROL_PLANE>:8077"
    fi
    trajectory_endpoint="http://127.0.0.1:8077"
  fi
  [[ "$trajectory_endpoint" =~ ^https?://[^[:space:]]+$ ]] \
    || fail "--trajectory-endpoint 必须是 http(s) URL"
  [[ -f /etc/uenv/swe.env ]] || fail "SWE 尚未启用；请用 install.sh --enable-swe 安装或升级 UEnv"
  [[ -f "$release/systemd/uenv-swe-agent.service" ]] || fail "release 缺少 uenv-swe-agent.service"
  [[ -f "$release/share/swe/openhands-runner.py" ]] || fail "release 缺少 OpenHands poller"
  [[ -f "$release/share/swe/openhands/run_swebenchpro_official.py" ]] || fail "release 缺少 OpenHands driver"
  [[ -f "$release/libexec/uenv/swe/run_agent_job.sh" ]] || fail "release 缺少通用 AgentJob runner"

  local installer="$release/libexec/uenv/swe/install_openhands.sh"
  local pin="$release/share/swe/PIN.md"
  if [[ "$skip_openhands" -eq 0 ]]; then
    [[ -x "$installer" || -f "$installer" ]] || fail "release 缺少 install_openhands.sh"
    [[ -f "$pin" ]] || fail "release 缺少 OpenHands PIN.md"
    bash "$installer" --install-dir "$openhands_dir" --pin "$pin"
  fi

  local openhands_python="$openhands_dir/.venv/bin/python"
  [[ -x "$openhands_python" ]] || fail "OpenHands Python 不存在：$openhands_python"
  env PYTHONPATH="$release/share/swe/openhands:$release/share/swe/openhands/uenv_runtime/gen:$openhands_dir${PYTHONPATH:+:$PYTHONPATH}" \
    "$openhands_python" -c \
    'from benchmarks.utils.llm_config import load_llm_config; from openhands.sdk import Agent, Conversation; from uenv_runtime.agent_client import _load_grpc_modules; _load_grpc_modules()'

  id uenv-agent >/dev/null 2>&1 || fail "缺少 uenv-agent 用户；请重新运行 OpenHands 安装器"
  install -d -o uenv-agent -g uenv -m 0750 /var/lib/uenv/agent /var/lib/uenv/agent/runs /opt/uenv/agent
  install -o root -g uenv -m 0755 "$release/libexec/uenv/swe/run_agent_job.sh" /opt/uenv/agent/run-agent-job.sh

  local temporary
  temporary="$(mktemp -d -t uenv-verl-agent.XXXXXXXX)"
  cat > "$temporary/openhands-llm.json" <<'EOF'
{
  "model": "openai/uenv-policy",
  "base_url": "http://127.0.0.1:1/v1",
  "api_key": "EMPTY",
  "temperature": 1.0,
  "max_output_tokens": 4096,
  "timeout": 1200,
  "request_timeout": 1200
}
EOF
  cat > "$temporary/swe-agent.env" <<EOF
OPENHANDS_AGENT_POLL=1
UENV_SERVER_ENDPOINT=$server
OPENHANDS_AGENT_POOL_ID=openhands-default
OPENHANDS_AGENT_BRIDGE_ID=uenv-agent-openhands
OPENHANDS_AGENT_BRIDGE_VERSION=1.0.0
OPENHANDS_AGENT_MAX_CONCURRENT=1
OPENHANDS_RUNNER_API_BIND=127.0.0.1:8888
OPENHANDS_RUNNER_HEALTH_BIND=127.0.0.1:8777
OPENHANDS_RUN_SCRIPT=/opt/uenv/agent/run-agent-job.sh
OPENHANDS_RUNS_DIR=/var/lib/uenv/agent/runs
OPENHANDS_BENCHMARKS_DIR=$openhands_dir
OPENHANDS_LLM_TEMPLATE=/etc/uenv/openhands-llm.json
UENV_AGENT_BRIDGE_DIR=$release/share/swe/openhands
UENV_RELEASE=$release
UENV_TRAJECTORY_ENDPOINT=$trajectory_endpoint
EOF
  install -o root -g uenv -m 0640 "$temporary/openhands-llm.json" /etc/uenv/openhands-llm.json
  install -o root -g uenv -m 0640 "$temporary/swe-agent.env" /etc/uenv/swe-agent.env
  install -m 0644 "$release/systemd/uenv-swe-agent.service" /etc/systemd/system/uenv-swe-agent.service
  rm -rf -- "$temporary"

  require_command systemctl
  if [[ "$profile" == "single-node" || "$profile" == "full" ]]; then
    systemctl is-active --quiet uenv-adapter-core.service \
      || fail "UEnv Server（服务名 uenv-adapter-core.service）未运行"
  fi
  systemctl is-active --quiet uenv-worker.service || fail "uenv-worker 未运行"
  systemctl daemon-reload
  systemctl enable uenv-swe-agent.service >/dev/null
  systemctl restart uenv-swe-agent.service
  if ! systemctl is-active --quiet uenv-swe-agent.service; then
    systemctl --no-pager --full status uenv-swe-agent.service || true
    fail "uenv-swe-agent 启动失败"
  fi
  if ! python3 - "$health_url" "$server" <<'PY'
import json
import sys
import time
import urllib.request

url, expected_server = sys.argv[1:]
deadline = time.monotonic() + 45
last_error = "no response"
while time.monotonic() < deadline:
    try:
        with urllib.request.urlopen(url, timeout=3) as response:
            document = json.load(response)
        if (
            isinstance(document, dict)
            and document.get("registered") is True
            and document.get("server_endpoint") == expected_server
        ):
            print("OpenHands Agent registration passed")
            raise SystemExit(0)
        last_error = f"runner not registered; health={document!r}"
    except Exception as exc:
        last_error = str(exc)
    time.sleep(2)
raise SystemExit(f"Agent local registration health failed at {url}: {last_error}")
PY
  then
    systemctl --no-pager --full status uenv-swe-agent.service || true
    echo "查看日志：journalctl -u uenv-swe-agent.service -n 100 --no-pager" >&2
    fail "OpenHands Agent 未注册到 UEnv Server"
  fi
  echo "UEnv SWE Agent 已就绪；Hub 未启用，也不是此训练流程的依赖。"
}

extract_release() {
  local bundle="$1" work_dir="$2"
  bundle="$(absolute_file "$bundle")"
  if tar -tzf "$bundle" | awk '/(^\/|(^|\/)\.\.($|\/))/ { bad=1 } END { exit bad ? 0 : 1 }'; then
    fail "bundle 包含不安全路径"
  fi
  require_command sha256sum
  local temporary payload version digest target
  temporary="$(mktemp -d -t uenv-verl-release.XXXXXXXX)"
  tar -xzf "$bundle" --no-same-owner -C "$temporary"
  payload="$(find "$temporary" -mindepth 1 -maxdepth 2 -type f -name manifest.json -printf '%h\n' | head -n1)"
  [[ -n "$payload" && -f "$payload/VERSION" ]] || fail "bundle 缺少 manifest.json 或 VERSION"
  version="$(tr -d '[:space:]' < "$payload/VERSION")"
  [[ "$version" =~ ^[0-9A-Za-z._+-]+$ ]] || fail "bundle 版本非法"
  digest="$(sha256sum "$bundle" | awk '{print substr($1, 1, 12)}')"
  [[ "$digest" =~ ^[0-9a-f]{12}$ ]] || fail "无法计算 bundle SHA-256"
  target="$work_dir/releases/$version-$digest"
  if [[ ! -f "$target/manifest.json" ]]; then
    install -d "$target"
    cp -a "$payload/." "$target/"
  fi
  rm -rf -- "$temporary"
  printf '%s\n' "$target"
}

ensure_verl_checkout() {
  local work_dir="$1"
  local checkout="$work_dir/verl"
  require_command git
  if [[ ! -d "$checkout/.git" ]]; then
    install -d "$checkout"
    git -C "$checkout" init -q
    git -C "$checkout" remote add origin "$VERL_REPOSITORY"
    git -C "$checkout" fetch --depth 1 origin "$VERL_COMMIT"
    git -C "$checkout" checkout --detach -q FETCH_HEAD
  fi
  local actual origin
  actual="$(git -C "$checkout" rev-parse HEAD 2>/dev/null || true)"
  origin="$(git -C "$checkout" remote get-url origin 2>/dev/null || true)"
  [[ "$origin" == "$VERL_REPOSITORY" ]] || fail "$checkout 不是预期 VeRL 仓库"
  if [[ "$actual" != "$VERL_COMMIT" ]]; then
    echo "==> 恢复未完成的 VeRL 下载" >&2
    git -C "$checkout" fetch --depth 1 origin "$VERL_COMMIT"
    git -C "$checkout" checkout --detach -q FETCH_HEAD
    actual="$(git -C "$checkout" rev-parse HEAD 2>/dev/null || true)"
  fi
  [[ "$actual" == "$VERL_COMMIT" ]] \
    || fail "$checkout 无法切换到 $VERL_VERSION 固定提交"
  printf '%s\n' "$checkout"
}

stage_gpu_assets() {
  local work_dir="$1" bundle="$2" release="$3" wheel="$4"
  install -d "$work_dir" "$work_dir/assets" "$work_dir/releases" "$work_dir/output"
  if [[ -n "$bundle" ]]; then
    release="$(extract_release "$bundle" "$work_dir")"
  elif [[ -z "$release" && -f "$SCRIPT_RELEASE/manifest.json" ]]; then
    release="$SCRIPT_RELEASE"
  fi
  if [[ -n "$release" ]]; then
    release="$(absolute_dir "$release")"
  fi

  if [[ -z "$wheel" && -n "$release" ]]; then
    [[ -d "$release/wheels" ]] || fail "release 缺少 wheels 目录"
    wheel="$(find "$release/wheels" -maxdepth 1 -type f \( -name 'uenv_bridge-*.whl' -o -name 'uenv-bridge-*.whl' \) -print | head -n1)"
  fi
  [[ -n "$wheel" ]] || fail "找不到 Bridge wheel；请传 --bundle、--uenv-release 或 --bridge-wheel"
  wheel="$(absolute_file "$wheel")"
  local staged_wheel="$work_dir/assets/$(basename "$wheel")"
  if [[ "$wheel" != "$staged_wheel" ]]; then
    install -m 0644 "$wheel" "$staged_wheel"
  fi

  local config="$work_dir/assets/uenv-agent-loop.yaml"
  if [[ -n "$release" && -f "$release/share/uenv-bridge/configs/uenv-agent-loop.yaml" ]]; then
    install -m 0644 "$release/share/uenv-bridge/configs/uenv-agent-loop.yaml" "$config"
  else
    write_agent_loop_config "$config"
  fi

  local runner=""
  if [[ -n "$release" && -f "$release/share/uenv-bridge/scripts/run_verl_main_ppo.py" ]]; then
    runner="$work_dir/assets/run_verl_main_ppo.py"
    install -m 0755 "$release/share/uenv-bridge/scripts/run_verl_main_ppo.py" "$runner"
  fi
  local state="$work_dir/assets/state.env"
  {
    printf 'UENV_BRIDGE_WHEEL=%q\n' "$staged_wheel"
    printf 'UENV_AGENT_CONFIG=%q\n' "$config"
    printf 'UENV_VERL_RUNNER=%q\n' "$runner"
    printf 'UENV_ASSET_RELEASE=%q\n' "$release"
  } > "$state"
  ensure_verl_checkout "$work_dir" >/dev/null
  echo "GPU 侧资产已就绪：$work_dir"
  echo "VeRL：$VERL_VERSION ($VERL_COMMIT)"
}

parse_asset_options() {
  BUNDLE=""
  UENV_RELEASE=""
  BRIDGE_WHEEL=""
  WORK_DIR="${UENV_VERL_WORK_DIR:-$PWD/.uenv-verl}"
}

prepare_gpu() {
  parse_asset_options
  while (($#)); do
    case "$1" in
      --bundle) BUNDLE="${2:-}"; shift 2 ;;
      --uenv-release) UENV_RELEASE="${2:-}"; shift 2 ;;
      --bridge-wheel) BRIDGE_WHEEL="${2:-}"; shift 2 ;;
      --work-dir) WORK_DIR="${2:-}"; shift 2 ;;
      -h|--help) usage; exit 0 ;;
      *) fail "prepare-gpu 未知参数：$1" ;;
    esac
  done
  WORK_DIR="$(mkdir -p "$WORK_DIR" && cd "$WORK_DIR" && printf '%s\n' "$PWD")"
  stage_gpu_assets "$WORK_DIR" "$BUNDLE" "$UENV_RELEASE" "$BRIDGE_WHEEL"
}

prepare_data() {
  local catalog="" benchmark_variant="" output_dir="" limit="" max_iterations="" runtime="" image=""
  local -a instances=()
  while (($#)); do
    case "$1" in
      --catalog) catalog="${2:-}"; shift 2 ;;
      --benchmark-variant) benchmark_variant="${2:-}"; shift 2 ;;
      --output-dir) output_dir="${2:-}"; shift 2 ;;
      --limit) limit="${2:-}"; shift 2 ;;
      --instance) instances+=("${2:-}"); shift 2 ;;
      --max-iterations) max_iterations="${2:-}"; shift 2 ;;
      --runtime) runtime="${2:-}"; shift 2 ;;
      --image) image="${2:-}"; shift 2 ;;
      -h|--help) usage; exit 0 ;;
      *) fail "prepare-data 未知参数：$1" ;;
    esac
  done
  [[ -n "$catalog" ]] || fail "prepare-data 需要 --catalog"
  [[ "$benchmark_variant" == "smith" ]] \
    || fail "prepare-data 需要 --benchmark-variant smith；当前数据转换器只支持 Smith"
  [[ -n "$output_dir" ]] || fail "prepare-data 需要 --output-dir"
  if [[ ${#instances[@]} -eq 0 && -z "$limit" ]]; then
    fail "prepare-data 需要 --instance ID（可重复）或 --limit N；0 表示全部"
  fi
  if [[ ${#instances[@]} -gt 0 && -n "$limit" ]]; then
    fail "prepare-data 的 --instance 与 --limit 二选一，不能同时使用"
  fi
  [[ -z "$limit" || "$limit" =~ ^[0-9]+$ ]] || fail "--limit 必须是非负整数"
  [[ "$max_iterations" =~ ^[1-9][0-9]*$ ]] || fail "--max-iterations 必须是正整数"
  catalog="$(absolute_file "$catalog")"
  install -d "$output_dir"
  output_dir="$(absolute_dir "$output_dir")"
  local preparer="$SCRIPT_RELEASE/libexec/uenv/swe/prepare_verl_data.py"
  [[ -f "$preparer" ]] || fail "找不到 prepare_verl_data.py：$preparer"

  local -a arguments=(
    --catalog "$catalog"
    --benchmark-variant "$benchmark_variant"
    --output-dir "$output_dir"
    --max-iterations "$max_iterations"
  )
  local instance
  for instance in "${instances[@]}"; do
    arguments+=(--instance "$instance")
  done
  [[ ${#instances[@]} -eq 0 ]] && arguments+=(--limit "$limit")

  if python3 -c 'import pandas, pyarrow' >/dev/null 2>&1; then
    python3 "$preparer" "${arguments[@]}"
    return 0
  fi

  [[ "$runtime" == docker || "$runtime" == podman ]] \
    || fail "主机缺少 pandas/pyarrow；使用容器准备数据时必须传 --runtime docker|podman"
  [[ -n "$image" ]] \
    || fail "主机缺少 pandas/pyarrow；使用容器准备数据时必须传 --image"
  runtime="$(choose_runtime "$runtime")"
  echo "主机缺少 pandas/pyarrow，改用 VeRL 容器准备数据。"
  local -a container_arguments=(
    run --rm
    -v "$preparer:/uenv/prepare_verl_data.py:ro"
    -v "$catalog:/uenv/catalog.json:ro"
    -v "$output_dir:/uenv/output"
    --entrypoint python3
    "$image"
    /uenv/prepare_verl_data.py
    --catalog /uenv/catalog.json
    --benchmark-variant "$benchmark_variant"
    --output-dir /uenv/output
    --max-iterations "$max_iterations"
  )
  for instance in "${instances[@]}"; do
    container_arguments+=(--instance "$instance")
  done
  [[ ${#instances[@]} -eq 0 ]] && container_arguments+=(--limit "$limit")
  if ! "$runtime" "${container_arguments[@]}"; then
    fail "容器中也缺少数据依赖；请安装：python3 -m pip install pandas pyarrow"
  fi
}

choose_runtime() {
  local requested="$1"
  if [[ -n "$requested" ]]; then
    [[ "$requested" == docker || "$requested" == podman ]] \
      || fail "--runtime 必须是 docker 或 podman"
    command -v "$requested" >/dev/null 2>&1 || fail "找不到容器运行时：$requested"
    printf '%s\n' "$requested"
  elif command -v docker >/dev/null 2>&1; then
    printf 'docker\n'
  elif command -v podman >/dev/null 2>&1; then
    printf 'podman\n'
  else
    fail "找不到 docker 或 podman"
  fi
}

check_endpoint() {
  python3 - "$1" <<'PY'
import socket
import sys

endpoint = sys.argv[1]
host, separator, port = endpoint.rpartition(":")
if not separator or not host or not port.isdigit():
    raise SystemExit(f"invalid HOST:PORT: {endpoint}")
try:
    with socket.create_connection((host, int(port)), timeout=5):
        pass
except OSError as exc:
    raise SystemExit(f"cannot connect to UEnv Server {endpoint}: {exc}")
PY
}

hydra_key() {
  local override="$1"
  [[ "$override" == *=* ]] || fail "Hydra 覆盖必须使用 KEY=VALUE：$override"
  local key="${override%%=*}"
  [[ "$key" =~ ^\+{0,2}[A-Za-z_][A-Za-z0-9_.-]*$ ]] \
    || fail "Hydra 配置键格式非法：$key"
  printf '%s\n' "$key"
}

validate_user_hydra_override() {
  local override="$1" key normalized
  key="$(hydra_key "$override")"
  normalized="${key#+}"
  normalized="${normalized#+}"
  case "$normalized" in
    data.train_files|data.val_files|data.train_batch_size|\
    actor_rollout_ref.model.path|actor_rollout_ref.rollout.n|\
    actor_rollout_ref.rollout.agent.default_agent_loop|\
    actor_rollout_ref.rollout.agent.agent_loop_config_path|\
    trainer.n_gpus_per_node|trainer.nnodes|trainer.total_training_steps)
      fail "Hydra 配置 $normalized 由 UEnv 公共参数管理，请修改对应命令行参数"
      ;;
  esac
}

read_verl_config() {
  local path="$1" raw line
  [[ -f "$path" ]] || fail "--verl-config 文件不存在：$path"
  while IFS= read -r raw || [[ -n "$raw" ]]; do
    raw="${raw%$'\r'}"
    line="${raw#"${raw%%[![:space:]]*}"}"
    line="${line%"${line##*[![:space:]]}"}"
    [[ -z "$line" || "$line" == \#* ]] && continue
    validate_user_hydra_override "$line"
    VERL_FILE_HYDRA+=("$line")
  done < "$path"
}

merge_hydra_overrides() {
  local -n destination="$1"
  shift
  local -A positions=()
  local override key index
  destination=()
  for override in "$@"; do
    key="$(hydra_key "$override")"
    if [[ -n "${positions[$key]+x}" ]]; then
      index="${positions[$key]}"
      destination[$index]="$override"
    else
      positions[$key]="${#destination[@]}"
      destination+=("$override")
    fi
  done
}

run_training() {
  parse_asset_options
  local env_type="" model="" data="" endpoint="" gateway_url="" gateway_port=18080 gateway_bind=""
  local gateway_port_explicit=0
  local gpus="" steps="" rollouts="" train_batch="" runtime="" image="" dry_run=0
  local device_backend="${UENV_DEVICE_BACKEND:-cuda}" ascend_devices="${ASCEND_VISIBLE_DEVICES:-0}"
  local verl_config="" print_effective_config=0
  local -a extra_hydra=()
  while (($#)); do
    case "$1" in
      --bundle) BUNDLE="${2:-}"; shift 2 ;;
      --uenv-release) UENV_RELEASE="${2:-}"; shift 2 ;;
      --bridge-wheel) BRIDGE_WHEEL="${2:-}"; shift 2 ;;
      --work-dir) WORK_DIR="${2:-}"; shift 2 ;;
      --env-type) env_type="${2:-}"; shift 2 ;;
      --model) model="${2:-}"; shift 2 ;;
      --data) data="${2:-}"; shift 2 ;;
      --uenv-endpoint) endpoint="${2:-}"; shift 2 ;;
      --gateway-public-url) gateway_url="${2:-}"; shift 2 ;;
      --gateway-port) gateway_port="${2:-}"; gateway_port_explicit=1; shift 2 ;;
      --gateway-bind) gateway_bind="${2:-}"; shift 2 ;;
      --gpus) gpus="${2:-}"; shift 2 ;;
      --steps) steps="${2:-}"; shift 2 ;;
      --rollouts) rollouts="${2:-}"; shift 2 ;;
      --train-batch-size) train_batch="${2:-}"; shift 2 ;;
      --runtime) runtime="${2:-}"; shift 2 ;;
      --image) image="${2:-}"; shift 2 ;;
      --device-backend) device_backend="${2:-}"; shift 2 ;;
      --ascend-devices) ascend_devices="${2:-}"; shift 2 ;;
      --verl-config) verl_config="${2:-}"; shift 2 ;;
      --set) extra_hydra+=("${2:-}"); shift 2 ;;
      --print-effective-config) print_effective_config=1; shift ;;
      --dry-run) dry_run=1; shift ;;
      -h|--help) usage; exit 0 ;;
      *) fail "run 未知参数：$1" ;;
    esac
  done

  [[ -n "$env_type" ]] || fail "run 需要 --env-type；底层入口不会猜测任务环境"
  [[ "$env_type" =~ ^[a-z0-9][a-z0-9._-]*$ ]] || fail "--env-type 格式非法"
  [[ "$gpus" =~ ^[1-9][0-9]*$ ]] || fail "run 需要正整数 --gpus"
  [[ "$steps" =~ ^[1-9][0-9]*$ ]] || fail "--steps 必须是正整数"
  [[ "$rollouts" =~ ^[1-9][0-9]*$ && "$rollouts" -ge 2 ]] || fail "--rollouts 必须至少为 2"
  [[ "$train_batch" =~ ^[1-9][0-9]*$ ]] || fail "--train-batch-size 必须是正整数"
  [[ -n "$endpoint" ]] || fail "run 需要 --uenv-endpoint HOST:PORT"
  [[ "$endpoint" =~ ^[A-Za-z0-9._:-]+$ ]] || fail "--uenv-endpoint 应为 HOST:PORT"
  [[ -n "$model" ]] || fail "run 需要 --model"
  [[ -n "$data" ]] || fail "run 需要 --data"
  [[ "$runtime" == docker || "$runtime" == podman ]] || fail "run 需要 --runtime docker|podman"
  [[ -n "$image" ]] || fail "run 需要 --image"
  device_backend="$(normalize_device_backend "$device_backend")"
  [[ -n "$ascend_devices" ]] || fail "--ascend-devices 不能为空"
  local torch_device_backend_autoload="${TORCH_DEVICE_BACKEND_AUTOLOAD:-0}"
  local ray_noset_ascend_rt_visible_devices="${RAY_EXPERIMENTAL_NOSET_ASCEND_RT_VISIBLE_DEVICES:-1}"
  model="$(absolute_dir "$model")"
  data="$(absolute_dir "$data")"
  [[ -f "$data/train.parquet" && -f "$data/test.parquet" ]] \
    || fail "--data 必须包含 train.parquet 和 test.parquet"
  if [[ -z "$gateway_url" ]]; then
    case "$endpoint" in
      127.0.0.1:*|localhost:*) gateway_url="http://127.0.0.1:$gateway_port/v1" ;;
      *) fail "双机训练必须传 --gateway-public-url，填写 CPU/UEnv 主机可访问的 GPU 地址" ;;
    esac
  fi
  [[ "$gateway_url" =~ ^https?://[^[:space:]]+$ ]] || fail "--gateway-public-url 必须是 http(s) URL"
  if [[ "$gateway_url" =~ ^https?://(\[[^]]+\]|[^/:]+):([0-9]+)(/|$) ]]; then
    url_port="${BASH_REMATCH[2]}"
    if [[ "$gateway_port_explicit" -eq 0 ]]; then
      gateway_port="$url_port"
    elif [[ "$gateway_port" != "$url_port" ]]; then
      fail "--gateway-port ($gateway_port) 与 --gateway-public-url 端口 ($url_port) 不一致"
    fi
  fi
  [[ "$gateway_port" =~ ^[1-9][0-9]*$ && "$gateway_port" -le 65535 ]] || fail "--gateway-port 非法"
  if [[ -z "$gateway_bind" ]]; then
    case "$endpoint" in
      127.0.0.1:*|localhost:*) gateway_bind="127.0.0.1" ;;
      *) gateway_bind="0.0.0.0" ;;
    esac
  fi
  [[ "$gateway_bind" =~ ^[A-Za-z0-9._:-]+$ ]] || fail "--gateway-bind 不是合法主机或 IP"

  local scheduling_var scheduling_value
  for scheduling_var in \
    UENV_EXPECTED_WORKER_PARALLELISM \
    UENV_MAX_EPISODE_CONCURRENCY \
    UENV_MAX_IN_FLIGHT_BATCHES \
    UENV_TARGET_WORKER_SLOTS \
    UENV_POOL_WARMUP_TARGET \
    UENV_MAX_PARALLEL_PER_WORKER \
    UENV_AGENT_JOB_MAX_CONCURRENCY \
    UENV_RUNTIME_GATEWAY_SESSION_LIMIT; do
    scheduling_value="${!scheduling_var:-}"
    [[ -z "$scheduling_value" || "$scheduling_value" =~ ^[0-9]+$ ]] \
      || fail "$scheduling_var 必须为空或非负整数"
  done
  case "${UENV_REQUIRE_WARM_SLOT:-false}" in
    true|false|1|0|yes|no|on|off) ;;
    *) fail "UENV_REQUIRE_WARM_SLOT 必须是 true/false" ;;
  esac
  [[ "${UENV_ADAPTER_CORE_GRPC_MAX_MESSAGE_BYTES:-16777216}" =~ ^[1-9][0-9]*$ ]] \
    || fail "UENV_ADAPTER_CORE_GRPC_MAX_MESSAGE_BYTES 必须是正整数"

  WORK_DIR="$(mkdir -p "$WORK_DIR" && cd "$WORK_DIR" && printf '%s\n' "$PWD")"

  local effective_batch=$((train_batch * rollouts))
  [[ "$effective_batch" -ge "$gpus" && $((effective_batch % gpus)) -eq 0 ]] \
    || fail "--train-batch-size × --rollouts 必须不小于训练设备数，且能被训练设备数整除"
  local -a baseline_hydra=(
    "algorithm.adv_estimator=grpo"
    "algorithm.use_kl_in_reward=False"
    "data.train_files=/data/train.parquet"
    "data.val_files=/data/test.parquet"
    "data.train_batch_size=$train_batch"
    "data.max_prompt_length=1024"
    "data.max_response_length=4096"
    "data.filter_overlong_prompts=True"
    "data.truncation=error"
    "data.return_raw_chat=True"
    "data.dataloader_num_workers=0"
    "actor_rollout_ref.model.path=/model"
    "actor_rollout_ref.model.use_remove_padding=True"
    "actor_rollout_ref.model.enable_gradient_checkpointing=True"
    "actor_rollout_ref.actor.optim.lr=1e-6"
    "actor_rollout_ref.actor.ppo_mini_batch_size=$train_batch"
    "actor_rollout_ref.actor.ppo_micro_batch_size_per_gpu=1"
    "actor_rollout_ref.actor.use_kl_loss=True"
    "actor_rollout_ref.actor.kl_loss_coef=0.001"
    "actor_rollout_ref.actor.kl_loss_type=low_var_kl"
    "actor_rollout_ref.actor.fsdp_config.param_offload=True"
    "actor_rollout_ref.actor.fsdp_config.optimizer_offload=True"
    "actor_rollout_ref.rollout.name=vllm"
    "actor_rollout_ref.rollout.tensor_model_parallel_size=1"
    "actor_rollout_ref.rollout.gpu_memory_utilization=0.6"
    "actor_rollout_ref.rollout.n=$rollouts"
    "actor_rollout_ref.rollout.calculate_log_probs=True"
    "actor_rollout_ref.rollout.log_prob_micro_batch_size_per_gpu=1"
    "actor_rollout_ref.rollout.enforce_eager=True"
    "actor_rollout_ref.rollout.agent.num_workers=1"
    "actor_rollout_ref.rollout.agent.default_agent_loop=uenv_agent"
    "actor_rollout_ref.rollout.agent.agent_loop_config_path=/uenv-assets/uenv-agent-loop.yaml"
    "actor_rollout_ref.ref.log_prob_micro_batch_size_per_gpu=1"
    "actor_rollout_ref.ref.fsdp_config.param_offload=True"
    "reward.reward_manager.source=register"
    "reward.reward_manager.name=naive"
    "reward.num_workers=1"
    "trainer.logger=['console']"
    "trainer.project_name=uenv"
    "trainer.experiment_name=uenv_${env_type}_grpo"
    "trainer.n_gpus_per_node=$gpus"
    "trainer.nnodes=1"
    "trainer.val_before_train=False"
    "trainer.test_freq=-1"
    "trainer.save_freq=-1"
    "trainer.total_epochs=1"
    "trainer.total_training_steps=$steps"
    "trainer.resume_mode=disable"
    "trainer.default_local_dir=/outputs/checkpoints"
    "ray_kwargs.ray_init.num_cpus=$((gpus * 4))"
    "+ray_kwargs.ray_init.runtime_env.env_vars.UENV_DEVICE_BACKEND=$device_backend"
    "+ray_kwargs.ray_init.runtime_env.env_vars.ASCEND_VISIBLE_DEVICES=$ascend_devices"
    "+ray_kwargs.ray_init.runtime_env.env_vars.ASCEND_RT_VISIBLE_DEVICES=${ASCEND_RT_VISIBLE_DEVICES:-$ascend_devices}"
    "+ray_kwargs.ray_init.runtime_env.env_vars.TORCH_DEVICE_BACKEND_AUTOLOAD=$torch_device_backend_autoload"
    "+ray_kwargs.ray_init.runtime_env.env_vars.RAY_EXPERIMENTAL_NOSET_ASCEND_RT_VISIBLE_DEVICES=$ray_noset_ascend_rt_visible_devices"
  )
  VERL_FILE_HYDRA=()
  [[ -z "$verl_config" ]] || read_verl_config "$verl_config"
  local override
  for override in "${extra_hydra[@]}"; do
    validate_user_hydra_override "$override"
  done
  local -a hydra_args=()
  merge_hydra_overrides hydra_args \
    "${baseline_hydra[@]}" "${VERL_FILE_HYDRA[@]}" "${extra_hydra[@]}"

  install -d "$WORK_DIR/output"
  local effective_config="$WORK_DIR/output/effective-hydra-overrides.txt"
  printf '%s\n' "${hydra_args[@]}" > "$effective_config"
  echo "最终 Hydra 配置：$effective_config"
  if [[ "$print_effective_config" -eq 1 ]]; then
    cat "$effective_config"
    return 0
  fi

  if [[ -n "$BUNDLE" || -n "$UENV_RELEASE" || -n "$BRIDGE_WHEEL" || ! -f "$WORK_DIR/assets/state.env" ]]; then
    stage_gpu_assets "$WORK_DIR" "$BUNDLE" "$UENV_RELEASE" "$BRIDGE_WHEEL"
  fi
  # shellcheck disable=SC1090
  source "$WORK_DIR/assets/state.env"
  [[ -f "$UENV_BRIDGE_WHEEL" && -f "$UENV_AGENT_CONFIG" ]] || fail "GPU 侧资产不完整，请先 prepare-gpu"
  local verl_checkout
  verl_checkout="$(ensure_verl_checkout "$WORK_DIR")"

  runtime="$(choose_runtime "$runtime")"
  require_command python3
  if [[ "$dry_run" -eq 0 ]]; then
    check_endpoint "$endpoint"
  fi

  local -a container_args=(run --rm --network host --shm-size=32g --workdir /workspace/verl)
  if [[ "$device_backend" == "ascend" ]]; then
    append_ascend_container_args container_args
  elif [[ "$runtime" == docker ]]; then
    container_args+=(--gpus all)
  else
    container_args+=(--device nvidia.com/gpu=all)
  fi
  container_args+=(
    -v "$verl_checkout:/workspace/verl:ro"
    -v "$model:/model:ro"
    -v "$data:/data:ro"
    -v "$UENV_BRIDGE_WHEEL:/uenv-assets/$(basename "$UENV_BRIDGE_WHEEL"):ro"
    -v "$UENV_AGENT_CONFIG:/uenv-assets/uenv-agent-loop.yaml:ro"
    -v "$WORK_DIR/output:/outputs"
    -e "UENV_ADAPTER_CORE_ENDPOINT=$endpoint"
    -e "UENV_DEVICE_BACKEND=$device_backend"
    -e "ASCEND_VISIBLE_DEVICES=$ascend_devices"
    -e "ASCEND_RT_VISIBLE_DEVICES=${ASCEND_RT_VISIBLE_DEVICES:-$ascend_devices}"
    -e "TORCH_DEVICE_BACKEND_AUTOLOAD=$torch_device_backend_autoload"
    -e "RAY_EXPERIMENTAL_NOSET_ASCEND_RT_VISIBLE_DEVICES=$ray_noset_ascend_rt_visible_devices"
    -e "UENV_DEFAULT_ENV_TYPE=$env_type"
    -e "UENV_MODEL_GATEWAY_PORT=$gateway_port"
    -e "UENV_MODEL_GATEWAY_BIND_HOST=$gateway_bind"
    -e "UENV_MODEL_GATEWAY_PUBLIC_URL=$gateway_url"
    -e "UENV_EXPECTED_WORKER_PARALLELISM=${UENV_EXPECTED_WORKER_PARALLELISM:-}"
    -e "UENV_MAX_EPISODE_CONCURRENCY=${UENV_MAX_EPISODE_CONCURRENCY:-}"
    -e "UENV_MAX_IN_FLIGHT_BATCHES=${UENV_MAX_IN_FLIGHT_BATCHES:-}"
    -e "UENV_TARGET_WORKER_SLOTS=${UENV_TARGET_WORKER_SLOTS:-}"
    -e "UENV_POOL_WARMUP_TARGET=${UENV_POOL_WARMUP_TARGET:-}"
    -e "UENV_MAX_PARALLEL_PER_WORKER=${UENV_MAX_PARALLEL_PER_WORKER:-}"
    -e "UENV_AGENT_JOB_MAX_CONCURRENCY=${UENV_AGENT_JOB_MAX_CONCURRENCY:-}"
    -e "UENV_RUNTIME_GATEWAY_SESSION_LIMIT=${UENV_RUNTIME_GATEWAY_SESSION_LIMIT:-}"
    -e "UENV_REQUIRE_WARM_SLOT=${UENV_REQUIRE_WARM_SLOT:-false}"
    -e "UENV_ADAPTER_CORE_GRPC_MAX_MESSAGE_BYTES=${UENV_ADAPTER_CORE_GRPC_MAX_MESSAGE_BYTES:-16777216}"
  )
  if [[ -n "$UENV_VERL_RUNNER" ]]; then
    [[ -f "$UENV_VERL_RUNNER" ]] || fail "VeRL 启动器不存在：$UENV_VERL_RUNNER"
    container_args+=(-v "$UENV_VERL_RUNNER:/uenv-assets/run_verl_main_ppo.py:ro")
  fi
  container_args+=(
    "$image"
    bash -lc
    'set -euo pipefail
export HYDRA_FULL_ERROR=1
export TOKENIZERS_PARALLELISM=false
export PROTOCOL_BUFFERS_PYTHON_IMPLEMENTATION=python
if [[ "${UENV_DEVICE_BACKEND:-cuda}" == "ascend" ]]; then
  if [[ -f /usr/local/Ascend/ascend-toolkit/set_env.sh ]]; then
    source /usr/local/Ascend/ascend-toolkit/set_env.sh
  fi
  if [[ -f /usr/local/Ascend/nnal/atb/set_env.sh ]]; then
    source /usr/local/Ascend/nnal/atb/set_env.sh
  fi
  if [[ -d /usr/local/Ascend/driver/lib64/driver ]]; then
    export LD_LIBRARY_PATH=/usr/local/Ascend/driver/lib64/driver:${LD_LIBRARY_PATH:-}
  fi
  export ASCEND_VISIBLE_DEVICES="${ASCEND_VISIBLE_DEVICES:-0}"
  export ASCEND_RT_VISIBLE_DEVICES="${ASCEND_RT_VISIBLE_DEVICES:-${ASCEND_VISIBLE_DEVICES}}"
  export TORCH_DEVICE_BACKEND_AUTOLOAD="${TORCH_DEVICE_BACKEND_AUTOLOAD:-0}"
  export RAY_EXPERIMENTAL_NOSET_ASCEND_RT_VISIBLE_DEVICES="${RAY_EXPERIMENTAL_NOSET_ASCEND_RT_VISIBLE_DEVICES:-1}"
  unset CUDA_VISIBLE_DEVICES
  export UENV_PATCH_TORCH_CUDA_IS_AVAILABLE_NO_DEVICES=0
  export UENV_PATCH_VERL_DEVICE_CAPABILITY_FALLBACK=0
fi
export UENV_AGENT_LOOP_CLIENT=rust_core
export UENV_ADAPTER_CORE_AUTO_START=0
export UENV_AGENT_LOOP_BATCH=0
export UENV_AGENT_LOOP_BATCH_SIZE=0
export UENV_AGENT_LOOP_TIMEOUT_SECONDS=3600
export UENV_MODEL_GATEWAY_ENABLED=1
export UENV_MODEL_GATEWAY_BIND_HOST="${UENV_MODEL_GATEWAY_BIND_HOST:-127.0.0.1}"
export UENV_MODEL_GATEWAY_STOP_ON_CLOSE=1
export UENV_REQUIRE_SWE_RESPONSE_TRACE=1
export UENV_AGENT_LOOP_FAILED_EPISODE_POLICY=raise
export UENV_AGENT_LOOP_REQUEST_RECORD_PATH=/outputs/agent-loop-requests.jsonl
export UENV_AGENT_LOOP_RESULT_RECORD_PATH=/outputs/agent-loop-results.jsonl
cp -a /workspace/verl /tmp/uenv-verl-src
python -m pip install --disable-pip-version-check --no-deps -e /tmp/uenv-verl-src
python -m pip install --disable-pip-version-check /uenv-assets/*.whl
cd /outputs
python - <<'PY'
import uenv.bridge.verl_agent_loop
import verl
print("VeRL and UEnv Bridge imports passed")
PY
if [[ -f /uenv-assets/run_verl_main_ppo.py ]]; then
  export UENV_PATCH_VERL_MODEL_VERSION_RESPONSE=1
  exec python /uenv-assets/run_verl_main_ppo.py "$@"
fi
exec python -m verl.trainer.main_ppo "$@"'
    uenv-verl
    "${hydra_args[@]}"
  )

  echo "VeRL：$VERL_VERSION ($VERL_COMMIT)"
  echo "设备后端：$device_backend"
  echo "UEnv：$endpoint"
  echo "模型网关：$gateway_url"
  echo "结果目录：$WORK_DIR/output"
  if [[ "$dry_run" -eq 1 ]]; then
    printf '%q ' "$runtime" "${container_args[@]}"
    printf '\n'
    return 0
  fi
  "$runtime" "${container_args[@]}"
}

COMMAND="${1:-help}"
if (($#)); then
  shift
fi
case "$COMMAND" in
  prepare-uenv) prepare_uenv "$@" ;;
  prepare-gpu) prepare_gpu "$@" ;;
  prepare-data) prepare_data "$@" ;;
  run) run_training "$@" ;;
  help|-h|--help) usage ;;
  *) fail "未知命令：$COMMAND" ;;
esac
