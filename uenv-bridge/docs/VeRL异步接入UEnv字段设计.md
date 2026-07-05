# VeRL 异步模式接入 UEnv 字段设计

> 版本：v0.2  
> 日期：2026-07-05  
> 范围：VeRL one-step off-policy 与 fully async 接入 UEnv pre-rollout 链路时，需要新增或透传的字段，以及 Server / Worker 对这些字段的处理方式。

## 1. 背景

当前 UEnv Adapter 的主线是 VeRL pre-rollout 接管：

```text
VeRL AgentLoop
  -> Python Adapter
  -> Rust adapter core
  -> UEnv Server
  -> UEnv Worker
  -> EpisodeResult
  -> Python Adapter
  -> VeRL AgentLoopOutput
  -> VeRL trainer
```

同步模式下，VeRL 发出一个 batch，UEnv 执行完整 rollout 和 reward 后返回结果，VeRL 再计算 advantage、loss 并更新 actor。这个模式主要依赖现有协议字段完成请求和结果对齐。

如果接入 VeRL 的异步训练模式，尤其是 `verl.experimental.one_step_off_policy.main_ppo` 和 `verl.experimental.fully_async_policy.fully_async_main`，UEnv 需要额外保留训练步、policy 版本、rollout 版本、请求归属和结果时效性信息。否则 Server / Worker 即使能返回 reward，也无法证明该结果属于哪个训练状态，排查 stale sample、乱序结果和权重同步问题会非常困难。

本文的设计原则是：

```text
VeRL 仍负责异步训练算法和正确性控制；
UEnv 不重新实现 one-step 或 fully async；
UEnv 只补齐跨 Adapter / Server / Worker 的异步元数据、日志和结果对齐能力。
```

## 2. 三种模式的差异

| 模式 | VeRL 入口 | UEnv 需要关注的问题 |
|---|---|---|
| 同步 GRPO/PPO | `verl.trainer.main_ppo` | 当前 batch 请求和结果一一对应，结果通常在同一个 trainer step 内消费 |
| One-step off-policy | `verl.experimental.one_step_off_policy.main_ppo` | rollout 与 update 做一拍流水线，结果可能由上一版 policy 生成，需要记录 generation step 与 consume step |
| Fully async | `verl.experimental.fully_async_policy.fully_async_main` | rollouter 和 trainer 通过队列解耦，结果可能乱序、延迟、多版本，需要记录 policy version、staleness、queue 与 rollout logprob 信息 |

## 3. 当前数据结构边界

当前链路中主要有三层数据结构：

| 层级 | 数据结构 | 作用 |
|---|---|---|
| Python Adapter 内部 | `EpisodeRequest` / `EpisodeResult` dataclass | Python 侧表达一次 episode 请求和结果 |
| Python -> Rust core gRPC | `SampleEnvelope` / `SampleResult` | Adapter core 的本地 gRPC envelope，用于批量传输、排序和结果映射 |
| Rust core -> Server / Worker | Server 侧 `EpisodeRequest` / `EpisodeResult` | Server / Worker 真正执行 episode 的协议 |

异步字段不应该只放在 Python 内部 dataclass 中，因为 Server / Worker 看不到 Python 对象。推荐做法是：

```text
Python EpisodeRequest.payload JSON
  -> SampleEnvelope.payload_json
  -> Rust adapter core 转成 Server EpisodeRequest.payload，payload 内保留 metadata
  -> Server / Worker 读取或透传
```

也就是说，异步字段应以 JSON metadata 的形式进入 episode payload，并由 Rust adapter core 放进 Server EpisodeRequest.payload.metadata，供 Server / Worker 读取、记录或继续透传。

## 4. 新增字段分级与统一命名

本节只列 VeRL 异步模式接入后新增或需要新增语义的字段。已有通信协议中的 `request_id`、`batch_id`、`sample_index`、`model_endpoint`、`generation_config`、`response_ids`、`response_mask`、`reward`、`trajectory` 等字段不在本文重复展开；它们仍按现有协议传输和使用。

### 4.0 统一命名约定

异步链路里容易混淆的是 policy 版本和 logprob 字段。本文统一使用下面的命名：

