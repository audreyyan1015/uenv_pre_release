# SciTab 表格理解基线评测

> 日期：2026-07-09  
> 阶段：Eval-first，未进行后训练  
> 任务书条目：2. 表格理解  
> Benchmark：SciTab  
> 目标模型：`Qwen/Qwen3.6-35B-A3B`

## 1. 任务说明

SciTab 是科学论文表格理解与 claim verification 任务。输入为一张科学论文表格、一条科学 claim 以及表格上下文，模型需要判断 claim 与表格之间的关系：

```text
supports / refutes / not enough info
```

本阶段目标是评估基准模型在该 benchmark 上的零训练表现，不进行 SFT、RL 或其他后训练。

## 2. 数据集准备

已下载 SciTab 官方公开数据：

| 项 | 内容 |
|---|---|
| 数据文件 | `data/benchmarks/scitab/sci_tab.json` |
| 样本数 | 1224 |
| 标签 | `supports`、`refutes`、`not enough info` |
| 标签分布 | supports: 457；refutes: 411；not enough info: 356 |
| 下载源 | SciTab 官方 GitHub 数据，经 jsDelivr 镜像下载 |
| 官方仓库 | https://github.com/XinyuanLu00/SciTab |

当前官方公开文件 `sci_tab.json` 中没有显式 train/dev/test split 字段。因此本阶段将该公开全量数据作为 SciTab benchmark/test set 进行基线评测，并在后续需要和论文或榜单严格对齐时再补充官方提交格式或隐藏测试集口径。

## 3. 评价指标

SciTab 是三分类任务，本次使用以下指标：

| 指标 | 说明 |
|---|---|
| Accuracy | 三分类准确率 |
| Macro-F1 | 对 supports/refutes/not enough info 三类分别计算 F1 后取平均 |
| Parse rate | 模型输出能否解析为合法标签 |
| Parsed accuracy | 仅在可解析样本上的准确率 |
| Per-class Precision / Recall / F1 | 各类别诊断指标 |
| Confusion matrix | 三分类混淆矩阵，另含 unparsed 列 |
| Label / prediction distribution | 标签和预测分布 |

主结果以 `Accuracy` 和 `Macro-F1` 为核心指标，`Parse rate` 用于判断当前 prompt 是否适合后续 RL/RLVR 训练。

## 4. 评测实现

新增评测脚本：

```text
scripts/benchmark/evaluate_scitab.py
```

新增运行脚本：

```text
scripts/benchmark/run_scitab_baseline.sh
```

脚本行为：

1. 如果 SciTab 数据不存在，则下载 `sci_tab.json`。
2. 如果目标模型权重不存在，则通过 ModelScope 下载 `Qwen/Qwen3.6-35B-A3B`。
3. 将表格转换为 Markdown table，并拼接 paper、table caption 和 claim。
4. 使用 vLLM 或 Transformers 进行推理。
5. 生成 `predictions_official.json`、`predictions.jsonl`、`predictions.csv` 和 `metrics.json`。

评测脚本支持两种推理方式：

| 推理方式 | 用法 | 说明 |
|---|---|---|
| `generate` | `INFERENCE_MODE=generate` | 生成式评测，让模型自由生成，再从输出中解析 `supports/refutes/not enough info` |
| `label_logprob` | `INFERENCE_MODE=label_logprob` | 分类式评测，分别计算三个候选标签的条件 log-likelihood，并选择得分最高的标签 |

`label_logprob` 不是让模型自由生成答案，而是把 SciTab 的三个合法标签都作为候选答案进行打分。具体来说，对同一个 prompt 分别拼接 `supports`、`refutes`、`not enough info`，计算候选标签 token 在当前上下文下的平均 log probability，然后选择分数最高的标签作为预测结果。

## 5. 运行命令

正式评测使用 8 张 GPU 和新版 vLLM 推理镜像：

```bash
cd /data/ronghao/uenv/uenv-bridge

IMAGE=localhost/vllm-openai:v0.19.0-cu130 \
MODEL_ID=Qwen/Qwen3.6-35B-A3B \
MODEL_DIR=/data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B \
OUTPUT_DIR=/data/ronghao/uenv/uenv-bridge/temp/benchmarks/scitab/qwen3_6_35b_a3b_vllm_generate_strict \
BACKEND=vllm \
INFERENCE_MODE=generate \
PROMPT_STYLE=strict_label \
MAX_TOKENS=512 \
TENSOR_PARALLEL_SIZE=8 \
MAX_MODEL_LEN=4096 \
GPU_MEMORY_UTILIZATION=0.8 \
./scripts/benchmark/run_scitab_baseline.sh
```

补充的 `label_logprob` 评测命令：

```bash
cd /data/ronghao/uenv/uenv-bridge

IMAGE=localhost/vllm-openai:v0.19.0-cu130 \
MODEL_ID=Qwen/Qwen3.6-35B-A3B \
MODEL_DIR=/data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B \
OUTPUT_DIR=/data/ronghao/uenv/uenv-bridge/temp/benchmarks/scitab/qwen3_6_35b_a3b_vllm_label_logprob \
BACKEND=vllm \
INFERENCE_MODE=label_logprob \
TENSOR_PARALLEL_SIZE=8 \
MAX_MODEL_LEN=4096 \
GPU_MEMORY_UTILIZATION=0.72 \
VLLM_LABEL_BATCH_SIZE=1 \
./scripts/benchmark/run_scitab_baseline.sh
```

