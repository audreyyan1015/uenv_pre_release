# SciTab 表格理解 UEnv 基线评测

> 日期：2026-07-14
> 阶段：Eval-first，未进行后训练
> 任务书条目：2. 表格理解
> Benchmark：SciTab
> 目标模型：`Qwen/Qwen3.6-35B-A3B`
> 正式口径：接入 UEnv，thinking 开启，`MAX_TOKENS=1024`

## 1. 任务说明

SciTab 是科学论文表格理解与 claim verification 任务。输入为科学论文表格、表格上下文和一条 claim，模型需要判断 claim 与表格之间的关系：

```text
supports / refutes / not enough info
```

本阶段目标是评估基准模型在该 benchmark 上通过 UEnv 链路的零训练表现，不进行 SFT、RL 或其他后训练。

## 2. 数据集

| 项 | 内容 |
|---|---|
| 数据文件 | `data/benchmarks/scitab/sci_tab.json` |
| 样本数 | 1224 |
| 标签 | `supports`、`refutes`、`not enough info` |
| 标签分布 | supports: 457；refutes: 411；not enough info: 356 |
| 官方仓库 | https://github.com/XinyuanLu00/SciTab |

当前公开文件 `sci_tab.json` 中没有显式 train/dev/test split 字段。因此本阶段将该公开全量数据作为 SciTab benchmark/test set 进行 UEnv 基线评测。

## 3. UEnv 评测链路

按照 Worker 侧五类 benchmark 文档，SciTab 复用 `math` 环境，由 Worker 内部根据 `env_config.dataset=scitab` 路由到对应判分逻辑。

```text
SciTab 样本
  -> Adapter 构造 EpisodeRequest
  -> AdapterCore / Server
  -> Worker math plugin
  -> Worker 调用 adapter model gateway
  -> gateway 转发到本机 vLLM 模型 endpoint
  -> Worker 解析 supports/refutes/not enough info 并计算 reward
  -> EpisodeResult 返回 Adapter
  -> driver 汇总 Accuracy / Macro-F1 / reward accuracy
```

核心请求字段：

| 字段 | 值 | 说明 |
|---|---|---|
| `env_type` | `math` | 由 Server 调度到 math Worker / plugin |
| `env_config.dataset` | `scitab` | Worker 内部路由到 SciTab 判分逻辑 |
| `reward_config.target` | `supports/refutes/not enough info` | 当前样本 gold label |
| `model_endpoint.url` | `http://10.10.20.142:18088/v1` | Worker 访问 adapter model gateway |
| `generation_config.max_tokens` | `1024` | 本次正式 thinking 口径最大生成长度 |
| `temperature` | `0.0` | 确定性生成 |
| `top_p` | `1.0` | 不额外截断采样分布 |

## 4. 运行命令

运行前需要先启动 Worker 可访问的 OpenAI-compatible 模型 endpoint。本次使用 adapter model gateway 对外暴露给 Worker，gateway 上游连接本机 vLLM。

启动 vLLM：

```bash
podman run --rm -d \
  --name uenv-scitab-vllm \
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

启动 adapter model gateway。正式 thinking 口径不传 `--disable-thinking`：

```bash
cd /data/ronghao/uenv/uenv-bridge

OUTPUT_DIR=/data/ronghao/uenv/uenv-bridge/temp/benchmarks/scitab/qwen3_6_35b_a3b_uenv_thinking_max1024_full_$(date +%Y%m%d_%H%M%S)
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
./scripts/benchmark/run_scitab_uenv_baseline.sh 2>&1 | tee "$OUTPUT_DIR/run.log"
```

本次正式结果目录：

```text
temp/benchmarks/scitab/qwen3_6_35b_a3b_uenv_thinking_max1024_full_20260714_213138/
```

## 5. 正式结果

| 模型 | UEnv endpoint | Model endpoint | 样本数 | completed | failed | Parse rate | Accuracy | Macro-F1 | reward accuracy |
|---|---|---|---:|---:|---:|---:|---:|---:|---:|
| `Qwen/Qwen3.6-35B-A3B` | `8.130.75.157:8088` | `http://10.10.20.142:18088/v1` | 1224 | 1224 | 0 | 1.0000 | 0.7418 | 0.7343 | 0.7418 |

说明：`Accuracy` 来自 adapter 侧对 `response_text` 的本地标签解析；`reward accuracy` 来自 Worker 返回的 `EpisodeResult.summary.total_reward`。2026-07-15 已将 adapter 侧 SciTab 标签解析调整为与 Worker math plugin 一致的 canonical-label 口径，即从整段输出中抽取最后一次出现的 `supports/refutes/not enough info` 标签。因此本次基于已有模型输出离线重算后，`Accuracy` 与 `reward accuracy` 对齐；没有重新调用模型，也没有重新跑 UEnv 链路。

