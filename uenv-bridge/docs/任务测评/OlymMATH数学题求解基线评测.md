# OlymMATH 数学题求解 UEnv 基线评测

> 日期：2026-07-19 / 2026-07-20 失败样本补测
> 阶段：Eval-first，未进行后训练
> 任务书条目：5. 数学题求解
> Benchmark：OlymMATH-EASY / OlymMATH-HARD
> 目标模型：`Qwen/Qwen3.6-35B-A3B`
> 正式口径：接入 UEnv，thinking 开启，`MAX_TOKENS=32768`，`thinking_token_budget=16384`，`preserve_thinking=false`，`strip_reasoning=true`，全量 400 题；初始全量后对 failed 样本执行 resume 补测

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
| 断点续跑 | 初始全量 `RESUME=0`；失败样本补测 `RESUME=1` |

## 5. 运行命令

从零开始运行时，先启动 8GPU vLLM，监听本机 `18081`。由于当前 `localhost/uenv-bridge-verl:layer4-build` 内的 vLLM 版本不能识别 Qwen3.6 MoE，本轮使用 `localhost/vllm-openai:v0.19.0-cu130`：

```bash
cd /data/ronghao/uenv/uenv-bridge

BASE=/data/ronghao/uenv/uenv-bridge/temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_reasoning_budget_20260715_111008
mkdir -p "$BASE"

podman rm -f uenv-olymmath-vllm-18081 2>/dev/null || true

podman run -d --name uenv-olymmath-vllm-18081 \
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

可用下面命令确认 vLLM 已就绪：

```bash
curl --noproxy '*' http://127.0.0.1:18081/v1/models
```

在独立终端启动 Worker 可访问的 adapter model gateway，转发到本机 vLLM：

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

初始全量运行中有 22 条样本因 Server/Worker 侧 episode 重试耗尽失败，随后使用同一输出目录按 `qid` 跳过已完成样本，只重测失败样本：

```bash
cd /data/ronghao/uenv/uenv-bridge

OUT=/data/ronghao/uenv/uenv-bridge/temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_thinking_max32768_budget16384_full_20260718_223005

