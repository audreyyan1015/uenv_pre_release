# clean proto 字段说明

本文档说明 `/home/uenv/proto-clean/proto/uenv/v1/*.proto` 中清理干净后的全部 message 字段。

干净版约定：

- 不再使用 `SampleEnvelope.payload_json`、`SampleEnvelope.meta_json` 和 `SampleEnvelope.model_output_json`，字段号 `6、7、18` 已 `reserved`。
- 不再使用 `CancelEpisodeResponse.cancelled`，字段号 `1` 已 `reserved`。
- `payload` 仍保留，但只承载环境/worker 业务输入，不承载调度、训练协议或结果字段。
- `metadata`、`info` 字段仍保留，但只承载上下文信息，不承载协议字段。
- `EpisodeRequest.model_endpoint = 8` 已删除并保留字段号，模型 URL、模型名、生成参数和重试次数统一放在 `model_endpoint_config`。
- `parallel_mode`、rollout 版本、logprobs、时间延迟、`response_ids/response_mask` 都使用类型化字段。
- 字段表里的“新增”表示该字段相对上一次代码提交中的原 proto 是否新增；`否（历史保留）` 表示该字段号只用于 `reserved`，不是业务字段。

## 相较于原 proto 的修改思路

原 proto 的主要问题是：很多真正属于“协议语义”的字段没有独立 schema，而是散落在 JSON 大包或自由字段里。
协议语义：通信双方必须共同理解、并会影响执行逻辑、调度逻辑、训练逻辑或结果解释的字段。

- bridge 到 adapter-core 的样本输入里，`payload_json` 同时承载 env 配置、episode 配置、reward 配置、model endpoint、metadata、timeout、rollout 输出等多种含义。
- `meta_json`、`payload.metadata`、`EpisodeRequest.metadata`、`EpisodeResult.metadata`、`StepRecord.info` 都曾经被用来兜底放协议字段。
- `response_ids/response_mask`、rollout 版本、logprobs、timing latency 等字段缺少统一位置，容易出现“写在 info 里、metadata 里、payload 里都能读”的多来源问题。
- `CancelEpisodeResponse.cancelled` 只能表达一个粗略结果，不能区分 server 本地取消是否成功、worker 物理执行是否真的停止。

这次改动采用两阶段迁移思路；截至当前代码状态，clean proto 已正式覆盖 `/home/uenv/proto`，运行主链路和正式 proto 都已收口到 typed 字段。

第一阶段是此前正式 proto 的兼容迁移版：

- 新增类型化字段，让新客户端优先写明确字段。
- 旧字段暂时保留字段号，避免 wire format 一次性破坏；当前主链路已经停止发送或读取已清理的旧入口。
- 早期兼容阶段曾要求多来源一致并记录迁移日志；当前 clean 收口方向是只接受 typed 来源。
- server、worker、adapter-core 在结果回填时过滤 metadata/info 中的协议 key，让协议字段逐步回到 typed 字段。

第二阶段是当前正式生效的 clean proto：

- 删除 `SampleEnvelope.payload_json = 6`、`SampleEnvelope.meta_json = 7` 和 `SampleEnvelope.model_output_json = 18`，并用 `reserved 6, 7, 18;` 保留字段号。
- 删除 `CancelEpisodeResponse.cancelled = 1`，并用 `reserved 1;` 保留字段号。
- `metadata` 和 `info` 字段本身不删除，但只保留上下文/环境自由信息，不再承载协议字段。
- 所有训练协议语义都进入类型化字段，例如 `parallel_mode`、`rollout_param_version`、`rollout_policy_version`、`rollout_log_probs`、`RolloutTrace` 和 timing latency 字段。
- 模型调用协议语义也进入类型化字段：`EpisodeRequest.model_endpoint_config` 承载 URL、model_name、generation_config_json 和 max_retries，不再写入 payload 或旧 string 字段。
- clean 输入侧不再接收上游预生成的 `model_output_json`；UEnv 自己做 rollout，模型输出相关信息进入输出侧的 `SampleResult`、`EpisodeResult`、`RolloutTrace`。

最终目标是把协议从“JSON 大包 + metadata/info 兜底”收敛成“typed proto 字段表达协议语义，metadata/info 只表达上下文”。

## EpisodeRequest payload / metadata 边界

`EpisodeRequest.payload` 不是旧 `SampleEnvelope.payload_json` 的替代品，也不是协议字段兜底容器。clean 语义下它只保存 worker 执行环境需要的业务输入。

可以保留在 `payload` 里的字段：

- 通用环境输入：`request_id`、`question`、`dataset`。
- SWE 执行业务输入：`instance_id`、`benchmark_variant`、`use_gold_patch`、`command_mode`、`driver_entrypoint`、`workspace_dir`、`llm_config_path`、`max_iterations`。
- 环境自定义上下文：不会影响 UEnv 调度、租约、训练协议或结果解释的上下文字段。

不应保留在 `payload` 或 `metadata` 里的字段：

