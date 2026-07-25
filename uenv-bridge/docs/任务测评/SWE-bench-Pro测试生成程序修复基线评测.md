# SWE-bench-Pro 测试生成/程序修复 UEnv 基线评测

> 日期：2026-07-20
> 阶段：Eval-first，未进行后训练
> 任务书条目：4. 测试生成/程序修复
> Benchmark：SWE-bench-Pro public test split
> 目标模型：`Qwen/Qwen3.6-35B-A3B`
> 正式口径：接入 UEnv，Worker SWE 环境 + OpenHands Agent 执行，`MAX_TOKENS=8192`，`THINKING_TOKEN_BUDGET=4096`，`workspace_dir=/app`

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

Adapter 侧 request 中主要传入 `instance_id`、`repo`、`base_commit`、`dockerhub_tag`、`env_package_id`、`workspace_dir`、`driver_entrypoint`、`llm_config_path` 等字段。`model_endpoint.url` 设置为 Adapter Model Gateway 的 worker-visible URL；OpenHands Agent 侧根据 `LLM_CONFIG_PATH` 访问模型，因此该配置文件里的 `base_url` 也需要指向同一个 gateway。

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
| vLLM `max_model_len` | 65536 |
| Adapter Model Gateway | `http://10.10.20.142:18097/v1` |
| Gateway upstream | `http://127.0.0.1:18081/v1` |
| UEnv batch size | 1 |
| `MAX_TOKENS` | 8192 |
| `THINKING_TOKEN_BUDGET` | 4096 |
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
| Max iterations | 50 |
| 输出目录 | `temp/benchmarks/swebenchpro/qwen3_6_35b_a3b_uenv_full_thinking8192_budget4096_20260719_205350/` |

OpenHands LLM 配置文件位于 208.77 Agent 机器上。本轮 208.77 通过本地 SSH 隧道 `127.0.0.1:18194` 访问 7142 上的 Adapter Model Gateway，因此该配置文件中的 `base_url` 使用 Agent 侧本地地址：

```json
{
  "model": "openai/Qwen/Qwen3.6-35B-A3B",
  "base_url": "http://127.0.0.1:18194/v1",
  "api_key": "EMPTY",
  "temperature": 0.0,
  "max_output_tokens": 8192,
  "timeout": 7200,
  "request_timeout": 7200,
  "num_retries": 2
}
```

SWE-bench-Pro 和其他四类 benchmark 的启动流程保持一致：先启动 vLLM，再启动 Adapter Model Gateway，最后运行 UEnv 评测脚本。区别只在于 SWE 的模型调用由 OpenHands Agent 根据 `LLM_CONFIG_PATH` 发起，因此这个 config 文件必须指向 Agent 侧可访问的同一个 gateway；当前 208.77 使用 `127.0.0.1:18194 -> 10.10.20.142:18097` 的隧道。

## 5. 运行命令

从零开始运行时，先启动 8GPU vLLM，监听本机 `18081`：

```bash
cd /data/ronghao/uenv/uenv-bridge

BASE=/data/ronghao/uenv/uenv-bridge/temp/benchmarks/swebenchpro/qwen3_6_35b_a3b_uenv_full_thinking8192_budget4096_$(date +%Y%m%d_%H%M%S)
mkdir -p "$BASE"

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

在独立终端启动 Worker/OpenHands 可访问的 adapter model gateway，转发到本机 vLLM：

```bash
cd /data/ronghao/uenv/uenv-bridge

BASE=/data/ronghao/uenv/uenv-bridge/temp/benchmarks/swebenchpro/qwen3_6_35b_a3b_uenv_full_thinking8192_budget4096_$(date +%Y%m%d_%H%M%S)
mkdir -p "$BASE"

PYTHONPATH=src python3 scripts/benchmark/run_model_gateway.py \
  --upstream http://127.0.0.1:18081/v1 \
  --bind-host 0.0.0.0 \
  --port 18097 \
  --public-url http://10.10.20.142:18097/v1 \
  --request-timeout-seconds 7200 \
  --enable-thinking \
  --thinking-token-budget 4096 \
  --strip-reasoning \
  --log-path "$BASE/model-gateway-swe-thinking-budget4096.jsonl"
