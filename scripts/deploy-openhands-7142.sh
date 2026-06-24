#!/usr/bin/env bash
# Deploy OpenHands/benchmarks + SDK on A100 7142 and wire UEnv driver.
# Worker 7143 must already expose runtime_gateway :28999 (Pro deploy yaml).
#
# Usage (from dev machine with SSH key):
#   UENV_SSH_KEY=secrets/... bash scripts/deploy-openhands-7142.sh
#   UENV_SSH_KEY=secrets/... bash scripts/deploy-openhands-7142.sh run-smoke
#
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ADAPTER_HOST="${UENV_ADAPTER_HOST:-219.147.100.43}"
ADAPTER_PORT="${UENV_ADAPTER_SSH_PORT:-7142}"
WORKER_HOST="${UENV_WORKER_HOST:-10.10.20.143}"
REMOTE_UENV="${UENV_REMOTE_DIR:-/root/UEnv}"
OPENHANDS_DIR="${OPENHANDS_BENCHMARKS_DIR:-/opt/openhands/benchmarks}"
BENCHMARKS_SHA="${OPENHANDS_BENCHMARKS_SHA:-82687c83dfcc193989336f41d235612c02f2c044}"
RUNS_DIR="${OPENHANDS_RUNS_DIR:-/var/log/uenv/openhands-runs}"

# 7142 无法直连 GitHub 时：在本机打包 benchmarks 上传
#   tar -czf /tmp/openhands-benchmarks.tgz -C /path/to/openhands-benchmarks .
#   scp /tmp/openhands-benchmarks.tgz root@7142:/tmp/
#   ssh 7142 'mkdir -p /opt/openhands/benchmarks && tar xzf /tmp/openhands-benchmarks.tgz -C /opt/openhands/benchmarks'
#   ssh 7142 'cd /opt/openhands/benchmarks/vendor/software-agent-sdk && uv sync'

resolve_key() {
  if [[ -n "${UENV_SSH_KEY:-}" && -f "${UENV_SSH_KEY}" ]]; then echo "${UENV_SSH_KEY}"; return; fi
  for k in "$HOME/Documents/143key" \
           "$REPO_ROOT/secrets/9aa460dab6678381f86a1022b8a54c9f_32e42d1c7902ce68ba6719d551645e02_8.143" \
           "$REPO_ROOT/secrets/2a9f778a35e7d08c738c79493ba643ef_65c3b455afbe3c81a8a757c01b0faae8_8.142"; do
    [[ -f "$k" ]] && { echo "$k"; return; }
  done
  echo "ERROR: set UENV_SSH_KEY to A100 private key" >&2
  exit 1
}

KEY="$(resolve_key)"
chmod 600 "$KEY" 2>/dev/null || true
SSH7142=(ssh -o BatchMode=yes -o ConnectTimeout=15 -o StrictHostKeyChecking=accept-new -i "$KEY" -p "$ADAPTER_PORT" root@"$ADAPTER_HOST")

cmd="${1:-deploy}"

echo "== rsync UEnv -> 7142:$REMOTE_UENV =="
rsync -az \
  --exclude 'target/' --exclude '.git/' --exclude 'frontend/' --exclude 'node_modules/' \
  -e "ssh -i $KEY -p $ADAPTER_PORT -o BatchMode=yes -o StrictHostKeyChecking=accept-new" \
  "$REPO_ROOT/" root@"$ADAPTER_HOST":"$REMOTE_UENV/"

"${SSH7142[@]}" bash -s <<REMOTE
set -euo pipefail
OPENHANDS_DIR="$OPENHANDS_DIR"
BENCHMARKS_SHA="$BENCHMARKS_SHA"
REMOTE_UENV="$REMOTE_UENV"
WORKER_HOST="$WORKER_HOST"
RUNS_DIR="$RUNS_DIR"

export DEBIAN_FRONTEND=noninteractive
command -v git >/dev/null || (apt-get update -qq && apt-get install -y -qq git curl)
command -v uv >/dev/null || curl -LsSf https://astral.sh/uv/install.sh | sh
export PATH="\$HOME/.local/bin:\$PATH"

