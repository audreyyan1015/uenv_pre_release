# 五类 Benchmark 任务专用模型候选调研

> 日期：2026-07-25
> 目标：为 UEnv 当前五类 benchmark 各选择 2-3 个更贴近任务领域的模型，作为后续替换基准模型后的对照评测候选。
> 当前基准模型：`Qwen/Qwen3.6-35B-A3B`

## 1. 选择标准

本轮优先选择“任务/领域专用”模型，而不是单纯更大的通用模型。筛选时采用以下标准：

1. 与 benchmark 任务形态相近，例如 PubMedQA 优先医学/生物医学 QA，SciTab 优先表格事实核验，SWE-bench-Pro 优先软件工程 agent 模型。
2. 优先选择公开权重或可复现部署的模型，便于后续在本地 vLLM / OpenAI-compatible endpoint 中替换。
3. 对于不能直接以 chat-completions 方式服务的 encoder/seq2seq 模型，标记为“需要 wrapper”，用于任务专用对照，不作为第一批最小接入目标。
4. 结合当前 8 张 A100 资源，优先选择 7B-32B 或 MoE 小激活量模型；72B 模型作为数学任务强基线候选。

## 2. 总览

| Benchmark | 任务类型 | 首选候选 | 备选候选 | 接入优先级 |
|---|---|---|---|---|
| PubMedQA | 生物医学阅读理解 | `google/medgemma-27b-text-it` | `BioMistral/BioMistral-7B`、`microsoft/BioGPT-Large-PubMedQA` | 先测 MedGemma / BioMistral，BioGPT 需要 wrapper |
| SciTab | 科学表格 claim verification | `RUCKBReasoning/TableLLM-13b` | `microsoft/tapex-large-finetuned-tabfact`、`google/tapas-large-finetuned-tabfact` | 先测 TableLLM，TAPEX/TAPAS 需要 wrapper |
| DSCodeBench | 数据科学代码生成 | `Qwen/Qwen3-Coder-30B-A3B-Instruct` | `Qwen/Qwen2.5-Coder-32B-Instruct`、`deepseek-ai/DeepSeek-Coder-V2-Lite-Instruct` | 三者均适合 vLLM/OpenAI 接入 |
| SWE-bench-Pro | 软件工程程序修复 | `all-hands/openhands-lm-32b-v0.1` | `SWE-bench/SWE-agent-LM-32B`、`Skywork/Skywork-SWE-32B` | 优先 OpenHands LM，因为当前链路就是 OpenHands |
| OlymMATH | 奥赛级数学推理 | `Qwen/Qwen2.5-Math-72B-Instruct` | `deepseek-ai/deepseek-math-7b-rl`、`internlm/internlm2-math-plus-7b` | 先测 Qwen2.5-Math，7B 模型可做低成本对照 |

### 2.1 发布时间与公开成绩

说明：

1. “对应 benchmark 成绩”优先记录与当前五类任务完全同名、同口径的公开成绩。
2. 若没有公开同名 benchmark 成绩，则明确写“未公开”，并只在“相近公开成绩”列列出可参考的近邻任务分数。
3. “发布时间”按模型卡、论文、博客或公告的首次公开时间记录；只有相对时间或无法确认精确日期时，写到年份或月份。

