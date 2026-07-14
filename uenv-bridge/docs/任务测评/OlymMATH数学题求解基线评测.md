# OlymMATH 数学题求解基线评测

> 日期：2026-07-09  
> 阶段：Eval-first，未进行后训练  
> 任务书条目：5. 数学题求解  
> Benchmark：OlymMATH-EASY / OlymMATH-HARD  
> 目标模型：`Qwen/Qwen3.6-35B-A3B`

## 1. 任务说明

OlymMATH 是奥赛级数学推理评测集。输入为自然语言数学题，模型需要完成推理并输出最终答案。官方 prompt 要求将最终答案写在：

```text
\boxed{}
```

本阶段目标是评估基准模型在该 benchmark 上的零训练表现，不进行 SFT、RL 或其他后训练。

## 2. 数据集准备

已下载 OlymMATH 官方公开数据：

| 文件 | 语言 | 难度 | 样本数 |
|---|---|---|---:|
| `data/benchmarks/olymmath/OlymMATH-EN-EASY.jsonl` | 英文 | EASY | 100 |
| `data/benchmarks/olymmath/OlymMATH-EN-HARD.jsonl` | 英文 | HARD | 100 |
| `data/benchmarks/olymmath/OlymMATH-ZH-EASY.jsonl` | 中文 | EASY | 100 |
| `data/benchmarks/olymmath/OlymMATH-ZH-HARD.jsonl` | 中文 | HARD | 100 |

每条样本包含：

| 字段 | 说明 |
|---|---|
| `problem` | 数学题题面 |
| `answer` | 标准答案 |
| `subject` | 题目学科类别 |
| `unique_id` | 样本 ID |

当前公开文件没有额外 train/dev/test split 字段。因此本阶段将四个公开 jsonl 文件作为 OlymMATH benchmark/test set 进行基线评测。

## 3. 评价指标

本次使用以下指标：

| 指标 | 说明 |
|---|---|
| Accuracy | 模型最终答案与标准答案等价的比例 |
| Parse rate | 是否能从输出中解析出 `\boxed{}` 或明确 final answer |
| Parsed accuracy | 仅在可解析样本上的准确率 |
| By difficulty accuracy | EASY / HARD 分难度准确率 |
| By language accuracy | EN / ZH 分语言准确率 |
| By subject accuracy | 分学科准确率 |
| Avg output tokens | 平均生成 token 数，用于判断是否存在长推理或截断 |

答案判分优先使用 `math-verify` 的 `parse/verify`，失败时退回 SymPy 等价判断和字符串归一化比较。

## 4. 评测实现

新增评测脚本：

```text
scripts/benchmark/evaluate_olymmath.py
```

新增运行脚本：

```text
scripts/benchmark/run_olymmath_baseline.sh
```

脚本行为：

1. 如果 OlymMATH 数据不存在，则从官方 GitHub 镜像下载四个 jsonl 文件。
2. 如果目标模型权重不存在，则通过 ModelScope 下载 `Qwen/Qwen3.6-35B-A3B`。
3. 使用 vLLM 进行 8GPU tensor parallel 推理。
4. 从模型输出中抽取 `\boxed{}` 或明确 final answer。
5. 生成 `predictions_official.json`、`predictions.jsonl`、`predictions.csv` 和 `metrics.json`。

本次记录两套评测口径：

| 配置 | Single-sample baseline | 官方对齐全量口径 |
|---|---|---|
| 后端 | `vLLM 0.19.0` | `vLLM 0.19.0` |
| GPU | 8 张 | 8 张 |
| Tensor parallel | 8 | 8 |
| `MAX_MODEL_LEN` | 16384 | 32768 |
| `MAX_TOKENS` | 8192 | 32768 |
| `TEMPERATURE` | 0.0 | 0.6 |
| `TOP_P` | 1.0 | 0.95 |
| `MIN_P` | 未设置 | 0.0 |
| 推理次数 | 每题 1 次 | 每题 10 次 |
| Prompt style | `official_no_think` | `official` |
| Thinking mode | 关闭 | 开启 |

