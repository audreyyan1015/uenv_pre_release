# VeRL 异步接入 UEnv：Worker / Agent 两条路径补充实现报告

> 文件路径：`/home/VeRL 异步接入 UEnv：Worker 侧补充实现报告.md`
>
> 目的：说明 Server 已支持 VeRL async 字段和统一完成管线后，执行侧还必须补哪些逻辑，才能真正支持 `one_step_off_policy` 和 `fully_async`。
>
> 本文同时覆盖两条路径：
> - Native Worker 路径：`DispatchEpisode` -> Worker 执行 -> `ReportResultRequest.result: EpisodeResult`
> - Agent/SWE 路径：`AgentJob` -> Agent 执行 -> `AgentJobCompleteRequest`

---

## 一句话结论

Server 侧已经能处理 VeRL async 字段，并且 Native Worker `ReportResult` 与 Agent/SWE `complete_agent_job` 都会执行同一套 async 结果校验。

因此，执行侧不能只返回 reward、trajectory 或 error message。只要请求的 `parallel_mode` 是：

```text
one_step_off_policy
fully_async
```

并且执行侧返回 `status = completed`，就必须返回以下字段：

- `parallel_mode`
- `rollout_param_version`
- `rollout_policy_version`
- `rollout_log_probs`

缺少任意必需字段时，Server 会把 completed 结果转成 failed，例如：

```text
error_code = 1004 / ERR_ASYNC_PROTOCOL_MISSING_FIELD
error_message = missing rollout_log_probs
```

执行侧真正要补的是：

```text
从同一次模型生成响应中读取真实版本和 token logprob，校验 token 对齐，然后写入当前路径对应的结果载体。
```

---

## 两条路径分别要写什么

| 路径 | Server 下发 | 执行侧返回 | 必须写入的位置 |
|---|---|---|---|
| Native Worker | `EpisodeRequest` / `DispatchEpisodeRequest` | `ReportResultRequest` | `ReportResultRequest.result` 里的 `EpisodeResult` 顶层字段 |
| Agent/SWE | `AgentJob` | `AgentJobCompleteRequest` | `AgentJobCompleteRequest` 顶层字段 |

两条路径的字段语义一致，但接口不同：

- Native Worker 路径最终直接上报 `EpisodeResult`。
- Agent/SWE 路径先上报 `AgentJobCompleteRequest`，Server 再把它转换成最终 `EpisodeResult`。

---

## 必须返回的 async 字段

### Native Worker 路径：写入 `EpisodeResult`

这些字段必须写在 `EpisodeResult` 顶层 proto 字段里，不能只写进 `StepRecord.info` 或 metadata。

| 字段 | 是否必需 | 来源 | 说明 |
|---|---|---|---|
| `parallel_mode` | async completed 必需 | request | 原样回填 `sync`、`one_step_off_policy` 或 `fully_async`。 |
| `rollout_param_version` | async completed 必需 | 模型响应 | 本次生成实际使用的参数版本。 |
| `rollout_policy_version` | async completed 必需 | 模型响应 | 可读 policy 版本，例如 `actor-step-11`。 |
| `rollout_log_probs` | async completed 必需 | 模型响应 | response token 级别的 logprob。 |
| `worker_start_ts` | 建议 | Worker | Worker 开始执行 episode 的 Unix 秒时间戳。 |
| `worker_finish_ts` | 建议 | Worker | Worker 完成 episode 的 Unix 秒时间戳。 |
| `worker_latency_ms` | 建议 | Worker | Worker 执行总耗时。 |
| `model_latency_ms` | 建议 | Worker | 模型调用耗时。 |

### Agent/SWE 路径：写入 `AgentJobCompleteRequest`

Agent/SWE 路径不调用 `ReportResult`，所以字段必须写到 `AgentJobCompleteRequest` 顶层字段。

| 字段 | 是否必需 | 来源 | 说明 |
|---|---|---|---|
| `parallel_mode` | async completed 必需 | `AgentJob.parallel_mode` | 原样回填 Server 下发的模式。 |
| `rollout_param_version` | async completed 必需 | 模型响应 | 本次生成实际使用的参数版本。 |
| `rollout_policy_version` | async completed 必需 | 模型响应 | 可读 policy 版本。 |
| `rollout_log_probs` | async completed 必需 | 模型响应 | response token 级别的 logprob。 |
| `worker_start_ts` | 建议 | Agent/Worker | Agent 开始执行 job 的 Unix 秒时间戳。 |
| `worker_finish_ts` | 建议 | Agent/Worker | Agent 完成 job 的 Unix 秒时间戳。 |
| `worker_latency_ms` | 建议 | Agent/Worker | Agent 执行总耗时。 |
| `model_latency_ms` | 建议 | Agent/Worker | 模型调用耗时。 |
| `metadata` | 建议 | Agent/Worker | 可记录 token 对齐、模型 endpoint、诊断信息等。 |

Server 会把这些字段转换到最终 `EpisodeResult`，并执行同一套 async 校验。

---

