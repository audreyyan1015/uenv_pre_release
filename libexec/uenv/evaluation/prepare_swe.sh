#!/usr/bin/env bash
# Enable UEnv's SWE runtime and install the OpenHands version pinned by the release.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RELEASE_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
INSTALLER="${UENV_INSTALLER:-$RELEASE_ROOT/install.sh}"
BUNDLE=""
PROFILE=""
RUNTIME=""
IMAGE_POLICY=""
GATEWAY=""
GATEWAY_PUBLIC=""
SERVER=""
ADVERTISE=""
TRAJECTORY_ENDPOINT=""
SHARED_KEY_FILE=""
FORCE_SWE_CONFIG=0
RESET_SWE_KEY=0
OPENHANDS_INSTALLER="${UENV_OPENHANDS_INSTALLER:-}"

usage() {
  cat <<'EOF'
一次完成 SWE 前置准备：启用 UEnv Runtime Gateway，并安装 release 固定版本的 OpenHands。

用法：
  sudo uenv evaluate prepare-swe --bundle FILE [选项]

必填：
  --bundle FILE              与当前 UEnv release 对应的安装包

常用选项：
  --installer FILE           install.sh 路径；默认使用当前 release 中的脚本
  --profile PROFILE          single-node、full、control-plane 或 worker（必填）
  --runtime NAME             Worker 使用的 docker 或 podman
  --image-policy POLICY      Worker 使用的 local_only 或 allow_public
  --gateway HOST:PORT        SWE Runtime Gateway 监听地址
  --gateway-public URL       OpenHands Agent 访问 SWE Runtime Gateway 的 URL
  --server HOST:PORT         UEnv Worker 连接的 UEnv Server gRPC 地址
  --advertise HOST:PORT      UEnv Server 访问 UEnv Worker gRPC 的地址
  --trajectory-endpoint URL  UEnv Worker/OpenHands Agent 上传交互轨迹的 UEnv Server URL
  --shared-key-file FILE     多机共享密钥；Server 主机可使用尚不存在的路径安全生成
  --force-swe-config         用本次参数替换已有 /etc/uenv/swe.env
  --reset-swe-key            协调所有节点后轮换为 --shared-key-file 中的 key

single-node/full 在同一主机准备控制面、Worker、Gateway 和 OpenHands。多机先在
control-plane 节点配置共享 key，再在每台 worker 节点用同一个 key 启用 Gateway；
OpenHands 安装在包含 Worker 的节点。容器运行时须事先可用。
EOF
}

fail() {
  echo "prepare_swe.sh: $*" >&2
  exit 1
}

need_value() {
  [[ $# -ge 2 && -n "${2:-}" ]] || fail "$1 缺少参数"
}

while (($#)); do
  case "$1" in
    --installer)
      need_value "$@"
      INSTALLER="$2"
      shift 2
      ;;
    --bundle)
      need_value "$@"
      BUNDLE="$2"
      shift 2
      ;;
    --profile)
      need_value "$@"
      PROFILE="$2"
      shift 2
      ;;
    --runtime)
      need_value "$@"
      RUNTIME="$2"
      shift 2
      ;;
    --image-policy)
      need_value "$@"
      IMAGE_POLICY="$2"
      shift 2
      ;;
    --gateway)
      need_value "$@"
      GATEWAY="$2"
      shift 2
      ;;
    --gateway-public)
      need_value "$@"
      GATEWAY_PUBLIC="$2"
      shift 2
      ;;
    --server)
      need_value "$@"
      SERVER="$2"
      shift 2
      ;;
    --advertise)
      need_value "$@"
      ADVERTISE="$2"
      shift 2
      ;;
    --trajectory-endpoint)
      need_value "$@"
      TRAJECTORY_ENDPOINT="$2"
      shift 2
      ;;
    --shared-key-file)
      need_value "$@"
      SHARED_KEY_FILE="$2"
      shift 2
      ;;
    --force-swe-config)
      FORCE_SWE_CONFIG=1
      shift
      ;;
    --reset-swe-key)
      RESET_SWE_KEY=1
      shift
      ;;
    -h|--help|help)
      usage
      exit 0
      ;;
    *)
      fail "未知参数：$1"
      ;;
  esac
done

[[ -n "$BUNDLE" ]] || fail "必须传入 --bundle FILE"
[[ -n "$PROFILE" ]] || fail "必须传入 --profile single-node|full|control-plane|worker"
[[ "${EUID:-$(id -u)}" -eq 0 ]] || fail "请使用 sudo 运行 prepare-swe"
[[ -f "$BUNDLE" ]] || fail "找不到安装包：$BUNDLE"
[[ -f "$INSTALLER" ]] || fail "找不到安装脚本：$INSTALLER"
case "$PROFILE" in
  single-node|full|control-plane|worker) ;;
  *) fail "--profile 必须是 single-node、full、control-plane 或 worker" ;;
esac

if [[ "$PROFILE" == "single-node" || "$PROFILE" == "full" || "$PROFILE" == "worker" ]]; then
  [[ -n "$RUNTIME" ]] || fail "$PROFILE 需要 --runtime docker|podman"
  [[ -n "$IMAGE_POLICY" ]] || fail "$PROFILE 需要 --image-policy local_only|allow_public"
  [[ -n "$GATEWAY" ]] || fail "$PROFILE 需要 --gateway HOST:PORT"
  case "$RUNTIME" in
    docker|podman) ;;
    *) fail "--runtime 必须是 docker 或 podman" ;;
  esac
  case "$IMAGE_POLICY" in
    local_only|allow_public) ;;
    *) fail "--image-policy 必须是 local_only 或 allow_public" ;;
  esac
