# 五类 Benchmark 基准模型基准测试

> 日期：2026-07-20
> 阶段：Eval-first，未进行后训练
> 基准模型：`Qwen/Qwen3.6-35B-A3B`
> 测评口径：UEnv 全链路

## 1. 测试进度总览



| 任务书条目 | Benchmark | 数据规模 | 当前状态 | 主指标 | 当前结果 | 结果目录 / 证据 |
|---|---|---|---|---|---|---|
| 1. 文本阅读理解 | PubMedQA | 1000 | UEnv 全量完成 | Accuracy / Macro-F1 | 0.8000 / 0.5912 | `temp/benchmarks/pubmedqa/qwen3_6_35b_a3b_uenv_official_reasoning_fields_max32768_budget16384_full_20260717_111446/` |
| 2. 表格理解 | SciTab | 1224 | UEnv 全量完成 | Accuracy / Macro-F1 | 0.7451 / 0.7340 | `temp/benchmarks/scitab/qwen3_6_35b_a3b_uenv_official_reasoning_fields_max32768_budget16384_full_20260717_121807/` |
| 3. 代码生成 | DSCodeBench | 1000 | UEnv 全量完成 | pass@1 | 0.2670 | `temp/benchmarks/dscodebench/qwen3_6_35b_a3b_uenv_thinking_max32768_budget16384_full_20260717_211508/` |
| 4. 测试生成/程序修复 | SWE-bench-Pro | 731 | UEnv 全量运行中 | resolved / resolve rate | 暂无最终有效分数 | `temp/benchmarks/swebenchpro/qwen3_6_35b_a3b_uenv_full_thinking8192_budget4096_20260719_205350/` |
| 5. 数学题求解 | OlymMATH | 400 | UEnv 全量完成 | UEnv reward accuracy | 0.6175 | `temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_thinking_max32768_budget16384_full_20260718_223005/` |


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
| DSCodeBench | 开启 | 65536 | 32768 | 16384 | 0.0 |
| SWE-bench-Pro | 开启 | 65536 | 8192 | 4096 | 0.0 |
| OlymMATH | 开启 | 65536 | 32768 | 16384 | 0.0 |


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
| executed_count | 267 |
| passed_count | 267 |
| error_count | 733 |
| completion_rate | 1.0000 |
| execution_rate | 0.2670 |
| pass@1 | 0.2670 |
| reward_accuracy | 0.2670 |

分库 pass@1：

| library | 样本数 | pass@1 |
|---|---:|---:|
| numpy | 131 | 0.4122 |
| pandas | 92 | 0.2283 |
| scipy | 112 | 0.3214 |
| sklearn | 108 | 0.3056 |
| matplotlib | 105 | 0.3333 |
| seaborn | 83 | 0.1566 |
| tensorflow | 110 | 0.1273 |
| pytorch | 101 | 0.3762 |
| keras | 104 | 0.1827 |
| lightgbm | 54 | 0.0741 |

结论：UEnv 代码生成链路已完成 1000/1000 条任务调度和结果回收，主指标 `pass@1=0.2670`。当前 Worker 的 `inline_harness` 在失败样本上多以错误方式返回，因此 `execution_rate` 在本轮 UEnv 口径下更接近“成功执行且通过比例”，不等价于直接官方评测中的执行完成率。

### 4.4 SWE-bench-Pro：测试生成/程序修复

SWE-bench-Pro public test split 共 731 条样本，模型需要针对真实仓库生成 patch，并通过官方 fail-to-pass / pass-to-pass 测试得到 `resolved` 结果。

当前状态：

| 项 | 值 |
|---|---:|
| 数据集样本数 | 731 |
| 当前 UEnv request 数 | 66 |
| 当前 UEnv result 数 | 65 |
| completed | 46 |
| failed | 19 |
| 当前 resolved=true | 0 |
| 当前 resolved=false | 65 |

