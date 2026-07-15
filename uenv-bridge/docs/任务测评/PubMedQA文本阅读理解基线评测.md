# PubMedQA 文本阅读理解 UEnv 基线评测

> 日期：2026-07-14
> 阶段：Eval-first，未进行后训练
> 任务书条目：1. 文本阅读理解
> Benchmark：PubMedQA
> 目标模型：`Qwen/Qwen3.6-35B-A3B`
> 正式口径：接入 UEnv，thinking 开启，`MAX_TOKENS=1024`

## 1. 任务说明

PubMedQA 是生物医学文本阅读理解任务。输入为 PubMed abstract 上下文和一个研究问题，模型需要输出三分类答案：

```text
yes / no / maybe
```

本阶段目标是评估基准模型在该 benchmark 上通过 UEnv 链路的零训练表现，不进行 SFT、RL 或其他后训练。

## 2. 数据集

| 项 | 内容 |
|---|---|
| 数据文件 | `data/benchmarks/pubmedqa/ori_pqal.json` |
| 样本数 | 1000 |
| 标签 | `yes`、`no`、`maybe` |
| 标签分布 | yes: 552；no: 338；maybe: 110 |
| 官方仓库 | https://github.com/pubmedqa/pubmedqa |
| 官方主页 | https://pubmedqa.github.io/ |

当前使用 1000 条 expert-labeled 样本作为本阶段基线验证集。

## 3. UEnv 评测链路

按照 Worker 侧五类 benchmark 文档，PubMedQA 复用 `math` 环境，由 Worker 内部根据 `env_config.dataset=pubmedqa` 路由到对应判分逻辑。

```text
PubMedQA 样本
  -> Adapter 构造 EpisodeRequest
  -> AdapterCore / Server
  -> Worker math plugin
  -> Worker 调用 adapter model gateway
  -> gateway 转发到本机 vLLM 模型 endpoint
  -> Worker 解析 yes/no/maybe 并计算 reward
  -> EpisodeResult 返回 Adapter
  -> driver 汇总 Accuracy / Macro-F1 / reward accuracy
```

核心请求字段：

| 字段 | 值 | 说明 |
|---|---|---|
| `env_type` | `math` | 由 Server 调度到 math Worker / plugin |
| `env_config.dataset` | `pubmedqa` | Worker 内部路由到 PubMedQA 判分逻辑 |
| `reward_config.target` | `yes/no/maybe` | 当前样本 gold label |
| `model_endpoint.url` | `http://10.10.20.142:18088/v1` | Worker 访问 adapter model gateway |
| `generation_config.max_tokens` | `1024` | 本次正式 thinking 口径最大生成长度 |
| `temperature` | `0.0` | 确定性生成 |
| `top_p` | `1.0` | 不额外截断采样分布 |

## 4. 运行命令

运行前需要先启动 Worker 可访问的 OpenAI-compatible 模型 endpoint。本次使用 adapter model gateway 对外暴露给 Worker，gateway 上游连接本机 vLLM。

启动 vLLM：

```bash
podman run --rm -d \
  --name uenv-pubmedqa-vllm \
  --network host \
  --device nvidia.com/gpu=all \
  --pids-limit=-1 \
  --shm-size=64g \
  -v /data/ronghao:/data/ronghao \
  -e MODELSCOPE_CACHE=/data/ronghao/models/modelscope \
  localhost/vllm-openai:v0.19.0-cu130 \
  --host 0.0.0.0 \
  --port 18080 \
  --model /data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B \
  --served-model-name Qwen/Qwen3.6-35B-A3B \
  --tensor-parallel-size 8 \
  --max-model-len 8192 \
  --gpu-memory-utilization 0.88 \
  --trust-remote-code
```

启动 adapter model gateway：

```bash
cd /data/ronghao/uenv/uenv-bridge

OUTPUT_DIR=/data/ronghao/uenv/uenv-bridge/temp/benchmarks/pubmedqa/qwen3_6_35b_a3b_uenv_thinking_max1024_full_$(date +%Y%m%d_%H%M%S)
mkdir -p "$OUTPUT_DIR"

nohup env PYTHONPATH=src \
scripts/benchmark/run_model_gateway.py \
  --upstream http://127.0.0.1:18080/v1 \
  --bind-host 0.0.0.0 \
  --port 18088 \
  --public-url http://10.10.20.142:18088/v1 \
  --log-path "$OUTPUT_DIR/model-gateway.jsonl" \
  > "$OUTPUT_DIR/model-gateway.out" 2>&1 &
```

运行 UEnv 全量评测：

```bash
cd /data/ronghao/uenv/uenv-bridge

IMAGE=localhost/uenv-bridge-verl:layer4-build \
UENV_ADAPTER_CORE_ENDPOINT=8.130.75.157:8088 \
UENV_ROLLOUT_MODEL_ENDPOINT=http://10.10.20.142:18088/v1 \
UENV_ROLLOUT_MODEL_NAME=Qwen/Qwen3.6-35B-A3B \
OUTPUT_DIR="$OUTPUT_DIR" \
BATCH_SIZE=1 \
PROMPT_STYLE=strict_label \
MAX_TOKENS=1024 \
./scripts/benchmark/run_pubmedqa_uenv_baseline.sh
```

