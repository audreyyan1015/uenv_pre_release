#!/usr/bin/env bash
# Publish or install an OCI image archive through UEnv Hub.
#
# This helper is only for Workers that cannot pull from a container registry.
# Online Workers should pull an image by immutable digest from a private
# registry instead of copying a tar archive through Hub.
set -euo pipefail

fail() {
  echo "错误：$*" >&2
  exit 1
}

usage() {
  cat <<'EOF'
通过 UEnv Hub 分发离线容器镜像

用法：
  image_bundle.sh publish \
    --package PACKAGE --version VERSION --image IMAGE@sha256:DIGEST [选项]

  image_bundle.sh install \
    --package PACKAGE --version VERSION [选项]

publish 在 UEnv Hub 主机执行：
  1. 将本机已有镜像导出到 UEnv Hub 的受控导入目录；
  2. 发布一个只包含镜像 tar 的 EnvPackage。

install 在离线 UEnv Worker 执行：
  1. 从 UEnv Hub 下载并校验该 EnvPackage；
  2. 自动执行 docker/podman load。

共同选项：
  --package ID          UEnv Hub 中的 EnvPackage ID
  --version VERSION     不可覆盖的 EnvPackage 版本
  --engine NAME         docker 或 podman（默认 docker）

publish 选项：
  --image REF           UEnv Hub 主机上已存在的镜像，推荐使用 @sha256:digest
  --import-dir DIR      UEnv Hub 受控导入目录（默认 /var/lib/uenv/hub/import）
  --worker-min VERSION  最低 UEnv Worker 版本（默认取 uenv version）

install 选项：
  --target-dir DIR      UEnv Worker 同步根目录（默认 /var/lib/uenv）
  --worker-version VER  当前 UEnv Worker 版本（默认取 uenv version）

UEnv Hub 启用访问令牌鉴权时，先为当前 root 用户执行 `uenv hub login`：
publish 使用发布者令牌，install 使用只读令牌。本机无鉴权 UEnv Hub 可跳过登录。
脚本必须由 root 运行。
EOF
}

installed_version() {
  local value
  value="$(uenv version 2>/dev/null | awk 'NR == 1 {print $2}')"
  [[ -n "$value" ]] || fail "无法读取 UEnv 版本，请显式传入版本参数"
  printf '%s\n' "$value"
}

require_root() {
  [[ "${EUID:-$(id -u)}" -eq 0 ]] || fail "请使用 sudo 运行此命令"
}

validate_id() {
  local label="$1" value="$2"
  [[ "$value" =~ ^[A-Za-z0-9][A-Za-z0-9._+-]*$ ]] \
    || fail "$label 只能包含字母、数字、点、下划线、加号和连字符：$value"
}

command_name="${1:-help}"
if (($#)); then
  shift
fi

PACKAGE=""
VERSION=""
IMAGE=""
ENGINE="docker"
IMPORT_DIR="/var/lib/uenv/hub/import"
TARGET_DIR="/var/lib/uenv"
WORKER_VERSION=""

while (($#)); do
  case "$1" in
    --package) PACKAGE="${2:-}"; shift 2 ;;
    --version) VERSION="${2:-}"; shift 2 ;;
    --image) IMAGE="${2:-}"; shift 2 ;;
    --engine) ENGINE="${2:-}"; shift 2 ;;
    --import-dir) IMPORT_DIR="${2:-}"; shift 2 ;;
    --target-dir) TARGET_DIR="${2:-}"; shift 2 ;;
    --worker-min|--worker-version) WORKER_VERSION="${2:-}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) fail "未知参数：$1" ;;
  esac
done

case "$command_name" in
  help|-h|--help)
    usage
    exit 0
    ;;
  publish|install) ;;
  *) fail "未知命令：$command_name（应为 publish 或 install）" ;;
esac

require_root
[[ "$ENGINE" == "docker" || "$ENGINE" == "podman" ]] \
  || fail "--engine 必须是 docker 或 podman"