- 调度/执行控制：`parallel_mode`、`timeout_seconds`、`correlation_id`、`dispatch_lease_id`、`dispatch_token`。
- 模型调用协议：`model_endpoint`、`model_endpoint_config`、`model_name`、`generation_config`、`env_package_id`、`env_package_version`。
- rollout 输入侧旧键：`response_text`、`response_ids`、`response_mask`、`rollout_log_probs`、`response_logprobs`、`response_log_probs`、`rollout_param_version`、`rollout_policy_version`、`uenv_model_version`。
- timing/result 字段：`enqueue_ts`、`dispatch_ts`、`worker_start_ts`、`worker_finish_ts`、`result_ready_ts`、`server_latency_ms`、`worker_latency_ms`、`model_latency_ms`。

这些字段如果属于协议语义，应进入 typed proto 字段；如果属于 rollout 结果，应由 UEnv 执行后写入 `SampleResult`、`EpisodeResult`、`RolloutTrace`，而不是作为请求 payload 传入。

## proto/uenv/v1/common.proto

### ErrorCode

错误码枚举：

- `ERROR_UNSPECIFIED = 0`：未指定。
- `OK = 1`：成功。
- `ERR_INVALID_REQUEST = 1001`：请求非法。
- `ERR_UNKNOWN_ENV_TYPE = 1002`：未知环境类型。
- `ERR_UNSUPPORTED_MODE = 1003`：不支持的执行模式。
- `ERR_ASYNC_PROTOCOL_MISSING_FIELD = 1004`：异步训练协议缺少必填字段。
- `ERR_NO_AVAILABLE_WORKER = 2001`：没有可用 worker。
- `ERR_WORKER_TIMEOUT = 2002`：worker 超时。
- `ERR_EPISODE_TIMEOUT = 2003`：episode 超时。
- `ERR_WORKER_CRASHED = 3001`：worker 崩溃。
- `ERR_ENV_INIT_FAILED = 3002`：环境初始化失败。
- `ERR_ENV_STEP_FAILED = 3003`：环境 step 失败。
- `ERR_MODEL_CALL_FAILED = 3004`：模型调用失败。
- `ERR_ALREADY_RUNNING = 3005`：任务已在运行。
- `ERR_ALREADY_COMPLETED = 3006`：任务已完成。
- `ERR_LEASE_EXPIRED = 3007`：dispatch lease 已过期。
- `ERR_LEASE_SUPERSEDED = 3008`：dispatch lease 被更新的 lease 取代。
- `ERR_MODEL_VERSION_MISSING = 3009`：缺少模型版本。
- `ERR_ROLLOUT_LOGPROBS_MISSING = 3010`：缺少 rollout logprobs。
- `ERR_MODEL_LOGPROBS_UNSUPPORTED = 3011`：模型侧不支持 logprobs。
- `ERR_INTERNAL = 5001`：内部错误。

### ResourceSpec

| 字段号 | 字段 | 类型 | 新增 | 含义 |
|---:|---|---|---|---|
| 1 | `cpu_cores` | `int32` | 否 | CPU 核数需求。 |
| 2 | `memory_mb` | `int32` | 否 | 内存需求，单位 MB。 |
| 3 | `gpu_count` | `int32` | 否 | GPU 数量需求。 |
| 4 | `gpu_type` | `string` | 否 | GPU 类型，例如 A100。 |

### ExecutionMode

执行模式枚举：

- `MODE_UNSPECIFIED = 0`：未指定。
- `MODE_SINGLE = 1`：单步/单任务模式。
- `MODE_MULTI = 2`：多步/多任务模式。
- `MODE_MODEL_CALLBACK = 3`：模型回调模式。
- `MODE_CUSTOM = 4`：自定义模式。

## proto/uenv/v1/adapter_core.proto

package：`uenv.bridge.v1`

### AdapterCoreService

- `ExecuteBatch(ExecuteBatchRequest) returns (ExecuteBatchResponse)`：批量提交样本。
- `ExecuteBatchStream(stream SampleEnvelope) returns (stream SampleResult)`：流式提交样本并流式返回结果。
- `HealthCheck(HealthCheckRequest) returns (HealthCheckResponse)`：健康检查。

### ExecuteBatchRequest

| 字段号 | 字段 | 类型 | 新增 | 含义 |
|---:|---|---|---|---|
| 1 | `request_id` | `string` | 否 | 批量请求 ID。 |
| 2 | `batch_id` | `string` | 否 | 批次 ID。 |
| 3 | `samples` | `repeated SampleEnvelope` | 否 | 本批次的样本列表。 |

### ExecuteBatchResponse

| 字段号 | 字段 | 类型 | 新增 | 含义 |
|---:|---|---|---|---|
| 1 | `request_id` | `string` | 否 | 对应请求 ID。 |
| 2 | `batch_id` | `string` | 否 | 对应批次 ID。 |
| 3 | `results` | `repeated SampleResult` | 否 | 每个样本的执行结果。 |

### ModelEndpoint

| 字段号 | 字段 | 类型 | 新增 | 含义 |
|---:|---|---|---|---|
| 1 | `endpoint_type` | `string` | 是 | 模型端点类型，例如 `http`。 |
| 2 | `url` | `string` | 是 | 模型服务 URL。 |
| 3 | `model_name` | `string` | 是 | 模型名称。 |
| 4 | `generation_config_json` | `bytes` | 是 | 生成参数 JSON，例如 temperature/top_p/max_new_tokens。 |
| 5 | `max_retries` | `int32` | 是 | 模型调用最大重试次数。 |

