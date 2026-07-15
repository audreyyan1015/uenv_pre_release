# VeRL 异步模式接入 UEnv 字段设计

> 版本：v0.3
> 日期：2026-07-06
> 范围：VeRL one-step off-policy / fully async 接入 UEnv pre-rollout 链路时，Adapter、Server、Worker 之间需要新增或透传的最小字段。

## 1. 背景

当前 UEnv Adapter 从 VeRL pre-rollout AgentLoop 接出：

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

已有协议中的 `request_id`、`batch_id`、`sample_index`、`model_endpoint`、`generation_config`、`response_ids`、`response_mask`、`reward`、`trajectory` 等字段继续按现有方式传输，本文不重复定义。

本文只保留异步训练新增的最小要求：

| 类别 | 结论 |
|---|---|
| Request 必选新增字段 | `parallel_mode` |
| Result 必选新增字段 | `parallel_mode`、`rollout_param_version`、`rollout_policy_version`、`rollout_log_probs` |
| 可选字段 | 仅保留时间戳 / latency 字段，用于观测 |

## 2. 必选字段

### 2.1 Request Metadata

Adapter 在 `payload.metadata` 中写入：

| 字段 | 类型 | 说明 |
|---|---|---|
| `parallel_mode` | string | 执行模式：`sync`、`one_step_off_policy` 或 `fully_async` |

示例：

```json
{
  "metadata": {
    "parallel_mode": "fully_async"
  }
}
```

### 2.2 Result Metadata / StepRecord.info

Worker / Server 返回 `EpisodeResult` 时必须带回：

| 字段 | 类型 | 说明 |
|---|---|---|
| `parallel_mode` | string | 原样透传 request 中的执行模式 |
| `rollout_param_version` | int | 生成该 response 时实际使用的模型参数版本 |
| `rollout_policy_version` | string | 可读 policy 版本，例如 `actor-step-11` |
| `rollout_log_probs` | list[float] | rollout policy 对每个 response token 的 token-level log probability |

`rollout_log_probs` 必须与 `response_ids` 对齐：

```text
len(rollout_log_probs) == len(response_ids)
```

如果某些 token 不参与训练，例如环境 token 或 tool token，应通过 `response_mask[i] = 0` 排除；对应位置的 `rollout_log_probs[i]` 可以填 `0.0`。

示例：

```json
{
  "parallel_mode": "fully_async",
  "rollout_param_version": 11,
  "rollout_policy_version": "actor-step-11",
  "rollout_log_probs": [-0.21, -0.33, -0.18]
}
```

## 3. 可选时间字段

下面字段只用于观测和排查，不参与训练正确性判断：

| 字段 | 类型 | 位置 | 说明 |
|---|---|---|---|
| `enqueue_ts` | float | Request metadata | 请求进入 UEnv / Server 的时间 |
| `dispatch_ts` | float | Result metadata | Server 派发给 Worker 的时间 |
| `worker_start_ts` | float | Result metadata | Worker 开始执行时间 |
| `worker_finish_ts` | float | Result metadata | Worker 完成执行时间 |
| `result_ready_ts` | float | Result metadata | Server 收到最终结果的时间 |
| `server_latency_ms` | int | Result metadata | Server 排队和派发耗时 |
| `worker_latency_ms` | int | Result metadata | Worker 执行耗时 |
| `model_latency_ms` | int | Result metadata | Worker 调模型耗时 |

## 4. 模型版本来源

模型 endpoint 在执行同一次 OpenAI-compatible 生成请求时，把本次生成实际使用的权重版本绑定到生成响应中。中转站只负责解析和透传该版本。

当前实现会同时在响应体 JSON 和 HTTP header 中返回版本信息。Worker 推荐优先读取响应体中的 `uenv_model_version`；如果响应体中没有该字段，再使用 header 作为 fallback。

模型 endpoint 的 `/v1/chat/completions` 响应体包含：

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
    "rollout_param_version": 11,
    "rollout_policy_version": "actor-step-11"
  }
}
```

同一次响应的 header 也会携带相同版本信息，用于兼容或 fallback：

```text
X-UEnv-Rollout-Param-Version: 11
X-UEnv-Rollout-Policy-Version: actor-step-11
```

Worker 必须把同一次生成响应中的版本字段写入 `EpisodeResult`。Adapter 收到 result 后，再用 `rollout_param_version` 回填 VeRL 内部需要的：

```text
global_steps
min_global_steps
max_global_steps
```

这些 `global_steps` 字段是 VeRL 内部 `AgentLoopOutput.extra_fields`，不是 Server / Worker 的通信版本字段。

## 5. rollout_log_probs 要求

当前协议要求 Worker 必须返回 `rollout_log_probs`，不再把缺省 logprob 作为正常路径。

Worker 调用模型 endpoint 时应请求 token-level logprob，并把生成 token 对应的 logprob 按 `response_ids` 顺序写入 result。Adapter 收到后回填：

```text
AgentLoopOutput.response_logprobs = rollout_log_probs
```

VeRL 后续会在 DataProto 中形成 `rollout_log_probs`，用于异步训练中的 old logprob / ratio 计算。如果模型 endpoint 不能返回 token-level logprob，Worker 应返回结构化错误或显式标记能力缺失，不能静默返回空数组。

## 6. Server / Worker / Adapter 职责

### 6.1 Server

| 职责 | 要求 |
|---|---|
| 透传 `parallel_mode` | 不删除 request metadata 中的 `parallel_mode` |
| 对齐请求和结果 | 继续按现有 `request_id` / `batch_id` / `sample_index` 对齐 |
| 透传 Worker 结果 | 不丢弃 `rollout_param_version`、`rollout_policy_version`、`rollout_log_probs` |
| 记录可选时间字段 | 能记录则记录，不能记录不影响训练 |

### 6.2 Worker

| 职责 | 要求 |
|---|---|
| 调模型生成 | 通过 Adapter 提供的模型 endpoint / gateway 发起 `/v1/chat/completions` |
| 读取真实版本 | 从同一次生成响应中的 `uenv_model_version` 或 header 读取版本 |
| 返回 token logprob | 返回与 `response_ids` 对齐的 `rollout_log_probs` |
| 写入 Result | 将 `parallel_mode`、`rollout_param_version`、`rollout_policy_version`、`rollout_log_probs` 写入 `EpisodeResult` |

### 6.3 Adapter

| 职责 | 要求 |
|---|---|
| 写入 request 模式 | 根据 `UENV_AGENT_LOOP_PARALLEL_MODE` 写入 `parallel_mode` |
| 校验 result | 检查 result 是否包含版本字段和 `rollout_log_probs` |
| 回填 VeRL | 把 `rollout_log_probs` 写入 `AgentLoopOutput.response_logprobs`；把 `rollout_param_version` 回填到 VeRL 内部 step 字段 |
| 保持现有对齐 | request / result 对齐继续依赖现有协议键 |

## 7. 检查清单

| 检查项 | 通过标准 |
|---|---|
| 模式透传 | Result 中能看到 request 的 `parallel_mode` |
| 版本准确 | Result 中有 `rollout_param_version` 和 `rollout_policy_version` |
| logprob 完整 | `rollout_log_probs` 非空，且长度与 `response_ids` 一致 |
| 时间可观测 | 可选 ts / latency 字段能辅助定位慢请求 |
| 无旧字段依赖 | Server / Worker 不依赖 `global_step`、`staleness`、`parameter_sync_id` 等旧设计字段 |