各类别指标：

| 类别 | Precision | Recall | F1 | Support |
|---|---:|---:|---:|---:|
| supports | 0.7156 | 0.8315 | 0.7692 | 457 |
| refutes | 0.7740 | 0.7664 | 0.7702 | 411 |
| not enough info | 0.7448 | 0.5983 | 0.6636 | 356 |

预测分布：

| 标签 | Gold | Pred |
|---|---:|---:|
| supports | 457 | 531 |
| refutes | 411 | 407 |
| not enough info | 356 | 286 |
| unparsed | 0 | 0 |

混淆矩阵：

| Gold \ Pred | supports | refutes | not enough info | unparsed |
|---|---:|---:|---:|---:|
| supports | 380 | 33 | 44 | 0 |
| refutes | 67 | 315 | 29 | 0 |
| not enough info | 84 | 59 | 213 | 0 |

输出文件：

```text
temp/benchmarks/scitab/qwen3_6_35b_a3b_uenv_thinking_max1024_full_20260714_213138/metrics.json
temp/benchmarks/scitab/qwen3_6_35b_a3b_uenv_thinking_max1024_full_20260714_213138/predictions_official.json
temp/benchmarks/scitab/qwen3_6_35b_a3b_uenv_thinking_max1024_full_20260714_213138/predictions.jsonl
temp/benchmarks/scitab/qwen3_6_35b_a3b_uenv_thinking_max1024_full_20260714_213138/predictions.csv
temp/benchmarks/scitab/qwen3_6_35b_a3b_uenv_thinking_max1024_full_20260714_213138/uenv_requests.jsonl
temp/benchmarks/scitab/qwen3_6_35b_a3b_uenv_thinking_max1024_full_20260714_213138/uenv_results.jsonl
temp/benchmarks/scitab/qwen3_6_35b_a3b_uenv_thinking_max1024_full_20260714_213138/model-gateway.jsonl
temp/benchmarks/scitab/qwen3_6_35b_a3b_uenv_thinking_max1024_full_20260714_213138/run.log
```

## 6. 模型输出截断分析

当前 `uenv_results.jsonl` 和 `model-gateway.jsonl` 没有保存 vLLM 原始 `finish_reason`，因此这里从模型输出结构判断是否被截断：Qwen thinking 正常情况下应先生成推理内容，再出现 `</think>`，随后给出最终标签。若输出中没有 `</think>`，则可认为该条生成没有完整收束，属于从输出角度可观察到的截断或未完成输出。

统计结果：

| 指标 | 数值 |
|---|---:|
| 样本数 | 1224 |
| 未出现 `</think>` 的输出 | 264 |
| 未出现 `</think>` 占比 | 21.57% |
| `</think>` 后缺少最终标签的输出 | 266 |
| adapter 无法解析标签的输出 | 0 |
| response 字符数中位数 / P90 / P95 / P99 / 最大值 | 2264 / 3687 / 4013 / 4463 / 4853 |
| response 词数中位数 / P90 / P95 / P99 / 最大值 | 349 / 595 / 648 / 705 / 766 |

最长的几条输出尾部仍停留在表格推理过程，没有闭合 `</think>`，也没有进入稳定的最终标签段。由于 SciTab prompt 包含表格，模型在 thinking 模式下更容易展开较长推理，因此 `MAX_TOKENS=1024` 对 SciTab 明显比 PubMedQA 更紧。

## 7. 参数是否合适

`MAX_TOKENS=1024` 可以跑通 SciTab 全量 UEnv thinking 评测，且 `completed=1224`、`failed=0`，整体指标可作为当前阶段的 UEnv 基线结果保留。

但从模型输出质量看，这组参数并不充分：21.57% 的样本没有生成 `</think>`，21.73% 的样本没有完整进入 `</think>` 后的最终答案段。虽然 adapter 侧已按 Worker 口径从全部样本中解析出标签，Worker reward 也能完成判分，但这说明很多样本的结果依赖从未完整收束的 reasoning 文本中抽取标签，存在格式不稳定风险。

如果后续目标是严格比较 thinking 模式下的模型能力，建议将 SciTab 的 `MAX_TOKENS` 提高到 2048 或更高，并保存上游 vLLM 的 `finish_reason`。如果目标只是建立低成本分类基线，则更适合关闭 thinking，让模型直接输出短标签；但本文件保留的是当前最终正式 thinking 口径结果。

当前效果短板主要是 `not enough info` 的召回偏低：`supports`、`refutes` 和 `not enough info` 的 recall 分别为 0.8315、0.7664 和 0.5983。后续如果进入训练阶段，应重点关注类别均衡和长 reasoning 输出下标签抽取规则的一致性。
