# gRPC 消息上限链路核验与 Server 补齐说明

> 记录日期：2026-08-08
> 适用问题：SWE / SWE-smith GRPO 训练中，`EpisodeResult` 包含较大的 trajectory、`response_ids`、`rollout_log_probs` 等字段时，gRPC 响应体超过默认 4 MiB，触发 `RESOURCE_EXHAUSTED: Received message larger than max`。

## 1. 问题背景

近期 SWE-smith GRPO 训练中出现如下错误：

```text
grpc._channel._InactiveRpcError
status = StatusCode.RESOURCE_EXHAUSTED
details = "CLIENT: Received message larger than max (4594969 vs. 4194304)"
```

其中 `4194304` 字节约等于 4 MiB，是 gRPC 默认消息大小限制；`4594969` 字节约等于 4.38 MiB，说明本轮返回体已超过默认上限。

需要注意：gRPC 消息大小限制不是全局开关，而是每一条 gRPC 链路两端各自维护的收发限制。只修改 Worker server 端并不能覆盖 Python Adapter client、Rust AdapterCore server、Server 到 Worker 的 client 等其他位置。

## 2. 链路与配置点

当前 UEnv 训练链路中，相关 gRPC 位置如下：

| 链路 | 角色 | 需要设置的位置 | 当前处理 |
|---|---|---|---|
| Python Adapter -> Rust AdapterCore | Python client | `grpc.insecure_channel(..., options=...)` | 已补 16 MiB |
| Rust AdapterCore service | Rust server | `AdapterCoreServiceServer` | 已补 16 MiB |
| Rust Server / AdapterCore -> Worker | Rust client | `WorkerGrpcServiceClient` | 已补 16 MiB |
| Worker runtime gRPC service | Rust server | `WorkerGrpcServiceServer` | Worker 侧此前已补 16 MiB |

因此，本次 server 侧修改不是重复修改 Worker，而是补齐 `Server / AdapterCore -> Worker` 这条 gRPC client 连接的消息上限。

## 3. Worker 侧已提升到 16 MiB 的证据

### 3.1 Worker runtime 代码

文件：`/data/ronghao/uenv/uenv-worker/src/runtime.rs`

关键代码：

```rust
let max_message_bytes =
    env_usize_default("UENV_WORKER_GRPC_MAX_MESSAGE_BYTES", 16 * 1024 * 1024);

let grpc_service = WorkerGrpcServiceServer::new(service)
    .max_decoding_message_size(max_message_bytes)
    .max_encoding_message_size(max_message_bytes);
```

含义：

- Worker gRPC server 默认消息上限为 `16 * 1024 * 1024 = 16777216` 字节。
- 可通过 `UENV_WORKER_GRPC_MAX_MESSAGE_BYTES` 覆盖。
- 同时设置了 decoding 与 encoding，分别覆盖接收和发送方向。

### 3.2 Worker 侧 git 证据

`git blame` 显示上述逻辑来自 Worker 侧历史提交：

```text
5ceaeddb34fe1b9da7a139f76ebd9df6df79fd7c
fix(worker): 支撑 OlymMATH 长耗时 Episode，避免 Dispatch 静默断连
AuthorDate: 2026-07-16
```

对应 blame 片段：

```text
5ceaeddb ... let max_message_bytes =
5ceaeddb ...     env_usize_default("UENV_WORKER_GRPC_MAX_MESSAGE_BYTES", 16 * 1024 * 1024);
5ceaeddb ... let grpc_service = WorkerGrpcServiceServer::new(service)
5ceaeddb ...     .max_decoding_message_size(max_message_bytes)
5ceaeddb ...     .max_encoding_message_size(max_message_bytes);
```

### 3.3 Worker 配置示例

文件：`/data/ronghao/uenv/config/uenv-worker-llm.env.example`

```text
# UENV_WORKER_GRPC_MAX_MESSAGE_BYTES=16777216
```

### 3.4 真机部署日志证据

文件：`/data/ronghao/uenv/Docs/hub/260802-真机部署联调报告/260802-真机部署联调报告.md`

日志片段：

