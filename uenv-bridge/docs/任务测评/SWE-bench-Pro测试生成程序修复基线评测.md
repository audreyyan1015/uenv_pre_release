# SWE-bench-Pro 测试生成/程序修复 UEnv 基线评测

> 日期：2026-07-24
> 阶段：Eval-first，未进行后训练
> 任务书条目：4. 测试生成/程序修复
> Benchmark：SWE-bench-Pro public test split
> 目标模型：`Qwen/Qwen3.6-35B-A3B`
> 正式口径：接入 UEnv，Worker SWE 环境 + OpenHands Agent 执行，`MAX_TOKENS=8192`，`THINKING_TOKEN_BUDGET=4096`，`workspace_dir=/app`，vLLM `max_model_len=131072`

## 1. 任务说明

SWE-bench-Pro 是长程软件工程任务评测。每条样本给定一个真实代码仓库、base commit、issue 描述、需求说明、接口信息和测试集合，模型需要通过 OpenHands 工具调用生成修复 patch，并由 Worker/Agent 侧执行官方测试，最终返回 `resolved`。

本阶段不进行 SFT、RL 或其他后训练，只验证基准模型通过 UEnv 链路执行 SWE-bench-Pro 的基线表现。

## 2. 数据集

数据来源：

```text
ScaleAI/SWE-bench_Pro
split: test
```

本地数据：

```text
data/benchmarks/swebenchpro/test.jsonl
data/benchmarks/swebenchpro/swe_bench_pro_full.csv
data/benchmarks/swebenchpro/dataset_summary.json
```

样本总数：731。

| 语言 | 样本数 |
|---|---:|
| Python | 266 |
| Go | 280 |
| JavaScript | 165 |
| TypeScript | 20 |

## 3. UEnv 评测链路

```text
SWE-bench-Pro 样本
  -> Adapter 构造 EpisodeRequest
  -> Adapter Core / Server
  -> Worker SWE 环境
  -> OpenHands Agent pool
  -> Agent 使用 llm_config 访问 Adapter Model Gateway
  -> OpenHands 修改目标 repo 并生成 patch
  -> Worker 执行官方 fail-to-pass / pass-to-pass 测试
  -> EpisodeResult 返回 Adapter
  -> Adapter 汇总 resolved / status / error
```

Adapter 侧 request 中主要传入 `instance_id`、`repo`、`base_commit`、`dockerhub_tag`、`env_package_id`、`workspace_dir`、`driver_entrypoint`、`llm_config_path` 等字段。`model_endpoint.url` 设置为 Worker 可访问的 Adapter Model Gateway；OpenHands Agent 侧根据 `LLM_CONFIG_PATH` 访问模型，因此该配置文件里的 `base_url` 也需要指向同一个 gateway。

最终联通链路是 Worker 本地 `127.0.0.1:18194` 通过反向 SSH 转发到 Adapter 本地 `127.0.0.1:18094`，再由 Adapter Model Gateway 转发到 vLLM `127.0.0.1:18081`。

## 4. UEnv 全量配置