| 字段 | 类型 | 使用边界 | 说明 |
|---|---|---|---|
| `rollout_policy_version` | string | Request / Result metadata | 实际生成该 response 的 rollout policy 版本。fully async 下判断样本新旧时优先看它。 |
| `trainer_policy_version` | string | Result metadata，可选 | Adapter / Trainer 消费该结果时的 trainer policy 版本；拿不到时可省略。 |
| `policy_version` | string | Request / Result metadata，兼容字段 | 当前 Adapter 已写出的通用 policy 版本字段；只有一个版本语义时应等于 `rollout_policy_version`。新增实现应优先明确使用 `rollout_policy_version`。 |
| `rollout_log_probs` | list[float] | Worker Result / StepRecord.info | rollout policy 对每个有效 response token 的 token-level log probability。跨 UEnv 协议统一用这个名字。 |
| `response_logprobs` | list[float] | VeRL `AgentLoopOutput` 内部 | Adapter 收到 `rollout_log_probs` 后回填到 VeRL 的字段名，不建议 Server / Worker 使用。 |
| `old_log_probs` | tensor | VeRL trainer 内部 | PPO/GRPO 更新时 ratio 分母的 anchor logprob，不应作为 Server / Worker 协议字段。 |

类型约定：

| 类型类别 | 约定 |
|---|---|
| step / epoch / staleness | int；拿不到时省略字段，不建议写字符串 `"unknown"` |
| policy / worker / queue ID | string |
| timestamp | float，Unix epoch seconds，例如 `1783000000.12` |
| latency | int，毫秒 |
| token logprob | list[float]，长度应与 `response_ids` 对齐 |

### 4.1 通用新增字段

这些字段用于保证异步请求和结果可以可靠对齐。Server / Worker 不一定需要理解其算法含义，但必须透传、记录，并在结果中保留。

| 字段 | 类型 | 建议位置 | 说明 |
|---|---|---|---|
| `parallel_mode` | string | `payload.metadata.parallel_mode` | `sync`、`one_step_off_policy`、`fully_async` |
| `global_step` | int | `payload.metadata.global_step` | VeRL trainer 当前 step；拿不到时省略 |
| `rollout_step` | int | `payload.metadata.rollout_step` | 发起 rollout 的 step；one-step / fully async 下可能不同于 consume step |
| `consume_step` | int | `payload.metadata.consume_step` | 预期被 trainer 消费的 step；拿不到时省略 |
| `policy_version` | string | `payload.metadata.policy_version` | 兼容字段；只有一个 policy 版本语义时等于 `rollout_policy_version` |
| `rollout_policy_version` | string | `payload.metadata.rollout_policy_version` | 生成 response 的 actor 权重版本 |
| `trainer_policy_version` | string | result metadata，可选 | Trainer 消费结果时的 actor 权重版本 |
| `parameter_sync_id` | string | `payload.metadata.parameter_sync_id` | 最近一次 trainer -> rollouter 权重同步 ID |

### 4.2 One-step off-policy 语义映射

One-step off-policy 的核心是“最多旧一拍”。这不需要再新增一组 `generation_step` / `target_train_step` 这样的别名字段，直接使用 4.1 的通用字段即可。

| One-step 语义 | 使用的通用字段 | 说明 |
|---|---|---|
| 样本生成 step | `rollout_step` | 样本开始 rollout 时对应的 step |
| 样本消费 step | `consume_step` | Trainer 计划消费该样本的 step |
| rollout 权重版本 | `rollout_policy_version` | 生成该样本时使用的 actor 权重版本 |
| 权重同步批次 | `parameter_sync_id` | rollout 侧最近一次收到的权重同步 ID |

One-step 可以额外携带 `max_allowed_staleness`，类型为 int，单位是 step。它是 Adapter / Trainer 校验用的阈值，不是要求 Server 做训练策略判断。`staleness` 也不建议作为请求字段传入，而是在结果消费时由 Adapter / Trainer 派生：

```text
staleness = consume_step - rollout_step
is_valid = staleness <= max_allowed_staleness
```

Server / Worker 的最小职责是保留并透传这些 metadata。是否丢弃 stale sample，应该由 VeRL / Adapter / Trainer 根据训练算法决定。

当前 Adapter 代码中为了兼容 VeRL one-step 入口，request metadata 里还可能出现 `generation_step` 和 `target_train_step`。它们分别对应 `rollout_step` 和 `consume_step`，Server / Worker 不应新增依赖，后续统一以 `rollout_step` / `consume_step` 为准。

