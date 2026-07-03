# VeRL 异步模式接入 UEnv 字段设计

> 版本：v0.1  
> 日期：2026-07-03  
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

同步模式下，VeRL 发出一个 batch，UEnv 执行完整 rollout 和 reward 后返回结果，VeRL 再计算 advantage、loss 并更新 actor。这个模式对字段要求较低：只要能把 prompt、model endpoint、reward、response、trajectory 对齐即可。

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

## 4. 字段分级

### 4.1 第一版必须新增或稳定透传的字段

这些字段用于保证异步请求和结果可以可靠对齐。Server / Worker 不一定需要理解其算法含义，但必须透传、记录，并在结果中保留。

| 字段 | 类型 | 建议位置 | 说明 |
|---|---|---|---|
| `parallel_mode` | string | `payload.metadata.parallel_mode` | `sync`、`one_step_off_policy`、`fully_async` |
| `request_id` | string | 顶层已有；payload 中也可冗余 | 单个 episode 唯一 ID |
| `batch_id` | string | `payload.metadata.batch_id` | VeRL batch 或 UEnv batch 标识 |
| `sample_index` | int | `payload.metadata.sample_index` | batch 内样本顺序 |
| `global_step` | int/string | `payload.metadata.global_step` | VeRL trainer 当前 step；拿不到时填 `unknown` |
| `rollout_step` | int/string | `payload.metadata.rollout_step` | 发起 rollout 的 step；one-step / fully async 下可能不同于 consume step |
| `consume_step` | int/string | `payload.metadata.consume_step` | 预期被 trainer 消费的 step；拿不到时可为空 |
| `policy_version` | string | `payload.metadata.policy_version` | rollout 使用的 actor 权重版本 |
| `parameter_sync_id` | string | `payload.metadata.parameter_sync_id` | 最近一次 trainer -> rollouter 权重同步 ID |
| `model_endpoint` | object | `payload.model_endpoint` | Worker 调用模型的 OpenAI-compatible endpoint |
| `required_result_fields` | list[string] | `payload.metadata.required_result_fields` | Adapter 希望 Worker 返回的字段 |

### 4.2 One-step off-policy 推荐字段

One-step off-policy 的核心是“最多旧一拍”。UEnv 需要帮助确认样本是否来自允许的旧 policy。

| 字段 | 类型 | 建议位置 | 说明 |
|---|---|---|---|
| `generation_step` | int | `payload.metadata.generation_step` | 样本开始 rollout 时对应的 step |
| `target_train_step` | int | `payload.metadata.target_train_step` | VeRL 计划用该样本训练的 step |
| `max_allowed_staleness` | int | `payload.metadata.max_allowed_staleness` | one-step 通常为 `1` |
| `staleness` | int | 结果 metadata | `target_train_step - generation_step`，可由 Adapter 或 Server 计算 |
| `rollout_policy_version` | string | `payload.metadata.rollout_policy_version` | 实际 rollout 使用的 policy version |
| `trainer_policy_version` | string | 结果 metadata，可选 | Trainer 消费时的 policy version；Server / Worker 通常不知道，可由 Adapter 补 |

One-step 下，Server / Worker 的最小职责不是判断 staleness 是否可训练，而是保留这些字段，使 Adapter / VeRL 能检查：

```text
staleness <= max_allowed_staleness
```

### 4.3 Fully async 推荐字段

Fully async 中，rollouter 持续产生样本，trainer 从队列中消费。结果可能乱序，也可能来自多轮之前的 policy。字段需要更完整。

