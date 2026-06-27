#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

cargo run -p uenv-mock-scheduler --example gen_math_fixture
echo "generated fixtures under fixtures/math/"
