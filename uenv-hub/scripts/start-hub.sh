#!/usr/bin/env bash
# 启动 uenv-hub-server（单一共享 Token 鉴权方案）。
#
# - require_token=true 在 config/hub.prod.toml 中开启。
# - 首次启动时从 data/.admin_token（权限 600）读取共享 token，经环境变量
#   UENV_HUB_AUTH__BOOTSTRAP_ADMIN_TOKEN 注入；服务在 token 表为空时据此创建。
# - token 持久化进 SQLite 后，后续重启即使不带该变量也仍生效（本脚本仍会带上，
#   bootstrap 为幂等：表非空时不重复创建）。
set -euo pipefail

cd "$(dirname "$0")/.."

TOKEN_FILE="data/.admin_token"
if [[ -f "$TOKEN_FILE" ]]; then
  export UENV_HUB_AUTH__BOOTSTRAP_ADMIN_TOKEN="$(cat "$TOKEN_FILE")"
else
  echo "warn: $TOKEN_FILE 不存在；若 DB 中已有 token 可忽略，否则将无法鉴权。" >&2
fi

exec ./target/release/uenv-hub-server --config config/hub.prod.toml