| 字段 | 类型 | 建议位置 | 说明 |
|---|---|---|---|
| `async_queue_id` | string | `payload.metadata.async_queue_id` | VeRL fully async message queue 或 UEnv result queue 标识 |
| `rollout_worker_id` | string | `payload.metadata.rollout_worker_id` | 产生该请求的 rollouter / AgentLoop worker |
| `rollout_epoch` | int/string | `payload.metadata.rollout_epoch` | rollouter 当前本地 epoch |
| `policy_version` | string | `payload.metadata.policy_version` | 生成该样本时使用的 policy |
| `policy_sync_ts` | float | `payload.metadata.policy_sync_ts` | rollouter 最近一次收到权重同步的时间 |
| `enqueue_ts` | float | `payload.metadata.enqueue_ts` | 请求进入 UEnv / Server 队列时间 |
| `dispatch_ts` | float | Server metadata | Server 派发给 Worker 时间 |
| `worker_start_ts` | float | Worker result metadata | Worker 开始执行时间 |
| `worker_finish_ts` | float | Worker result metadata | Worker 完成执行时间 |
| `result_ready_ts` | float | Server / Worker result metadata | 结果可被 Adapter 消费时间 |
| `staleness_threshold` | float | `payload.metadata.staleness_threshold` | VeRL fully async 配置中的 freshness 上限 |
| `sample_staleness` | float/int | 结果 metadata | 样本相对当前 trainer policy 的陈旧程度 |
| `partial_rollout` | bool | `payload.metadata.partial_rollout` | 是否允许 partial rollout |
| `rollout_logprobs_required` | bool | `payload.metadata.rollout_logprobs_required` | fully async 常需要 rollout 侧 old logprob |

## 5. Payload 建议格式

推荐在 `payload.metadata` 中扩展异步字段，而不是改变现有顶层结构。

```json
{
  "protocol_version": "1.1",
  "framework": "verl",
  "correlation_id": "verl-agent-loop-abc-0",
  "env_config": {
    "task_name": "math",
    "data_source": "gsm8k",
    "raw_prompt": "..."
  },
  "model_endpoint": {
    "endpoint_type": "http",
    "url": "http://10.10.20.142:18088/v1",
    "model_name": "Qwen2.5-0.5B-Instruct",
    "generation_config": {
      "temperature": 1.0,
      "top_p": 1.0,
      "max_new_tokens": 512
    },
    "max_retries": 3
  },
  "episode_config": {
    "max_steps": 10,
    "max_turns": 1,
    "seed": 42,
    "initial_observation": {
      "prompt_text": "...",
      "prompt_ids": [151644, 872, 198],
      "token_source": "verl_agent_loop"
    }
  },
  "reward_config": {
    "reward_type": "rubric",
    "rubric_config": {
      "ground_truth": "72"
    }
  },
  "metadata": {
    "parallel_mode": "one_step_off_policy",
    "batch_id": "verl-agent-loop-001",
    "sample_index": 0,
    "global_step": 12,
    "generation_step": 12,
    "target_train_step": 13,
    "rollout_step": 12,
    "consume_step": 13,
    "policy_version": "actor-step-12",
    "rollout_policy_version": "actor-step-12",
    "parameter_sync_id": "sync-12",
    "max_allowed_staleness": 1,
    "required_result_fields": [
      "response_ids",
      "response_mask",
      "response_text",
      "reward",
      "trajectory",
      "finish_reason",
      "metadata"
    ]
  }
}
```

Fully async 示例：

```json
{
  "protocol_version": "1.1",
  "framework": "verl",
  "model_endpoint": {
    "endpoint_type": "http",
    "url": "http://10.10.20.142:18088/v1",
    "model_name": "Qwen2.5-0.5B-Instruct",
    "generation_config": {
      "max_new_tokens": 512,
      "logprobs": 1
    }
  },
  "episode_config": {
    "max_steps": 10,
    "max_turns": 1,
    "seed": 43,
    "initial_observation": {
      "prompt_text": "...",
      "prompt_ids": [151644, 872, 198]
    }
  },
  "metadata": {
    "parallel_mode": "fully_async",
    "batch_id": "fa-rollout-window-07",
    "sample_index": 3,
    "async_queue_id": "fully-async-main-queue",
    "rollout_worker_id": "rollouter-2",
    "rollout_epoch": 7,
    "global_step": 20,
    "rollout_step": 24,
    "policy_version": "actor-sync-18",
    "parameter_sync_id": "sync-18",
    "policy_sync_ts": 1783000000.12,
    "enqueue_ts": 1783000001.34,
    "staleness_threshold": 0.5,
    "partial_rollout": false,
    "rollout_logprobs_required": true,
    "required_result_fields": [
      "response_ids",
      "response_mask",
      "response_text",
      "response_logprobs",
      "reward",
      "trajectory",
      "finish_reason",
      "metadata"
    ]
  }
}
```