### 4.3 Fully async 推荐字段

Fully async 中，rollouter 持续产生样本，trainer 从队列中消费。结果可能乱序，也可能来自多轮之前的 policy。字段需要更完整。

| 字段 | 类型 | 建议位置 | 说明 |
|---|---|---|---|
| `async_queue_id` | string | `payload.metadata.async_queue_id` | VeRL fully async message queue 或 UEnv result queue 标识 |
| `rollout_worker_id` | string | `payload.metadata.rollout_worker_id` | 产生该请求的 rollouter / AgentLoop worker |
| `rollout_epoch` | int | `payload.metadata.rollout_epoch` | rollouter 当前本地 epoch |
| `policy_sync_ts` | float | `payload.metadata.policy_sync_ts` | rollouter 最近一次收到权重同步的时间 |
| `enqueue_ts` | float | `payload.metadata.enqueue_ts` | 请求进入 UEnv / Server 队列时间 |
| `dispatch_ts` | float | Server metadata | Server 派发给 Worker 时间 |
| `worker_start_ts` | float | Worker result metadata | Worker 开始执行时间 |
| `worker_finish_ts` | float | Worker result metadata | Worker 完成执行时间 |
| `result_ready_ts` | float | Server / Worker result metadata | 结果可被 Adapter 消费时间 |
| `max_allowed_staleness` | int | `payload.metadata.max_allowed_staleness` | 允许的最大 policy / step 陈旧程度 |
| `staleness` | int | 结果 metadata | 样本相对当前 trainer policy 的陈旧程度 |
| `partial_rollout` | bool | `payload.metadata.partial_rollout` | 是否允许 partial rollout |
| `rollout_log_probs_required` | bool | `payload.metadata.rollout_log_probs_required` | 是否要求 Worker 返回 token-level `rollout_log_probs` |

### 4.4 rollout_log_probs

Fully async 常需要 rollout 侧返回 token-level logprob。这里的协议字段应叫 `rollout_log_probs`，含义是：

```text
rollout_log_probs[i] = rollout_policy_version 对 response_ids[i] 的 log probability
```

来源可以有两种：

| VeRL 使用方式 | 对 UEnv 的要求 | 说明 |
|---|---|---|
| bypass mode | Worker 必须返回 `rollout_log_probs` | VeRL 直接令 `old_log_probs = rollout_log_probs`，省掉训练侧 old logprob forward |
| decoupled mode | Worker 可以不返回 `rollout_log_probs` | VeRL 在训练侧重新计算 `old_log_probs`；速度慢一些，但接口要求低 |

从 VeRL 侧看，这对应两种使用方式：

1. bypass mode

   VeRL 直接使用 rollout 阶段返回的 token logprob：

   ```text
   old_log_probs = rollout_log_probs
   ```

   这种方式的优点是省掉训练侧重新计算 old logprob 的 actor forward，速度更快；缺点是 Worker 必须返回准确的 token-level `rollout_log_probs`，并且它必须和 `response_ids`、`response_mask` 严格对齐。

2. decoupled mode

   VeRL 训练侧自己重新计算 `old_log_probs`。如果 Worker 同时返回了 `rollout_log_probs`，VeRL 仍然可以用它做 off-policy correction、rollout / trainer policy 差异分析或 staleness 观测。

   这种方式的优点是接口更灵活，即使当前 Worker 或模型 endpoint 不能返回 token logprob，也可以先跑通异步链路；缺点是训练侧需要多做一次 actor forward，整体速度会慢一些。

长度要求：

```text
len(rollout_log_probs) == len(response_ids)
```

如果某些 token 是环境/tool token，通常应设置：

```text
response_mask[i] = 0
rollout_log_probs[i] = 0.0
```

训练侧会通过 `response_mask` 排除这些位置。Adapter 收到 `rollout_log_probs` 后，应转换为 VeRL `AgentLoopOutput.response_logprobs`，VeRL `_postprocess()` 再生成 DataProto 中的 `rollout_log_probs`。

### 4.5 async_queue / result_pool 的实现选择

异步训练中需要区分两类队列：

```text
框架内部 queue：
  管理 rollouter 生成的 sample 如何被 trainer 消费。

UEnv 跨边界 result_pool：
  管理已经发给 Server / Worker、但尚未返回或尚未被 Adapter 消费的 episode。
```

