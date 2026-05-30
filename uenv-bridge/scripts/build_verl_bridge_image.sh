#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Build a VeRL image with uenv-bridge, Rust, Cargo, and protoc installed.

Usage:
  scripts/build_verl_bridge_image.sh [--no-verify]

Environment:
  CONTAINER_TOOL  Container runtime. Default: podman
  IMAGE           Output image tag. Default: localhost/uenv-bridge-verl:latest
  BASE_IMAGE      VeRL base image. Default: docker.io/verlai/verl:vllm011.latest
  BUILD_NETWORK   Build network mode. Default: host

Examples:
  ./scripts/build_verl_bridge_image.sh
  CONTAINER_TOOL=docker IMAGE=uenv-bridge-verl:latest ./scripts/build_verl_bridge_image.sh
EOF
}

VERIFY=1
while [ "$#" -gt 0 ]; do
  case "$1" in
    --no-verify)
      VERIFY=0
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
  shift
done

REPO_DIR=${REPO_DIR:-"$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"}
CONTAINER_TOOL=${CONTAINER_TOOL:-podman}
IMAGE=${IMAGE:-localhost/uenv-bridge-verl:latest}
BASE_IMAGE=${BASE_IMAGE:-docker.io/verlai/verl:vllm011.latest}
BUILD_NETWORK=${BUILD_NETWORK:-host}

if ! command -v "${CONTAINER_TOOL}" >/dev/null 2>&1; then
  echo "${CONTAINER_TOOL} is not installed or not on PATH" >&2
  exit 127
fi

BUILD_ARGS=(build)
if [ -n "${BUILD_NETWORK}" ]; then
  BUILD_ARGS+=(--network "${BUILD_NETWORK}")
fi
BUILD_ARGS+=(
  --build-arg "BASE_IMAGE=${BASE_IMAGE}"
  -t "${IMAGE}"
  -f "${REPO_DIR}/Containerfile"
  "${REPO_DIR}"
)

echo "Building ${IMAGE} from ${BASE_IMAGE}"
"${CONTAINER_TOOL}" "${BUILD_ARGS[@]}"

if [ "${VERIFY}" = "1" ]; then
  echo "Verifying ${IMAGE}"
  "${CONTAINER_TOOL}" run --rm --entrypoint bash "${IMAGE}" -lc '
set -euo pipefail
python -c '"'"'import importlib; [importlib.import_module(m) for m in ("verl", "uenv.bridge")]; print("python imports ok: verl, uenv.bridge")'"'"'
rustc --version
cargo --version
protoc --version
'
fi

echo "Image ready: ${IMAGE}"