## 6. EpisodeResult 建议扩展

当前 Adapter 从 `EpisodeResult.trajectory.steps[-1].info` 中读取：

| 字段 | 说明 |
|---|---|
| `response_text` | 模型最终回复文本 |
| `response_ids` | 回复 token id 列表，JSON string |
| `response_mask` | 回复 mask，JSON string |

异步模式下建议 Worker 在最后一个 `StepRecord.info` 或 result metadata 中额外返回：

| 字段 | 类型 | 必要性 | 说明 |
|---|---|---|---|
| `response_logprobs` | JSON list[float] | fully async 推荐 | rollout policy 下每个 response token 的 logprob |
| `finish_reason` | string | 推荐 | `stop`、`length`、`tool_done`、`env_done`、`error` |
| `policy_version` | string | 必须透传 | 与请求中的 rollout policy 对齐 |
| `parameter_sync_id` | string | 必须透传 | 与请求中的 sync ID 对齐 |
| `parallel_mode` | string | 必须透传 | 方便 Adapter 校验 |
| `generation_step` | int/string | one-step 必须透传 | 样本生成 step |
| `target_train_step` | int/string | one-step 推荐透传 | 预期训练 step |
| `async_queue_id` | string | fully async 推荐 | 结果所属队列 |
| `worker_id` | string | 推荐 | 实际执行 Worker |
| `dispatch_ts` | float | 推荐 | Server 派发时间 |
| `worker_start_ts` | float | 推荐 | Worker 开始时间 |
| `worker_finish_ts` | float | 推荐 | Worker 完成时间 |
| `result_ready_ts` | float | 推荐 | Server 收到最终结果时间 |
| `server_latency_ms` | int | 推荐 | Server 排队 + 派发耗时 |
| `worker_latency_ms` | int | 推荐 | Worker 执行耗时 |
| `model_latency_ms` | int | 推荐 | 调模型耗时 |

如果无法立即修改 proto，可以先放在 `StepRecord.info` 中：

```json
{
  "response_text": "...",
  "response_ids": "[151645, 198]",
  "response_mask": "[1, 1]",
  "response_logprobs": "[-0.21, -0.33]",
  "finish_reason": "stop",
  "parallel_mode": "fully_async",
  "policy_version": "actor-sync-18",
  "parameter_sync_id": "sync-18",
  "async_queue_id": "fully-async-main-queue",
  "worker_id": "worker-143",
  "worker_latency_ms": "1832",
  "model_latency_ms": "1710"
}
```

## 7. Server 侧处理要求

Server 侧不需要理解 VeRL 的 loss 或 advantage，但需要保证请求调度和结果归属正确。

### 7.1 必须行为

| 行为 | 说明 |
|---|---|
| 透传 payload metadata | 不删除 `parallel_mode`、`policy_version`、`batch_id`、`sample_index` 等字段 |
| 按 `request_id` 做结果映射 | 不依赖返回顺序判断结果归属 |
| 保留 `batch_id` / `sample_index` | 批量请求或异步结果乱序时用于恢复顺序 |
| 记录 dispatch 时间 | 写入 `dispatch_ts`、`result_ready_ts` 或等价日志 |
| 支持乱序返回 | fully async 下不能假设 result 按提交顺序完成 |
| 支持超时和失败回填 | 超时必须返回 failed / timeout result，不能让 Adapter 永久等待 |
| 结果幂等 | 同一个 `request_id` 重复上报时要去重或返回一致结果 |