如果完全使用 VeRL、ROLL、NexRL 自己的异步执行链路，框架通常已经提供了内部 queue / pool。以 VeRL fully async 为例，本地实现中已有：

```text
Rollouter -> MessageQueue -> Trainer
```

对应代码位置：

```text
verl/experimental/fully_async_policy/message_queue.py
verl/experimental/fully_async_policy/fully_async_rollouter.py
verl/experimental/fully_async_policy/fully_async_trainer.py
```

这个 queue 的职责是让 VeRL rollouter 持续生产 sample，并让 trainer 按 `require_batches * ppo_mini_batch_size` 消费 sample。ROLL / NexRL 的异步 rollout 或服务化训练链路中也有类似能力。因此，如果 UEnv 只是作为这些框架内部 rollout worker 的一部分被调用，优先复用框架已有 queue，不建议在 Adapter 侧重复实现一套训练框架级别的 queue。

但当前 UEnv pre-rollout 链路把 rollout 交给外部 Server / Worker：

```text
VeRL Adapter
  -> Rust adapter core
  -> UEnv Server
  -> UEnv Worker
  -> EpisodeResult
  -> VeRL Adapter
```

框架内部 queue 管不到 Server / Worker 的跨进程、跨机器调度，也无法处理外部 Worker 超时、重复结果、乱序返回、Server 重启或结果暂存。因此，如果要把 UEnv 做成统一外部环境层，Server / adapter core 侧最终仍需要一个轻量的 request/result pool。

#### 4.5.1 方案 A：复用框架已有 queue

适用场景：

| 场景 | 说明 |
|---|---|
| 框架内部 fully async | VeRL / ROLL / NexRL 自己负责 rollouter 和 trainer 解耦 |
| UEnv 调用是同步 episode 函数 | 每个框架 rollouter 调用 UEnv 后直接拿到 result，再把 sample 放入框架 queue |
| 不需要 Server 长时间暂存结果 | 结果由框架内部 queue 管理 |

设计形态：

```text
Framework Rollouter
  -> UEnv Server / Worker 同步执行 episode
  -> Framework sample
  -> Framework async queue
  -> Framework Trainer
```

字段要求：

| 字段 | 作用 |
|---|---|
| `request_id` | UEnv 调用内的结果匹配 |
| `batch_id` / `sample_index` | Adapter 日志与 batch 内排序 |
| `rollout_step` / `consume_step` | 框架判断 sample 属于哪一拍 |
| `rollout_policy_version` | 框架判断 sample 新旧 |
| `rollout_log_probs` | bypass mode 或 fully async correction 需要 |

优点：

| 优点 | 说明 |
|---|---|
| 实现最小 | 不需要在 UEnv 侧重写 queue / scheduler |
| 贴合框架语义 | VeRL / ROLL / NexRL 自己知道如何消费 sample |
| 调试简单 | 先保证 UEnv episode 调用正确即可 |

缺点：

| 缺点 | 说明 |
|---|---|
| 框架耦合更强 | 每个框架的 queue 语义、字段和调度参数不同 |
| UEnv 不掌握全局 pending / running 状态 | Server 侧难以统一观测所有外部 Worker |
| 跨机器容错有限 | Worker 失败或 Server 重启后的结果恢复依赖框架外部逻辑 |

阶段性建议：短期优先采用方案 A，先复用 VeRL fully async / one-step off-policy 已有机制，UEnv 只负责同步 episode 调用、字段透传和结果回填。

#### 4.5.2 方案 B：UEnv 自建 request/result pool

适用场景：

| 场景 | 说明 |
|---|---|
| UEnv Server / Worker 跨机器部署 | Adapter 发出请求后，Worker 可能异步返回 |
| 需要统一服务多个训练框架 | VeRL、ROLL、NexRL 都通过同一套 UEnv Server 调度 |
| 需要 leader 观测平台 | 统一展示 pending、running、completed、failed、latency、staleness |
| 需要更强容错 | Server 重启、Worker 重试、重复上报、超时扫描 |

最小接口：

```text
SubmitEpisodeAsync(requests) -> ack(request_ids)
PollEpisodeResults(async_queue_id, max_results, timeout_ms) -> results
AckEpisodeResults(async_queue_id, result_ids) -> ok
```

最小状态结构：

