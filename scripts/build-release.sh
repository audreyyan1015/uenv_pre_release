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
  "$PAYLOAD/libexec/uenv/environment" "$PAYLOAD/libexec/uenv/evaluation" \
  "$PAYLOAD/libexec/uenv/swe" "$PAYLOAD/libexec/uenv/training" \
  "$PAYLOAD/share/hub-config" "$PAYLOAD/share/swe/openhands" \
  "$PAYLOAD/share/docs" \
  "$PAYLOAD/share/templates/process-plugin" \
  "$PAYLOAD/share/uenv-bridge/configs" "$PAYLOAD/share/uenv-bridge/scripts" \
  "$PAYLOAD/examples/cases/evaluation" "$PAYLOAD/examples/cases/training" \
  "$PAYLOAD/tools/hub" "$PAYLOAD/tools/swe" "$PAYLOAD/wheels"

install -m 0755 "$ROOT/uenv" "$PAYLOAD/bin/uenv"
install -m 0755 "$ROOT/scripts/uenv-train" "$PAYLOAD/bin/uenv-train"
install -m 0755 "$ROOT/target/release/uenv-adapter-core" "$PAYLOAD/bin/"
install -m 0755 "$ROOT/target/release/uenv-worker" "$PAYLOAD/bin/"
install -m 0755 "$ROOT/target/release/uenv-math-plugin" "$PAYLOAD/bin/"
install -m 0755 "$ROOT/target/release/uenv-code-plugin" "$PAYLOAD/bin/"
install -m 0755 "$ROOT/uenv-hub/target/release/uenv-hub-server" "$PAYLOAD/bin/"
install -m 0755 "$ROOT/uenv-hub/target/release/uenv" "$PAYLOAD/bin/uenv-hub-cli"
install -m 0755 "$ROOT/install.sh" "$PAYLOAD/install.sh"
install -m 0644 "$ROOT/deploy/config/server.yaml" "$PAYLOAD/config/server.yaml"
install -m 0644 "$ROOT/deploy/config/worker.yaml" "$PAYLOAD/config/worker.yaml"
install -m 0644 "$ROOT/deploy/config/hub.toml" "$PAYLOAD/config/hub.toml"
# *.env is intentionally ignored by Git.  Package the committed templates under
# the runtime names expected by install.sh so a clean checkout is releasable.
install -m 0644 "$ROOT/deploy/config/server.env.example" "$PAYLOAD/config/server.env"
install -m 0644 "$ROOT/deploy/config/worker.env.example" "$PAYLOAD/config/worker.env"
install -m 0644 "$ROOT/deploy/systemd/uenv-adapter-core.service" "$PAYLOAD/systemd/"
install -m 0644 "$ROOT/deploy/systemd/uenv-worker.service" "$PAYLOAD/systemd/"
install -m 0644 "$ROOT/deploy/systemd/uenv-hub.service" "$PAYLOAD/systemd/"
install -m 0644 "$ROOT/deploy/systemd/uenv-swe-agent.service" "$PAYLOAD/systemd/"

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

# Minimal SWE assets.  Hub is optional for the single-Worker examples: the
# Worker reads these catalogs locally and OpenHands talks to its Runtime Gateway.
install -m 0644 "$ROOT/config/swe/verified.json" "$PAYLOAD/share/swe/verified.json"
install -m 0644 "$ROOT/config/swe/smith-sample-catalog.json" \
  "$PAYLOAD/share/swe/smith-sample-catalog.json"
install -m 0644 "$ROOT/integrations/openhands/PIN.md" "$PAYLOAD/share/swe/PIN.md"
install -m 0644 "$ROOT/integrations/openhands/requirements-agent.txt" \
  "$PAYLOAD/share/swe/requirements-agent.txt"
install -m 0755 "$ROOT/integrations/openhands/run_swebenchpro_official.py" \
  "$PAYLOAD/share/swe/openhands/run_swebenchpro_official.py"
while IFS= read -r -d '' source; do
  relative="${source#"$ROOT/integrations/openhands/"}"
  install -D -m 0644 "$source" "$PAYLOAD/share/swe/openhands/$relative"
done < <(find "$ROOT/integrations/openhands/uenv_runtime" -type f -name '*.py' -print0)
install -m 0755 "$ROOT/scripts/openhands/openhands_runner.py" \
  "$PAYLOAD/share/swe/openhands-runner.py"

install -m 0644 "$ROOT/uenv-bridge/configs/uenv-agent-loop.yaml" \
  "$PAYLOAD/share/uenv-bridge/configs/uenv-agent-loop.yaml"
install -m 0755 "$ROOT/uenv-bridge/scripts/run_verl_main_ppo.py" \
  "$PAYLOAD/share/uenv-bridge/scripts/run_verl_main_ppo.py"
for area in environment evaluation swe training; do
  for source in "$ROOT/libexec/uenv/$area/"*; do
    [[ -f "$source" ]] || continue
    case "$source" in
      *.sh|*.py) mode=0755 ;;
      *) mode=0644 ;;
    esac
    install -m "$mode" "$source" "$PAYLOAD/libexec/uenv/$area/"
  done
done

install -m 0644 "$ROOT/examples/README.md" "$PAYLOAD/examples/README.md"
for kind in evaluation training; do
  for example in "$ROOT/examples/cases/$kind/"*; do
    [[ -f "$example" ]] || continue
    install -m 0644 "$example" "$PAYLOAD/examples/cases/$kind/"
  done
done

for area in hub swe; do
  for source in "$ROOT/tools/$area/"*; do
    [[ -f "$source" ]] || continue
    case "$source" in
      *.sh|*.py) mode=0755 ;;
      *) mode=0644 ;;
    esac
    install -m "$mode" "$source" "$PAYLOAD/tools/$area/"
  done
done

if [[ -d "$ROOT/templates/process-plugin" ]]; then
  # Development venvs, wheels and Python caches must never leak into a release.
  (cd "$ROOT/templates/process-plugin" && tar \
    --exclude='.venv' \
    --exclude='wheelhouse' \
    --exclude='__pycache__' \
    --exclude='.pytest_cache' \
    --exclude='.mypy_cache' \
    --exclude='*.pyc' \
    -cf - .) | (cd "$PAYLOAD/share/templates/process-plugin" && tar -xf -)
fi

for guide in \
  UEnv基础部署指南.md \
  UEnv多机部署指南.md \
  'UEnv Hub使用指南.md' \
  UEnv评测指南.md \
  UEnv训练指南.md; do
  install -m 0644 "$ROOT/Docs/deployment/$guide" "$PAYLOAD/share/docs/"
done

echo "==> building Python Bridge wheel"
python3 -m pip wheel --no-deps --wheel-dir "$PAYLOAD/wheels" "$ROOT/uenv-bridge"

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
    "components": [
        "adapter-core", "worker", "hub", "hub-cli", "bridge", "qa", "math", "code",
        "environment-evaluation", "environment-training", "process-plugin-template",
        "environment-authoring", "hub-image-transfer", "swe-catalog-tools",
        "swe-runtime", "openhands-agent", "example-cases"
    ],
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