| 配置 | 值 |
|---|---|
| Adapter 运行脚本 | `scripts/benchmark/run_swebenchpro_uenv_baseline.sh` |
| Adapter Core endpoint | `8.130.75.157:8088` |
| 数据集 | `data/benchmarks/swebenchpro/test.jsonl` |
| 样本数 | 731 |
| 模型服务 | Adapter 侧 vLLM + Adapter Model Gateway |
| vLLM endpoint | `http://127.0.0.1:18081/v1` |
| vLLM 端口 | `18081` |
| vLLM `max_model_len` | 131072 |
| Adapter Model Gateway public URL | `http://10.10.20.142:18094/v1` |
| Gateway upstream | `http://127.0.0.1:18081/v1` |
| Worker 侧 LLM config base_url | `http://127.0.0.1:18194/v1` |
| UEnv batch size | 1 |
| `MAX_TOKENS` | 8192 |
| `THINKING_TOKEN_BUDGET` | 4096 |
| Gateway `strip_reasoning` | true |
| `TEMPERATURE` | 0.0 |
| `TOP_P` | 1.0 |
| Episode timeout | 7200s |
| Client timeout | 7600s |
| Benchmark variant | `pro` |
| Command mode | `full_shell` |
| Env package | `swe-bench-pro@0.3.4` |
| Agent bridge | `uenv-agent-openhands@1.0.0` |
| Agent pool | `openhands-default` |
| Driver entrypoint | `run_swebenchpro_official.py` |
| Workspace dir | `/app` |
| OpenHands LLM config | `/root/UEnv/config/openhands-llm-qwen3-thinking-max-token-8192.json` |
| Max iterations | 60 |
| 输出目录 | `temp/benchmarks/swebenchpro/qwen3_6_35b_a3b_uenv_full_131k_iter60_budget4096_20260722_214724/` |

OpenHands LLM 配置文件位于 Worker/Agent 侧本地环境中。本轮通过本地 SSH 隧道 `127.0.0.1:18194 -> 127.0.0.1:18094` 访问 Adapter Model Gateway，因此该配置文件中的 `base_url` 使用 Agent 侧本地地址：

```json
{
  "model": "openai/Qwen/Qwen3.6-35B-A3B",
  "base_url": "http://127.0.0.1:18194/v1",
  "api_key": "EMPTY",
  "temperature": 0.0,
  "top_p": 1.0,
  "max_output_tokens": 8192,
  "timeout": 7200,
  "request_timeout": 7200,
  "num_retries": 2
}
```

SWE-bench-Pro 和其他四类 benchmark 的启动流程保持一致：先启动 vLLM，再启动 Adapter Model Gateway，最后运行 UEnv 评测脚本。区别只在于 SWE 的模型调用由 OpenHands Agent 根据 `LLM_CONFIG_PATH` 发起，因此这个 config 文件必须指向 Agent 侧可访问的同一个 gateway。

## 5. 运行命令

从零开始运行时，先启动 8GPU vLLM，监听本机 `18081`：

```bash
cd /data/ronghao/uenv/uenv-bridge

podman rm -f uenv-swebenchpro-vllm-18081 2>/dev/null || true

podman run -d --name uenv-swebenchpro-vllm-18081 \
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
  --max-model-len 131072 \
  --max-num-seqs 1 \
  --max-num-batched-tokens 131072 \
  --gpu-memory-utilization 0.95 \
  --enable-auto-tool-choice \
  --tool-call-parser qwen3_xml \
  --reasoning-parser qwen3 \
  --trust-remote-code
```

可用下面命令确认 vLLM 已就绪：

```bash
curl --noproxy '*' http://127.0.0.1:18081/v1/models
```

在独立终端启动 Worker/OpenHands 可访问的 adapter model gateway，转发到本机 vLLM：

```bash
cd /data/ronghao/uenv/uenv-bridge

BASE=/data/ronghao/uenv/uenv-bridge/temp/benchmarks/swebenchpro/qwen3_6_35b_a3b_uenv_full_131k_iter60_budget4096_20260722_214724
mkdir -p "$BASE"

PYTHONPATH=src python3 scripts/benchmark/run_model_gateway.py \
  --upstream http://127.0.0.1:18081/v1 \
  --bind-host 0.0.0.0 \
  --port 18094 \
  --public-url http://10.10.20.142:18094/v1 \
  --request-timeout-seconds 7200 \
  --enable-thinking \
  --thinking-token-budget 4096 \
  --strip-reasoning \
  --log-path "$BASE/model-gateway-18094-swe-budget4096.jsonl"
```

可用下面命令确认 gateway 已就绪：

```bash
curl --noproxy '*' http://127.0.0.1:18094/v1/models
```

通过 UEnv 跑 SWE-bench-Pro 全量任务：

