# UEnv Server 代码改动总结：VeRL 异步接入

> 工作目录：服务器 `8.130.75.157:/home/yzy_uenv`，分支 `merge/bridge-verl-adapter`
>
> 本文档总结 **Server 侧** 为接入 VeRL `one_step_off_policy` / `fully_async` 所做的代码改动。
>
> 对比范围：当前 HEAD `ba48426 refactor server episode result completion` 相对 async 前基线 `e7c69ca merge verl bridge adapter into bridge alignment`。
>
> 改动规模：8 个文件，`+1308 / -736`。

---

## 一句话总结

这次 Server 侧改动把 UEnv 从“只透传普通 episode 结果”改为 **支持 VeRL 异步 rollout 语义**：协议层新增 async 字段，Server 保存并恢复 `parallel_mode` / 时间上下文，对 async completed 结果做必需字段校验，并把 Native Worker `ReportResult` 与 SWE/Agent completion 统一使用同一套 `complete_episode_result` 完成管线。

---

## 实现了什么功能

### 1. 协议层新增 VeRL async 字段

`EpisodeRequest` 新增了 Server 识别异步模式所需字段：

- `parallel_mode`：支持 `sync` / `one_step_off_policy` / `fully_async`。
- `enqueue_ts`：请求进入 Server 的时间戳。
- `metadata`：承载 Adapter 或上游透传的补充信息。

`EpisodeResult` 新增了 VeRL 训练侧需要回收的 rollout 字段和时间字段：

- `parallel_mode`
- `rollout_param_version`
- `rollout_policy_version`
- `rollout_log_probs`
- `metadata`
- `dispatch_ts`
- `worker_start_ts`
- `worker_finish_ts`
- `result_ready_ts`
- `server_latency_ms`
- `worker_latency_ms`
- `model_latency_ms`

### 2. Agent/SWE 协议也支持 async 字段

`AgentJob` 增加 `parallel_mode` / `enqueue_ts` / `metadata`，Server 派发 SWE/Agent job 时会把请求中的 async 上下文发送给 Agent。

`AgentJobCompleteRequest` 增加了与 `EpisodeResult` 对齐的 async 结果字段：

- `parallel_mode`
- `rollout_param_version`
- `rollout_policy_version`
- `rollout_log_probs`
- Worker/model 时间字段
- `metadata`

这意味着 Agent/SWE 路径不再只能返回 reward 和 trajectory，也可以把 VeRL 异步训练所需字段返回给 Server。

### 3. Server 保存并恢复 async 上下文

Server 在请求进入时会规范化 async 上下文：

1. 优先读取 `EpisodeRequest.parallel_mode`。
2. 其次读取 `EpisodeRequest.metadata["parallel_mode"]`。
3. 再从 `EpisodeRequest.payload` JSON 的 `metadata.parallel_mode` 读取。
4. 都没有时默认为 `sync`。

`ActiveEpisode` 和 `PendingResult` 记录：

- `parallel_mode`
- `enqueue_at`
- `enqueue_ts`
- `dispatch_at`
- `dispatch_ts`

这样 Native Worker 上报 `ReportResult` 时，即使只带 result，Server 仍能从 `PendingResult` 恢复原始 episode 的 async 上下文。

### 4. async completed 结果校验

Server 对 `one_step_off_policy` 和 `fully_async` 的 completed 结果执行严格校验：

- result 的 `parallel_mode` 必须与 request 一致。
- `rollout_param_version` 必须存在。
- `rollout_policy_version` 必须存在且非空。
- `rollout_log_probs` 必须非空。

如果 Worker 或 Agent 返回 `status = completed` 但缺少这些字段，Server 会把结果降级为 failed：

```text
error_code = ERR_ASYNC_PROTOCOL_MISSING_FIELD / 1004
error_message = missing rollout_log_probs 等具体原因
metadata["async_protocol_error"] = 具体原因
```

这个校验是 Server 校验能力，不替代 Worker 侧更精细的错误码。Worker 仍应该在模型版本或 logprobs 缺失时主动返回 failed。