`official_no_think` 保留官方“逐步推理并将最终答案放入 `\boxed{}`”的 prompt，但通过 tokenizer 的 `enable_thinking=False` 关闭 Qwen3.6 内部 thinking 模式。该口径用于获得稳定、低成本的 single-sample baseline。

官方对齐全量口径使用更长的 `MAX_TOKENS=32768`、多采样和 thinking 模式，更接近 OlymMATH 官方 tester 的默认设置。该口径用于观察长推理和 Pass@10 表现。

## 5. 运行命令

英文 EASY+HARD：

```bash
cd /data/ronghao/uenv/uenv-bridge

IMAGE=localhost/vllm-openai:v0.19.0-cu130 \
MODEL_ID=Qwen/Qwen3.6-35B-A3B \
MODEL_DIR=/data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B \
DATASETS=EN-EASY,EN-HARD \
OUTPUT_DIR=/data/ronghao/uenv/uenv-bridge/temp/benchmarks/olymmath/qwen3_6_35b_a3b_en_easy_hard_official_no_think \
TENSOR_PARALLEL_SIZE=8 \
MAX_MODEL_LEN=16384 \
MAX_TOKENS=8192 \
GPU_MEMORY_UTILIZATION=0.9 \
TEMPERATURE=0.0 \
TOP_P=1.0 \
PROMPT_STYLE=official_no_think \
./scripts/benchmark/run_olymmath_baseline.sh
```

中文 EASY+HARD：

```bash
cd /data/ronghao/uenv/uenv-bridge

IMAGE=localhost/vllm-openai:v0.19.0-cu130 \
MODEL_ID=Qwen/Qwen3.6-35B-A3B \
MODEL_DIR=/data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B \
DATASETS=ZH-EASY,ZH-HARD \
OUTPUT_DIR=/data/ronghao/uenv/uenv-bridge/temp/benchmarks/olymmath/qwen3_6_35b_a3b_zh_easy_hard_official_no_think \
TENSOR_PARALLEL_SIZE=8 \
MAX_MODEL_LEN=16384 \
MAX_TOKENS=8192 \
GPU_MEMORY_UTILIZATION=0.9 \
TEMPERATURE=0.0 \
TOP_P=1.0 \
PROMPT_STYLE=official_no_think \
./scripts/benchmark/run_olymmath_baseline.sh
```

官方对齐全量口径：

| 配置 | 值 |
|---|---|
| Prompt style | `official` |
| Thinking mode | 开启；未传 `enable_thinking=False` |
| `MAX_MODEL_LEN` | 32768 |
| `MAX_TOKENS` | 32768 |
| `SAMPLE` | 10 |
| `TEMPERATURE` | 0.6 |
| `TOP_P` | 0.95 |
| `MIN_P` | 0.0 |
| GPU | 8 张 |
| Tensor parallel | 8 |

四个子集分别运行：

| 子集 | `DATASETS` | `OUTPUT_DIR` | 日志 |
|---|---|---|---|
| EN-EASY | `EN-EASY` | `temp/benchmarks/olymmath/qwen3_6_35b_a3b_official_thinking_sample10_no_limit_en_easy` | `temp/benchmarks/olymmath/qwen3_6_35b_a3b_official_thinking_sample10_no_limit_en_easy.log` |
| EN-HARD | `EN-HARD` | `temp/benchmarks/olymmath/qwen3_6_35b_a3b_official_thinking_sample10_no_limit_en_hard` | `temp/benchmarks/olymmath/qwen3_6_35b_a3b_official_thinking_sample10_no_limit_en_hard.log` |
| ZH-EASY | `ZH-EASY` | `temp/benchmarks/olymmath/qwen3_6_35b_a3b_official_thinking_sample10_no_limit_zh_easy` | `temp/benchmarks/olymmath/qwen3_6_35b_a3b_official_thinking_sample10_no_limit_zh_easy.log` |
| ZH-HARD | `ZH-HARD` | `temp/benchmarks/olymmath/qwen3_6_35b_a3b_official_thinking_sample10_no_limit_zh_hard` | `temp/benchmarks/olymmath/qwen3_6_35b_a3b_official_thinking_sample10_no_limit_zh_hard.log` |