### SampleEnvelope

| 字段号 | 字段 | 类型 | 新增 | 含义 |
|---:|---|---|---|---|
| 1 | `request_id` | `string` | 否 | 单个样本请求 ID，最终映射为 episode_id。 |
| 2 | `batch_id` | `string` | 否 | 样本所属批次 ID。 |
| 3 | `sample_index` | `uint32` | 否 | 样本在批次中的下标。 |
| 4 | `framework` | `string` | 否 | 上游训练框架，例如 `verl`。 |
| 5 | `env_type` | `string` | 否 | 环境类型，例如 `math`、`swe`。 |
| 6 | reserved | `reserved` | 否（历史保留） | 历史字段号，禁止复用。 |
| 7 | reserved | `reserved` | 否（历史保留） | 历史字段号，禁止复用。 |
| 8 | `parallel_mode` | `string` | 是 | 训练并行模式：`sync`、`one_step_off_policy`、`fully_async`。 |
| 9 | `env_config_json` | `bytes` | 是 | 环境配置 JSON。 |
| 10 | `episode_config_json` | `bytes` | 是 | episode 配置 JSON，例如 max_steps、seed、initial_observation。 |
| 11 | `reward_config_json` | `bytes` | 是 | 奖励配置 JSON。 |
| 12 | `model_endpoint` | `ModelEndpoint` | 是 | 模型端点结构化配置。 |
| 13 | `timeout_seconds` | `int32` | 是 | 超时时间，单位秒。 |
| 14 | `correlation_id` | `string` | 是 | 跨层追踪 ID。 |
| 15 | `sample_context_json` | `bytes` | 是 | 样本上下文 JSON，只承载上下文信息。 |
| 16 | `env_package_id` | `string` | 是 | EnvPackage ID。 |
| 17 | `env_package_version` | `string` | 是 | EnvPackage 版本。 |
| 18 | reserved | `reserved` | 否（历史保留） | 迁移期 `model_output_json` 字段号，禁止复用。 |

### SampleResult

| 字段号 | 字段 | 类型 | 新增 | 含义 |
|---:|---|---|---|---|
| 1 | `request_id` | `string` | 否 | 对应样本请求 ID。 |
| 2 | `batch_id` | `string` | 否 | 对应批次 ID。 |
| 3 | `sample_index` | `uint32` | 否 | 对应样本下标。 |
| 4 | `status` | `string` | 否 | 执行状态，例如 `completed`、`failed`、`timeout`。 |
| 5 | `reward` | `double` | 否 | 样本 reward。 |
| 6 | `done` | `bool` | 否 | episode 是否结束。 |
| 7 | `termination_reason` | `string` | 否 | 终止原因。 |
| 8 | `trajectory_json` | `bytes` | 否 | 轨迹 JSON。 |
| 9 | `error_code` | `string` | 否 | 错误码字符串。 |
| 10 | `error_message` | `string` | 否 | 错误信息。 |
| 11 | `rollout_param_version` | `int64` | 是 | rollout 使用的模型参数版本。 |
| 12 | `rollout_policy_version` | `string` | 是 | rollout 使用的策略版本。 |
| 13 | `rollout_log_probs` | `repeated float` | 是 | token 级 rollout logprob。 |

### HealthCheckRequest

无字段。

### HealthCheckResponse

| 字段号 | 字段 | 类型 | 新增 | 含义 |
|---:|---|---|---|---|
| 1 | `ok` | `bool` | 否 | 服务是否健康。 |
| 2 | `version` | `string` | 否 | adapter-core 版本。 |

## proto/uenv/v1/episode.proto

package：`uenv.v1`

### ModelEndpoint

| 字段号 | 字段 | 类型 | 新增 | 含义 |
|---:|---|---|---|---|
| 1 | `endpoint_type` | `string` | 是 | 模型端点类型，例如 `http`。 |
| 2 | `url` | `string` | 是 | 模型服务 URL。 |
| 3 | `model_name` | `string` | 是 | 模型名称。 |
| 4 | `generation_config_json` | `bytes` | 是 | 生成参数 JSON，例如 temperature/top_p/max_new_tokens。 |
| 5 | `max_retries` | `int32` | 是 | 模型调用最大重试次数。 |

### EpisodeRequest

