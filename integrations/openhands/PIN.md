# OpenHands pin record (7142 deployment)
#
# Update only after gold + LLM smoke on 7143 Pro instance.

OPENHANDS_BENCHMARKS_REPO=https://github.com/OpenHands/benchmarks
OPENHANDS_BENCHMARKS_SHA=82687c83dfcc193989336f41d235612c02f2c044

OPENHANDS_SDK_REPO=https://github.com/OpenHands/software-agent-sdk
OPENHANDS_SDK_SHA=43376f1868ffd702746080714a59c16d3f69ec12

UENV_GATEWAY_URL=http://10.10.20.143:28999
UENV_GATEWAY_PUBLIC_URL=http://219.147.100.43:28099

# Driver (this repo)
UENV_OPENHANDS_DRIVER=integrations/openhands/run_swebenchpro_official.py

# Install location on 7142
OPENHANDS_BENCHMARKS_DIR=/opt/openhands/benchmarks