说明：SciTab prompt 中包含完整表格，`vLLM + label_logprob` 对长 prompt 计算候选标签 logprob 时显存压力较大，因此稳定配置中将 `VLLM_LABEL_BATCH_SIZE` 降为 1，并将 `GPU_MEMORY_UTILIZATION` 降为 0.72。

## 6. 当前结果

目标模型已在 SciTab 1224 条公开样本上完成基线评测。主结果采用 `vLLM + generate + strict_label`，补充结果保留 `vLLM + label_logprob`。

结果汇总：

| 模型 | 后端 | 推理方式 | 样本数 | Parse rate | Accuracy | Macro-F1 |
|---|---|---|---:|---:|---:|---:|
| `Qwen/Qwen3.6-35B-A3B` | `vLLM 0.19.0` | `generate` | 1224 | 0.9984 | 0.5433 | 0.4992 |
| `Qwen/Qwen3.6-35B-A3B` | `vLLM 0.19.0` | `label_logprob` | 1224 | 1.0000 | 0.2908 | 0.1502 |

`vLLM + generate` 各类别指标：

| 类别 | Precision | Recall | F1 | Support |
|---|---:|---:|---:|---:|
| supports | 0.4543 | 0.9672 | 0.6182 | 457 |
| refutes | 0.9216 | 0.2287 | 0.3665 | 411 |
| not enough info | 0.8776 | 0.3624 | 0.5129 | 356 |

`vLLM + generate` 预测分布：

| 标签 | Gold | Pred |
|---|---:|---:|
| supports | 457 | 973 |
| refutes | 411 | 102 |
| not enough info | 356 | 147 |
| unparsed | 0 | 2 |

`vLLM + generate` 混淆矩阵：

| Gold \ Pred | supports | refutes | not enough info | unparsed |
|---|---:|---:|---:|---:|
| supports | 442 | 1 | 14 | 0 |
| refutes | 313 | 94 | 4 | 0 |
| not enough info | 218 | 7 | 129 | 2 |

`vLLM + generate` 输出文件：

```text
temp/benchmarks/scitab/qwen3_6_35b_a3b_vllm_generate_strict/metrics.json
temp/benchmarks/scitab/qwen3_6_35b_a3b_vllm_generate_strict/predictions_official.json
temp/benchmarks/scitab/qwen3_6_35b_a3b_vllm_generate_strict/predictions.jsonl
temp/benchmarks/scitab/qwen3_6_35b_a3b_vllm_generate_strict/predictions.csv
```

`vLLM + label_logprob` 各类别指标：

| 类别 | Precision | Recall | F1 | Support |
|---|---:|---:|---:|---:|
| supports | 0.0000 | 0.0000 | 0.0000 | 457 |
| refutes | 0.0000 | 0.0000 | 0.0000 | 411 |
| not enough info | 0.2908 | 1.0000 | 0.4506 | 356 |

`vLLM + label_logprob` 预测分布：

| 标签 | Gold | Pred |
|---|---:|---:|
| supports | 457 | 0 |
| refutes | 411 | 0 |
| not enough info | 356 | 1224 |

`vLLM + label_logprob` 输出文件：

```text
temp/benchmarks/scitab/qwen3_6_35b_a3b_vllm_label_logprob/metrics.json
temp/benchmarks/scitab/qwen3_6_35b_a3b_vllm_label_logprob/predictions_official.json
temp/benchmarks/scitab/qwen3_6_35b_a3b_vllm_label_logprob/predictions.jsonl
temp/benchmarks/scitab/qwen3_6_35b_a3b_vllm_label_logprob/predictions.csv
```

## 7. 结果分析

`vLLM + generate` 能稳定产出可解析标签，parse rate 为 99.84%，说明当前 prompt 已基本满足后续 Eval-first 和 RL/RLVR 的格式 gate。主要问题是预测分布明显偏向 `supports`：supports recall 达到 96.72%，但 refutes 和 not enough info 的 recall 分别只有 22.87% 和 36.24%，导致 Macro-F1 低于 Accuracy。

`vLLM + label_logprob` 在 SciTab 上退化为全部预测 `not enough info`，Accuracy 接近该类别占比，Macro-F1 明显偏低。该模式在 PubMedQA 上可以作为三分类基线，但在 SciTab 的长表格 claim verification prompt 下存在候选标签打分偏置，当前不适合作为主评测结果。

## 8. 当前结论

本阶段已经跑通表格理解任务的完整基线评测链路：数据下载、8GPU vLLM 推理、预测文件落盘、指标统计和结果分析均已完成。

当前基准模型在 SciTab 上的主结果为：

```text
Accuracy: 54.33%
Macro-F1: 49.92%
Parse rate: 99.84%
```

后续如果进入训练阶段，建议重点提升 `refutes` 和 `not enough info` 两类的召回率，并在 reward/verifier 设计中加入类别均衡或难例采样，避免模型继续偏向 `supports`。