| 字段号 | 字段 | 类型 | 新增 | 含义 |
|---:|---|---|---|---|
| 1 | `episode_id` | `string` | 否 | episode 唯一 ID。 |
| 2 | `attempt_id` | `uint32` | 否 | episode 尝试次数。 |
| 3 | `env_type` | `string` | 否 | 环境类型。 |
| 4 | `payload` | `bytes` | 否 | 下发给 worker 的环境/业务 payload；不承载调度、训练协议或结果字段。 |
| 5 | `mode` | `ExecutionMode` | 否 | 执行模式。 |
| 6 | `max_steps` | `int32` | 否 | 最大步数。 |
| 7 | `resource_spec` | `ResourceSpec` | 否 | 资源需求。 |
| 8 | reserved | `reserved` | 否（历史保留） | 迁移期 `model_endpoint` 字符串字段号，禁止复用。 |
| 9 | `seed` | `optional int32` | 否 | 随机种子。 |
| 10 | `correlation_id` | `string` | 否 | 跨层追踪 ID。 |
| 11 | `timeout_seconds` | `int32` | 否 | 超时时间，单位秒。 |
| 12 | `reward_config` | `bytes` | 否 | 奖励配置 JSON。 |
| 13 | `dispatch_lease_id` | `string` | 否 | server 分发给 worker 的 lease ID。 |
| 14 | `lease_expire_at` | `google.protobuf.Timestamp` | 否 | lease 过期时间。 |
| 15 | `scheduler_epoch` | `uint64` | 否 | 分发时 server epoch。 |
| 16 | `dispatch_token` | `bytes` | 否 | 分发 token，用于 ReportResult 校验。 |
| 17 | `env_package_id` | `string` | 否 | EnvPackage ID。 |
| 18 | `env_package_version` | `string` | 否 | EnvPackage 版本。 |
| 19 | `parallel_mode` | `string` | 否 | VeRL 异步训练模式：`sync`、`one_step_off_policy`、`fully_async`。 |
| 20 | `enqueue_ts` | `optional double` | 否 | 进入 server 队列的 Unix 秒时间戳。 |
| 21 | `metadata` | `map<string,string>` | 否 | 上下文元数据；不承载调度、训练协议、rollout 结果或 timing 字段。 |
| 22 | `model_endpoint_config` | `ModelEndpoint` | 是 | 结构化模型端点配置；替代旧 `model_endpoint` string 和 payload 里的 model_name/generation_config。 |

### RolloutTrace

| 字段号 | 字段 | 类型 | 新增 | 含义 |
|---:|---|---|---|---|
| 1 | `response_ids` | `repeated int64` | 是 | 模型 response token ids。 |
| 2 | `response_mask` | `repeated int32` | 是 | response token mask。 |

### StepRecord

| 字段号 | 字段 | 类型 | 新增 | 含义 |
|---:|---|---|---|---|
| 1 | `step_index` | `int32` | 否 | step 下标。 |
| 2 | `observation` | `bytes` | 否 | step observation。 |
| 3 | `action` | `bytes` | 否 | step action。 |
| 4 | `reward` | `double` | 否 | step reward。 |
| 5 | `terminated` | `bool` | 否 | 是否自然终止。 |
| 6 | `truncated` | `bool` | 否 | 是否截断终止。 |
| 7 | `info` | `map<string,string>` | 否 | 环境自由信息；不承载 response_ids/response_mask 等协议字段。 |
| 8 | `duration_ms` | `int64` | 否 | step 耗时，单位毫秒。 |
| 9 | `rollout_trace` | `RolloutTrace` | 是 | 结构化 rollout trace。 |

### Trajectory

| 字段号 | 字段 | 类型 | 新增 | 含义 |
|---:|---|---|---|---|
| 1 | `steps` | `repeated StepRecord` | 否 | step 列表。 |
| 2 | `total_reward` | `double` | 否 | 总 reward。 |
| 3 | `total_steps` | `int32` | 否 | 总 step 数。 |

### EpisodeResult

| 字段号 | 字段 | 类型 | 新增 | 含义 |
|---:|---|---|---|---|
| 1 | `episode_id` | `string` | 否 | episode ID。 |
| 2 | `attempt_id` | `uint32` | 否 | attempt ID。 |
| 3 | `status` | `string` | 否 | 终态，例如 `completed`、`failed`、`timeout`、`cancelled`。 |
| 4 | `trajectory` | `Trajectory` | 否 | episode 轨迹。 |
| 5 | `summary` | `EpisodeResult.Summary` | 否 | 汇总信息。 |
| 6 | `error_code` | `optional ErrorCode` | 否 | 错误码。 |
| 7 | `error_message` | `string` | 否 | 错误信息。 |
| 8 | `trajectory_checksum` | `string` | 否 | 轨迹校验值。 |
| 9 | `integrity_verified` | `bool` | 否 | 轨迹完整性是否验证通过。 |
| 10 | `trajectory_id` | `string` | 否 | trajectory store 中的轨迹 ID。 |
| 11 | `trajectory_storage_url` | `string` | 否 | 轨迹存储位置。 |
| 12 | `gateway_session_id` | `string` | 否 | SWE+Agent 路径的 Gateway session ID。 |
| 13 | `parallel_mode` | `string` | 否 | 训练并行模式。 |
| 14 | `rollout_param_version` | `optional int64` | 否 | rollout 模型参数版本。 |
| 15 | `rollout_policy_version` | `optional string` | 否 | rollout 策略版本。 |
| 16 | `rollout_log_probs` | `repeated float` | 否 | token 级 rollout logprob。 |
| 17 | `metadata` | `map<string,string>` | 否 | 上下文元数据；不承载 parallel/rollout/timing 协议字段。 |
| 18 | `dispatch_ts` | `optional double` | 否 | server dispatch 的 Unix 秒时间戳。 |
| 19 | `worker_start_ts` | `optional double` | 否 | worker 开始执行的 Unix 秒时间戳。 |
| 20 | `worker_finish_ts` | `optional double` | 否 | worker 完成执行的 Unix 秒时间戳。 |
| 21 | `result_ready_ts` | `optional double` | 否 | server 结果可用的 Unix 秒时间戳。 |
| 22 | `server_latency_ms` | `optional int64` | 否 | server 侧耗时，单位毫秒。 |
| 23 | `worker_latency_ms` | `optional int64` | 否 | worker 侧耗时，单位毫秒。 |
| 24 | `model_latency_ms` | `optional int64` | 否 | 模型调用耗时，单位毫秒。 |

