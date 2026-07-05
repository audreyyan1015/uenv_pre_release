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

如果接入 VeRL 的异步训练模式，尤其是 `verl.experimental.one_step_off_policy.main_ppo` 和 `verl.experimental.fully_async_policy.fully_async_main`，UEnv 跨边界必须额外保留的是执行模式 `parallel_mode`。训练步、policy 版本、rollout 版本、logprob 和时间戳属于可选增强字段或 VeRL 内部字段，用于后续排查 sample 归属、乱序结果和权重同步问题。

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
| One-step off-policy | `verl.experimental.one_step_off_policy.main_ppo` | rollout 与 update 做一拍流水线；UEnv 当前只需保留 `parallel_mode`，policy 版本等字段作为可选观测 |
| Fully async | `verl.experimental.fully_async_policy.fully_async_main` | rollouter 和 trainer 通过 VeRL 内部队列解耦；UEnv 需要区分 request 发起 step 与实际 rollout 参数版本 |

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

本节只讨论 VeRL 异步模式接入后新增的字段。已有通信协议中的 `request_id`、`batch_id`、`sample_index`、`model_endpoint`、`generation_config`、`response_ids`、`response_mask`、`reward`、`trajectory` 等字段不在本文重复展开；它们仍按现有协议传输和使用。

新增字段分为三类：

| 类别 | 说明 |
|---|---|
| 跨 UEnv 传输字段 | Adapter 放入 `payload.metadata`，Server / Worker 需要保留、记录或透传 |
| VeRL 内部字段 | Adapter 回填给 VeRL 的 `AgentLoopOutput.extra_fields`，不传给 Server / Worker |
| 未来观测字段 | 当前链路不依赖，后续为了排查性能、policy 版本或队列状态时再增加 |

### 4.0 统一命名约定

异步链路里容易混淆的是 policy 版本、step 和 logprob 字段。本文统一使用下面的命名：

| 字段 | 类型 | 使用边界 | 说明 |
|---|---|---|---|
| `parallel_mode` | string | Request metadata | `sync`、`one_step_off_policy`、`fully_async` |
| `rollout_step` | int | Request / Result metadata，可选 | 发起 rollout 的 step；主要用于日志定位和结果归属 |
| `policy_version` | string | Request / Result metadata，可选 | 兼容字段；只有一个版本语义时等于 `rollout_policy_version` |
| `rollout_policy_version` | string | Request / Result metadata，可选 | 实际生成该 response 的 rollout policy 版本；Adapter 能从框架 runtime 获取时再传 |
| `rollout_param_version` | int | Result metadata / `StepRecord.info` | 实际生成 response 的模型参数版本；必须优先来自同一次 `/v1/chat/completions` 响应体或响应 header |
| `parameter_sync_id` | string | Request / Result metadata，可选 | 最近一次 trainer -> rollout 侧权重同步 ID；Adapter 能获取时再传 |
| `rollout_log_probs` | list[float] | Worker Result / StepRecord.info | rollout policy 对每个有效 response token 的 token-level log probability。跨 UEnv 协议统一用这个名字。 |

类型约定：

| 类型类别 | 约定 |
|---|---|
| step / epoch | int；拿不到时省略字段，不建议写字符串 `"unknown"` |
| policy / worker / queue ID | string |
| timestamp | float，Unix epoch seconds，例如 `1783000000.12` |
| latency | int，毫秒 |
| token logprob | list[float]，长度应与 `response_ids` 对齐 |

### 4.1 One-step 当前必须新增字段

One-step off-policy 的训练正确性由 VeRL one-step trainer 保证。UEnv 当前必须新增的跨边界字段只用于表明执行模式。

| 字段 | 类型 | 位置 | Server / Worker 要求 |
|---|---|---|---|
| `parallel_mode` | string | `payload.metadata.parallel_mode` | 保留并透传；值为 `one_step_off_policy` |

### 4.2 One-step 可选或未来增强字段

