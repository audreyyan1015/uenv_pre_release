#!/usr/bin/env bash
# Public root wrapper for bounded SWE batch evaluation.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
fail() { echo "错误：$*" >&2; exit 1; }

[[ "$EUID" -eq 0 ]] || fail "run-swe 需要 sudo，以读取 Gateway 凭据并使用 Worker 容器运行时"
[[ -f "$SCRIPT_DIR/evaluate_batch.py" ]] || fail "缺少 SWE batch runner"
[[ -f "$SCRIPT_DIR/evaluate_one.sh" ]] || fail "缺少 SWE single-case runner"

config_value() {
  local file="$1" key="$2"
  [[ -r "$file" ]] || return 0
  awk -F= -v wanted="$key" '$1 == wanted {print substr($0, length(wanted) + 2); exit}' "$file"
}

GATEWAY_KEY="${UENV_GATEWAY_API_KEY:-${UENV_RUNTIME_GATEWAY_API_KEY:-}}"
if [[ -z "$GATEWAY_KEY" ]]; then
  GATEWAY_KEY="$(config_value /etc/uenv/secrets/swe.env UENV_SWE_GATEWAY_API_KEY)"
fi
if [[ -z "$GATEWAY_KEY" ]]; then
  GATEWAY_KEY="$(config_value /etc/uenv/secrets/swe.env UENV_RUNTIME_GATEWAY_API_KEY)"
fi
export UENV_GATEWAY_API_KEY="$GATEWAY_KEY"
exec python3 "$SCRIPT_DIR/evaluate_batch.py" "$@"
