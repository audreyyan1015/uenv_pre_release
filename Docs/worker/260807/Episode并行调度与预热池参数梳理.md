# Episode 并行调度与预热池参数梳理

> 日期：2026-08-07  
> 范围：确认当前 Server / Worker / Adapter 中已有的 episode 并行与预热池参数，并记录需要补齐的显式调度参数。  
> 结论：当前已有全局 admission、Worker 容量和 Worker 预热池目标，但 Adapter 还不能显式传递本次 run/batch 的最大并行 episode 上限和目标预热池规模。

## 1. 当前已有参数

### 1.1 Server admission

文件：`config/server.yaml`

```yaml
episode:
  queue_dynamic: true
```

代码：`uenv-server/src/admission.rs`

当前语义：

- `queue_dynamic=true` 时，Server 的全局 episode admission 容量跟随 Worker 注册、容量变化和下线动态调整。
- `queue_max_in_flight>0` 时，可以使用固定容量 semaphore。
- `queue_max_in_flight==0` 且 `queue_dynamic=false` 时，不限制 Server 侧 in-flight。

当前部署配置没有显式写 `queue_max_in_flight`，实际主要依赖动态 Worker capacity。

### 1.2 Worker capacity

文件：

- `config/uenv-worker.yaml`
- `config/uenv-worker.deploy-7143-swe-pro.yaml`

当前 7143 SWE 配置：

```yaml
worker:
  max_concurrent: 4
```

代码：

- `uenv-worker/src/control_plane/client.rs`
- `uenv-server/src/scheduler/mod.rs`

当前语义：

- Worker 注册和心跳向 Server 上报 `max_concurrent` / capacity。
- Server `RoundRobinScheduler.reserve()` 会按 `env_type`、package、资源、drain/degraded 状态和容量过滤。
- 调度容量使用 `effective_load = max(reserved_load, reported_load)`，防止并发 reserve 超卖。

### 1.3 Worker WarmupPool

文件：`config/uenv-worker.deploy-7143-swe-pro.yaml`

```yaml
pool:
  warmup_size: 4
  prewarm_on_startup: true
  max_idle_time: 300
  cool_timeout: 60
  max_episode_count: 1000
```

代码：`uenv-worker/src/pool/warmup_pool.rs`

当前语义：

- `warmup_size` 是 Worker 本地 warm 实例目标。
- `acquire()` 会优先租用 ready 实例，不足时按需 spawn，并触发 `fill_pool()` 补齐。
- `snapshot()` 上报 `pool_summary` 和 `pool_slots`，前端可看到实例池状态。
- 目前这个参数不是由 Adapter 每次 run 显式传入，而是 Worker 部署配置。

### 1.4 Runtime Gateway capacity

文件：`config/uenv-worker.deploy-7143-swe-pro.yaml`

```yaml
runtime_gateway:
  capacity: 1
```

代码：`uenv-worker/src/runtime.rs`

当前事实：

- 配置里 gateway capacity 仍是 1。
- 代码中 SWE capacity 有 `self.gateway_capacity.max(self.max_concurrent).max(1)` 的抬升逻辑，因此实际可能不会严格受 YAML 中 1 限制。
- 但该行为对运维不直观。并行语义应显式化，避免配置看起来是 1、实际调度又按 4 准备。

### 1.5 Adapter batch size

文件：`uenv-bridge/src/uenv/bridge/verl_agent_loop.py`

当前语义：

- `run_batch()` 按 `UEnvAgentLoopConfig.batch_size` 或 `UENV_AGENT_LOOP_BATCH_SIZE` 切分 VeRL batch。
- 如果 `batch_size <= 0`，一次把整个 VeRL batch 发送给 Adapter Core / Server。
- `submit_episode_batch()` 和 gRPC adapter 实现内部使用 `join_all`，所以同一个 batch 中的 episode 会并发进入 Server admission 和 Worker reserve。

### 1.6 Proto 当前缺口

文件：

- `proto/uenv/v1/adapter_core.proto`
- `proto/uenv/v1/episode.proto`

当前 `ExecuteBatchRequest` 只有：

```proto
message ExecuteBatchRequest {
  string request_id = 1;
  string batch_id = 2;
  repeated SampleEnvelope samples = 3;
}
```

`SampleEnvelope` 和 `EpisodeRequest` 有 `parallel_mode`、timeout、env package、reward config、model endpoint 等字段，但没有：

- `max_episode_concurrency`
- `max_in_flight_batches`
- `target_worker_slots`
- `pool_warmup_target`
- `runtime_gateway_session_limit`
- `agent_job_max_concurrency`

因此 Adapter 目前只能通过 VeRL batch 大小、rollout.n 和 `UENV_AGENT_LOOP_BATCH_SIZE` 间接影响并行，不具备明确的训练 run 调度上限协议。

## 2. 当前实际并行链路

当前链路可以并行，但并行上限分散在多层：

```text
VeRL rollout.n / train_batch_size
  -> UEnvAgentLoop batch_size chunk
  -> ExecuteBatch / ExecuteBatchStream
  -> Server submit_episode_batch join_all
  -> Server admission queue_dynamic / queue_max_in_flight
  -> Scheduler reserve Worker capacity
  -> Worker max_concurrent
  -> Worker WarmupPool acquire / spawn / fill_pool
  -> SWE AgentJob / Runtime Gateway / env instance
```

这意味着“可以并发”不是问题，问题是并发策略没有在 Adapter 到 Server 的协议里被 run 显式声明，导致训练脚本、Server、Worker、Gateway、Agent 之间的上限容易不一致。

## 3. 是否需要新增多个参数

