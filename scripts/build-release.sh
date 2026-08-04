#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST="$ROOT/dist"
VERSION=""
SKIP_BUILD=0

usage() {
  printf '%s\n' \
    'Build a self-contained UEnv Linux release archive.' \
    '' \
    'Usage: scripts/build-release.sh [--version VERSION] [--skip-build]' \
    '' \
    '  --version VERSION  package version (default: Cargo version + Git SHA)' \
    '  --skip-build       package existing release binaries'
}

while (($#)); do
  case "$1" in
    --version) VERSION="${2:-}"; shift 2 ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

case "$(uname -m)" in
  x86_64|amd64) ARCH="x86_64" ;;
  aarch64|arm64) ARCH="aarch64" ;;
  *) echo "unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac

if [[ -z "$VERSION" ]]; then
  CRATE_VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$ROOT/uenv-worker/Cargo.toml" | head -n1)"
  GIT_SHA="$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || printf unknown)"
  VERSION="${CRATE_VERSION}+${GIT_SHA}"
fi
[[ "$VERSION" =~ ^[0-9A-Za-z._+-]+$ ]] || { echo "invalid version: $VERSION" >&2; exit 2; }

if [[ "$SKIP_BUILD" -eq 0 ]]; then
  BUILD_TIME="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  GIT_SHA="$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || printf unknown)"
  echo "==> building control plane, Worker, and plugins"
  (cd "$ROOT" && UENV_BUILD_GIT_SHA="$GIT_SHA" UENV_BUILD_TIME="$BUILD_TIME" cargo build --release --workspace --locked)
  echo "==> building Hub server and client"
  (cd "$ROOT/uenv-hub" && cargo build --release --workspace --locked)
fi

for binary in \
  "$ROOT/target/release/uenv-adapter-core" \
  "$ROOT/target/release/uenv-worker" \
  "$ROOT/target/release/uenv-math-plugin" \
  "$ROOT/target/release/uenv-code-plugin" \
  "$ROOT/uenv-hub/target/release/uenv-hub-server" \
  "$ROOT/uenv-hub/target/release/uenv"; do
  [[ -x "$binary" ]] || { echo "missing release binary: $binary" >&2; exit 1; }
done

WORK_DIR="$(mktemp -d -t uenv-release.XXXXXXXX)"
cleanup() {
  [[ -n "${WORK_DIR:-}" && -d "$WORK_DIR" ]] && rm -rf -- "$WORK_DIR"
}
trap cleanup EXIT
PAYLOAD="$WORK_DIR/uenv-$VERSION"
install -d "$PAYLOAD/bin" "$PAYLOAD/config" "$PAYLOAD/systemd" \
  "$PAYLOAD/plugins/qa" "$PAYLOAD/plugins/math" "$PAYLOAD/plugins/code" \
  "$PAYLOAD/share/hub-config" "$PAYLOAD/wheels"

install -m 0755 "$ROOT/uenv" "$PAYLOAD/bin/uenv"
install -m 0755 "$ROOT/target/release/uenv-adapter-core" "$PAYLOAD/bin/"
install -m 0755 "$ROOT/target/release/uenv-worker" "$PAYLOAD/bin/"
install -m 0755 "$ROOT/target/release/uenv-math-plugin" "$PAYLOAD/bin/"
install -m 0755 "$ROOT/target/release/uenv-code-plugin" "$PAYLOAD/bin/"
install -m 0755 "$ROOT/uenv-hub/target/release/uenv-hub-server" "$PAYLOAD/bin/"
install -m 0755 "$ROOT/uenv-hub/target/release/uenv" "$PAYLOAD/bin/uenv-hub-cli"
install -m 0755 "$ROOT/install.sh" "$PAYLOAD/install.sh"
install -m 0644 "$ROOT/deploy/config/"* "$PAYLOAD/config/"
install -m 0644 "$ROOT/deploy/systemd/uenv-adapter-core.service" "$PAYLOAD/systemd/"
install -m 0644 "$ROOT/deploy/systemd/uenv-worker.service" "$PAYLOAD/systemd/"
install -m 0644 "$ROOT/deploy/systemd/uenv-hub.service" "$PAYLOAD/systemd/"

for plugin in qa math code; do
  install -m 0644 "$ROOT/plugins/$plugin/manifest.yaml" "$PAYLOAD/plugins/$plugin/"
  install -m 0755 "$ROOT/plugins/$plugin/run.sh" "$PAYLOAD/plugins/$plugin/"
done
install -m 0644 "$ROOT/plugins/qa/RUBRIC.md" "$PAYLOAD/plugins/qa/"
if [[ -d "$ROOT/plugins/code/scripts" ]]; then
  cp -a "$ROOT/plugins/code/scripts" "$PAYLOAD/plugins/code/"
fi
if [[ -d "$ROOT/uenv-hub/config/swe" ]]; then
  cp -a "$ROOT/uenv-hub/config/swe" "$PAYLOAD/share/hub-config/"
fi

if [[ "$SKIP_BUILD" -eq 0 ]]; then
  echo "==> building Python Bridge wheel"
  python3 -m pip wheel --no-deps --wheel-dir "$PAYLOAD/wheels" "$ROOT/uenv-bridge"
elif compgen -G "$ROOT/uenv-bridge/dist/*.whl" >/dev/null; then
  install -m 0644 "$ROOT"/uenv-bridge/dist/*.whl "$PAYLOAD/wheels/"
fi

printf '%s\n' "$VERSION" > "$PAYLOAD/VERSION"
python3 - "$PAYLOAD/manifest.json" "$VERSION" "$ARCH" <<'PY'
import json
import sys
from datetime import datetime, timezone

path, version, arch = sys.argv[1:]
manifest = {
    "schema_version": 1,
    "name": "uenv",
    "version": version,
    "os": "linux",
    "arch": arch,
    "components": ["adapter-core", "worker", "hub", "hub-cli", "bridge", "qa", "math", "code"],
    "built_at": datetime.now(timezone.utc).replace(microsecond=0).isoformat(),
}
with open(path, "w", encoding="utf-8") as handle:
    json.dump(manifest, handle, ensure_ascii=False, indent=2)
    handle.write("\n")
PY

install -d "$DIST"
ASSET="$DIST/uenv-linux-${ARCH}.tar.gz"
tar --sort=name --owner=0 --group=0 --numeric-owner -czf "$ASSET" -C "$WORK_DIR" "uenv-$VERSION"
(cd "$DIST" && sha256sum "$(basename "$ASSET")" > "$(basename "$ASSET").sha256")
install -m 0755 "$ROOT/install.sh" "$DIST/install.sh"
echo "release: $ASSET"
echo "checksum: $ASSET.sha256"
