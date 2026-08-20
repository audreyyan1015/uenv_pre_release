# 接入强化学习框架

> 如果你只想用 UEnv 运行已支持的训练，不需要自己对接框架。请直接阅读[强化学习训练指南](../3-运行任务/07-post-training.md)。本组文档面向需要开发或维护框架接入的读者。

UEnv 向强化学习框架提供的是 **Episode 级 rollout 接口**，而不是需要框架逐步调用的 `reset()` / `step()` 接口。一条训练 sample 被转换成一个请求，UEnv 完成调度和环境执行后，返回这次 Episode 的 reward 与 trajectory。

## 先看结论

| 问题 | 答案 |
|---|---|
| UEnv 提供什么接口？ | gRPC 服务 `uenv.bridge.v1.AdapterCoreService`，包含健康检查、批量执行和流式执行三个 RPC。 |
| 输入是什么？ | 每次 rollout 对应一个 `SampleEnvelope`：它描述环境、Episode、奖励、模型端点和用于对齐结果的 ID。 |
| 输出是什么？ | 每次 rollout 对应一个 `SampleResult`：它包含状态、reward、trajectory、终止原因和错误信息。 |
| 框架接入者实现什么？ | 在 rollout 前找到合适的 hook，实现 `framework sample -> SampleEnvelope` 和 `SampleResult -> framework output` 两个映射，并管理 gRPC client 的生命周期。 |
| UEnv 负责什么？ | UEnv Server 管理和调度 Worker；UEnv Worker 调用模型、执行环境并计算 reward。 |

公开协议的唯一真源是仓库中的 `proto/uenv/v1/adapter_core.proto`。框架接入不应依赖 UEnv Server 或 Worker 的内部 Rust 类型。

## 一次 rollout 如何通过 UEnv

1. 强化学习框架选出一条训练 sample。
2. 框架侧的接入代码把 sample 编码为 `SampleEnvelope`。
3. 接入代码调用 UEnv Server 的 `AdapterCoreService`。
4. UEnv Server 把请求调度给合适的 UEnv Worker。
5. Worker 完成模型调用、环境交互和 reward 计算。
6. UEnv 返回 `SampleResult`，接入代码按 `request_id` 找回原 sample，再转成框架需要的 rollout 输出。
7. 强化学习框架继续计算 loss、更新参数并保存 checkpoint。

`request_id` 是这条调用链上的主键。不要依赖结果在数组中的位置；流式调用的结果会按完成顺序返回。

## Bridge 是什么

本组文档中的 **UEnv Bridge** 是运行在强化学习框架一侧的适配层，也就是你需要实现或复用的接入代码。它可以是框架内的一个 hook、AgentLoop 实现或独立 Python 模块。

Bridge 不是另一个需要框架开发者实现的调度服务，也不执行环境。它只负责翻译两边的数据，并通过 gRPC 调用 UEnv Server。

| 框架侧（含 Bridge） | UEnv 侧 |
|---|---|
| 选择训练 sample，决定 rollout 数量 | 管理 Worker 注册、心跳、能力与容量 |
| 为每次 rollout 生成稳定且唯一的 `request_id` | 调度 Episode 并执行环境 |
| 构造 `SampleEnvelope` | 调用 Worker 可访问的模型端点 |
| 按 ID 校验和还原 `SampleResult` | 计算 reward，产生状态和 trajectory |
| 转成框架需要的 token、mask、reward 等输出 | 返回公开协议中的 `SampleResult` |
| 计算 loss、更新模型、保存 checkpoint | 不参与框架的优化器和参数更新 |

## 先选择接入模式

接入时首先要说清楚“模型响应由谁生成”。两种模式的数据来源不同，不应混在同一个示例里。

### 完整 rollout（推荐）

框架把任务、环境配置和当前策略的模型端点交给 UEnv。UEnv Worker 调用模型，完成环境交互和判分。

- 请求通常需要 `env_config_json`、`episode_config_json`、`reward_config_json` 和 `model_endpoint`。
- 结果中的 response token/mask 位于 `trajectory_json.steps[*].rollout_trace`；它们是否存在取决于实际 Worker/环境是否记录了 rollout trace。
- 适合希望 UEnv 接管模型调用和环境交互的训练流程。

### reward-only（后置判分）

框架先在本地完成生成，再把已生成的响应交给支持该用法的 UEnv 环境判分。当前适配层会把 `env_config_json.response_text` 传给 Worker，但只有目标环境明确支持外部 response 时才能使用此模式。

- token 和 mask 来自框架原本的生成结果，不要再从 UEnv 结果中猜测或重建。
- UEnv 返回 reward、状态和环境轨迹；此模式不能表述为“UEnv 完成了整个 rollout”。
- 是否可用、`response_text` 的具体要求和判分配置都由目标环境决定。

## 框架接入者需要实现什么

一个最小接入可以用两个映射函数表达：

```python
def encode_sample(sample, rollout_context) -> PreparedEnvelope:
    """Build one SampleEnvelope and retain data needed to restore the result."""


def decode_result(result, prepared) -> FrameworkRolloutOutput:
    """Validate one SampleResult and convert it to the framework's output."""
```

在这两个函数外，还需要完成三件事：

1. **选择 hook**：完整 rollout 模式的 hook 应位于“sample 已确定、rollout 尚未执行”的位置。
2. **保存对齐信息**：保留 `request_id -> 原 sample / 原位置` 的映射；一条 sample 如果要生成多个 rollout，每个 rollout 使用不同的 `request_id`。
3. **管理 client**：在任务开始时创建 gRPC channel/stub，多个 sample 复用它，在训练结束时关闭。

批次调用的核心逻辑是：

```python
prepared = [encode_sample(sample, context) for sample in samples]
results = client.execute_batch([item.envelope for item in prepared])
results_by_id = validate_and_index(results)
outputs = [
    decode_result(results_by_id[item.request_id], item)
    for item in prepared
]
```

实现时不要把某个数据集的 `question`、`target` 或 `rule_reward` 当成 UEnv 公开协议的固定字段；它们都是特定环境的 `env_config_json` / `reward_config_json` 内容。

## 推荐阅读顺序

1. [接口与数据契约](./02-contract.md)：准确了解 RPC、`SampleEnvelope` 和 `SampleResult`。
2. [自定义强化学习框架接入](./03-custom-framework.md)：用 Python 实现一个最小 Bridge。
3. [VeRL 强化学习接入](./04-verl.md)：了解已有 VeRL 适配层如何实现同一套契约。
4. [生产运行语义](./06-runtime-semantics.md)：在最小闭环跑通后，再处理流式调用、超时、重试、背压和异步策略版本。
5. [支持状态与接入验收](./05-support-matrix.md)：确认目标框架的当前支持等级和限制。
