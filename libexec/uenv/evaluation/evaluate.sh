#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RELEASE_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
VENV="${UENV_EVAL_VENV:-$HOME/.local/share/uenv/evaluation-venv}"
BIN="$VENV/bin/uenv-evaluate"
SETUP_SCRIPT="${UENV_EVAL_SETUP_SCRIPT:-$SCRIPT_DIR/setup.sh}"
CONFIGURE_MODEL_SCRIPT="${UENV_EVAL_CONFIGURE_MODEL_SCRIPT:-$SCRIPT_DIR/configure_model.sh}"
PREPARE_SWE_SCRIPT="${UENV_EVAL_PREPARE_SWE_SCRIPT:-$SCRIPT_DIR/prepare_swe.sh}"
SWE_EVALUATE_SCRIPT="$RELEASE_ROOT/libexec/uenv/swe/evaluate.sh"
SWE_CATALOG_TOOL="$RELEASE_ROOT/tools/swe/build_catalog.py"

usage() {
  cat <<'EOF'
Run an evaluation through UEnv.

Run a process-plugin task (task identity is always explicit):
  uenv evaluate run-task \
    --endpoint HOST:PORT --env-type NAME --dataset NAME \
    --input FILE --output FILE --max-steps N [OPTIONS]

SWE workflow:
  sudo uenv evaluate prepare-swe \
    --bundle FILE --profile single-node|full|control-plane|worker [ROLE OPTIONS]
  sudo uenv evaluate run-swe \
    --provider local --model MODEL --base-url URL \
    --gateway URL --catalog FILE --benchmark-variant VARIANT \
    --input FILE --output FILE --artifacts-dir DIR \
    --max-iterations N --batch-size N
  sudo uenv evaluate run-swe \
    --provider volcengine --model ENDPOINT_ID \
    --gateway URL --catalog FILE --benchmark-variant VARIANT \
    --input FILE --output FILE --artifacts-dir DIR \
    --max-iterations N --batch-size N

Build a Worker catalog from an official JSON, JSONL, or Parquet export:
  uenv evaluate build-swe-catalog \
    --variant verified|lite|pro|smith --input FILE --output FILE

Worker model connection for QA/Code/process plugins:
  sudo uenv evaluate configure-model [OPTIONS]

Required task arguments:
  --endpoint HOST:PORT  UEnv Server gRPC address
  --env-type NAME       interaction and scoring implementation
  --dataset NAME        task/dataset route inside that environment
  --input FILE          portable Episode JSONL
  --output FILE         result JSONL
  --max-steps N         maximum environment steps per Episode

Optional execution arguments:
  --limit N             run only the first N rows

All evaluator arguments:
  uenv evaluate run-task --help

First-run behavior:
  The evaluation venv is prepared automatically when missing. This can access
  PyPI for Bridge dependencies. Pass --no-auto-setup (or set
  UENV_EVAL_AUTO_SETUP=0) to disable it. For a strictly offline first run, pass
  --offline and set UENV_EVAL_WHEELHOUSE=/path/to/complete/wheelhouse.
EOF
}

has_option() {
  local wanted="$1"
  shift
  local arg
  for arg in "$@"; do
    [[ "$arg" == "$wanted" ]] && return 0
  done
  return 1
}

require_task_arguments() {
  local -a args=("$@")
  local option
  for option in --endpoint --env-type --dataset --input --output --max-steps; do
    has_option "$option" "${args[@]}" || {
      echo "run-task requires $option; these arguments define the task" >&2
      exit 2
    }
  done
}

run_swe_usage() {
  cat <<'EOF'
Run a SWE evaluation batch through UEnv.

Usage:
  sudo uenv evaluate run-swe --provider local|volcengine [REQUIRED OPTIONS]

Required for both providers:
  --model NAME           local model name or volcengine endpoint ID
  --gateway URL          Worker Runtime Gateway URL
  --catalog FILE         SWE catalog JSON
  --benchmark-variant V  verified, lite, pro, or smith
  --input FILE           JSONL; each line selects one instance_id from the catalog
  --output FILE          per-instance result JSONL
  --artifacts-dir DIR    per-instance evaluation run files
  --max-iterations N     Agent iteration limit per instance
  --batch-size N         instances executed concurrently

Provider options:
  --base-url URL         required with --provider local; optional for volcengine
  --api-key-file FILE    single-line API key file (0600); otherwise prompted,
                         or taken from ARK_API_KEY for volcengine

Optional:
  --offline              use only locally imported instance images
EOF
}

