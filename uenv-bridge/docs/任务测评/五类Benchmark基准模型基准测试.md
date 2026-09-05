# 五类 Benchmark 基准模型基准测试

> 日期：2026-07-24
> 阶段：Eval-first，未进行后训练
> 基准模型：`Qwen/Qwen3.6-35B-A3B`
> 测评口径：UEnv 全链路

## 1. 测试进度总览



| 任务书条目 | Benchmark | 数据规模 | 当前状态 | 主指标 | 当前结果 | 结果目录 / 证据 |
|---|---|---|---|---|---|---|
| 1. 文本阅读理解 | PubMedQA | 1000 | UEnv 全量完成 | Accuracy / Macro-F1 | 0.8000 / 0.5912 | `temp/benchmarks/pubmedqa/qwen3_6_35b_a3b_uenv_official_reasoning_fields_max32768_budget16384_full_20260717_111446/` |
| 2. 表格理解 | SciTab | 1224 | UEnv 全量完成 | Accuracy / Macro-F1 | 0.7451 / 0.7340 | `temp/benchmarks/scitab/qwen3_6_35b_a3b_uenv_official_reasoning_fields_max32768_budget16384_full_20260717_121807/` |
| 3. 代码生成 | DSCodeBench | 1000 | UEnv 全量完成 | pass@1 | 0.2810 | `temp/benchmarks/dscodebench/qwen3_6_35b_a3b_uenv_thinking_max32768_budget16384_worker_execute_fields_20260720_151535/` |
| 4. 测试生成/程序修复 | SWE-bench-Pro | 731 | UEnv 全量完成 | resolved / resolve rate | 106 / 14.50% | `temp/benchmarks/swebenchpro/qwen3_6_35b_a3b_uenv_full_131k_iter60_budget4096_20260722_214724/` |
| 5. 数学题求解 | OlymMATH | 400 | UEnv 全量完成 | UEnv reward accuracy | 0.6575 | `temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_thinking_max32768_budget16384_full_20260718_223005/` |


## 2. 测评过程

本阶段目标是先评估原始基准模型在五类任务上的零训练表现，不进行 SFT、RL 或其他后训练。测评流程如下：

```text
Benchmark 数据集
  -> Adapter driver 构造 EpisodeRequest
  -> Adapter Core / Server
  -> Worker 按 env_type 和 dataset 路由到对应任务逻辑
  -> Worker 访问 Adapter Model Gateway
  -> Gateway 转发到本机 vLLM OpenAI-compatible endpoint
  -> Worker 执行解析、判分、代码运行或 agent 任务
  -> EpisodeResult 返回 Adapter
  -> Adapter driver 汇总 metrics.json / predictions / request-result 日志
```

各任务的 Worker 路由方式：

| Benchmark | UEnv env / plugin | Worker 侧主要职责 |
|---|---|---|
| PubMedQA | `env_type=math`，`env_config.dataset=pubmedqa` | 解析 `yes/no/maybe`，计算三分类 reward。 |
| SciTab | `env_type=math`，`env_config.dataset=scitab` | 解析 `supports/refutes/not enough info`，计算 claim verification reward。 |
| DSCodeBench | code env | 调用模型生成代码，执行 DSCodeBench harness，返回 pass/fail 和错误信息。 |
| SWE-bench-Pro | `env_type=swe`，OpenHands agent route | 创建目标仓库环境，运行 OpenHands，生成 patch，执行官方测试并返回 resolved。 |
| OlymMATH | `env_type=math`，`env_config.dataset=olymmath-*` | 抽取最终数学答案，使用 OlymMATH backend 判分。 |

## 3. 统一配置

### 3.1 通用配置

| 配置项 | 值 |
|---|---|
| 基准模型 | `Qwen/Qwen3.6-35B-A3B` |
| 模型路径 | `/data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B` |
| 推理服务 | vLLM OpenAI-compatible server |
| 推理镜像 | `localhost/vllm-openai:v0.19.0-cu130` |
| GPU | 8 张 A100 |
| Tensor parallel | 8 |
| vLLM reasoning parser | `qwen3` |
| Adapter Core endpoint | `8.130.75.157:8088` |
| UEnv batch size | 1 |
| 后训练 | 未进行 SFT/RL，Eval-first 基线 |

