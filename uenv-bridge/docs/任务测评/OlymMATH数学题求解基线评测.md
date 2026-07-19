# OlymMATH 数学题求解 UEnv 基线评测

> 日期：2026-07-19
> 阶段：Eval-first，未进行后训练
> 任务书条目：5. 数学题求解
> Benchmark：OlymMATH-EASY / OlymMATH-HARD
> 目标模型：`Qwen/Qwen3.6-35B-A3B`
> 正式口径：接入 UEnv，thinking 开启，`MAX_TOKENS=32768`，`thinking_token_budget=16384`，`preserve_thinking=false`，`strip_reasoning=true`，全量 400 题

## 1. 任务说明

OlymMATH 是奥赛级数学推理评测集。输入为自然语言数学题，模型需要完成推理并输出最终答案。官方 prompt 要求将最终答案写在：

```text
\boxed{}
```

本阶段目标是评估基准模型在该 benchmark 上通过 UEnv 链路的零训练表现，不进行 SFT、RL 或其他后训练。

## 2. 数据集

| 文件 | 语言 | 难度 | 样本数 |
|---|---|---|---:|
| `data/benchmarks/olymmath/OlymMATH-EN-EASY.jsonl` | 英文 | EASY | 100 |
| `data/benchmarks/olymmath/OlymMATH-EN-HARD.jsonl` | 英文 | HARD | 100 |
| `data/benchmarks/olymmath/OlymMATH-ZH-EASY.jsonl` | 中文 | EASY | 100 |
| `data/benchmarks/olymmath/OlymMATH-ZH-HARD.jsonl` | 中文 | HARD | 100 |

四个公开文件共 400 条样本。当前公开文件没有额外 train/dev/test split 字段，因此本阶段将这 400 条样本作为 OlymMATH benchmark/test set 进行 UEnv 全量基线评测。

## 3. UEnv 评测链路

按照 Worker 侧五类 benchmark 文档，OlymMATH 复用 `math` 环境，由 Worker 内部根据 `env_config.dataset` 路由到 OlymMATH 判分逻辑。

```text
OlymMATH 样本
  -> Adapter 构造 EpisodeRequest
  -> AdapterCore / Server
  -> Worker math plugin
  -> Worker 调用 adapter model gateway
  -> gateway 转发到本机 vLLM 模型 endpoint
  -> Worker 抽取最终答案并计算 reward
  -> EpisodeResult 返回 Adapter
  -> driver 汇总 UEnv reward accuracy / completed / failed / parse rate
```

核心请求字段：

| 字段 | 值 | 说明 |
|---|---|---|
| `env_type` | `math` | 由 Server 调度到 math Worker / plugin |
| `env_config.dataset` | `olymmath-easy` / `olymmath-hard` | 按样本 difficulty 设置 |
| `reward_config.target` | 官方 `answer` | Worker 使用 OlymMATH backend 抽取并判分 |
| `model_endpoint.url` | `http://10.10.20.142:18094/v1` | Worker 访问 adapter model gateway |
| `generation_config.max_tokens` | `32768` | 本次 UEnv thinking 全量口径最大生成长度 |
| `generation_config.thinking_token_budget` | `16384` | gateway 注入到 vLLM `/v1/chat/completions` 请求中，用于控制 thinking 预算 |
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
| vLLM reasoning config | `{"reasoning_start_str":"<think>","reasoning_end_str":"</think>"}` |
| Adapter model gateway | `http://10.10.20.142:18094/v1` |
| Gateway upstream | `http://127.0.0.1:18081/v1` |
| Gateway request timeout | 7200s |
| Gateway thinking 注入 | `--enable-thinking --strip-reasoning --thinking-token-budget 16384` |
| AdapterCore endpoint | `8.130.75.157:8088` |
| UEnv batch size | 1 |
| Prompt style | `official` |
| Thinking mode | 开启 |
| `MAX_TOKENS` | 32768 |
| `THINKING_TOKEN_BUDGET` | 16384 |
| `TEMPERATURE` | 0.0 |
| `TOP_P` | 1.0 |
| 数据集 | EN-EASY、EN-HARD、ZH-EASY、ZH-HARD |
| 断点续跑 | 关闭，`RESUME=0` |

