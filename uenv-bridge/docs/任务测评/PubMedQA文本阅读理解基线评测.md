# PubMedQA 文本阅读理解基线评测

> 日期：2026-07-07  
> 阶段：Eval-first，未进行后训练  
> 任务书条目：1. 文本阅读理解  
> Benchmark：PubMedQA  
> 目标模型：`Qwen/Qwen3.6-35B-A3B`

## 1. 任务说明

PubMedQA 是生物医学文本阅读理解任务。输入为 PubMed abstract 上下文和一个研究问题，模型需要输出三分类答案：

```text
yes / no / maybe
```

本阶段目标是评估基准模型在该 benchmark 上的零训练表现，不进行 SFT、RL 或其他后训练。

## 2. 数据集准备

已下载官方 PubMedQA expert-labeled 数据：

| 项 | 内容 |
|---|---|
| 数据文件 | `data/benchmarks/pubmedqa/ori_pqal.json` |
| 样本数 | 1000 |
| 标签 | `yes`、`no`、`maybe` |
| 标签分布 | yes: 552；no: 338；maybe: 110 |
| 下载源 | PubMedQA 官方 GitHub 数据，经 jsDelivr 镜像下载 |
| 官方仓库 | https://github.com/pubmedqa/pubmedqa |
| 官方主页 | https://pubmedqa.github.io/ |

当前评测脚本默认使用 1000 条 expert-labeled 样本，作为本阶段基线验证集。

## 3. 评价指标

PubMedQA 官方评测脚本 `evaluation.py` 主要输出：

| 指标 | 说明 |
|---|---|
| Accuracy | 三分类准确率 |
| Macro-F1 | 对 yes/no/maybe 三类分别计算 F1 后取平均 |

官方评测要求预测文件为 JSON，key 是 PMID，value 是 `yes`、`no` 或 `maybe`；本地脚本额外保存 jsonl/csv，便于排查单条样本输出。

为了分析模型输出质量，脚本还会额外记录：

| 指标 | 说明 |
|---|---|
| Parse rate | 输出能否解析为 yes/no/maybe |
| Parsed accuracy | 仅在可解析样本上的准确率 |
| Per-class Precision / Recall / F1 | 各类别诊断指标 |
| Confusion matrix | 三分类混淆矩阵，另含 unparsed 列 |
| Label / prediction distribution | 标签和预测分布 |

## 4. 评测实现

新增评测脚本：

```text
scripts/benchmark/evaluate_pubmedqa.py
```

新增运行脚本：

```text
scripts/benchmark/run_pubmedqa_baseline.sh
```

运行方式：

```bash
cd /data/ronghao/uenv/uenv-bridge

IMAGE=localhost/vllm-openai:v0.19.0-cu130 \
MODEL_ID=Qwen/Qwen3.6-35B-A3B \
MODEL_DIR=/data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B \
BACKEND=vllm \
INFERENCE_MODE=label_logprob \
TENSOR_PARALLEL_SIZE=8 \
MAX_MODEL_LEN=4096 \
VLLM_LABEL_BATCH_SIZE=64 \
./scripts/benchmark/run_pubmedqa_baseline.sh
```

脚本行为：

1. 如果 PubMedQA 数据不存在，则下载 `ori_pqal.json`。
2. 如果目标模型权重不存在，则通过 ModelScope 下载 `Qwen/Qwen3.6-35B-A3B`。
3. 按配置使用 vLLM 或 Transformers 进行推理。
4. 默认使用 tokenizer chat template 构造 instruct/chat prompt，要求模型只输出 `yes`、`no` 或 `maybe`。
5. 生成 `predictions_official.json`、`predictions.jsonl`、`predictions.csv` 和 `metrics.json`。

评测脚本支持两个推理后端：

| 后端 | 用法 | 说明 |
|---|---|---|
| `vllm` | `BACKEND=vllm` | 默认后端，适合高吞吐评测；目标 Qwen3.6 需要新版 vLLM |
| `transformers` | `BACKEND=transformers` | 备用后端，用于当前 vLLM 不支持目标模型时继续评测 |

评测脚本支持两种推理方式：