运行时将下方命令中的 `DATASETS`、`OUTPUT_DIR` 和日志路径替换为上表对应值：

```bash
cd /data/ronghao/uenv/uenv-bridge

nohup env IMAGE=localhost/vllm-openai:v0.19.0-cu130 \
MODEL_ID=Qwen/Qwen3.6-35B-A3B \
MODEL_DIR=/data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B \
DATASETS=EN-EASY \
OUTPUT_DIR=/data/ronghao/uenv/uenv-bridge/temp/benchmarks/olymmath/qwen3_6_35b_a3b_official_thinking_sample10_no_limit_en_easy \
TENSOR_PARALLEL_SIZE=8 \
MAX_MODEL_LEN=32768 \
MAX_TOKENS=32768 \
GPU_MEMORY_UTILIZATION=0.9 \
TEMPERATURE=0.6 \
TOP_P=0.95 \
MIN_P=0.0 \
PROMPT_STYLE=official \
SAMPLE=10 \
./scripts/benchmark/run_olymmath_baseline.sh > /data/ronghao/uenv/uenv-bridge/temp/benchmarks/olymmath/qwen3_6_35b_a3b_official_thinking_sample10_no_limit_en_easy.log 2>&1 &
```

## 6. 当前结果

四个公开文件共 400 条样本已完成评测。

### 6.1 Single-sample baseline

总结果：

| 模型 | 样本数 | Parse rate | Accuracy | Parsed accuracy | Avg output tokens |
|---|---:|---:|---:|---:|---:|
| `Qwen/Qwen3.6-35B-A3B` | 400 | 0.8175 | 0.5025 | 0.6147 | 5732.31 |

按语言：

| 语言 | 样本数 | Parse rate | Accuracy |
|---|---:|---:|---:|
| EN | 200 | 0.8600 | 0.5350 |
| ZH | 200 | 0.7750 | 0.4700 |

按难度：

| 难度 | 样本数 | Parse rate | Accuracy |
|---|---:|---:|---:|
| EASY | 200 | 0.8750 | 0.7250 |
| HARD | 200 | 0.7600 | 0.2800 |

英文分难度：

| 子集 | 样本数 | Parse rate | Accuracy |
|---|---:|---:|---:|
| EN-EASY | 100 | 0.9300 | 0.7700 |
| EN-HARD | 100 | 0.7900 | 0.3000 |

中文分难度：

| 子集 | 样本数 | Parse rate | Accuracy |
|---|---:|---:|---:|
| ZH-EASY | 100 | 0.8200 | 0.6800 |
| ZH-HARD | 100 | 0.7300 | 0.2600 |

英文分学科：

| 学科 | 样本数 | Parse rate | Accuracy |
|---|---:|---:|---:|
| Algebra | 50 | 0.8000 | 0.5400 |
| Combinatorics | 54 | 0.9259 | 0.5185 |
| Geometry | 58 | 0.8966 | 0.6207 |
| Number Theory | 38 | 0.7895 | 0.4211 |

中文分学科：

| 学科 | 样本数 | Parse rate | Accuracy |
|---|---:|---:|---:|
| 代数 | 50 | 0.8400 | 0.5200 |
| 几何 | 58 | 0.8448 | 0.6552 |
| 数论 | 38 | 0.7105 | 0.3684 |
| 组合 | 54 | 0.6852 | 0.2963 |

答案抽取与判分情况：

| 项 | 数量 |
|---|---:|
| `boxed` 抽取 | 296 |
| `answer_phrase` 抽取 | 31 |
| 未抽取到最终答案 | 73 |
| `math_verify` 判定正确 | 201 |
| 不匹配 | 126 |
| 缺失答案 | 73 |

输出文件：