## parallel_mode 的读取规则

### Native Worker 路径

Worker 收到 `EpisodeRequest` 后，建议统一封装 helper 读取执行模式，顺序如下：

1. 优先读 `EpisodeRequest.parallel_mode`。
2. 其次读 `EpisodeRequest.metadata["parallel_mode"]`。
3. 再解析 `EpisodeRequest.payload` JSON 中的 `metadata.parallel_mode`。
4. 如果都没有，则默认为 `sync`。

### Agent/SWE 路径

Agent 收到 `AgentJob` 后，建议按下面顺序读取：

1. 优先读 `AgentJob.parallel_mode`。
2. 其次读 `AgentJob.metadata["parallel_mode"]`。
3. 如果都没有，则默认为 `sync`。

合法值只有：

```text
sync
one_step_off_policy
fully_async
```

如果收到其他值，执行侧应该返回 failed，推荐错误码：

```text
ERR_UNSUPPORTED_MODE = 1003
```

不要在非法模式下继续执行并返回 completed。

---

## 模型版本读取

两条路径要求相同：版本必须来自“同一次模型生成响应”，不能用本地缓存的当前版本替代。

优先读取响应体 JSON：

```json
{
  "uenv_model_version": {
    "rollout_param_version": 11,
    "rollout_policy_version": "actor-step-11"
  }
}
```

如果响应体没有，再 fallback 到 HTTP header：

```text
X-UEnv-Rollout-Param-Version: 11
X-UEnv-Rollout-Policy-Version: actor-step-11
```

如果 async 模式下读不到版本字段，执行侧应返回 failed，推荐错误码：

```text
ERR_MODEL_VERSION_MISSING = 3009
```

---

## rollout_log_probs 的请求、解析和校验

两条路径要求相同：调用 OpenAI-compatible `/v1/chat/completions` 时，需要显式请求 token-level logprob。

常见请求字段如下，具体以当前 gateway/vLLM 兼容实现为准：

```json
{
  "logprobs": true,
  "top_logprobs": 0
}
```

最终必须拿到与 response token 对齐的 logprob：

```text
len(rollout_log_probs) == len(response_ids)
```

要求：

- `rollout_log_probs` 对齐的是 response token，不包含 prompt token。
- 如果环境 token 或 tool token 不参与训练，应使用 `response_mask[i] = 0` 排除；对应 logprob 可以填 `0.0`。
- 如果模型 endpoint 不支持 logprobs，不能静默返回空数组。
- 如果 logprobs 为空，不能返回 completed。

推荐错误码：

```text
ERR_ROLLOUT_LOGPROBS_MISSING = 3010
ERR_MODEL_LOGPROBS_UNSUPPORTED = 3011
```

---

## 时间字段职责

Server 已经能设置 Server 自己知道的时间字段：

| 字段 | 谁设置 | 说明 |
|---|---|---|
| `enqueue_ts` | Server | 请求进入 Server 时生成或保留。 |
| `dispatch_ts` | Server | Server 派发给 Worker 或 Agent 前生成。 |
| `result_ready_ts` | Server | Server 收到并整理结果时生成。 |
| `server_latency_ms` | Server | Server 侧耗时。 |

执行侧需要设置 Worker/model 内部时间字段：

| 字段 | 谁设置 | Native Worker 载体 | Agent/SWE 载体 |
|---|---|---|---|
| `worker_start_ts` | 执行侧 | `EpisodeResult.worker_start_ts` | `AgentJobCompleteRequest.worker_start_ts` |
| `worker_finish_ts` | 执行侧 | `EpisodeResult.worker_finish_ts` | `AgentJobCompleteRequest.worker_finish_ts` |
| `worker_latency_ms` | 执行侧 | `EpisodeResult.worker_latency_ms` | `AgentJobCompleteRequest.worker_latency_ms` |
| `model_latency_ms` | 执行侧 | `EpisodeResult.model_latency_ms` | `AgentJobCompleteRequest.model_latency_ms` |

Server 只透传这些执行侧时间字段，不会生成这些值。

---

## 上报要求

### Native Worker：ReportResult

Worker 调 `ControlPlaneService.ReportResult` 时，必须原样带回 Server 派发时的租约信息：

- `episode_id`
- `attempt_id`
- `dispatch_lease_id`
- `dispatch_token`
- `scheduler_epoch`
- `worker_id`

`idempotency_key` 必须对同一次结果重试保持稳定。推荐格式：

```text
{episode_id}:{attempt_id}:{worker_id}:{dispatch_lease_id}
```

网络重试时不能每次生成新的 idempotency key，否则 Server 不能正确识别重复上报。

### Agent/SWE：complete_agent_job

Agent 调 `complete_agent_job` 时必须保持 job 身份一致：

- `job_id`
- `run_id`
- `agent_id`

并在 `AgentJobCompleteRequest` 顶层字段中写入 async 字段。Agent/SWE 路径不需要 `dispatch_lease_id` / `dispatch_token` / `idempotency_key`，这些是 Native Worker `ReportResult` 路径的控制面字段。

