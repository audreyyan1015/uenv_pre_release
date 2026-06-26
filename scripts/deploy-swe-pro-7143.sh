#!/usr/bin/env bash
# 7143 SWE-bench Pro 部署 + 联调（Hub 元数据 / Worker pull / Gateway / OpenHands demo）
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORKER_HOST="${UENV_WORKER_HOST:-219.147.100.43}"
WORKER_PORT="${UENV_WORKER_SSH_PORT:-7143}"
REMOTE_DIR="${UENV_REMOTE_DIR:-/root/UEnv}"
KEY="${UENV_SSH_KEY:-}"
if [[ -z "$KEY" ]]; then
  for k in "$ROOT/secrets/9aa460dab6678381f86a1022b8a54c9f_32e42d1c7902ce68ba6719d551645e02_8.143" \
           "$HOME/Documents/143key"; do
    [[ -f "$k" ]] && KEY="$k" && break
  done
fi
[[ -n "$KEY" ]] || { echo "set UENV_SSH_KEY"; exit 1; }
chmod 600 "$KEY" 2>/dev/null || true
SSH=(ssh -o BatchMode=yes -o ConnectTimeout=20 -i "$KEY" -p "$WORKER_PORT" root@"$WORKER_HOST")

echo "== [1/6] sync -> $WORKER_HOST:$REMOTE_DIR =="
tar -czf - \
  --exclude='target' --exclude='.git' --exclude='frontend' \
  --exclude='node_modules' --exclude='__pycache__' --exclude='*.parquet' \
  -C "$ROOT" . | "${SSH[@]}" "mkdir -p $REMOTE_DIR && tar -xzf - -C $REMOTE_DIR"

echo "== [2/6] export 1 Pro instance (7143 egress -> HF) =="
"${SSH[@]}" "cd $REMOTE_DIR && pip3 -q install datasets 2>/dev/null || true; python3 scripts/export_swe_pro_instances.py --limit 1 --repo-language Python --out config/swe/pro.json"

echo "== [3/6] build worker =="
"${SSH[@]}" "source ~/.cargo/env; cd $REMOTE_DIR && bash scripts/gen-worker-proto.sh && cargo build -p uenv-worker --release 2>&1 | tail -8"

echo "== [4/6] inject prewarm instance_id + restart worker =="
"${SSH[@]}" "cd $REMOTE_DIR && PRO_ID=\$(python3 -c \"import json; print(next(iter(json.load(open('config/swe/pro.json')))))\") && \
  echo prewarm_instance=\$PRO_ID && \
  pkill -f 'uenv-worker.*serve' || true; sleep 2; \
  source /root/.uenv-worker.env 2>/dev/null || true; \
  export UENV_WORKER_ALLOW_DEGRADED_START=1; \
  export UENV_SWE_INSTANCES=$REMOTE_DIR/config/swe/pro.json; \
  export UENV_SWE_RUNTIME=docker; \
  export UENV_SWE_IMAGE_PULL=1; \
  nohup ./target/release/uenv-worker --config config/uenv-worker.deploy-7143-swe-pro.yaml serve >> /var/log/uenv/worker-swe-pro.log 2>&1 & \
  sleep 5; curl -s http://127.0.0.1:28777/health; echo; ss -tlnp | grep 28999 || true"

echo "== [5/6] prewarm pro image =="
"${SSH[@]}" "cd $REMOTE_DIR && PRO_ID=\$(python3 -c \"import json; print(next(iter(json.load(open('config/swe/pro.json')))))\") && \
  source /root/.uenv-worker.env 2>/dev/null || true; \
  IMG=\$(python3 -c \"import json; d=json.load(open('config/swe/pro.json')); print(d['\$PRO_ID']['image_cache_key'])\") && \
  echo pulling \$IMG && docker pull \$IMG"

echo "== [6/6] OpenHands gold-path demo =="
"${SSH[@]}" "cd $REMOTE_DIR && PRO_ID=\$(python3 -c \"import json; print(next(iter(json.load(open('config/swe/pro.json')))))\") && \
  python3 integrations/openhands/run_swebench.py --gateway 127.0.0.1:28999 --api-key swe-pro-secret \
    --instance \$PRO_ID --instances config/swe/pro.json --benchmark-variant pro --gold 2>&1 | tail -20"

echo "done."
