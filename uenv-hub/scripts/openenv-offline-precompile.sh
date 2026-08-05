#!/usr/bin/env bash
# openenv-offline-precompile.sh — prepare an OpenEnv environment for an
# air-gapped (内网零外拉) deployment.
#
# Run this ONCE on a machine that still has internet access. It produces, inside
# the environment project, everything the air-gapped side needs:
#
#   <env>/offline/wheels/*.whl     dependency wheels (no PyPI at install time)
#   <env>/offline/images/*.tar     `docker save` archive of the runtime image
#   <env>/**/__pycache__/*.pyc     pre-compiled bytecode (no compile on first import)
#   <env>/offline/precompile.json  machine-readable evidence of this run
#
# `uenv env test --project <env>` reads that evidence (check C11) and refuses to
# let an environment be packaged when its dependencies are declared but the
# wheelhouse is empty.
#
# Usage:
#   uenv-hub/scripts/openenv-offline-precompile.sh <env-dir> [options]
#
#   --platform TAG     target platform tag for the wheels, e.g.
#                      manylinux2014_x86_64. REQUIRED when the preparation host
#                      and the air-gapped target differ (a macOS-vendored
#                      pydantic_core will NOT install on a Linux worker).
#   --python-version V target CPython version for the wheels, e.g. 3.12
#   --image REF        `docker save` this runtime image into offline/images/
#   --python BIN       interpreter used for pip/compileall (default python3)
#   --engine ENGINE    docker|podman (default docker)
#   --skip-image       do not touch the container engine
#
# Companion scripts: airgap-offline-bundle.sh / airgap-offline-build.sh (Rust
# side, `cargo vendor`), airgap-image-bundle.sh (bulk image staging).

set -euo pipefail

usage() {
    sed -n '2,38p' "$0" | sed 's/^# \{0,1\}//'
    exit "${1:-0}"
}

[[ $# -ge 1 ]] || usage 1
case "$1" in
    -h|--help) usage 0 ;;
esac
ENV_DIR="$1"; shift

IMAGE=""
PYTHON_BIN="python3"
ENGINE="docker"
SKIP_IMAGE=0
TARGET_PLATFORM=""
TARGET_PYVER=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --image)          IMAGE="$2"; shift 2 ;;
        --python)         PYTHON_BIN="$2"; shift 2 ;;
        --engine)         ENGINE="$2"; shift 2 ;;
        --platform)       TARGET_PLATFORM="$2"; shift 2 ;;
        --python-version) TARGET_PYVER="$2"; shift 2 ;;
        --skip-image)     SKIP_IMAGE=1; shift ;;
        -h|--help)        usage 0 ;;
        *) echo "unknown argument: $1" >&2; usage 1 ;;
    esac
done

[[ -d "$ENV_DIR" ]] || { echo "not a directory: $ENV_DIR" >&2; exit 1; }
ENV_DIR="$(cd "$ENV_DIR" && pwd)"
OFFLINE_DIR="$ENV_DIR/offline"
WHEEL_DIR="$OFFLINE_DIR/wheels"
IMAGE_DIR="$OFFLINE_DIR/images"
mkdir -p "$WHEEL_DIR" "$IMAGE_DIR"

echo "== OpenEnv offline precompile =="
echo "   project : $ENV_DIR"
echo "   python  : $PYTHON_BIN ($("$PYTHON_BIN" --version 2>&1))"

# ---------------------------------------------------------------------------
# 1. Dependency wheels — so the air-gapped install never reaches PyPI.
# ---------------------------------------------------------------------------
REQ_FILE=""
for candidate in "$ENV_DIR/requirements.txt" "$ENV_DIR/server/requirements.txt"; do
    [[ -f "$candidate" ]] && { REQ_FILE="$candidate"; break; }
done

# Cross-platform vendoring: pip can only resolve wheels for a foreign platform
# when it is forbidden from falling back to source distributions.
PIP_PLATFORM_ARGS=()
if [[ -n "$TARGET_PLATFORM" ]]; then
    PIP_PLATFORM_ARGS+=(--platform "$TARGET_PLATFORM" --only-binary=:all:)
    [[ -n "$TARGET_PYVER" ]] && PIP_PLATFORM_ARGS+=(--python-version "$TARGET_PYVER")
    echo "   target  : platform=$TARGET_PLATFORM python=${TARGET_PYVER:-<host>}"
else
    echo "   target  : <preparation host> — pass --platform/--python-version when the"
    echo "             air-gapped worker differs (platform-specific wheels will not install)"
fi

if [[ -n "$REQ_FILE" ]]; then
    echo "-- downloading wheels from $REQ_FILE"
    "$PYTHON_BIN" -m pip download \
        --dest "$WHEEL_DIR" \
        --requirement "$REQ_FILE" \
        --prefer-binary \
        "${PIP_PLATFORM_ARGS[@]}"
elif [[ -f "$ENV_DIR/pyproject.toml" ]]; then
    echo "-- no requirements.txt; downloading wheels for the project itself (pyproject.toml)"
    # `pip download <dir>` resolves the project's declared dependencies.
    "$PYTHON_BIN" -m pip download --dest "$WHEEL_DIR" --prefer-binary "$ENV_DIR" || {
        echo "!! pip download failed for the project; falling back to no wheels." >&2
        echo "!! Declare dependencies in requirements.txt for a reproducible wheelhouse." >&2
    }
else
    echo "-- no requirements.txt / pyproject.toml found; nothing to vendor"
fi