### 3.2 局部配置

各任务的 gateway 与 reasoning 处理方式：

| Benchmark | Thinking | MAX_MODEL_LEN | MAX_TOKENS | THINKING_TOKEN_BUDGET | TEMPERATURE |
|---|---|---:|---:|---:|---:|
| PubMedQA | 开启 | 65536 | 32768 | 16384 | 0.0 |
| SciTab | 开启 | 65536 | 32768 | 16384 | 0.0 |
| DSCodeBench | 开启 | 65536 | 32768 | 16384 | 0.2 |
| SWE-bench-Pro | 开启 | 131072 | 8192 | 4096 | 0.0 |
| OlymMATH | 开启 | 65536 | 32768 | 16384 | 0.0 |

### 3.3 Checkpoint 评测方式

五类 UEnv benchmark 入口脚本均支持两种模型来源：

| 模型来源 | 使用方式 | 说明 |
|---|---|---|
| 已有模型服务 | 设置 `UENV_ROLLOUT_MODEL_ENDPOINT` 或 `MODEL_ENDPOINT` | 复用已经启动好的 OpenAI-compatible `/v1` endpoint。 |
| 训练 checkpoint / HF 目录 | 设置 `CHECKPOINT_DIR`、`HF_DIR` 或 `MODEL_DIR` | 脚本会启动本地 vLLM 和 Adapter Model Gateway，再把生成的 endpoint 传给评测 driver。 |

`CHECKPOINT_DIR` 可以指向 VeRL 的 `global_step_xxx` 目录，也可以直接指向其中的 `actor` 目录。若还没有 HuggingFace 权重，脚本会先调用 VeRL FSDP merger，默认输出到 `actor/huggingface/`；若已经有可加载的 HF 权重，可以直接设置 `HF_DIR` 或 `MODEL_DIR` 跳过合并。

通用入口示例：

```bash
cd /data/ronghao/uenv/uenv-bridge

CHECKPOINT_DIR=/data/ronghao/uenv/uenv-bridge/checkpoints/uenv_grpo/<run_id>/global_step_xxx \
OUTPUT_DIR=/data/ronghao/uenv/uenv-bridge/temp/benchmarks/<benchmark>/<run_id> \
LIMIT=100 \
./scripts/benchmark/<benchmark>/run_<benchmark>_uenv_baseline.sh
```

可覆盖的关键变量：

| 变量 | 说明 |
|---|---|
| `CHECKPOINT_DIR` | VeRL checkpoint 目录，支持 `global_step_xxx` 或 `global_step_xxx/actor`。 |
| `HF_DIR` | 已合并或待输出的 HuggingFace 目录。 |
| `MODEL_DIR` | 已可直接被 vLLM 加载的模型目录。 |
| `SKIP_MERGE=1` | 跳过 checkpoint merge，直接使用 `HF_DIR`。 |
| `KEEP_SERVE=1` | 评测结束后保留 vLLM 和 model gateway。 |
| `VLLM_PORT` / `GATEWAY_PORT` | 本机 vLLM 与 Adapter Model Gateway 端口。 |
| `MODEL_GATEWAY_PUBLIC_URL` | Worker / Agent 实际访问的 gateway 地址。 |

SWE-bench-Pro 仍有一个额外约束：OpenHands Agent 侧的 `LLM_CONFIG_PATH` 或本地隧道必须指向同一个 Adapter Model Gateway。脚本只负责在 Adapter 侧启动模型与 gateway，不会自动修改远端 Agent 配置文件。


下面给出 Adapter 当前实际放入请求的 prompt 模板。`{...}` 表示每条样本动态填充的数据字段。

PubMedQA system prompt：

```text
You are answering PubMedQA biomedical reading comprehension questions.
```

PubMedQA user prompt：

```text
Read the abstract context and answer the biomedical question with exactly one label: yes, no, or maybe.

Context:
[1] {context_1}
[2] {context_2}
...

Question: {question}

Return only one word: yes, no, or maybe.
```

SciTab system prompt：

```text
You are a scientific table claim verification classifier.
```

SciTab user prompt：

```text
Given a scientific paper table and a claim, choose exactly one label: supports, refutes, or not enough info.

Paper: {paper}
Table caption: {table_caption}
Table:
| {column_1} | {column_2} | ... |
| --- | --- | ... |
| {row_1_value_1} | {row_1_value_2} | ... |
...

Claim: {claim}

Return only one label: supports, refutes, or not enough info.
```