## 5. 运行命令

启动 8GPU vLLM，监听本机 `18081`。由于当前 `localhost/uenv-bridge-verl:layer4-build` 内的 vLLM 版本不能识别 Qwen3.6 MoE，本轮使用 `localhost/vllm-openai:v0.19.0-cu130`：

```bash
cd /data/ronghao/uenv/uenv-bridge

BASE=/data/ronghao/uenv/uenv-bridge/temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_reasoning_budget_20260715_111008
mkdir -p "$BASE"

podman run --rm \
  --entrypoint bash \
  --network host \
  --pids-limit=-1 \
  --shm-size=64g \
  --device nvidia.com/gpu=all \
  -v /data/ronghao:/data/ronghao \
  -w /data/ronghao/uenv/uenv-bridge \
  localhost/vllm-openai:v0.19.0-cu130 \
  -lc 'exec python3 -m vllm.entrypoints.openai.api_server \
    --model /data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B \
    --served-model-name Qwen/Qwen3.6-35B-A3B \
    --host 0.0.0.0 \
    --port 18081 \
    --tensor-parallel-size 8 \
    --max-model-len 65536 \
    --gpu-memory-utilization 0.90 \
    --reasoning-parser qwen3 \
    --reasoning-config "{\"reasoning_start_str\":\"<think>\",\"reasoning_end_str\":\"</think>\"}" \
    --trust-remote-code \
    > /data/ronghao/uenv/uenv-bridge/temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_reasoning_budget_20260715_111008/vllm_reasoning_content_budget16384_65536.log 2>&1'
```

可用下面命令确认 vLLM 已就绪：

```bash
curl --noproxy '*' http://127.0.0.1:18081/v1/models
```

启动 Worker 可访问的 adapter model gateway，转发到本机 vLLM：

```bash
cd /data/ronghao/uenv/uenv-bridge

BASE=/data/ronghao/uenv/uenv-bridge/temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_reasoning_budget_20260715_111008

PYTHONPATH=src python3 scripts/benchmark/run_model_gateway.py \
  --upstream http://127.0.0.1:18081/v1 \
  --bind-host 0.0.0.0 \
  --port 18094 \
  --public-url http://10.10.20.142:18094/v1 \
  --request-timeout-seconds 7200 \
  --enable-thinking \
  --strip-reasoning \
  --thinking-token-budget 16384 \
  --log-path "$BASE/model-gateway-thinking-strip-reasoning-18094-budget16384.jsonl"
```

可用下面命令确认 gateway 已就绪：

```bash
curl --noproxy '*' http://127.0.0.1:18094/v1/models
```

通过 UEnv 跑全量 OlymMATH：

```bash
cd /data/ronghao/uenv/uenv-bridge

OUT=/data/ronghao/uenv/uenv-bridge/temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_thinking_max32768_budget16384_full_20260718_223005
mkdir -p "$OUT"

RESUME=0 \
OUTPUT_DIR="$OUT" \
UENV_ADAPTER_CORE_ENDPOINT=8.130.75.157:8088 \
UENV_ROLLOUT_MODEL_ENDPOINT=http://10.10.20.142:18094/v1 \
UENV_ROLLOUT_MODEL_NAME=Qwen/Qwen3.6-35B-A3B \
DATASETS=EN-EASY,EN-HARD,ZH-EASY,ZH-HARD \
BATCH_SIZE=1 \
PROMPT_STYLE=official \
MAX_TOKENS=32768 \
ENABLE_THINKING=1 \
PRESERVE_THINKING=0 \
THINKING_TOKEN_BUDGET=16384 \
TEMPERATURE=0.0 \
TOP_P=1.0 \
TIMEOUT_SECONDS=7200 \
CLIENT_TIMEOUT_SECONDS=7800 \
./scripts/benchmark/run_olymmath_uenv_baseline.sh
```

本轮正式结果目录：

