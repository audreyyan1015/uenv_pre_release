# uenv-server — UEnv 全栈调度服务

UEnv Server 是 UEnv **全栈方案** 的控制平面：接收训练框架（或 Mock 客户端）提交的 Episode，维护 Worker 注册表与调度决策，**主动**调用 Worker `DispatchEpisode`。

> Layer 2 Worker Pool 权威文档：[Docs/worker-pool-layer-design.md](../Docs/worker-pool-layer-design.md)  
> 协议规范：[PROTOCOL.md](../PROTOCOL.md)

## 架构

```
Mock 客户端 / uenv-bridge  --[UEnvService]-------->  uenv-server
Worker                     --[ControlPlaneService]->  uenv-server
uenv-server                --[WorkerGrpcService]-->  Worker
运维工具                    --[AdminService]-------->  uenv-server
```

## gRPC Service（统一 proto）

| Service | Proto | 说明 |
|---------|-------|------|
| `UEnvService` | `proto/uenv/v1/server.proto` | `SubmitEpisode` 等 |
| `ControlPlaneService` | `proto/uenv/v1/scheduler.proto` | Worker 注册、心跳、`ReportResult` |
| `AdminService` | `proto/uenv/v1/server.proto` | 运维查询 |

Server 作为 **客户端** 调用 Worker 的 `uenv.worker.v1.WorkerGrpcService`（见 `uenv-worker/proto/worker_service.proto`）。

## 构建与运行

```bash
cargo build -p uenv-server
./target/debug/uenv-server -b 0.0.0.0:50051
```

Proto 在 `build.rs` 中从 `proto/` 编译，无需单独 `make proto-server`（但 Worker 等 crate 仍需 `make proto`）。

## Worker 接入

Worker 启动后连接同一端口的 `ControlPlaneService`：

| 字段 | 说明 |
|------|------|
| `worker_id` | 唯一标识 |
| `endpoint` | Worker gRPC 地址（Server 回连用） |
| `supported_env_types` | 如 `["gsm8k"]` |
| `max_concurrent` | 最大并发 |

`SubmitEpisode` 流程：调度 Worker → 填充 `dispatch_lease_id` → `DispatchEpisode` → 等待 `ReportResult` → 返回客户端。

## 实机联调

见 [Docs/discussions/a100-server-worker-e2e/README.md](../Docs/discussions/a100-server-worker-e2e/README.md)。