### 5. Native Worker ReportResult 接入统一完成管线

Native Worker 上报结果的路径仍保留原有控制面安全语义：

- 校验 `server_epoch`。
- 校验稳定 `idempotency_key`。
- 校验 `dispatch_lease_id` / `dispatch_token`。
- 校验上报 worker 是否拥有该 lease。
- 处理重复上报、超时后迟到、取消后迟到等情况。

通过这些前置校验后，`control_plane.rs` 不再自己 finalize 和拼 `EpisodeResultRow`，而是调用统一函数：

```rust
complete_episode_result(
    state,
    request_for_result,
    result,
    Some(timing),
    Some(ResultPersistenceContext::native(worker_id, idempotency_key)),
    false,
)
```

这里 `publish = false`，因为 Native 路径最后仍通过 `pending.tx.send(result)` 交回正在等待的 `submit_episode`；广播发布由 `submit_episode` 收到结果后处理。

### 6. SWE/Agent completion 接入同一套完成管线

SWE/Agent 路径会先从 `AgentJobCompleteRequest` 构造 `EpisodeResult`，包括：

- `parallel_mode`
- `rollout_param_version`
- `rollout_policy_version`
- `rollout_log_probs`
- `worker_start_ts`
- `worker_finish_ts`
- `result_ready_ts`
- `worker_latency_ms`
- `model_latency_ms`
- `metadata`

然后调用同一套完成管线：

```rust
complete_episode_result(
    state,
    request,
    result,
    Some(timing),
    Some(ResultPersistenceContext::swe_agent(
        worker_id,
        job_id,
        env_package_id,
        agent_bridge_version,
    )),
    true,
)
```

因此 Native 与 SWE/Agent 的差异只剩前置调度、鉴权和返回通道；共同的 finalize、async validate、trajectory 落库、broadcast 发布都集中到 service 层。

### 7. 异步提交 / 轮询语义更新

`submit_episode_async` 增加幂等处理：

- 如果同一个 `episode_id` 已在 `completed_async` 中，直接返回。
- 如果同一个 `episode_id` 已在 `active_episode_handles` 中，直接返回。

`get_result` 改为非破坏读取：

```text
completed_async.get(episode_id).map(|result| result.clone())
```

这样异步轮询不会因为客户端重复 poll 把结果删除或丢失。

---

## 修改了哪些文件

### 协议 / 公共契约

| 文件 | 改动 |
|------|------|
| `proto/uenv/v1/episode.proto` | `EpisodeRequest` 增加 `parallel_mode` / `enqueue_ts` / `metadata`；`EpisodeResult` 增加 rollout 版本、logprobs、metadata 和时间字段。 |
| `proto/uenv/v1/agent.proto` | `AgentJob` 增加 async 上下文字段；`AgentJobCompleteRequest` 增加 async result 字段。 |
| `proto/uenv/v1/common.proto` | 新增 async 协议缺字段、模型版本缺失、logprobs 缺失、模型不支持 logprobs 等错误码。 |

### Server 端

| 文件 | 改动 |
|------|------|
| `uenv-server/src/state.rs` | `ActiveEpisode` / `PendingResult` 增加 async 上下文和时间字段；增加 `completed_async` 结果缓存相关状态。 |
| `uenv-server/src/service.rs` | **核心实现**：解析/规范化 `parallel_mode`；设置 `enqueue_ts` / `dispatch_ts` / `result_ready_ts` / `server_latency_ms`；校验 async completed 结果；实现 `submit_episode_async` 幂等和 `get_result` 非破坏读取；新增 `ResultPersistenceContext` / `persist_episode_result` / `publish_episode_result` / `complete_episode_result`，统一 Native 与 SWE/Agent 完成管线。 |
| `uenv-server/src/control_plane.rs` | Native Worker `ReportResult` 路径根据 `PendingResult` 恢复 async 上下文，接入 `complete_episode_result`；移除本文件内重复的 trajectory row 拼装逻辑。 |
| `uenv-server/src/agent_job.rs` | Agent job 派发和 completion 协议适配新增 async 字段，测试构造适配 proto 默认字段。 |
| `uenv-server/tests/swe_agent_orchestration.rs` | 测试中的 `AgentJobCompleteRequest` 构造适配新增字段。 |

