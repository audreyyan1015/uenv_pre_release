# 协议与调用方向

本页面向框架接入、UEnv Server 和 UEnv Worker 开发者，以及需要定位分布式状态问题的运维人员。

## 服务边界

UEnv 主链使用 gRPC 和 Protobuf；UEnv Hub 使用 HTTP API；UEnv Worker 与 process plugin 使用本机 Unix domain socket 协议。

| 代码服务名 | 调用方 → 实现方 | 主要 RPC | 权威定义 |
|---|---|---|---|
| `AdapterCoreService` | 框架接入代码 → UEnv Server | `ExecuteBatch`、`ExecuteBatchStream`、`HealthCheck` | `proto/uenv/v1/adapter_core.proto` |
| `ControlPlaneService` | UEnv Worker → UEnv Server | `RegisterWorker`、`WorkerHeartbeat`、`ReportResult`、`ListWorkers` | `proto/uenv/v1/scheduler.proto` |
| `WorkerGrpcService` | UEnv Server → UEnv Worker | `DispatchEpisode`、`HealthCheck`、`CancelEpisode`、`PrepareEnvironment` | `uenv-worker/proto/worker_service.proto` |
| `AdminService` | 运维客户端 → UEnv Server | `ListWorkers`、`DrainWorker`、`CancelEpisode`、`GetServerStatus` | `proto/uenv/v1/server.proto` |
| plugin L2 | UEnv Worker → process plugin | reset、step、close、health | `plugin_proto/uenv/plugin/v1/plugin.proto` |

`AdapterCoreService` 和 `uenv-adapter-core` 是当前兼容代码标识；它们都属于公开所称的 UEnv Server。`ControlPlaneService` 聚合了 UEnv Server 的内部控制能力 RPC。上述 UEnv Server 侧 gRPC 服务由同一个进程在 `50051` 提供。

## 接入请求和结果

框架接入代码把框架样本整理为 `SampleEnvelope`。关键字段包括：

- 唯一请求标识、批次标识和样本位置
- 框架名称、目标 `env_type` 和并行模式
- 环境、Episode 与奖励配置
- 类型化模型 endpoint
- 超时、关联标识和样本上下文
- 可选 EnvPackage ID 与版本

UEnv Server 返回 `SampleResult`，包含原始样本标识、终态、奖励、轨迹、终止原因和错误信息；异步训练还可以携带 rollout 版本与 token 级 log probability。

接入代码必须按请求标识匹配结果，不能假设返回顺序与输入顺序相同。字段类型、required/optional 语义和枚举值以 `proto/uenv/v1/adapter_core.proto` 为准。

## UEnv Worker 注册与心跳

`RegisterWorker` 上报 UEnv Worker ID、回连 endpoint、支持的环境类型、资源、容量、环境包和运行能力。UEnv Worker ID 为空或为 `auto` 时，UEnv Server 分配 UUID；响应同时返回当前 `server_epoch`。

当前默认注册行为：

| 设置 | 默认值 | 含义 |
|---|---:|---|
| `UENV_WORKER_REGISTER_TIMEOUT_SECS` | 10 秒 | 单次注册总超时 |
| `UENV_WORKER_REGISTER_MAX_ATTEMPTS` | 5 | 启动时最多尝试次数 |
| `UENV_WORKER_REGISTER_RETRY_BACKOFF_MS` | 200 ms | 指数退避基数，带随机抖动 |

`WorkerHeartbeat` 刷新负载、容量、环境和实例池能力。协议类型是双向流，但当前 UEnv Worker 每个周期建立一次只发送一条请求的流，不应把它描述为一条永久心跳连接。

UEnv Server 默认约 30 秒未收到心跳时暂停向该 UEnv Worker 分配新 Episode，观测接口默认约 90 秒后显示离线。心跳响应中的 UEnv Server 运行实例标识变化，或响应表明注册已不存在时，UEnv Worker 自动重新注册。

心跳中的 drain 字段是预留字段；当前 UEnv Server 不通过该字段通知 UEnv Worker。排空状态由 `AdminService.DrainWorker` 修改 UEnv Server 注册表，当前发布 CLI 没有独立 drain 子命令。

用户侧安装、地址和验收步骤见[配置并注册 UEnv Worker](../2-部署UEnv/04-worker-registration.md)。

## 派发与最终结果

UEnv Server 调用 `DispatchEpisode` 时附带本次派发的临时所有权和校验信息。UEnv Worker 可以在该 RPC 流中返回进度 `StreamReport`，但最终权威 `EpisodeResult` 通过独立的 `ReportResult` 反向上报。

`ReportResult` 使用以下身份字段拒绝迟到、冲突或非持有者结果：

- `worker_id`
- `server_epoch`
- `dispatch_lease_id`
- `dispatch_token`
- `idempotency_key`

UEnv Worker 在 UEnv Server 确认前通过 WAL 保留最终结果。UEnv Server 用幂等键和结果 checksum 区分合法重放与冲突重用。

| 响应或错误 | 含义和处理 |
|---|---|
| `ack=true` | UEnv Server 已接受结果，UEnv Worker 可以清理对应 WAL |
| `duplicate=true` | 已有确定终态，UEnv Worker 停止重试并清理 WAL |
| `ack=false, duplicate=false` | 尚未确认，UEnv Worker 保留 WAL 并继续重试 |
| `STALE_EPOCH` | 结果来自不再有效的 UEnv Server 运行实例 |
| `WORKER_MISMATCH` / `TOKEN_MISMATCH` | 结果身份与当前派发不一致 |
| `IDEMPOTENCY_CONFLICT` | 相同幂等键被用于不同结果 |
| `ERR_LEASE_SUPERSEDED` | 同 endpoint 的新 UEnv Worker 已替换旧派发所有者 |

## 权威来源和兼容边界

- 消息结构、字段和枚举以仓库当前 `proto/`、`uenv-worker/proto/` 与 `plugin_proto/` 为准。
- 用户侧文档只承诺 框架 → UEnv Server → UEnv Worker 的公开关系，不承诺内部 Rust library API 稳定。
- UEnv Worker 不拉取 Episode；派发方向固定为 UEnv Server 主动调用 UEnv Worker。
- 旧设计记录只用于了解背景，不能覆盖当前 proto、发布配置和实现行为。