### EpisodeResult.Summary

| 字段号 | 字段 | 类型 | 新增 | 含义 |
|---:|---|---|---|---|
| 1 | `total_reward` | `double` | 否 | 总 reward。 |
| 2 | `total_steps` | `int32` | 否 | 总 step 数。 |
| 3 | `total_duration_ms` | `int64` | 否 | 总耗时，单位毫秒。 |
| 4 | `terminate_reason` | `string` | 否 | 终止原因。 |

### ReportType

流式报告类型枚举：

- `REPORT_TYPE_UNSPECIFIED = 0`：未指定。
- `PROGRESS = 1`：进度。
- `STEP_COMPLETE = 2`：step 完成。
- `REWARD_SIGNAL = 3`：奖励信号。
- `LOG = 4`：日志。
- `PACING = 5`：节奏/限速信息。

### StreamReport

| 字段号 | 字段 | 类型 | 新增 | 含义 |
|---:|---|---|---|---|
| 1 | `episode_id` | `string` | 否 | episode ID。 |
| 2 | `attempt_id` | `uint32` | 否 | attempt ID。 |
| 3 | `current_step` | `int32` | 否 | 当前 step。 |
| 4 | `total_steps` | `int32` | 否 | 总 step 数。 |
| 5 | `current_reward` | `double` | 否 | 当前累计或当前 step reward。 |
| 6 | `phase` | `string` | 否 | 兼容字段，例如 `running`、`step_complete`、`episode_complete`。 |
| 7 | `last_step` | `optional StepRecord` | 否 | 最近完成的 step。 |
| 8 | `report_type` | `ReportType` | 否 | 报告类型。 |
| 9 | `step_latency_ms` | `int64` | 否 | step 耗时，单位毫秒。 |
| 10 | `model_latency_ms` | `int64` | 否 | 模型调用耗时，单位毫秒。 |
| 11 | `estimated_remaining_seconds` | `double` | 否 | 预估剩余秒数。 |
| 12 | `worker_active_episodes` | `int32` | 否 | worker 当前 active episode 数。 |
| 13 | `worker_capacity` | `int32` | 否 | worker 容量。 |
| 14 | `correlation_id` | `string` | 否 | 跨层追踪 ID。 |
| 15 | `worker_id` | `string` | 否 | worker ID。 |

## proto/uenv/v1/scheduler.proto

package：`uenv.scheduler.v1`

### ControlPlaneService

- `RegisterWorker(RegisterWorkerRequest) returns (RegisterWorkerResponse)`：worker 注册。
- `WorkerHeartbeat(stream HeartbeatRequest) returns (stream HeartbeatResponse)`：worker 心跳双向流。
- `ReportResult(ReportResultRequest) returns (ReportResultResponse)`：worker 上报结果。
- `ListWorkers(ListWorkersRequest) returns (ListWorkersResponse)`：查询 worker 列表。

### SyncedEnvPackage

| 字段号 | 字段 | 类型 | 新增 | 含义 |
|---:|---|---|---|---|
| 1 | `package_id` | `string` | 否 | EnvPackage ID。 |
| 2 | `version` | `string` | 否 | EnvPackage 版本。 |
| 3 | `bundle_digest` | `string` | 否 | 包内容 digest。 |

### RegisterWorkerRequest

| 字段号 | 字段 | 类型 | 新增 | 含义 |
|---:|---|---|---|---|
| 1 | `worker_id` | `string` | 否 | worker ID。 |
| 2 | `supported_env_types` | `repeated string` | 否 | worker 支持的环境类型列表。 |
| 3 | `resource` | `uenv.v1.ResourceSpec` | 否 | worker 资源信息。 |
| 4 | `endpoint` | `string` | 否 | worker gRPC endpoint。 |
| 5 | `max_concurrent` | `uint32` | 否 | 历史容量配置值。 |
| 6 | `gateway_public_url` | `string` | 否 | Runtime Gateway 公网地址。 |
| 7 | `synced_env_packages` | `repeated SyncedEnvPackage` | 否 | worker 已同步的 EnvPackage 列表。 |
| 8 | `load` | `int32` | 是 | 注册时 worker 当前负载。 |
| 9 | `max_load` | `int32` | 是 | 注册时 worker 真实容量。 |

### RegisterWorkerResponse

| 字段号 | 字段 | 类型 | 新增 | 含义 |
|---:|---|---|---|---|
| 1 | `accepted` | `bool` | 否 | 注册是否被 server 接受。 |
| 2 | `worker_id` | `string` | 否 | server 确认的 worker ID。 |
| 3 | `message` | `string` | 否 | 注册结果说明。 |
| 4 | `server_epoch` | `uint64` | 否 | 当前 server epoch。 |

