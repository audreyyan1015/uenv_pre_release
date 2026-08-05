#!/usr/bin/env bash
set -euo pipefail

REPOSITORY="${UENV_GITHUB_REPOSITORY:-audreyyan1015/uenv_pre_release}"
PROFILE="single-node"
VERSION="latest"
BUNDLE=""
SERVER_ENDPOINT=""
ADVERTISE_ENDPOINT=""
HUB_ENDPOINT=""
HUB_TOKEN_FILE=""
NO_START=0
FORCE_CONFIG=0
FORCE_SWE_CONFIG=0
RESET_SWE_KEY=0
ENABLE_SWE=0
SWE_RUNTIME="docker"
SWE_GATEWAY_BIND="127.0.0.1:28999"
SWE_GATEWAY_PUBLIC=""
SWE_IMAGE_PULL_POLICY="local_only"

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
    '  --hub-token-file FILE  Worker Reader token; copied to a protected local file' \
    '  --enable-swe           enable the local SWE runtime and gateway' \
    '  --swe-runtime NAME     container runtime: docker or podman (default: docker)' \
    '  --swe-gateway HOST:PORT  SWE gateway bind address (default: 127.0.0.1:28999)' \
    '  --swe-gateway-public URL address advertised to the local/remote Agent' \
    '  --swe-image-policy POLICY  local_only or allow_public (default: local_only)' \
    '  --no-start             install without starting systemd units' \
    '  --force-config         replace existing component configs' \
    '  --force-swe-config     replace only the generated SWE runtime config' \
    '  --reset-swe-key        generate a new shared Server/Worker Gateway key' \
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
    --hub-token-file) HUB_TOKEN_FILE="${2:-}"; shift 2 ;;
    --enable-swe) ENABLE_SWE=1; shift ;;
    --swe-runtime) SWE_RUNTIME="${2:-}"; shift 2 ;;
    --swe-gateway) SWE_GATEWAY_BIND="${2:-}"; shift 2 ;;
    --swe-gateway-public) SWE_GATEWAY_PUBLIC="${2:-}"; shift 2 ;;
    --swe-image-policy) SWE_IMAGE_PULL_POLICY="${2:-}"; shift 2 ;;
    --no-start) NO_START=1; shift ;;
    --force-config) FORCE_CONFIG=1; shift ;;
    --force-swe-config) FORCE_SWE_CONFIG=1; shift ;;
    --reset-swe-key) RESET_SWE_KEY=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) fail "未知参数：$1" ;;
  esac
done

case "$PROFILE" in
  single-node|control-plane|worker|hub|full) ;;
  *) fail "--profile 必须是 single-node、control-plane、worker、hub 或 full" ;;
esac

if [[ -n "$HUB_TOKEN_FILE" ]]; then
  [[ -f "$HUB_TOKEN_FILE" ]] || fail "找不到 Hub token 文件：$HUB_TOKEN_FILE"
  [[ -s "$HUB_TOKEN_FILE" ]] || fail "Hub token 文件为空：$HUB_TOKEN_FILE"
  if [[ -z "$HUB_ENDPOINT" && "$PROFILE" != "full" ]]; then
    fail "--hub-token-file 需要同时指定 --hub URL（full profile 除外）"
  fi
  HUB_TOKEN_FILE="$(cd "$(dirname "$HUB_TOKEN_FILE")" && pwd)/$(basename "$HUB_TOKEN_FILE")"
fi

if [[ "$ENABLE_SWE" -eq 1 ]]; then
  [[ "$PROFILE" == "single-node" || "$PROFILE" == "full" ]] \
    || fail "--enable-swe 当前只支持 single-node 或 full；SWE Agent 需要控制面和 Worker"
  [[ "$SWE_RUNTIME" == "docker" || "$SWE_RUNTIME" == "podman" ]] \
    || fail "--swe-runtime 必须是 docker 或 podman"
  python3 - "$SWE_GATEWAY_BIND" <<'PY' \
    || fail "--swe-gateway 当前只支持 IPv4:PORT，例如 127.0.0.1:28999"
import ipaddress
import sys

host, separator, port = sys.argv[1].rpartition(":")
if not separator:
    raise SystemExit(1)
ipaddress.IPv4Address(host)
if not port.isdigit() or not 1 <= int(port) <= 65535:
    raise SystemExit(1)