fi

if [[ "$PROFILE" == "control-plane" || "$PROFILE" == "worker" ]]; then
  [[ -n "$SHARED_KEY_FILE" ]] \
    || fail "$PROFILE 多机准备需要 --shared-key-file FILE"
  if [[ "$PROFILE" == "control-plane" && ! -e "$SHARED_KEY_FILE" && ! -L "$SHARED_KEY_FILE" ]]; then
    python3 - "$SHARED_KEY_FILE" <<'PY' \
      || fail "无法安全生成 --shared-key-file"
import os
import secrets
import sys

path = os.path.abspath(sys.argv[1])
parent = os.path.dirname(path)
if not os.path.isdir(parent):
    raise SystemExit(f"共享 key 的父目录不存在：{parent}")
flags = (
    os.O_WRONLY
    | os.O_CREAT
    | os.O_EXCL
    | getattr(os, "O_CLOEXEC", 0)
    | getattr(os, "O_NOFOLLOW", 0)
)
descriptor = os.open(path, flags, 0o600)
try:
    payload = (secrets.token_hex(32) + "\n").encode("ascii")
    written = 0
    while written < len(payload):
        written += os.write(descriptor, payload[written:])
    os.fchmod(descriptor, 0o600)
    os.fsync(descriptor)
except BaseException:
    os.close(descriptor)
    os.unlink(path)
    raise
else:
    os.close(descriptor)
parent_descriptor = os.open(parent, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
try:
    os.fsync(parent_descriptor)
finally:
    os.close(parent_descriptor)
PY
    echo "==> 已生成共享 Gateway key 文件：$SHARED_KEY_FILE"
  fi
  [[ -f "$SHARED_KEY_FILE" && ! -L "$SHARED_KEY_FILE" ]] \
    || fail "--shared-key-file 必须指向普通文件，不能是符号链接"
fi
if [[ "$PROFILE" == "worker" ]]; then
  [[ -n "$SERVER" ]] || fail "worker 需要 --server HOST:PORT"
  [[ -n "$ADVERTISE" ]] || fail "worker 需要 --advertise HOST:PORT"
  [[ -n "$GATEWAY_PUBLIC" ]] || fail "worker 需要 --gateway-public URL"
  [[ -n "$TRAJECTORY_ENDPOINT" ]] \
    || fail "worker 需要 --trajectory-endpoint http://<CONTROL_PLANE>:8077"
fi

install_args=(
  --bundle "$BUNDLE"
  --profile "$PROFILE"
  --enable-swe
)
if [[ "$PROFILE" == "single-node" || "$PROFILE" == "full" || "$PROFILE" == "worker" ]]; then
  install_args+=(
    --swe-runtime "$RUNTIME"
    --swe-image-policy "$IMAGE_POLICY"
    --swe-gateway "$GATEWAY"
  )
fi
if [[ -n "$GATEWAY_PUBLIC" ]]; then
  install_args+=(--swe-gateway-public "$GATEWAY_PUBLIC")
fi
if [[ -n "$SERVER" ]]; then
  install_args+=(--server "$SERVER")
fi
if [[ -n "$ADVERTISE" ]]; then
  install_args+=(--advertise "$ADVERTISE")
fi
if [[ -n "$TRAJECTORY_ENDPOINT" ]]; then
  install_args+=(--swe-trajectory-endpoint "$TRAJECTORY_ENDPOINT")
fi
if [[ -n "$SHARED_KEY_FILE" ]]; then
  install_args+=(--swe-shared-key-file "$SHARED_KEY_FILE")
fi
if [[ "$FORCE_SWE_CONFIG" -eq 1 ]]; then
  install_args+=(--force-swe-config)
fi
if [[ "$RESET_SWE_KEY" -eq 1 ]]; then
  install_args+=(--reset-swe-key)
fi

echo "==> 准备 UEnv SWE 角色：$PROFILE"
bash "$INSTALLER" "${install_args[@]}"

if [[ "$PROFILE" == "control-plane" ]]; then
  cat <<'EOF'

SWE 控制面准备完成。请在每台 Worker 上使用同一个 --shared-key-file 运行 prepare-swe。
EOF
  exit 0
fi

if [[ -z "$OPENHANDS_INSTALLER" ]]; then
  for candidate in \
    /opt/uenv/current/libexec/uenv/swe/install_openhands.sh \
    "$RELEASE_ROOT/libexec/uenv/swe/install_openhands.sh"
  do
    if [[ -f "$candidate" ]]; then
      OPENHANDS_INSTALLER="$candidate"
      break
    fi
  done
fi
[[ -n "$OPENHANDS_INSTALLER" && -f "$OPENHANDS_INSTALLER" ]] \
  || fail "UEnv 已启用 SWE，但找不到 install_openhands.sh"

echo "==> 安装 release 固定版本的 OpenHands"
bash "$OPENHANDS_INSTALLER"

cat <<'EOF'

SWE Worker/单机前置准备完成。
下一步请用 `uenv evaluate run-swe --help` 明确指定 provider、catalog、实例、模型和输出目录。
首次评测会按所选实例准备容器镜像；UEnv 不会下载模型或完整 benchmark 数据集。
EOF
