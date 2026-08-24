#!/usr/bin/env bash
# Internal single-case SWE runner. Public users call `uenv evaluate run-swe`.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RELEASE_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
OPENHANDS_DIR="${OPENHANDS_BENCHMARKS_DIR:-/opt/uenv/agent/openhands-benchmarks}"
AGENT_USER="uenv-agent"
AGENT_HOME="/opt/uenv/agent"

fail() {
  echo "错误：$*" >&2
  exit 1
}

[[ "$EUID" -eq 0 ]] || fail "内部 SWE runner 必须由 sudo uenv evaluate run-swe 启动"
[[ $# -ge 1 ]] || fail "缺少 provider"
PROVIDER="$1"
shift
case "$PROVIDER" in local|volcengine) ;; *) fail "provider 必须是 local 或 volcengine" ;; esac

MODEL=""
BASE_URL=""
GATEWAY=""
CATALOG=""
VARIANT=""
INSTANCE=""
OUTPUT_DIR=""
MAX_ITERATIONS=""
OFFLINE=0
while (($#)); do
  case "$1" in
    --model) MODEL="${2:-}"; shift 2 ;;
    --base-url) BASE_URL="${2:-}"; shift 2 ;;
    --gateway) GATEWAY="${2:-}"; shift 2 ;;
    --catalog) CATALOG="${2:-}"; shift 2 ;;
    --benchmark-variant) VARIANT="${2:-}"; shift 2 ;;
    --instance) INSTANCE="${2:-}"; shift 2 ;;
    --output-dir) OUTPUT_DIR="${2:-}"; shift 2 ;;
    --max-iterations) MAX_ITERATIONS="${2:-}"; shift 2 ;;
    --offline) OFFLINE=1; shift ;;
    *) fail "内部 SWE runner 未知参数：$1" ;;
  esac
done

[[ -n "$MODEL" && -n "$BASE_URL" && -n "$GATEWAY" && -n "$CATALOG" ]] \
  || fail "内部 SWE runner 缺少模型、Gateway 或 catalog 参数"
[[ -n "$INSTANCE" && -n "$OUTPUT_DIR" ]] || fail "内部 SWE runner 缺少实例或输出目录"
case "$VARIANT" in verified|lite|pro|smith) ;; *) fail "无效 benchmark variant：$VARIANT" ;; esac
[[ "$MAX_ITERATIONS" =~ ^[1-9][0-9]*$ ]] || fail "max-iterations 必须是正整数"
[[ "$INSTANCE" =~ ^[A-Za-z0-9_.-]+$ ]] || fail "instance_id 含不支持的字符"
[[ -f "$CATALOG" && -r "$CATALOG" ]] || fail "无法读取 catalog：$CATALOG"
[[ -d "$OUTPUT_DIR" && ! -L "$OUTPUT_DIR" ]] || fail "case 制品目录无效：$OUTPUT_DIR"
# 归一化为绝对路径：本脚本结尾会 cd 进该目录再降权启动 Agent，相对路径会失效
OUTPUT_DIR="$(cd "$OUTPUT_DIR" && pwd)" || fail "无法进入 case 制品目录：$OUTPUT_DIR"
id "$AGENT_USER" >/dev/null 2>&1 || fail "缺少 $AGENT_USER；请先运行 prepare-swe"
command -v runuser >/dev/null 2>&1 || fail "缺少 runuser"

find_first_file() {
  local candidate
  for candidate in "$@"; do
    [[ -f "$candidate" ]] || continue
    printf '%s\n' "$candidate"
    return 0
  done
  return 1
}

DRIVER="$(find_first_file \
  "/opt/uenv/current/share/swe/openhands/run_swebenchpro_official.py" \
  "$RELEASE_ROOT/integrations/openhands/run_swebenchpro_official.py")" \
  || fail "找不到 UEnv OpenHands 驱动"
