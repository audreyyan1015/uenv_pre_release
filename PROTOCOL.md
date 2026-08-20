# UEnv 协议总览

本文面向源码维护者，说明当前协议边界和权威定义。部署与使用文档统一采用下面的对外组件关系：

```text
评测程序 / 强化学习框架 -> UEnv Bridge -> UEnv Server -> UEnv Worker
```

- Bridge 在框架一侧完成 sample 与 UEnv 请求/结果的转换。
- Server 是唯一的中心服务。当前兼容二进制名为 `uenv-adapter-core`。
- `uenv-server` 是 Server 使用的 Rust library crate，不是另一个运行进程。
- `ControlPlaneService` 是 Server 对 Worker 提供的一组兼容 RPC 名称，不是另一个组件。
- Hub 是可选的环境版本和 EnvPackage 服务，不在 Episode 热路径。

用户侧的完整解释见 [协议参考](./Docs/guide/reference/protocols.md) 和 [组件与边界](./Docs/guide/concepts/architecture.md)。

## 权威协议文件

| 路径 | 内容 |
|---|---|
| `proto/uenv/v1/adapter_core.proto` | Bridge 调用 Server 的批次与流式接口 |
| `proto/uenv/v1/scheduler.proto` | Worker 注册、心跳、结果上报与 Worker 列表 |
| `proto/uenv/v1/server.proto` | Episode、Admin 与 Agent 管理接口 |
| `proto/uenv/v1/episode.proto` | Episode 请求、结果、轨迹与进度类型 |
| `proto/uenv/v1/wal.proto` | Worker 结果 WAL 类型 |
| `uenv-worker/proto/worker_service.proto` | Server 反向调用 Worker 的派发接口 |
| `plugin_proto/uenv/plugin/v1/plugin.proto` | Worker 与进程插件之间的 L2 接口 |

实现与本文不一致时，以 proto 和当前实现为准，并同步修正文档。

## Server 进程中的服务

`uenv-adapter-core` 在同一个 gRPC listener 上装载：

- `AdapterCoreService`
- `ControlPlaneService`
- `AgentControlService`
- `AdminService`

Server 的兼容进程入口位于 `uenv-bridge/core/`，注册、调度、租约、持久化和结果管理实现位于 `uenv-server/`。目录布局不改变其运行时边界。

## 调用方向

| 发起方 | 目标 | RPC / 接口 | 作用 |
|---|---|---|---|
| Bridge | Server | `ExecuteBatch`、`ExecuteBatchStream` | 提交 sample，接收 reward、trajectory 和状态 |
| Worker | Server | `RegisterWorker` | 上报身份、回连 endpoint、容量、资源和环境能力 |
| Worker | Server | `WorkerHeartbeat` | 刷新负载、容量、能力并发现 Server epoch 变化 |
| Worker | Server | `ReportResult` | 上报带 lease、token 和幂等键的最终结果 |
| Server | Worker | `DispatchEpisode` | 主动派发 Episode；stream 只承载执行进度 |
| Server | Worker | `CancelEpisode`、`PrepareEnvironment`、`HealthCheck` | 取消、准备环境和探活 |
| Worker | 环境插件 | L2 IPC / UDS | `reset`、`step`、`close`、`health_check` |
| Worker | 模型服务 | HTTP / gRPC | 调用推理服务，不经 Server 转发 |

Server 与 Worker 之间必须双向可达。默认发布端口为 Server `50051/TCP`、Worker `50054/TCP`。

## Worker 注册与心跳

`RegisterWorkerRequest` 包含：

- `worker_id` 与 Server 可回连的 `endpoint`
- `supported_env_types`、资源与并发容量
- 当前 `load` / `max_load`
- EnvPackage、backend、platform feature、trajectory/tool schema
- 实例池与 package 状态

`worker_id` 为空或为 `auto` 时由 Server 分配。响应返回是否接受、最终 Worker ID 和当前 `server_epoch`。

协议把 `WorkerHeartbeat` 定义为双向 stream；当前 Worker 每个周期重新建立一次只发送一条请求的 stream。Server 默认 30 秒未收到心跳时停止向该 Worker 分配新任务，但不会因超时自动删除注册记录。

心跳响应中的 `drain` 是预留字段，当前 Server 不下发，Worker 也不读取。实际排空由 `AdminService.DrainWorker` 修改 Server 注册表状态。

## 派发、租约与结果

Server 选择满足环境、资源、EnvPackage、状态和容量要求的 Worker。每次派发包含：

- `episode_id` 与 `attempt_id`
- `dispatch_lease_id`
- `dispatch_token`
- `scheduler_epoch`
- `lease_expire_at`

`DispatchEpisode` 的服务端 stream 只传送 `StreamReport` 进度。Worker 完成后先把最终结果写入本地 WAL，再通过独立的 `ReportResult` RPC 上报 `EpisodeResult`。

Server 校验 Worker、epoch、lease、token 和幂等键。确定的重复终态会让 Worker 清理 WAL；暂时性错误保留记录并退避重放。

`max_attempts=3` 表示最多三次总 attempt（首次加最多两次重试），不是首次执行后再重试三次。

## Epoch 与重启

Server 每次启动生成新的 `server_epoch`。Worker 在心跳响应中发现 epoch 变化，或收到 `ok=false`，会自动重新注册。下发给 Worker 的相同标识在 Episode 请求中名为 `scheduler_epoch`。

同一 endpoint 被新 Worker ID 注册替换时，旧 Worker 的在途 lease 以 `ERR_LEASE_SUPERSEDED` 收口，避免迟到结果无限重试。

## 稳定性约束

- Episode 热路径不依赖 Redis/Kafka 等消息中间件。
- Server 主动调用 Worker，不使用 Worker 拉取任务的订阅模型。
- Bridge 用 `request_id` 对齐乱序结果，不能按数组位置猜测。
- 传输重试保持原逻辑请求 ID；新的 rollout 使用新的 ID。
- 密钥不进入 sample payload、trajectory 或普通日志。

更详细的字段、状态与运维含义见：

- [Bridge 接入契约](./Docs/guide/integration/contract.md)
- [Worker 接入与注册](./Docs/guide/deployment/worker-registration.md)
- [Episode 生命周期](./Docs/guide/concepts/episode-lifecycle.md)
