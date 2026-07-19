# DSCodeBench 代码生成基线评测

## 1. 任务说明

本轮对应任务书中的“3. 代码生成测试”。数据集选用 DSCodeBench / DS-Bench，用于评测模型在真实数据科学代码生成任务上的能力。每道题给出代码问题描述，模型需要输出 Python 代码；评测侧抽取代码块，结合官方测试脚本生成测试用例，执行模型代码并与标准实现输出做比对。

本阶段不进行后训练，只评估基准模型 `Qwen/Qwen3.6-35B-A3B` 的代码生成能力。

本文档包含两组结果：

| 章节 | 口径 | 用途 |
|---|---|---|
| 第 5-8 节 | 直接 vLLM + 官方抽取/测试执行，thinking 关闭，`MAX_TOKENS=2048` | 早期官方对齐 baseline，保留作为历史参考。 |
| 第 9-12 节 | UEnv 全链路，thinking 开启，`MAX_TOKENS=32768`，`THINKING_TOKEN_BUDGET=16384` | 本轮正式接入 UEnv 后的全量结果。 |

两组结果不是严格同参数对比。本轮 UEnv 正式结果以第 9-12 节为准。

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

官方评测脚本默认每题生成 200 个测试用例。本轮 UEnv 正式结果使用全量 1000 条样本、每题 200 个测试用例，并加入单题 300 秒超时保护，防止模型生成的长循环或训练代码卡住整轮评测。

## 4. 评测实现

直接 vLLM baseline 脚本：

```text
/data/ronghao/uenv/uenv-bridge/scripts/benchmark/evaluate_dscodebench.py
/data/ronghao/uenv/uenv-bridge/scripts/benchmark/run_dscodebench_baseline.sh
```

UEnv 全链路评测脚本：

```text
/data/ronghao/uenv/uenv-bridge/scripts/benchmark/evaluate_dscodebench_uenv.py
/data/ronghao/uenv/uenv-bridge/scripts/benchmark/run_dscodebench_uenv_baseline.sh
```

直接 vLLM baseline 的实现方式：

1. `generate` 阶段使用 vLLM 加载 `Qwen/Qwen3.6-35B-A3B`，对 DSCodeBench 题目生成代码。
2. prompt 参考官方 `LLM_generate_solution.py`，并要求模型只返回一个 Python markdown code block，方便官方 `extract_code()` 抽取。
3. `evaluate` 阶段复用官方 `run_test.py` 中的 `extract_code()`、`get_exec_output()`、`evaluate_outputs()`。
4. 每道题的执行测试放入独立子进程，超过 `PER_PROBLEM_TIMEOUT` 后记为失败并继续评测后续样本。

UEnv 全链路的实现方式：

1. Adapter 为每道 DSCodeBench 样本构造 `EpisodeRequest`，显式写入 `dataset=dscodebench`、`task_id`、`library`、`ground_truth_code`、`test_code` 等字段。
2. 请求经 Adapter Core / Server 分发到 Worker code env。
3. Worker 通过 Model Gateway 请求本机 vLLM 生成代码，然后在 code env 中运行 DSCodeBench harness。
4. Adapter 回收 `EpisodeResult`，生成 `uenv_results.jsonl`、`predictions.jsonl` 和 `metrics.json`。

## 5. 直接 vLLM 全量官方对齐配置（历史 baseline）

| 配置 | 值 |
|---|---|
| 评测口径 | 直接 vLLM 生成 + 官方代码抽取/测试执行 |
| 模型 | `Qwen/Qwen3.6-35B-A3B` |
| 生成镜像 `GEN_IMAGE` | `localhost/vllm-openai:v0.19.0-cu130` |
| 评测镜像 `EVAL_IMAGE` | `localhost/uenv-bridge-verl:layer4-build` |
| 模型目录 `MODEL_DIR` | `/data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B` |
| GPU | 8 张 A100 |
| Tensor parallel | 8 |
| `MAX_MODEL_LEN` | 32768 |
| `MAX_TOKENS` | 2048 |
| `TEMPERATURE` | 0.2 |
| `TOP_P` | 1.0 |
| Thinking mode | 关闭，`DISABLE_THINKING=1` |
| 数据集 | DSCodeBench 全量 1000 条 |
| 库过滤 | 不限制，覆盖 10 个数据科学库 |
| 官方测试用例数 `TEST_CASE_NUMBER` | 200 |
| 单题外层超时 `PER_PROBLEM_TIMEOUT` | 300s |
| 输出目录 | `temp/benchmarks/dscodebench/qwen3_6_35b_a3b_full_official_tc200/` |
| 后训练 | 未进行 SFT/RL，Eval-first 基线 |

