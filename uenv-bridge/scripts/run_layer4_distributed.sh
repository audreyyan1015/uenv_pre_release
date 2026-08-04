#!/usr/bin/env bash
set -euo pipefail

REPO_DIR=${REPO_DIR:-"$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"}
NEW_ENTRYPOINT="${REPO_DIR}/scripts/train/run_verl_uenv_grpo.sh"

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
  cat <<'EOF'
Compatibility entrypoint for the old Layer 4 training script.

Use the clearer generic entrypoint for new runs:
  ./scripts/train/run_verl_uenv_grpo.sh

This wrapper preserves existing commands and forwards all environment overrides.
EOF
fi

echo "Deprecated entrypoint: scripts/run_layer4_distributed.sh"
echo "Forwarding to: scripts/train/run_verl_uenv_grpo.sh"

exec "${NEW_ENTRYPOINT}" "$@"