### HeartbeatRequest

| 字段号 | 字段 | 类型 | 新增 | 含义 |
|---:|---|---|---|---|
| 1 | `worker_id` | `string` | 否 | worker ID。 |
| 2 | `load` | `int32` | 否 | worker 当前负载。 |
| 3 | `max_load` | `int32` | 否 | worker 当前容量。 |
| 4 | `timestamp_ms` | `int64` | 否 | worker 发送心跳时的毫秒时间戳。 |
| 5 | `server_epoch` | `uint64` | 否 | worker 当前认知的 server epoch。 |

### HeartbeatResponse

| 字段号 | 字段 | 类型 | 新增 | 含义 |
|---:|---|---|---|---|
| 1 | `ok` | `bool` | 否 | 心跳是否被接受。 |
| 2 | `drain` | `DrainCommand` | 否 | drain 指令。 |
| 3 | `server_epoch` | `uint64` | 否 | 当前 server epoch。 |
| 4 | `next_heartbeat_interval_ms` | `int32` | 否 | 建议下次心跳间隔，单位毫秒。 |

### DrainCommand

| 字段号 | 字段 | 类型 | 新增 | 含义 |
|---:|---|---|---|---|
| 1 | `drain` | `bool` | 否 | 是否要求 worker 进入 draining。 |
| 2 | `grace_period_sec` | `int32` | 否 | drain 宽限期，单位秒。 |

### ReportResultRequest

| 字段号 | 字段 | 类型 | 新增 | 含义 |
|---:|---|---|---|---|
| 1 | `idempotency_key` | `string` | 否 | 上报幂等键。 |
| 2 | `worker_id` | `string` | 否 | 上报结果的 worker ID。 |
| 3 | `server_epoch` | `uint64` | 否 | worker 上报时认知的 server epoch。 |
| 4 | `result` | `uenv.v1.EpisodeResult` | 否 | episode 执行结果。 |
| 5 | `dispatch_lease_id` | `string` | 否 | dispatch lease ID。 |
| 6 | `dispatch_token` | `bytes` | 否 | dispatch token。 |

### ReportResultResponse

| 字段号 | 字段 | 类型 | 新增 | 含义 |
|---:|---|---|---|---|
| 1 | `ack` | `bool` | 否 | server 是否确认接收。 |
| 2 | `duplicate` | `bool` | 否 | 是否为重复上报。 |
| 3 | `code` | `string` | 否 | 上报结果码。 |
| 4 | `message` | `string` | 否 | 上报结果说明。 |

### ListWorkersRequest

| 字段号 | 字段 | 类型 | 新增 | 含义 |
|---:|---|---|---|---|
| 1 | `env_types` | `repeated string` | 否 | 按环境类型过滤 worker。 |

### WorkerInfo

| 字段号 | 字段 | 类型 | 新增 | 含义 |
|---:|---|---|---|---|
| 1 | `worker_id` | `string` | 否 | worker ID。 |
| 2 | `supported_env_types` | `repeated string` | 否 | 支持的环境类型。 |
| 3 | `load` | `int32` | 否 | 当前负载。 |
| 4 | `max_load` | `int32` | 否 | 当前容量。 |
| 5 | `status` | `string` | 否 | worker 状态。 |
| 6 | `endpoint` | `string` | 否 | worker endpoint。 |

### ListWorkersResponse

| 字段号 | 字段 | 类型 | 新增 | 含义 |
|---:|---|---|---|---|
| 1 | `workers` | `repeated WorkerInfo` | 否 | worker 列表。 |

## proto/uenv/v1/server.proto

package：`uenv.v1`

### AdminService

- `ListWorkers(ListWorkersRequest) returns (ListWorkersResponse)`：查询 worker。
- `DrainWorker(DrainWorkerRequest) returns (DrainWorkerResponse)`：drain worker。
- `CancelEpisode(CancelEpisodeRequest) returns (CancelEpisodeResponse)`：取消 episode。
- `GetServerStatus(GetServerStatusRequest) returns (ServerStatus)`：获取 server 状态。

### DrainWorkerRequest

| 字段号 | 字段 | 类型 | 新增 | 含义 |
|---:|---|---|---|---|
| 1 | `worker_id` | `string` | 否 | 要 drain 的 worker ID。 |
| 2 | `grace_period_sec` | `int32` | 否 | drain 宽限期，单位秒。 |

### DrainWorkerResponse

| 字段号 | 字段 | 类型 | 新增 | 含义 |
|---:|---|---|---|---|
| 1 | `accepted` | `bool` | 否 | drain 请求是否被接受。 |

### CancelEpisodeRequest

| 字段号 | 字段 | 类型 | 新增 | 含义 |
|---:|---|---|---|---|
| 1 | `episode_id` | `string` | 否 | 要取消的 episode ID。 |
| 2 | `attempt_id` | `uint32` | 否 | 要取消的 attempt ID；`0` 表示当前 active attempt。 |

### CancelEpisodeResponse

