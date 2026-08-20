# 生产运行语义

完成 `encode_sample` 和 `decode_result` 后，框架已经可以跑通一个 UEnv 批次。要在长时间训练中稳定运行，还需明确返回顺序、超时、重试、容量、取消和异步策略。本页专门说明这些运行边界；第一次实现映射时，先阅读[自定义强化学习框架接入](./03-custom-framework.md)。

## 选择 unary 还是 stream

`AdapterCoreService` 提供两种执行方式：

| 接口 | 适合场景 | 运行特点 |
|---|---|---|
| `ExecuteBatch` | 首次接入、有界批次、同步训练 | 一次发送整个批次，等待一个完整响应，实现最简单 |
| `ExecuteBatchStream` | 持续产生 sample、需要逐条回收结果或更高吞吐 | 双向流；发送和接收必须独立进行，并限制 in-flight |

新接入建议先用 unary 验证映射和错误语义，只在吞吐或延迟确实需要时切换到 stream。切换时不应改变 `request_id` 或 encode/decode 逻辑。

stream 中某条 sample 发生协议错误时，整个 RPC 可能以 gRPC 错误结束。此时已提交但未收到结果的 ID 处于“结果未知”状态，不能当作从未执行。

## 结果可以乱序

unary 和 stream 都必须按 `request_id` 关联输入和输出。stream 会自然按 Episode 完成顺序返回；unary 也不应假定 Server 响应顺序与请求数组一致。

框架侧维护以下状态：

```text
request_id -> 原 sample、框架批次位置、rollout 序号、提交时的策略版本
```

遇到未知 ID、重复 ID、缺失 ID，或 `batch_id` / `sample_index` 与本地记录不符时，应视为协议错误，而不是猜测它属于哪条 sample。

## Episode timeout 与 gRPC deadline

这是两个不同的计时器：

| 设置 | 作用范围 | 到期后的含义 |
|---|---|---|
| `SampleEnvelope.timeout_seconds` | UEnv 中的 Episode 业务超时 | UEnv 应以 timeout/失败终态结束 Episode，并记录终止原因 |
| gRPC 调用的 `timeout=` | client 等待该 RPC 的 deadline | client 停止等待；不证明已提交 Episode 已停止 |

通常将 gRPC deadline 设为大于 Episode timeout，并为排队、结果序列化和网络留出余量。批量中各 Episode 可以有各自的 `timeout_seconds`，而 unary RPC 只有一个整体 deadline。

如果 gRPC deadline 先到，将未收到的 `request_id` 标记为结果未知，不要立即生成新 ID 再提交，否则可能让两个 Episode 同时执行。

## 重试与幂等

首先区分“传输失败”和“Episode 业务失败”：

- 连接中断、临时不可用和容量拒绝可能适合传输级重试。
- `SampleResult.status` 为 `failed` / `timeout`、模型输出低分或 reward 为 0 都是已完成的业务结果，不做传输重试。

同一逻辑 Episode 的重放必须保持原 `request_id`、原 payload 和原策略版本。新的 `request_id` 表示新 rollout，不是重试。

但“保持原 ID”只是幂等重放的必要条件，不是可以无条件重放的证明。在 deadline、stream 中断或未收到响应时，原请求可能已被接收。只有在当前部署的重放/查重语义经协议测试验证后，才自动重试这类结果未知的请求。

当前仓库中的 Python `RustCoreEpisodeClient` 会对 `UNAVAILABLE`、`CANCELLED`、`UNKNOWN`、`INTERNAL` 和 `DEADLINE_EXCEEDED` 进行有限次传输重试，**不会自动重试 `RESOURCE_EXHAUSTED`**。[自定义框架页](./03-custom-framework.md)使用的原始 `AdapterCoreServiceStub` 也没有添加应用级重试；如果需要，必须在框架接入层显式实现并测试。

## 背压与 `RESOURCE_EXHAUSTED`

AdapterCore 会限制 pending batch 数；超出上限时，unary 和 stream 都可能返回 `RESOURCE_EXHAUSTED`。当前默认上限为 64 个 pending batch，可由部署方通过 `UENV_MAX_PENDING_BATCHES` 调整；单个 stream 内并行处理的 sample 上限默认为 64，由 `UENV_MAX_STREAM_SAMPLES` 调整。这些是 AdapterCore 上限，不等于 Worker 实际容量。

框架侧应同时限制：

- 正在等待的 unary batch 数；
- stream 中已发送但未收到结果的 sample 数；
- 单个 batch 的 sample 数和消息字节数。

收到 `RESOURCE_EXHAUSTED` 时，先降低并发或拆小后续批次，再使用带随机扰动的有界退避。不要用无上限循环立即重试，否则会加重拥塞。由于当前 Python client 不会为该状态自动重试，框架侧必须明确选择“退避后重试”或“结束本批”。

## 失败如何进入训练