### 7.2 One-step 下的 Server 要求

One-step 的 Server 可以保持现有同步调度模型，但要额外记录：

```text
request_id -> batch_id -> sample_index -> generation_step -> target_train_step -> policy_version
```

Server 不应该主动丢弃 `generation_step` 旧一拍的请求。是否允许使用该样本训练，由 VeRL / Adapter 根据 staleness 规则判断。

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

Worker 负责真正执行 rollout、环境交互和 reward。异步模式下 Worker 的重点是：使用正确模型 endpoint，返回足够完整的 rollout 结果，并保留请求元数据。

### 8.1 必须行为

| 行为 | 说明 |
|---|---|
| 读取 `payload.model_endpoint` | 使用 Adapter 传入的 OpenAI-compatible URL 调模型 |
| 读取 generation config | 使用 `max_new_tokens`、`temperature`、`top_p`、`logprobs` 等配置 |
| 透传 metadata | 把关键 metadata 写入 result info 或 result metadata |
| 返回 response token | 至少返回 `response_text`，推荐返回 `response_ids` 和 `response_mask` |
| 返回 reward | `summary.total_reward` 必须是 VeRL 可训练使用的 reward |
| 返回 trajectory | 至少包含最后一步 observation/action/reward/info |
| 返回错误结果 | 模型失败、环境失败、reward 失败都要返回结构化失败结果 |

### 8.2 Fully async 对 Worker 的额外要求

VeRL fully async 通常需要 rollout 侧的 old logprob 来保证训练时 importance sampling / PPO ratio 使用的是生成该 token 的 policy 概率。因此如果 `rollout_logprobs_required=true`，Worker 应尽量返回：

| 字段 | 说明 |
|---|---|
| `response_logprobs` | 每个 response token 的 rollout logprob |
| `response_ids` | 与 logprob 一一对应的 token ids |
| `response_mask` | 标记有效 response token |
| `policy_version` | 生成这些 token 的 policy |

如果当前模型 endpoint 不支持 logprobs，Worker 必须显式返回能力缺失，而不是静默返回空值：

```json
{
  "status": "failed",
  "error_code": "ROLLOUT_LOGPROBS_UNSUPPORTED",
  "error_message": "rollout_logprobs_required=true but model endpoint does not return token logprobs"
}
```

Adapter 或 VeRL 也可以选择关闭 `actor_rollout_ref.actor.use_rollout_log_probs` 或使用 trainer 侧重新计算 old logprob，但这属于 VeRL 配置策略，不能由 Worker 自行决定。

## 9. Adapter 侧需要修改的点

虽然本文重点是 Server / Worker 字段处理，但 Adapter 也需要补齐字段来源。

| 修改点 | 说明 |
|---|---|
| 识别 parallel mode | 根据 `UENV_AGENT_LOOP_PARALLEL_MODE` 写入 `parallel_mode`，默认 `sync` |
| 提取 global step | 从 VeRL sample kwargs / extra_info 获取；one-step 下由 batch patch 将 `gen_batch.meta_info.global_steps` 注入 sample extra_info |
| 提取 policy version | 如果 VeRL runtime 暴露 actor version / sync step，应写入 metadata |
| 记录 rollout step / consume step | one-step 下尤其重要 |
| fully async 入口适配 | `fully_async_main` 使用 `FullyAsyncAgentLoopManager`，需要验证 UEnvAgentLoop 输出字段是否满足其 message queue |
| result 校验 | 校验 `request_id`、`batch_id`、`sample_index`、`policy_version` 是否匹配；Adapter 按 `request_id` 恢复 batch result 顺序 |
| logprob 回填 | 如果 result 中有 `response_logprobs`，需要回填到 `AgentLoopOutput.response_logprobs` |

当前 adapter 侧已落地的 one-step 字段能力：