```text
temp/benchmarks/olymmath/qwen3_6_35b_a3b_en_easy_hard_official_no_think/metrics.json
temp/benchmarks/olymmath/qwen3_6_35b_a3b_en_easy_hard_official_no_think/predictions.jsonl
temp/benchmarks/olymmath/qwen3_6_35b_a3b_zh_easy_hard_official_no_think/metrics.json
temp/benchmarks/olymmath/qwen3_6_35b_a3b_zh_easy_hard_official_no_think/predictions.jsonl
temp/benchmarks/olymmath/qwen3_6_35b_a3b_all_official_no_think/metrics.json
temp/benchmarks/olymmath/qwen3_6_35b_a3b_all_official_no_think/predictions.jsonl
```

### 6.2 官方对齐全量口径

官方对齐口径使用 `official` prompt、开启 thinking、`MAX_TOKENS=32768`，并对每题采样 10 次。四个子集共 400 题、4000 条 generation sample 已完成评测。

总结果：

| 模型 | 题目数 | Generation samples | Parse rate | Sample accuracy | Parsed accuracy | Problem parse rate | Pass@10 | Avg output tokens |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| `Qwen/Qwen3.6-35B-A3B` | 400 | 4000 | 0.7468 | 0.5128 | 0.6866 | 0.9625 | 0.7775 | 28002.33 |

按语言：

| 语言 | 题目数 | Samples | Parse rate | Sample accuracy | Parsed accuracy | Problem parse rate | Pass@10 | Avg output tokens |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| EN | 200 | 2000 | 0.7550 | 0.5230 | 0.6927 | 0.9650 | 0.8000 | 28781.47 |
| ZH | 200 | 2000 | 0.7385 | 0.5025 | 0.6804 | 0.9600 | 0.7550 | 27223.18 |

按难度：

| 难度 | 题目数 | Samples | Parse rate | Sample accuracy | Parsed accuracy | Problem parse rate | Pass@10 | Avg output tokens |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| EASY | 200 | 2000 | 0.8625 | 0.7415 | 0.8597 | 0.9850 | 0.9550 | 25603.49 |
| HARD | 200 | 2000 | 0.6310 | 0.2840 | 0.4501 | 0.9400 | 0.6000 | 30401.16 |

按子集：

| 子集 | 题目数 | Samples | Parse rate | Sample accuracy | Parsed accuracy | Problem parse rate | Pass@10 | Consensus accuracy | Avg output tokens | Max output tokens |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| EN-EASY | 100 | 1000 | 0.8690 | 0.7550 | 0.8688 | 0.9800 | 0.9700 | 0.8600 | 26421.22 | 32699 |
| EN-HARD | 100 | 1000 | 0.6410 | 0.2910 | 0.4540 | 0.9500 | 0.6300 | 0.4500 | 31141.72 | 32700 |
| ZH-EASY | 100 | 1000 | 0.8560 | 0.7280 | 0.8505 | 0.9900 | 0.9400 | 0.8400 | 24785.77 | 32706 |
| ZH-HARD | 100 | 1000 | 0.6210 | 0.2770 | 0.4461 | 0.9300 | 0.5700 | 0.4300 | 29660.60 | 32701 |

英文分学科：

| 学科 | 题目数 | Samples | Parse rate | Sample accuracy | Pass@10 |
|---|---:|---:|---:|---:|---:|
| Algebra | 50 | 500 | 0.7780 | 0.5940 | 0.9200 |
| Combinatorics | 54 | 540 | 0.7333 | 0.4444 | 0.7407 |
| Geometry | 58 | 580 | 0.8448 | 0.6052 | 0.8448 |
| Number Theory | 38 | 380 | 0.6184 | 0.4158 | 0.6579 |

中文分学科：

| 学科 | 题目数 | Samples | Parse rate | Sample accuracy | Pass@10 |
|---|---:|---:|---:|---:|---:|
| 代数 | 50 | 500 | 0.7560 | 0.5520 | 0.7800 |
| 几何 | 58 | 580 | 0.8362 | 0.6069 | 0.8793 |
| 数论 | 38 | 380 | 0.5947 | 0.4184 | 0.6579 |
| 组合 | 54 | 540 | 0.7185 | 0.4037 | 0.6667 |

输出文件与日志：

