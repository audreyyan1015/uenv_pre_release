# uenv-server — UEnv 调度服务

UEnv Server 是 UEnv 分布式环境框架的**控制平面**，负责 Episode 的调度编排、Worker 注册、实例池管理等元数据操作。不参与 step 级数据流，确保不会成为性能瓶颈。

## 职责

- **环境注册表**：维护 env_type → worker 的全局映射
- **调度器**：接收 EpisodeRequest，过滤候选 Worker，打分排序选择最优
- **实例池**：管理环境实例的生命周期（预热 / 复用 / 销毁）
- **后端管理器**：管理 Process / Podman 后端的启动和停止
- **状态管理**：维护 Episode 状态机和 Worker 生命周期
- **容错**：Write-Ahead Log 保证调度决策持久化

## 架构

```
┌────────────────────────────────────────────┐
│  uenv-server                                │
│                                              │
│  gRPC (port 50051)                          │
│  ┌──────────────────────────────────────┐   │
│  │ UEnvService (Bridge → Server)       │   │
│  │   SubmitEpisode / SubmitStream / ... │   │
│  └──────────────────────────────────────┘   │
│  ┌──────────────────────────────────────┐   │
│  │ AdminService (运维管理)               │   │
│  │   ListWorkers / DrainWorker / ...    │   │
│  └──────────────────────────────────────┘   │
│                                              │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐   │
│  │ 注册表    │ │ 调度器   │ │ 实例池   │   │
│  │ env_type→ │ │ 过滤+打分 │ │ 预热+复用 │   │
│  └──────────┘ └──────────┘ └──────────┘   │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐   │
│  │ 后端管理器│ │ 状态机  │ │ WAL      │   │
│  │Process/  │ │Episode/ │ │ 持久化   │   │
│  │ Podman   │ │ Worker  │ │          │   │
│  └──────────┘ └──────────┘ └──────────┘   │
└────────────────────────────────────────────┘
```

## 调度策略

| 策略 | 权重 | 说明 |
|:-----|:-----|:------|
| 负载均衡 | 50% | 优先选择负载最低的 Worker |
| 类型亲和 | 30% | 优先选择已加载该环境的 Worker |
| 延迟优化 | 20% | 优先选择网络延迟最低的 Worker |

## gRPC 服务

### UEnvService（Bridge 调用）

| RPC | 模式 | 说明 |
|:----|:-----|:------|
| SubmitEpisode | Unary | 同步提交单个 Episode |
| SubmitEpisodeStream | Bidi Streaming | 流式提交/返回 |

### AdminService（运维操作）

| RPC | 说明 |
|:----|:------|
| ListWorkers | 列出所有注册的 Worker |
| DrainWorker | 排空指定 Worker（优雅下线） |
| CancelEpisode | 取消正在执行的 Episode |

## 快速使用

```bash
# 启动
cargo run -- start --port 50051

# 查看 Worker 列表
cargo run -- list-workers

# 排空 Worker
cargo run -- drain worker-1 --grace 30
```

## 配置

参考 ../config/server.example.toml：

```toml
port = 50051

[scheduler]
strategy = "weighted"

[pool]
max_idle = 16
warmup_enabled = true
warmup_env_types = ["math", "code"]
```

## 依赖

- **通信**: tonic (gRPC), prost (Protobuf)
- **运行时**: tokio
- **配置**: serde, serde_json
- **日志**: tracing, tracing-subscriber
- **并发**: dashmap, parking_lot