run_swe() {
  local provider=""
  local -a forwarded=()
  while (($#)); do
    case "$1" in
      -h|--help) run_swe_usage; exit 0 ;;
      --provider)
        [[ $# -ge 2 && -n "${2:-}" ]] || {
          echo "run-swe: --provider requires local or volcengine" >&2
          exit 2
        }
        provider="$2"
        shift 2
        ;;
      *) forwarded+=("$1"); shift ;;
    esac
  done
  case "$provider" in
    local|volcengine) ;;
    "") echo "run-swe requires --provider local|volcengine" >&2; exit 2 ;;
    *) echo "run-swe: unsupported provider: $provider" >&2; exit 2 ;;
  esac
  local option
  for option in --model --gateway --catalog --benchmark-variant --input --output --artifacts-dir --max-iterations --batch-size; do
    has_option "$option" "${forwarded[@]}" || {
      echo "run-swe requires $option; these arguments define the evaluation batch" >&2
      exit 2
    }
  done
  if [[ "$provider" == "local" ]] && ! has_option --base-url "${forwarded[@]}"; then
    echo "run-swe with --provider local requires --base-url" >&2
    exit 2
  fi
  exec_script "$SWE_EVALUATE_SCRIPT" "$provider" "${forwarded[@]}"
}

exec_script() {
  local script="$1"
  shift
  [[ -f "$script" ]] || {
    echo "required script not found: $script" >&2
    exit 1
  }
  exec bash "$script" "$@"
}

# These commands manage services or run the standalone SWE Agent.  Dispatch
# them before checking the generic Python evaluator so first-time SWE users do
# not create an unrelated evaluation venv.
case "${1:-}" in
  configure-model)
    shift
    exec_script "$CONFIGURE_MODEL_SCRIPT" "$@"
    ;;
  prepare-swe)
    shift
    exec_script "$PREPARE_SWE_SCRIPT" "$@"
    ;;
  run-swe)
    shift
    run_swe "$@"
    ;;
  build-swe-catalog)
    shift
    [[ -f "$SWE_CATALOG_TOOL" ]] || {
      echo "SWE catalog tool not found: $SWE_CATALOG_TOOL" >&2
      exit 1
    }
    exec python3 "$SWE_CATALOG_TOOL" "$@"
    ;;
esac

AUTO_SETUP="${UENV_EVAL_AUTO_SETUP:-1}"
OFFLINE="${UENV_EVAL_OFFLINE:-0}"
forward_args=()
for arg in "$@"; do
  case "$arg" in
    --no-auto-setup) AUTO_SETUP=0 ;;
    --offline) OFFLINE=1 ;;
    *) forward_args+=("$arg") ;;
  esac
done
set -- "${forward_args[@]}"

case "${1:-}" in
  -h|--help|help) usage; exit 0 ;;
  "") usage >&2; exit 2 ;;
  run-task)
    shift
    if has_option -h "$@" || has_option --help "$@"; then usage; exit 0; fi
    require_task_arguments "$@"
    ;;
  *) echo "unknown evaluate command: $1" >&2; usage >&2; exit 2 ;;
esac

if [[ ! -x "$BIN" ]]; then
  case "$AUTO_SETUP" in
    0|false|FALSE|no|NO)
      echo "evaluation environment is not ready: $VENV" >&2
      echo "automatic setup is disabled (--no-auto-setup or UENV_EVAL_AUTO_SETUP=0)" >&2
      echo "run: bash $SETUP_SCRIPT --venv '$VENV'" >&2
      echo "offline: add --offline --wheelhouse DIR; the wheelhouse must contain every dependency" >&2
      exit 1
      ;;
  esac
  [[ -f "$SETUP_SCRIPT" ]] || {
    echo "evaluation setup script not found: $SETUP_SCRIPT" >&2
    exit 1
  }
  setup_args=(--venv "$VENV")
  if [[ -n "${UENV_EVAL_WHEELHOUSE:-}" ]]; then
    setup_args+=(--wheelhouse "$UENV_EVAL_WHEELHOUSE")
  fi
  case "$OFFLINE" in
    1|true|TRUE|yes|YES) setup_args+=(--offline) ;;
  esac
  echo "==> first evaluation: preparing an isolated Python environment" >&2
  if ! bash "$SETUP_SCRIPT" "${setup_args[@]}"; then
    echo "automatic evaluation setup failed" >&2
    echo "online: check python3-venv and access to the Python package index" >&2
    echo "offline: set UENV_EVAL_WHEELHOUSE to a compatible wheelhouse and retry" >&2
    exit 1
  fi
fi

[[ -x "$BIN" ]] || {
  echo "setup completed without creating evaluator: $BIN" >&2
  exit 1
}

exec "$BIN" "$@"