```text
temp/benchmarks/olymmath/qwen3_6_35b_a3b_official_thinking_sample10_no_limit_en_easy/metrics.json
temp/benchmarks/olymmath/qwen3_6_35b_a3b_official_thinking_sample10_no_limit_en_hard/metrics.json
temp/benchmarks/olymmath/qwen3_6_35b_a3b_official_thinking_sample10_no_limit_zh_easy/metrics.json
temp/benchmarks/olymmath/qwen3_6_35b_a3b_official_thinking_sample10_no_limit_zh_hard/metrics.json
temp/benchmarks/olymmath/qwen3_6_35b_a3b_official_thinking_sample10_no_limit_en_easy.log
temp/benchmarks/olymmath/qwen3_6_35b_a3b_official_thinking_sample10_no_limit_en_hard.log
temp/benchmarks/olymmath/qwen3_6_35b_a3b_official_thinking_sample10_no_limit_zh_easy.log
temp/benchmarks/olymmath/qwen3_6_35b_a3b_official_thinking_sample10_no_limit_zh_hard.log
```

## 7. UEnv 环境口径

按照 Worker 侧五类 benchmark 文档，OlymMATH 复用 `math` 环境：

| 字段 | 值 | 说明 |
|---|---|---|
| `env_type` | `math` | 由 Server 调度到 math Worker / plugin |
| `env_config.dataset` | `olymmath-easy` / `olymmath-hard` | 按样本 difficulty 显式设置 |
| `reward_config.target` | 官方 `answer` | Worker 使用 OlymMATH backend 抽取并判分 |
| `model_endpoint.url` | adapter gateway `http://10.10.20.142:18088/v1` | Worker 调用冻结模型生成答案 |

本次 UEnv 口径包含两类已完成评测，以及一次 thinking 长输出口径的真实联调尝试：

| 口径 | 样本 | 结果 | 说明 |
|---|---:|---|---|
| `official_no_think` + `MAX_TOKENS=1024` | EN-EASY 4 条 | completed 4/4；parse rate 0；reward accuracy 0.25 | 单题约 66-68 秒，输出长推理且被截断，不适合作为 Worker 串行全量口径 |
| `boxed_no_think` + `MAX_TOKENS=256` | EN/ZH EASY/HARD 400 条 | completed 400/400；parse rate 0.875；reward accuracy 0.1325 | 强制短答案，适合验证 UEnv 全链路可跑，但不代表官方长推理能力 |
| `official` + thinking + `MAX_TOKENS=512` | EN-EASY 1 条 | completed 1/1；parse rate 0；reward accuracy 0 | 模型保留 reasoning，但 512 token 内没有输出可解析最终答案 |
| `official` + thinking + `MAX_TOKENS=2048` | EN-EASY 1 条请求 | Adapter 已发出请求；gateway 收到 4 次 `/v1/chat/completions`，每次约 134-138 秒；client 持续等待 `ExecuteBatch` 返回，未生成 `uenv_results.jsonl` | 进一步放大 token 后，当前阻塞在 server/worker 返回 EpisodeResult 阶段，未得到可计算 Accuracy 的完整结果 |

UEnv `boxed_no_think` 全量结果：

| 模型 | AdapterCore endpoint | 样本数 | completed | Parse rate | Accuracy / reward accuracy | Parsed accuracy |
|---|---|---:|---:|---:|---:|---:|
| `Qwen/Qwen3.6-35B-A3B` | `8.130.75.157:8088` | 400 | 400 | 0.8750 | 0.1325 | 0.1514 |

按语言：

| 语言 | 样本数 | Parse rate | Accuracy |
|---|---:|---:|---:|
| EN | 200 | 0.9900 | 0.1300 |
| ZH | 200 | 0.7600 | 0.1350 |

按难度：

| 难度 | 样本数 | Parse rate | Accuracy |
|---|---:|---:|---:|
| EASY | 200 | 0.8850 | 0.1300 |
| HARD | 200 | 0.8650 | 0.1350 |

按抽取方式：

| 抽取方式 | 数量 |
|---|---:|
| `boxed` | 350 |
| `missing` | 50 |

UEnv 输出文件：

