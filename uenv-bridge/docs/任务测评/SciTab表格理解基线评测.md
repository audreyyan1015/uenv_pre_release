# SciTab 表格理解 UEnv 基线评测

> 日期：2026-07-17
> 阶段：Eval-first，未进行后训练
> 任务书条目：2. 表格理解
> Benchmark：SciTab
> 目标模型：`Qwen/Qwen3.6-35B-A3B`
> 正式口径：接入 UEnv，`official` prompt，thinking 开启，reasoning 以独立字段透传，`MAX_TOKENS=32768`，`THINKING_TOKEN_BUDGET=16384`

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
| `model_endpoint.url` | `http://10.10.20.142:18096/v1` | Worker 访问 adapter model gateway |
| `generation_config.max_tokens` | `32768` | 本次 UEnv thinking 口径最大生成长度 |
| `generation_config.thinking_token_budget` | `16384` | Qwen thinking token budget |
| `generation_config.chat_template_kwargs.enable_thinking` | `true` | 显式开启 thinking |
| `generation_config.chat_template_kwargs.preserve_thinking` | `true` | 要求上游保留 reasoning 独立字段 |
| `temperature` | `0.0` | 确定性生成 |
| `top_p` | `1.0` | 不额外截断采样分布 |

## 4. UEnv Thinking 全量配置

| 配置 | 值 |
|---|---|
| 模型 | `Qwen/Qwen3.6-35B-A3B` |
| 本机推理服务 | vLLM OpenAI-compatible server，镜像 `localhost/vllm-openai:v0.19.0-cu130` |
| GPU | 8 张 A100 |
| Tensor parallel | 8 |
| vLLM `max_model_len` | 65536 |
| vLLM reasoning parser | `qwen3` |
| Adapter model gateway | `http://10.10.20.142:18096/v1` |
| Gateway upstream | `http://127.0.0.1:18081/v1` |
| Gateway thinking 注入 | `--enable-thinking --preserve-thinking --thinking-token-budget 16384` |
| AdapterCore endpoint | `8.130.75.157:8088` |
| UEnv batch size | 1 |
| Prompt style | `official` |
| Thinking mode | 开启 |
| Reasoning 返回方式 | OpenAI message 独立字段，例如 `message.reasoning` |
| `MAX_TOKENS` | 32768 |
| `THINKING_TOKEN_BUDGET` | 16384 |
| `TEMPERATURE` | 0.0 |
| `TOP_P` | 1.0 |
| 数据集 | SciTab 公开全量 1224 条 |
| 后训练 | 未进行 SFT/RL，Eval-first 基线 |

## 5. 运行命令

本轮复用 OlymMATH / PubMedQA 测评阶段已经启动的 8GPU vLLM，监听本机 `18081`。vLLM 启动参数包含：

```bash
python3 -m vllm.entrypoints.openai.api_server \
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

启动 Worker 可访问的 adapter model gateway：

```bash
cd /data/ronghao/uenv/uenv-bridge

BASE=/data/ronghao/uenv/uenv-bridge/temp/benchmarks/scitab/qwen3_6_35b_a3b_uenv_official_reasoning_fields_max32768_budget16384_$(date +%Y%m%d_%H%M%S)
mkdir -p "$BASE"

PYTHONPATH=src python3 scripts/benchmark/run_model_gateway.py \
  --upstream http://127.0.0.1:18081/v1 \
  --bind-host 0.0.0.0 \
  --port 18096 \
  --public-url http://10.10.20.142:18096/v1 \
  --request-timeout-seconds 7200 \
  --enable-thinking \
  --preserve-thinking \
  --thinking-token-budget 16384 \
  --log-path "$BASE/model-gateway-official-reasoning-fields-18096-budget16384.jsonl"
```

运行 UEnv 全量评测：

```bash
cd /data/ronghao/uenv/uenv-bridge

OUT=/data/ronghao/uenv/uenv-bridge/temp/benchmarks/scitab/qwen3_6_35b_a3b_uenv_official_reasoning_fields_max32768_budget16384_full_20260717_121807
mkdir -p "$OUT"

OUTPUT_DIR="$OUT" \
UENV_ADAPTER_CORE_ENDPOINT=8.130.75.157:8088 \
UENV_ROLLOUT_MODEL_ENDPOINT=http://10.10.20.142:18096/v1 \
UENV_ROLLOUT_MODEL_NAME=Qwen/Qwen3.6-35B-A3B \
BATCH_SIZE=1 \
PROMPT_STYLE=official \
MAX_TOKENS=32768 \
ENABLE_THINKING=1 \
PRESERVE_THINKING=1 \
THINKING_TOKEN_BUDGET=16384 \
TEMPERATURE=0.0 \
TOP_P=1.0 \
TIMEOUT_SECONDS=7200 \
CLIENT_TIMEOUT_SECONDS=7800 \
./scripts/benchmark/run_scitab_uenv_baseline.sh
```

本次正式结果目录：

```text
temp/benchmarks/scitab/qwen3_6_35b_a3b_uenv_official_reasoning_fields_max32768_budget16384_full_20260717_121807/
```

## 6. 正式结果

| 模型 | UEnv endpoint | Model endpoint | 样本数 | completed | failed | Parse rate | Accuracy | Macro-F1 | reward accuracy |
|---|---|---|---:|---:|---:|---:|---:|---:|---:|
| `Qwen/Qwen3.6-35B-A3B` | `8.130.75.157:8088` | `http://10.10.20.142:18096/v1` | 1224 | 1224 | 0 | 1.0000 | 0.7451 | 0.7340 | 0.7451 |

