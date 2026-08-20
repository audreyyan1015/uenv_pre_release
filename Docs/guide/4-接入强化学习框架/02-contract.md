# 接口与数据契约

本文介绍强化学习框架与 UEnv 之间的公开网络协议（wire API）。无论 Bridge 用什么语言实现，都应以仓库中的 `proto/uenv/v1/adapter_core.proto` 为唯一真源。

这个接口的核心关系很简单：

```text
一个 SampleEnvelope  --request_id-->  UEnv 中的一次 Episode
一个 SampleResult    --request_id-->  这次 Episode 的执行结果
```

`env_config_json`、`reward_config_json` 中的业务字段由具体环境定义，不是 `AdapterCoreService` 的固定字段。本文只解释公开外层协议和当前实现会实际读取的通用字段。

## `AdapterCoreService`

完整服务名为 `uenv.bridge.v1.AdapterCoreService`。

| RPC | 请求 | 响应 | 用途 |
|---|---|---|---|
| `HealthCheck` | `HealthCheckRequest` | `HealthCheckResponse` | 检查 gRPC 服务是否可达，并读取服务版本。 |
| `ExecuteBatch` | `ExecuteBatchRequest` | `ExecuteBatchResponse` | 一次提交一个有界批次，等待批次中的所有 Episode 结束后返回。首次接入建议先使用这个 RPC。 |
| `ExecuteBatchStream` | `stream SampleEnvelope` | `stream SampleResult` | 双向流式提交；每个 sample 独立执行，结果按完成顺序返回。 |

`HealthCheckResponse` 只包含：

| 字段 | 类型 | 含义 |
|---|---|---|
| `ok` | `bool` | 当前实现在 RPC 可达时返回 `true`。 |
| `version` | `string` | 当前服务程序的 package 版本。 |

> **注意：** `HealthCheck` 成功只说明 `AdapterCoreService` RPC 存活。它不检查是否有可用 Worker，也不检查指定环境或模型端点能否执行。

## `ExecuteBatch` 的外层消息

### `ExecuteBatchRequest`

| 字段 | protobuf 类型 | 谁提供 | 必填性 / 默认 | 说明 |
|---|---|---|---|---|
| `request_id` | `string` | Bridge | 协议默认为空；当前服务不强制校验 | 这一次批量 RPC 的 ID，会原样回填到响应。为了排查问题，客户端应始终填写。 |
| `batch_id` | `string` | Bridge | 协议默认为空；当前服务不强制校验 | 这一次 RPC 的批次 ID，会原样回填到响应。客户端应使它与各 sample 的 `batch_id` 一致。 |
| `samples` | `repeated SampleEnvelope` | Bridge | 可为空，但空批次不执行任何 Episode | 本批次要执行的 rollout。 |

### `ExecuteBatchResponse`

| 字段 | protobuf 类型 | 来源 | 说明 |
|---|---|---|---|
| `request_id` | `string` | 请求外层 | 原样回填 `ExecuteBatchRequest.request_id`。 |
| `batch_id` | `string` | 请求外层 | 原样回填 `ExecuteBatchRequest.batch_id`。 |
| `results` | `repeated SampleResult` | UEnv | 每个已提交 sample 对应一个结果。客户端应按 `request_id` 关联，不要用 `zip(samples, results)` 作为唯一对齐方式。 |

当前 `ExecuteBatch` 实现会检查 Episode 结果数量、未知 ID 和重复 ID。如果底层返回的结果无法与提交的 sample 一一对应，整个 RPC 失败，而不是生成一个伪造的默认结果。

## `SampleEnvelope`：一次 rollout 的输入

可以先用下面这四句话判断字段应放在哪里：

- 环境如何初始化、本条任务的数据是什么 → `env_config_json`。
- Episode 最多走几步、使用什么随机种子 → `episode_config_json`。
- 如何判分、标准答案或 rubric 是什么 → `reward_config_json`。
- UEnv Worker 如何访问当前策略模型 → `model_endpoint`。

### 身份与路由

| 字段 | protobuf 类型 | 谁提供 | 必填性 / 默认 | 说明 |
|---|---|---|---|---|
| `request_id` | `string` | Bridge | **服务端强制非空** | 这次 rollout 的唯一 ID，内部直接用作 `episode_id`，也由 `SampleResult` 回填。 |
| `batch_id` | `string` | Bridge | **服务端强制非空** | 归属的框架批次，同时用作默认的关联/调度分组 ID。 |
| `sample_index` | `uint32` | Bridge | protobuf 默认 `0`，当前服务不校验唯一性 | sample 在框架批次中的稳定索引，会由结果回填。 |
| `framework` | `string` | Bridge | **服务端强制非空** | 产生 sample 的框架标识，例如 `verl`。 |
| `env_type` | `string` | 任务/环境配置 | 协议默认为空；**完整执行实际必填** | UEnv 用它选择环境和 Worker。当前 Adapter Core 不会提前拒绝空值，因此 Bridge 必须先校验。 |
| `parallel_mode` | `string` | Bridge | 空字符串按 `sync` 处理 | 只接受 `sync`、`one_step_off_policy` 或 `fully_async`；其他非空值会被拒绝。首次接入使用 `sync`。 |