```text
request_table:
  request_id -> RequestState

pending_queue:
  async_queue_id -> queued request_id list

running_table:
  request_id -> worker_id / dispatch_ts / worker_start_ts

result_pool:
  async_queue_id -> completed EpisodeResult list

consumed_set:
  result_id / request_id 去重
```

`RequestState` 建议字段：

```json
{
  "request_id": "req-001",
  "async_queue_id": "verl-run-001",
  "batch_id": "batch-12",
  "sample_index": 3,
  "rollout_step": 12,
  "consume_step": 13,
  "rollout_policy_version": "actor-step-12",
  "parameter_sync_id": "sync-12",
  "status": "pending",
  "enqueue_ts": 1783000000.1,
  "dispatch_ts": 1783000001.2,
  "worker_start_ts": 1783000001.4,
  "worker_finish_ts": null
}
```

运行流程：

```text
1. Adapter 提交一批 EpisodeRequest。
2. Server 写入 request_table 和 pending_queue，立即返回 ack。
3. Server 后台调度 Worker，把 request 标记为 running。
4. Worker 完成 rollout / reward 后返回 EpisodeResult。
5. Server 写入 result_pool，保留 request metadata。
6. Adapter 或框架按 async_queue_id 拉取完成结果。
7. Trainer 消费后 ack，Server 标记 consumed 或删除结果。
```

存储选择：

| 实现 | 适用阶段 | 说明 |
|---|---|---|
| 内存 HashMap / VecDeque | MVP | 实现简单，但 Server 重启会丢状态 |
| Redis list / stream / hash | 联调与长期运行 | 支持跨进程、可观测、重启恢复和 leader 平台读取 |
| 数据库 + Redis | 稳定服务 | Redis 做热队列，数据库做长期审计 |

优点：

| 优点 | 说明 |
|---|---|
| 框架无关 | VeRL / ROLL / NexRL 可以共享一套外部 episode 调度 |
| 易观测 | Server 能统一记录 queue size、latency、失败率、staleness |
| 更适合跨机器 Worker | 能处理乱序、超时、重试、重复结果 |

缺点：

| 缺点 | 说明 |
|---|---|
| 实现复杂度更高 | 需要设计状态机、超时扫描和 ack 语义 |
| 与框架 queue 有重叠 | 需要明确哪个 queue 负责 sample 消费，避免双重缓存 |
| 正确性责任更重 | 必须保证 request/result 不重复消费、不丢失、不错配 |

#### 4.5.3 推荐路线

短期不要在 Python Adapter 里实现完整 queue。推荐分两阶段：

```text
阶段 1：
  复用 VeRL / ROLL / NexRL 框架内部 queue。
  UEnv Adapter 保持 request/result 对齐、metadata 透传、rollout_log_probs 回填。

阶段 2：
  当 UEnv Server / Worker 需要跨机器长期运行、支持多框架共享或接入观测平台时，
  在 Server / adapter core 侧实现 request/result pool。
```

无论选择哪种方案，训练正确性判断仍应由框架 / Adapter 根据下面字段完成：

```text
request_id
async_queue_id
rollout_step
consume_step
rollout_policy_version
trainer_policy_version
max_allowed_staleness
rollout_log_probs
```

queue / pool 只负责可靠接收、调度、缓存和返回结果，不应该替代 VeRL / ROLL / NexRL 的训练算法判断。

## 5. Payload 建议格式

推荐只在 `payload.metadata` 中扩展异步字段，不改变现有顶层结构。下面示例只展示新增 metadata 片段，省略已有协议字段。

```json
{
  "metadata": {
    "parallel_mode": "one_step_off_policy",
    "global_step": 12,
    "rollout_step": 12,
    "consume_step": 13,
    "policy_version": "actor-step-12",
    "rollout_policy_version": "actor-step-12",
    "parameter_sync_id": "sync-12",
    "max_allowed_staleness": 1
  }
}
```

Fully async 示例：

```json
{
  "metadata": {
    "parallel_mode": "fully_async",
    "async_queue_id": "fully-async-main-queue",
    "rollout_worker_id": "rollouter-2",
    "rollout_epoch": 7,
    "global_step": 20,
    "rollout_step": 24,
    "consume_step": 25,
    "policy_version": "actor-sync-18",
    "rollout_policy_version": "actor-sync-18",
    "parameter_sync_id": "sync-18",
    "policy_sync_ts": 1783000000.12,
    "enqueue_ts": 1783000001.34,
    "max_allowed_staleness": 2,
    "partial_rollout": false,
    "rollout_log_probs_required": true
  }
}
```

