#!/usr/bin/env bash
set -euo pipefail

TARGET="${UENV_WORKER_LLM_ENV_FILE:-/etc/uenv/secrets/worker-llm.env}"
WORKER_USER="${UENV_WORKER_USER:-uenv}"
WORKER_SERVICE="${UENV_WORKER_SERVICE:-uenv-worker.service}"
SYSTEMCTL="${UENV_SYSTEMCTL:-systemctl}"
ENDPOINT=""
MODEL_NAME=""
API_KEY_FILE=""
NO_API_KEY=0

usage() {
  cat <<'EOF'
Safely configure the model used by a UEnv Worker.

Usage:
  sudo configure_model.sh [--endpoint URL] [--model NAME]
                          [--api-key-file FILE | --no-api-key]

Endpoint and model are prompted when omitted. By default the API key is read
interactively without echo. Use --no-api-key only for an unauthenticated HTTP
model service. Secrets are never accepted as command-line values.
EOF
}

fail() {
  echo "configure_model.sh: $*" >&2
  exit 1
}

while (($#)); do
  case "$1" in
    --endpoint) ENDPOINT="${2:-}"; shift 2 ;;
    --model) MODEL_NAME="${2:-}"; shift 2 ;;
    --api-key-file) API_KEY_FILE="${2:-}"; shift 2 ;;
    --no-api-key) NO_API_KEY=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) fail "unknown argument: $1" ;;
  esac
done

[[ "${EUID:-$(id -u)}" -eq 0 ]] || fail "run with sudo on each Worker node"
[[ "$NO_API_KEY" -eq 0 || -z "$API_KEY_FILE" ]] \
  || fail "--no-api-key conflicts with --api-key-file"

if [[ -z "$ENDPOINT" ]]; then
  read -rp 'OpenAI-compatible API base URL: ' ENDPOINT
fi
if [[ -z "$MODEL_NAME" ]]; then
  read -rp 'Model name or endpoint ID: ' MODEL_NAME
fi

case "$ENDPOINT" in
  http://*|https://*) ;;
  *) fail "endpoint must start with http:// or https://" ;;
esac
[[ "$ENDPOINT" != *[$'\r\n\t ']* ]] || fail "endpoint must not contain whitespace"
[[ -n "$MODEL_NAME" ]] || fail "model name is required"
[[ "$MODEL_NAME" != *[$'\r\n']* ]] || fail "model name must be one line"

API_KEY=""
if [[ -n "$API_KEY_FILE" ]]; then
  [[ -f "$API_KEY_FILE" && -r "$API_KEY_FILE" ]] \
    || fail "cannot read API key file: $API_KEY_FILE"
  API_KEY="$(<"$API_KEY_FILE")"
elif [[ "$NO_API_KEY" -eq 0 ]]; then
  read -rsp 'API key (input hidden): ' API_KEY
  echo
fi
[[ "$API_KEY" != *[$'\r\n']* ]] || fail "API key file must contain exactly one line"
if [[ "$NO_API_KEY" -eq 0 && -z "$API_KEY" ]]; then
  fail "API key is empty; use --no-api-key only when the service is intentionally unauthenticated"
fi

command -v "$SYSTEMCTL" >/dev/null 2>&1 || fail "systemctl command not found: $SYSTEMCTL"
id "$WORKER_USER" >/dev/null 2>&1 || fail "Worker user does not exist: $WORKER_USER"

escape_env_value() {
  local value="$1"
  value="${value//\\/\\\\}"
  value="${value//\"/\\\"}"
  printf '"%s"' "$value"
}

TARGET_DIR="$(dirname "$TARGET")"
# The Worker only needs to traverse this directory and read its own file.  Keep
# the directory owned by root so the service account cannot replace sibling
# secrets such as the SWE Gateway key.
install -d -o root -g "$WORKER_USER" -m 0750 "$TARGET_DIR"
TMP="$(mktemp "$TARGET_DIR/.worker-llm.env.XXXXXXXX")"
cleanup() {
  if [[ -n "${TMP:-}" && -e "$TMP" ]]; then
    rm -f -- "$TMP"
  fi
}
trap cleanup EXIT HUP INT TERM

if [[ -f "$TARGET" ]]; then
  awk '
    !/^UENV_LLM_ENDPOINT=/ &&
    !/^UENV_LLM_MODEL_NAME=/ &&
    !/^UENV_LLM_API_KEY=/
  ' "$TARGET" > "$TMP"
fi
{
  printf 'UENV_LLM_ENDPOINT=%s\n' "$(escape_env_value "$ENDPOINT")"
  printf 'UENV_LLM_MODEL_NAME=%s\n' "$(escape_env_value "$MODEL_NAME")"
  printf 'UENV_LLM_API_KEY=%s\n' "$(escape_env_value "$API_KEY")"
} >> "$TMP"

chown "root:$WORKER_USER" "$TMP"
chmod 0640 "$TMP"
if [[ -f "$TARGET" ]] && cmp -s "$TMP" "$TARGET"; then
  # An earlier release may have left the same bytes with weaker ownership or
  # mode.  Treat content as unchanged, but still restore the security contract.
  chown "root:$WORKER_USER" "$TARGET"
  chmod 0640 "$TARGET"
  echo "model configuration is unchanged; Worker was not restarted"
  exit 0
fi

mv -f -- "$TMP" "$TARGET"
TMP=""
echo "model configuration updated: $TARGET"
echo "restarting $WORKER_SERVICE because systemd reads EnvironmentFile only when the process starts"
if ! "$SYSTEMCTL" restart "$WORKER_SERVICE"; then
  fail "configuration was written, but Worker restart failed; inspect systemctl status $WORKER_SERVICE"
fi
echo "Worker restarted; future Episodes use the new model configuration"