---

## 建议修改的代码位置

| 路径 | 文件 | 建议修改 |
|---|---|---|
| Native Worker | `uenv-worker/src/episode/executor.rs` | 普通 episode 和 Native SWE episode 构造 `EpisodeResult` 时写入 async 顶层字段。 |
| Native Worker | `uenv-worker/src/control_plane/client.rs` | `ReportResultRequest` 使用稳定 idempotency key，并带回 lease/token/epoch。 |
| Agent/SWE | Agent bridge / SWE agent 的 job completion 实现 | 构造 `AgentJobCompleteRequest` 时写入 async 顶层字段。具体文件以当前 Agent 实现位置为准。 |
| 两条路径共用 | `uenv-worker/src/llm.rs` | 调模型时请求 logprobs，并解析响应体/header 中的模型版本。 |
| 两条路径共用 | `uenv-worker/src/episode/model_client.rs` | 如果模型 HTTP 调用封装在这里，应返回 response text、response ids、logprobs、model version、latency。 |
| 两条路径共用 | `uenv-worker/src/wal/mod.rs` | WAL replay 的结果应保留新增 async 字段，避免重放时丢字段。 |

建议新增一个内部结构体，统一承载模型生成元信息：

```rust
struct RolloutModelMeta {
    rollout_param_version: Option<i64>,
    rollout_policy_version: Option<String>,
    rollout_log_probs: Vec<f32>,
    response_ids: Vec<i64>,
    response_mask: Vec<i32>,
    model_latency_ms: i64,
}
```

然后：

- Native Worker executor 把它写入 `EpisodeResult`。
- Agent/SWE completion 把它写入 `AgentJobCompleteRequest`。

---

## async 模式失败策略

当 `parallel_mode` 是 `one_step_off_policy` 或 `fully_async` 时，执行侧应该尽早失败并给出明确原因。

| 场景 | 执行侧行为 |
|---|---|
| 模型响应缺版本字段 | 返回 failed，错误码 `ERR_MODEL_VERSION_MISSING`。 |
| 模型不支持 logprobs | 返回 failed，错误码 `ERR_MODEL_LOGPROBS_UNSUPPORTED`。 |
| logprobs 为空 | 返回 failed，错误码 `ERR_ROLLOUT_LOGPROBS_MISSING`。 |
| logprobs 与 response_ids 长度不一致 | 返回 failed，错误信息写清 expected / actual。 |
| parallel_mode 非法 | 返回 failed，错误码 `ERR_UNSUPPORTED_MODE`。 |

Server 会执行统一校验，但执行侧应该提供更精确的错误码和错误信息。

---

## 测试清单

### 单元测试

建议至少补这些单测：

- Native Worker 从 `EpisodeRequest.parallel_mode` 读取模式。
- Native Worker 从 `EpisodeRequest.metadata` 读取模式。
- Native Worker 从 `payload.metadata` 读取模式。
- Agent/SWE 从 `AgentJob.parallel_mode` 读取模式。
- Agent/SWE 从 `AgentJob.metadata` 读取模式。
- async 模式缺版本时返回 failed。
- async 模式缺 logprobs 时返回 failed。
- logprobs 与 response_ids 长度不一致时返回 failed。
- sync 模式缺 logprobs 不影响原有 completed 行为。

### Worker + Server E2E

建议做以下端到端用例：

| 用例 | 路径 | 输入 | 预期 |
|---|---|---|---|
| 正例 | Native Worker | `parallel_mode = one_step_off_policy`，模型响应带版本和 logprobs | Server 返回 completed。 |
| 负例 | Native Worker | `parallel_mode = one_step_off_policy`，缺 logprobs | Worker 返回 failed；如果 Worker 漏拦，Server 转 failed，错误码 1004。 |
| 正例 | Agent/SWE | `AgentJob.parallel_mode = one_step_off_policy`，Agent completion 带版本和 logprobs | Server 返回 completed。 |
| 负例 | Agent/SWE | Agent completion 缺 logprobs | Agent 返回 failed；如果 Agent 漏拦，Server 转 failed，错误码 1004。 |
| header fallback | 两条路径 | response body 没有 `uenv_model_version`，header 有版本字段 | 执行侧能写入版本字段。 |
| 时间字段 | 两条路径 | result/complete request 中写入 worker/model 时间字段 | Server 返回中能看到执行侧时间字段和 Server 时间字段。 |

---

## 完整链路目标

最终链路应变成：

```text
VeRL / Adapter 写 parallel_mode
  -> Server 读取并保存模式
  -> Server 将模式下发给 Native Worker 或 Agent/SWE
  -> 执行侧调用模型，并请求 logprobs
  -> 执行侧从同一次模型响应读取版本和 token logprobs
  -> Native Worker 写入 EpisodeResult 顶层 async 字段，或 Agent/SWE 写入 AgentJobCompleteRequest 顶层 async 字段
  -> Server 校验 async completed 结果
  -> Adapter 回填 VeRL AgentLoopOutput / DataProto
```