本次正式结果目录：

```text
temp/benchmarks/pubmedqa/qwen3_6_35b_a3b_uenv_thinking_max1024_full_20260713_154812/
```

## 5. 正式结果

| 模型 | UEnv endpoint | Model endpoint | 样本数 | completed | failed | Parse rate | Accuracy | Macro-F1 | reward accuracy |
|---|---|---|---:|---:|---:|---:|---:|---:|---:|
| `Qwen/Qwen3.6-35B-A3B` | `8.130.75.157:8088` | `http://10.10.20.142:18088/v1` | 1000 | 1000 | 0 | 1.0000 | 0.8000 | 0.5802 | 0.8000 |

各类别指标：

| 类别 | Precision | Recall | F1 | Support |
|---|---:|---:|---:|---:|
| yes | 0.8122 | 0.9167 | 0.8613 | 552 |
| no | 0.8039 | 0.8609 | 0.8314 | 338 |
| maybe | 0.2000 | 0.0273 | 0.0480 | 110 |

预测分布：

| 标签 | Gold | Pred |
|---|---:|---:|
| yes | 552 | 623 |
| no | 338 | 362 |
| maybe | 110 | 15 |

混淆矩阵：

| Gold \ Pred | yes | no | maybe | unparsed |
|---|---:|---:|---:|---:|
| yes | 506 | 37 | 9 | 0 |
| no | 44 | 291 | 3 | 0 |
| maybe | 73 | 34 | 3 | 0 |

输出文件：

```text
temp/benchmarks/pubmedqa/qwen3_6_35b_a3b_uenv_thinking_max1024_full_20260713_154812/metrics.json
temp/benchmarks/pubmedqa/qwen3_6_35b_a3b_uenv_thinking_max1024_full_20260713_154812/predictions_official.json
temp/benchmarks/pubmedqa/qwen3_6_35b_a3b_uenv_thinking_max1024_full_20260713_154812/predictions.jsonl
temp/benchmarks/pubmedqa/qwen3_6_35b_a3b_uenv_thinking_max1024_full_20260713_154812/predictions.csv
temp/benchmarks/pubmedqa/qwen3_6_35b_a3b_uenv_thinking_max1024_full_20260713_154812/uenv_requests.jsonl
temp/benchmarks/pubmedqa/qwen3_6_35b_a3b_uenv_thinking_max1024_full_20260713_154812/uenv_results.jsonl
temp/benchmarks/pubmedqa/qwen3_6_35b_a3b_uenv_thinking_max1024_full_20260713_154812/model-gateway.jsonl
```

## 6. 模型输出截断分析

当前 `uenv_results.jsonl` 和 `model-gateway.jsonl` 没有保存 vLLM 原始 `finish_reason`，因此这里从模型输出结构判断是否被截断：Qwen thinking 正常情况下应先生成推理内容，再出现 `</think>`，随后给出最终标签。若输出中没有 `</think>`，则可认为该条生成没有完整收束，属于从输出角度可观察到的截断或未完成输出。

统计结果：

| 指标 | 数值 |
|---|---:|
| 样本数 | 1000 |
| 未出现 `</think>` 的输出 | 62 |
| 未出现 `</think>` 占比 | 6.20% |
| `</think>` 后缺少最终标签的输出 | 62 |
| adapter 无法解析标签的输出 | 0 |
| response 字符数中位数 / P90 / P95 / P99 / 最大值 | 1816 / 3748 / 4333 / 4765 / 5058 |
| response 词数中位数 / P90 / P95 / P99 / 最大值 | 267 / 553 / 651 / 711 / 748 |

最长的几条输出尾部仍停留在推理过程，例如没有闭合 `</think>`，也没有稳定进入最终答案段。虽然 adapter 仍能从推理文本中解析出 `yes/no/maybe`，因此 parse rate 为 1.0000，但这不代表输出格式完全健康；它只是说明当前解析器可以从未完整收束的文本中提取到标签。

## 7. 参数是否合适

`MAX_TOKENS=1024` 对 PubMedQA 的正式 UEnv thinking 评测基本可用：全量 1000 条均完成，parse rate 为 1.0000，Accuracy 为 0.8000，未闭合 thinking 的比例为 6.20%。从评测跑通和指标稳定性看，这组参数可以作为当前基线结果保留。

但如果目标是严格评估 thinking 模式下完整推理与最终答案，`MAX_TOKENS=1024` 仍偏紧。建议后续正式对齐或报告型评测将 `MAX_TOKENS` 提高到 1536 或 2048，并在结果文件中额外保存上游 vLLM 的 `finish_reason`，避免只能通过 `</think>` 做间接判断。

当前主要效果短板不是截断，而是类别偏置：模型很少预测 `maybe`，`maybe` recall 只有 0.0273，导致 Macro-F1 明显低于 Accuracy。后续如果进入训练阶段，应重点关注 `maybe` 类召回和类别均衡。