| Benchmark | 候选模型 | 发布时间 | 对应 benchmark 成绩 | 相近公开成绩 / 参考口径 | 结论 |
|---|---|---|---|---|---|
| PubMedQA | `google/medgemma-27b-text-it` | 2025-05-20（MedGemma v1） | PubMedQA accuracy 76.8 | 同一任务，Google MedGemma v1 model card 报告 | 可直接作为 PubMedQA 领域 chat 基线候选 |
| PubMedQA | `BioMistral/BioMistral-7B` | 2024-02（BioMistral 论文） | PubMedQA accuracy 37.6±1.5 | BioMistral-DARE 合并变体在同表中更高，但不是该原始 checkpoint | 原始 BioMistral-7B 的 PubMedQA 分数并不突出，适合作领域预训练对照 |
| PubMedQA | `microsoft/BioGPT-Large-PubMedQA` | 2022-08 / 2022-11（BioGPT 工具与论文） | PubMedQA 81.0 | Microsoft BioGPT 页面报告 BioGPT-Large 在 PubMedQA 上超过此前最佳 78.2 | 分数强，但不是 chat endpoint，需 wrapper |
| SciTab | `RUCKBReasoning/TableLLM-13b` | 2024 年左右（TableLLM 公开模型/论文期） | 未公开 SciTab 成绩 | 主要公开表格 QA、表格操作与结构化数据处理任务结果 | 最小接入风险低，但 SciTab 收益需 UEnv 实测 |
| SciTab | `microsoft/tapex-large-finetuned-tabfact` | 2021（TAPEX 论文/模型） | SciTab 2-class macro-F1 56.06 | SciTab 论文零样本表中 `TAPEX-large (TabFact)`；不含 SciTab 三分类完整口径 | 和表格事实核验最接近，但需要 seq2seq wrapper |
| SciTab | `google/tapas-large-finetuned-tabfact` | 2020（TAPAS 论文/模型） | SciTab 2-class macro-F1 50.30 | SciTab 论文零样本表中 `TAPAS-large (TabFact)`；不含 SciTab 三分类完整口径 | 可做经典表格模型对照，接入成本高于 chat LLM |
| DSCodeBench | `Qwen/Qwen3-Coder-30B-A3B-Instruct` | 2025-07（Qwen3-Coder 系列） | 未公开 DSCodeBench 成绩 | 官方主要公开 agentic coding、SWE、浏览器和通用代码 benchmark | 最值得优先实测的代码专用模型 |
| DSCodeBench | `Qwen/Qwen2.5-Coder-32B-Instruct` | 2024-11（Qwen2.5-Coder 系列） | 未公开 DSCodeBench 成绩 | 公开 HumanEval、MBPP、LiveCodeBench 等代码 benchmark 成绩 | 成熟稳定，适合作为 Qwen3-Coder 的旧一代对照 |
| DSCodeBench | `deepseek-ai/DeepSeek-Coder-V2-Lite-Instruct` | 2024-06（DeepSeek-Coder-V2） | 未公开 DSCodeBench 成绩 | DeepSeek-Coder-V2 系列公开 HumanEval、MBPP、SWE-bench 等结果；Lite checkpoint 的 DSCodeBench 未见公开 | 低成本代码模型对照，需本地复测 |
| SWE-bench-Pro | `all-hands/openhands-lm-32b-v0.1` | 2025-03-31 | 未公开 SWE-bench-Pro 成绩 | SWE-bench Verified resolve rate 37.2%（OpenHands 博客） | 与当前 OpenHands 链路最匹配，但 Pro 仍需 UEnv 实测 |
| SWE-bench-Pro | `SWE-bench/SWE-agent-LM-32B` | 2025-04（SWE-smith / SWE-agent-LM） | 未公开 SWE-bench-Pro 成绩 | SWE-bench Verified pass@1 40.2%（SWE-agent-LM 公开说明） | 更贴近 SWE-agent 工具协议，接入 OpenHands 需实测 |
| SWE-bench-Pro | `Skywork/Skywork-SWE-32B` | 2025 年（Skywork-SWE 模型卡） | 未公开 SWE-bench-Pro 成绩 | SWE-bench Verified pass@1 38.0%，TTS 47.0% | 与 OpenHands 路线接近，适合第二批实测 |
| OlymMATH | `Qwen/Qwen2.5-Math-72B-Instruct` | 2024-09（Qwen2.5-Math 系列） | 未公开 OlymMATH 成绩 | 官方公开 MATH 强结果，例如 TIR/RM@8 口径下 MATH 92.9 | 数学强基线首选，OlymMATH 必须本地实测 |
| OlymMATH | `deepseek-ai/deepseek-math-7b-rl` | 2024-02 | 未公开 OlymMATH 成绩 | DeepSeekMath 论文报告 MATH 51.7（无工具、无投票），self-consistency 可到 60.9 | 低成本数学 RL 对照 |
| OlymMATH | `internlm/internlm2-math-plus-7b` | 2024-05（InternLM2-Math-Plus 系列） | 未公开 OlymMATH 成绩 | 官方主要公开 MATH / OlympiadBench / MathBench 等数学评测，未见 OlymMATH 同口径分数 | 可做中英双语数学对照，需先确认 vLLM/chat template |

## 3. PubMedQA：文本阅读理解

PubMedQA 是生物医学阅读理解任务，模型需要基于 abstract context 判断 `yes/no/maybe`。当前 UEnv 评测链路使用 OpenAI chat 生成最终标签，因此候选模型分为“可直接 chat 接入”和“经典任务模型，需要 wrapper”两类。

