#!/usr/bin/env bash
# 208.77 上执行 OpenHands gold + Server 轨迹验收
set -euo pipefail

cd /root/UEnv
sed -i 's/\r$//' /root/.uenv-trajectory.env /root/.openhands-20877.env \
  scripts/run-openhands-pro-20877.sh integrations/openhands/*.py \
  integrations/openhands/uenv_runtime/*.py 2>/dev/null || true

[[ -f /root/.uenv-trajectory.env ]] || cp config/uenv-trajectory.env.example /root/.uenv-trajectory.env
[[ -f /root/.openhands-20877.env ]] || cp config/openhands-20877.env.example /root/.openhands-20877.env
chmod 600 /root/.uenv-trajectory.env /root/.openhands-20877.env 2>/dev/null || true

source /root/.openhands-20877.env
source /root/.uenv-trajectory.env
export UENV_GATEWAY=http://127.0.0.1:28097

echo "== preflight =="
curl -sf http://127.0.0.1:8777/health && echo " runner_ok" || echo " runner_skip"
curl -sf -H 'X-API-Key: swe-pro-secret' http://127.0.0.1:28097/runtime/v1/health && echo " gateway_ok"
curl -sf "${UENV_TRAJECTORY_ENDPOINT%/}/control/v1/trajectories/health" && echo " server_trj_ok"

echo "== gold run =="
OUT=$(bash scripts/run-openhands-pro-20877.sh gold 2>&1 | tee /tmp/oh-gold-full.log | tail -1)
echo "RESULT=$OUT"
python3 - <<'PY'
import json, sys
line = open("/tmp/oh-gold-full.log").read().strip().splitlines()[-1]
d = json.loads(line)
assert d.get("reward") == 1.0, d
assert d.get("server_verified") is True, d
print("ACCEPTANCE_OK", d.get("trajectory_id"), d.get("run_id"))
PY