command -v "$ENGINE" >/dev/null 2>&1 || fail "找不到容器命令：$ENGINE"
command -v uenv >/dev/null 2>&1 || fail "找不到 uenv CLI"
[[ -n "$PACKAGE" ]] || fail "缺少 --package"
[[ -n "$VERSION" ]] || fail "缺少 --version"
validate_id "--package" "$PACKAGE"
validate_id "--version" "$VERSION"
WORKER_VERSION="${WORKER_VERSION:-$(installed_version)}"

if [[ "$command_name" == "publish" ]]; then
  [[ -n "$IMAGE" ]] || fail "publish 缺少 --image"
  "$ENGINE" image inspect "$IMAGE" >/dev/null 2>&1 \
    || fail "本机容器运行时中没有镜像：$IMAGE"

  install -d -o uenv -g uenv -m 0750 "$IMPORT_DIR"
  archive="$IMPORT_DIR/${PACKAGE}-${VERSION}.tar"
  [[ ! -e "$archive" ]] \
    || fail "$archive 已存在；版本制品不可覆盖，请改用新版本或由管理员确认旧文件"

  echo "==> 导出镜像 $IMAGE"
  "$ENGINE" save -o "$archive" "$IMAGE"
  chown uenv:uenv "$archive"
  chmod 0640 "$archive"

  echo "==> 发布 $PACKAGE@$VERSION"
  uenv env publish-image "$PACKAGE" \
    --version "$VERSION" \
    --tar "$archive" \
    --worker-min "$WORKER_VERSION"
  echo "已发布。保留 $archive，直到至少一台 Worker 完成下载验收。"
  exit 0
fi

echo "==> 下载并校验 $PACKAGE@$VERSION"
sync_args=(
  env sync "$PACKAGE"
  --version "$VERSION"
  --target-dir "$TARGET_DIR"
  --consumer worker
  --worker-version "$WORKER_VERSION"
)
command -v python3 >/dev/null 2>&1 || fail "找不到 python3"
uenv "${sync_args[@]}"

if [[ "$ENGINE" == "podman" ]]; then
  # Root and rootless Podman have separate image stores. Verify the same
  # service account that runs Worker owns a usable store.
  command -v runuser >/dev/null 2>&1 || fail "找不到 runuser"
  id uenv >/dev/null 2>&1 || fail "缺少 uenv 服务用户"
  runuser -u uenv -- podman info >/dev/null \
    || fail "uenv 用户的 rootless Podman 尚不可用；请先为该系统用户配置 subuid/subgid 和运行时目录"
fi

# Always perform the load explicitly, including when the package was already
# synced earlier. `uenv env sync` may legitimately return its verified cache;
# the container image store can still be empty after cleanup or reinstallation.
manifest="$TARGET_DIR/envs/$PACKAGE/$VERSION/manifest.json"
[[ -f "$manifest" ]] || fail "同步完成但找不到 manifest：$manifest"
image_list="$(mktemp -p "$TARGET_DIR" .uenv-image-list.XXXXXXXX)"
cleanup_image_list() {
  rm -f -- "$image_list"
}
trap cleanup_image_list EXIT HUP INT TERM
python3 - "$manifest" > "$image_list" <<'PY'
import json
import sys
from pathlib import Path, PurePosixPath

manifest = Path(sys.argv[1])
root = manifest.parent
value = json.loads(manifest.read_text(encoding="utf-8"))
for artifact in value.get("artifacts", []):
    if artifact.get("kind") != "image_tar" or artifact.get("sync_mode") != "inline":
        continue
    relative = PurePosixPath(str(artifact.get("target_rel_path", "")))
    if relative.is_absolute() or not relative.parts or ".." in relative.parts:
        raise SystemExit(f"unsafe image target in manifest: {relative}")
    path = root.joinpath(*relative.parts)
    if not path.is_file():
        raise SystemExit(f"synced image tar is missing: {path}")
    print(path, end="\0")
PY
mapfile -d '' image_tars < "$image_list"
rm -f -- "$image_list"
trap - EXIT HUP INT TERM
[[ "${#image_tars[@]}" -gt 0 ]] || fail "package 中没有可导入的 image_tar"
for archive in "${image_tars[@]}"; do
  if [[ "$ENGINE" == "podman" ]]; then
    runuser -u uenv -- podman load -i "$archive"
  else
    docker load -i "$archive"
  fi
done
echo "镜像已导入 $ENGINE。"
