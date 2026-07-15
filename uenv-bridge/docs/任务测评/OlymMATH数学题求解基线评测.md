# OlymMATH 数学题求解 UEnv 基线评测

> 日期：2026-07-15
> 阶段：Eval-first，未进行后训练
> 任务书条目：5. 数学题求解
> Benchmark：OlymMATH-EASY / OlymMATH-HARD
> 目标模型：`Qwen/Qwen3.6-35B-A3B`
> 正式口径：接入 UEnv，thinking 开启，`MAX_TOKENS=32768`，`thinking_token_budget=16384`，全量 400 题

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
| `model_endpoint.url` | `http://10.10.20.142:18092/v1` | Worker 访问 adapter model gateway |
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
| Adapter model gateway | `http://10.10.20.142:18092/v1` |
| Gateway upstream | `http://127.0.0.1:18081/v1` |
| Gateway request timeout | 7200s |
| Gateway thinking 注入 | `--enable-thinking --preserve-thinking --thinking-token-budget 16384` |
| AdapterCore endpoint | `8.130.75.157:8088` |
| UEnv batch size | 1 |
| Prompt style | `official` |
| Thinking mode | 开启 |
| `MAX_TOKENS` | 32768 |
| `THINKING_TOKEN_BUDGET` | 16384 |
| `TEMPERATURE` | 0.0 |
| `TOP_P` | 1.0 |
| 数据集 | EN-EASY、EN-HARD、ZH-EASY、ZH-HARD |

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
  --port 18092 \
  --public-url http://10.10.20.142:18092/v1 \
  --request-timeout-seconds 7200 \
  --enable-thinking \
  --preserve-thinking \
  --thinking-token-budget 16384 \
  --log-path "$BASE/model-gateway-reasoning-content-18092-budget16384.jsonl"
```

可用下面命令确认 gateway 已就绪：

```bash
curl --noproxy '*' http://127.0.0.1:18092/v1/models
```

通过 UEnv 跑全量 OlymMATH：

```bash
cd /data/ronghao/uenv/uenv-bridge

BASE=/data/ronghao/uenv/uenv-bridge/temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_reasoning_budget_20260715_111008
OUT="$BASE/full_seq_reasoning_content_budget16384_$(date +%Y%m%d_%H%M%S)"
mkdir -p "$OUT"