说明：本节是历史直接 vLLM baseline，保留用于参考；本轮正式 UEnv 结果见第 9-12 节。

## 6. 直接 vLLM 运行命令

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

直接 vLLM baseline 的关键参数见第 5 节。

## 7. 直接 vLLM 全量官方对齐结果

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

## 8. 直接 vLLM baseline 结论

在全量 1000 条样本、每题 200 个测试用例口径下，`Qwen/Qwen3.6-35B-A3B` 在 DSCodeBench 上的 `pass@1=0.348`，`parse_rate=0.965`，`execution_rate=0.877`。模型输出整体可解析率较高，说明代码块格式基本可用；主要失败来源包括输出代码逻辑未通过测试、少数输出不可解析，以及部分生成代码执行时间过长。

该结果已经跑通“模型生成 + 官方代码抽取 + 官方测试执行 + 指标统计”的完整链路，并完成 `TEST_CASE_NUMBER=200` 的全量官方对齐口径评测。分库结果上，`pytorch`、`keras`、`sklearn` 表现相对较好；`lightgbm` 和 `seaborn` 的 `pass@1` 较低，其中 `lightgbm` 的主要问题是执行超时较多。

## 9. UEnv Thinking 全量配置

本轮补充接入 UEnv 链路后的 DSCodeBench 全量评测。整体链路为：

```text
Adapter -> Adapter Core / Server -> Worker code env -> Model Gateway -> vLLM -> Worker harness -> Adapter
```

关键配置如下：

| 配置 | 值 |
|---|---|
| 评测口径 | UEnv 链路生成与 Worker code env 执行评测 |
| 模型 | `Qwen/Qwen3.6-35B-A3B` |
| Adapter Core | `8.130.75.157:8088` |
| Model Gateway | `http://10.10.20.142:18094/v1` |
| Gateway upstream | `http://127.0.0.1:18081/v1` |
| vLLM 端口 | `18081` |
| Tensor parallel | 8 |
| `max_model_len` | 65536 |
| `MAX_TOKENS` | 32768 |
| `THINKING_TOKEN_BUDGET` | 16384 |
| Thinking mode | 开启，`ENABLE_THINKING=1` |
| Gateway reasoning 处理 | 使用 `--strip-reasoning`，只向 Worker 返回最终代码 content |
| Adapter `PRESERVE_THINKING` | `0` |
| Prompt style | `official_fenced` |
| Evaluation mode | `inline_harness` |
| 数据集 | DSCodeBench 全量 1000 条 |
| 库过滤 | 不限制，覆盖 10 个数据科学库 |
| Worker 测试用例数 | `TEST_CASE_NUMBER=200` |
| Worker 单题执行超时 | `CODE_TIMEOUT_SECS=300` |
| UEnv Episode 超时 | `TIMEOUT_SECONDS=7200` |
| 后训练 | 未进行 SFT/RL，Eval-first 基线 |

`inline_harness` 表示 Adapter 将每道题的 `ground_truth_code` 与由 `test_script` 构造出的 `test_code` wrapper 直接放入 `EpisodeRequest`，Worker 不依赖本地 `test_script_path + UENV_DSCODEBENCH_ROOT` 查找测试脚本。

本次复用的 `18094` Model Gateway 开启 thinking，但会在返回 Worker 前移除 reasoning 字段，避免思考过程混入代码抽取与执行评测。

## 10. UEnv 全量运行命令

启动 8GPU vLLM：