WHEEL_COUNT=$(find "$WHEEL_DIR" -name '*.whl' -type f | wc -l | tr -d ' ')
SDIST_COUNT=$(find "$WHEEL_DIR" \( -name '*.tar.gz' -o -name '*.zip' \) -type f | wc -l | tr -d ' ')
echo "   wheels  : $WHEEL_COUNT (+$SDIST_COUNT source archive(s))"

# Platform self-check. A wheel whose tag is not `any` only installs on a matching
# platform; shipping the wrong one to an air-gapped worker fails at install time,
# when there is no PyPI left to fall back to.
PLATFORM_MISMATCH=0
NATIVE_TAGS="$(find "$WHEEL_DIR" -name '*.whl' -type f -exec basename {} \; \
    | awk -F- '{print $NF}' | sed 's/\.whl$//' | grep -v '^any$' | sort -u || true)"
if [[ -n "$NATIVE_TAGS" ]]; then
    echo "   native  : platform-specific wheel tag(s): $(echo "$NATIVE_TAGS" | tr '\n' ' ')"
    if [[ -n "$TARGET_PLATFORM" ]]; then
        while read -r tag; do
            [[ -z "$tag" ]] && continue
            if [[ "$TARGET_PLATFORM" != *"${tag%%.*}"* && "$tag" != *"$TARGET_PLATFORM"* ]]; then
                echo "!! wheel tag '$tag' does not match --platform '$TARGET_PLATFORM'" >&2
                PLATFORM_MISMATCH=1
            fi
        done <<< "$NATIVE_TAGS"
    else
        echo "!! no --platform given; these wheels only install on this preparation host." >&2
        echo "!! Re-run with --platform <target> (e.g. manylinux2014_x86_64) for a Linux worker." >&2
        PLATFORM_MISMATCH=1
    fi
fi

# ---------------------------------------------------------------------------
# 2. Bytecode precompilation — no first-import compile inside the container,
#    and it fails loudly on a syntax error, which is itself a useful test.
# ---------------------------------------------------------------------------
echo "-- precompiling bytecode (compileall)"
COMPILE_STATUS=0
"$PYTHON_BIN" -m compileall -q "$ENV_DIR" || COMPILE_STATUS=$?
PYC_COUNT=$(find "$ENV_DIR" -name '*.pyc' -type f | wc -l | tr -d ' ')
PY_COUNT=$(find "$ENV_DIR" -name '*.py' -type f | wc -l | tr -d ' ')
echo "   bytecode: $PYC_COUNT .pyc / $PY_COUNT .py (compileall exit=$COMPILE_STATUS)"
if [[ "$COMPILE_STATUS" -ne 0 ]]; then
    echo "!! compileall reported errors — fix the sources before packaging." >&2
fi

# ---------------------------------------------------------------------------
# 3. Runtime image → tar, so the Worker `docker load`s from the Hub instead of
#    pulling a third-party registry.
# ---------------------------------------------------------------------------
IMAGE_TAR=""
if [[ "$SKIP_IMAGE" -eq 0 && -n "$IMAGE" ]]; then
    if command -v "$ENGINE" >/dev/null 2>&1; then
        SAFE_NAME="$(echo "$IMAGE" | tr '/:' '__')"
        IMAGE_TAR="$IMAGE_DIR/${SAFE_NAME}.tar"
        echo "-- saving image $IMAGE -> $IMAGE_TAR"
        "$ENGINE" save -o "$IMAGE_TAR" "$IMAGE"
    else
        echo "!! $ENGINE not found; skipping image save (run this step on a host with $ENGINE)" >&2
    fi
elif [[ "$SKIP_IMAGE" -eq 0 ]]; then
    echo "-- no --image given; skipping image save"
fi
IMAGE_TAR_COUNT=$(find "$IMAGE_DIR" -name '*.tar' -type f | wc -l | tr -d ' ')

# ---------------------------------------------------------------------------
# 4. Evidence for the conformance gate.
# ---------------------------------------------------------------------------
cat > "$OFFLINE_DIR/precompile.json" <<JSON
{
  "generated_at": $(date +%s),
  "python": "$("$PYTHON_BIN" --version 2>&1)",
  "requirements_file": "${REQ_FILE#"$ENV_DIR"/}",
  "target_platform": "$TARGET_PLATFORM",
  "target_python_version": "$TARGET_PYVER",
  "platform_mismatch": $PLATFORM_MISMATCH,
  "wheel_count": $WHEEL_COUNT,
  "sdist_count": $SDIST_COUNT,
  "pyc_count": $PYC_COUNT,
  "py_source_count": $PY_COUNT,
  "compileall_exit": $COMPILE_STATUS,
  "image": "$IMAGE",
  "image_tar_count": $IMAGE_TAR_COUNT
}
JSON

echo
echo "== done =="
echo "   evidence: $OFFLINE_DIR/precompile.json"
if [[ "$PLATFORM_MISMATCH" -ne 0 ]]; then
    echo "!! WHEELHOUSE IS NOT PORTABLE to the declared target — fix before packaging." >&2
fi
echo "   next    : uenv env test --manifest $ENV_DIR/manifest.toml --project $ENV_DIR --strict"
if [[ -n "$IMAGE_TAR" ]]; then
    echo "   then    : uenv env publish-image <pkg> --tar <hub-host-path>/$(basename "$IMAGE_TAR")"
fi

# Air-gapped side, for reference:
#   pip install --no-index --find-links offline/wheels -r requirements.txt
#   (no network access required)
