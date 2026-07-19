# DSCodeBench UEnv 评测 Worker 核验问题说明

## 1. 背景

本轮 DSCodeBench 已经通过 UEnv 完成全量 1000 条样本评测，链路如下：

```text
Adapter -> Adapter Core / Server -> Worker code env -> Model Gateway -> vLLM -> Worker harness -> Adapter
```

本次 UEnv 全量结果路径：

```text
/data/ronghao/uenv/uenv-bridge/temp/benchmarks/dscodebench/qwen3_6_35b_a3b_uenv_thinking_max32768_budget16384_full_20260717_211508/
```

核心配置：

| 配置 | 值 |
|---|---|
| 模型 | `Qwen/Qwen3.6-35B-A3B` |
| Adapter Core | `8.130.75.157:8088` |
| Model Gateway | `http://10.10.20.142:18094/v1` |
| Evaluation mode | `inline_harness` |
| Prompt style | `official_fenced` |
| Thinking | 开启 |
| `MAX_TOKENS` | 32768 |
| `THINKING_TOKEN_BUDGET` | 16384 |
| `TEST_CASE_NUMBER` | 200 |
| `CODE_TIMEOUT_SECS` | 300 |

结果概览：

| problem_count | completed_count | failed_count | passed_count | pass@1 / reward_accuracy | error_count |
|---:|---:|---:|---:|---:|---:|
| 1000 | 1000 | 0 | 267 | 0.267 | 733 |

这里的 `failed_count=0` 表示 UEnv 调度层面没有失败；`error_count=733` 主要来自 Worker code env 执行候选代码或 harness 判分时返回的错误。

## 2. 需要 Worker 核验的问题一：执行环境与直接 baseline 不完全一致

直接 vLLM baseline 的结果为：

| 口径 | pass@1 | execution_rate | error_count |
|---|---:|---:|---:|
| 直接 vLLM + 官方本地 evaluator | 0.348 | 0.877 | 70 |
| UEnv + Worker code env | 0.267 | 0.267 | 733 |

这两组不是严格同参数实验，因此不能直接判定是 UEnv 传输链路导致精度下降。但 UEnv 结果中存在一些明显与 Worker code env / Python 依赖 / harness 执行环境相关的错误，需要 Worker 侧核验。

### 2.1 当前观察到的错误类型

| 类型 | 数量 | 说明 |
|---|---:|---|
| test assertion failed / wrong answer | 390 | harness 返回未通过，部分样本实际有 `tests_run=200` 和部分通过数。 |
| candidate produced no outputs | 229 | 候选代码没有产生可比较输出，通常是语法、运行时错误或函数实现不符合预期。 |
| numpy array truth value ambiguous | 46 | harness 比较或模型代码中出现 NumPy array 布尔判断问题。 |
| evaluation timed out after 300s | 25 | code env 单题执行超时。 |
| LightGBM runtime / warning error | 20 | LightGBM 训练或指标配置相关运行问题。 |
| pandas Series truth value ambiguous | 6 | pandas Series 布尔判断问题。 |
| ImportError | 4 | 生成代码中存在不兼容导入。 |
| missing python dependency / import | 3 | 少量样本仍触发缺失或不兼容依赖。 |
| other worker error | 10 | 其他执行错误。 |

### 2.2 典型样例

部分样本不是调度失败，而是 Worker 执行环境或 harness 返回错误。例如：

```text
problem_id=scipy_21
library=scipy
error=ModuleNotFoundError: No module named 'scipy.mstats'
```

```text
problem_id=scipy_12
library=scipy
error=ImportError: cannot import name 'mstats' from 'scipy'
```

```text
problem_id=numpy_104
library=numpy
error=evaluation timed out after 300s
```

```text
problem_id=pandas_9
library=pandas
error=The truth value of a Series is ambiguous
```

这些问题可能来自三类差异：

1. Worker code env 的 Python 包版本与直接 baseline evaluator 不一致。
2. Worker 内的 `dscodebench_harness.py` 与直接 baseline 使用的官方 evaluator 行为不一致。
3. Worker 对候选代码执行错误、依赖错误、超时错误的结构化返回粒度不足。

### 2.3 建议 Worker 侧核验

请 Worker 侧确认：

1. 当前 code env 实际使用的 Python 路径，例如 `UENV_CODE_PYTHON` 指向哪里。
2. DSCodeBench 需要的 10 个库是否都已安装，并记录版本：

```bash
${UENV_CODE_PYTHON:-python3} - <<'PY'
import numpy, pandas, scipy, sklearn, matplotlib, seaborn, lightgbm, tensorflow, keras, torch
for m in [numpy, pandas, scipy, sklearn, matplotlib, seaborn, lightgbm, tensorflow, keras, torch]:
    print(m.__name__, getattr(m, "__version__", "unknown"))
PY
```

3. Worker code env 中的 DSCodeBench harness 是否和直接 baseline 使用的官方 evaluator 对齐。
4. `scipy.mstats`、`numpy.ma.mstats`、TensorFlow/Keras、LightGBM 相关错误是否属于环境兼容问题，还是模型生成代码本身的问题。
5. 超时样本是否需要更细粒度记录：是候选代码死循环、模型生成代码过慢、还是官方测试脚本本身耗时较长。

## 3. 需要 Worker 核验的问题二：失败返回口径导致指标不可比

当前 UEnv 结果中：

```text
tests_run=200: 267 条
tests_run=0:   733 条
```

