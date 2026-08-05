#!/usr/bin/env bash
# Run one real SWE-bench Verified evaluation through the UEnv Worker Gateway.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RELEASE_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
OPENHANDS_DIR="${OPENHANDS_BENCHMARKS_DIR:-/opt/uenv/agent/openhands-benchmarks}"
GATEWAY=""
INSTANCE=""
CATALOG=""
OUTPUT_DIR=""
MODEL=""
BASE_URL=""
MODEL_API_KEY=""
MODEL_API_KEY_FILE=""
BENCHMARK_VARIANT=""
MAX_ITERATIONS=""
OFFLINE=0

usage() {
  cat <<'EOF'
用法：
  evaluate.sh local --model MODEL --base-url URL --gateway URL \
    --catalog FILE --benchmark-variant VARIANT --instance ID \
    --output-dir DIR --max-iterations N
  evaluate.sh volcengine --model ENDPOINT_ID --gateway URL \
    --catalog FILE --benchmark-variant VARIANT --instance ID \
    --output-dir DIR --max-iterations N

示例：
任务选项（全部必填）：
  --gateway URL          Worker Runtime Gateway
  --catalog FILE         benchmark catalog
  --benchmark-variant V  verified、pro 或 smith
  --instance ID          catalog 中的评测实例
  --output-dir DIR       结果目录
  --max-iterations N     Agent 最大迭代次数
  --offline              只使用本地已有容器镜像，不访问镜像仓库

模型选项：
  --model NAME           本地模型名或火山方舟接入点 ID（必填）
  --base-url URL         本地模型必填；方舟未传时使用官方区域地址
  --api-key-file FILE    从权限受控文件读取模型 API Key

密钥处理（不会作为命令行参数进入历史）：
  LOCAL_MODEL_API_KEY    本地模型服务密钥；未设置时使用 EMPTY
  ARK_API_KEY            火山方舟 API Key；非交互运行时必填
  UENV_GATEWAY_API_KEY   Gateway 密钥；默认从 /etc/uenv/secrets/swe.env 读取

交互终端未设置 ARK_API_KEY 时，脚本会隐藏地提示输入，不必先 export。
EOF
}

fail() {
  echo "错误：$*" >&2
  exit 1
}

[[ $# -ge 1 ]] || { usage; exit 2; }
MODE="$1"
shift
case "$MODE" in
  local|volcengine) ;;
  -h|--help|help)
    usage
    exit 0
    ;;
  *) fail "第一个参数必须是 local 或 volcengine" ;;
esac

