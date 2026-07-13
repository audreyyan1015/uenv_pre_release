# DSCodeBench 代码生成基线评测

## 1. 任务说明

本轮对应任务书中的“3. 代码生成测试”。数据集选用 DSCodeBench / DS-Bench，用于评测模型在真实数据科学代码生成任务上的能力。每道题给出代码问题描述，模型需要输出 Python 代码；评测侧抽取代码块，结合官方测试脚本生成测试用例，执行模型代码并与标准实现输出做比对。

本阶段不进行后训练，只评估基准模型 `Qwen/Qwen3.6-35B-A3B` 的代码生成能力。

## 2. 数据集

数据集来源为 DSCodeBench 官方仓库，本地保存在：

```text
/data/ronghao/uenv/uenv-bridge/data/benchmarks/dscodebench/DSCodeBench.json
```

该文件为 JSONL 格式，共 1000 条样本。字段如下：

| 字段 | 说明 |
|---|---|
| `problem_id` | 题目 ID，例如 `numpy_0`。 |
| `library` | 所属数据科学库。 |
| `code_problem` | 需要模型解决的代码问题描述。 |
| `ground_truth_code` | 官方标准实现。 |
| `test_script` | 官方测试用例生成脚本。 |

数据分布：

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

## 3. 评测指标

主指标为 `pass@1`：每道题只采样 1 个答案，若生成代码能够通过该题所有测试用例，则记为通过。

本次同时记录辅助指标：

| 指标 | 含义 |
|---|---|
| `parse_rate` | 模型输出中能抽取到 Python markdown code block 的比例。 |
| `execution_rate` | 已抽取代码能够完成官方执行测试并产出测试结果的比例。 |
| `pass@1` | 已生成答案一次通过全部测试用例的比例。 |
| `error_count` | 执行阶段出现异常或超时的样本数。 |

官方评测脚本默认每题生成 200 个测试用例。本轮最终结果使用全量 1000 条样本、每题 200 个测试用例，并加入单题 300 秒超时保护，防止模型生成的长循环或训练代码卡住整轮评测。

## 4. 评测实现

新增脚本：

```text
/data/ronghao/uenv/uenv-bridge/scripts/benchmark/evaluate_dscodebench.py
/data/ronghao/uenv/uenv-bridge/scripts/benchmark/run_dscodebench_baseline.sh
```

实现方式：

1. `generate` 阶段使用 vLLM 加载 `Qwen/Qwen3.6-35B-A3B`，对 DSCodeBench 题目生成代码。
2. prompt 参考官方 `LLM_generate_solution.py`，并要求模型只返回一个 Python markdown code block，方便官方 `extract_code()` 抽取。
3. `evaluate` 阶段复用官方 `run_test.py` 中的 `extract_code()`、`get_exec_output()`、`evaluate_outputs()`。
4. 每道题的执行测试放入独立子进程，超过 `PER_PROBLEM_TIMEOUT` 后记为失败并继续评测后续样本。

## 5. 运行命令

```bash
cd /data/ronghao/uenv/uenv-bridge

nohup env OUTPUT_DIR=/data/ronghao/uenv/uenv-bridge/temp/benchmarks/dscodebench/qwen3_6_35b_a3b_full_official_tc200 \
LIMIT= \
LIBRARY= \
MAX_PER_LIBRARY= \
TEST_CASE_NUMBER=200 \
MAX_MODEL_LEN=32768 \
PER_PROBLEM_TIMEOUT=300 \
INSTALL_EVAL_DEPS=1 \
./scripts/benchmark/run_dscodebench_baseline.sh > /data/ronghao/uenv/uenv-bridge/temp/benchmarks/dscodebench/qwen3_6_35b_a3b_full_official_tc200.log 2>&1 &
```

说明：

1. `LIMIT=`、`LIBRARY=`、`MAX_PER_LIBRARY=` 均设置为空，表示不限制样本数、不限制库类型，使用 DSCodeBench 全量 1000 条样本。该写法要求 `run_dscodebench_baseline.sh` 中 `LIMIT` 只在未设置时使用默认值；当前脚本已按该语义处理。
2. `TEST_CASE_NUMBER=200` 对齐官方 `run_test.py` 的默认测试用例数量。
3. `MAX_MODEL_LEN=32768` 用于覆盖 DSCodeBench 中较长的代码题 prompt。当前全量 1000 条中最长样本为 `matplotlib_42`，prompt 约 28153 tokens；若仍使用默认 `8192`，生成阶段会在该样本处中断。
4. `PER_PROBLEM_TIMEOUT=300` 是外层单题保护，主要防止异常生成代码拖死整轮评测；官方执行逻辑内部仍会对模型生成代码设置 200 秒超时。若希望完全取消本项目外层超时保护，可改为 `PER_PROBLEM_TIMEOUT=0`，但异常样本可能导致整轮评测长时间卡住。
5. 如果已经完成生成、只想复用已有 `generations.json` 重新跑评测，可以额外加 `RUN_GENERATE=0`：

```bash
cd /data/ronghao/uenv/uenv-bridge

RUN_GENERATE=0 \
OUTPUT_DIR=/data/ronghao/uenv/uenv-bridge/temp/benchmarks/dscodebench/qwen3_6_35b_a3b_full_official_tc200 \
LIMIT= \
LIBRARY= \
MAX_PER_LIBRARY= \
TEST_CASE_NUMBER=200 \
MAX_MODEL_LEN=32768 \
PER_PROBLEM_TIMEOUT=300 \
INSTALL_EVAL_DEPS=1 \
./scripts/benchmark/run_dscodebench_baseline.sh
```