RESUME=1 \
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
./scripts/benchmark/run_olymmath_uenv_baseline.sh 2>&1 | tee "$OUT/resume_failed_20260720_olymmath.log"
```

本轮正式结果目录：

```text
temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_thinking_max32768_budget16384_full_20260718_223005/
```

## 6. 正式结果

| 模型 | 样本数 | requests | results | completed | failed | UEnv reward accuracy | completed-only reward accuracy | Parse rate |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| `Qwen/Qwen3.6-35B-A3B` | 400 | 400 | 400 | 400 | 0 | 0.6575 | 0.6575 | 0.9500 |

说明：

| 指标 | 说明 |
|---|---|
| `UEnv reward accuracy` | 全量 400 条样本上，Worker 返回 `EpisodeResult.summary.total_reward` 的均值 |
| `completed-only reward accuracy` | 只在 `uenv_status=completed` 的样本上计算 reward 均值 |
| `Parse rate` | Adapter 本地从 `raw_output` 中抽取到最终答案的比例；UEnv 正确性主口径仍以 Worker reward 为准 |
| `parsed accuracy` | 只在 Adapter 成功抽取最终答案的样本上计算正确率，本轮为 0.6921 |
| `requests/results` | 表中按 `qid` 取最新记录统计为 400/400；由于 resume 在同一 jsonl 中追加 22 条补测记录，原始 `uenv_requests.jsonl` 和 `uenv_results.jsonl` 各有 422 行 |

按子集：

| 子集 | 样本数 | completed | failed | UEnv reward accuracy | Parse rate |
|---|---:|---:|---:|---:|---:|
| EN-EASY | 100 | 100 | 0 | 0.7900 | 0.9800 |
| EN-HARD | 100 | 100 | 0 | 0.5000 | 0.9600 |
| ZH-EASY | 100 | 100 | 0 | 0.8000 | 0.9500 |
| ZH-HARD | 100 | 100 | 0 | 0.5400 | 0.9100 |

按语言：

| 语言 | 样本数 | completed | failed | UEnv reward accuracy | Parse rate |
|---|---:|---:|---:|---:|---:|
| EN | 200 | 200 | 0 | 0.6450 | 0.9700 |
| ZH | 200 | 200 | 0 | 0.6700 | 0.9300 |

按难度：

| 难度 | 样本数 | completed | failed | UEnv reward accuracy | Parse rate |
|---|---:|---:|---:|---:|---:|
| EASY | 200 | 200 | 0 | 0.7950 | 0.9650 |
| HARD | 200 | 200 | 0 | 0.5200 | 0.9350 |

按学科：

| 学科 | 样本数 | completed | failed | Parse rate | UEnv reward accuracy |
|---|---:|---:|---:|---:|---:|
| Algebra | 50 | 50 | 0 | 0.9800 | 0.7200 |
| Combinatorics | 54 | 54 | 0 | 0.9815 | 0.5000 |
| Geometry | 58 | 58 | 0 | 0.9828 | 0.7241 |
| Number Theory | 38 | 38 | 0 | 0.9211 | 0.6316 |
| 代数 | 50 | 50 | 0 | 0.9600 | 0.7400 |
| 几何 | 58 | 58 | 0 | 0.9310 | 0.7586 |
| 数论 | 38 | 38 | 0 | 0.8684 | 0.6316 |
| 组合 | 54 | 54 | 0 | 0.9444 | 0.5370 |

答案抽取与判分分布：

| 项 | 数量 |
|---|---:|
| `answer_phrase` 抽取 | 4 |
| `boxed` 抽取 | 376 |
| 未抽取到最终答案 | 20 |
| Worker reward 正确 | 263 |
| Worker reward 不正确 | 137 |

## 7. 运行稳定性

| 项 | 值 |
|---|---:|
| 初始全量运行时间 | 约 9 小时 26 分钟 |
| 失败样本补测运行时间 | 约 29 分 25 秒 |
| 最新 400 条样本平均 episode 耗时 | 89.11s |
| Worker 并发 / UEnv batch size | 1 |
| 补测启动时剩余失败样本 | 22 |
| 补测后 failed | 0 |
| Gateway 初始全量 `/v1/chat/completions` 调用 | 389 |
| Gateway 初始全量 `/v1/models` 调用 | 1 |
| Gateway 初始全量 HTTP 200 | 390 |
| Gateway 初始全量 error | 0 |
| Gateway 初始全量 `/v1/chat/completions` 平均 latency | 82.12s |
| completed 样本 `raw_output` 字符数均值 | 2430.10 |
| completed 样本 `raw_output` 字符数范围 | 24 - 8206 |

初始失败样本补测情况：

| 项 | 值 |
|---|---|
| 初始失败样本数 | 22 |
| 初始失败错误码 | `5001` |
| 初始错误信息 | `episode ... exceeded max attempts (3)` |
| 失败分布 | 22 条全部来自 EN-EASY，样本编号集中在 `OlymMATH-EASY-64-EN` 至 `OlymMATH-EASY-85-EN` |
| 补测方式 | `RESUME=1`，同一输出目录下按 `qid` 跳过已完成样本，只发送 failed 样本 |
| 补测结果 | 22 条全部恢复为 `completed` |
| 补测样本 reward | 16 条为 1.0，6 条为 0.0 |

因此，初始失败更像是运行期间 Server/Worker 侧 episode 重试、服务状态或调度稳定性问题，而不是模型本身无法回答这些样本。补测后 22 条均成功返回，最终 OlymMATH 正式口径使用按 `qid` 去重后的最新结果。

## 8. 输出截断与字段缺口

| 观察项 | 数量 |
|---|---:|
| completed 样本 | 400 |
| failed 空输出样本 | 0 |
| completed 样本中出现 `\boxed{}` | 376 |
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
temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_thinking_max32768_budget16384_full_20260718_223005/resume_failed_20260720_olymmath.log
temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_thinking_max32768_budget16384_full_20260718_223005/run_full_olymmath_uenv.sh
temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_reasoning_budget_20260715_111008/model-gateway-thinking-strip-reasoning-18094-budget16384.jsonl
```

## 10. 当前结论

本轮已经完成 OlymMATH 400 题 UEnv thinking 全量评测，并在初始 22 条 failed 后使用 `RESUME=1` 做失败样本补测。按 `qid` 取最新结果后，400 条样本全部为 `completed`，说明全量请求、失败补测和结果聚合链路是闭合的。

从指标看，最终整体 UEnv reward accuracy 为 0.6575，parse rate 为 0.9500。中文子集本轮表现略高于英文子集：ZH reward accuracy 为 0.6700，EN reward accuracy 为 0.6450；难度上 EASY 为 0.7950，HARD 为 0.5200。

因此，本轮结果可以作为“UEnv 链路全量 OlymMATH thinking 口径”的正式基线。稳定性层面，初始 22 条 EN-EASY 连续 failed 经补测全部恢复，后续仍建议补充 `finish_reason`、Worker 原始错误和 retry attempt 级日志，区分模型答案质量、输出解析和服务稳定性三类问题。