## 6. Result 新增字段建议

本节只列异步模式下建议新增到 result metadata 或 `StepRecord.info` 的字段。已有协议中用于训练的 response、reward、trajectory 字段不重复说明。

异步模式下建议 Worker 在最后一个 `StepRecord.info` 或 result metadata 中额外返回：

| 字段 | 类型 | 必要性 | 说明 |
|---|---|---|---|
| `rollout_log_probs` | list[float] | fully async 推荐；bypass mode 必须 | rollout policy 下每个 response token 的 logprob；若放在 `StepRecord.info` 中则编码为 JSON 字符串 |
| `policy_version` | string | 必须透传 | 兼容字段；只有一个版本语义时等于 `rollout_policy_version` |
| `rollout_policy_version` | string | fully async 必须 | 与请求中的 rollout policy 对齐 |
| `trainer_policy_version` | string | 推荐 | Adapter / Trainer 消费结果时的 policy 版本；拿不到可省略 |
| `parameter_sync_id` | string | 必须透传 | 与请求中的 sync ID 对齐 |
| `parallel_mode` | string | 必须透传 | 方便 Adapter 校验 |
| `rollout_step` | int | one-step 必须透传 | 样本生成 step |
| `consume_step` | int | one-step 推荐透传 | 预期训练 step |
| `async_queue_id` | string | fully async 推荐 | 结果所属队列 |
| `rollout_worker_id` | string | 推荐 | 实际执行 rollout 的 Worker |
| `dispatch_ts` | float | 推荐 | Server 派发时间 |
| `worker_start_ts` | float | 推荐 | Worker 开始时间 |
| `worker_finish_ts` | float | 推荐 | Worker 完成时间 |
| `result_ready_ts` | float | 推荐 | Server 收到最终结果时间 |
| `staleness` | int | fully async 推荐 | 消费时的样本陈旧程度 |
| `server_latency_ms` | int | 推荐 | Server 排队 + 派发耗时 |
| `worker_latency_ms` | int | 推荐 | Worker 执行耗时 |
| `model_latency_ms` | int | 推荐 | 调模型耗时 |

如果无法立即修改 proto，可以先放在 `StepRecord.info` 中：

```json
{
  "rollout_log_probs": "[-0.21, -0.33]",
  "parallel_mode": "fully_async",
  "policy_version": "actor-sync-18",
  "rollout_policy_version": "actor-sync-18",
  "parameter_sync_id": "sync-18",
  "async_queue_id": "fully-async-main-queue",
  "rollout_worker_id": "worker-143",
  "staleness": "1",
  "worker_latency_ms": "1832",
  "model_latency_ms": "1710"
}
```

## 7. Server 侧处理要求

Server 侧不需要理解 VeRL 的 loss 或 advantage，但需要保证请求调度和结果归属正确。

### 7.1 必须行为

| 行为 | 说明 |
|---|---|
| 透传新增 metadata | 不删除 `parallel_mode`、`global_step`、`rollout_policy_version`、`parameter_sync_id` 等新增字段 |
| 记录调度时间 | 写入 `dispatch_ts`、`result_ready_ts` 或等价日志 |
| 支持异步乱序语义 | fully async 下不能假设新增 metadata 对应的结果按提交顺序完成 |
| 超时保留上下文 | 超时或失败时也应保留新增 metadata，方便 Adapter 判断是哪一个 policy / step 的样本失败 |

### 7.2 One-step 下的 Server 要求

One-step 的 Server 可以保持现有同步调度模型，但要额外记录：

```text
rollout_step -> consume_step -> rollout_policy_version -> parameter_sync_id
```

Server 不应该主动丢弃 `rollout_step` 旧一拍的请求。是否允许使用该样本训练，由 VeRL / Adapter 根据 staleness 规则判断。

### 7.3 Fully async 下的 Server 要求

Fully async 下 Server 更接近异步结果路由器，需要支持：

| 能力 | 说明 |
|---|---|
| request table | 保存未完成请求的 metadata |
| result table | 保存已完成但尚未被 Adapter 消费的结果 |
| timeout scanner | 定期检查超时请求 |
| duplicate guard | 防止 Worker 重复上报导致重复消费 |
| queue metrics | 记录 pending、running、completed、failed 数量 |

