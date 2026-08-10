# OpenHands versions used by the UEnv SWE integration

OPENHANDS_BENCHMARKS_REPO=https://github.com/OpenHands/benchmarks
OPENHANDS_BENCHMARKS_SHA=82687c83dfcc193989336f41d235612c02f2c044

OPENHANDS_SDK_REPO=https://github.com/OpenHands/software-agent-sdk
OPENHANDS_SDK_SHA=43376f1868ffd702746080714a59c16d3f69ec12

Do not replace these SHAs with a floating branch in a release.  The generic
installer is `libexec/uenv/swe/install_openhands.sh`; the default install directory
is `/opt/uenv/agent/openhands-benchmarks`.