```bash
podman run -d --name uenv-dscodebench-vllm-18081 \
  --entrypoint python3 \
  --network host \
  --pids-limit=-1 \
  --shm-size=64g \
  --device nvidia.com/gpu=all \
  -v /data/ronghao:/data/ronghao \
  -w /data/ronghao/uenv/uenv-bridge \
  localhost/vllm-openai:v0.19.0-cu130 \
  -m vllm.entrypoints.openai.api_server \
  --model /data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B \
  --served-model-name Qwen/Qwen3.6-35B-A3B \
  --host 0.0.0.0 \
  --port 18081 \
  --tensor-parallel-size 8 \
  --max-model-len 65536 \
  --gpu-memory-utilization 0.90 \
  --reasoning-parser qwen3 \
  --reasoning-config "{\"reasoning_start_str\":\"<think>\",\"reasoning_end_str\":\"</think>\"}" \
  --trust-remote-code
```

运行 UEnv 全量评测：

```bash
cd /data/ronghao/uenv/uenv-bridge

OUTPUT_DIR=/data/ronghao/uenv/uenv-bridge/temp/benchmarks/dscodebench/qwen3_6_35b_a3b_uenv_thinking_max32768_budget16384_full_20260717_211508 \
UENV_ROLLOUT_MODEL_ENDPOINT=http://10.10.20.142:18094/v1 \
UENV_ROLLOUT_MODEL_NAME=Qwen/Qwen3.6-35B-A3B \
LIMIT= \
LIBRARY= \
MAX_PER_LIBRARY= \
BATCH_SIZE=1 \
PROMPT_STYLE=official_fenced \
MAX_TOKENS=32768 \
ENABLE_THINKING=1 \
PRESERVE_THINKING=0 \
THINKING_TOKEN_BUDGET=16384 \
TEMPERATURE=0.2 \
TOP_P=1.0 \
TEST_CASE_NUMBER=200 \
CODE_TIMEOUT_SECS=300 \
TIMEOUT_SECONDS=7200 \
CLIENT_TIMEOUT_SECONDS=7800 \
EVALUATION_MODE=inline_harness \
RESUME=0 \
./scripts/benchmark/run_dscodebench_uenv_baseline.sh
```

本次全量产物路径如下：

```text
/data/ronghao/uenv/uenv-bridge/temp/benchmarks/dscodebench/qwen3_6_35b_a3b_uenv_thinking_max32768_budget16384_full_20260717_211508/uenv_requests.jsonl
/data/ronghao/uenv/uenv-bridge/temp/benchmarks/dscodebench/qwen3_6_35b_a3b_uenv_thinking_max32768_budget16384_full_20260717_211508/uenv_results.jsonl
/data/ronghao/uenv/uenv-bridge/temp/benchmarks/dscodebench/qwen3_6_35b_a3b_uenv_thinking_max32768_budget16384_full_20260717_211508/predictions.jsonl
/data/ronghao/uenv/uenv-bridge/temp/benchmarks/dscodebench/qwen3_6_35b_a3b_uenv_thinking_max32768_budget16384_full_20260717_211508/predictions.csv
/data/ronghao/uenv/uenv-bridge/temp/benchmarks/dscodebench/qwen3_6_35b_a3b_uenv_thinking_max32768_budget16384_full_20260717_211508/metrics.json
```

## 11. UEnv 全量结果

总体指标：

| problem_count | completed_count | failed_count | executed_count | passed_count | error_count | completion_rate | execution_rate | pass@1 | reward_accuracy |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1000 | 1000 | 0 | 267 | 267 | 733 | 1.000 | 0.267 | 0.267 | 0.267 |

分库指标：

