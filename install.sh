#!/usr/bin/env bash
set -euo pipefail

REPOSITORY="${UENV_GITHUB_REPOSITORY:-audreyyan1015/uenv}"
PROFILE="single-node"
VERSION="latest"
BUNDLE=""
SERVER_ENDPOINT=""
ADVERTISE_ENDPOINT=""
HUB_ENDPOINT=""
NO_START=0
FORCE_CONFIG=0

usage() {
  printf '%s\n' \
    'Install a prebuilt UEnv release on Linux.' \
    '' \
    'Usage: sudo ./install.sh [options]' \
    '' \
    '  --profile single-node|control-plane|worker|hub|full' \
    '  --bundle FILE          use a local release tarball' \
    '  --version VERSION      GitHub release tag (default: latest)' \
    '  --server HOST:PORT     control-plane address for a worker' \
    '  --advertise HOST:PORT  address used to reach this Worker' \
    '  --hub URL              enable the Worker Hub connection' \
    '  --no-start             install without starting systemd units' \
    '  --force-config         replace existing component configs' \
    '  -h, --help'
}

fail() {
  echo "安装失败：$*" >&2
  exit 1
}

info() {
  echo "==> $*"
}

while (($#)); do
  case "$1" in
    --profile) PROFILE="${2:-}"; shift 2 ;;
    --bundle) BUNDLE="${2:-}"; shift 2 ;;
    --version) VERSION="${2:-}"; shift 2 ;;
    --server) SERVER_ENDPOINT="${2:-}"; shift 2 ;;
    --advertise) ADVERTISE_ENDPOINT="${2:-}"; shift 2 ;;
    --hub) HUB_ENDPOINT="${2:-}"; shift 2 ;;
    --no-start) NO_START=1; shift ;;
    --force-config) FORCE_CONFIG=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) fail "未知参数：$1" ;;
  esac
done

case "$PROFILE" in
  single-node|control-plane|worker|hub|full) ;;
  *) fail "--profile 必须是 single-node、control-plane、worker、hub 或 full" ;;
esac

[[ "$(uname -s)" == "Linux" ]] || fail "当前安装器只支持 Linux"
[[ "${EUID:-$(id -u)}" -eq 0 ]] || fail "请使用 sudo 运行安装器"
for command in tar install sed ln cp; do
  command -v "$command" >/dev/null || fail "缺少命令：$command"
done
command -v python3 >/dev/null || fail "缺少 Python 3.10+（统一 uenv 命令需要）"
python3 -c 'import sys; raise SystemExit(0 if sys.version_info >= (3, 10) else 1)' \
  || fail "需要 Python 3.10 或更新版本"

case "$(uname -m)" in
  x86_64|amd64) ARCH="x86_64" ;;
  aarch64|arm64) fail "当前 Release 流程尚未发布 ARM64 安装包；请从源码构建" ;;
  *) fail "不支持的 CPU 架构：$(uname -m)" ;;
esac

TMP_DIR="$(mktemp -d -t uenv-install.XXXXXXXX)"
cleanup() {
  [[ -n "${TMP_DIR:-}" && -d "$TMP_DIR" ]] && rm -rf -- "$TMP_DIR"
}
trap cleanup EXIT

if [[ -z "$BUNDLE" ]]; then
  command -v curl >/dev/null || fail "在线安装需要 curl"
  ASSET="uenv-linux-${ARCH}.tar.gz"
  if [[ "$VERSION" == "latest" ]]; then
    BASE_URL="https://github.com/${REPOSITORY}/releases/latest/download"
  else
    BASE_URL="https://github.com/${REPOSITORY}/releases/download/${VERSION}"
  fi
  BUNDLE="$TMP_DIR/$ASSET"
  info "下载 UEnv ${VERSION} (${ARCH})"
  curl --fail --location --retry 3 --output "$BUNDLE" "$BASE_URL/$ASSET"
  curl --fail --location --retry 3 --output "$BUNDLE.sha256" "$BASE_URL/$ASSET.sha256"
  (cd "$TMP_DIR" && sha256sum --check "${ASSET}.sha256")
else
  [[ -f "$BUNDLE" ]] || fail "找不到安装包：$BUNDLE"
  BUNDLE="$(cd "$(dirname "$BUNDLE")" && pwd)/$(basename "$BUNDLE")"
  if [[ -f "$BUNDLE.sha256" ]]; then
    info "校验本地安装包"
    (cd "$(dirname "$BUNDLE")" && sha256sum --check "$(basename "$BUNDLE").sha256")
  fi
fi

if tar -tzf "$BUNDLE" | awk '/(^\/|(^|\/)\.\.($|\/))/ { bad=1 } END { exit bad ? 0 : 1 }'; then
  fail "安装包包含不安全的路径"
fi
tar -xzf "$BUNDLE" --no-same-owner -C "$TMP_DIR"
PAYLOAD="$(find "$TMP_DIR" -mindepth 1 -maxdepth 2 -type f -name manifest.json -printf '%h\n' | head -n1)"
[[ -n "$PAYLOAD" && -f "$PAYLOAD/VERSION" ]] || fail "安装包缺少 manifest.json 或 VERSION"
RELEASE_VERSION="$(tr -d '[:space:]' < "$PAYLOAD/VERSION")"
[[ "$RELEASE_VERSION" =~ ^[0-9A-Za-z._+-]+$ ]] || fail "安装包版本号非法"