PY
  [[ "$SWE_IMAGE_PULL_POLICY" == "local_only" || "$SWE_IMAGE_PULL_POLICY" == "allow_public" ]] \
    || fail "--swe-image-policy 必须是 local_only 或 allow_public"
  if [[ -z "$SWE_GATEWAY_PUBLIC" ]]; then
    if [[ "$SWE_GATEWAY_BIND" == 0.0.0.0:* || "$SWE_GATEWAY_BIND" == \[*\]:* ]]; then
      fail "网关监听非回环地址时必须传 --swe-gateway-public http://<Agent可达地址>:端口"
    fi
    SWE_GATEWAY_PUBLIC="http://$SWE_GATEWAY_BIND"
  fi
  [[ "$SWE_GATEWAY_PUBLIC" =~ ^https?://[^[:space:]]+$ ]] \
    || fail "--swe-gateway-public 必须是 http(s) URL"
fi

[[ "$(uname -s)" == "Linux" ]] || fail "当前安装器只支持 Linux"
[[ "${EUID:-$(id -u)}" -eq 0 ]] || fail "请使用 sudo 运行安装器"
for command in tar install sed ln cp mv find sha256sum; do
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
RELEASE_STAGE=""
cleanup() {
  [[ -n "${TMP_DIR:-}" && -d "$TMP_DIR" ]] && rm -rf -- "$TMP_DIR"
  if [[ -n "${RELEASE_STAGE:-}" && -d "$RELEASE_STAGE" \
    && "$RELEASE_STAGE" == /opt/uenv/releases/.uenv-stage.* ]]; then
    rm -rf -- "$RELEASE_STAGE"
  fi
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

if [[ "$ENABLE_SWE" -eq 1 ]]; then
  command -v "$SWE_RUNTIME" >/dev/null \
    || fail "未找到 $SWE_RUNTIME；SWE 环境需要先安装可用的 Docker 或 Podman"
  [[ -f "$PAYLOAD/share/swe/verified.json" ]] \
    || fail "安装包缺少 SWE Verified catalog"
  [[ -f "$PAYLOAD/share/swe/smith-example.json" ]] \
    || fail "安装包缺少 SWE-smith 示例 catalog"
fi

RELEASE_DIR="/opt/uenv/releases/$RELEASE_VERSION"
info "安装 UEnv $RELEASE_VERSION（$PROFILE）"
install -d -m 0755 /opt/uenv/releases /var/log/uenv
BUNDLE_DIGEST="$(sha256sum "$BUNDLE" | awk '{print $1}')"
[[ "$BUNDLE_DIGEST" =~ ^[0-9a-f]{64}$ ]] || fail "无法计算安装包 SHA-256"
if [[ -e "$RELEASE_DIR" ]]; then
  [[ -d "$RELEASE_DIR" ]] || fail "$RELEASE_DIR 已存在但不是目录"
  INSTALLED_DIGEST="$(cat "$RELEASE_DIR/.bundle.sha256" 2>/dev/null || true)"
  [[ "$INSTALLED_DIGEST" == "$BUNDLE_DIGEST" ]] \
    || fail "版本 $RELEASE_VERSION 已存在但不是同一安装包；请用新的 --version 重新构建，不能覆盖现役 release"
  info "复用已安装的同一 UEnv release"
else
  RELEASE_STAGE="$(mktemp -d -p /opt/uenv/releases .uenv-stage.XXXXXXXX)"
  cp -a "$PAYLOAD/." "$RELEASE_STAGE/"
  printf '%s\n' "$BUNDLE_DIGEST" > "$RELEASE_STAGE/.bundle.sha256"
  chmod 0644 "$RELEASE_STAGE/.bundle.sha256"
  chown root:root "$RELEASE_STAGE"
  chmod 0755 "$RELEASE_STAGE"
  mv -T "$RELEASE_STAGE" "$RELEASE_DIR"
  RELEASE_STAGE=""
fi
ln -sfn "$RELEASE_DIR" /opt/uenv/current
ln -sfn /opt/uenv/current/bin/uenv /usr/local/bin/uenv
ln -sfn /opt/uenv/current/bin/uenv /usr/local/bin/uenv-ctl

if ! getent group uenv >/dev/null; then
  groupadd --system uenv
fi
if ! id uenv >/dev/null 2>&1; then
  useradd --system --gid uenv --home-dir /var/lib/uenv --shell /usr/sbin/nologin uenv
fi
install -d -o root -g uenv -m 0750 /etc/uenv/secrets
install -d -o uenv -g uenv -m 0750 \
  /var/lib/uenv/server /var/lib/uenv/worker /var/lib/uenv/hub \
  /var/lib/uenv/hub/import \
  /var/lib/uenv/plugins \
  /var/lib/uenv/server/obs /var/lib/uenv/server/trajectory \
  /var/lib/uenv/worker/wal /var/lib/uenv/hub/artifacts
install -d -o uenv -g uenv -m 0750 /var/lib/uenv/worker/swe-artifacts
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
  HUB_TOKEN_CONFIG_PATH=""
  if [[ -n "$HUB_TOKEN_FILE" ]]; then
    install -o uenv -g uenv -m 0600 "$HUB_TOKEN_FILE" /etc/uenv/secrets/hub.token
    HUB_TOKEN_CONFIG_PATH="/etc/uenv/secrets/hub.token"
  elif [[ -s /etc/uenv/secrets/hub.token ]]; then
    chown uenv:uenv /etc/uenv/secrets/hub.token
    chmod 0600 /etc/uenv/secrets/hub.token
    HUB_TOKEN_CONFIG_PATH="/etc/uenv/secrets/hub.token"
  fi
  sed \
    -e "s|@SERVER_ENDPOINT@|$SERVER_ENDPOINT|g" \
    -e "s|@ADVERTISE_ENDPOINT@|$ADVERTISE_ENDPOINT|g" \
    -e "s|@HUB_ENABLED@|$HUB_ENABLED|g" \
    -e "s|@HUB_ENDPOINT@|$HUB_ENDPOINT|g" \
    -e "s|@HUB_TOKEN_FILE@|$HUB_TOKEN_CONFIG_PATH|g" \
    "$RELEASE_DIR/config/worker.yaml" > "$TMP_DIR/worker.yaml"
  install_config "$TMP_DIR/worker.yaml" /etc/uenv/worker.yaml
  install_config "$RELEASE_DIR/config/worker.env" /etc/uenv/worker.env
  if [[ ! -e /etc/uenv/secrets/worker-llm.env ]]; then
    install -o root -g uenv -m 0640 /dev/null /etc/uenv/secrets/worker-llm.env
  fi
  # Worker only needs read access.  Re-applying install/upgrade must not give
  # the service account permission to replace its model credential file.
  chown root:uenv /etc/uenv/secrets/worker-llm.env
  chmod 0640 /etc/uenv/secrets/worker-llm.env
  install -m 0644 "$RELEASE_DIR/systemd/uenv-worker.service" /etc/systemd/system/uenv-worker.service
  UNITS+=(uenv-worker.service)
fi

if [[ "$ENABLE_SWE" -eq 1 ]]; then
  cat > "$TMP_DIR/swe.env" <<EOF
UENV_ENV_TYPES=qa,math,code,swe
UENV_RUNTIME_GATEWAY_ENABLED=true
UENV_RUNTIME_GATEWAY_LISTEN=$SWE_GATEWAY_BIND
UENV_RUNTIME_GATEWAY_CAPACITY=4
UENV_SWE_GATEWAY_PUBLIC_URL=$SWE_GATEWAY_PUBLIC
UENV_SWE_INSTANCES=/opt/uenv/current/share/swe/verified.json
UENV_SWE_EXTRA_CATALOG=/opt/uenv/current/share/swe/smith-example.json
UENV_SWE_VARIANTS=verified,smith
UENV_SWE_RUNTIME=$SWE_RUNTIME
UENV_SWE_IMAGE_PULL_POLICY=$SWE_IMAGE_PULL_POLICY
UENV_SWE_ARTIFACT_DIR=/var/lib/uenv/worker/swe-artifacts
UENV_TRAJECTORY_ENDPOINT=http://127.0.0.1:8077
EOF
  if [[ -e /etc/uenv/swe.env && "$FORCE_SWE_CONFIG" -ne 1 ]]; then
    cmp -s "$TMP_DIR/swe.env" /etc/uenv/swe.env \
      || fail "已有 /etc/uenv/swe.env 与本次参数不同；确认后使用 --force-swe-config"
    info "保留已有 SWE 配置 /etc/uenv/swe.env"
  else
    install -m 0644 "$TMP_DIR/swe.env" /etc/uenv/swe.env
  fi

  if [[ "$RESET_SWE_KEY" -eq 1 || ! -s /etc/uenv/secrets/swe.env ]]; then
    SWE_GATEWAY_KEY="$(python3 -c 'import secrets; print(secrets.token_hex(24))')"
    cat > "$TMP_DIR/swe-secret.env" <<EOF
UENV_RUNTIME_GATEWAY_API_KEY=$SWE_GATEWAY_KEY
UENV_SWE_GATEWAY_API_KEY=$SWE_GATEWAY_KEY
EOF
    install -o root -g uenv -m 0640 "$TMP_DIR/swe-secret.env" /etc/uenv/secrets/swe.env
  else
    RUNTIME_GATEWAY_KEY="$(awk -F= '$1 == "UENV_RUNTIME_GATEWAY_API_KEY" {print substr($0, length($1) + 2); exit}' /etc/uenv/secrets/swe.env)"
    SERVER_GATEWAY_KEY="$(awk -F= '$1 == "UENV_SWE_GATEWAY_API_KEY" {print substr($0, length($1) + 2); exit}' /etc/uenv/secrets/swe.env)"
    [[ -n "$RUNTIME_GATEWAY_KEY" && "$RUNTIME_GATEWAY_KEY" == "$SERVER_GATEWAY_KEY" ]] \
      || fail "/etc/uenv/secrets/swe.env 的两项 Gateway key 缺失或不一致；备份后使用 --reset-swe-key"
    chown root:uenv /etc/uenv/secrets/swe.env
    chmod 0640 /etc/uenv/secrets/swe.env
  fi

  if [[ "$SWE_RUNTIME" == "docker" ]]; then
    command -v usermod >/dev/null || fail "缺少命令：usermod"
    getent group docker >/dev/null 2>&1 \
      || fail "Docker 已安装但缺少 docker 用户组；请先完成 Docker 服务安装"
    usermod -a -G docker uenv
  fi
  command -v runuser >/dev/null || fail "缺少命令：runuser"
  runuser -u uenv -- "$SWE_RUNTIME" info >/dev/null 2>&1 \
    || fail "uenv 用户无法使用 $SWE_RUNTIME；请确认容器服务已启动并检查 socket/用户权限"
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
  for unit in "${UNITS[@]}"; do
    ready=0
    for ((_attempt = 1; _attempt <= 30; _attempt++)); do
      if systemctl is-active --quiet "$unit"; then
        ready=1
        break
      fi
      sleep 1
    done
    [[ "$ready" -eq 1 ]] \
      || fail "$unit 未能启动；请运行 journalctl -u $unit -n 100 --no-pager"
  done

  if [[ "$ENABLE_SWE" -eq 1 ]]; then
    SWE_HEALTH_KEY="$(awk -F= '$1 == "UENV_RUNTIME_GATEWAY_API_KEY" {print substr($0, length($1) + 2); exit}' /etc/uenv/secrets/swe.env)"
    python3 - "$SWE_GATEWAY_BIND" "$SWE_HEALTH_KEY" <<'PY'
import sys
import time
import urllib.request

bind, api_key = sys.argv[1:]
host, separator, port = bind.rpartition(":")
if not separator or not port.isdigit():
    raise SystemExit(f"SWE Gateway 监听地址无效：{bind}")
if host in {"", "0.0.0.0"}:
    host = "127.0.0.1"
url = f"http://{host}:{port}/runtime/v1/health"
last_error = None
for _ in range(30):
    request = urllib.request.Request(url)
    if api_key:
        request.add_header("X-API-Key", api_key)
    try:
        with urllib.request.urlopen(request, timeout=2) as response:
            if response.status // 100 == 2:
                break
    except Exception as exc:  # installation diagnostic
        last_error = exc
    time.sleep(1)
else:
    raise SystemExit(f"SWE Runtime Gateway 未就绪 ({url})：{last_error}")
PY
  fi
fi

echo
echo "UEnv $RELEASE_VERSION 已安装。"
echo "  检查：sudo -u uenv uenv doctor"
echo "  状态：uenv status"
echo "  日志：uenv logs server（或 worker / hub）"
if [[ "$ENABLE_SWE" -eq 1 ]]; then
  echo "  SWE：已启用；先运行评测脚本拉取所选实例镜像"
  echo "  网关：$SWE_GATEWAY_PUBLIC"
  echo "  指南：/opt/uenv/current/share/docs/UEnv评测指南.md"
fi
if [[ "$NO_START" -eq 1 ]]; then
  echo "服务尚未启动；检查配置后运行：systemctl enable --now ${UNITS[*]}"
fi