| 模型 | 类型 | 推荐理由 | 接入方式 |
|---|---|---|---|
| `google/medgemma-27b-text-it` | 医学文本 instruction model | MedGemma 27B text-only 面向医学文本训练，并针对医学推理优化，适合直接替换当前 chat 模型做 PubMedQA 标签生成。 | 优先直接用 vLLM，保持当前 prompt 和 `yes/no/maybe` 解析。 |
| `BioMistral/BioMistral-7B` | 生物医学领域 LLM | BioMistral 基于 Mistral，在 PubMed Central Open Access 文本上继续预训练，适合验证“领域继续预训练”对 PubMedQA 的收益。 | 可尝试 vLLM；如不是 chat-tuned，需补 instruction prompt 或 chat template。 |
| `microsoft/BioGPT-Large-PubMedQA` | PubMedQA 专门微调模型 | BioGPT 是生物医学生成模型，`BioGPT-Large-PubMedQA` 是针对 PubMedQA 的任务模型，Microsoft 资料中 BioGPT-Large 在 PubMedQA 上达到 81.0%。 | 需要 Transformers wrapper 或单独 inference driver，不建议第一批直接接 OpenAI chat。 |

建议：第一轮先测 `MedGemma-27B` 和 `BioMistral-7B`，它们更接近当前 UEnv 的 chat endpoint 模式；`BioGPT-Large-PubMedQA` 作为任务专用上界或 sanity check，但需要额外封装。

## 4. SciTab：表格理解

SciTab 是科学表格 claim verification，输入为论文表格和 claim，输出 `supports/refutes/not enough info`。SciTab 论文指出该任务对表格 grounding、claim ambiguity 和 compositional reasoning 要求较高，并且多数模型在该 benchmark 上接近随机水平，因此候选模型应覆盖“表格 LLM”和“经典表格事实核验模型”两类。

| 模型 | 类型 | 推荐理由 | 接入方式 |
|---|---|---|---|
| `RUCKBReasoning/TableLLM-13b` | 表格理解/表格操作 LLM | TableLLM 面向表格数据处理与表格操作任务，相比通用 LLM 更贴近 SciTab 的表格输入结构。 | 可优先尝试 vLLM/chat 接入，保持当前 markdown table prompt。 |
| `microsoft/tapex-large-finetuned-tabfact` | 表格事实核验 seq2seq 模型 | TAPEX 有 TabFact fine-tuned checkpoint，Hugging Face model card 明确用于 table fact verification，任务形态与 SciTab 最接近。 | 需要 wrapper，把 SciTab 表格线性化后输出 entail/refute，再映射到 SciTab 三分类。 |
| `google/tapas-large-finetuned-tabfact` | 表格事实核验 encoder 模型 | TAPAS 是经典表格预训练模型，TabFact 版本适合做表格事实核验对照。 | 需要 Transformers wrapper，不适合直接走 chat-completions。 |

建议：第一轮从 `TableLLM-13b` 开始，因为改动最小；如果要做“真正表格事实核验专用模型”对比，再给 TAPEX/TAPAS 写轻量 wrapper。

## 5. DSCodeBench：代码生成

DSCodeBench 是数据科学代码生成 benchmark，包含 1000 条来自 10 个 Python 数据科学库的复杂任务。候选模型应优先选择代码专用、Python/数据科学能力强、支持 vLLM/OpenAI-compatible endpoint 的模型。

| 模型 | 类型 | 推荐理由 | 接入方式 |
|---|---|---|---|
| `Qwen/Qwen3-Coder-30B-A3B-Instruct` | 代码/agentic coding 专用模型 | Qwen3-Coder-30B-A3B-Instruct 强调 agentic coding、长上下文和工具调用能力，规模与当前 Qwen3.6-35B-A3B 接近，适合作为最直接替换。 | 优先 vLLM 接入，保留当前 DSCodeBench prompt 和 Worker inline harness。 |
| `Qwen/Qwen2.5-Coder-32B-Instruct` | 代码专用 instruction model | Qwen2.5-Coder 32B 是成熟代码模型，适合与 Qwen3-Coder 对比“新旧代码专用模型”收益。 | vLLM 接入成熟，工程风险低。 |
| `deepseek-ai/DeepSeek-Coder-V2-Lite-Instruct` | 代码专用 MoE/Lite 模型 | DeepSeek-Coder-V2-Lite-Instruct 提供 vLLM/SGLang 启动示例，激活参数较小，适合做低成本代码模型对照。 | 可直接 vLLM 接入，适合快速全量复测。 |

建议：优先测 `Qwen3-Coder-30B-A3B-Instruct`；如果资源或稳定性有问题，再测 `Qwen2.5-Coder-32B-Instruct` 和 `DeepSeek-Coder-V2-Lite-Instruct`。