DSCodeBench system prompt：

```text
You are a careful Python data science coding assistant.
```

DSCodeBench user prompt：

````text
Please generate Python3 solution for the following code problem description:

# Code problem description #
{code_problem}

# Response #
Do not generate additional code, such as "__main__" block. Return only one Python markdown code block containing the solution code.
Solution:
```python
````

OlymMATH 英文 system prompt：

```text
You are a careful mathematical problem solver.
```

OlymMATH 英文 user prompt：

```text
Please reason step by step, and put your final answer within \boxed{}.

{problem}
```

OlymMATH 中文 system prompt：

```text
你是一个严谨的数学题求解助手。
```

OlymMATH 中文 user prompt：

```text
请逐步推理，并在 \boxed{} 内给出您的最终答案。

{problem}
```

SWE-bench-Pro 当前 UEnv 运行走 OpenHands agent 路线。Adapter request 中不直接放完整 issue prompt，而是放 `instance_id`、`repo`、`base_commit`、`driver_entrypoint=run_swebenchpro_official.py` 和 `llm_config_path`；OpenHands driver 读取实例 catalog 后，向 agent 发送下面的任务指令。

SWE-bench-Pro user instruction：

```text
The git repository is already checked out at `{repo_path}`.
All investigation and edits must stay under `{repo_path}`.
Start by confirming the workspace:
1. `pwd`
2. `git -C {repo_path} rev-parse --show-toplevel`
3. `ls -la {repo_path}`

Inspect the repository structure and identify the relevant language/framework before searching.
This instance is labeled as `{repo_language}`; prioritize files matching the repository language.
Use targeted searches with `rg` for symbols, error messages, routes, tests, or issue keywords.
When relevant, also inspect non-test project files such as JSON, YAML, templates, and generated schemas.
Do not search or edit outside `{repo_path}`. Do not inspect `/opt/openhands`, benchmark harness directories, `/tmp`, or `/root` unless explicitly required by a tool.

<issue_description>
{problem_statement}
</issue_description>