| library | problem_count | completion_rate | execution_rate | pass@1 | error_count |
|---|---:|---:|---:|---:|---:|
| keras | 104 | 1.000 | 0.183 | 0.183 | 85 |
| lightgbm | 54 | 1.000 | 0.074 | 0.074 | 50 |
| matplotlib | 105 | 1.000 | 0.333 | 0.333 | 70 |
| numpy | 131 | 1.000 | 0.412 | 0.412 | 77 |
| pandas | 92 | 1.000 | 0.228 | 0.228 | 71 |
| pytorch | 101 | 1.000 | 0.376 | 0.376 | 63 |
| scipy | 112 | 1.000 | 0.321 | 0.321 | 76 |
| seaborn | 83 | 1.000 | 0.157 | 0.157 | 70 |
| sklearn | 108 | 1.000 | 0.306 | 0.306 | 75 |
| tensorflow | 110 | 1.000 | 0.127 | 0.127 | 96 |

失败与异常统计：

| 类型 | 数量 | 说明 |
|---|---:|---|
| test assertion failed / wrong answer | 390 | Worker harness 执行后结果未通过，Adapter 侧记录为 `AssertionError`。 |
| candidate produced no outputs | 229 | 候选代码未产生可比较输出，通常属于语法、运行时错误或未按题目要求返回结果。 |
| numpy array truth value ambiguous | 46 | 代码中直接对 NumPy array 做布尔判断。 |
| evaluation timed out after 300s | 25 | Worker code env 单题执行超时。 |
| LightGBM runtime / warning error | 20 | LightGBM 训练或指标配置相关运行问题。 |
| pandas Series truth value ambiguous | 6 | 代码中直接对 pandas Series 做布尔判断。 |
| ImportError | 4 | 生成代码中存在不兼容导入。 |
| missing python dependency / import | 3 | 少量样本仍触发缺失或不兼容依赖。 |
| other worker error | 10 | 其他 Worker 执行错误。 |

运行耗时统计：

| 指标 | 值 |
|---|---:|
| 单条 Episode `elapsed_ms` 最小值 | 6626 |
| 单条 Episode `elapsed_ms` 最大值 | 349677 |
| 单条 Episode `elapsed_ms` 平均值 | 65050.55 |
| Worker 执行阶段 `execution_time_ms` 最小值 | 52 |
| Worker 执行阶段 `execution_time_ms` 最大值 | 300013 |
| Worker 执行阶段 `execution_time_ms` 平均值 | 32157.13 |

说明：当前 UEnv Worker 的 `inline_harness` wrapper 在 `_result.passed=false` 时会主动抛出 `AssertionError`，因此失败样本顶层 `tests_run` 被记录为 `0`；只有全通过样本顶层 `tests_run=200`。所以本节中的 `execution_rate=0.267` 是当前 UEnv/Worker 返回口径下的“成功执行且通过比例”，不等价于第 7 节直接 vLLM 官方评测中的 `execution_rate=0.877`。

## 12. UEnv 结果结论

本次 UEnv 全量评测完成 1000/1000 条 DSCodeBench 样本，没有 Adapter Core / Server / Worker 调度层面的失败，说明代码生成任务已经能够通过 UEnv 全链路完成请求、模型生成、Worker code env 评测和结果回收。

在当前 `official_fenced + thinking` 配置下，`Qwen/Qwen3.6-35B-A3B` 的 UEnv 链路 `pass@1=0.267`。第 7 节直接 vLLM baseline 的 `pass@1=0.348` 只作为历史参考，不能直接证明 UEnv 链路导致模型能力下降，因为两次实验的 thinking 设置、`MAX_TOKENS`、网关处理和执行位置均不同。

当前更重要的结论是：UEnv 链路完成了 1000/1000 条样本的调度与结果回收，代码生成任务已经能够在 UEnv 中全量运行。后续若希望严格对比“接入 UEnv 是否影响 DSCodeBench 指标”，需要补跑一组同参数实验，例如 `UEnv + thinking 关闭 + MAX_TOKENS=2048`，再与第 7 节直接 vLLM baseline 比较。

另外，Worker 当前把未通过 harness 的样本包装为错误返回，使失败样本的执行细节粒度低于直接官方评测。后续若希望严格对齐官方 `execution_rate`，需要 Worker 在 `_result.passed=false` 时仍返回结构化的 `tests_run`、`tests_passed` 和具体失败原因，而不是只通过 `AssertionError` 结束。
