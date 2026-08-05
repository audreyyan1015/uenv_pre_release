#!/usr/bin/env bash
# Compatibility entry. New callers should use examples/training/train_verl.sh.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec bash "$SCRIPT_DIR/../training/verl_runner.sh" "$@"
