#!/usr/bin/env bash

# Common helpers for uenv-bridge shell entrypoints.

build_podman_gpu_args() {
  local value="$1"
  if [ -z "${value}" ]; then
    printf '%s\n' "--device nvidia.com/gpu=all"
    return 0
  fi

  case "${value}" in
    --device*|--gpus*)
      printf '%s\n' "${value}"
      return 0
      ;;
    all|nvidia.com/gpu=all)
      printf '%s\n' "--device nvidia.com/gpu=all"
      return 0
      ;;
    nvidia.com/gpu=*)
      value="${value#nvidia.com/gpu=}"
      ;;
  esac

  local output=""
  local old_ifs="${IFS}"
  IFS=','
  for gpu_id in ${value}; do
    gpu_id="$(printf '%s' "${gpu_id}" | tr -d '[:space:]')"
    if [ -n "${gpu_id}" ]; then
      output="${output} --device nvidia.com/gpu=${gpu_id}"
    fi
  done
  IFS="${old_ifs}"
  printf '%s\n' "${output# }"
}

ensure_file_exists() {
  local path="$1"
  local message="$2"
  if [ ! -f "${path}" ]; then
    echo "${message}: ${path}" >&2
    exit 1
  fi
}

ensure_path() {
  local path="$1"
  local message="$2"
  if [ ! -e "${path}" ]; then
    echo "${message}: ${path}" >&2
    exit 1
  fi
}

ensure_policy_model_exists() {
  local model_path="${1:-${MODEL_PATH:-}}"
  if [ -n "${model_path}" ] && [ -f "${model_path}/config.json" ] && compgen -G "${model_path}/*.safetensors" >/dev/null; then
    return 0
  fi

  echo "Policy model not found at ${model_path:-<empty>}." >&2
  echo "Prepare the policy model there, or override MODEL_PATH/CONTAINER_MODEL_PATH." >&2
  exit 1
}

ensure_positive_int() {
  local name="$1"
  local value="$2"
  if ! printf '%s' "${value}" | grep -Eq '^[1-9][0-9]*$'; then
    echo "${name} must be a positive integer, got: ${value}" >&2
    exit 1
  fi
}

split_host() {
  local addr="$1"
  printf '%s\n' "${addr%:*}"
}

split_port() {
  local addr="$1"
  printf '%s\n' "${addr##*:}"
}

port_open() {
  local host="$1"
  local port="$2"
  python3 - "$host" "$port" >/dev/null 2>&1 <<'PYNET'
import socket
import sys

host = sys.argv[1]
port = int(sys.argv[2])
sock = socket.socket()
sock.settimeout(0.5)
try:
    sock.connect((host, port))
except OSError:
    sys.exit(1)
else:
    sys.exit(0)
finally:
    sock.close()
PYNET
}

wait_for_addr() {
  local name="$1"
  local addr="$2"
  local timeout_seconds="$3"
  local host
  local port
  host="$(split_host "$addr")"
  port="$(split_port "$addr")"
  for _ in $(seq 1 "$timeout_seconds"); do
    if port_open "$host" "$port"; then
      echo "${name} is listening on ${addr}"
      return 0
    fi
    sleep 1
  done
  echo "Timed out waiting for ${name} on ${addr}" >&2
  return 1
}

require_free_addr() {
  local name="$1"
  local addr="$2"
  local host
  local port
  host="$(split_host "$addr")"
  port="$(split_port "$addr")"
  if port_open "$host" "$port"; then
    echo "${name} address ${addr} is already in use" >&2
    echo "Stop the process on ${addr}, or override the address before running this script." >&2
    exit 1
  fi
}