下面字段不是当前跑通 one-step 的必要条件。Adapter 能从 VeRL runtime 稳定获取时可以写入，Server / Worker 只需要保留和透传，不应据此过滤样本。

| 字段 | 类型 | 位置 | 说明 |
|---|---|---|---|
| `rollout_step` | int | Request / Result metadata | 发起 rollout 的 step；用于日志和结果归属 |
| `policy_version` | string | Request / Result metadata | 兼容字段；只有一个版本语义时等于 `rollout_policy_version` |
| `rollout_policy_version` | string | Request / Result metadata | 生成 response 的 actor 权重版本 |
| `parameter_sync_id` | string | Request / Result metadata | rollout 侧最近一次权重同步 ID |
| `generation_step` | int | Request metadata，兼容字段 | 旧命名；语义等同于 `rollout_step`，不建议 Server / Worker 新增依赖 |
| `target_train_step` | int | Request metadata，兼容字段 | 旧命名；仅用于 Adapter 观测，不建议 Server / Worker 新增依赖 |
| `consume_step` | int | Adapter / VeRL 观测字段 | 预期被 trainer 消费的 step；不要求 Server / Worker 处理 |

### 4.3 Fully async 当前必须新增字段

Fully async 的 queue 和 stale sample 控制由 VeRL 内部实现。UEnv request 侧当前只需要标记执行模式；真实 rollout 版本必须由生成侧在 result 中返回。

| 字段 | 类型 | 位置 | Server / Worker 要求 |
|---|---|---|---|
| `parallel_mode` | string | `payload.metadata.parallel_mode` | 保留并透传；值为 `fully_async` |

同时，VeRL fully async 内部还需要下面三个字段，但它们不是 Server / Worker 的 payload 字段，而是 Adapter 收到 result 后根据真实 `rollout_param_version` 回填给 VeRL 的 `AgentLoopOutput.extra_fields`：

| 字段 | 类型 | 位置 | 说明 |
|---|---|---|---|
| `global_steps` | int | `AgentLoopOutput.extra_fields` | VeRL fully async 组 batch 和统计 stale trajectory 使用 |
| `min_global_steps` | int | `AgentLoopOutput.extra_fields` | VeRL fully async 判断样本 step 范围使用 |
| `max_global_steps` | int | `AgentLoopOutput.extra_fields` | VeRL fully async 判断样本 step 范围使用 |

如果 result 中没有真实 `rollout_param_version`，Adapter 不能把 request 侧的 trainer step 当作严格版本来源。第一版可以把缺版本样本记录为能力缺失或调试 fallback，但不能把它作为 fully async off-policy 正确性的依据。

### 4.4 Fully async 可选或未来增强字段

下面字段用于观测、性能分析或 future bypass mode。当前 Server / Worker 不应强依赖这些字段；如果出现则保留和透传。

| 字段 | 类型 | 位置 | 说明 |
|---|---|---|---|
| `rollout_step` | int | Request / Result metadata | 发起 rollout 的 step；用于日志定位 |
| `policy_version` | string | Request / Result metadata | 兼容字段；只有一个版本语义时等于 `rollout_policy_version` |
| `rollout_policy_version` | string | Request / Result metadata | 生成 response 的 actor 权重版本 |
| `rollout_param_version` | int | Result metadata / `StepRecord.info` | 实际生成 response 的参数版本；Adapter 优先用它回填 `global_steps/min_global_steps/max_global_steps` |
| `min_rollout_param_version` | int | Result metadata / `StepRecord.info` | partial rollout 跨版本时的最小参数版本 |
| `max_rollout_param_version` | int | Result metadata / `StepRecord.info` | partial rollout 跨版本时的最大参数版本 |
| `model_upstream` | string | Result metadata / `StepRecord.info` | 中转站实际转发到的模型 endpoint |
| `parameter_sync_id` | string | Request / Result metadata | 最近一次权重同步 ID |
| `rollout_worker_id` | string | Result metadata | 实际执行 rollout 的 Worker |
| `enqueue_ts` | float | Request metadata，可选 | 请求进入 UEnv / Server 的时间 |
| `dispatch_ts` | float | Server result metadata，可选 | Server 派发给 Worker 的时间 |
| `worker_start_ts` | float | Worker result metadata，可选 | Worker 开始执行时间 |
| `worker_finish_ts` | float | Worker result metadata，可选 | Worker 完成执行时间 |
| `result_ready_ts` | float | Server / Worker result metadata，可选 | 结果可被 Adapter 消费的时间 |
| `server_latency_ms` | int | Result metadata，可选 | Server 排队和派发耗时 |
| `worker_latency_ms` | int | Result metadata，可选 | Worker 执行耗时 |
| `model_latency_ms` | int | Result metadata，可选 | Worker 调模型耗时 |
| `rollout_log_probs` | list[float] | Worker Result / StepRecord.info，可选 | bypass mode 必须；decoupled mode 可省略 |