已完成的非 UEnv 侧准备工作：

| 项 | 状态 |
|---|---|
| 数据集下载与字段确认 | 已完成 |
| 直接 vLLM patch 生成 | 已完成 731/731 |
| patch 格式指标 | `nonempty_patch_rate=1.000`，`diff_git_patch_rate=1.000`，`hunk_patch_rate=1.000` |
| 官方 Docker evaluator | 已建立分批按需评测方案，但镜像源、空间和网络仍影响完整 resolved 评测 |
| UEnv OpenHands agent 链路 | 全量运行中，仍需 Worker/Server 侧核验路径错位、环境 catalog、OpenHands 工作目录等问题 |

当前不能把 65 条运行中结果作为最终 resolve rate。已观察到的关键风险包括：部分样例可能创建或修改了与目标项目不一致的路径；部分 request `resolved=false` 需要 Worker 侧结合 OpenHands 轨迹和最终 patch 核验。

### 4.5 OlymMATH：数学题求解

OlymMATH 包含 EN-EASY、EN-HARD、ZH-EASY、ZH-HARD 四个子集，每个子集 100 条，共 400 条。模型需要输出最终数学答案，官方 prompt 要求答案写入 `\boxed{}`。

| 指标 | 值 |
|---|---:|
| 样本数 | 400 |
| requests / results | 400 / 400 |
| completed / failed | 378 / 22 |
| UEnv reward accuracy | 0.6175 |
| completed-only reward accuracy | 0.6534 |
| Parse rate | 0.8950 |
| parsed accuracy | 0.6899 |

按子集：

| 子集 | 样本数 | completed | failed | UEnv reward accuracy | Parse rate |
|---|---:|---:|---:|---:|---:|
| EN-EASY | 100 | 78 | 22 | 0.6300 | 0.7600 |
| EN-HARD | 100 | 100 | 0 | 0.5000 | 0.9600 |
| ZH-EASY | 100 | 100 | 0 | 0.8000 | 0.9500 |
| ZH-HARD | 100 | 100 | 0 | 0.5400 | 0.9100 |

结论：OlymMATH 已完成 400 条 request/result 聚合，整体 `UEnv reward accuracy=0.6175`。22 条 failed 全部集中在 EN-EASY 连续区间，gateway 时间窗口内无 HTTP error，因此后续需要结合 Server/Worker request-level 日志定位 retry 或调度侧原因。

## 5. 当前结论与后续事项

当前五类 benchmark 中，PubMedQA、SciTab、DSCodeBench、OlymMATH 已经完成 UEnv 全量基线；SWE-bench-Pro 已开始全量 UEnv 运行，但尚未得到可作为最终分数的完整 resolved 结果。

从结果看，分类类任务链路最稳定，PubMedQA 和 SciTab 均达到 `completed=全量`、`failed=0`、`parse_rate=1.0`。代码生成任务已经能通过 UEnv 完成全量执行，但失败样本的 Worker 返回粒度仍需与官方口径进一步对齐。数学任务指标可用，但仍有 22 条集中失败需要 Server/Worker 侧排查。SWE-bench-Pro 是当前最复杂链路，后续重点是确认 OpenHands 工作目录、目标仓库路径、最终 patch 和 official resolved 的一致性。

下一步建议：

1. 等待 SWE-bench-Pro 当前 UEnv 全量任务完成，补充最终 `resolved_count / resolve_rate`。
2. 要求 Worker 在代码和 SWE 任务中返回更细粒度的执行信息，例如测试数、失败原因、最终 patch 路径和 OpenHands 轨迹摘要。
3. 对 DSCodeBench 补一组与直接 vLLM 完全同参数的 UEnv 实验，区分模型能力变化和评测链路口径差异。
4. 对 OlymMATH 的 22 条 EN-EASY failed 样本做 request-level 复盘，判断是服务重启、重试上限、调度失败还是样本本身异常。