| 字段号 | 字段 | 类型 | 新增 | 含义 |
|---:|---|---|---|---|
| 1 | reserved | `reserved` | 否（历史保留） | 历史字段号，禁止复用。 |
| 2 | `server_cancelled` | `bool` | 是 | server 侧是否已写入取消终态。 |
| 3 | `worker_cancel_attempted` | `bool` | 是 | server 是否尝试通知原生 worker 停止物理执行。 |
| 4 | `worker_cancel_accepted` | `bool` | 是 | worker 是否确认接受物理取消请求。 |
| 5 | `worker_cancel_code` | `string` | 是 | worker 取消结果码，例如 `ACCEPTED`、`RPC_FAILED`、`NOT_DISPATCHED`。 |
| 6 | `worker_cancel_message` | `string` | 是 | worker 取消结果详情。 |

### GetServerStatusRequest

无字段。

### ServerStatus

| 字段号 | 字段 | 类型 | 新增 | 含义 |
|---:|---|---|---|---|
| 1 | `server_epoch` | `uint64` | 否 | 当前 server epoch。 |
| 2 | `worker_count` | `int32` | 否 | worker 数量。 |
| 3 | `active_episode_count` | `int32` | 否 | active episode 数量。 |
| 4 | `pending_episode_count` | `int32` | 否 | pending result 数量。 |

## proto/uenv/v1/agent.proto

package：`uenv.v1`

### AgentJob

| 字段号 | 字段 | 类型 | 新增 | 含义 |
|---:|---|---|---|---|
| 1 | `job_id` | `string` | 否 | AgentJob ID。 |
| 2 | `run_id` | `string` | 否 | 运行 ID。 |
| 3 | `gateway_url` | `string` | 否 | Runtime Gateway URL。 |
| 4 | `gateway_api_key` | `string` | 否 | Gateway API key 或引用。 |
| 5 | `session_id` | `string` | 否 | Gateway session ID。 |
| 6 | `instance_id` | `string` | 否 | SWE/benchmark 实例 ID。 |
| 7 | `benchmark_variant` | `string` | 否 | benchmark variant，例如 default/pro。 |
| 8 | `env_package_id` | `string` | 否 | EnvPackage ID。 |
| 9 | `env_package_version` | `string` | 否 | EnvPackage 版本。 |
| 10 | `agent_bridge_id` | `string` | 否 | Agent bridge 包 ID。 |
| 11 | `agent_bridge_version` | `string` | 否 | Agent bridge 版本。 |
| 12 | `driver_entrypoint` | `string` | 否 | agent driver 入口。 |
| 13 | `model_endpoint` | `string` | 否 | 模型端点。 |
| 14 | `max_iterations` | `int32` | 否 | agent 最大迭代次数。 |
| 15 | `workspace_dir` | `string` | 否 | workspace 路径。 |
| 16 | `episode_id` | `string` | 否 | 关联 episode ID。 |
| 17 | `llm_config_path` | `string` | 否 | LLM 配置路径。 |
| 18 | `mode` | `string` | 否 | agent 模式，例如 `llm`、`gold`。 |
| 19 | `parallel_mode` | `string` | 否 | 训练并行模式。 |
| 20 | `enqueue_ts` | `optional double` | 否 | 入队 Unix 秒时间戳。 |
| 21 | `metadata` | `map<string,string>` | 否 | 上下文元数据。 |

### SyncedAgentBridge

| 字段号 | 字段 | 类型 | 新增 | 含义 |
|---:|---|---|---|---|
| 1 | `package_id` | `string` | 否 | Agent bridge 包 ID。 |
| 2 | `version` | `string` | 否 | Agent bridge 版本。 |
| 3 | `bundle_digest` | `string` | 否 | 包内容 digest。 |

### RegisterAgentRequest

| 字段号 | 字段 | 类型 | 新增 | 含义 |
|---:|---|---|---|---|
| 1 | `agent_id` | `string` | 否 | Agent ID。 |
| 2 | `agent_pool_id` | `string` | 否 | Agent 池 ID。 |
| 3 | `synced_agent_bridges` | `repeated SyncedAgentBridge` | 否 | 已同步的 Agent bridge 列表。 |
| 4 | `max_concurrent_jobs` | `uint32` | 否 | 最大并发 AgentJob 数。 |
| 5 | `endpoint` | `string` | 否 | Agent endpoint；poll 模式可为空。 |
| 6 | `labels` | `map<string,string>` | 否 | Agent 路由标签，例如 region、gpu。 |

### RegisterAgentResponse

| 字段号 | 字段 | 类型 | 新增 | 含义 |
|---:|---|---|---|---|
| 1 | `accepted` | `bool` | 否 | 注册是否被接受。 |
| 2 | `agent_id` | `string` | 否 | server 确认的 Agent ID。 |
| 3 | `message` | `string` | 否 | 注册结果说明。 |

### AgentHeartbeatRequest

| 字段号 | 字段 | 类型 | 新增 | 含义 |
|---:|---|---|---|---|
| 1 | `agent_id` | `string` | 否 | Agent ID。 |
| 2 | `active_jobs` | `uint32` | 否 | 当前 active job 数。 |
| 3 | `timestamp_ms` | `int64` | 否 | Agent 发送心跳时的毫秒时间戳。 |

### AgentHeartbeatResponse

