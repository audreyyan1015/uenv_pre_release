#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HUB_SERVER_BIN="${UENV_HUB_SERVER_BIN:-$REPO_ROOT/uenv-hub/target/debug/uenv-hub-server}"
UENV_BIN="${UENV_HUB_CLI_BIN:-$REPO_ROOT/uenv-hub/target/debug/uenv}"
PORT="${UENV_HUB_TEST_PORT:-18090}"
RUN_DIR="$(mktemp -d "${TMPDIR:-/tmp}/uenv-smith-hub-e2e.XXXXXX")"
SERVER_PID=""

cleanup() {
  if [[ -n "$SERVER_PID" ]]; then
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
  rm -rf "$RUN_DIR"
}
trap cleanup EXIT

for binary in "$HUB_SERVER_BIN" "$UENV_BIN"; do
  if [[ ! -x "$binary" ]]; then
    echo "required binary is not executable: $binary" >&2
    exit 1
  fi
done

mkdir -p "$RUN_DIR/artifacts" "$RUN_DIR/sync"
UENV_HUB_SERVER__HOST=127.0.0.1 \
UENV_HUB_SERVER__PORT="$PORT" \
UENV_HUB_DATABASE__URL="sqlite://$RUN_DIR/hub.db" \
UENV_HUB_AUTH__REQUIRE_TOKEN=false \
UENV_HUB_PACKAGES__ARTIFACT_DIR="$RUN_DIR/artifacts" \
UENV_HUB_PACKAGES__CATALOG_SEED_DIR="$REPO_ROOT/config/swe" \
UENV_HUB_PACKAGES__SEED_EXAMPLES=true \
  "$HUB_SERVER_BIN" >"$RUN_DIR/server.log" 2>&1 &
SERVER_PID=$!

ready=0
for _ in {1..30}; do
  if curl -sf "http://127.0.0.1:$PORT/healthz" >"$RUN_DIR/health.json"; then
    ready=1
    break
  fi
  sleep 1
done
if [[ "$ready" != 1 ]]; then
  echo "Hub did not become ready; log follows:" >&2
  printf '%s\n' "$( <"$RUN_DIR/server.log")" >&2
  exit 1
fi

base_url="http://127.0.0.1:$PORT"
curl -sf "$base_url/api/v1/packages/swe-bench-smith/versions/latest" \
  >"$RUN_DIR/package.json"
curl -sf "$base_url/api/v1/packages/swe-bench-smith/versions/latest/sync-plan" \
  >"$RUN_DIR/sync-plan.json"
curl -sf \
  "$base_url/api/v1/episode-stacks/swe-bench-smith-openhands/versions/latest/resolve" \
  >"$RUN_DIR/stack.json"

"$UENV_BIN" --endpoint "$base_url" env sync swe-bench-smith \
  --version latest \
  --target-dir "$RUN_DIR/sync" \
  --worker-version 0.2.0

python3 - "$RUN_DIR" <<'PY'
import json
import pathlib
import sys

run_dir = pathlib.Path(sys.argv[1])
package = json.loads((run_dir / "package.json").read_text())
assert package["package_id"] == "swe-bench-smith"
assert package["version"] == "0.1.0"
assert package["worker_overlay"]["swe"]["benchmark_variant"] == "smith"
assert package["worker_overlay"]["swe"]["grader"] == "swesmith"
assert package["agent_defaults"]["workspace_dir"] == "/testbed"
assert package["agent_defaults"]["driver_entrypoint"] == "run_swesmith_official.py"

stack = json.loads((run_dir / "stack.json").read_text())
assert stack["task_env"]["dataset"] == "swe-bench-smith"
assert stack["runtime_gateway"]["required"] is True

package_dir = run_dir / "sync/envs/swe-bench-smith/0.1.0"
expected_files = [
    "catalog.json",
    "images.manifest.json",
    "eval_spec.json",
    "worker.overlay.yaml",
    "manifest.json",
    ".synced",
]
assert all((package_dir / name).is_file() for name in expected_files)

sync_plan = json.loads((run_dir / "sync-plan.json").read_text())
synced = json.loads((package_dir / ".synced").read_text())
assert sync_plan["bundle_digest"] == synced["bundle_digest"]
catalog = json.loads((package_dir / "catalog.json").read_text())
assert len(catalog) == 5
assert all(row["benchmark_variant"] == "smith" for row in catalog.values())
assert all(
    "swesmith" in row["image_cache_key"]
    for row in catalog.values()
)
images = json.loads((package_dir / "images.manifest.json").read_text())
assert images["variant"] == "smith"
assert images["pull_policy"] == "local_only"
assert len(images["images"]) == len(catalog)
assert all("swesmith" in image["image"] for image in images["images"])
eval_spec = json.loads((package_dir / "eval_spec.json").read_text())
assert eval_spec == {
    "grader": "swesmith",
    "log_parser": "pytest",
    "variant": "smith",
}

print(
    "SMITH_HUB_E2E_OK",
    f"instances={len(catalog)}",
    f"bundle={sync_plan['bundle_digest']}",
    f"stack={stack['stack_digest']}",
)
PY