这并不表示 733 条完全没有运行测试。原因是当前 `inline_harness` wrapper 在 `_result.passed=false` 时会抛 `AssertionError`：

```python
_result = evaluate_problem(...)
if not _result.get("passed"):
    raise AssertionError(json.dumps(_result, ensure_ascii=False))
```

Worker 捕获到这个异常后，顶层结果被记录为：

```json
{
  "passed": false,
  "tests_run": 0,
  "tests_passed": 0,
  "error": "Traceback ... AssertionError: {\"passed\": false, \"tests_run\": 200, \"tests_passed\": 100, ...}"
}
```

也就是说，真实的 harness 结果在 `AssertionError` 字符串内部，但 Worker 顶层结构没有展开它，导致 Adapter 侧只能看到 `tests_run=0`。

### 3.1 典型样例

例如 `numpy_2`：

```text
顶层:
tests_run=0
tests_passed=0
worker_error=AssertionError

AssertionError 内部:
{"passed": false, "tests_run": 200, "tests_passed": 100, "error": "some tests failed"}
```

例如 `numpy_0`：

```text
顶层:
tests_run=0
tests_passed=0
worker_error=AssertionError

AssertionError 内部:
{"passed": false, "tests_run": 200, "tests_passed": 0, "error": "candidate produced no outputs (syntax/runtime error?)"}
```

这会造成两个问题：

1. `execution_rate` 被压低为 `0.267`，它实际更像“全通过比例”，不等价于直接 baseline 的“成功执行比例”。
2. `error_count=733` 混合了“模型答案错误”和“环境/运行时错误”，不能直接作为 Worker 环境错误数量。

### 3.2 建议 Worker 侧返回结构

建议 Worker code env 无论候选代码是否通过，都返回结构化字段，而不是把 harness 的失败结果只放在异常字符串里。

建议至少包含：

| 字段 | 类型 | 说明 |
|---|---|---|
| `passed` | bool | 是否全部测试通过 |
| `tests_run` | int | 实际运行测试数 |
| `tests_passed` | int | 通过测试数 |
| `error` | string | 失败原因摘要 |
| `error_category` | string | `wrong_answer` / `candidate_runtime_error` / `dependency_error` / `timeout` / `harness_error` 等 |
| `execution_time_ms` | int | Worker 执行评测耗时 |
| `raw_error` | string | 可选，完整 traceback |

推荐语义：

| 场景 | `tests_run` | `tests_passed` | `error_category` |
|---|---:|---:|---|
| 全部通过 | 200 | 200 | 空或 `none` |
| 部分测试失败 | 200 | 例如 100 | `wrong_answer` |
| 候选代码运行失败，无法产出输出 | 200 或实际尝试数 | 0 | `candidate_runtime_error` |
| Python 包缺失或导入不兼容 | 0 | 0 | `dependency_error` |
| 单题超时 | 已完成数或 0 | 已通过数或 0 | `timeout` |
| harness 自身异常 | 0 | 0 | `harness_error` |

### 3.3 可选实现方向

短期可以选一种方式：

1. Worker 解析 `AssertionError` 中的 JSON，把内部 `_result` 展开到顶层结果。
2. Worker 调整 `evaluate_code.py`，让 `inline_harness` 返回 `_result`，不要把“测试未通过”作为异常处理。
3. Adapter 与 Worker 约定新的 `test_code` 返回协议，由 Worker 从 namespace 中读取 `_result`。

长期建议把“模型答案错误”和“环境执行错误”分开：

```text
模型答案错误: passed=false, tests_run>0, error_category=wrong_answer
环境执行错误: status=failed 或 error_category=dependency_error/timeout/harness_error
```

这样 Adapter 侧才能准确区分：

1. 模型能力问题。
2. Worker code env 依赖或执行问题。
3. UEnv 调度链路问题。

## 4. 希望 Worker 侧确认的问题清单

请 Worker 侧重点核验：

1. 当前 code env 是否与直接 baseline evaluator 的依赖版本一致。
2. `scipy.mstats`、`numpy.ma.mstats`、TensorFlow/Keras、LightGBM 相关错误是否可通过环境或 harness 兼容性修复。
3. 对 `_result.passed=false` 的样本，是否能返回 `_result.tests_run` 和 `_result.tests_passed`，而不是顶层统一记为 `tests_run=0`。
4. `error_count` 是否可以拆分为 `wrong_answer_count`、`candidate_runtime_error_count`、`dependency_error_count`、`timeout_count`、`harness_error_count`。
5. 后续如果切换到 `test_script_path + UENV_DSCODEBENCH_ROOT`，Worker 是否能保证路径、依赖和 harness 版本稳定。

## 5. Adapter 侧当前结论

当前结果不能直接说明 UEnv 传输链路降低了 DSCodeBench 效果。更合理的判断是：

1. UEnv 调度链路已经完成 1000/1000 条样本，没有调度层 failed。
2. UEnv 与直接 baseline 的生成参数不同，尤其是 thinking 开启和 `MAX_TOKENS=32768`。
3. Worker code env 与直接 baseline evaluator 存在环境和返回口径差异。
4. 在 Worker 返回结构修正前，UEnv 侧的 `execution_rate` 和直接 baseline 的 `execution_rate` 不可直接比较。

后续如需判断“UEnv 是否影响模型效果”，建议补跑同口径实验：

```text
UEnv + thinking 关闭 + MAX_TOKENS=2048 + TEST_CASE_NUMBER=200
```

然后再和直接 vLLM baseline 对比。
