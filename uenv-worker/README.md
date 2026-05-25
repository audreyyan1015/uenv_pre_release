# uenv-worker — UEnv Worker Pool 执行层

Worker 是 UEnv **Layer 2 Worker Pool** 的执行节点：gRPC **Server** 接收 Scheduler 主动下发的 `DispatchEpisode`，并通过 ControlPlane **Client** 上报 `RegisterWorker` / `Heartbeat` / `ReportResult`。

权威设计文档：[Docs/worker-pool-layer-design.md](../Docs/worker-pool-layer-design.md)

## 职责

- **Episode 执行**：`EpisodeExecutor` 管理 reset → N×step → close（M2+）
- **模型回调**：`ModelClient` 直连推理服务（HTTP/gRPC）
- **预热池**：`WarmupPool` 管理进程级插件实例（M3+）
- **插件子进程**：`ProcessBackend` + `plugins/gsm8k/`（M4+）；**非**内嵌 Python 主路径
- **Worker WAL**：断连重放（schema M1 冻结，持久化 M8）

## 模块结构（design §13）

```
src/
├── cli/                 # serve / version / health
├── config/              # YAML/JSON（ADR-002）
├── runtime.rs
├── control_plane/       # RegisterWorker / Heartbeat / ReportResult
├── grpc_server/         # DispatchEpisode / HealthCheck
├── episode/             # executor, model_client
├── pool/                # warmup_pool
├── plugin/              # host, instance, arpc (L2)
├── backend/             # process, podman
├── wal/
└── logging/
```

## CLI

```bash
# 启动 Worker（M2 实现完整运行时）
uenv-worker serve --config config/uenv-worker.yaml

uenv-worker version
uenv-worker health
```

## 配置

主示例：`config/uenv-worker.yaml`（或 `config/uenv-worker.json`）。

`config/worker.example.toml` 已 **deprecated**，请迁移至 YAML/JSON。

## 环境插件

Phase 0 唯一环境：`plugins/gsm8k/`（`manifest.yaml`, `ipc=proto-uds`）。

`uenv-worker/python/` 为历史内嵌环境路径，**非 MVP 主路径**（Phase 1+ 或 legacy）。

## Mock 联调

MVP 阶段使用独立 crate `uenv-mock-scheduler` 作为 ControlPlane，无需完整 `uenv-server`：

```bash
uenv-mock-scheduler serve --fixture-dir ./fixtures/gsm8k
uenv-worker serve --config config/uenv-worker.yaml
```

## 术语对照

| 设计文档 | 本仓库 |
|----------|--------|
| `uenv-adapter` | `uenv-bridge` |
| `WarmupPool` | `src/pool/warmup_pool.rs` |
| `ModelClient` | `src/episode/model_client.rs` |