| 字段号 | 字段 | 类型 | 新增 | 含义 |
|---:|---|---|---|---|
| 1 | `ok` | `bool` | 否 | 心跳是否被接受。 |
| 2 | `next_heartbeat_interval_ms` | `int32` | 否 | 建议下次心跳间隔，单位毫秒。 |

### PollAgentJobRequest

| 字段号 | 字段 | 类型 | 新增 | 含义 |
|---:|---|---|---|---|
| 1 | `agent_pool_id` | `string` | 否 | Agent 池 ID。 |
| 2 | `worker_id` | `string` | 否 | 承载该 agent 的 worker ID。 |

### PollAgentJobResponse

| 字段号 | 字段 | 类型 | 新增 | 含义 |
|---:|---|---|---|---|
| 1 | `has_job` | `bool` | 否 | 是否取到 job。 |
| 2 | `job` | `AgentJob` | 否 | 取到的 AgentJob。 |

### AgentJobCompleteRequest

| 字段号 | 字段 | 类型 | 新增 | 含义 |
|---:|---|---|---|---|
| 1 | `job_id` | `string` | 否 | AgentJob ID。 |
| 2 | `run_id` | `string` | 否 | 运行 ID。 |
| 3 | `status` | `string` | 否 | 终态，例如 `completed`、`failed`、`timeout`。 |
| 4 | `reward` | `double` | 否 | job reward。 |
| 5 | `trajectory_id` | `string` | 否 | 轨迹 ID。 |
| 6 | `error_message` | `string` | 否 | 错误信息。 |
| 7 | `agent_id` | `string` | 否 | 完成该 job 的 Agent ID。 |
| 8 | `parallel_mode` | `string` | 否 | 训练并行模式。 |
| 9 | `rollout_param_version` | `optional int64` | 否 | rollout 模型参数版本。 |
| 10 | `rollout_policy_version` | `optional string` | 否 | rollout 策略版本。 |
| 11 | `rollout_log_probs` | `repeated float` | 否 | token 级 rollout logprob。 |
| 12 | `worker_start_ts` | `optional double` | 否 | worker 开始执行 Unix 秒时间戳。 |
| 13 | `worker_finish_ts` | `optional double` | 否 | worker 完成执行 Unix 秒时间戳。 |
| 14 | `result_ready_ts` | `optional double` | 否 | 结果可用 Unix 秒时间戳。 |
| 15 | `worker_latency_ms` | `optional int64` | 否 | worker 耗时，单位毫秒。 |
| 16 | `model_latency_ms` | `optional int64` | 否 | 模型耗时，单位毫秒。 |
| 17 | `metadata` | `map<string,string>` | 否 | 上下文元数据。 |
| 18 | `rollout_trace` | `RolloutTrace` | 是 | 结构化 rollout trace，用于传递 response_ids/response_mask。 |

### AgentJobCompleteResponse

| 字段号 | 字段 | 类型 | 新增 | 含义 |
|---:|---|---|---|---|
| 1 | `ack` | `bool` | 否 | server 是否确认接收。 |
| 2 | `code` | `string` | 否 | 完成结果码。 |
| 3 | `message` | `string` | 否 | 完成结果说明。 |

### AgentControlService

- `RegisterAgent(RegisterAgentRequest) returns (RegisterAgentResponse)`：注册 Agent。
- `AgentHeartbeat(AgentHeartbeatRequest) returns (AgentHeartbeatResponse)`：Agent 心跳。
- `PollAgentJob(PollAgentJobRequest) returns (PollAgentJobResponse)`：Agent 拉取任务。
- `CompleteAgentJob(AgentJobCompleteRequest) returns (AgentJobCompleteResponse)`：Agent 上报任务完成。

## proto/uenv/v1/wal.proto

package：`uenv.v1`

### ReplayState

WAL replay 状态枚举：

- `REPLAY_STATE_UNSPECIFIED = 0`：未指定。
- `REPLAY_STATE_PENDING = 1`：待 replay。
- `REPLAY_STATE_SENT = 2`：已发送。
- `REPLAY_STATE_ACKED = 3`：已确认。

### WalRecord

| 字段号 | 字段 | 类型 | 新增 | 含义 |
|---:|---|---|---|---|
| 1 | `episode_id` | `string` | 否 | episode ID。 |
| 2 | `attempt_id` | `uint32` | 否 | attempt ID。 |
| 3 | `worker_id` | `string` | 否 | worker ID。 |
| 4 | `dispatch_lease_id` | `string` | 否 | dispatch lease ID。 |
| 5 | `server_epoch` | `uint64` | 否 | server epoch。 |
| 6 | `request_checksum` | `string` | 否 | 请求校验值。 |
| 7 | `result_checksum` | `string` | 否 | 结果校验值。 |
| 8 | `status` | `string` | 否 | WAL 记录状态。 |
| 9 | `protobuf_payload` | `bytes` | 否 | 序列化后的 protobuf payload。 |
| 10 | `created_at` | `google.protobuf.Timestamp` | 否 | WAL 记录创建时间。 |
| 11 | `replay_state` | `ReplayState` | 否 | replay 状态。 |
| 12 | `dispatch_token` | `bytes` | 否 | dispatch token。 |
