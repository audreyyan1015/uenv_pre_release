# DSCodeBench UEnv 评测 Worker 依赖问题说明

## 1. 背景

本轮目标是在 UEnv 链路下测试 DSCodeBench 全量代码生成任务：

```text
Adapter -> AdapterCore/Server -> Worker(code env) -> Model Gateway/vLLM -> Worker 评测 -> Adapter
```

Adapter 侧已经补充 DSCodeBench 的 UEnv 评测脚本，并完成 1 条样本的真实链路 smoke。当前链路能够正常调度到 Worker，模型也能够返回代码，但 Worker code env 的 Python 执行环境缺少 DSCodeBench 所需的数据科学依赖，导致评测无法有效进行。

## 2. 本次 smoke 配置

Adapter 侧运行命令：

```bash
cd /data/ronghao/uenv/uenv-bridge

OUTPUT_DIR=/data/ronghao/uenv/uenv-bridge/temp/benchmarks/dscodebench/qwen3_6_35b_a3b_uenv_thinking_max32768_budget16384_smoke_20260717_103422 \
UENV_ROLLOUT_MODEL_ENDPOINT=http://10.10.20.142:18094/v1 \
LIMIT=1 \
MAX_TOKENS=32768 \
THINKING_TOKEN_BUDGET=16384 \
ENABLE_THINKING=1 \
TEST_CASE_NUMBER=2 \
CODE_TIMEOUT_SECS=120 \
TIMEOUT_SECONDS=1200 \
CLIENT_TIMEOUT_SECONDS=1500 \
./scripts/benchmark/run_dscodebench_uenv_baseline.sh
```

产物路径：

```text
/data/ronghao/uenv/uenv-bridge/temp/benchmarks/dscodebench/qwen3_6_35b_a3b_uenv_thinking_max32768_budget16384_smoke_20260717_103422/uenv_requests.jsonl
/data/ronghao/uenv/uenv-bridge/temp/benchmarks/dscodebench/qwen3_6_35b_a3b_uenv_thinking_max32768_budget16384_smoke_20260717_103422/uenv_results.jsonl
/data/ronghao/uenv/uenv-bridge/temp/benchmarks/dscodebench/qwen3_6_35b_a3b_uenv_thinking_max32768_budget16384_smoke_20260717_103422/metrics.json
```

## 3. Adapter 发给 Worker 的关键字段

当前请求使用：

| 字段 | 值 / 说明 |
|---|---|
| `env_type` | `code` |
| `dataset` | `dscodebench` |
| `task_id` | 例如 `numpy_0` |
| `library` | 例如 `numpy` |
| `question` | DSCodeBench 的代码题 prompt |
| `ground_truth_code` | 官方标准实现 |
| `test_code` | Adapter 构造的 inline wrapper，用于调用 Worker 侧 `dscodebench_harness.evaluate_problem()` |
| `num_tests` | smoke 为 `2`，全量正式评测为 `200` |
| `random_seed` | `42 + sample_index` |
| `timeout_secs` | code env 内部单题执行超时 |
| `model_endpoint.url` | `http://10.10.20.142:18094/v1` |
| `generation_config.max_tokens` | `32768` |
| `generation_config.thinking_token_budget` | `16384` |
| `generation_config.chat_template_kwargs.enable_thinking` | `true` |

说明：本次没有依赖 Worker 机器上的 DSCodeBench 数据文件路径，而是通过请求 payload 直接携带 `ground_truth_code` 和由 `test_script` 生成的 inline `test_code`。因此当前 blocker 不是找不到 DSCodeBench 数据文件，而是 Worker 执行候选代码时缺少 Python 库。

## 4. 当前现象

smoke 返回：

| 项 | 值 |
|---|---|
| `request_id` | `dscodebench-numpy_0-e52e5faf` |
| `problem_id` | `numpy_0` |
| `library` | `numpy` |
| `uenv_status` | `completed` |
| `uenv_reward` | `0.0` |
| `tests_run` | `0` |
| `tests_passed` | `0` |

Worker 返回的关键错误：

```text
Traceback (most recent call last):
  File "/root/UEnv/plugins/code/scripts/evaluate_code.py", line 119, in main
    result = run_inline_tests(cfg, started)
  File "/root/UEnv/plugins/code/scripts/evaluate_code.py", line 43, in run_inline_tests
    exec(compile(code, "<candidate>", "exec"), namespace)
  File "<candidate>", line 1, in <module>
ModuleNotFoundError: No module named 'numpy'
```

含义：

1. 请求已经到达 Worker 的 `code` 插件。
2. Worker 已经拿到模型生成代码，并进入 `evaluate_code.py`。
3. 失败发生在执行候选代码阶段，第一行 `import numpy as np` 无法导入。
4. 因为候选代码执行失败，后续 DSCodeBench harness 没有机会开始跑测试用例，所以 `tests_run=0`。

## 5. 需要 Worker 侧处理的问题

DSCodeBench 覆盖多个数据科学库，全量 1000 条样本分布如下：

| library | 样本数 |
|---|---:|
| numpy | 131 |
| scipy | 112 |
| tensorflow | 110 |
| sklearn | 108 |
| matplotlib | 105 |
| keras | 104 |
| pytorch | 101 |
| pandas | 92 |
| seaborn | 83 |
| lightgbm | 54 |

Worker code env 的 Python 至少需要能导入这些库。否则全量评测会出现系统性失败，指标没有参考价值。

建议 Worker 侧确认以下内容：