```text
temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_gateway_boxed_full/metrics.json
temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_gateway_boxed_full/predictions_official.json
temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_gateway_boxed_full/predictions.jsonl
temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_gateway_boxed_full/predictions.csv
temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_gateway_boxed_full/uenv_requests.jsonl
temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_gateway_boxed_full/uenv_results.jsonl
```

UEnv thinking 尝试证据：

```text
temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_thinking_official_max512_en_easy_1/metrics.json
temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_thinking_official_max512_en_easy_1/uenv_requests.jsonl
temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_thinking_official_max512_en_easy_1/uenv_results.jsonl
temp/benchmarks/uenv_thinking_gateway/model-gateway-max512.jsonl
temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_thinking_official_max2048_en_easy_2/uenv_requests.jsonl
temp/benchmarks/uenv_thinking_gateway/model-gateway-rerun.jsonl
```

其中 `MAX_TOKENS=512` 口径已生成完整 `EpisodeResult`：

| 模型 | AdapterCore endpoint | 样本数 | completed | Parse rate | Accuracy / reward accuracy | elapsed |
|---|---|---:|---:|---:|---:|---:|
| `Qwen/Qwen3.6-35B-A3B` | `8.130.75.157:8088` | 1 | 1 | 0.0000 | 0.0000 | 42.60s |

`MAX_TOKENS=2048` 口径中，`uenv_requests.jsonl` 已记录 1 条 `olymmath-easy` 请求，`model-gateway-rerun.jsonl` 记录到 4 次 `/v1/chat/completions` 转发成功：

| 请求阶段 | 次数 | 单次 vLLM latency |
|---|---:|---:|
| `/v1/chat/completions` | 4 | 134.42s、134.50s、138.16s、138.46s |

这说明本地 adapter gateway 与 vLLM 能收到 Worker 的模型调用；问题不在 model endpoint 可达性，而是长 thinking 输出在当前 Worker/Server 串行评测路径下没有及时形成 `EpisodeResult` 返回。

UEnv 运行命令：

```bash
cd /data/ronghao/uenv/uenv-bridge

IMAGE=localhost/uenv-bridge-verl:layer4-build \
UENV_ADAPTER_CORE_ENDPOINT=8.130.75.157:8088 \
UENV_ROLLOUT_MODEL_ENDPOINT=http://10.10.20.142:18088/v1 \
UENV_ROLLOUT_MODEL_NAME=Qwen/Qwen3.6-35B-A3B \
DATASETS=EN-EASY,EN-HARD,ZH-EASY,ZH-HARD \
OUTPUT_DIR=/data/ronghao/uenv/uenv-bridge/temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_gateway_boxed_full \
BATCH_SIZE=1 \
PROMPT_STYLE=boxed_no_think \
MAX_TOKENS=256 \
./scripts/benchmark/run_olymmath_uenv_baseline.sh
```

UEnv thinking 尝试命令如下。与 no-thinking 口径相比，gateway 启动时不传 `--disable-thinking`，并使用 `PROMPT_STYLE=official`、`MAX_TOKENS=512`。

```bash
cd /data/ronghao/uenv/uenv-bridge

mkdir -p temp/benchmarks/uenv_thinking_gateway

nohup env PYTHONPATH=src \
scripts/benchmark/run_model_gateway.py \
  --upstream http://127.0.0.1:18080/v1 \
  --bind-host 0.0.0.0 \
  --port 18088 \
  --public-url http://10.10.20.142:18088/v1 \
  --log-path temp/benchmarks/uenv_thinking_gateway/model-gateway-max512.jsonl \
  > temp/benchmarks/uenv_thinking_gateway/model-gateway-max512.out 2>&1 &

PYTHONPATH=src \
python3 scripts/benchmark/evaluate_olymmath_uenv.py \
  --data-dir /data/ronghao/uenv/uenv-bridge/data/benchmarks/olymmath \
  --datasets EN-EASY \
  --output-dir /data/ronghao/uenv/uenv-bridge/temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_thinking_official_max512_en_easy_1 \
  --endpoint 8.130.75.157:8088 \
  --model-endpoint http://10.10.20.142:18088/v1 \
  --model-name Qwen/Qwen3.6-35B-A3B \
  --limit 1 \
  --batch-size 1 \
  --prompt-style official \
  --max-tokens 512 \
  --temperature 0.0 \
  --top-p 1.0 \
  --timeout-seconds 1200 \
  --client-timeout-seconds 1600
```