RELEASE_DIR="/opt/uenv/releases/$RELEASE_VERSION"
info "安装 UEnv $RELEASE_VERSION（$PROFILE）"
install -d -m 0755 /opt/uenv/releases "$RELEASE_DIR" /etc/uenv/secrets /var/log/uenv
cp -a "$PAYLOAD/." "$RELEASE_DIR/"
ln -sfn "$RELEASE_DIR" /opt/uenv/current
ln -sfn /opt/uenv/current/bin/uenv /usr/local/bin/uenv
ln -sfn /opt/uenv/current/bin/uenv /usr/local/bin/uenv-ctl

if ! getent group uenv >/dev/null; then
  groupadd --system uenv
fi
if ! id uenv >/dev/null 2>&1; then
  useradd --system --gid uenv --home-dir /var/lib/uenv --shell /usr/sbin/nologin uenv
fi
install -d -o uenv -g uenv -m 0750 \
  /var/lib/uenv/server /var/lib/uenv/worker /var/lib/uenv/hub \
  /var/lib/uenv/server/obs /var/lib/uenv/server/trajectory \
  /var/lib/uenv/worker/wal /var/lib/uenv/hub/artifacts
chown -R uenv:uenv /var/log/uenv

install_config() {
  local source="$1" target="$2" mode="${3:-0644}"
  if [[ -e "$target" && "$FORCE_CONFIG" -ne 1 ]]; then
    info "保留已有配置 $target"
  else
    install -m "$mode" "$source" "$target"
  fi
}

UNITS=()
if [[ "$PROFILE" == "single-node" || "$PROFILE" == "control-plane" || "$PROFILE" == "full" ]]; then
  install_config "$RELEASE_DIR/config/server.yaml" /etc/uenv/server.yaml
  install_config "$RELEASE_DIR/config/server.env" /etc/uenv/server.env
  install -m 0644 "$RELEASE_DIR/systemd/uenv-adapter-core.service" /etc/systemd/system/uenv-adapter-core.service
  UNITS+=(uenv-adapter-core.service)
fi

if [[ "$PROFILE" == "single-node" || "$PROFILE" == "worker" || "$PROFILE" == "full" ]]; then
  if [[ -z "$SERVER_ENDPOINT" ]]; then
    if [[ "$PROFILE" == "worker" ]]; then
      fail "worker 安装需要 --server HOST:PORT"
    fi
    SERVER_ENDPOINT="127.0.0.1:50051"
  fi
  [[ "$SERVER_ENDPOINT" =~ ^[A-Za-z0-9._:-]+$ ]] || fail "--server 格式非法"
  if [[ -z "$ADVERTISE_ENDPOINT" ]]; then
    if [[ "$PROFILE" == "worker" ]]; then
      HOST_IP="$(hostname -I 2>/dev/null | awk '{print $1}')"
      [[ -n "$HOST_IP" ]] || fail "无法自动确定 Worker IP；请传入 --advertise HOST:50054"
      ADVERTISE_ENDPOINT="${HOST_IP}:50054"
    else
      ADVERTISE_ENDPOINT="127.0.0.1:50054"
    fi
  fi
  [[ "$ADVERTISE_ENDPOINT" =~ ^[A-Za-z0-9._:-]+$ ]] || fail "--advertise 格式非法"
  HUB_ENABLED=false
  if [[ -n "$HUB_ENDPOINT" ]]; then
    HUB_ENABLED=true
  elif [[ "$PROFILE" == "full" ]]; then
    HUB_ENDPOINT="http://127.0.0.1:8080"
    HUB_ENABLED=true
  else
    HUB_ENDPOINT="http://127.0.0.1:8080"
  fi
  sed \
    -e "s|@SERVER_ENDPOINT@|$SERVER_ENDPOINT|g" \
    -e "s|@ADVERTISE_ENDPOINT@|$ADVERTISE_ENDPOINT|g" \
    -e "s|@HUB_ENABLED@|$HUB_ENABLED|g" \
    -e "s|@HUB_ENDPOINT@|$HUB_ENDPOINT|g" \
    "$RELEASE_DIR/config/worker.yaml" > "$TMP_DIR/worker.yaml"
  install_config "$TMP_DIR/worker.yaml" /etc/uenv/worker.yaml
  install_config "$RELEASE_DIR/config/worker.env" /etc/uenv/worker.env
  if [[ ! -e /etc/uenv/secrets/worker-llm.env ]]; then
    install -m 0600 /dev/null /etc/uenv/secrets/worker-llm.env
  fi
  install -m 0644 "$RELEASE_DIR/systemd/uenv-worker.service" /etc/systemd/system/uenv-worker.service
  UNITS+=(uenv-worker.service)
fi

if [[ "$PROFILE" == "hub" || "$PROFILE" == "full" ]]; then
  install_config "$RELEASE_DIR/config/hub.toml" /etc/uenv/hub.toml
  install -m 0644 "$RELEASE_DIR/systemd/uenv-hub.service" /etc/systemd/system/uenv-hub.service
  UNITS+=(uenv-hub.service)
fi

if [[ "$NO_START" -eq 0 ]]; then
  command -v systemctl >/dev/null || fail "找不到 systemctl；可使用 --no-start 仅安装文件"
  systemctl daemon-reload
  for unit in "${UNITS[@]}"; do
    systemctl enable "$unit" >/dev/null
    systemctl restart "$unit"
  done
  info "等待服务就绪"
  sleep 2
fi

echo
echo "UEnv $RELEASE_VERSION 已安装。"
echo "  检查：sudo -u uenv uenv doctor"
echo "  状态：uenv status"
echo "  日志：uenv logs server（或 worker / hub）"
if [[ "$NO_START" -eq 1 ]]; then
  echo "服务尚未启动；检查配置后运行：systemctl enable --now ${UNITS[*]}"
fi
