#!/usr/bin/env bash
# 7143 上执行：把 Pro catalog seed 到 Hub 并验证 GET /api/v1/swe/pro/instances
set -euo pipefail
export SSHPASS='pku@345'
HUB=root@8.130.95.176
PRO=/root/UEnv/config/swe/pro.json
[[ -f "$PRO" ]] || { echo "missing $PRO"; exit 1; }
sshpass -e ssh -o StrictHostKeyChecking=no "$HUB" 'mkdir -p /root/uenv/uenv-hub/config/swe'
sshpass -e scp -o StrictHostKeyChecking=no "$PRO" "$HUB:/root/uenv/uenv-hub/config/swe/pro.json"
source /root/.uenv-worker.env
code=$(curl -s -o /tmp/hub_pro.json -w '%{http_code}' -H "Authorization: Bearer $UENV_HUB_TOKEN" \
  http://8.130.95.176:8088/api/v1/swe/pro/instances)
echo "hub_pro_http=$code"
head -c 400 /tmp/hub_pro.json
echo