| 推理方式 | 用法 | 说明 |
|---|---|---|
| `generate` | `INFERENCE_MODE=generate` | 生成式评测，会解析模型输出中的 `yes/no/maybe`；Qwen3.6 正式评测使用 `PROMPT_STYLE=strict_label`、`MAX_TOKENS=512`，不使用换行 stop 截断输出 |
| `label_logprob` | `INFERENCE_MODE=label_logprob` | 分类式评测，分别计算 `yes/no/maybe` 三个候选答案的条件 log-likelihood，并选择得分最高的标签 |

`label_logprob` 不是让模型自由生成答案，而是把 PubMedQA 的三个合法标签都作为候选答案进行打分。

具体来说，对同一个 prompt 分别拼接 `yes`、`no`、`maybe`，计算每个候选标签 token 在当前上下文下的平均 log probability，然后选择分数最高的标签作为预测结果。

因此该模式的输出天然可解析，适合三分类 benchmark；但它衡量的是模型对固定候选标签的偏好，不等同于真实生成场景下模型最终会写出的答案。

默认输出目录：

```text
temp/benchmarks/pubmedqa/qwen3_6_35b_a3b
```

## 5. 当前执行状态

已完成：

| 项 | 状态 |
|---|---|
| PubMedQA 数据下载 | 已完成 |
| 评测指标确认 | 已完成 |
| 评测脚本编写 | 已完成 |
| vLLM 镜像准备 | 已完成，从镜像站拉取 `docker.1ms.run/vllm/vllm-openai:v0.19.0-cu130`，并标记为 `localhost/vllm-openai:v0.19.0-cu130` |
| 容器环境确认 | 已完成，正式评测镜像内为 vLLM 0.19.0 / torch 2.10.0+cu130 / transformers 4.57.6 |
| GPU 可用性确认 | 已完成，8 张 A100 80GB 可用 |
| 目标模型完整权重下载 | 已完成，26 个 safetensors shard 已落地 |
| 评测链路 smoke test | 已完成，使用本地小模型验证脚本可以产出 predictions 和 metrics |
| 目标模型 vLLM smoke test | 已完成，`vLLM + label_logprob` 在 3 条样本上跑通 |
| 目标模型全量评测 | 已完成，使用 `vLLM + label_logprob` 和 `vLLM + generate` 跑完 1000 条 expert-labeled 样本 |

当前本地模型目录已完整：

```text
/data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B
```

权重大小：

```text
26 个 safetensors shard，共 71,903,776,776 bytes，约 67GiB
```

兼容性验证结果：

```text
正式评测镜像 vLLM: 0.19.0
正式评测镜像 torch: 2.10.0+cu130
正式评测镜像 transformers: 4.57.6
GPU 可见数量: 8
vLLM supported archs: 包含 Qwen3_5MoeForConditionalGeneration
Qwen3.6 README 推荐: vllm>=0.19.0 或 sglang>=0.5.10
```

说明：项目原有 `localhost/uenv-bridge-verl:layer4-build` 镜像内为 vLLM 0.11.0，不能识别 Qwen3.6 对应的 `qwen3_5_moe` 架构，因此本次 benchmark 单独使用新版 vLLM 推理镜像完成评测。

## 6. 当前结果

目标模型已在 PubMedQA 1000 条 expert-labeled 样本上完成基线评测。当前保留两类 vLLM 结果：`label_logprob` 分类式评测和 `generate` 生成式评测。`label_logprob` 分别计算 `yes`、`no`、`maybe` 三个候选标签在 prompt 后的条件 log-likelihood，并选择得分最高的标签；`generate` 则让模型自由生成，再从输出中解析最终标签。

正式结果汇总：

| 模型 | 后端 | 推理方式 | 样本数 | Parse rate | Accuracy | Macro-F1 |
|---|---|---|---:|---:|---:|---:|
| `Qwen/Qwen3.6-35B-A3B` | `vLLM 0.19.0` | `label_logprob` | 1000 | 1.0000 | 0.6780 | 0.4749 |
| `Qwen/Qwen3.6-35B-A3B` | `vLLM 0.19.0` | `generate` | 1000 | 1.0000 | 0.7980 | 0.6202 |

`vLLM + label_logprob` 各类别指标：