### 4.5 模型版本来源

不能把 request 发出时的 trainer step 当成实际 rollout 模型版本。原因是 request 发出后，Server/Worker 处理前，VeRL 可能已经完成新一轮参数更新，模型 endpoint 实际使用的权重版本可能已经变化。因此 `global_step` 不再作为 UEnv request 必传字段。

本文采用的严格方案是：模型 endpoint 在执行同一次 OpenAI-compatible 生成请求时，把本次生成实际使用的权重版本绑定到生成响应中。中转站只负责解析、补齐和透传该版本，不把事后查询 `/uenv/model_version` 作为训练正确性的主来源。

模型 endpoint 的 `/v1/chat/completions` 响应体建议包含：

```json
{
  "choices": [
    {
      "message": {
        "content": "..."
      }
    }
  ],
  "uenv_model_version": {
    "model_upstream": "http://127.0.0.1:30001/v1",
    "rollout_param_version": 11,
    "rollout_policy_version": "actor-step-11",
    "parameter_sync_id": "sync-11"
  }
}
```

如果模型 endpoint 更适合通过 HTTP header 返回，也可以在同一次 `/v1/chat/completions` 响应中包含：

```text
X-UEnv-Model-Upstream: http://127.0.0.1:30001/v1
X-UEnv-Rollout-Param-Version: 11
X-UEnv-Rollout-Policy-Version: actor-step-11
X-UEnv-Parameter-Sync-Id: sync-11
```

中转站的职责是：

| 行为 | 说明 |
|---|---|
| 优先解析生成响应体 | 如果响应体有 `uenv_model_version`，使用其中的 `rollout_param_version`、`rollout_policy_version`、`parameter_sync_id` |
| 其次解析生成响应 header | 如果响应体没有版本，则读取 `X-UEnv-Rollout-Param-Version`、`X-UEnv-Rollout-Policy-Version` |
| 补齐透传字段 | 在返回给 Worker 的 JSON 和 header 中补齐 `model_upstream`、版本字段和 source 信息 |
| 不做事后查询 | 如果生成响应没有版本，中转站不再主动查询 upstream；缺版本应在日志中暴露并由 Worker / Server / Adapter 侧处理 |

`GET /uenv/model_version` 可以作为健康检查或人工调试接口保留，建议返回：

```json
{
  "rollout_param_version": 11,
  "rollout_policy_version": "actor-step-11"
}
```

但它不能作为训练版本来源，因为下面这种流程存在竞态：

```text
POST /v1/chat/completions 使用 actor-step-10
生成结束后模型更新到 actor-step-11
GET /uenv/model_version 返回 actor-step-11
```

因此 Worker / Server 应把同一次生成响应中的版本字段写入 `EpisodeResult` 的 metadata 或 `StepRecord.info`。Adapter 收到 result 后优先使用：

```text
rollout_param_version
min_rollout_param_version / max_rollout_param_version
```

回填 VeRL：

```text
global_steps
min_global_steps
max_global_steps
```