Implement the minimal fix in non-test project files required by the issue. Tests are already provided by the benchmark; do not modify tests unless the issue explicitly requires it.
Before finishing, inspect `git diff` and make sure the patch is focused.
Use terminal and file_editor tools. When done, call the finish tool.
```

## 4. 测评结果

### 4.1 PubMedQA：文本阅读理解

PubMedQA 输入为生物医学 abstract 上下文和研究问题，模型需要输出 `yes`、`no` 或 `maybe`。本轮使用 expert-labeled 1000 条样本作为全量基线验证集。

| 指标 | 值 |
|---|---:|
| 样本数 | 1000 |
| completed / failed | 1000 / 0 |
| Parse rate | 1.0000 |
| Accuracy | 0.8000 |
| Macro-F1 | 0.5912 |
| reward accuracy | 0.8000 |

类别表现：

| 类别 | Precision | Recall | F1 | Support |
|---|---:|---:|---:|---:|
| yes | 0.8158 | 0.9149 | 0.8625 | 552 |
| no | 0.8192 | 0.8580 | 0.8382 | 338 |
| maybe | 0.1852 | 0.0455 | 0.0730 | 110 |

结论：链路层面 1000 条全量闭合，Adapter 解析结果与 Worker reward 完全一致。主要短板是 `maybe` 类召回率较低，模型倾向预测 `yes` 或 `no`。

### 4.2 SciTab：表格理解

SciTab 输入为科学论文表格、上下文和 claim，模型需要判断 `supports`、`refutes` 或 `not enough info`。当前公开数据没有显式 split 字段，本轮使用公开全量 1224 条样本。

| 指标 | 值 |
|---|---:|
| 样本数 | 1224 |
| completed / failed | 1224 / 0 |
| Parse rate | 1.0000 |
| Accuracy | 0.7451 |
| Macro-F1 | 0.7340 |
| reward accuracy | 0.7451 |

类别表现：

| 类别 | Precision | Recall | F1 | Support |
|---|---:|---:|---:|---:|
| supports | 0.7028 | 0.8796 | 0.7813 | 457 |
| refutes | 0.7640 | 0.7640 | 0.7640 | 411 |
| not enough info | 0.8133 | 0.5506 | 0.6566 | 356 |

结论：SciTab UEnv 全量链路稳定，`completed=1224`、`failed=0`。模型对 `supports` 召回较高，但对 `not enough info` 仍偏保守，部分信息不足样本被预测为支持或反驳。

### 4.3 DSCodeBench：代码生成

DSCodeBench 共 1000 条真实数据科学代码生成任务，覆盖 10 个 Python 数据科学库。本轮 UEnv 使用 `inline_harness` 方式，由 Worker code env 执行每题 200 个测试用例。

| 指标 | 值 |
|---|---:|
| problem_count | 1000 |
| completed / failed | 1000 / 0 |
| executed_count | 892 |
| passed_count | 281 |
| error_count | 329 |
| wrong_answer_count | 390 |
| completion_rate | 1.0000 |
| execution_rate | 0.8920 |
| pass@1 | 0.2810 |
| reward_accuracy | 0.2810 |

分库 pass@1：

| library | 样本数 | pass@1 |
|---|---:|---:|
| numpy | 131 | 0.3969 |
| pandas | 92 | 0.3152 |
| scipy | 112 | 0.3036 |
| sklearn | 108 | 0.3611 |
| matplotlib | 105 | 0.3238 |
| seaborn | 83 | 0.1687 |
| tensorflow | 110 | 0.1273 |
| pytorch | 101 | 0.3960 |
| keras | 104 | 0.2019 |
| lightgbm | 54 | 0.0741 |

结论：UEnv 代码生成链路已完成 1000/1000 条任务调度和结果回收，主指标 `pass@1=0.2810`。Worker 新返回口径已经能回填 `tests_run`、`tests_passed` 和 `error_category`；因此本轮 `execution_rate=0.8920` 表示实际运行测试的比例，失败样本可进一步拆分为 `wrong_answer=390`、`candidate_runtime_error=223`、`harness_error=74`、`dependency_error=9` 和 `timeout=23`。

### 4.4 SWE-bench-Pro：测试生成/程序修复

SWE-bench-Pro public test split 共 731 条样本，模型需要针对真实仓库生成 patch，并通过官方 fail-to-pass / pass-to-pass 测试得到 `resolved` 结果。本轮走 UEnv + Worker SWE 环境 + OpenHands Agent，全量 731 条样本已完成。

| 项 | 值 |
|---|---:|
| 数据集样本数 | 731 |
| UEnv requests / results | 731 / 731 |
| completed / failed | 675 / 56 |
| resolved=true | 106 |
| resolved=false | 625 |
| resolve rate | 14.50% |
| completed 内 resolve rate | 15.70% |
| 全量运行总耗时 | 35h29m27s |
| 平均 episode 耗时 | 174.78s |

失败类型：

| 类型 | 数量 | 说明 |
|---|---:|---|
| patch 应用失败 | 45 | 全部集中在 `protonmail/webclients` |
| timeout | 8 | 全部集中在 `gravitational/teleport` |
| `ContextWindowExceededError` | 3 | `gravitational/teleport`、`future-architect/vuls`、`element-hq/element-web` 各 1 条 |

按语言：

| 语言 | 样本数 | completed | failed | resolved | resolve rate |
|---|---:|---:|---:|---:|---:|
| Go | 280 | 270 | 10 | 0 | 0.00% |
| Python | 266 | 266 | 0 | 94 | 35.34% |
| JavaScript | 165 | 119 | 46 | 12 | 7.27% |
| TypeScript | 20 | 20 | 0 | 0 | 0.00% |

按仓库表现：

| 仓库 | 样本数 | completed | failed | resolved | resolve rate |
|---|---:|---:|---:|---:|---:|
| `internetarchive/openlibrary` | 91 | 91 | 0 | 57 | 62.64% |
| `NodeBB/NodeBB` | 44 | 44 | 0 | 12 | 27.27% |
| `qutebrowser/qutebrowser` | 79 | 79 | 0 | 17 | 21.52% |
| `ansible/ansible` | 96 | 96 | 0 | 20 | 20.83% |
| `gravitational/teleport` | 76 | 67 | 9 | 0 | 0.00% |
| `protonmail/webclients` | 65 | 20 | 45 | 0 | 0.00% |
| `future-architect/vuls` | 62 | 61 | 1 | 0 | 0.00% |
| `navidrome/navidrome` | 57 | 57 | 0 | 0 | 0.00% |
| `element-hq/element-web` | 56 | 55 | 1 | 0 | 0.00% |
| `flipt-io/flipt` | 85 | 85 | 0 | 0 | 0.00% |
| `tutao/tutanota` | 20 | 20 | 0 | 0 | 0.00% |

Gateway 侧观测到 25292 次 `/v1/chat/completions` 请求，平均延迟 3034.25 ms，P90 延迟 5001.33 ms，最大延迟 50609.04 ms。3 次 gateway HTTP 400 均对应上下文窗口超限。

结论：SWE-bench-Pro 已经形成 UEnv 全量基线，主指标为 `106/731=14.50%`。相较早期 0 resolved 的中间状态，本轮已经能证明 OpenHands + UEnv + gateway 链路可以产生有效 resolved 样本；剩余问题主要集中在仓库级环境/patch 应用、长任务超时和少量上下文边界。

### 4.5 OlymMATH：数学题求解

OlymMATH 包含 EN-EASY、EN-HARD、ZH-EASY、ZH-HARD 四个子集，每个子集 100 条，共 400 条。模型需要输出最终数学答案，官方 prompt 要求答案写入 `\boxed{}`。

| 指标 | 值 |
|---|---:|
| 样本数 | 400 |
| requests / results | 400 / 400 |
| completed / failed | 400 / 0 |
| UEnv reward accuracy | 0.6575 |
| completed-only reward accuracy | 0.6575 |
| Parse rate | 0.9500 |
| parsed accuracy | 0.6921 |

按子集：

| 子集 | 样本数 | completed | failed | UEnv reward accuracy | Parse rate |
|---|---:|---:|---:|---:|---:|
| EN-EASY | 100 | 100 | 0 | 0.7900 | 0.9800 |
| EN-HARD | 100 | 100 | 0 | 0.5000 | 0.9600 |
| ZH-EASY | 100 | 100 | 0 | 0.8000 | 0.9500 |
| ZH-HARD | 100 | 100 | 0 | 0.5400 | 0.9100 |

结论：OlymMATH 已完成 400 条 request/result 聚合，初始 22 条 EN-EASY failed 样本经 `RESUME=1` 补测后全部恢复为 `completed`。按 `qid` 取最新结果后，整体 `UEnv reward accuracy=0.6575`，parse rate 为 0.9500。

## 5. 当前结论与后续事项

当前五类 benchmark 均已完成 UEnv 全量基线。分类、表格、代码生成、数学求解和 SWE 程序修复都已经通过 Adapter -> Adapter Core / Server -> Worker -> Adapter Model Gateway -> vLLM -> Worker 判分 -> Adapter 汇总的链路闭合。

从结果看，分类类任务链路最稳定，PubMedQA 和 SciTab 均达到 `completed=全量`、`failed=0`、`parse_rate=1.0`。代码生成任务已经能通过 UEnv 完成全量执行，且 Worker 新返回口径已经支持基于 `tests_run/tests_passed/error_category` 的失败归因。数学任务在失败样本补测后达到 `completed=400`、`failed=0`，可以作为正式基线。SWE-bench-Pro 是当前最复杂链路，但已经完成 731 条全量运行，并得到 `resolve_rate=14.50%` 的初始基线。

下一步建议：

1. 要求 Worker 在 SWE 任务中继续返回更细粒度的执行信息，例如测试数、失败原因、最终 patch 路径和 OpenHands 轨迹摘要，优先排查 `protonmail/webclients` 的 patch 应用失败和 `gravitational/teleport` 的 timeout。
2. 对 SWE-bench-Pro 的 Go / TypeScript 0 resolved 现象做仓库级和语言级复盘，区分模型能力、OpenHands 策略和环境执行问题。
3. 基于 DSCodeBench 的 `error_category` 统计，继续分析代码生成失败主要来自模型答案错误、运行时错误还是环境依赖问题。
4. 保留 OlymMATH 的失败样本补测日志，后续如需做稳定性复盘，可对比初始 failed 与 resume completed 的 Server/Worker request-level 日志。