| 类别 | Precision | Recall | F1 | Support |
|---|---:|---:|---:|---:|
| yes | 0.7575 | 0.7355 | 0.7463 | 552 |
| no | 0.5862 | 0.8047 | 0.6783 | 338 |
| maybe | 0.0000 | 0.0000 | 0.0000 | 110 |

预测分布：

| 标签 | Gold | Pred |
|---|---:|---:|
| yes | 552 | 536 |
| no | 338 | 464 |
| maybe | 110 | 0 |

混淆矩阵：

| Gold \\ Pred | yes | no | maybe | unparsed |
|---|---:|---:|---:|---:|
| yes | 406 | 146 | 0 | 0 |
| no | 66 | 272 | 0 | 0 |
| maybe | 64 | 46 | 0 | 0 |

`vLLM + label_logprob` 输出文件：

```text
temp/benchmarks/pubmedqa/qwen3_6_35b_a3b_vllm_label_logprob/metrics.json
temp/benchmarks/pubmedqa/qwen3_6_35b_a3b_vllm_label_logprob/predictions_official.json
temp/benchmarks/pubmedqa/qwen3_6_35b_a3b_vllm_label_logprob/predictions.jsonl
temp/benchmarks/pubmedqa/qwen3_6_35b_a3b_vllm_label_logprob/predictions.csv
```

`vLLM + generate` 使用严格标签 prompt，并将 `MAX_TOKENS` 设为 512，避免 Qwen3.6 thinking 输出被过早截断。该模式下模型仍可能先生成推理文本，但最终输出可以稳定解析为 `yes/no/maybe`。

`vLLM + generate` 各类别指标：

| 类别 | Precision | Recall | F1 | Support |
|---|---:|---:|---:|---:|
| yes | 0.8404 | 0.8967 | 0.8677 | 552 |
| no | 0.8101 | 0.8580 | 0.8333 | 338 |
| maybe | 0.2453 | 0.1182 | 0.1595 | 110 |

`vLLM + generate` 预测分布：

| 标签 | Gold | Pred |
|---|---:|---:|
| yes | 552 | 589 |
| no | 338 | 358 |
| maybe | 110 | 53 |

`vLLM + generate` 混淆矩阵：

| Gold \\ Pred | yes | no | maybe | unparsed |
|---|---:|---:|---:|---:|
| yes | 495 | 34 | 23 | 0 |
| no | 31 | 290 | 17 | 0 |
| maybe | 63 | 34 | 13 | 0 |

`vLLM + generate` 输出文件：

```text
temp/benchmarks/pubmedqa/qwen3_6_35b_a3b_vllm_generate_strict/metrics.json
temp/benchmarks/pubmedqa/qwen3_6_35b_a3b_vllm_generate_strict/predictions_official.json
temp/benchmarks/pubmedqa/qwen3_6_35b_a3b_vllm_generate_strict/predictions.jsonl
temp/benchmarks/pubmedqa/qwen3_6_35b_a3b_vllm_generate_strict/predictions.csv
```

对照结果：

| 模型 | 后端 | 推理方式 | 样本数 | Parse rate | Accuracy | Macro-F1 | 说明 |
|---|---|---|---:|---:|---:|---:|---|
| `Qwen/Qwen3.6-35B-A3B` | `transformers` | `label_logprob` | 1000 | 1.0000 | 0.6980 | 0.4885 | vLLM 镜像准备前的备用评测结果 |

对照结果输出目录：

```text
temp/benchmarks/pubmedqa/qwen3_6_35b_a3b_label_logprob/
```

为了验证评测链路，已用本地已有小模型执行 8 条样本 smoke test：

```bash
cd /data/ronghao/uenv/uenv-bridge

podman run --rm \
  --entrypoint bash \
  --network host \
  --device nvidia.com/gpu=0 \
  -v /data/ronghao:/data/ronghao \
  -w /data/ronghao/uenv/uenv-bridge \
  localhost/uenv-bridge-verl:layer4-build \
  -lc 'python3 scripts/benchmark/evaluate_pubmedqa.py \
    --data data/benchmarks/pubmedqa/ori_pqal.json \
    --model /data/ronghao/models/modelscope/Qwen/Qwen2___5-0___5B-Instruct \
    --output-dir temp/benchmarks/pubmedqa/qwen2_5_0_5b_smoke \
    --limit 8 \
    --tensor-parallel-size 1 \
    --max-model-len 2048 \
    --gpu-memory-utilization 0.5 \
    --enforce-eager'
```