OPENHANDS_PYTHON="$OPENHANDS_DIR/.venv/bin/python"
[[ -x "$OPENHANDS_PYTHON" ]] || fail "OpenHands 尚未安装，请先运行 prepare-swe"

config_value() {
  local file="$1" key="$2"
  [[ -r "$file" ]] || return 0
  awk -F= -v wanted="$key" '$1 == wanted {print substr($0, length(wanted) + 2); exit}' "$file"
}

RUNTIME="${UENV_SWE_RUNTIME:-$(config_value /etc/uenv/swe.env UENV_SWE_RUNTIME)}"
TRAJECTORY_ENDPOINT="${UENV_TRAJECTORY_ENDPOINT:-$(config_value /etc/uenv/swe.env UENV_TRAJECTORY_ENDPOINT)}"
[[ -z "$TRAJECTORY_ENDPOINT" || "$TRAJECTORY_ENDPOINT" =~ ^https?://[^[:space:]]+$ ]] \
  || fail "UENV_TRAJECTORY_ENDPOINT 必须是 http(s) URL"
# Agent 回读/校验服务器侧轨迹需要同一个 token；Worker 上传用的是 systemd 加载的
# /etc/uenv/secrets/swe.env，这里显式读出并随 env 传入（env -i 会清空父进程环境）。
TRAJECTORY_TOKEN="${UENV_TRAJECTORY_TOKEN:-$(config_value /etc/uenv/secrets/swe.env UENV_TRAJECTORY_TOKEN)}"
if [[ -z "$RUNTIME" ]]; then
  command -v docker >/dev/null 2>&1 && RUNTIME=docker
  [[ -n "$RUNTIME" ]] || { command -v podman >/dev/null 2>&1 && RUNTIME=podman; }
fi
[[ "$RUNTIME" == docker || "$RUNTIME" == podman ]] || fail "找不到可用的 docker 或 podman"

IMAGE="$(python3 -S - "$CATALOG" "$INSTANCE" <<'PY'
import json, pathlib, sys
catalog, instance_id = sys.argv[1:]
data = json.loads(pathlib.Path(catalog).read_text(encoding="utf-8"))
if not isinstance(data, dict) or instance_id not in data or not isinstance(data[instance_id], dict):
    raise SystemExit(f"实例 {instance_id} 不在 catalog")
row = data[instance_id]
image = row.get("image_cache_key") or row.get("image_name")
if not image:
    image = "swebench/sweb.eval.x86_64." + instance_id.replace("__", "_1776_") + ":latest"
print(image)
PY
)" || fail "无法从 catalog 解析实例镜像"

runtime_as_worker() {
  if id uenv >/dev/null 2>&1; then
    runuser -u uenv -- "$RUNTIME" "$@"
  else
    "$RUNTIME" "$@"
  fi
}
if ! runtime_as_worker image inspect "$IMAGE" >/dev/null 2>&1; then
  [[ "$OFFLINE" -eq 0 ]] || fail "本机缺少实例镜像：$IMAGE"
  echo "==> 拉取实例镜像 $IMAGE"
  runtime_as_worker pull "$IMAGE"
fi

umask 077
LLM_CONFIG="$OUTPUT_DIR/.uenv-llm-config.json"
GATEWAY_KEY_FILE="$OUTPUT_DIR/.uenv-gateway-key"
[[ ! -e "$LLM_CONFIG" && ! -L "$LLM_CONFIG" ]] || fail "临时模型配置已存在"
[[ ! -e "$GATEWAY_KEY_FILE" && ! -L "$GATEWAY_KEY_FILE" ]] || fail "临时 Gateway 密钥文件已存在"

export UENV_INTERNAL_MODEL="$MODEL"
export UENV_INTERNAL_BASE_URL="$BASE_URL"
export UENV_INTERNAL_MODEL_KEY="${UENV_EVAL_MODEL_API_KEY:-}"
python3 -S - "$LLM_CONFIG" <<'PY'
import json, os, pathlib, sys
model = os.environ["UENV_INTERNAL_MODEL"]
if not model.startswith("openai/"):
    model = "openai/" + model
config = {
    "model": model,
    "base_url": os.environ["UENV_INTERNAL_BASE_URL"].rstrip("/"),
    "api_key": os.environ.get("UENV_INTERNAL_MODEL_KEY") or "EMPTY",
    "temperature": 0.2,
    "max_output_tokens": 4096,
    "timeout": 1200,
    "request_timeout": 1200,
}
path = pathlib.Path(sys.argv[1])
with path.open("x", encoding="utf-8") as handle:
    json.dump(config, handle)
PY
unset UENV_INTERNAL_MODEL UENV_INTERNAL_BASE_URL UENV_INTERNAL_MODEL_KEY UENV_EVAL_MODEL_API_KEY
printf '%s' "${UENV_GATEWAY_API_KEY:-}" > "$GATEWAY_KEY_FILE"
unset UENV_GATEWAY_API_KEY
chmod 0600 "$LLM_CONFIG" "$GATEWAY_KEY_FILE"
chown "$AGENT_USER:uenv" "$LLM_CONFIG" "$GATEWAY_KEY_FILE"
chown "$AGENT_USER:uenv" "$OUTPUT_DIR"
chmod 0700 "$OUTPUT_DIR"

# 调用者的 cwd（如 /root）对 agent 不可读；Python editable install 的 finder
# 在 cwd 不可读时会崩溃并抛出误导性 KeyError: 'openhands.sdk'，导致所有实例
# 瞬时 failed。降权启动前切到 agent 拥有的 case 制品目录。
cd "$OUTPUT_DIR" || fail "无法进入 case 制品目录：$OUTPUT_DIR"

cleanup() {
  unlink "$LLM_CONFIG" 2>/dev/null || true
  unlink "$GATEWAY_KEY_FILE" 2>/dev/null || true
}
trap cleanup EXIT HUP INT TERM

TMP_DIR="$OUTPUT_DIR/.tmp"
install -d -o "$AGENT_USER" -g uenv -m 0700 "$TMP_DIR"
DRIVER_ARGS=(
  --llm-config "$LLM_CONFIG"
  --gateway "$GATEWAY"
  --instance "$INSTANCE"
  --benchmark-variant "$VARIANT"
  --output-dir "$OUTPUT_DIR"
  --max-iterations "$MAX_ITERATIONS"
  --mode llm
  --rollout-trace off
  --instances "$CATALOG"
)

echo "==> 评测 $INSTANCE"
runuser -u "$AGENT_USER" -- /usr/bin/env -i \
  HOME="$AGENT_HOME" USER="$AGENT_USER" LOGNAME="$AGENT_USER" \
  PATH=/usr/local/bin:/usr/bin:/bin \
  LANG=C.UTF-8 LC_ALL=C.UTF-8 PYTHONUTF8=1 \
  TMPDIR="$TMP_DIR" XDG_CACHE_HOME="$AGENT_HOME/.cache" UV_CACHE_DIR="$AGENT_HOME/.cache/uv" \
  OPENHANDS_BENCHMARKS_DIR="$OPENHANDS_DIR" \
  UENV_TRAJECTORY_ENDPOINT="$TRAJECTORY_ENDPOINT" \
  UENV_TRAJECTORY_TOKEN="$TRAJECTORY_TOKEN" \
  UENV_INTERNAL_GATEWAY_KEY_FILE="$GATEWAY_KEY_FILE" \
  /bin/bash -c '
    set -euo pipefail
    UENV_GATEWAY_API_KEY="$(cat -- "$UENV_INTERNAL_GATEWAY_KEY_FILE")"
    export UENV_GATEWAY_API_KEY
    exec "$@"
  ' _ "$OPENHANDS_PYTHON" "$DRIVER" "${DRIVER_ARGS[@]}"