### 环境、Episode 与奖励配置

| 字段 | protobuf 类型 | 谁提供 | 必填性 / 默认 | 当前通用层行为 |
|---|---|---|---|---|
| `env_config_json` | `bytes` | 环境适配代码 | 协议可空；实际必填字段由 `env_type` 决定 | UTF-8 JSON，整个值会作为环境配置传给 Worker。通用层还会读取 `question`/`raw_prompt`、`dataset`/`data_source` 和可选 `response_text`。 |
| `episode_config_json` | `bytes` | Bridge | 协议可空 | 当前 Adapter Core **只读取** `max_steps` 和 `seed`，两者必须是 i32 范围内的 JSON 整数。缺少时分别向下游传 `0` 和“未设置”。 |
| `reward_config_json` | `bytes` | 环境/奖励适配代码 | 协议可空；能否判分由目标环境决定 | `{"type":"rule_reward", ...}` 原样传递；如果存在 `rubric_config.ground_truth`，通用层会转成 `rule_reward` 的 `target`；其他值原样传递。 |

> **JSON 需要由 Bridge 先校验：** 这三个字段在 protobuf 中都是 `bytes`。当前 Adapter Core 不会因空字节或非法 JSON 拒绝请求，而是把解析失败的值当作 JSON `null`。为避免配置静默丢失，Bridge 应在提交前确认它们是 UTF-8 JSON，并按目标环境的约定检查类型和必填键。

### 模型端点

`model_endpoint` 的 protobuf 类型是 `ModelEndpoint`。消息本身可以缺省；在完整 rollout 模式中，如果 Worker 需要调用当前策略模型，则必须提供 Worker 实际可访问的端点。

| 字段 | protobuf 类型 | 谁提供 | 必填性 / 默认 | 说明 |
|---|---|---|---|---|
| `endpoint_type` | `string` | 模型服务适配代码 | 默认空字符串 | 端点类型。公开 wire 层不自动填入 `http`。 |
| `url` | `string` | 模型服务/框架 | 默认空字符串；需要模型调用时必填 | 必须从 UEnv Worker 所在网络访问；远程 Worker 不能使用框架主机的 `127.0.0.1`。 |
| `model_name` | `string` | 模型服务/框架 | 默认空字符串 | 传给模型服务的模型名称；是否必填取决于端点。 |
| `generation_config_json` | `bytes` | 强化学习框架 | 默认空字节 | UTF-8 JSON 生成参数。当前 Adapter Core 不校验其 JSON 合法性。 |
| `max_retries` | `int32` | Bridge | 默认 `0` | 表示模型端点层的重试上限；Adapter Core 只原样传给下游，是否执行由实际模型适配实现决定。 |

### 超时、关联与环境包

| 字段 | protobuf 类型 | 谁提供 | 必填性 / 默认 | 当前通用层行为 |
|---|---|---|---|---|
| `timeout_seconds` | `int32` | Bridge | `<= 0` 时使用 `300` | 单个 Episode 的业务超时，不是 gRPC deadline。 |
| `correlation_id` | `string` | Bridge | 空字符串时使用 `sample.batch_id` | 用于跨框架、Server 和 Worker 追踪这次执行。 |
| `sample_context_json` | `bytes` | Bridge | 默认空字节 | UTF-8 JSON 上下文。当前非 object 或解析失败的值会被忽略；不得放入 API Key 等密钥。 |
| `env_package_id` | `string` | 环境配置 | 默认空字符串 | 留空时会回退读取 `env_config_json.env_package_id` 或 `package_id`。 |
| `env_package_version` | `string` | 环境配置 | 默认空字符串 | 留空时会回退读取 `env_config_json.env_package_version` 或 `package_version`。 |

如果显式的 `env_package_id` / `env_package_version` 与 `env_config_json` 中的回退值同时存在且不一致，当前实现会以非法请求拒绝，不会猜测应使用哪一个。

## `SampleResult`：一次 rollout 的输出