while [[ $# -gt 0 ]]; do
  case "$1" in
    --model)
      [[ $# -ge 2 ]] || fail "--model 缺少参数"
      MODEL="$2"
      shift 2
      ;;
    --base-url)
      [[ $# -ge 2 ]] || fail "--base-url 缺少参数"
      BASE_URL="$2"
      shift 2
      ;;
    --api-key-file)
      [[ $# -ge 2 ]] || fail "--api-key-file 缺少参数"
      MODEL_API_KEY_FILE="$2"
      shift 2
      ;;
    --gateway)
      [[ $# -ge 2 ]] || fail "--gateway 缺少参数"
      GATEWAY="$2"
      shift 2
      ;;
    --instance)
      [[ $# -ge 2 ]] || fail "--instance 缺少参数"
      INSTANCE="$2"
      shift 2
      ;;
    --catalog)
      [[ $# -ge 2 ]] || fail "--catalog 缺少参数"
      CATALOG="$2"
      shift 2
      ;;
    --benchmark-variant)
      [[ $# -ge 2 ]] || fail "--benchmark-variant 缺少参数"
      BENCHMARK_VARIANT="$2"
      shift 2
      ;;
    --output-dir)
      [[ $# -ge 2 ]] || fail "--output-dir 缺少参数"
      OUTPUT_DIR="$2"
      shift 2
      ;;
    --max-iterations)
      [[ $# -ge 2 ]] || fail "--max-iterations 缺少参数"
      MAX_ITERATIONS="$2"
      shift 2
      ;;
    --offline)
      OFFLINE=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *) fail "未知参数：$1" ;;
  esac
done

[[ -n "$MODEL" ]] || fail "请通过 --model 指定模型；火山模式填写方舟推理接入点 ID"
[[ -n "$GATEWAY" ]] || fail "请通过 --gateway 指定 Worker Runtime Gateway"
[[ -n "$CATALOG" ]] || fail "请通过 --catalog 指定 benchmark catalog"
[[ -n "$INSTANCE" ]] || fail "请通过 --instance 指定 catalog 中的实例"
[[ -n "$OUTPUT_DIR" ]] || fail "请通过 --output-dir 指定结果目录"
case "$BENCHMARK_VARIANT" in
  verified|pro|smith) ;;
  "") fail "请通过 --benchmark-variant 指定 verified、pro 或 smith" ;;
  *) fail "不支持的 --benchmark-variant：$BENCHMARK_VARIANT" ;;
esac
[[ "$MAX_ITERATIONS" =~ ^[1-9][0-9]*$ ]] || fail "--max-iterations 必须是正整数"
[[ "$INSTANCE" =~ ^[A-Za-z0-9_.-]+$ ]] || fail "--instance 含有不支持的字符"

FILE_API_KEY=""
if [[ -n "$MODEL_API_KEY_FILE" ]]; then
  [[ -f "$MODEL_API_KEY_FILE" && -r "$MODEL_API_KEY_FILE" ]] \
    || fail "无法读取 API Key 文件：$MODEL_API_KEY_FILE"
  FILE_API_KEY="$(<"$MODEL_API_KEY_FILE")"
  [[ "$FILE_API_KEY" != *[$'\r\n']* ]] || fail "API Key 文件只能包含一行"
fi

if [[ "$MODE" == "local" ]]; then
  [[ -n "$BASE_URL" ]] || fail "本地模型需要 --base-url"
  MODEL_API_KEY="${LOCAL_MODEL_API_KEY:-${FILE_API_KEY:-EMPTY}}"
  unset LOCAL_MODEL_API_KEY
else
  BASE_URL="${BASE_URL:-${ARK_BASE_URL:-https://ark.cn-beijing.volces.com/api/v3}}"
  MODEL_API_KEY="${ARK_API_KEY:-$FILE_API_KEY}"
  if [[ -z "$MODEL_API_KEY" && -t 0 ]]; then
    if ! read -rsp '火山方舟 API Key（输入不显示）: ' MODEL_API_KEY; then
      echo >&2
      fail "未读到火山方舟 API Key"
    fi
    echo >&2
  fi
  [[ -n "$MODEL_API_KEY" ]] \
    || fail "非交互运行需要设置 ARK_API_KEY；交互终端会隐藏地提示输入"
  unset ARK_API_KEY
fi
[[ "$BASE_URL" == http://* || "$BASE_URL" == https://* ]] \
  || fail "--base-url 必须以 http:// 或 https:// 开头"

find_first_file() {
  local candidate
  for candidate in "$@"; do
    if [[ -f "$candidate" ]]; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done
  return 1
}

DRIVER="$(find_first_file \
  "/opt/uenv/current/share/swe/openhands/run_swebenchpro_official.py" \
  "$RELEASE_ROOT/integrations/openhands/run_swebenchpro_official.py")" \
  || fail "找不到 UEnv OpenHands 驱动，请确认安装包包含 SWE 组件"

[[ -f "$CATALOG" ]] || fail "catalog 不存在：$CATALOG"

OPENHANDS_PYTHON="$OPENHANDS_DIR/.venv/bin/python"
[[ -x "$OPENHANDS_PYTHON" ]] \
  || fail "OpenHands 尚未安装，请先运行：sudo bash $SCRIPT_DIR/install_openhands.sh"

config_value() {
  local file="$1" key="$2"
  [[ -r "$file" ]] || return 0
  awk -F= -v wanted="$key" '$1 == wanted {print substr($0, length(wanted) + 2); exit}' "$file"
}

GATEWAY_API_KEY="${UENV_GATEWAY_API_KEY:-${UENV_RUNTIME_GATEWAY_API_KEY:-}}"
if [[ -z "$GATEWAY_API_KEY" ]]; then
  GATEWAY_API_KEY="$(config_value /etc/uenv/secrets/swe.env UENV_SWE_GATEWAY_API_KEY)"
fi
if [[ -z "$GATEWAY_API_KEY" ]]; then
  GATEWAY_API_KEY="$(config_value /etc/uenv/secrets/swe.env UENV_RUNTIME_GATEWAY_API_KEY)"
fi
if [[ -z "$GATEWAY_API_KEY" && -e /etc/uenv/secrets/swe.env && ! -r /etc/uenv/secrets/swe.env ]]; then
  fail "无法读取 Gateway 密钥；请使用 sudo，或设置 UENV_GATEWAY_API_KEY"
fi

echo "==> 检查 Worker Runtime Gateway"
UENV_GATEWAY_API_KEY="$GATEWAY_API_KEY" "$OPENHANDS_PYTHON" - "$GATEWAY" <<'PY'
import os
import sys
import urllib.request

base = sys.argv[1]
api_key = os.environ.get("UENV_GATEWAY_API_KEY", "")
base = base.rstrip("/")
if not base.startswith(("http://", "https://")):
    base = "http://" + base
request = urllib.request.Request(base + "/runtime/v1/health")
if api_key:
    request.add_header("X-API-Key", api_key)
try:
    with urllib.request.urlopen(request, timeout=5) as response:
        if response.status // 100 != 2:
            raise RuntimeError(f"HTTP {response.status}")
except Exception as exc:
    raise SystemExit(f"Gateway 不可用：{exc}\n请检查 sudo systemctl status uenv-worker")
PY

RUNTIME="${UENV_SWE_RUNTIME:-$(config_value /etc/uenv/swe.env UENV_SWE_RUNTIME)}"
if [[ -z "$RUNTIME" ]]; then
  if command -v docker >/dev/null 2>&1; then
    RUNTIME=docker
  elif command -v podman >/dev/null 2>&1; then
    RUNTIME=podman
  else
    fail "未找到 docker 或 podman"
  fi
fi
[[ "$RUNTIME" == docker || "$RUNTIME" == podman ]] || fail "不支持的容器运行时：$RUNTIME"
command -v "$RUNTIME" >/dev/null 2>&1 || fail "找不到容器命令：$RUNTIME"

IMAGE="$({ "$OPENHANDS_PYTHON" -S - "$CATALOG" "$INSTANCE" <<'PY'
import json
import pathlib
import sys

catalog_path, instance_id = sys.argv[1:]
row = {}
if catalog_path:
    data = json.loads(pathlib.Path(catalog_path).read_text(encoding="utf-8"))
    if not isinstance(data, dict) or instance_id not in data:
        raise SystemExit(f"实例 {instance_id} 不在 catalog：{catalog_path}")
    row = data.get(instance_id, {})
image = row.get("image_cache_key") or row.get("image_name")
if not image:
    image = "swebench/sweb.eval.x86_64." + instance_id.replace("__", "_1776_") + ":latest"
print(image)
PY
} 2>&1)" || fail "无法解析实例镜像：$IMAGE"

runtime_as_worker() {
  if [[ "$EUID" -eq 0 ]] && id uenv >/dev/null 2>&1 && command -v runuser >/dev/null 2>&1; then
    runuser -u uenv -- "$RUNTIME" "$@"
  else
    "$RUNTIME" "$@"
  fi
}

if ! runtime_as_worker image inspect "$IMAGE" >/dev/null 2>&1; then
  if [[ "$OFFLINE" -eq 1 ]]; then
    fail "本机没有实例镜像 $IMAGE；去掉 --offline 可从镜像仓库下载"
  fi
  echo "==> 下载实例镜像 $IMAGE"
  runtime_as_worker pull "$IMAGE"
else
  echo "==> 实例镜像已就绪"
fi

mkdir -p "$OUTPUT_DIR"

case "$MODEL" in
  openai/*) LLM_MODEL="$MODEL" ;;
  *) LLM_MODEL="openai/$MODEL" ;;
esac

umask 077
LLM_CONFIG="$(mktemp "${TMPDIR:-/tmp}/uenv-llm.XXXXXX.json")"
cleanup() {
  rm -f -- "$LLM_CONFIG"
}
trap cleanup EXIT HUP INT TERM

export UENV_EVAL_LLM_MODEL="$LLM_MODEL"
export UENV_EVAL_LLM_BASE_URL="${BASE_URL%/}"
export UENV_EVAL_LLM_API_KEY="$MODEL_API_KEY"
"$OPENHANDS_PYTHON" - "$LLM_CONFIG" <<'PY'
import json
import os
import pathlib
import sys

config = {
    "model": os.environ["UENV_EVAL_LLM_MODEL"],
    "base_url": os.environ["UENV_EVAL_LLM_BASE_URL"],
    "api_key": os.environ["UENV_EVAL_LLM_API_KEY"],
    "temperature": 0.2,
    "max_output_tokens": 4096,
    "timeout": 1200,
    "request_timeout": 1200,
}
pathlib.Path(sys.argv[1]).write_text(json.dumps(config), encoding="utf-8")
PY
unset UENV_EVAL_LLM_MODEL UENV_EVAL_LLM_BASE_URL UENV_EVAL_LLM_API_KEY MODEL_API_KEY
chmod 0600 "$LLM_CONFIG"

DRIVER_ARGS=(
  --llm-config "$LLM_CONFIG"
  --gateway "$GATEWAY"
  --instance "$INSTANCE"
  --benchmark-variant "$BENCHMARK_VARIANT"
  --output-dir "$OUTPUT_DIR"
  --max-iterations "$MAX_ITERATIONS"
  --mode llm
  --rollout-trace off
)
if [[ -n "$CATALOG" ]]; then
  DRIVER_ARGS+=(--instances "$CATALOG")
fi

echo "==> 开始评测 $INSTANCE"
echo "    模型：$MODEL"
echo "    结果：$OUTPUT_DIR"
export OPENHANDS_BENCHMARKS_DIR="$OPENHANDS_DIR"
export UENV_GATEWAY_API_KEY="$GATEWAY_API_KEY"
"$OPENHANDS_PYTHON" "$DRIVER" "${DRIVER_ARGS[@]}"

echo
echo "评测完成，结果保存在：$OUTPUT_DIR"