## 6. SWE-bench-Pro：测试生成/程序修复

SWE-bench-Pro 是真实仓库级软件工程 agent 任务。当前 UEnv 链路已经接入 OpenHands，因此模型候选不应只看普通代码生成能力，还要看是否经过软件工程 agent 任务训练。

| 模型 | 类型 | 推荐理由 | 接入方式 |
|---|---|---|---|
| `all-hands/openhands-lm-32b-v0.1` | OpenHands 软件工程 agent 模型 | OpenHands LM 明确面向 OpenHands/SWE-bench 场景，model card 报告其在 SWE-Bench Verified 上有 37.2% verified resolve rate，与当前 UEnv 的 OpenHands agent 路线最匹配。 | 首选，替换 OpenHands LLM config 中的 `model/base_url`。 |
| `SWE-bench/SWE-agent-LM-32B` | SWE-agent / SWE-smith 训练模型 | SWE-agent-LM-32B 是面向软件工程任务训练的模型，Hugging Face 页面提供 vLLM 接入说明。 | 可直接 vLLM 接入，但 prompt/工具格式可能更贴近 SWE-agent，需要对 OpenHands 表现做实测。 |
| `Skywork/Skywork-SWE-32B` | SWE 专用 32B 模型 | Skywork-SWE-32B model card 报告其在 SWE-bench Verified 上达到 38.0% pass@1，并基于 OpenHands agent framework 做评测，适合当前 OpenHands 路线。 | 候选优先级高，但需先确认权重、license 和 vLLM 支持情况。 |

建议：第一轮直接测 `all-hands/openhands-lm-32b-v0.1`，因为它与现有 OpenHands driver 的行为假设最接近；第二轮再测 `Skywork-SWE-32B` 或 `SWE-agent-LM-32B`。

## 7. OlymMATH：数学题求解

OlymMATH 是奥赛级数学推理任务，要求模型输出 `\boxed{}` 最终答案。候选模型应优先选择数学专用 instruction/RL 模型，并兼顾中英文数学能力。

| 模型 | 类型 | 推荐理由 | 接入方式 |
|---|---|---|---|
| `Qwen/Qwen2.5-Math-72B-Instruct` | 数学专用 instruction model | Qwen2.5-Math 系列是 Qwen 的数学专用模型，model card 报告 72B Instruct 在 MATH benchmark 上表现强，且支持聊天式使用。 | 8 A100 可优先尝试 TP=8 vLLM，保持当前 OlymMATH prompt。 |
| `deepseek-ai/deepseek-math-7b-rl` | 数学 RL 模型 | DeepSeekMath 7B 通过数学数据继续预训练与 RL 提升数学推理，适合作为低成本数学专用模型对照。 | vLLM/Transformers 均可尝试，若 chat template 不稳定则用 completion-style prompt。 |
| `internlm/internlm2-math-plus-7b` | 中英双语数学模型 | InternLM2-Math-Plus 明确面向 informal math reasoning、code interpreter 和 formal math reasoning，适合 OlymMATH 的中英文子集对照。 | 可用 Transformers/vLLM 尝试，重点观察中文 HARD 子集。 |

建议：如果目标是追求数学基线强度，优先测 `Qwen2.5-Math-72B-Instruct`；如果目标是快速验证 UEnv 链路和低成本对照，优先测 `DeepSeekMath-7B-RL` 与 `InternLM2-Math-Plus-7B`。

## 8. 推荐执行顺序

第一批建议只选择“最少改动、最可能直接跑通”的模型：

| 顺序 | Benchmark | 模型 | 原因 |
|---:|---|---|---|
| 1 | DSCodeBench | `Qwen/Qwen3-Coder-30B-A3B-Instruct` | 代码专用、OpenAI-compatible 接入自然，能最快看到代码任务收益。 |
| 2 | SWE-bench-Pro | `all-hands/openhands-lm-32b-v0.1` | 与当前 OpenHands agent 链路最匹配。 |
| 3 | OlymMATH | `Qwen/Qwen2.5-Math-72B-Instruct` | 数学专用强模型，适合验证 OlymMATH 是否能明显提升。 |
| 4 | PubMedQA | `google/medgemma-27b-text-it` | 医学 instruction model，最接近当前 chat endpoint 形态。 |
| 5 | SciTab | `RUCKBReasoning/TableLLM-13b` | 表格 LLM，可先不写 TAPAS/TAPEX wrapper。 |

第二批再补“任务专用但需要 wrapper”的模型：

