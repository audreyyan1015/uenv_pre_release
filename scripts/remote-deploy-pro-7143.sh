#!/usr/bin/env bash
set -euo pipefail
cd /root/UEnv
export HF_ENDPOINT=https://hf-mirror.com
VENV=/root/UEnv/.venv-pro-export
if [[ ! -x "$VENV/bin/python" ]]; then
  python3 -m venv "$VENV"
  "$VENV/bin/pip" -q install datasets
fi
"$VENV/bin/python" scripts/export_swe_pro_instances.py \
  --ids "${PRO_INSTANCE_ID:-instance_NodeBB__NodeBB-04998908ba6721d64eba79ae3b65a351dcfbc5b5-vnan}" \
  --out config/swe/pro.json
python3 <<'PY'
import json
d = json.load(open("config/swe/pro.json"))
k = next(iter(d))
r = d[k]
print("instance", k)
print("F2P", len(r.get("FAIL_TO_PASS") or []))
print("P2P", len(r.get("PASS_TO_PASS") or []))
print("setup_cmd_len", len(r.get("setup_cmd") or ""))
print("test_cmd", r.get("test_cmd"))
print("image", r.get("image_cache_key"))
PY

echo "== seed Hub pro catalog =="
HUB=8.130.95.176
source /root/.uenv-worker.env 2>/dev/null || true
if command -v sshpass >/dev/null 2>&1; then
  sshpass -p 'pku@345' scp -o StrictHostKeyChecking=no config/swe/pro.json root@${HUB}:/root/uenv/uenv-hub/config/swe/pro.json
  if [[ -n "${UENV_HUB_TOKEN:-}" ]]; then
    code=$(curl -s -o /tmp/hub_pro.json -w '%{http_code}' \
      -H "Authorization: Bearer ${UENV_HUB_TOKEN}" \
      "http://${HUB}:8088/api/v1/swe/pro/instances")
    echo "hub_pro_http=${code}"
    head -c 200 /tmp/hub_pro.json; echo
  else
    echo "UENV_HUB_TOKEN unset; hub verify skipped"
  fi
else
  echo "sshpass missing; skip hub seed"
fi

source ~/.cargo/env
find scripts -name '*.sh' -exec sed -i 's/\r$//' {} +
bash scripts/gen-worker-proto.sh
cargo build -p uenv-worker --release 2>&1 | tail -8
PRO_ID=$(python3 -c "import json; print(next(iter(json.load(open('config/swe/pro.json')))))")
echo "PRO_ID=$PRO_ID"
IMG=$(python3 -c "import json; d=json.load(open('config/swe/pro.json')); print(d[next(iter(d))]['image_cache_key'])")
echo "pulling $IMG ..."
docker pull "$IMG" || echo "docker pull failed (may retry on session)"
pkill -f 'uenv-worker.*serve' || true
sleep 2
source /root/.uenv-worker.env 2>/dev/null || true
export UENV_WORKER_ALLOW_DEGRADED_START=1
export UENV_SWE_INSTANCES=/root/UEnv/config/swe/pro.json
export UENV_SWE_RUNTIME=docker
export UENV_SWE_IMAGE_PULL=1
nohup ./target/release/uenv-worker --config config/uenv-worker.deploy-7143-swe-pro.yaml serve >> /var/log/uenv/worker-swe-pro.log 2>&1 &
sleep 6
curl -s http://127.0.0.1:28777/health; echo
ss -tlnp | grep 28999 || true
tail -15 /var/log/uenv/worker-swe-pro.log
python3 integrations/openhands/run_swebench.py \
  --gateway 127.0.0.1:28999 --api-key swe-pro-secret \
  --instance "$PRO_ID" --instances config/swe/pro.json \
  --benchmark-variant pro --gold 2>&1
