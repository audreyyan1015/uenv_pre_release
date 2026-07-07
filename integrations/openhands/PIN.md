# OpenHands pin record (208.77 deployment — migrated from 7142)

OPENHANDS_BENCHMARKS_REPO=https://github.com/OpenHands/benchmarks
OPENHANDS_BENCHMARKS_SHA=82687c83dfcc193989336f41d235612c02f2c044

OPENHANDS_SDK_REPO=https://github.com/OpenHands/software-agent-sdk
OPENHANDS_SDK_SHA=43376f1868ffd702746080714a59c16d3f69ec12

# 208.77 → 7143 Worker Runtime Gateway（SSH 隧道 localhost:28097 → 7143:28097）
UENV_GATEWAY_URL=http://127.0.0.1:28097
UENV_GATEWAY_PUBLIC_URL=http://219.147.100.43:28097

# Runner HTTP（208.77 对外）
OPENHANDS_RUNNER_HEALTH=http://8.130.208.77:8777/health
OPENHANDS_RUNNER_API=http://8.130.208.77:8888

# Driver (this repo)
UENV_OPENHANDS_DRIVER=integrations/openhands/run_swebenchpro_official.py

# Install location on 208.77
OPENHANDS_BENCHMARKS_DIR=/opt/openhands/benchmarks
OPENHANDS_HOST=8.130.208.77

# Deprecated: 7142 不再承载 OpenHands（7142 专用于 VeRL + 本地 LLM :18888）
# Legacy internal URL was http://10.10.20.143:28999