需要。单一 `max_concurrency` 不够表达完整资源模型，至少应拆成以下几类。

| 参数 | 建议位置 | 作用 |
|------|----------|------|
| `max_episode_concurrency` | Adapter `ExecuteBatchRequest` 或 run-level config | 本次 batch/run 允许同时在 Server 中推进的 episode 数 |
| `max_in_flight_batches` | Adapter run-level config | 防止多个 batch 同时堆积导致 Worker 池被旧 batch 占满 |
| `target_worker_slots` / `pool_warmup_target` | RuntimeProfile 或 Episode scheduling policy | 希望 Worker 对该 env_type/package 准备多少 warm slots |
| `max_parallel_per_worker` | Server scheduling policy | 限制单 Worker 上同一 run 或同一 env_type 的并行占用 |
| `agent_job_max_concurrency` | AgentRegistry / RuntimeProfile | 保证 Agent 侧接得住 Worker 侧并行 |
| `runtime_gateway_session_limit` | Worker config / RuntimeProfile | Gateway session 上限与 Worker pool 上限对齐 |

## 4. 推荐协议形状

### 4.1 AdapterCore

```proto
message ExecuteBatchRequest {
  string request_id = 1;
  string batch_id = 2;
  repeated SampleEnvelope samples = 3;
  SchedulingPolicy scheduling_policy = 4;
}

message SchedulingPolicy {
  uint32 max_episode_concurrency = 1;
  uint32 max_in_flight_batches = 2;
  uint32 target_worker_slots = 3;
  uint32 max_parallel_per_worker = 4;
  bool require_warm_slot = 5;
}
```

### 4.2 EpisodeRequest

EpisodeRequest 可只携带最终 Server 计算后的租约和必要调度标签，不建议每个 Episode 重复承载完整 run policy。推荐增加轻量字段：

```proto
message EpisodeRequest {
  string scheduling_group_id = 23;
  uint32 scheduling_priority = 24;
}
```

完整 policy 保留在 Server admission / run state 中。

### 4.3 Worker / RuntimeProfile

RuntimeProfile 中声明默认池策略：

```json
{
  "pool": {
    "warmup_target": 4,
    "max_parallel_episodes": 4,
    "max_parallel_per_worker": 4,
    "require_warm_slot": true
  },
  "agent": {
    "max_concurrent_jobs": 4
  },
  "runtime_gateway": {
    "session_limit": 4
  }
}
```

Adapter 传入的 run-level policy 可以覆盖 RuntimeProfile 默认值，但不能超过 Worker 和 Server 的硬上限。

## 5. Server 应用方式

Server 应维护 run/batch 级调度状态：

1. Adapter 提交 batch 时注册 `scheduling_group_id`。
2. Server 为该 group 建立 semaphore，容量为 `max_episode_concurrency`。
3. 每个 episode 先拿 group permit，再进入全局 admission。
4. Scheduler reserve 时同时检查 Worker capacity、env pool、tool capacity、agent capacity。
5. 完成、失败、取消、超时都释放 group permit 和 Worker lease。

这样可以显式控制“一个训练 run 当前最多并行多少 episode”，同时仍保留 Worker 全局资源保护。

## 6. Worker 应用方式

Worker 侧应区分两个上限：

- `pool_warmup_target`：准备多少 ready 实例，影响等待时间。
- `max_parallel_episodes`：允许多少 busy 实例，影响并发吞吐和资源占用。

二者不一定相等。例如：

```text
warmup_target = 2
max_parallel_episodes = 4
```

表示常驻 2 个 ready 实例，但高峰时可以按需拉到 4 个 busy 实例。

当前 `warmup_size` 近似承担 warmup target 的角色，`worker.max_concurrent` 近似承担 max parallel 的角色。后续应把二者在 RuntimeProfile / Worker config 中显式命名，并在前端分别显示。

## 7. 结论

当前框架已经能并发调度 episode，并且 Server 有动态 admission、Worker 有容量和预热池。但它缺少 Adapter 显式传入的 run/batch 并行策略。

为提高训练效率且避免资源误配，需要新增多参数调度协议：run 级 `max_episode_concurrency`、Worker pool 级 `warmup_target`、Worker busy 级 `max_parallel_episodes`、Agent/Gateway 级上限。Server 应按这些参数显式并行调度，而不是只依赖 batch 大小和 Worker 默认配置。

## 8. Adapter 侧落实状态（2026-08-07）

Adapter 已按本文推荐补齐第一阶段字段：

- `ExecuteBatchRequest.scheduling_policy`：承载 `max_episode_concurrency`、`max_in_flight_batches`、`target_worker_slots`、`pool_warmup_target`、`max_parallel_per_worker`、`agent_job_max_concurrency`、`runtime_gateway_session_limit` 和 `require_warm_slot`。
- `EpisodeRequest.scheduling_group_id` / `scheduling_priority`：作为轻量调度标签下发给 Server/Worker。
- SWE 训练脚本已暴露对应 `UENV_*` 环境变量，并通过 `configs/uenv-agent-loop.yaml` 传入 `UEnvAgentLoop`。
- adapter-core 当前已用 `max_episode_concurrency` 对单个 batch 做保守分块，并把 `scheduling_group_id` 写为 batch id。

仍需 Server/Worker 后续消费的部分：run 级 in-flight batch admission、Worker slot 对齐、WarmupPool 目标、单 Worker 并行上限、AgentJob 并发和 Runtime Gateway session 上限。详细 adapter 侧说明见 `Docs/adapter/20260807-UENV-SchedulingPolicy调度字段接入说明.md`。