| Benchmark | 模型 | 需要补的 adapter 能力 |
|---|---|---|
| PubMedQA | `microsoft/BioGPT-Large-PubMedQA` | Transformers text-generation wrapper，输出映射到 `yes/no/maybe`。 |
| SciTab | `microsoft/tapex-large-finetuned-tabfact`、`google/tapas-large-finetuned-tabfact` | 表格线性化、二分类到三分类映射、非 chat endpoint wrapper。 |

## 9. 接入注意事项

1. 直接 vLLM 接入的模型只需要替换模型路径、served model name、chat template 和必要的 parser 参数。
2. BioGPT、TAPAS、TAPEX 这类非 chat 模型不能直接复用当前 OpenAI chat gateway，需要单独 wrapper 或在 Worker 侧支持特定推理接口。
3. SWE-bench-Pro 的模型替换要同步修改 OpenHands `LLM_CONFIG_PATH` 指向的 config；对 OpenHands LM / SWE-agent LM，需要额外关注工具调用格式是否和当前 OpenHands driver 匹配。
4. 数学模型可能有专用 prompt 格式。OlymMATH 第一轮应保持当前 `\boxed{}` prompt 不变，第二轮再考虑 TIR 或 code-interpreter prompt。
5. 表格模型的评测口径需要特别小心：SciTab 是三分类且包含 `not enough info`，而 TabFact 通常是二分类 entail/refute，不能直接把 TAPAS/TAPEX 结果当成完全同口径。

## 10. 参考来源

- PubMedQA / 医学模型：
  - [`microsoft/BioGPT-Large-PubMedQA`](https://huggingface.co/microsoft/BioGPT-Large-PubMedQA)
  - [Microsoft BioGPT publication page](https://www.microsoft.com/en-us/research/publication/biogpt-generative-pre-trained-transformer-for-biomedical-text-generation-and-mining/)
  - [`BioMistral/BioMistral-7B`](https://huggingface.co/BioMistral/BioMistral-7B)
  - [`google/medgemma-27b-text-it`](https://huggingface.co/google/medgemma-27b-text-it)
- SciTab / 表格模型：
  - [SCITAB EMNLP 2023 paper](https://aclanthology.org/2023.emnlp-main.483/)
  - [`google/tapas-large-finetuned-tabfact`](https://huggingface.co/google/tapas-large-finetuned-tabfact)
  - [`microsoft/tapex-large-finetuned-tabfact`](https://huggingface.co/microsoft/tapex-large-finetuned-tabfact)
  - [`RUCKBReasoning/TableLLM-13b`](https://huggingface.co/RUCKBReasoning/TableLLM-13b)
- DSCodeBench / 代码模型：
  - [DSCodeBench arXiv paper](https://arxiv.org/abs/2505.15621)
  - [`Qwen/Qwen3-Coder-30B-A3B-Instruct`](https://huggingface.co/Qwen/Qwen3-Coder-30B-A3B-Instruct)
  - [`Qwen/Qwen2.5-Coder-32B-Instruct`](https://huggingface.co/Qwen/Qwen2.5-Coder-32B-Instruct)
  - [`deepseek-ai/DeepSeek-Coder-V2-Lite-Instruct`](https://huggingface.co/deepseek-ai/DeepSeek-Coder-V2-Lite-Instruct)
- SWE-bench-Pro / 软件工程 agent 模型：
  - [SWE-Bench Pro public leaderboard](https://labs.scale.com/leaderboard/swe_bench_pro_public)
  - [`all-hands/openhands-lm-32b-v0.1`](https://huggingface.co/all-hands/openhands-lm-32b-v0.1)
  - [OpenHands LM 32B release blog](https://www.openhands.dev/blog/introducing-openhands-lm-32b----a-strong-open-coding-agent-model)
  - [`SWE-bench/SWE-agent-LM-32B`](https://huggingface.co/SWE-bench/SWE-agent-LM-32B)
  - [`Skywork/Skywork-SWE-32B`](https://huggingface.co/Skywork/Skywork-SWE-32B)
- OlymMATH / 数学模型：
  - [`Qwen/Qwen2.5-Math-72B-Instruct`](https://huggingface.co/Qwen/Qwen2.5-Math-72B-Instruct)
  - [`deepseek-ai/deepseek-math-7b-rl`](https://huggingface.co/deepseek-ai/deepseek-math-7b-rl)
  - [`internlm/internlm2-math-plus-7b`](https://huggingface.co/internlm/internlm2-math-plus-7b)