如果 Adapter 仍使用同步 RPC 等待结果，Server 可以先不实现持久 result pool；但 fully async 长期形态下应支持“提交请求”和“拉取结果”解耦。

## 8. Worker 侧处理要求

Worker 负责真正执行 rollout、环境交互和 reward。异步模式下 Worker 的重点是：保留请求元数据、记录执行耗时，并在结果中透传新增异步字段。

### 8.1 必须行为

| 行为 | 说明 |
|---|---|
| 读取新增 metadata | 从 payload 中读取 `parallel_mode`、`rollout_policy_version`、`parameter_sync_id` 等新增字段 |
| 透传新增 metadata | 把关键 metadata 写入 result info 或 result metadata |
| 记录执行时间 | 返回或记录 `worker_start_ts`、`worker_finish_ts`、`worker_latency_ms`、`model_latency_ms` |
| 错误保留上下文 | 模型失败、环境失败、reward 失败时仍保留新增 metadata，便于排查 stale sample 或权重同步问题 |

### 8.2 Fully async 对 Worker 的额外要求

VeRL fully async 常需要 rollout 侧的 token-level logprob 来保证 importance sampling / PPO ratio 使用的是生成该 token 的 policy 概率。因此如果 `rollout_log_probs_required=true`，Worker 应尽量返回：

| 字段 | 说明 |
|---|---|
| `rollout_log_probs` | 每个 response token 的 rollout logprob |
| `response_ids` | 与 `rollout_log_probs` 等长的 response token ids |
| `response_mask` | 标识哪些 token 参与训练；tool/env token 通常为 0 |
| `rollout_policy_version` | 生成这些 token 的 policy |

如果当前模型 endpoint 不支持 logprobs，Worker 必须显式返回能力缺失，而不是静默返回空值：

```json
{
  "status": "failed",
  "error_code": "ROLLOUT_LOGPROBS_UNSUPPORTED",
  "error_message": "rollout_log_probs_required=true but model endpoint does not return token logprobs"
}
```

Adapter 或 VeRL 也可以选择关闭 bypass mode，使用 trainer 侧重新计算 `old_log_probs`。这属于 VeRL 配置策略，不能由 Worker 自行决定。

## 9. Adapter 侧需要修改的点

虽然本文重点是 Server / Worker 字段处理，但 Adapter 也需要补齐字段来源。

| 修改点 | 说明 |
|---|---|
| 识别 parallel mode | 根据 `UENV_AGENT_LOOP_PARALLEL_MODE` 写入 `parallel_mode`，默认 `sync` |
| 提取 global step | 从 VeRL sample kwargs / extra_info 获取；one-step 下由 batch patch 将 `gen_batch.meta_info.global_steps` 注入 sample extra_info |
| 提取 policy version | 如果 VeRL runtime 暴露 actor version / sync step，应写入 `rollout_policy_version`；兼容字段 `policy_version` 可同步写入 |
| 记录 rollout step / consume step | one-step 下尤其重要 |
| fully async 入口适配 | `fully_async_main` 使用 `FullyAsyncAgentLoopManager`，需要验证 UEnvAgentLoop 输出字段是否满足其 message queue |
| result 校验 | 校验 result 中的新增异步 metadata 是否与 request 一致；已有 request/result 对齐仍使用现有协议键 |
| logprob 回填 | 如果 result 中有 `rollout_log_probs`，需要回填到 `AgentLoopOutput.response_logprobs` |

当前 adapter 侧已落地的 one-step 字段能力：

| 能力 | 当前实现 |
|---|---|
| 配置入口 | `configs/uenv-agent-loop.yaml` 读取 `UENV_AGENT_LOOP_PARALLEL_MODE` |
| one-step 默认字段 | `parallel_mode=one_step_off_policy` 时，根据 `global_step/global_steps` 派生 `rollout_step`、`consume_step`、`policy_version`、`rollout_policy_version`、`parameter_sync_id`、`max_allowed_staleness` |
| 字段覆盖 | 上述字段可以通过 sample `extra_info` 显式覆盖 |
| Rust core 透传 | Rust adapter core 将 `payload.metadata` 放入 Server / Worker payload 的 `metadata` 字段 |
| 日志验证 | AgentLoop request/result JSONL 中记录 request metadata，便于证明 result 归属 |
| 启动入口 | `scripts/onestep_offpolicy/run_verl_grpo_onestep_offpolicy_uenv.sh` 启用 VeRL one-step trainer + UEnvAgentLoop |

