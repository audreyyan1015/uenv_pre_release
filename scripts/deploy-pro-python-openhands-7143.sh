#!/usr/bin/env bash
# 7143：Python Pro 实例 + OpenHands gold 验收
set -euo pipefail
cd /root/UEnv
export HF_ENDPOINT=https://hf-mirror.com
VENV=/root/UEnv/.venv-pro-export
PRO_INSTANCE_ID="${PRO_INSTANCE_ID:-instance_qutebrowser__qutebrowser-f91ace96223cac8161c16dd061907e138fe85111-v059c6fdc75567943479b23ebca7c07b5e9a7f34c}"

if [[ ! -x "$VENV/bin/python" ]]; then
  python3 -m venv "$VENV"
  "$VENV/bin/pip" -q install datasets
fi

"$VENV/bin/python" scripts/export_swe_pro_instances.py \
  --ids "$PRO_INSTANCE_ID" \
  --out config/swe/pro.json

python3 <<PY
import json
d = json.load(open("config/swe/pro.json"))
k = next(iter(d))
r = d[k]
print("instance", k)
print("test_cmd", r.get("test_cmd"))
print("pre_test_cmd", r.get("pre_test_cmd"))
print("image", r.get("image_cache_key"))
print("F2P", len(r.get("FAIL_TO_PASS") or []), "P2P", len(r.get("PASS_TO_PASS") or []))
PY

echo "== seed Hub =="
source /root/.uenv-worker.env 2>/dev/null || true
if command -v sshpass >/dev/null 2>&1; then
  sshpass -p 'pku@345' scp -o StrictHostKeyChecking=no config/swe/pro.json \
    root@8.130.95.176:/root/uenv/uenv-hub/config/swe/pro.json
  if [[ -n "${UENV_HUB_TOKEN:-}" ]]; then
    curl -s -o /tmp/hub_pro.json -w 'hub_http=%{http_code}\n' \
      -H "Authorization: Bearer ${UENV_HUB_TOKEN}" \
      http://8.130.95.176:8088/api/v1/swe/pro/instances
    head -c 120 /tmp/hub_pro.json; echo
  fi
fi

echo "== docker pull (multi-mirror) =="
TAG=$(python3 -c "import json; d=json.load(open('config/swe/pro.json')); print(next(iter(d.values()))['image_cache_key'].split(':',1)[1])")
bash scripts/pull-pro-image-7143.sh "$TAG"

source ~/.cargo/env
cargo build -p uenv-worker --release 2>&1 | tail -5

pkill -f 'uenv-worker.*serve' || true
sleep 2
source /root/.uenv-worker.env 2>/dev/null || true
export UENV_WORKER_ALLOW_DEGRADED_START=1
export UENV_SWE_INSTANCES=/root/UEnv/config/swe/pro.json
export UENV_SWE_RUNTIME=docker
export UENV_SWE_IMAGE_PULL=1
nohup ./target/release/uenv-worker --config config/uenv-worker.deploy-7143-swe-pro.yaml serve \
  >> /var/log/uenv/worker-swe-pro.log 2>&1 &
sleep 6
curl -s http://127.0.0.1:28777/health; echo

echo "== OpenHands Python Pro gold =="
python3 integrations/openhands/run_swebench.py \
  --gateway 127.0.0.1:28097 --api-key swe-pro-secret \
  --instance "$PRO_INSTANCE_ID" \
  --instances config/swe/pro.json \
  --benchmark-variant pro --gold