---

## 设计说明

1. **Server 校验结果，不伪造训练数据**：Server 可以补自己知道的时间字段和 metadata，但不会伪造 Worker/model 的版本、logprobs、latency。`rollout_log_probs` 必须来自 Worker 或 Agent。
2. **completed 结果严格校验，failed 结果保留错误信息**：只有 async `completed` 结果要求版本和 logprobs 必须完整；Worker 主动返回 failed 时，Server 保留其错误信息。
3. **共同语义集中到 service 层**：Native `ReportResult`、SWE/Agent completion 的前置流程不同，但最终完成一个 episode result 的语义一致，统一进 `complete_episode_result`。
4. **异步 poll 非破坏读取结果**：`get_result` 非破坏读取，适配 VeRL 侧重复轮询和异常重试。
5. **部署入口是 adapter-core**：线上 systemd 跑的是 `/usr/local/bin/uenv-adapter-core`，`uenv-server` 是被它链接的 library，因此修改 server 逻辑后必须构建并重启 adapter-core。

---

## 已验证内容

### 编译检查

远端 `/home/yzy_uenv` 已执行：

```bash
/root/.cargo/bin/cargo check -p uenv-server
/root/.cargo/bin/cargo build -p uenv-adapter-core --release
```

结果通过。

备注：远端 stable toolchain 未安装 `cargo-fmt` 组件，因此 `cargo fmt` 没有执行；已执行 `git diff --check`，结果干净。

### GitHub 提交

代码已提交并推送到 GitHub：

```text
branch: merge/bridge-verl-adapter
HEAD: ba48426 refactor server episode result completion
remote: origin git@github-yzy-uenv:04beckman/yzy_uenv.git
```

GitHub 提示可创建 PR：

```text
https://github.com/04beckman/yzy_uenv/pull/new/merge/bridge-verl-adapter
```

### 线上部署验证

线上服务实际运行的二进制：

```text
/usr/local/bin/uenv-adapter-core
```

源码入口：

```text
/home/yzy_uenv/uenv-bridge/core/src/main.rs
```

已重新构建并替换运行二进制，随后执行：

```bash
systemctl restart uenv-server.service
```

服务状态验证：

```text
uenv-server.service active (running)
/usr/local/bin/uenv-adapter-core listening on 0.0.0.0:8088
trajectory HTTP listening on 0.0.0.0:8077
```

---

## Worker / Adapter 仍需实现的事项

Server 侧已经具备 async 协议处理、校验和统一完成逻辑，但完整 VeRL 训练链路还需要 Worker / Adapter 继续实现：

1. Worker 从同一次模型响应中读取真实 `rollout_param_version` / `rollout_policy_version`。
2. Worker 请求并解析 token-level `rollout_log_probs`。
3. Worker 校验 `len(rollout_log_probs) == len(response_ids)`。
4. Worker 在 Native `ReportResult` 和 Agent/SWE `AgentJobCompleteRequest` 两条路径都写入 async 顶层字段。
5. AdapterCore / Python Adapter 把 rollout 字段完整回填到 VeRL `AgentLoopOutput` / `DataProto`。

---

## 总结

Server 侧现在完成了五类核心能力：

1. 协议字段：Request / Result / AgentJob / AgentComplete 全部具备 async 字段。
2. 上下文传递：Server 能读取、保存、派发并回填 `parallel_mode` 和时间上下文。
3. 结果校验：async completed 结果缺版本或 logprobs 会被拒绝并转 failed。
4. 共同抽象：Native ReportResult 与 SWE/Agent completion 共享 `complete_episode_result` 完成管线。
5. 异步接口语义：`submit_episode_async` 幂等，`get_result` 非破坏读取。

这解决了之前的设计问题：不再是“一个功能要在 Native 和 Agent/SWE 两处各加一遍”，而是把共同完成语义统一放在 Server service 层。
