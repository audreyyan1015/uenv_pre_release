#!/usr/bin/env bash
# Install the OpenHands benchmark runner pinned by this UEnv release.
set -euo pipefail

# uv 会从当前目录向上递归查找配置文件；若从 /root 等受限目录启动，
# runuser 切换用户后无权读取会报 Permission denied，固定到中性目录。
cd /

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RELEASE_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
INSTALL_DIR="${OPENHANDS_BENCHMARKS_DIR:-/opt/uenv/agent/openhands-benchmarks}"
AGENT_USER="${UENV_AGENT_USER:-uenv-agent}"
AGENT_HOME="/var/lib/uenv/agent"
PIN_FILE="${UENV_OPENHANDS_PIN:-}"
UV_VERSION="${UENV_UV_VERSION:-0.8.14}"

usage() {
  cat <<'EOF'
用法：sudo bash install_openhands.sh [--install-dir DIR] [--pin FILE]

安装 UEnv SWE 评测所需的固定版本 OpenHands benchmarks 和 SDK。
默认安装目录：/opt/uenv/agent/openhands-benchmarks

可选环境变量：
  UENV_UV_VERSION          用于安装依赖的 uv 版本（默认 0.8.14）
  UENV_AGENT_USER         运行 OpenHands 的系统用户（默认 uenv-agent）
EOF
}

