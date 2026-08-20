# uenv-server — UEnv Server 调度库

本页只面向维护 UEnv Server 源码的开发者。部署和使用 UEnv 时，请从 [UEnv 使用手册](../Docs/guide/1-了解UEnv/01-index.md) 开始。

`uenv-server` 是 UEnv Server 使用的内部 Rust library crate，负责 Worker 注册、心跳、调度、状态持久化和结果管理。当前发布包把它链接进 `uenv-adapter-core` 可执行程序，因此不能单独启动本 crate。

对外主链始终是：

```text
评测程序 / 强化学习框架 → UEnv Bridge → UEnv Server → UEnv Worker
```

当前 Server 入口位于 `uenv-bridge/core/`，构建出的二进制是 `uenv-adapter-core`，发布服务是 `uenv-adapter-core.service`。这些是现有代码名称。

## 模块边界

UEnv Server 进程同时提供：

| 服务/接口 | 调用方 | 本 crate 的职责 |
|---|---|---|
| `AdapterCoreService` | Bridge | Server 将 sample 转换为 Episode 后调用本 crate 的 `EpisodeService` |
| `ControlPlaneService` | Worker | 注册、心跳、最终结果上报 |
| `WorkerGrpcService` client | Server 内部 | 主动向 Worker 派发、取消和准备环境 |
| `AdminService` / Admin HTTP | 运维工具 | Worker、Episode 和 Server 状态 |
| Obs / trajectory HTTP | 可观测与轨迹客户端 | 状态聚合、事件流和 trajectory 存取 |

`ControlPlaneService` 只是 Worker 调用的一组 RPC 名称。

## Worker 调用方向

| 方向 | RPC |
|---|---|
| Worker → Server | `RegisterWorker`、`WorkerHeartbeat`、`ReportResult` |
| Server → Worker | `DispatchEpisode`、`CancelEpisode`、`PrepareEnvironment`、`HealthCheck` |

Worker 注册时上报它的公开 endpoint。Server 通过该地址反向调用 Worker，因此多机环境必须同时允许 Worker → Server 和 Server → Worker。

注册和重启语义见 [配置并注册 UEnv Worker](../Docs/guide/2-部署UEnv/04-worker-registration.md)。

## 作为 library 使用

核心构造入口包括：

- `ServerConfig`
- `create_persistent_state_with_config`
- `UEnvEpisodeService`
- `ControlPlaneServiceImpl`
- `AdminServiceImpl`

生产启动由 `uenv-adapter-core` 完成。不要使用不存在的独立 `uenv-server` 二进制启动命令。

## 配置

发布模板：

```text
deploy/config/server.yaml
deploy/config/server.env.example
deploy/systemd/uenv-adapter-core.service
```

安装后：

```text
/etc/uenv/server.yaml
/etc/uenv/server.env
/var/lib/uenv/server/server-state.db
```

Server gRPC 实际 bind 由 `UENV_ADDR` 控制，发布默认 `0.0.0.0:50051`。`server.yaml` 负责调度、Episode、Admin HTTP 与持久化设置。完整参考见 [UEnv Server 与 UEnv Worker 配置参考](../Docs/guide/6-查阅参考/02-configuration.md)。

## 默认接口

| 地址 | 能力 |
|---|---|
| `0.0.0.0:50051` | Server 与控制类 gRPC |
| `127.0.0.1:50052` | Admin HTTP |
| `0.0.0.0:50053` | Obs HTTP/SSE |
| `0.0.0.0:8077` | trajectory HTTP |

## 构建与测试

在 monorepo 根目录运行：

```bash
cargo check -p uenv-server
cargo test -p uenv-server
cargo check -p uenv-adapter-core
cargo test -p uenv-adapter-core
```

源码运行 Server 时：

```bash
UENV_ADDR=0.0.0.0:50051 \
UENV_CONFIG_PATH="$PWD/config/server.yaml" \
cargo run -p uenv-adapter-core
```

生产安装使用发布包与 `uenv-adapter-core.service`，见 [配置 UEnv Server](../Docs/guide/2-部署UEnv/03-server.md)。

## 协议权威来源

- Worker 控制协议：`proto/uenv/v1/scheduler.proto`
- Admin 协议：`proto/uenv/v1/server.proto`
- Worker 服务：`uenv-worker/proto/worker_service.proto`
- Bridge/Server API：`proto/uenv/v1/adapter_core.proto`

若旧设计文档与当前 proto/实现冲突，以当前 proto 和代码为准。