如果缺少这些 result 字段，Adapter 应把样本标记为版本信息缺失；是否允许 fallback 只能作为本地调试策略，不应写成 Server / Worker 的协议要求。

### 4.6 rollout_log_probs

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

### 4.7 async_queue / result_pool 的实现选择

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

#### 4.7.1 方案 A：复用框架已有 queue

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
| `parallel_mode` | 区分同步、one-step off-policy、fully async 等执行模式 |
| `rollout_step` / `rollout_policy_version` | 可选观测字段；框架可用来分析 sample 新旧 |
| `rollout_param_version` | result 中的真实 rollout 参数版本；Adapter 用它回填 VeRL fully async 版本字段 |
| `rollout_log_probs` | 可选结果字段；bypass mode 需要，decoupled mode 可省略 |

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

#### 4.7.2 方案 B：UEnv 自建 request/result pool

适用场景：

| 场景 | 说明 |
|---|---|
| UEnv Server / Worker 跨机器部署 | Adapter 发出请求后，Worker 可能异步返回 |
| 需要统一服务多个训练框架 | VeRL、ROLL、NexRL 都通过同一套 UEnv Server 调度 |
| 需要 leader 观测平台 | 统一展示 pending、running、completed、failed、latency 等执行状态 |
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
  "parallel_mode": "fully_async",
  "rollout_step": 12,
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
| 易观测 | Server 能统一记录 queue size、latency、失败率等执行指标 |
| 更适合跨机器 Worker | 能处理乱序、超时、重试、重复结果 |

缺点：

| 缺点 | 说明 |
|---|---|
| 实现复杂度更高 | 需要设计状态机、超时扫描和 ack 语义 |
| 与框架 queue 有重叠 | 需要明确哪个 queue 负责 sample 消费，避免双重缓存 |
| 正确性责任更重 | 必须保证 request/result 不重复消费、不丢失、不错配 |

