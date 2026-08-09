# UENV Worker 并行度期望参数说明

## 1. 参数目的

训练脚本新增：

```bash
UENV_EXPECTED_WORKER_PARALLELISM
```

该参数用于记录本次训练期望的 UENV Worker 执行并行度，便于后续结合
`agent-loop-requests.jsonl`、`agent-loop-results.jsonl`、Server/Worker 日志和
前端观测数据分析实际并发情况。

该参数是**观测和配置口径字段**，当前不会直接控制 Server/Worker 启动多少个
Worker，也不会修改 Worker 的实际调度容量。

## 2. 默认配置

| 入口 | 默认值 | 说明 |
|---|---:|---|
| `scripts/train/run_verl_uenv_grpo.sh` | 空 | 通用入口不预设 Worker 容量 |
| `scripts/train/presets/swe_smith_grpo_train.sh` | `8` | SWE-smith 当前训练口径 |
| `scripts/train/presets/swe_pro_grpo_sleep_probe.sh` | `8` | SWE-bench-Pro 当前训练口径 |

也可以在启动时覆盖：

```bash
UENV_EXPECTED_WORKER_PARALLELISM=4 \
./scripts/train/presets/swe_smith_grpo_train.sh
```

## 3. 参数传递链路

```text
训练脚本
  -> UENV_EXPECTED_WORKER_PARALLELISM
  -> VeRL 容器环境变量
  -> configs/uenv-agent-loop.yaml
  -> UEnvAgentLoopConfig
  -> EpisodeRequest.metadata.expected_worker_parallelism
  -> request 记录与 Server/Worker 观测链路
```

当前涉及的主要文件：

- `scripts/train/run_verl_uenv_grpo.sh`
- `scripts/train/presets/swe_smith_grpo_train.sh`
- `scripts/train/presets/swe_pro_grpo_sleep_probe.sh`
- `configs/uenv-agent-loop.yaml`
- `src/uenv/bridge/verl_agent_loop.py`

当参数设置为整数时，每条 EpisodeRequest 的 metadata 中会出现：

```json
{
  "expected_worker_parallelism": 8
}
```

## 4. 与其他并行参数的区别

| 参数 | 所属层级 | 作用 |
|---|---|---|
| `TRAIN_BATCH_SIZE` | VeRL | 每个训练 step 使用的 prompt 数量 |
| `ROLLOUT_N` | VeRL | 每个 prompt 采样的 rollout 数量 |
| `TRAIN_BATCH_SIZE * ROLLOUT_N` | Adapter/VeRL | 一次 rollout batch 中的 episode 数量 |
| `AGENT_NUM_WORKERS` | VeRL AgentLoop | VeRL 侧 AgentLoop worker 数 |
| `max_num_seqs` | vLLM | 模型推理侧的并发序列上限 |
| `UENV_EXPECTED_WORKER_PARALLELISM` | 观测口径 | 记录期望的 UENV Worker 并行度 |

例如：

```bash
TRAIN_BATCH_SIZE=2
ROLLOUT_N=4
```

表示一个 VeRL rollout batch 通常包含 8 条 episode，但不代表 UENV Worker
一定会同时执行 8 条 episode。实际并发数仍由 Server/Worker 的实例数量、
Worker slot、环境资源和调度策略决定。

## 5. 观测方式

该参数可以与以下数据结合使用：

- `agent-loop-requests.jsonl`：统计提交的 episode 数和 batch 大小。
- `agent-loop-results.jsonl`：统计完成、失败和单条 episode 耗时。
- `model-gateway.jsonl`：观察模型请求是否形成并发。
- vLLM 日志：观察 `Running`、`Waiting` 和 KV cache 使用情况。
- Server/Worker 日志：确认实际 Worker slot 和环境执行并发。

目前该字段只能表示“期望并行度”，不能替代 Worker 侧的真实指标。后续如需
精确判断并发瓶颈，还需要 Server/Worker 上报 active episode、queued episode、
available slots 和 worker_id 等运行时信息。

## 6. 2026-08-07 补充：SchedulingPolicy 字段

在 Worker 侧 `Docs/worker/260807/Episode并行调度与预热池参数梳理.md` 的基础上，
adapter 已新增 `SchedulingPolicy` 透传字段，用于表达更完整的 run/batch 调度意图。

`UENV_EXPECTED_WORKER_PARALLELISM` 仍保留为观测口径；实际新增的策略字段包括
`UENV_MAX_EPISODE_CONCURRENCY`、`UENV_MAX_IN_FLIGHT_BATCHES`、
`UENV_TARGET_WORKER_SLOTS`、`UENV_POOL_WARMUP_TARGET`、
`UENV_MAX_PARALLEL_PER_WORKER`、`UENV_AGENT_JOB_MAX_CONCURRENCY`、
`UENV_RUNTIME_GATEWAY_SESSION_LIMIT` 和 `UENV_REQUIRE_WARM_SLOT`。

详细字段含义、传递链路和当前 Server/Worker 待消费项见：

`Docs/adapter/20260807-UENV-SchedulingPolicy调度字段接入说明.md`
