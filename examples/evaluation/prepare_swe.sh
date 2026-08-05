#!/usr/bin/env bash
# Enable UEnv's SWE runtime and install the OpenHands version pinned by the release.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RELEASE_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
INSTALLER="${UENV_INSTALLER:-$RELEASE_ROOT/install.sh}"
BUNDLE=""
PROFILE=""
RUNTIME=""
IMAGE_POLICY=""
GATEWAY=""
GATEWAY_PUBLIC=""
FORCE_SWE_CONFIG=0
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
  --profile PROFILE          single-node 或 full（必填）
  --runtime NAME             docker 或 podman（必填）
  --image-policy POLICY      local_only 或 allow_public（必填）
  --gateway HOST:PORT        Runtime Gateway 监听地址（必填）
  --gateway-public URL       Agent 访问 Gateway 的地址；跨主机时必须设置
  --force-swe-config         用本次参数替换已有 /etc/uenv/swe.env

脚本不会安装 Docker/Podman、模型、benchmark 数据或任务镜像。容器运行时必须
事先可用；所选 SWE 实例的镜像会在首次评测时按需拉取。
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
    --force-swe-config)
      FORCE_SWE_CONFIG=1
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
[[ -n "$PROFILE" ]] || fail "必须传入 --profile single-node|full"
[[ -n "$RUNTIME" ]] || fail "必须传入 --runtime docker|podman"
[[ -n "$IMAGE_POLICY" ]] || fail "必须传入 --image-policy local_only|allow_public"
[[ -n "$GATEWAY" ]] || fail "必须传入 --gateway HOST:PORT"
[[ -f "$BUNDLE" ]] || fail "找不到安装包：$BUNDLE"
[[ -f "$INSTALLER" ]] || fail "找不到安装脚本：$INSTALLER"
case "$PROFILE" in
  single-node|full) ;;
  *) fail "--profile 必须是 single-node 或 full" ;;
esac
case "$RUNTIME" in
  docker|podman) ;;
  *) fail "--runtime 必须是 docker 或 podman" ;;
esac
case "$IMAGE_POLICY" in
  local_only|allow_public) ;;
  *) fail "--image-policy 必须是 local_only 或 allow_public" ;;
esac

install_args=(
  --bundle "$BUNDLE"
  --profile "$PROFILE"
  --enable-swe
  --swe-runtime "$RUNTIME"
  --swe-image-policy "$IMAGE_POLICY"
  --swe-gateway "$GATEWAY"
)
if [[ -n "$GATEWAY_PUBLIC" ]]; then
  install_args+=(--swe-gateway-public "$GATEWAY_PUBLIC")
fi
if [[ "$FORCE_SWE_CONFIG" -eq 1 ]]; then
  install_args+=(--force-swe-config)
fi

echo "==> 启用 UEnv SWE Runtime Gateway"
bash "$INSTALLER" "${install_args[@]}"

if [[ -z "$OPENHANDS_INSTALLER" ]]; then
  for candidate in \
    /opt/uenv/current/examples/swe/install_openhands.sh \
    "$RELEASE_ROOT/examples/swe/install_openhands.sh"
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

SWE 前置准备完成。
下一步请用 `uenv evaluate run-swe --help` 明确指定 provider、catalog、实例、模型和输出目录。
首次评测会按所选实例准备容器镜像；UEnv 不会下载模型或完整 benchmark 数据集。
EOF