| 字段 | protobuf 类型 | 当前值的来源 / 默认 | Bridge 应如何使用 |
|---|---|---|---|
| `request_id` | `string` | 来自底层 `EpisodeResult.episode_id` | 用它找回原 `SampleEnvelope`；未知或重复 ID 是协议错误。 |
| `batch_id` | `string` | 从原 `SampleEnvelope.batch_id` 恢复 | 校验结果属于预期批次。 |
| `sample_index` | `uint32` | 从原 `SampleEnvelope.sample_index` 恢复 | 在按 ID 校验后，用于恢复框架顺序。 |
| `status` | `string` | 原样传递底层 Episode 状态 | 只有明确识别为成功的状态才能进入训练更新；当前正常成功值为 `completed`。 |
| `reward` | `double` | `EpisodeResult.summary.total_reward`；缺少 summary 时为 `0.0` | 回填框架 reward。状态失败时不要把默认 `0.0` 当作模型能力得分。 |
| `done` | `bool` | 当前 Adapter Core 仅对 `completed` / `failed` / `timeout` 设为 `true` | 不要用它单独判断成功。例如底层可返回 `cancelled`，当前该值为 `false`。 |
| `termination_reason` | `string` | `EpisodeResult.summary.terminate_reason`；缺少时为空 | 写入框架输出和日志，不要代替 `status`。 |
| `trajectory_json` | `bytes` | 由 Episode trajectory 序列化；也可能是空字节 | 非空时按下文 schema 解析。 |
| `error_code` | `string` | 底层枚举数值转成十进制字符串；缺少时为空 | 与 `status` / `error_message` 一起保留。不要假定它是符号名。 |
| `error_message` | `string` | 底层错误信息；缺少时为空 | 用于失败诊断，不转换成 reward。 |
| `rollout_param_version` | `int64` | 底层可选值；缺少时会变成 `0` | 异步训练用。当前 wire 层无法区分“缺少”和真实版本 `0`。 |
| `rollout_policy_version` | `string` | 底层可选值；缺少时为空 | 异步训练用。 |
| `rollout_log_probs` | `repeated float` | 底层 `EpisodeResult.rollout_log_probs` | 如果框架使用，必须自行验证与 response token 对齐；Adapter Core 当前不做长度校验。 |

### `trajectory_json` 的完整结构

`trajectory_json` 有三种形态：

1. 底层有完整 trajectory 时，返回包含 steps 的 JSON object。
2. 没有内联 trajectory，但有 `trajectory_id` 或 metadata 时，返回 `steps: []` 的 JSON object。
3. 内联 trajectory、`trajectory_id` 和 metadata 都不存在时，返回空字节 `b""`。空字节不是 JSON，解析前必须先判空。

非空时，当前所有已知字段由下面的 JSON Schema 描述。其中 `$schema` 是 schema 文档本身的声明，不是实际 `trajectory_json` 里的字段。客户端应忽略未来可能增加的字段。

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "required": ["steps", "total_reward", "total_steps"],
  "properties": {
    "steps": {
      "type": "array",
      "items": {
        "type": "object",
        "required": [
          "step_index",
          "observation",
          "action",
          "reward",
          "terminated",
          "truncated",
          "info",
          "duration_ms",
          "rollout_trace"
        ],
        "properties": {
          "step_index": {"type": "integer"},
          "observation": {"type": "string"},
          "action": {"type": "string"},
          "reward": {"type": "number"},
          "terminated": {"type": "boolean"},
          "truncated": {"type": "boolean"},
          "info": {
            "type": "object",
            "additionalProperties": {"type": "string"}
          },
          "duration_ms": {"type": "integer"},
          "rollout_trace": {
            "type": "object",
            "properties": {
              "response_ids": {
                "type": "array",
                "items": {"type": "integer"}
              },
              "response_mask": {
                "type": "array",
                "items": {"type": "integer"}
              }
            },
            "additionalProperties": true
          }
        },
        "additionalProperties": true
      }
    },
    "total_reward": {"type": "number"},
    "total_steps": {"type": "integer"},
    "trajectory_id": {"type": "string"},
    "metadata": {
      "type": "object",
      "additionalProperties": {"type": "string"}
    }
  },
  "additionalProperties": true
}
```

还需要注意：

- `response_ids` 和 `response_mask` 不在 `SampleResult` 顶层，而在 `steps[*].rollout_trace` 中。
- 某一步没有 rollout trace 时，当前序列化结果是空 object `{}`；有 trace 时会同时输出两个数组。Bridge 应检查两个数组等长。
- `observation` 和 `action` 由底层 bytes 以 lossy UTF-8 转成字符串；它们不适合承载需要逐字节无损还原的二进制数据。
- `total_reward` / `total_steps` 在有内联 trajectory 时来自 trajectory；只有外壳时分别来自 summary 和其默认值。

## ID 与状态的最小规则

1. 每次新 rollout 都生成新的非空 `request_id`。同一 `ExecuteBatch` 内的重复 ID 会被拒绝。
2. 当前流式 RPC 会逐 sample 校验，不会在整条流上替客户端检测重复 `request_id`；Bridge 仍必须保证全局不冲突。
3. `SampleEnvelope.batch_id` 必须非空。当前服务不会校验它是否等于 `ExecuteBatchRequest.batch_id`，Bridge 必须自己保持一致。
4. 先按 `request_id` 建立结果索引，再校验 `batch_id` 和 `sample_index`。不识别的 ID、重复结果或缺失结果都应使本批次失败。
5. 默认只把 `status == "completed"` 的结果交给训练更新，并且仍要校验 reward 和所需 trace。`failed`、`cancelled` 或未知状态都不能仅因 `done` 的值而被当作成功。

超时与 gRPC deadline 的区别、重试与幂等、取消、背压、流式并发和异步策略版本属于运行时行为，见[生产运行语义](./06-runtime-semantics.md)。
