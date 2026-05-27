# uenv-server — UEnv 全栈调度服务

UEnv Server 是 UEnv **全栈方案** 的控制平面：接收训练框架提交的 Episode，维护 Worker 注册表与调度决策。

> **Worker Pool MVP（M1–M6）不依赖本 crate 完整实现。** MVP 联调请使用 [`uenv-mock-scheduler`](../uenv-mock-scheduler/)。M7 起与真实 Server 集成。

Layer 2 Worker Pool 权威文档：[Docs/worker-pool-layer-design.md](../Docs/worker-pool-layer-design.md)

## 架构

```
训练框架 (Python)  --[UEnvService]---------->  uenv-server  --[WorkerExecution]-->  Worker
运维工具           --[AdminService]-------->  uenv-server
Worker             --[WorkerRegistration]-->  uenv-server
```

- **UEnvService**：训练框架调用的主接口，提交 episode、获取结果
- **WorkerRegistration**：Worker 启动时向 Server 注册自己
- **WorkerExecution**：Server 主动调用 Worker 分发任务（Server 是客户端）
- **AdminService**：查询 Server 状态、管理 Worker

## 与 Worker Pool 职责边界

| 能力 | uenv-server（全栈） | uenv-worker（Worker Pool） |
|------|---------------------|----------------------------|
| Bridge → Server Episode 提交 | ✅ `UEnvService` | — |
| Scheduler 主动 `DispatchEpisode` | M7+ | ✅ Worker gRPC Server |
| Worker 注册 / 心跳 / 结果上报 | M7+ ControlPlane | ✅ ControlPlane Client |
| 环境实例 Backend / 预热池 | — | ✅ `backend/`、`pool/` |
| Worker WAL | — | ✅ `wal/` |
| Mock 联调 | — | ✅ `uenv-mock-scheduler` |

## 目录结构

```
uenv-server/
├── proto/server.proto       # gRPC 接口定义（所有消息类型和 4 个 service）
├── src/
│   ├── main.rs              # 启动入口，注册三个 gRPC service
│   ├── proto.rs             # tonic::include_proto! 引入生成代码
│   ├── service.rs           # UEnvService / AdminService / WorkerRegistration 实现
│   ├── state.rs             # ServerState（调度器 + 活跃 episode 表）
│   └── scheduler/
│       ├── traits.rs        # Scheduler trait 和相关数据类型
│       └── mod.rs           # RoundRobinScheduler 实现
├── build.rs                 # 调用 tonic-build 从 server.proto 生成 Rust 代码
└── Cargo.toml
```

## 构建与运行

```bash
cargo build
./target/debug/uenv-server                   # 默认监听 [::]:50051
./target/debug/uenv-server -b 0.0.0.0:8080    # 指定绑定地址
```

## Worker 接入

Worker 启动后需调用 `WorkerRegistration.RegisterWorker` 注册自己：

| 字段 | 说明 |
|------|------|
| `worker_id` | 唯一标识，如 UUID |
| `endpoint` | Worker 的 gRPC 地址（`host:port`），Server 回调用此地址 |
| `supported_env_types` | 支持的环境类型，如 `["math", "gsm8k"]` |
| `capacity` | 最大并发 episode 数 |

注册成功后，Server 开始向该 Worker 分发匹配 `env_type` 的 episode。

Worker 需实现 `WorkerExecution.DispatchEpisode`：接收 `DispatchRequest`，通过 stream 上报进度，最后发一条 `report_type=EPISODE_RESULT` 的消息，其 `payload` 为 prost 序列化的 `EpisodeResult`。

## 调度策略

Round-Robin：从支持对应 `env_type` 且 `current_load < capacity` 的 Worker 中轮询选取。若所有候选 Worker 均满载，每 500ms 重试一次直到超时（默认 300s）。