各类别指标：

| 类别 | Precision | Recall | F1 | Support |
|---|---:|---:|---:|---:|
| supports | 0.7028 | 0.8796 | 0.7813 | 457 |
| refutes | 0.7640 | 0.7640 | 0.7640 | 411 |
| not enough info | 0.8133 | 0.5506 | 0.6566 | 356 |

预测分布：

| 标签 | Gold | Pred |
|---|---:|---:|
| supports | 457 | 572 |
| refutes | 411 | 411 |
| not enough info | 356 | 241 |
| unparsed | 0 | 0 |

混淆矩阵：

| Gold \ Pred | supports | refutes | not enough info | unparsed |
|---|---:|---:|---:|---:|
| supports | 402 | 28 | 27 | 0 |
| refutes | 79 | 314 | 18 | 0 |
| not enough info | 91 | 69 | 196 | 0 |

输出文件：

```text
temp/benchmarks/scitab/qwen3_6_35b_a3b_uenv_official_reasoning_fields_max32768_budget16384_full_20260717_121807/metrics.json
temp/benchmarks/scitab/qwen3_6_35b_a3b_uenv_official_reasoning_fields_max32768_budget16384_full_20260717_121807/predictions_official.json
temp/benchmarks/scitab/qwen3_6_35b_a3b_uenv_official_reasoning_fields_max32768_budget16384_full_20260717_121807/predictions.jsonl
temp/benchmarks/scitab/qwen3_6_35b_a3b_uenv_official_reasoning_fields_max32768_budget16384_full_20260717_121807/predictions.csv
temp/benchmarks/scitab/qwen3_6_35b_a3b_uenv_official_reasoning_fields_max32768_budget16384_full_20260717_121807/uenv_requests.jsonl
temp/benchmarks/scitab/qwen3_6_35b_a3b_uenv_official_reasoning_fields_max32768_budget16384_full_20260717_121807/uenv_results.jsonl
temp/benchmarks/scitab/qwen3_6_35b_a3b_uenv_official_reasoning_fields_max32768_budget16384_full_20260717_121807/run.log
```

## 7. Reasoning 字段说明

本轮“返回给 Worker 思考过程”的含义是：adapter model gateway 对 Worker 的 OpenAI-compatible HTTP 响应保留 `message.reasoning` / `message.reasoning_content` 等独立字段，而不是把思考过程拼接到 `message.content`。

抽查 gateway 响应可见：

| 项 | 值 |
|---|---|
| `message.content` | `"\n\nsupports"` |
| `message` keys | `annotations, audio, content, function_call, reasoning, refusal, role, tool_calls` |
| `message.reasoning` 长度 | 3221 字符 |

因此，Worker 访问 gateway 时可以拿到 reasoning 独立字段。当前 Adapter 侧 `uenv_results.jsonl` 中没有 `<think>`，这是因为 Worker 构造 `EpisodeResult` 时只同步回传最终 action/content 字段；`uenv_results.jsonl` 中 1224 条结果的 `response_text` 最大长度为 17，均为最终标签形式，例如 `supports`、`refutes`、`not enough info`。

如果后续需要在 Adapter 侧证明每个样本的 reasoning 内容，需要 Worker 在 `EpisodeResult.step.info` 中额外写入从 OpenAI 响应获得的 `reasoning` / `reasoning_content` 字段，或者另行记录 Worker 侧原始模型响应日志。

## 8. 结果分析

本轮 UEnv 链路全量 SciTab 1224 条样本全部完成，`completed=1224`、`failed=0`，说明 Adapter -> AdapterCore/Server -> Worker -> gateway/vLLM -> Worker 判分 -> Adapter 汇总链路稳定。

Accuracy 与 reward accuracy 均为 0.7451，说明 Adapter 自身解析结果与 Worker reward 结果一致。类别表现上，`supports` recall 较高，为 0.8796；`refutes` precision/recall 较均衡，均为 0.7640；`not enough info` precision 较高但 recall 偏低，为 0.5506，模型仍倾向把部分信息不足样本判断为 `supports` 或 `refutes`。

由于本轮 reasoning 以独立字段返回，Adapter 结果文件不再适合用 `<think>` 闭合情况做截断分析。当前 `content` 只保留最终标签，因此从 Adapter 侧观察不到 reasoning 是否截断；若需要分析 thinking 完整性，应让 Worker 或 gateway 保存原始 OpenAI 响应体中的 `reasoning` 字段和 `finish_reason`。