fail() {
  echo "错误：$*" >&2
  exit 1
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --install-dir)
      [[ $# -ge 2 ]] || fail "--install-dir 缺少参数"
      INSTALL_DIR="$2"
      shift 2
      ;;
    --pin)
      [[ $# -ge 2 ]] || fail "--pin 缺少参数"
      PIN_FILE="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      fail "未知参数：$1"
      ;;
  esac
done

[[ "$EUID" -eq 0 ]] || fail "请使用 sudo 运行此安装脚本"
case "$INSTALL_DIR" in
  /opt/uenv/agent/*) ;;
  *) fail "--install-dir 必须位于 /opt/uenv/agent/ 下" ;;
esac

if [[ -z "$PIN_FILE" ]]; then
  for candidate in \
    "/opt/uenv/current/share/swe/PIN.md" \
    "$RELEASE_ROOT/integrations/openhands/PIN.md"
  do
    if [[ -f "$candidate" ]]; then
      PIN_FILE="$candidate"
      break
    fi
  done
fi
[[ -n "$PIN_FILE" && -f "$PIN_FILE" ]] || fail "找不到本版本的 OpenHands PIN.md"

pin_value() {
  local key="$1"
  awk -F= -v wanted="$key" '$1 == wanted {print substr($0, length(wanted) + 2); exit}' "$PIN_FILE"
}

BENCHMARKS_REPO="$(pin_value OPENHANDS_BENCHMARKS_REPO)"
BENCHMARKS_SHA="$(pin_value OPENHANDS_BENCHMARKS_SHA)"
SDK_REPO="$(pin_value OPENHANDS_SDK_REPO)"
SDK_SHA="$(pin_value OPENHANDS_SDK_SHA)"

[[ "$BENCHMARKS_REPO" == https://github.com/OpenHands/* ]] \
  || fail "PIN.md 中的 benchmarks 仓库地址无效"
[[ "$SDK_REPO" == https://github.com/OpenHands/* ]] \
  || fail "PIN.md 中的 SDK 仓库地址无效"
[[ "$BENCHMARKS_SHA" =~ ^[0-9a-f]{40}$ ]] || fail "PIN.md 中的 benchmarks SHA 无效"
[[ "$SDK_SHA" =~ ^[0-9a-f]{40}$ ]] || fail "PIN.md 中的 SDK SHA 无效"

if ! command -v git >/dev/null 2>&1 || ! command -v python3 >/dev/null 2>&1; then
  if command -v apt-get >/dev/null 2>&1; then
    echo "==> 安装基础依赖"
    apt-get update
    DEBIAN_FRONTEND=noninteractive apt-get install -y ca-certificates git python3 python3-venv
  else
    fail "请先安装 git、python3 和 python3-venv"
  fi
fi
command -v runuser >/dev/null 2>&1 || fail "系统缺少 runuser（通常由 util-linux 提供）"

if ! getent group uenv >/dev/null; then
  groupadd --system uenv
fi
if ! id "$AGENT_USER" >/dev/null 2>&1; then
  useradd --system --gid uenv --home-dir "$AGENT_HOME" --shell /usr/sbin/nologin "$AGENT_USER"
fi

install -d -o "$AGENT_USER" -g uenv -m 0750 \
  "$AGENT_HOME" "$AGENT_HOME/.cache" /opt/uenv/agent

run_agent() {
  runuser -u "$AGENT_USER" -- env \
    HOME="$AGENT_HOME" \
    XDG_CACHE_HOME="$AGENT_HOME/.cache" \
    UV_CACHE_DIR="$AGENT_HOME/.cache/uv" \
    "$@"
}

if [[ -e "$INSTALL_DIR" && ! -d "$INSTALL_DIR/.git" ]]; then
  fail "$INSTALL_DIR 已存在但不是 Git 仓库，请先移走该目录后重试"
fi

if [[ ! -d "$INSTALL_DIR/.git" ]]; then
  echo "==> 下载 OpenHands benchmarks"
  run_agent git clone "$BENCHMARKS_REPO" "$INSTALL_DIR"
else
  origin="$(run_agent git -C "$INSTALL_DIR" remote get-url origin)"
  [[ "$origin" == "$BENCHMARKS_REPO" || "$origin" == "${BENCHMARKS_REPO%.git}.git" ]] \
    || fail "$INSTALL_DIR 的 origin 不是 PIN.md 指定的仓库"
  [[ -z "$(run_agent git -C "$INSTALL_DIR" status --porcelain --ignore-submodules=none)" ]] \
    || fail "$INSTALL_DIR 有未提交改动，为避免覆盖已停止安装"
fi

if ! run_agent git -C "$INSTALL_DIR" cat-file -e "${BENCHMARKS_SHA}^{commit}" 2>/dev/null; then
  echo "==> 获取固定版本"
  run_agent git -C "$INSTALL_DIR" fetch --depth 1 origin "$BENCHMARKS_SHA"
fi
run_agent git -C "$INSTALL_DIR" checkout --detach "$BENCHMARKS_SHA"
run_agent git -C "$INSTALL_DIR" submodule sync --recursive
run_agent git -C "$INSTALL_DIR" submodule update --init --recursive

SDK_DIR="$INSTALL_DIR/vendor/software-agent-sdk"
[[ -d "$SDK_DIR/.git" || -f "$SDK_DIR/.git" ]] || fail "benchmarks 未包含 software-agent-sdk 子模块"
SDK_ORIGIN="$(run_agent git -C "$SDK_DIR" remote get-url origin)"
[[ "$SDK_ORIGIN" == "$SDK_REPO" || "$SDK_ORIGIN" == "${SDK_REPO%.git}.git" ]] \
  || fail "software-agent-sdk 子模块仓库与 PIN.md 不一致"
ACTUAL_SDK_SHA="$(run_agent git -C "$SDK_DIR" rev-parse HEAD)"
[[ "$ACTUAL_SDK_SHA" == "$SDK_SHA" ]] \
  || fail "benchmarks 子模块版本与 PIN.md 不一致：期望 $SDK_SHA，实际 $ACTUAL_SDK_SHA"

if command -v uv >/dev/null 2>&1 \
  && run_agent "$(command -v uv)" --version >/dev/null 2>&1; then
  UV_BIN="$(command -v uv)"
else
  echo "==> 安装 uv $UV_VERSION"
  BOOTSTRAP_DIR="/opt/uenv/agent/uv-bootstrap"
  if [[ ! -x "$BOOTSTRAP_DIR/bin/python" ]]; then
    if ! python3 -m venv "$BOOTSTRAP_DIR"; then
      if command -v apt-get >/dev/null 2>&1; then
        echo "==> 补充安装 python3-venv"
        apt-get update
        DEBIAN_FRONTEND=noninteractive apt-get install -y python3-venv
        python3 -m venv "$BOOTSTRAP_DIR"
      else
        fail "无法创建 Python venv，请先安装 python3-venv"
      fi
    fi
  fi
  "$BOOTSTRAP_DIR/bin/python" -m pip install --disable-pip-version-check --upgrade \
    "uv==$UV_VERSION"
  UV_BIN="$BOOTSTRAP_DIR/bin/uv"
fi

echo "==> 创建固定依赖环境"
run_agent "$UV_BIN" python install 3.12
run_agent "$UV_BIN" --directory "$INSTALL_DIR" sync --frozen --python 3.12

PYTHON_BIN="$INSTALL_DIR/.venv/bin/python"
[[ -x "$PYTHON_BIN" ]] || fail "依赖安装完成，但未生成 $PYTHON_BIN"

AGENT_REQUIREMENTS=""
INTEGRATION_DIR=""
for candidate in \
  "/opt/uenv/current/share/swe/requirements-agent.txt" \
  "$RELEASE_ROOT/integrations/openhands/requirements-agent.txt"
do
  if [[ -f "$candidate" ]]; then
    AGENT_REQUIREMENTS="$candidate"
    break
  fi
done
for candidate in \
  "/opt/uenv/current/share/swe/openhands" \
  "$RELEASE_ROOT/integrations/openhands"
do
  if [[ -f "$candidate/uenv_runtime/agent_client.py" ]]; then
    INTEGRATION_DIR="$candidate"
    break
  fi
done
[[ -n "$AGENT_REQUIREMENTS" ]] || fail "安装包缺少 OpenHands Agent 依赖清单"
[[ -n "$INTEGRATION_DIR" ]] || fail "安装包缺少 UEnv OpenHands integration"

echo "==> 安装 UEnv AgentControl 客户端依赖"
run_agent "$UV_BIN" pip install --python "$PYTHON_BIN" --requirement "$AGENT_REQUIREMENTS"
run_agent env \
  PYTHONPATH="$INTEGRATION_DIR/uenv_runtime/gen:$INTEGRATION_DIR:$INSTALL_DIR" \
  "$PYTHON_BIN" -c \
  'from benchmarks.utils.llm_config import load_llm_config; from openhands.sdk import Agent, Conversation; from uenv_runtime.agent_client import _load_grpc_modules; _load_grpc_modules()'

echo
echo "OpenHands 已安装：$INSTALL_DIR"
echo "benchmarks：$BENCHMARKS_SHA"
echo "SDK：       $SDK_SHA"