Smoke test 输出：

| 模型 | 样本数 | Parse rate | Accuracy | Macro-F1 | 说明 |
|---|---:|---:|---:|---:|---|
| `Qwen2.5-0.5B-Instruct` | 8 | 1.0000 | 0.1250 | 0.0741 | 仅用于验证评测脚本，不作为任务书目标模型结果 |

输出文件：

```text
temp/benchmarks/pubmedqa/qwen2_5_0_5b_smoke/metrics.json
temp/benchmarks/pubmedqa/qwen2_5_0_5b_smoke/predictions_official.json
temp/benchmarks/pubmedqa/qwen2_5_0_5b_smoke/predictions.jsonl
temp/benchmarks/pubmedqa/qwen2_5_0_5b_smoke/predictions.csv
```

同时使用 `transformers` 后端执行了 3 条样本 smoke test，验证备用后端可用：

| 模型 | 后端 | 样本数 | Parse rate | Accuracy | Macro-F1 | 说明 |
|---|---|---:|---:|---:|---:|---|
| `Qwen2.5-0.5B-Instruct` | `transformers` | 3 | 1.0000 | 0.0000 | 0.0000 | 仅用于验证备用后端，不作为任务书目标模型结果 |

## 7. 复现命令

`vLLM + label_logprob` 正式评测命令：

```bash
cd /data/ronghao/uenv/uenv-bridge

IMAGE=localhost/vllm-openai:v0.19.0-cu130 \
MODEL_ID=Qwen/Qwen3.6-35B-A3B \
MODEL_DIR=/data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B \
OUTPUT_DIR=/data/ronghao/uenv/uenv-bridge/temp/benchmarks/pubmedqa/qwen3_6_35b_a3b_vllm_label_logprob \
BACKEND=vllm \
INFERENCE_MODE=label_logprob \
TENSOR_PARALLEL_SIZE=8 \
MAX_MODEL_LEN=4096 \
VLLM_LABEL_BATCH_SIZE=64 \
./scripts/benchmark/run_pubmedqa_baseline.sh
```

`vLLM + generate` 正式评测命令：

```bash
cd /data/ronghao/uenv/uenv-bridge

IMAGE=localhost/vllm-openai:v0.19.0-cu130 \
MODEL_ID=Qwen/Qwen3.6-35B-A3B \
MODEL_DIR=/data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B \
OUTPUT_DIR=/data/ronghao/uenv/uenv-bridge/temp/benchmarks/pubmedqa/qwen3_6_35b_a3b_vllm_generate_strict \
BACKEND=vllm \
INFERENCE_MODE=generate \
PROMPT_STYLE=strict_label \
MAX_TOKENS=512 \
TENSOR_PARALLEL_SIZE=8 \
MAX_MODEL_LEN=4096 \
./scripts/benchmark/run_pubmedqa_baseline.sh
```

`transformers + label_logprob` 对照评测命令：

```bash
cd /data/ronghao/uenv/uenv-bridge

IMAGE=localhost/uenv-bridge-verl:layer4-build \
MODEL_ID=Qwen/Qwen3.6-35B-A3B \
MODEL_DIR=/data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B \
OUTPUT_DIR=/data/ronghao/uenv/uenv-bridge/temp/benchmarks/pubmedqa/qwen3_6_35b_a3b_label_logprob \
BACKEND=transformers \
INFERENCE_MODE=label_logprob \
PYTHON_BIN=/data/ronghao/venvs/qwen36-transformers/bin/python \
TRANSFORMERS_DEVICE_MAP=auto \
./scripts/benchmark/run_pubmedqa_baseline.sh
```

## 8. 观察

`vLLM + label_logprob` 的主要问题是 `maybe` 类完全没有被预测出来，导致 `maybe` 的 F1 为 0，并显著拉低 Macro-F1。`vLLM + generate` 在严格标签 prompt 下可以预测出部分 `maybe`，整体 Accuracy 和 Macro-F1 更高，但它依赖对生成文本的解析，且推理成本高于候选标签打分。后续如果针对 PubMedQA 做后训练或格式/分类校准，重点应关注 `maybe` 类的判别能力，而不仅是总体 Accuracy。