OUTPUT_DIR="$OUT" \
UENV_ADAPTER_CORE_ENDPOINT=8.130.75.157:8088 \
UENV_ROLLOUT_MODEL_ENDPOINT=http://10.10.20.142:18092/v1 \
UENV_ROLLOUT_MODEL_NAME=Qwen/Qwen3.6-35B-A3B \
DATASETS=EN-EASY,EN-HARD,ZH-EASY,ZH-HARD \
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
./scripts/benchmark/run_olymmath_uenv_baseline.sh 2>&1 | tee "$OUT/run.log"
```

本轮正式结果目录按启动时间生成，例如：

```text
temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_reasoning_budget_20260715_111008/full_seq_reasoning_content_budget16384_YYYYmmdd_HHMMSS/
```

## 6. 正式结果

| 模型 | 样本数 | requests | results | completed | failed | UEnv reward accuracy | completed-only reward accuracy | Parse rate |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| `Qwen/Qwen3.6-35B-A3B` | 400 |  |  |  |  |  |  |  |

说明：

| 指标 | 说明 |
|---|---|
| `UEnv reward accuracy` | 全量 400 条样本上，Worker 返回 `EpisodeResult.summary.total_reward` 的均值 |
| `completed-only reward accuracy` | 只在 `status=completed` 的样本上计算 reward 均值 |
| `Parse rate` | Adapter 本地从 `raw_output` 中抽取到最终答案的比例；UEnv 正确性主口径仍以 Worker reward 为准 |

按子集：

| 子集 | 样本数 | completed | failed | UEnv reward accuracy | Parse rate |
|---|---:|---:|---:|---:|---:|
| EN-EASY | 100 |  |  |  |  |
| EN-HARD | 100 |  |  |  |  |
| ZH-EASY | 100 |  |  |  |  |
| ZH-HARD | 100 |  |  |  |  |

按语言：

| 语言 | 样本数 | completed | failed | UEnv reward accuracy | Parse rate |
|---|---:|---:|---:|---:|---:|
| EN | 200 |  |  |  |  |
| ZH | 200 |  |  |  |  |

按难度：

| 难度 | 样本数 | completed | failed | UEnv reward accuracy | Parse rate |
|---|---:|---:|---:|---:|---:|
| EASY | 200 |  |  |  |  |
| HARD | 200 |  |  |  |  |

按学科：

| 学科 | 样本数 | Parse rate | UEnv reward accuracy |
|---|---:|---:|---:|
| Algebra | 50 |  |  |
| Combinatorics | 54 |  |  |
| Geometry | 58 |  |  |
| Number Theory | 38 |  |  |
| 代数 | 50 |  |  |
| 几何 | 58 |  |  |
| 数论 | 38 |  |  |
| 组合 | 54 |  |  |

答案抽取与判分分布：

| 项 | 数量 |
|---|---:|
| `answer_phrase` 抽取 |  |
| `boxed` 抽取 |  |
| 未抽取到最终答案 |  |
| Worker reward 正确 |  |
| Worker reward 不正确 |  |

## 7. 运行稳定性

| 项 | 值 |
|---|---:|
| 总运行时间 |  |
| 平均 episode 耗时 |  |
| Worker 并发 / UEnv batch size | 1 |
| Gateway `/v1/chat/completions` 调用 |  |
| Gateway error |  |
| completed 样本 `raw_output` 字符数均值 |  |
| completed 样本 `raw_output` 字符数范围 |  |

失败情况：

| 项 | 值 |
|---|---|
| 失败样本数 |  |
| 失败错误码 |  |
| 本地结果中的错误信息 |  |
| 远端 AdapterCore 日志中的主要原因 |  |
| 失败分布 |  |

本节等待全量运行完成后填写。

## 8. 输出截断与字段缺口

| 观察项 | 数量 |
|---|---:|
| completed 样本 |  |
| failed 空输出样本 |  |
| completed 样本中出现 `\boxed{}` |  |
| completed 样本中没有 `</think>` |  |

当前 `EpisodeResult` 和 `model-gateway.jsonl` 没有保存 vLLM 原始 `finish_reason`。因此不能仅凭是否出现 `</think>` 判断是否真实被 `MAX_TOKENS=32768` 截断。后续需要 Worker 将 vLLM 的 `finish_reason` 写回 `EpisodeResult.trajectory.steps[i].info.finish_reason`，才能准确区分 `stop`、`length` 和传输失败。

当前 UEnv driver 的 `avg_output_tokens` 为 0，是因为 Worker 返回的 `EpisodeResult` 未携带 token id 数组；该项在 UEnv 口径下暂不作为有效指标。

## 9. 输出文件

```text
temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_reasoning_budget_20260715_111008/full_seq_reasoning_content_budget16384_YYYYmmdd_HHMMSS/metrics.json
temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_reasoning_budget_20260715_111008/full_seq_reasoning_content_budget16384_YYYYmmdd_HHMMSS/predictions_official.json
temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_reasoning_budget_20260715_111008/full_seq_reasoning_content_budget16384_YYYYmmdd_HHMMSS/predictions.jsonl
temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_reasoning_budget_20260715_111008/full_seq_reasoning_content_budget16384_YYYYmmdd_HHMMSS/predictions.csv
temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_reasoning_budget_20260715_111008/full_seq_reasoning_content_budget16384_YYYYmmdd_HHMMSS/uenv_requests.jsonl
temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_reasoning_budget_20260715_111008/full_seq_reasoning_content_budget16384_YYYYmmdd_HHMMSS/uenv_results.jsonl
temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_reasoning_budget_20260715_111008/full_seq_reasoning_content_budget16384_YYYYmmdd_HHMMSS/run.log
temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_reasoning_budget_20260715_111008/model-gateway-reasoning-content-18092-budget16384.jsonl
```

## 10. 当前结论

本节等待全量运行完成后填写。