```text
INFO uenv_worker::runtime: listen=0.0.0.0:38888 http2_keepalive_interval_secs=30 max_message_bytes=16777216 msg="grpc_server_start"
```

这说明至少在该次真机部署中，Worker runtime 启动时已经按 16 MiB 上限运行。

## 4. Server 侧本次补齐的内容

### 4.1 修改位置

文件：`/data/ronghao/uenv/uenv-server/src/ports.rs`

近期提交：

```text
212fb0b fix(server): 放宽Worker gRPC客户端消息上限
```

### 4.2 修改内容

新增统一的 Worker gRPC client 创建函数：

```rust
async fn connect_worker_grpc_client(
    endpoint: &str,
) -> anyhow::Result<WorkerGrpcServiceClient<Channel>> {
    let max_message_bytes = grpc_max_message_bytes();
    let client = WorkerGrpcServiceClient::connect(format!("http://{endpoint}"))
        .await?
        .max_decoding_message_size(max_message_bytes)
        .max_encoding_message_size(max_message_bytes);
    Ok(client)
}
```

消息上限读取逻辑：

```rust
fn grpc_max_message_bytes() -> usize {
    env_usize("UENV_ADAPTER_CORE_GRPC_MAX_MESSAGE_BYTES")
        .or_else(|| env_usize("UENV_WORKER_GRPC_MAX_MESSAGE_BYTES"))
        .unwrap_or(DEFAULT_GRPC_MAX_MESSAGE_BYTES)
}
```

默认值：

```rust
const DEFAULT_GRPC_MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
```

### 4.3 覆盖的 Server -> Worker 调用路径

本次修改使以下三类 Worker gRPC 调用都复用 16 MiB client：

- `dispatch_episode`
- `cancel_episode`
- `prepare_environment`

其中最关键的是 `dispatch_episode`，因为它涉及 episode 下发和 Worker stream report 读取，是 SWE/OpenHands 长轨迹任务最容易遇到大消息或长连接问题的路径。

## 5. 为什么 Worker 已经 16 MiB 后还需要改 Server

Worker 侧的修改只说明 `WorkerGrpcServiceServer` 这个服务端允许最大 16 MiB 消息。

但 Server 调 Worker 时使用的是 `WorkerGrpcServiceClient`。如果这个 client 不设置 `max_decoding_message_size` / `max_encoding_message_size`，它仍可能保留 tonic / gRPC 默认上限。这样即使 Worker server 允许 16 MiB，Server client 仍可能在接收或发送方向被默认 4 MiB 卡住。

因此需要形成完整闭环：

```text
Python Adapter client: 16 MiB
Rust AdapterCore service: 16 MiB
Rust Server -> Worker client: 16 MiB
Worker runtime service: 16 MiB
```

本次 server 侧补丁就是补齐第三项。

## 6. 重启要求

修改生效需要重启对应进程：

| 组件 | 是否需要重启 | 原因 |
|---|---|---|
| Python Adapter / VeRL 训练进程 | 需要 | Python gRPC client 配置在进程启动时读取 |
| Rust AdapterCore / Server 进程 | 需要 | `AdapterCoreServiceServer` 和 `WorkerGrpcServiceClient` 代码需要新二进制生效 |
| Worker 进程 | 通常不需要 | Worker 侧此前已默认 16 MiB；若当前部署不是新代码或环境变量异常，则需要重启 |
| vLLM / model gateway | 不需要 | 本问题发生在 UEnv gRPC 结果传输链路，不是模型 HTTP 请求链路 |

## 7. 核验建议

重启后建议在 Server / Worker 日志中确认：

```text
max_message_bytes=16777216
grpc_server_start
```

同时在训练脚本或 AdapterCore 启动环境中显式保留：

```text
UENV_ADAPTER_CORE_GRPC_MAX_MESSAGE_BYTES=16777216
UENV_WORKER_GRPC_MAX_MESSAGE_BYTES=16777216
```

若后续单条 SWE/OpenHands episode 的 trajectory 继续膨胀，16 MiB 仍可能不足。届时应优先同时评估两类方案：

- 继续提高 gRPC 上限，例如 32 MiB。
- 控制 episode result 体积，例如减少不必要的完整 token trace、长文本、重复 trajectory 内容。
