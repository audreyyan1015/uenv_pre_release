#!/usr/bin/env bash
set -euo pipefail

# D1：固定墙钟时间下的 SWE-smith 有效训练量对比入口。
# 这个脚本只负责统一默认参数，并把执行分流到 UEnv 或 native baseline。
#
# 默认行为：
#   - 训练子集：LIMIT=100
#   - holdout 子集：由后续评测脚本固定使用
#   - 墙钟预算：WALL_CLOCK_BUDGET_SECONDS=21600（6h）
#   - backend：D1_BACKEND=uenv|native
#
# 示例：
#   D1_BACKEND=uenv ./scripts/experiments/d1/run_swe_smith_wallclock_compare.sh
#   D1_BACKEND=native NATIVE_SWE_RUNTIME_GATEWAY_URL=http://host:28097 \
#     ./scripts/experiments/d1/run_swe_smith_wallclock_compare.sh

usage() {
  cat <<'EOF'
Run the D1 SWE-smith wall-clock comparison entrypoint.

Usage:
  D1_BACKEND=uenv|native ./scripts/experiments/d1/run_swe_smith_wallclock_compare.sh [extra args]

Environment overrides:
  D1_BACKEND                  uenv or native. Default: uenv
  LIMIT                       SWE-smith train subset size. Default: 100
  OFFSET                      SWE-smith train subset offset. Default: 0
  WALL_CLOCK_BUDGET_SECONDS   Soft wall-clock budget for the batch. Default: 21600
  TRAIN_BATCH_SIZE            Default: 4
  ROLLOUT_N                   Default: 4
  TRAINING_STEPS              Safety cap for the underlying trainer. Default: 999999
  SWE_PREPARE_DATA            Default: 0
  NATIVE_SWE_RUNTIME_GATEWAY_URL
                              Required when D1_BACKEND=native
EOF
}

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
  usage
  exit 0
fi

REPO_DIR=${REPO_DIR:-"$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"}
cd "${REPO_DIR}"

D1_BACKEND=${D1_BACKEND:-uenv}
LIMIT=${LIMIT:-100}
OFFSET=${OFFSET:-0}
WALL_CLOCK_BUDGET_SECONDS=${WALL_CLOCK_BUDGET_SECONDS:-21600}

# D1 以“有效训练量”为主，不把 step 数作为唯一目标，但仍保留一个足够大的 safety cap。
TRAINING_STEPS=${TRAINING_STEPS:-999999}
TRAIN_BATCH_SIZE=${TRAIN_BATCH_SIZE:-4}
ROLLOUT_N=${ROLLOUT_N:-4}
SWE_PREPARE_DATA=${SWE_PREPARE_DATA:-0}

export LIMIT OFFSET WALL_CLOCK_BUDGET_SECONDS TRAINING_STEPS TRAIN_BATCH_SIZE ROLLOUT_N SWE_PREPARE_DATA

case "${D1_BACKEND}" in
  uenv)
    exec "${REPO_DIR}/scripts/train/launchers/swe/swe_smith_grpo_train.sh" --limit "${LIMIT}" --offset "${OFFSET}" "$@"
    ;;
  native)
    if [ -z "${NATIVE_SWE_RUNTIME_GATEWAY_URL:-}" ]; then
      echo "NATIVE_SWE_RUNTIME_GATEWAY_URL is required when D1_BACKEND=native." >&2
      exit 1
    fi
    exec "${REPO_DIR}/scripts/train/launchers/swe/native/swe_smith_native_verl_grpo_train.sh" --limit "${LIMIT}" --offset "${OFFSET}" "$@"
    ;;
  *)
    echo "D1_BACKEND must be uenv or native." >&2
    exit 2
    ;;
esac
