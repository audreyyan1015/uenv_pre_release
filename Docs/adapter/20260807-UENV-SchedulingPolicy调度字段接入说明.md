# UENV SchedulingPolicy 调度字段接入说明

## 1. 背景

Worker 侧文档 `Docs/worker/260807/Episode并行调度与预热池参数梳理.md`
指出，当前训练并行上限分散在 VeRL batch、Adapter batch、Server admission、
Worker capacity、WarmupPool、AgentJob 和 Runtime Gateway 多层，Adapter 缺少
显式传递本次 run/batch 调度意图的协议字段。

本次 adapter 侧先补齐字段和透传链路，使训练脚本可以声明调度意图。真正的
Server/Worker 硬上限、Worker 预热和资源租约仍由 Server/Worker 侧负责解释与执行。

## 2. 新增协议字段

### 2.1 AdapterCore ExecuteBatchRequest

文件：`proto/uenv/v1/adapter_core.proto`

新增 batch/run 级字段：

```proto
message ExecuteBatchRequest {
  string request_id = 1;
  string batch_id = 2;
  repeated SampleEnvelope samples = 3;
  SchedulingPolicy scheduling_policy = 4;
}
```

`SchedulingPolicy` 当前包含：

| 字段 | 含义 |
|---|---|
| `max_episode_concurrency` | 本次 batch/run 同时推进的 episode 上限 |
| `max_in_flight_batches` | 同一 run 允许同时积压的 batch 上限 |
| `target_worker_slots` | 希望 Server 为该 run/env_type 对齐的 Worker slot 数 |
| `max_parallel_per_worker` | 单个 Worker 上同一 run/env_type 的并行占用上限 |
| `require_warm_slot` | 是否要求只使用预热好的 slot |
| `pool_warmup_target` | Worker WarmupPool ready 实例目标 |
| `agent_job_max_concurrency` | Agent job 接收侧并发上限 |
| `runtime_gateway_session_limit` | Runtime Gateway session 上限 |

### 2.2 EpisodeRequest

文件：`proto/uenv/v1/episode.proto`

新增轻量调度标签：

```proto
string scheduling_group_id = 23;
uint32 scheduling_priority = 24;
```

当前 adapter-core 会把 `scheduling_group_id` 设置为 batch id，便于 Server 后续按
run/batch 建立 admission 状态。完整 policy 不写入 metadata。

## 3. Adapter 已落实的链路

### 3.1 训练脚本环境变量

通用入口 `uenv-bridge/scripts/train/run_verl_uenv_grpo.sh` 已支持并传入容器：

| 环境变量 | 对应字段 |
|---|---|
| `UENV_MAX_EPISODE_CONCURRENCY` | `max_episode_concurrency` |
| `UENV_MAX_IN_FLIGHT_BATCHES` | `max_in_flight_batches` |
| `UENV_TARGET_WORKER_SLOTS` | `target_worker_slots` |
| `UENV_POOL_WARMUP_TARGET` | `pool_warmup_target` |
| `UENV_MAX_PARALLEL_PER_WORKER` | `max_parallel_per_worker` |
| `UENV_AGENT_JOB_MAX_CONCURRENCY` | `agent_job_max_concurrency` |
| `UENV_RUNTIME_GATEWAY_SESSION_LIMIT` | `runtime_gateway_session_limit` |
| `UENV_REQUIRE_WARM_SLOT` | `require_warm_slot` |

SWE-smith 和 SWE-Pro preset 默认值：

```bash
UENV_MAX_EPISODE_CONCURRENCY=${UENV_EXPECTED_WORKER_PARALLELISM}
UENV_MAX_IN_FLIGHT_BATCHES=1
UENV_TARGET_WORKER_SLOTS=${UENV_EXPECTED_WORKER_PARALLELISM}
UENV_POOL_WARMUP_TARGET=${UENV_EXPECTED_WORKER_PARALLELISM}
UENV_MAX_PARALLEL_PER_WORKER=4
UENV_AGENT_JOB_MAX_CONCURRENCY=4
UENV_RUNTIME_GATEWAY_SESSION_LIMIT=4
UENV_REQUIRE_WARM_SLOT=false
```

### 3.2 AgentLoop 到 Python Client

文件：

- `uenv-bridge/configs/uenv-agent-loop.yaml`
- `uenv-bridge/src/uenv/bridge/verl_agent_loop.py`

`UEnvAgentLoop` 从环境变量读取调度字段，并写入每条请求 payload 的
`metadata.scheduling_policy`。该字段只作为 Python adapter client 构造
`ExecuteBatchRequest.scheduling_policy` 的中间表示，不作为业务 metadata 传给 Worker。

### 3.3 Python Client 到 Rust AdapterCore

文件：`uenv-bridge/src/uenv/bridge/clients.py`

`RustCoreEpisodeClient` 会从 batch 中第一条 request 的
`metadata.scheduling_policy` 提取策略，并在 unary `ExecuteBatch` 请求中设置
`scheduling_policy`。同时会从 `sample_context_json` 中过滤所有调度字段。

当前 streaming `ExecuteBatchStream` 仍只传 `SampleEnvelope`，不承载 batch policy。
SWE 训练默认走 unary batch 路径。

### 3.4 Rust AdapterCore

文件：`uenv-bridge/core/src/protocol.rs`、`uenv-bridge/core/src/core.rs`

Rust adapter-core 已能接收 `SchedulingPolicy`。目前已执行两件事：

- 将每条 `EpisodeRequest.scheduling_group_id` 设置为样本 batch id。
- 当 `max_episode_concurrency > 0` 且本次 batch 大于该值时，adapter-core 会按该大小
  分块调用 `EpisodeService.submit_episode_batch`，避免一个 adapter batch 一次性把全部
  episode 推给 Server。

## 4. 仍需 Server/Worker 消费的部分

adapter 侧已经把字段传到 adapter-core，并提供了 batch 内的保守分块。以下语义仍需
Server/Worker 明确实现：

| 字段 | 需要消费的位置 |
|---|---|
| `max_in_flight_batches` | Server run/batch admission |
| `target_worker_slots` | Server scheduler / Worker capacity 视图 |
| `pool_warmup_target` | Worker WarmupPool / RuntimeProfile |
| `max_parallel_per_worker` | Server scheduler reserve 与 Worker 本地执行上限 |
| `agent_job_max_concurrency` | AgentRegistry / AgentJob dispatch |
| `runtime_gateway_session_limit` | Worker Runtime Gateway session 管理 |
| `require_warm_slot` | Server scheduler 与 Worker pool acquire 策略 |

因此这些字段当前应理解为“adapter 已声明并透传的调度意图”。如果 Server/Worker
尚未消费，前端和日志中看到的真实并发仍以 Server/Worker 的实际 capacity、reserve、
pool 和 agent job 日志为准。

## 5. 验证方式

建议验证三层：

1. Adapter request 记录：`agent-loop-requests.jsonl` 中应能看到
   `request_metadata.scheduling_policy`。
2. AdapterCore gRPC：Python client 构造的 `ExecuteBatchRequest` 应包含
   `scheduling_policy`。
3. Server/Worker 侧：确认 `scheduling_group_id` 是否进入 EpisodeRequest，并观察
   Server admission、Worker reserve、WarmupPool、AgentJob 和 Runtime Gateway 的实际并发。

如果前端仍显示 DISPATCH/EXECUTE 关联实体为 0，优先检查 Server/Worker 是否把 episode
状态事件投递到了同一个 Obs run。