1. `uenv-code-plugin` 实际使用的 Python 是哪个。当前代码逻辑中，如果未设置 `UENV_CODE_PYTHON`，默认使用 `python3`。
2. 该 Python 环境是否能导入 DSCodeBench 所需库。
3. 如果依赖安装在独立 conda/venv 中，需要设置 `UENV_CODE_PYTHON` 指向该环境的 Python。
4. 依赖安装或环境变量变更后，需要重启 Worker / code plugin，使新环境生效。

可在 Worker 机器上用类似命令检查：

```bash
${UENV_CODE_PYTHON:-python3} - <<'PY'
import numpy
import pandas
import scipy
import sklearn
import matplotlib
import seaborn
import lightgbm
import tensorflow
import keras
import torch
print("dscodebench deps ok")
PY
```

如果该命令失败，则 DSCodeBench 全量评测暂时不能有效运行。

## 6. 修复后 Adapter 侧验证方式

Worker 环境修复后，Adapter 侧可以先重跑 1 条 smoke：

```bash
cd /data/ronghao/uenv/uenv-bridge

UENV_ROLLOUT_MODEL_ENDPOINT=http://10.10.20.142:18094/v1 \
LIMIT=1 \
MAX_TOKENS=32768 \
THINKING_TOKEN_BUDGET=16384 \
ENABLE_THINKING=1 \
TEST_CASE_NUMBER=2 \
CODE_TIMEOUT_SECS=120 \
TIMEOUT_SECONDS=1200 \
CLIENT_TIMEOUT_SECONDS=1500 \
./scripts/benchmark/run_dscodebench_uenv_baseline.sh
```

期望现象：

1. `uenv_status=completed`。
2. 不再出现 `ModuleNotFoundError`。
3. `tests_run` 应该大于 0。smoke 中为 `2`，正式全量中应接近 `200`。
4. `uenv_reward` 根据模型生成代码是否通过测试为 `0.0` 或 `1.0`，但不应因为依赖缺失导致 `tests_run=0`。

smoke 通过后，再启动正式全量：

```bash
cd /data/ronghao/uenv/uenv-bridge

UENV_ROLLOUT_MODEL_ENDPOINT=http://10.10.20.142:18094/v1 \
MAX_TOKENS=32768 \
THINKING_TOKEN_BUDGET=16384 \
ENABLE_THINKING=1 \
TEST_CASE_NUMBER=200 \
CODE_TIMEOUT_SECS=300 \
TIMEOUT_SECONDS=7200 \
CLIENT_TIMEOUT_SECONDS=7800 \
./scripts/benchmark/run_dscodebench_uenv_baseline.sh
```

## 7. 两种评测脚本交付方式

DSCodeBench 的评测需要两类材料：

1. `ground_truth_code`：官方标准实现。
2. `test_script`：官方测试用例生成脚本。

这些材料可以通过两种方式交给 Worker。

### 7.1 inline_harness

`inline_harness` 是当前 Adapter 使用的方式。Adapter 会把每道题的 `ground_truth_code` 和 `test_script` 都放进本次 `EpisodeRequest`，并构造一个 `test_code` wrapper。Worker 收到请求后，不需要本地提前保存 DSCodeBench 数据文件，直接使用请求里的内容完成评测。

当前请求大致是：

```json
{
  "env_type": "code",
  "dataset": "dscodebench",
  "task_id": "numpy_0",
  "ground_truth_code": "...",
  "test_code": "from dscodebench_harness import evaluate_problem\n..."
}
```

优点：

1. 请求自包含，适合当前联调。
2. Worker 不需要额外同步 DSCodeBench 数据文件。
3. 能减少“路径不一致 / 文件缺失”导致的联调问题。

缺点：

1. 每个 request payload 会比较大。
2. 全量 1000 条样本会重复传输大量 `ground_truth_code` 和 `test_script`。
3. 长期看不如环境包方式清晰。

### 7.2 test_script_path + UENV_DSCODEBENCH_ROOT

这是更正式的文件路径方式。Worker 机器提前同步好 DSCodeBench 数据和测试脚本，并设置：

```bash
export UENV_DSCODEBENCH_ROOT=/path/to/dscodebench
```

Adapter 请求中不再传完整 `test_script`，而是传相对路径：

```json
{
  "env_type": "code",
  "dataset": "dscodebench",
  "task_id": "numpy_0",
  "ground_truth_code": "...",
  "test_script_path": "numpy/numpy_0.py"
}
```

Worker 根据 `UENV_DSCODEBENCH_ROOT + test_script_path` 读取测试脚本，再运行官方 harness。

优点：

1. request 更小。
2. 更适合全量评测和长期运行。
3. 更符合后续 Hub EnvPackage / 环境包同步的设计。

缺点：

1. Worker 侧必须先同步 DSCodeBench 文件。
2. `UENV_DSCODEBENCH_ROOT` 和 `test_script_path` 的路径约定必须稳定。
3. 如果路径或文件缺失，会出现“找不到 test script”的失败。

短期建议继续使用 `inline_harness` 完成联调和全量首轮评测；如果后续要产品化或大规模反复评测，再切换到 `test_script_path + UENV_DSCODEBENCH_ROOT`。

## 8. 需要确认的问题

请 Worker 侧确认：

1. 当前部署的 code env 是否计划支持 DSCodeBench 全量官方依赖。
2. `UENV_CODE_PYTHON` 是否已经指向包含上述依赖的 Python 环境。
3. 如果依赖通过 Hub EnvPackage 分发，是否已经在当前 Worker 机器同步并生效。
4. code env 是否希望继续使用当前 `inline_harness` 方式，还是后续切换为 `test_script_path + UENV_DSCODEBENCH_ROOT` 的文件路径方式。