## 10. 推荐落地顺序

### 阶段一：One-step 字段透传

先接入 one-step off-policy，因为它只允许一拍旧样本，复杂度低于 fully async。

| 步骤 | 内容 |
|---|---|
| 1 | Adapter 通过 `UENV_AGENT_LOOP_PARALLEL_MODE=one_step_off_policy` 写入 `parallel_mode=one_step_off_policy` |
| 2 | Adapter 写入 `global_step`、`rollout_step`、`consume_step`、`rollout_policy_version`；只有 VeRL batch patch 能稳定把 one-step `global_steps` 带到每个 sample |
| 3 | Server / Worker 原样透传 metadata |
| 4 | Result 保留新增 metadata；已有 response / reward / trajectory 按现有协议返回 |
| 5 | 日志验证新增 metadata 从 request 到 result 不丢失 |
| 6 | 再运行 1-step、2-step、10-step smoke |

### 阶段二：Fully async 最小接入

fully async 接入前，先确认 VeRL fully async 对 AgentLoopOutput 的字段要求，尤其是 `AgentLoopOutput.response_logprobs`。跨 UEnv 协议中对应字段统一命名为 `rollout_log_probs`。

| 步骤 | 内容 |
|---|---|
| 1 | 使用 `verl.experimental.fully_async_policy.fully_async_main` 启动 |
| 2 | 启用 UEnvAgentLoop，确认 fully async rollouter 能调用 UEnv |
| 3 | Adapter 写入 `parallel_mode=fully_async`、`async_queue_id`、`rollout_policy_version`、`parameter_sync_id` |
| 4 | Worker 返回 `rollout_log_probs` 或显式报不支持 |
| 5 | Server 支持 request/result 乱序映射 |
| 6 | 记录 queue、staleness、latency、failure 指标 |

### 阶段三：异步结果池

如果 fully async 长期运行中同步 RPC 等待成为瓶颈，再考虑 Server 增加 result pool：

```text
SubmitEpisodeAsync(request) -> ack
PollAsyncEpisodeResult(async_queue_id, cursor) -> result
```

这不是第一版必须能力，但是真正 fully async 化后会更自然。

## 11. 正确性检查清单

| 检查项 | 通过标准 |
|---|---|
| 新增 metadata 透传 | Adapter 发出的异步 metadata 能在 Server / Worker result 中看到 |
| policy version 可追踪 | 日志中能看到每个样本的 rollout policy |
| staleness 可计算 | one-step 至少能计算 `consume_step - rollout_step` |
| stale 样本可判断 | `staleness` 与 `max_allowed_staleness` 可比较 |
| fully async logprob | 如果要求 rollout logprob，则每个有效 response token 有对应 `rollout_log_probs` |
| 延迟可观测 | `enqueue_ts`、`dispatch_ts`、`worker_start_ts`、`worker_finish_ts`、`result_ready_ts` 能还原耗时 |
| 超时可见 | Worker / Server 超时会返回结构化失败，并保留新增 metadata |
| 重复结果可控 | 重复上报不会让同一组异步 metadata 被重复消费 |

## 12. 结论

接入 VeRL 异步模式时，UEnv 不应该重新实现 VeRL 的异步算法。第一阶段最重要的是把异步训练所需的上下文字段补齐，并保证这些字段从 Adapter 经过 Rust core、Server、Worker 到 Result 全程不丢失。

One-step off-policy 主要复用通用字段中的 `rollout_step`、`consume_step`、`rollout_policy_version` 和 `parameter_sync_id`，可选新增 `max_allowed_staleness` 作为消费侧校验阈值。Fully async 的关键新增字段是 `async_queue_id`、`rollout_policy_version`、`trainer_policy_version`、`parameter_sync_id`、`max_allowed_staleness`、时间戳和 `rollout_log_probs`。

Server 侧重点是保留新增 metadata、记录调度时间、支持乱序结果和超时上下文。Worker 侧重点是透传新增 metadata、记录执行耗时，并在 fully async 需要时返回 `rollout_log_probs`。已有 response、reward、trajectory 等训练字段继续按现有协议返回；本文不再重复定义。
