#!/usr/bin/env bash
# hub-stage-image-package.sh — 在 Hub 主机上「预制存储」SWE-bench 镜像，供 Worker 直接从
# Hub 拉取（docker load），不再联网到第三方 registry。
#
# 用法（在 Hub 主机 8.130.95.176 上执行；已 docker + 已构建/安装 `uenv` CLI）：
#   scripts/hub-stage-image-package.sh <package_id> <version> <image> [<image> ...]
#
# 例：
#   scripts/hub-stage-image-package.sh swe-bench-verified-images 0.1.0 \
#       swebench/sweb.eval.x86_64.django_1776_django-11095:latest
#
# 流程：
#   1) 确保每个镜像本地存在（缺失则按镜像源拉取一次，仅此一次在 Hub 侧联网）；
#   2) `docker save` 成 tar 到 $STAGE_DIR；
#   3) `uenv env publish-image` 把这些 tar 作为 image_tar 制品发布进 Hub（流式入库、算 sha256）；
#   4) Worker 侧：`uenv env sync <package_id> --docker-load`（或经 EnvPackage 目录自动 docker load）。
#
# 环境变量：
#   ENGINE       docker|podman（默认 docker）
#   STAGE_DIR    tar 暂存目录（默认 /var/lib/uenv/hub-image-stage）
#   UENV_BIN     uenv CLI 路径（默认 PATH 中的 uenv）
#   UENV_HUB_ENDPOINT / UENV_HUB_TOKEN  发布所需（Publisher token）
#   KEEP_TARS    非空则保留暂存 tar（默认发布后删除）
set -euo pipefail

if [[ $# -lt 3 ]]; then
  echo "usage: $0 <package_id> <version> <image> [<image> ...]" >&2
  exit 2
fi

PACKAGE_ID="$1"; shift
VERSION="$1"; shift
IMAGES=("$@")

ENGINE="${ENGINE:-docker}"
STAGE_DIR="${STAGE_DIR:-/var/lib/uenv/hub-image-stage}"
UENV_BIN="${UENV_BIN:-uenv}"

mkdir -p "$STAGE_DIR"

# 镜像名 → 文件系统安全的 tar 基名（与 uenv-hub-core::seed::sanitize_tar_name 语义一致）。
sanitize() { echo "$1" | tr -c 'A-Za-z0-9._-' '-'; }

declare -a TAR_ARGS=()
declare -a TAR_FILES=()
for img in "${IMAGES[@]}"; do
  if ! "$ENGINE" image inspect "$img" >/dev/null 2>&1; then
    echo "== pull $img (一次性，Hub 侧联网) =="
    "$ENGINE" pull "$img"
  fi
  base="$(sanitize "$img").tar"
  tar_path="$STAGE_DIR/$base"
  echo "== save $img -> $tar_path =="
  "$ENGINE" save -o "$tar_path" "$img"
  TAR_ARGS+=(--tar "$tar_path")
  TAR_FILES+=("$tar_path")
done

echo "== publish ${#IMAGES[@]} image tarball(s) to Hub package $PACKAGE_ID@$VERSION =="
"$UENV_BIN" env publish-image "$PACKAGE_ID" --version "$VERSION" "${TAR_ARGS[@]}"

if [[ -z "${KEEP_TARS:-}" ]]; then
  echo "== cleanup staged tars (set KEEP_TARS=1 to keep) =="
  for f in "${TAR_FILES[@]}"; do rm -f "$f"; done
fi

cat <<EOF

done. Worker 侧现在可直接从 Hub 拉取镜像（无需第三方 registry）：
  uenv env sync $PACKAGE_ID --version $VERSION --docker-load
或将同步目录经 UENV_SWE_ENV_PACKAGE 交给 uenv-worker，由池在 provision/prewarm 时自动 docker load。
EOF