| 能力 | 当前实现 |
|---|---|
| 配置入口 | `configs/uenv-agent-loop.yaml` 读取 `UENV_AGENT_LOOP_PARALLEL_MODE` |
| one-step 默认字段 | `parallel_mode=one_step_off_policy` 时，根据 `global_step/global_steps` 派生 `generation_step`、`target_train_step`、`rollout_step`、`consume_step`、`policy_version`、`rollout_policy_version`、`parameter_sync_id`、`max_allowed_staleness` |
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
| 2 | Adapter 写入 `global_step`、`generation_step`、`target_train_step`、`policy_version`；只有 VeRL batch patch 能稳定把 one-step `global_steps` 带到每个 sample |
| 3 | Server / Worker 原样透传 metadata |
| 4 | Result 返回 `response_ids`、`response_mask`、`reward`、`trajectory`、metadata |
| 5 | 日志验证每个 result 都能映射回 request |
| 6 | 再运行 1-step、2-step、10-step smoke |

### 阶段二：Fully async 最小接入

fully async 接入前，先确认 VeRL fully async 对 AgentLoopOutput 的字段要求，尤其是 `response_logprobs`。

| 步骤 | 内容 |
|---|---|
| 1 | 使用 `verl.experimental.fully_async_policy.fully_async_main` 启动 |
| 2 | 启用 UEnvAgentLoop，确认 fully async rollouter 能调用 UEnv |
| 3 | Adapter 写入 `parallel_mode=fully_async`、`async_queue_id`、`policy_version`、`parameter_sync_id` |
| 4 | Worker 返回 `response_logprobs` 或显式报不支持 |
| 5 | Server 支持 request/result 乱序映射 |
| 6 | 记录 queue、staleness、latency、failure 指标 |

### 阶段三：异步结果池

如果 fully async 长期运行中同步 RPC 等待成为瓶颈，再考虑 Server 增加 result pool：

```text
SubmitEpisodeAsync(request) -> ack
PollEpisodeResult(request_id or batch_id) -> result
```

这不是第一版必须能力，但是真正 fully async 化后会更自然。

## 11. 正确性检查清单

| 检查项 | 通过标准 |
|---|---|
| request/result 对齐 | 每个 result 都能通过 `request_id` 找到原 request |
| batch 顺序恢复 | 即使 result 乱序，也能按 `sample_index` 回填给 VeRL |
| policy version 可追踪 | 日志中能看到每个样本的 rollout policy |
| staleness 可计算 | one-step 至少能计算 `target_train_step - generation_step` |
| reward 正常 | VeRL log 中 reward mean/max/min 与 UEnv result 一致 |
| token 正常 | `response_ids`、`response_mask` 长度一致 |
| fully async logprob | 如果要求 rollout logprob，则每个有效 response token 有对应 logprob |
| 超时可见 | Worker / Server 超时会返回结构化失败，不会卡死 |
| 重复结果可控 | 同一个 `request_id` 重复上报不会重复训练 |

## 12. 结论

接入 VeRL 异步模式时，UEnv 不应该重新实现 VeRL 的异步算法。第一阶段最重要的是把异步训练所需的上下文字段补齐，并保证这些字段从 Adapter 经过 Rust core、Server、Worker 到 Result 全程不丢失。

One-step off-policy 的关键字段是 `generation_step`、`target_train_step`、`policy_version` 和 `max_allowed_staleness`。Fully async 的关键字段是 `async_queue_id`、`policy_version`、`parameter_sync_id`、`staleness_threshold`、时间戳和 `response_logprobs`。

Server 侧重点是请求归属、乱序结果、超时和去重。Worker 侧重点是使用正确模型 endpoint，返回 response token、reward、trajectory，并在 fully async 需要时返回 rollout logprobs。只有这些字段稳定后，才能进一步评估 UEnv 接入 VeRL one-step / fully async 后的训练吞吐和正确性。