说明：当前 UEnv driver 的 `avg_output_tokens` 为 0，是因为 Worker 返回的 `EpisodeResult` 未携带 token id 数组；该项在 UEnv 口径下暂不作为有效指标。

## 8. 结果分析

Single-sample baseline 中，当前模型在 OlymMATH 上表现出明显的难度差异：EASY 准确率为 72.50%，HARD 准确率为 28.00%。这说明基准模型已经具备一定奥赛题求解能力，但对高难度题仍有明显提升空间。

语言维度上，英文准确率为 53.50%，中文准确率为 47.00%。中文 parse rate 更低，说明中文 prompt 下模型更容易长推理或未能在 `MAX_TOKENS=8192` 内给出明确 boxed 答案。

格式方面，400 条中有 327 条可解析，parse rate 为 81.75%，未达到任务书中“输出可解析率 ≥ 90%”的直接进入 RL gate。主要原因是仍有 73 条没有抽取到最终答案，很多样本生成长度接近或达到 8192 tokens。后续如果继续优化 baseline，可以尝试 `MAX_TOKENS=32768` 或更强的终止/格式约束。

官方对齐全量口径中，thinking + `MAX_TOKENS=32768` 显著拉长了生成：4000 条 sample 的平均输出长度为 28002.33 tokens，远高于 single-sample baseline 的 5732.31 tokens。长输出使 problem-level parse rate 提升到 96.25%，但 sample-level parse rate 仍为 74.68%，说明部分采样仍然无法稳定给出可解析最终答案。

多采样后，sample-level accuracy 为 51.28%，与 single-sample baseline 的 50.25% 接近；但 problem-level Pass@10 达到 77.75%。这说明在每题 10 次采样中，模型经常至少有一次能答对，但单次采样稳定性和答案一致性仍不足。EASY 的 Pass@10 为 95.50%，HARD 为 60.00%，高难度题仍是主要瓶颈。

UEnv 口径下，`boxed_no_think` 能稳定完成 400 条全链路评测，证明 AdapterCore/Server/Worker/model gateway 的 OlymMATH 路径已经打通；但该口径关闭长推理且强制短答案，准确率只有 13.25%，因此只能作为 UEnv 链路回归口径。thinking + `MAX_TOKENS=512` 能完整返回 EpisodeResult，但输出被截断且没有最终答案；thinking + `MAX_TOKENS=2048` 已经证明 Worker 能访问 gateway 并触发更长 vLLM 生成，但 EpisodeResult 没有返回到本地 client。在正式采用 UEnv 运行 OlymMATH 长推理前，需要继续定位 server/worker 对长输出的判分、重试或返回逻辑。

## 9. 当前结论

本阶段已经跑通数学题求解任务的完整基线评测链路：数据下载、8GPU vLLM 推理、答案抽取、数学等价判分、分语言/难度/学科统计和结果落盘均已完成。同时，官方对齐口径的 400 题 × 10 采样全量评测也已完成。

当前基准模型在 OlymMATH EN/ZH EASY/HARD 400 条公开样本上的 single-sample baseline 主结果为：

```text
Accuracy: 50.25%
Parse rate: 81.75%
Parsed accuracy: 61.47%
```

官方对齐全量口径结果为：

```text
Sample-level accuracy: 51.28%
Parse rate: 74.68%
Parsed accuracy: 68.66%
Problem-level Pass@10: 77.75%
Problem parse rate: 96.25%
```

后续如果进入训练阶段，建议优先解决两个问题：一是提升最终答案格式稳定性，使 `\boxed{}` 输出率达到 90% 以上；二是针对 HARD 和组合/数论题增加 verifier-driven rejection SFT 或 RLVR 训练样本。
