#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

cargo run -p uenv-mock-scheduler --example gen_gsm8k_fixture
echo "generated fixtures under fixtures/gsm8k/"