mkdir -p "\$(dirname "\$OPENHANDS_DIR")"
if [[ ! -d "\$OPENHANDS_DIR/.git" ]]; then
  git clone --recurse-submodules https://github.com/OpenHands/benchmarks.git "\$OPENHANDS_DIR"
fi
cd "\$OPENHANDS_DIR"
git fetch origin
git checkout "\$BENCHMARKS_SHA"
git submodule update --init --recursive
make build

mkdir -p "\$RUNS_DIR"

# LLM config from Worker template if present
LLM_JSON="\$REMOTE_UENV/config/openhands-llm-7142.json"
if [[ ! -f "\$LLM_JSON" && -f "\$REMOTE_UENV/config/uenv-worker-llm.env" ]]; then
  python3 <<'PY'
import json, re
from pathlib import Path
env = {}
for line in Path("$REMOTE_UENV/config/uenv-worker-llm.env").read_text().splitlines():
    line=line.strip()
    if not line or line.startswith("#") or "=" not in line: continue
    k,v=line.split("=",1)
    env[k.strip()]=v.strip().strip('"').strip("'")
out = {
    "model": "openai/" + env.get("UENV_LLM_MODEL_NAME","deepseek-v4-flash"),
    "base_url": env.get("UENV_LLM_ENDPOINT","https://dashscope.aliyuncs.com/compatible-mode/v1"),
    "api_key": env.get("UENV_LLM_API_KEY",""),
    "temperature": float(env.get("UENV_LLM_TEMPERATURE","0.2")),
    "max_output_tokens": int(env.get("UENV_LLM_MAX_TOKENS","4096")),
}
Path("$REMOTE_UENV/config/openhands-llm-7142.json").write_text(json.dumps(out, indent=2)+"\n")
print("wrote openhands-llm-7142.json from uenv-worker-llm.env")
PY
fi

echo "OpenHands benchmarks at \$OPENHANDS_DIR @ \$BENCHMARKS_SHA"
uv run python -c "import openhands.sdk; print('openhands.sdk OK')"
REMOTE

if [[ "$cmd" == "deploy" ]]; then
  echo "deploy complete."
  exit 0
fi

if [[ "$cmd" == "run-smoke" || "$cmd" == "run-llm" ]]; then
  MODE=llm
  [[ "$cmd" == "run-smoke" ]] && MODE=gold
  STAMP="$(date +%Y%m%d-%H%M%S)"
  OUT_DIR="${RUNS_DIR}/pro-official-${MODE}-${STAMP}"
  INSTANCE="${UENV_PRO_INSTANCE:-instance_qutebrowser__qutebrowser-f91ace96223cac8161c16dd061907e138fe85111-v059c6fdc75567943479b23ebca7c07b5e9a7f34c}"

  "${SSH7142[@]}" "OPENHANDS_DIR='$OPENHANDS_DIR' REMOTE_UENV='$REMOTE_UENV' WORKER_HOST='$WORKER_HOST' OUT_DIR='$OUT_DIR' INSTANCE='$INSTANCE' MODE='$MODE' bash -s" <<'REMOTE'
set -euo pipefail
export PATH="$HOME/.local/bin:$PATH"
cd "$OPENHANDS_DIR"
export OPENHANDS_BENCHMARKS_DIR="$OPENHANDS_DIR"
export UENV_REPO="$REMOTE_UENV"
uv run python "$REMOTE_UENV/integrations/openhands/run_swebenchpro_official.py" \
  --llm-config "$REMOTE_UENV/config/openhands-llm-7142.json" \
  --gateway "http://${WORKER_HOST}:28999" \
  --api-key "swe-pro-secret" \
  --instance "$INSTANCE" \
  --instances "$REMOTE_UENV/config/swe/pro-python-smoke.json" \
  --benchmark-variant pro \
  --mode "$MODE" \
  --max-iterations 30 \
  --output-dir "$OUT_DIR"
echo "artifacts: $OUT_DIR"
REMOTE
  exit 0
fi

echo "usage: $0 [deploy|run-smoke|run-llm]" >&2
exit 1
