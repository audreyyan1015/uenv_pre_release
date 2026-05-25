# uenv-server — UEnv 全栈调度服务

UEnv Server 是 UEnv **全栈方案** 的控制平面：接收 Bridge 提交的 Episode，维护 Worker 注册表与调度决策。

> **Worker Pool MVP（M1–M6）不依赖本 crate 完整实现。** MVP 联调请使用 [`uenv-mock-scheduler`](../uenv-mock-scheduler/)。M7 起与真实 Server 集成。

Layer 2 Worker Pool 权威文档：[Docs/worker-pool-layer-design.md](../Docs/worker-pool-layer-design.md)

## 与 Worker Pool 职责边界

| 能力 | uenv-server（全栈） | uenv-worker（Worker Pool） |
|------|---------------------|----------------------------|
| Bridge → Server Episode 提交 | ✅ `UEnvService` | — |
| Scheduler 主动 `DispatchEpisode` | M7+ | ✅ Worker gRPC Server |
| Worker 注册 / 心跳 / 结果上报 | M7+ ControlPlane | ✅ ControlPlane Client |
| 环境实例 Backend / 预热池 | **已 deprecated** | ✅ `backend/`、`pool/` |
| Worker WAL | **已 deprecated** | ✅ `wal/` |
| Mock 联调 | — | ✅ `uenv-mock-scheduler` |

`src/wal.rs` 与 `src/backend.rs` 已标记 **deprecated**，避免与 Worker 侧模块职责冲突。

## 快速使用

```bash
uenv-server serve --port 50051   # 长期运行入口统一为 serve（逐步迁移）
```

## 配置

见 `config/server.example.toml`（全栈 Server 配置，与 Worker YAML 分离）。