#### 4.7.3 推荐路线

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
rollout_step
rollout_policy_version
rollout_param_version
rollout_log_probs
```

queue / pool 只负责可靠接收、调度、缓存和返回结果，不应该替代 VeRL / ROLL / NexRL 的训练算法判断。

## 5. Payload 建议格式

推荐只在 `payload.metadata` 中扩展异步字段，不改变现有顶层结构。下面示例只展示新增 metadata 片段，省略已有协议字段。

```json
{
  "metadata": {
    "parallel_mode": "one_step_off_policy",
    "rollout_step": 12,
    "policy_version": "actor-step-12",
    "rollout_policy_version": "actor-step-12",
    "parameter_sync_id": "sync-12"
  }
}
```

Fully async request 当前最小示例：

```json
{
  "metadata": {
    "parallel_mode": "fully_async"
  }
}
```

`global_step` 不再作为 UEnv request 字段下发。实际训练版本以 result 中的 `rollout_param_version` 为准；`staleness` 和 `max_allowed_staleness` 也不下发给 Server / Worker。

## 6. Result 新增字段建议

本节只列异步模式下建议新增到 result metadata 或 `StepRecord.info` 的字段。已有协议中用于训练的 response、reward、trajectory 字段不重复说明。

当前最小要求是：Result 中保留请求侧已有的 `parallel_mode`，并返回实际生成 response 的模型版本。`global_step` 不作为 result 必要字段，也不能作为真实 rollout 版本。

| 字段 | 类型 | 必要性 | 说明 |
|---|---|---|---|
| `parallel_mode` | string | 当前必须 | 方便 Adapter 校验 result 对应的执行模式 |
| `rollout_param_version` | int | fully async 当前必须 | 实际生成 response 的模型参数版本；Adapter 优先用它回填 VeRL `global_steps/min_global_steps/max_global_steps` |
| `rollout_policy_version` | string | fully async 当前必须 | 与 `rollout_param_version` 对应的可读 policy 版本 |
| `model_upstream` | string | 推荐 | 中转站实际转发到的模型 endpoint |
| `min_rollout_param_version` | int | partial rollout 时必须 | 一条 trajectory 跨版本时的最小参数版本 |
| `max_rollout_param_version` | int | partial rollout 时必须 | 一条 trajectory 跨版本时的最大参数版本 |
| `rollout_log_probs` | list[float] | 可选；bypass mode 必须 | rollout policy 下每个 response token 的 logprob；若放在 `StepRecord.info` 中则编码为 JSON 字符串 |
| `policy_version` | string | 可选 | 兼容字段；只有一个版本语义时等于 `rollout_policy_version` |
| `parameter_sync_id` | string | 可选 | 与请求中的 sync ID 对齐 |
| `rollout_step` | int | 可选 | 样本生成 step |
| `rollout_worker_id` | string | 可选 | 实际执行 rollout 的 Worker |
| `dispatch_ts` | float | 可选 | Server 派发时间 |
| `worker_start_ts` | float | 可选 | Worker 开始时间 |
| `worker_finish_ts` | float | 可选 | Worker 完成时间 |
| `result_ready_ts` | float | 可选 | Server 收到最终结果时间 |
| `server_latency_ms` | int | 可选 | Server 排队 + 派发耗时 |
| `worker_latency_ms` | int | 可选 | Worker 执行耗时 |
| `model_latency_ms` | int | 可选 | 调模型耗时 |

如果无法立即修改 proto，可以先放在 `StepRecord.info` 中：

```json
{
  "rollout_log_probs": "[-0.21, -0.33]",
  "parallel_mode": "fully_async",
  "model_upstream": "http://127.0.0.1:30001/v1",
  "rollout_param_version": "19",
  "policy_version": "actor-sync-18",
  "rollout_policy_version": "actor-step-19",
  "parameter_sync_id": "sync-18",
  "rollout_worker_id": "worker-143",
  "worker_latency_ms": "1832",
  "model_latency_ms": "1710"
}
```

## 7. Server 侧处理要求

Server 侧不需要理解 VeRL 的 loss、advantage、one-step 旧一拍语义或 fully async stale sample 策略。它的职责是保留 metadata、正确调度 episode、把 Worker 结果和原请求对齐。

### 7.1 必须行为

| 行为 | 说明 |
|---|---|
| 保留最小异步 metadata | 不删除 `parallel_mode` |
| 透传可选 metadata | 如果请求中有 `rollout_step`、`policy_version`、`parameter_sync_id` 或时间字段，原样带到日志和 result metadata |
| 保留真实 rollout 版本 | 将 Worker 返回的 `rollout_param_version`、`rollout_policy_version`、`model_upstream` 写入 result metadata 或 `StepRecord.info` |
| 记录调度时间 | 尽量写入 `dispatch_ts`、`result_ready_ts` 或等价日志；这是观测字段，不是训练正确性的必要条件 |
| 支持异步乱序语义 | fully async 下不能假设新增 metadata 对应的结果按提交顺序完成 |
| 超时保留上下文 | 超时或失败时也应保留 request metadata，方便 Adapter 判断是哪一个 step 的样本失败 |

### 7.2 One-step 下的 Server 要求

One-step 的 Server 可以保持现有同步调度模型。当前只要求保留：

```text
parallel_mode=one_step_off_policy
```

如果请求里还有 `rollout_step`、`policy_version`、`rollout_policy_version`、`parameter_sync_id`，Server 可以记录并透传，便于排查权重同步和样本归属。Server 不应该主动丢弃旧一拍请求。是否允许使用该样本训练，由 VeRL one-step trainer 决定。

### 7.3 Fully async 下的 Server 要求

Fully async 当前仍可以使用同步 RPC：Adapter 发出请求并等待 Server 返回完整 result。在这种形态下，Server 的要求和 7.1 一致，不需要额外实现 `async_queue_id` 或持久 result pool。

长期形态下，如果 UEnv 要把“提交请求”和“拉取结果”解耦，Server 才需要自建 request/result pool：

| 能力 | 说明 |
|---|---|
| request table | 保存未完成请求的 metadata |
| result table | 保存已完成但尚未被 Adapter 消费的结果 |
| timeout scanner | 定期检查超时请求 |
| duplicate guard | 防止 Worker 重复上报导致重复消费 |
| queue metrics | 记录 pending、running、completed、failed 数量 |

这个 result pool 是未来增强，不是当前 VeRL fully async 接入的前置条件。即使实现 result pool，Server 也只负责缓存、路由、去重和超时，不替代 VeRL 判断 staleness。

## 8. Worker 侧处理要求

Worker 负责真正执行 rollout、环境交互和 reward。异步模式下 Worker 的重点是：保留请求元数据、记录执行耗时，并在结果中透传新增异步字段。

### 8.1 必须行为

| 行为 | 说明 |
|---|---|
| 读取最小异步 metadata | 从 payload 中读取 `parallel_mode`；不认识的 metadata 应忽略但保留 |
| 透传新增 metadata | 把 `parallel_mode` 写入 result info 或 result metadata；其他可选字段存在时也尽量透传 |
| 透传模型版本 | 从同一次模型生成响应的 JSON `uenv_model_version` 或 HTTP header 中读取 `rollout_param_version`、`rollout_policy_version`、`model_upstream`，并写入 result；不要在生成完成后单独查询版本作为严格来源 |
| 记录执行时间 | 尽量返回或记录 `worker_start_ts`、`worker_finish_ts`、`worker_latency_ms`、`model_latency_ms` |
| 错误保留上下文 | 模型失败、环境失败、reward 失败时仍保留 request metadata，便于排查是哪一个 step / request 失败 |

### 8.2 Fully async 对 Worker 的额外要求

VeRL fully async 在 bypass mode 下需要 rollout 侧 token-level logprob 来保证 PPO ratio 使用的是生成该 token 的 policy 概率。因此 Worker 如果具备能力，应返回：

| 字段 | 说明 |
|---|---|
| `rollout_log_probs` | 每个 response token 的 rollout logprob |
| `response_ids` | 与 `rollout_log_probs` 等长的 response token ids |
| `response_mask` | 标识哪些 token 参与训练；tool/env token 通常为 0 |
| `rollout_policy_version` | 生成这些 token 的 policy |

Worker 调用中转站时，优先从响应 JSON 中读取：

```text
uenv_model_version.rollout_param_version
uenv_model_version.rollout_policy_version
uenv_model_version.model_upstream
```

也可以从 HTTP header 中读取：

```text
X-UEnv-Rollout-Param-Version
X-UEnv-Rollout-Policy-Version
X-UEnv-Model-Upstream
```

如果当前模型 endpoint 不支持 logprobs，Worker 可以在 result metadata 或日志里显式记录能力缺失，例如：

```json
{
  "status": "completed",
  "error_code": "ROLLOUT_LOGPROBS_UNSUPPORTED",
  "error_message": "model endpoint does not return token logprobs"
}
```

是否因为缺少 `rollout_log_probs` 而失败，不由 Worker 自行决定：bypass mode 下 Adapter / VeRL 应把它视为错误；decoupled mode 下 Adapter / VeRL 可以选择训练侧重新计算 `old_log_probs`。

## 9. Adapter 侧需要修改的点

虽然本文重点是 Server / Worker 字段处理，但 Adapter 也需要补齐字段来源。

| 修改点 | 说明 |
|---|---|
| 识别 parallel mode | 根据 `UENV_AGENT_LOOP_PARALLEL_MODE` 写入 `parallel_mode`，默认 `sync` |
| 内部读取 VeRL step | 如果 VeRL sample kwargs / extra_info 中有 `global_steps`，Adapter 可以用于本地日志或 VeRL extra_fields 回填，但不作为 UEnv request 必传字段 |
| 提取真实 rollout version | 从 result `StepRecord.info` 中读取 `rollout_param_version`、`rollout_policy_version`；缺失时记录能力缺失，不把 request step 当成严格版本 |
| 记录 rollout step / consume step | one-step 下可作为 Adapter 本地观测字段；Server / Worker 不应依赖 |
| fully async 入口适配 | `fully_async_main` 使用 `FullyAsyncAgentLoopManager`，需要验证 UEnvAgentLoop 输出字段是否满足其 message queue |
| result 校验 | 校验 result 中的新增异步 metadata 是否与 request 一致；已有 request/result 对齐仍使用现有协议键 |
| logprob 回填 | 如果 result 中有 `rollout_log_probs`，需要回填到 `AgentLoopOutput.response_logprobs` |

当前 adapter 侧已落地的 one-step 字段能力：

| 能力 | 当前实现 |
|---|---|
| 配置入口 | `configs/uenv-agent-loop.yaml` 读取 `UENV_AGENT_LOOP_PARALLEL_MODE` |
| one-step 最小字段 | `parallel_mode=one_step_off_policy` |
| one-step 观测字段 | Adapter 可在本地日志中记录 `global_steps`、`rollout_step`、`consume_step`、`policy_version`、`rollout_policy_version`、`parameter_sync_id`；这些不作为 Server / Worker 必须处理的字段 |
| 字段覆盖 | 上述字段可以通过 sample `extra_info` 显式覆盖 |
| Rust core 透传 | Rust adapter core 将 `payload.metadata` 放入 Server / Worker payload 的 `metadata` 字段 |
| 日志验证 | AgentLoop request/result JSONL 中记录 request metadata，便于证明 result 归属 |
| 启动入口 | `scripts/onestep_offpolicy/run_verl_grpo_onestep_offpolicy_uenv.sh` 启用 VeRL one-step trainer + UEnvAgentLoop |

## 10. 正确性检查清单

| 检查项 | 通过标准 |
|---|---|
| 新增 metadata 透传 | Adapter 发出的异步 metadata 能在 Server / Worker result 中看到 |
| rollout 版本可追踪 | result 中能看到每个样本实际使用的 `rollout_param_version` 或 `rollout_policy_version` |
| stale 样本控制 | one-step / fully async 的 stale sample 使用策略由 VeRL 内部控制，Server / Worker 不参与判断 |
| fully async logprob | 如果要求 rollout logprob，则每个有效 response token 有对应 `rollout_log_probs` |
| 延迟可观测 | `enqueue_ts`、`dispatch_ts`、`worker_start_ts`、`worker_finish_ts`、`result_ready_ts` 能还原耗时 |
| 超时可见 | Worker / Server 超时会返回结构化失败，并保留新增 metadata |
| 重复结果可控 | 重复上报不会让同一组异步 metadata 被重复消费 |

## 12. 结论

接入 VeRL 异步模式时，UEnv 不应该重新实现 VeRL 的异步算法。第一阶段最重要的是把异步训练所需的上下文字段补齐，并保证这些字段从 Adapter 经过 Rust core、Server、Worker 到 Result 全程不丢失。

One-step off-policy 跨 UEnv 边界的最小新增字段是 `parallel_mode`；`rollout_step`、`rollout_policy_version`、`parameter_sync_id` 和 `consume_step` 可作为 Adapter 侧观测字段保留，但不是 Server / Worker 执行 episode 的必要字段。Fully async 中，真实版本应由模型 endpoint 在同一次 `/v1/chat/completions` 生成响应中返回，经中转站和 Worker/Server 写入 result 的 `rollout_param_version` / `rollout_policy_version`。Adapter 再用这些 result 字段回填 VeRL 内部需要的 `global_steps/min_global_steps/max_global_steps`。

Server 侧重点是保留新增 metadata、记录调度时间、支持乱序结果和超时上下文。Worker 侧重点是透传新增 metadata、记录执行耗时，并在 fully async 需要时返回 `rollout_log_probs`。已有 response、reward、trajectory 等训练字段继续按现有协议返回；本文不再重复定义。
