# UEnv Protobuf — L1 控制面（canonical）

本目录为 **L1 控制面** 共享 Protobuf 的 **唯一权威路径**。各 crate 通过 `protoc -I=proto` 引用，**禁止**在 crate 内重复定义 `EpisodeRequest` / `EpisodeResult` 等核心消息。

权威规范文档：[PROTOCOL.md](../PROTOCOL.md)

## 目录结构

```
proto/uenv/v1/
├── common.proto      # ErrorCode、ResourceSpec、ExecutionMode
├── episode.proto     # EpisodeRequest / EpisodeResult / StreamReport
├── wal.proto         # WalRecord（§7.5 冻结）
├── scheduler.proto   # ControlPlaneService（Worker ↔ Scheduler/Server 控制面）
└── server.proto      # UEnvService + AdminService（Bridge ↔ Server）
```

Worker 侧 gRPC Server 定义见 `uenv-worker/proto/worker_service.proto`（`WorkerGrpcService`）。

## PRD v7.2 服务名对照

| PRD §4.2 名称 | 规范 proto 服务 | 实现方 |
|---------------|-----------------|--------|
| UEnvService | `uenv.v1.UEnvService` | uenv-server |
| DispatcherService + WorkerDirectService | `uenv.scheduler.v1.ControlPlaneService` | uenv-server / uenv-mock-scheduler |
| DispatchEpisode（Server → Worker） | `uenv.worker.v1.WorkerGrpcService` | uenv-worker |

## L1 / L2 边界

| 层 | 目录 | 参与方 | 说明 |
|----|------|--------|------|
| **L1 控制面** | `proto/`、`uenv-worker/proto/` | Scheduler ↔ Worker | gRPC；Scheduler **主动** `DispatchEpisode` |
| **L2 插件 IPC** | `plugin_proto/` | Worker ↔ 插件子进程 | Protobuf over UDS；**禁止** import 进 L1 |
| **Hub 元数据** | HTTP REST + `uenv-hub-types` | Hub ↔ CLI/Server | 非 gRPC；DTO 与 L1 字段语义对齐 |

**规则**：

1. L1 proto 不得出现 UDS 路径、插件 PID、L2 message 类型。
2. `plugin_proto/` 不得被 Scheduler / Server crate 引用。
3. Phase 0 默认 `env_type = "gsm8k"`。

## 代码生成

**依赖**：`protoc`、`protoc-gen-prost`、`protoc-gen-tonic`。

```bash
make proto
# 或
bash scripts/proto-gen.sh
```

| Crate | 生成方式 | 编译的 proto |
|-------|----------|--------------|
| uenv-server | `build.rs`（`tonic::include_proto!`） | server + scheduler + worker_service |
| uenv-worker | Makefile / script | worker_service + scheduler + episode + … |
| uenv-mock-scheduler | Makefile / script | scheduler + episode + … |
| uenv-bridge | Makefile / script | server + scheduler + episode（Python） |

生成物：`uenv-server` 写入 `OUT_DIR`；其余 crate 写入各 `src/gen/`（不提交）。

## protocol_version

当前冻结版本：**v1**（package `uenv.v1` / `uenv.scheduler.v1` / `uenv.worker.v1`）。