```bash
cd /data/ronghao/uenv/uenv-bridge

OUT=/data/ronghao/uenv/uenv-bridge/temp/benchmarks/swebenchpro/qwen3_6_35b_a3b_uenv_full_131k_iter60_budget4096_20260722_214724
mkdir -p "$OUT"

nohup env \
REPO_DIR=/data/ronghao/uenv/uenv-bridge \
DATA_PATH=/data/ronghao/uenv/uenv-bridge/data/benchmarks/swebenchpro/test.jsonl \
OUTPUT_DIR="$OUT" \
UENV_ADAPTER_CORE_ENDPOINT=8.130.75.157:8088 \
UENV_ROLLOUT_MODEL_ENDPOINT=http://127.0.0.1:18194/v1 \
UENV_ROLLOUT_MODEL_NAME=Qwen/Qwen3.6-35B-A3B \
LIMIT= \
BATCH_SIZE=1 \
MAX_TOKENS=8192 \
THINKING_TOKEN_BUDGET=4096 \
TEMPERATURE=0.0 \
TOP_P=1.0 \
TIMEOUT_SECONDS=7200 \
CLIENT_TIMEOUT_SECONDS=7600 \
BENCHMARK_VARIANT=pro \
COMMAND_MODE=full_shell \
ENV_PACKAGE_ID=swe-bench-pro \
ENV_PACKAGE_VERSION=0.3.4 \
AGENT_BRIDGE_ID=uenv-agent-openhands \
AGENT_BRIDGE_VERSION=1.0.0 \
AGENT_POOL_ID=openhands-default \
DRIVER_ENTRYPOINT=run_swebenchpro_official.py \
WORKSPACE_DIR=/app \
LLM_CONFIG_PATH=/root/UEnv/config/openhands-llm-qwen3-thinking-max-token-8192.json \
MAX_ITERATIONS=60 \
RESUME=0 \
./scripts/benchmark/run_swebenchpro_uenv_baseline.sh \
> "$OUT/full-run.log" 2>&1 &

echo $! > "$OUT/full-run.pid"
```

查看运行进度：

```bash
tail -f /data/ronghao/uenv/uenv-bridge/temp/benchmarks/swebenchpro/qwen3_6_35b_a3b_uenv_full_131k_iter60_budget4096_20260722_214724/full-run.log
```

汇总当前结果：

```bash
python3 - <<'PY'
from pathlib import Path
import collections
import json

out = Path("/data/ronghao/uenv/uenv-bridge/temp/benchmarks/swebenchpro/qwen3_6_35b_a3b_uenv_full_131k_iter60_budget4096_20260722_214724")
rows = [json.loads(line) for line in (out / "uenv_results.jsonl").open(encoding="utf-8") if line.strip()]

print("results", len(rows))
print("status", dict(collections.Counter(row.get("uenv_status") for row in rows)))
print("resolved", dict(collections.Counter(str(row.get("resolved")) for row in rows)))
PY
```

## 6. 当前结果

截至当前结果文件：

```text
temp/benchmarks/swebenchpro/qwen3_6_35b_a3b_uenv_full_131k_iter60_budget4096_20260722_214724/uenv_results.jsonl
```

同目录下还保留 `metrics.json`、`official_like_results.json`、`uenv_predictions.jsonl`、`uenv_predictions.csv`、`uenv_requests.jsonl` 和 `model-gateway-18094-swe-budget4096.jsonl`。

整体统计如下：

| 指标 | 值 |
|---|---:|
| 数据集样本数 | 731 |
| 已返回 result | 731 |
| `uenv_status=completed` | 675 |
| `uenv_status=failed` | 56 |
| `resolved=true` | 106 |
| `resolved=false` | 625 |
| resolved rate | 14.50% |
| completed 内 resolved rate | 15.70% |
| completed 中非空 diff | 630 |
| completed 中空 diff | 45 |
| 全量执行总耗时 | 35h29m27s |
| 平均耗时 | 174.78s/条 |