本次最终运行关键参数：

| 参数 | 值 |
|---|---|
| `GEN_IMAGE` | `localhost/vllm-openai:v0.19.0-cu130` |
| `EVAL_IMAGE` | `localhost/uenv-bridge-verl:layer4-build` |
| `MODEL_DIR` | `/data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B` |
| `TENSOR_PARALLEL_SIZE` | `8` |
| `MAX_MODEL_LEN` | `32768` |
| `MAX_TOKENS` | `2048` |
| `TEMPERATURE` | `0.2` |
| `TOP_P` | `1.0` |
| `DISABLE_THINKING` | `1` |
| `TEST_CASE_NUMBER` | `200` |
| `PER_PROBLEM_TIMEOUT` | `300` |

## 6. 全量官方对齐结果

本次最终产物路径如下：

```text
/data/ronghao/uenv/uenv-bridge/temp/benchmarks/dscodebench/qwen3_6_35b_a3b_full_official_tc200/generations.json
/data/ronghao/uenv/uenv-bridge/temp/benchmarks/dscodebench/qwen3_6_35b_a3b_full_official_tc200/evaluation_results.jsonl
/data/ronghao/uenv/uenv-bridge/temp/benchmarks/dscodebench/qwen3_6_35b_a3b_full_official_tc200/metrics.json
/data/ronghao/uenv/uenv-bridge/temp/benchmarks/dscodebench/qwen3_6_35b_a3b_full_official_tc200.log
```

最终生成阶段完成全量 1000 条样本，日志末尾显示：

```json
{
  "generated": 1000,
  "output": "/data/ronghao/uenv/uenv-bridge/temp/benchmarks/dscodebench/qwen3_6_35b_a3b_full_official_tc200"
}
```

生成阶段统计：

| 项目 | 值 |
|---|---:|
| 实际生成样本数 | 1000 |
| `output_tokens` 最小值 | 80 |
| `output_tokens` 最大值 | 2048 |
| `output_tokens` 平均值 | 689.39 |
| 触达 `MAX_TOKENS=2048` 的样本数 | 35 |

分库生成数量：

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

评测阶段使用 `TEST_CASE_NUMBER=200` 和 `PER_PROBLEM_TIMEOUT=300`。最终 `evaluation_results.jsonl` 共 1000 行，其中 877 条完成 200 个测试用例执行，123 条未执行完成；未执行完成的样本主要来自输出不可解析或单题超时。

总体指标：

| problem_count | parsed_count | executed_count | passed_count | error_count | parse_rate | execution_rate | pass@1 |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 1000 | 965 | 877 | 348 | 70 | 0.965 | 0.877 | 0.348 |

分库指标：

| library | problem_count | parse_rate | execution_rate | pass@1 | error_count |
|---|---:|---:|---:|---:|---:|
| keras | 104 | 0.990 | 0.885 | 0.452 | 7 |
| lightgbm | 54 | 1.000 | 0.370 | 0.130 | 33 |
| matplotlib | 105 | 1.000 | 0.962 | 0.276 | 3 |
| numpy | 131 | 0.947 | 0.916 | 0.313 | 1 |
| pandas | 92 | 0.935 | 0.913 | 0.283 | 1 |
| pytorch | 101 | 0.891 | 0.891 | 0.554 | 0 |
| scipy | 112 | 0.964 | 0.955 | 0.384 | 0 |
| seaborn | 83 | 1.000 | 0.867 | 0.133 | 10 |
| sklearn | 108 | 0.991 | 0.796 | 0.444 | 15 |
| tensorflow | 110 | 0.955 | 0.955 | 0.364 | 0 |

异常统计：

| 项目 | 数量 |
|---|---:|
| 未解析出 Python code block | 35 |
| 未完成执行 | 123 |
| 记录 error 的样本 | 70 |
| `TimeoutError: exceeded 300s` | 69 |
| `ProcessError: evaluator exited with code -6` | 1 |

说明：`error_count` 只统计评测脚本记录了 `error` 字段的样本；另有部分样本因为未解析出代码或未能产出可执行结果而计入未执行完成，但不一定带有 `error` 字段。`case_count=200` 的 877 条样本说明官方对齐测试用例数量已经生效。

## 7. 结论

在全量 1000 条样本、每题 200 个测试用例口径下，`Qwen/Qwen3.6-35B-A3B` 在 DSCodeBench 上的 `pass@1=0.348`，`parse_rate=0.965`，`execution_rate=0.877`。模型输出整体可解析率较高，说明代码块格式基本可用；主要失败来源包括输出代码逻辑未通过测试、少数输出不可解析，以及部分生成代码执行时间过长。

该结果已经跑通“模型生成 + 官方代码抽取 + 官方测试执行 + 指标统计”的完整链路，并完成 `TEST_CASE_NUMBER=200` 的全量官方对齐口径评测。分库结果上，`pytorch`、`keras`、`sklearn` 表现相对较好；`lightgbm` 和 `seaborn` 的 `pass@1` 较低，其中 `lightgbm` 的主要问题是执行超时较多。
