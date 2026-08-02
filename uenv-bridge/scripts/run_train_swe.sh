#!/usr/bin/env bash
set -euo pipefail

REPO_DIR=${REPO_DIR:-"$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"}
PRESET="${REPO_DIR}/scripts/train/presets/swe_pro_grpo_sleep_probe.sh"

echo "Deprecated entrypoint: scripts/run_train_swe.sh"
echo "Forwarding to: scripts/train/presets/swe_pro_grpo_sleep_probe.sh"

exec "${PRESET}" "$@"