completed 样本的测试结果分布：

| 类型 | 数量 | 说明 |
|---|---:|---|
| `resolved=true` | 106 | 官方测试通过 |
| partial pass | 120 | `tests_passed > 0`，但未达到 resolved |
| zero pass | 449 | `tests_passed = 0` |

失败样本的错误分布：

| 错误类型 | 数量 | 说明 |
|---|---:|---|
| patch 应用失败 | 45 | 全部集中在 `protonmail/webclients`，Worker/OpenHands 在提交 patch 到目标仓库时失败 |
| timeout | 8 | 全部集中在 `gravitational/teleport` |
| `ContextWindowExceededError` | 3 | `gravitational/teleport`、`future-architect/vuls`、`element-hq/element-web` 各 1 条 |

按语言看，resolved 分布如下：

| 语言 | 样本数 | completed | failed | resolved | resolved rate |
|---|---:|---:|---:|---:|---:|
| Go | 280 | 270 | 10 | 0 | 0.00% |
| Python | 266 | 266 | 0 | 94 | 35.34% |
| JavaScript | 165 | 119 | 46 | 12 | 7.27% |
| TypeScript | 20 | 20 | 0 | 0 | 0.00% |

按仓库看，resolved 分布如下：

| 仓库 | 样本数 | completed | failed | resolved | resolved rate |
|---|---:|---:|---:|---:|---:|
| `ansible/ansible` | 96 | 96 | 0 | 20 | 20.83% |
| `internetarchive/openlibrary` | 91 | 91 | 0 | 57 | 62.64% |
| `flipt-io/flipt` | 85 | 85 | 0 | 0 | 0.00% |
| `qutebrowser/qutebrowser` | 79 | 79 | 0 | 17 | 21.52% |
| `gravitational/teleport` | 76 | 67 | 9 | 0 | 0.00% |
| `protonmail/webclients` | 65 | 20 | 45 | 0 | 0.00% |
| `future-architect/vuls` | 62 | 61 | 1 | 0 | 0.00% |
| `navidrome/navidrome` | 57 | 57 | 0 | 0 | 0.00% |
| `element-hq/element-web` | 56 | 55 | 1 | 0 | 0.00% |
| `NodeBB/NodeBB` | 44 | 44 | 0 | 12 | 27.27% |
| `tutao/tutanota` | 20 | 20 | 0 | 0 | 0.00% |

Gateway 观测数据如下：

| 指标 | 值 |
|---|---:|
| `POST /v1/chat/completions` | 25292 |
| `GET /v1/models` | 1 |
| 平均延迟 | 3034.25 ms |
| P90 延迟 | 5001.33 ms |
| 最大延迟 | 50609.04 ms |
| 平均请求体大小 | 128090.37 bytes |
| P90 请求体大小 | 224588 bytes |
| 最大请求体大小 | 458826 bytes |
| Gateway 400 错误 | 3 |

这 3 次 400 都对应 `ContextWindowExceededError` 请求。

## 7. 当前结论

SWE-bench-Pro 的 UEnv 调度链路已经可以稳定跑完全量 731 条样本，并返回最终结果。当前最终基线是 `106/731 = 14.50%` resolved。

这轮结果的几个关键信号是：

1. 结果已经不再是早期中间态的 0 resolved，而是形成了可对比的全量基线。
2. 主要失败集中在 `protonmail/webclients` 的 patch 应用失败，以及 `gravitational/teleport` 的超时。
3. Python 样本表现最好，Go 和 TypeScript 仍然是 0 resolved，JavaScript 整体偏弱。
4. 3 条 `ContextWindowExceededError` 说明即使 `max_model_len=131072`，部分长样本仍会触发上下文边界，需要后续继续优化历史裁剪或单次输出预算。
5. 这份结果可以直接作为后续接入优化、prompt 调整、Worker 行为修正和模型升级的基线。