```text
temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_thinking_max32768_budget16384_full_20260718_223005/
```

## 6. 正式结果

| 模型 | 样本数 | requests | results | completed | failed | UEnv reward accuracy | completed-only reward accuracy | Parse rate |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| `Qwen/Qwen3.6-35B-A3B` | 400 | 400 | 400 | 378 | 22 | 0.6175 | 0.6534 | 0.8950 |

说明：

| 指标 | 说明 |
|---|---|
| `UEnv reward accuracy` | 全量 400 条样本上，Worker 返回 `EpisodeResult.summary.total_reward` 的均值 |
| `completed-only reward accuracy` | 只在 `uenv_status=completed` 的样本上计算 reward 均值 |
| `Parse rate` | Adapter 本地从 `raw_output` 中抽取到最终答案的比例；UEnv 正确性主口径仍以 Worker reward 为准 |
| `parsed accuracy` | 只在 Adapter 成功抽取最终答案的样本上计算正确率，本轮为 0.6899 |

按子集：

| 子集 | 样本数 | completed | failed | UEnv reward accuracy | Parse rate |
|---|---:|---:|---:|---:|---:|
| EN-EASY | 100 | 78 | 22 | 0.6300 | 0.7600 |
| EN-HARD | 100 | 100 | 0 | 0.5000 | 0.9600 |
| ZH-EASY | 100 | 100 | 0 | 0.8000 | 0.9500 |
| ZH-HARD | 100 | 100 | 0 | 0.5400 | 0.9100 |

按语言：

| 语言 | 样本数 | completed | failed | UEnv reward accuracy | Parse rate |
|---|---:|---:|---:|---:|---:|
| EN | 200 | 178 | 22 | 0.5650 | 0.8600 |
| ZH | 200 | 200 | 0 | 0.6700 | 0.9300 |

按难度：

| 难度 | 样本数 | completed | failed | UEnv reward accuracy | Parse rate |
|---|---:|---:|---:|---:|---:|
| EASY | 200 | 178 | 22 | 0.7150 | 0.8550 |
| HARD | 200 | 200 | 0 | 0.5200 | 0.9350 |

按学科：

| 学科 | 样本数 | completed | failed | Parse rate | UEnv reward accuracy |
|---|---:|---:|---:|---:|---:|
| Algebra | 50 | 43 | 7 | 0.8400 | 0.6200 |
| Combinatorics | 54 | 48 | 6 | 0.8704 | 0.4259 |
| Geometry | 58 | 49 | 9 | 0.8276 | 0.6034 |
| Number Theory | 38 | 38 | 0 | 0.9211 | 0.6316 |
| 代数 | 50 | 50 | 0 | 0.9600 | 0.7400 |
| 几何 | 58 | 58 | 0 | 0.9310 | 0.7586 |
| 数论 | 38 | 38 | 0 | 0.8684 | 0.6316 |
| 组合 | 54 | 54 | 0 | 0.9444 | 0.5370 |

答案抽取与判分分布：

| 项 | 数量 |
|---|---:|
| `answer_phrase` 抽取 | 4 |
| `boxed` 抽取 | 354 |
| 未抽取到最终答案 | 42 |
| Worker reward 正确 | 247 |
| Worker reward 不正确 | 153 |

## 7. 运行稳定性

| 项 | 值 |
|---|---:|
| 总运行时间 | 约 9 小时 26 分钟 |
| 平均 episode 耗时 | 84.86s |
| completed 平均 episode 耗时 | 89.63s |
| failed 平均 episode 耗时 | 3.00s |
| Worker 并发 / UEnv batch size | 1 |
| Gateway `/v1/chat/completions` 调用 | 389 |
| Gateway `/v1/models` 调用 | 1 |
| Gateway HTTP 200 | 390 |
| Gateway error | 0 |
| Gateway `/v1/chat/completions` 平均 latency | 82.12s |
| completed 样本 `raw_output` 字符数均值 | 2450.29 |
| completed 样本 `raw_output` 字符数范围 | 24 - 8206 |

失败情况：

