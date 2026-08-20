#!/usr/bin/env bash

# Common helpers for uenv-bridge shell entrypoints.

normalize_device_backend() {
  local value
  value="$(printf '%s' "${1:-cuda}" | tr '[:upper:]' '[:lower:]')"
  case "${value}" in
    cuda|gpu|nvidia)
      printf '%s\n' cuda
      ;;
    ascend|npu|910c)
      printf '%s\n' ascend
      ;;
    *)
      echo "unsupported UENV_DEVICE_BACKEND=${1}; expected cuda or ascend" >&2
      return 1
      ;;
  esac
}

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

build_podman_ascend_args() {
  local value="$1"
  if [ -n "${value}" ]; then
    printf '%s\n' "${value}"
    return 0
  fi

  local output=""
  for device in /dev/davinci_manager /dev/devmm_svm /dev/hisi_hdc; do
    if [ -e "${device}" ]; then
      output="${output} --device ${device}"
    fi
  done

  local davinci
  for davinci in /dev/davinci[0-9]*; do
    if [ -e "${davinci}" ]; then
      output="${output} --device ${davinci}"
    fi
  done

  if [ -d /usr/local/Ascend/driver ]; then
    output="${output} -v /usr/local/Ascend/driver:/usr/local/Ascend/driver:ro"
  fi
  printf '%s\n' "${output# }"
}

build_podman_accelerator_args() {
  local backend="$1"
  local cuda_args="${2:-}"
  local ascend_args="${3:-}"
  backend="$(normalize_device_backend "${backend}")" || return 1
  case "${backend}" in
    cuda)
      build_podman_gpu_args "${cuda_args}"
      ;;
    ascend)
      build_podman_ascend_args "${ascend_args}"
      ;;
  esac
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

write_json_metadata() {
  local output="$1"
  shift
  python3 - "$output" "$@" <<'PY'
import json
import sys
from datetime import datetime, timezone
from pathlib import Path

output = Path(sys.argv[1])
metadata = {
    "metadata_version": 1,
    "created_at": datetime.now(timezone.utc).isoformat(),
}
for item in sys.argv[2:]:
    key, separator, value = item.partition("=")
    if separator:
        metadata[key] = value

output.parent.mkdir(parents=True, exist_ok=True)
with output.open("w", encoding="utf-8") as file:
    json.dump(metadata, file, ensure_ascii=False, indent=2, sort_keys=True)
    file.write("\n")
PY
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