默认使用 fail fast：只有 `status=completed` 且 token/mask/reward 校验通过的结果才进入参数更新。以下情况不能默默变成普通的零分样本：

- 模型 endpoint 不可达；
- UEnv Worker 或环境容器失败；
- gRPC 超时或中断；
- 结果 ID、token/mask 或协议字段不一致。

只有当任务定义明确允许时，才可以使用 `zero_reward` 失败策略。此时同时保留原 `status`、`error_code`、`error_message` 和终止原因，并使用零长度或全 0 mask 确保该位置不产生训练梯度。

## 异步训练与 staleness

异步训练中，提交 rollout 后策略可能已更新。`SampleResult` 提供：

- `rollout_param_version`：rollout 使用的参数版本；
- `rollout_policy_version`：可追溯的策略/模型版本；
- `rollout_log_probs`：rollout 时的 token 级 logprob。

框架接入必须事先定义：

1. 当前训练参数版本与 rollout 版本的差如何计算。
2. 允许的最大 staleness。
3. 过期结果是丢弃、重新 rollout，还是使用框架支持的 off-policy 校正。
4. 权重切换时，in-flight rollout 继续使用旧版本还是停止新提交并排空。

将提交时期望的策略版本与 `request_id` 一起保存，在 `decode_result` 中比对返回版本。如果使用 `rollout_log_probs`，还要验证它与框架实际训练的 response token 对齐；不能把不同 rollout 或不同 tokenizer 的 logprob 混用。

## 取消的当前边界

当前 `AdapterCoreService` **没有取消 RPC**。关闭 `ExecuteBatchStream`、取消本地 gRPC call 或关闭 channel，可以让框架停止发送/等待，但不保证已提交的 Episode 在 UEnv Server/Worker 中停止物理执行。

因此框架取消时应：

1. 立即停止产生和提交新 sample。
2. 记录所有已提交但未收到终态的 `request_id`。
3. 关闭发送协程、stream/call 和 channel。
4. 把剩余 ID 标记为结果未知，而不是声称 Episode 已取消。

UEnv Server 的管理面有独立的 Episode 取消能力，但它不属于本页的框架侧 `AdapterCoreService` 契约。如果运维流程需要硬取消，应通过经授权的管理入口单独设计和验收，不要在框架适配器中暗中依赖它。

## `HealthCheck` 的边界

当前 `HealthCheck` 在 AdapterCore 进程可响应时返回 `ok=true` 和组件版本。它不检查：

- 是否有满足目标 `env_type` 的可用 Worker；
- 环境包或容器是否能启动；
- `model_endpoint` 是否能从 Worker 访问；
- reward 配置是否有效；
- 当前队列是否仍有容量。

所以可以用 `HealthCheck` 做进程存活和版本检查，但 readiness 必须再提交一条与真实任务相同的小型 Episode，并验证终态、reward 和 trajectory。

## 安全与可观测性

生产接入至少遵守以下要求：

- 明文 gRPC 只用于受控内网；跨不可信网络时使用组织批准的 TLS 入口或隧道。
- API Key、token 和凭据通过服务配置或 secret manager 传入，不放入 `env_config_json`、`reward_config_json`、`sample_context_json` 或普通日志。
- 对 Worker 可访问的 `model_endpoint` 使用网络白名单和最小权限，不允许训练数据任意指定内网 URL。
- prompt、response、源码和 trajectory 按同一数据分级、脱敏和保留策略处理。

每个训练结果至少要能用以下标识串起来：

| 标识 | 用途 |
|---|---|
| `run_id` | 一次训练作业 |
| `batch_id` | 一个框架批次 |
| `request_id` / `episode_id` | 一个逻辑 rollout；在 AdapterCore 映射中两者相关联 |
| `correlation_id` | 跨框架、Server 和 Worker 日志检索 |
| `trajectory_id` | 轨迹存储和追溯 |

同时记录框架阶段、RPC 类型、gRPC status、等待时间、Episode 终态、错误码、策略版本和 staleness 决策，才能区分映射错误、容量不足、环境失败和模型质量问题。

## 上线前检查

- 乱序、重复、缺失和未知 ID 都有确定的处理。
- Episode timeout 与 gRPC deadline 分开配置并测试。
- 重试保持原 ID/payload，且结果未知场景的重放语义已验证。
- unary batch 和 stream in-flight 都有上限，`RESOURCE_EXHAUSTED` 不会引发无界重试。
- 基础设施失败不会伪装成普通零分训练数据。
- 异步训练已定义版本来源、最大 staleness 和过期处理。
- 取消流程不会将“client 停止等待”误报为“Episode 已停止”。
- `HealthCheck` 之外还有目标环境的真实 readiness Episode。
- 密钥、model endpoint 和轨迹数据符合组织的安全策略。

这些运行项通过后，仍需将框架的发布入口、固定版本、限制和验收证据登记到[支持状态与接入验收](./05-support-matrix.md)。