```

可用下面命令确认 gateway 已就绪：

```bash
curl --noproxy '*' http://127.0.0.1:18097/v1/models
```

通过 UEnv 跑 SWE-bench-Pro 全量任务：

```bash
cd /data/ronghao/uenv/uenv-bridge

OUT=/data/ronghao/uenv/uenv-bridge/temp/benchmarks/swebenchpro/qwen3_6_35b_a3b_uenv_full_thinking8192_budget4096_20260719_205350
mkdir -p "$OUT"

nohup env \
REPO_DIR=/data/ronghao/uenv/uenv-bridge \
DATA_PATH=/data/ronghao/uenv/uenv-bridge/data/benchmarks/swebenchpro/test.jsonl \
OUTPUT_DIR="$OUT" \
UENV_ADAPTER_CORE_ENDPOINT=8.130.75.157:8088 \
UENV_ROLLOUT_MODEL_ENDPOINT=http://10.10.20.142:18097/v1 \
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
MAX_ITERATIONS=50 \
RESUME=0 \
./scripts/benchmark/run_swebenchpro_uenv_baseline.sh \
> "$OUT/run.log" 2>&1 &

echo $! > "$OUT/run.pid"
```

查看运行进度：

```bash
tail -f /data/ronghao/uenv/uenv-bridge/temp/benchmarks/swebenchpro/qwen3_6_35b_a3b_uenv_full_thinking8192_budget4096_20260719_205350/run.log
```

汇总当前结果：

```bash
python3 - <<'PY'
from pathlib import Path
import collections
import json

out = Path("/data/ronghao/uenv/uenv-bridge/temp/benchmarks/swebenchpro/qwen3_6_35b_a3b_uenv_full_thinking8192_budget4096_20260719_205350")
rows = [json.loads(line) for line in (out / "uenv_results.jsonl").open(encoding="utf-8") if line.strip()]

print("results", len(rows))
print("status", dict(collections.Counter(row.get("uenv_status") for row in rows)))
print("resolved", dict(collections.Counter(str(row.get("resolved")) for row in rows)))
PY
```

## 6. 当前结果

截至当前结果文件：

```text
temp/benchmarks/swebenchpro/qwen3_6_35b_a3b_uenv_full_thinking8192_budget4096_20260719_205350/uenv_results.jsonl
```

统计如下：

| 指标 | 值 |
|---|---:|
| 数据集样本数 | 731 |
| 已返回 result | 184 |
| `uenv_status=completed` | 146 |
| `uenv_status=failed` | 38 |
| `resolved=true` | 0 |
| `resolved=false` | 184 |
| 当前 resolved rate | 0.0000 |

38 条 failed 的错误分布：

| 错误类型 | 数量 | 说明 |
|---|---:|---|
| `ContextWindowExceededError` | 29 | OpenHands 多轮工具调用后累计 prompt 接近或超过 vLLM `65536` 上下文限制 |
| timeout | 2 | HTTP / socket / episode 等待超时 |
| other | 7 | 需要结合 Worker / OpenHands trajectory 继续排查 |

当前最重要的现象不是少量上下文超长或 timeout，而是已经 completed 的 146 条样本也全部 `resolved=false`。这需要 Worker/Agent 侧结合 OpenHands trajectory、最终 `git diff`、modified files、测试执行目录和测试日志继续定位。

## 7. 当前结论

SWE-bench-Pro 的 UEnv 调度链路已经可以返回 result，但当前 184 条已返回样本中没有任何 resolved 样本。后续排查重点应放在 Worker/OpenHands 层面：

1. OpenHands 实际工作目录是否就是目标 repo 根目录 `/app`。
2. 最终 patch 是否真实修改了目标 repo 下的源码文件。
3. Worker 是否正确收集 OpenHands 最终 `git diff`。
4. 官方测试是否在正确的 repo、base commit 和容器环境中执行。
5. 对 `ContextWindowExceededError` 样本，是否需要 Worker/Agent 侧启用更强的 history truncation 或降低 OpenHands 单次输出预算。

历史上曾做过“直接 vLLM 生成 patch + 官方 Docker evaluator”的早期实验，使用 `MAX_TOKENS=4096`、`TEMPERATURE=0.2`、thinking 关闭。该实验只作为历史参考，不是当前 UEnv 正式运行命令。