| 项 | 值 |
|---|---|
| 失败样本数 | 22 |
| 失败错误码 | `5001` |
| 本地结果中的错误信息 | `episode ... exceeded max attempts (3)` |
| Gateway 侧情况 | 本轮时间窗口内 389 次 `/v1/chat/completions` 和 1 次 `/v1/models` 均为 HTTP 200，未记录 gateway error |
| 失败分布 | 22 条全部来自 EN-EASY，样本编号集中在 `OlymMATH-EASY-64-EN` 至 `OlymMATH-EASY-85-EN` |

因此，本轮失败主要表现为 Server/Worker 侧 episode 三次尝试后仍未完成，而不是 adapter gateway 无法访问 vLLM。失败样本集中出现在 EN-EASY 的连续区间，后续需要结合 Server/Worker 的 request-level 日志确认是否存在该时间段的服务状态、重试上限或任务调度问题。

## 8. 输出截断与字段缺口

| 观察项 | 数量 |
|---|---:|
| completed 样本 | 378 |
| failed 空输出样本 | 22 |
| completed 样本中出现 `\boxed{}` | 354 |
| completed 样本中没有 `\boxed{}` | 24 |
| completed 样本中出现 `</think>` | 2 |

本轮 gateway 使用 `--strip-reasoning`，绝大多数 completed 样本的 `raw_output` 中不再包含独立 reasoning 内容。仍有 2 条 completed 样本出现字面量 `</think>`，需要后续结合原始 vLLM 响应确认是模型把标签写入最终 content，还是 reasoning 字段剥离仍有边界情况。当前 `EpisodeResult` 和 `model-gateway.jsonl` 没有保存 vLLM 原始 `finish_reason`，后续需要 Worker 将 vLLM 的 `finish_reason` 写回 `EpisodeResult.trajectory.steps[i].info.finish_reason`，才能准确区分 `stop`、`length` 和传输失败。

当前 UEnv driver 的 `avg_output_tokens` 为 0，是因为 Worker 返回的 `EpisodeResult` 未携带 token id 数组；该项在 UEnv 口径下暂不作为有效指标。

## 9. 输出文件

```text
temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_thinking_max32768_budget16384_full_20260718_223005/metrics.json
temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_thinking_max32768_budget16384_full_20260718_223005/predictions_official.json
temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_thinking_max32768_budget16384_full_20260718_223005/predictions.jsonl
temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_thinking_max32768_budget16384_full_20260718_223005/predictions.csv
temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_thinking_max32768_budget16384_full_20260718_223005/uenv_requests.jsonl
temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_thinking_max32768_budget16384_full_20260718_223005/uenv_results.jsonl
temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_thinking_max32768_budget16384_full_20260718_223005/full.log
temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_thinking_max32768_budget16384_full_20260718_223005/run_full_olymmath_uenv.sh
temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_reasoning_budget_20260715_111008/model-gateway-thinking-strip-reasoning-18094-budget16384.jsonl
```

## 10. 当前结论

本轮已经完成 OlymMATH 400 题 UEnv thinking 全量非 resume 评测，Adapter 侧成功记录 400 条 request 和 400 条 result，说明全量请求和结果聚合链路是闭合的。

从指标看，整体 UEnv reward accuracy 为 0.6175；如果只看 completed 样本，reward accuracy 为 0.6534。中文子集本轮表现高于英文子集：ZH reward accuracy 为 0.6700，EN reward accuracy 为 0.5650；主要差异来自 EN-EASY 中 22 条连续样本发生 Server/Worker 侧 `max attempts (3)` 失败。

因此，本轮结果可以作为“UEnv 链路全量 OlymMATH thinking 口径”的正式基线。链路层面，Adapter 侧完成 400 条 request 和 400 条 result 聚合，gateway 时间窗口内没有 HTTP error；稳定性层面，仍需要定位 22 条连续 EN-EASY failed 的 Server/Worker 原因，并补充 `finish_reason`、Worker 原始错误和 retry attempt 级日志，区分模型答案质量、输出解析和服务稳定性三类问题。
