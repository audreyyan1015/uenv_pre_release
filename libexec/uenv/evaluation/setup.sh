#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RELEASE_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
VENV="${UENV_EVAL_VENV:-$HOME/.local/share/uenv/evaluation-venv}"
WHEEL="${UENV_EVAL_WHEEL:-}"
WHEELHOUSE="${UENV_EVAL_WHEELHOUSE:-}"
OFFLINE="${UENV_EVAL_OFFLINE:-0}"

usage() {
  cat <<'EOF'
Prepare an isolated Python environment for UEnv evaluation.

Usage:
  setup.sh [--venv DIR] [--wheel FILE] [--wheelhouse DIR] [--offline]

Defaults:
  --venv  $HOME/.local/share/uenv/evaluation-venv
  --wheel      first uenv_bridge wheel in the current release
  --wheelhouse unset; pip may use its configured online index

When --wheelhouse or --offline is set, setup never contacts PyPI. A strictly
offline install requires a complete wheelhouse matching the target Python,
Linux, and CPU architecture; the release only guarantees its own Bridge wheel.
EOF
}

need_value() {
  [[ $# -ge 2 && -n "${2:-}" ]] || {
    echo "$1 requires a value" >&2
    exit 2
  }
}

while (($#)); do
  case "$1" in
    --venv) need_value "$@"; VENV="$2"; shift 2 ;;
    --wheel) need_value "$@"; WHEEL="$2"; shift 2 ;;
    --wheelhouse) need_value "$@"; WHEELHOUSE="$2"; OFFLINE=1; shift 2 ;;
    --offline) OFFLINE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

command -v python3 >/dev/null 2>&1 || { echo "python3 is required" >&2; exit 1; }
if [[ -z "$WHEEL" ]]; then
  if [[ -d "$RELEASE_ROOT/wheels" ]]; then
    WHEEL="$(find "$RELEASE_ROOT/wheels" -maxdepth 1 -type f \( -name 'uenv_bridge-*.whl' -o -name 'uenv-bridge-*.whl' \) -print -quit)"
  fi
fi
[[ -n "$WHEEL" && -f "$WHEEL" ]] || {
  echo "UEnv Bridge wheel not found; pass --wheel FILE" >&2
  exit 1
}

[[ -z "$WHEELHOUSE" ]] || OFFLINE=1
case "$OFFLINE" in
  1|true|TRUE|yes|YES)
    OFFLINE=1
    [[ -n "$WHEELHOUSE" ]] || {
      echo "offline setup requires --wheelhouse DIR (or UENV_EVAL_WHEELHOUSE)" >&2
      echo "prepare a complete dependency wheelhouse on a compatible online machine" >&2
      exit 1
    }
    ;;
  *) OFFLINE=0 ;;
esac
[[ -z "$WHEELHOUSE" || -d "$WHEELHOUSE" ]] || {
  echo "wheelhouse directory not found: $WHEELHOUSE" >&2
  exit 1
}

if ! python3 -m venv "$VENV"; then
  echo "failed to create Python venv: $VENV" >&2
  echo "Ubuntu/Debian: sudo apt-get install -y python3-venv" >&2
  exit 1
fi

pip_args=(--disable-pip-version-check)
if [[ "$OFFLINE" -eq 1 ]]; then
  pip_args+=(--no-index --find-links "$WHEELHOUSE")
fi
if ! "$VENV/bin/python" -m pip install "${pip_args[@]}" "$WHEEL"; then
  if [[ "$OFFLINE" -eq 1 ]]; then
    echo "offline install failed: the wheelhouse is missing a compatible dependency" >&2
    echo "download the Bridge wheel and its dependency closure on matching Linux/Python, then retry" >&2
  fi
  exit 1
fi
"$VENV/bin/uenv-evaluate" --help >/dev/null
echo "evaluation environment ready: $VENV"
echo "next: uenv evaluate run-task --endpoint HOST:PORT --env-type NAME --dataset NAME --input FILE --output FILE --max-steps N"
