# UEnv Protobuf — L1 控制面（canonical）

本目录为 **L1 控制面** 共享 Protobuf 的 **唯一权威路径**。各 crate 通过 `protoc -I=proto` 引用，禁止在多个 crate 内拷贝 `episode.proto` / `common.proto`。

## 目录结构

```
proto/uenv/v1/
├── common.proto      # 错误码、ResourceSpec、ExecutionMode
├── episode.proto     # EpisodeRequest / EpisodeResult / StreamReport
├── wal.proto         # WAL record schema（M1 冻结，M8 实现落盘）
└── scheduler.proto   # ControlPlaneService（Scheduler ↔ Worker 主动 RPC）
```

Worker 侧 gRPC Server 定义见 `uenv-worker/proto/worker_service.proto`（`DispatchEpisode`、`HealthCheck`）。

## L1 / L2 边界

| 层 | 目录 | 参与方 | 说明 |
|----|------|--------|------|
| **L1 控制面** | `proto/`、`uenv-worker/proto/` | Scheduler ↔ Worker | gRPC；Scheduler **主动** `DispatchEpisode` |
| **L2 插件 IPC** | `plugin_proto/` | Worker ↔ 插件子进程 | Protobuf over UDS；**禁止** import 进 L1 |

**规则**：

1. L1 proto 不得出现 UDS 路径、插件 PID、L2 message 类型。
2. `plugin_proto/` 不得被 Scheduler / Mock / Server crate 引用。
3. `env_type` Phase 0 仅 `"gsm8k"`。

## 代码生成

**依赖**：`protoc`、`protoc-gen-prost`、`protoc-gen-tonic`（`cargo install protoc-gen-prost protoc-gen-tonic`）。

```bash
make proto
# 或
bash scripts/proto-gen.sh
```

生成物写入各 crate 的 `src/gen/`（构建时生成，不提交）。使用 `--prost_out` / `--tonic_out`（非 protoc 内置 `--rust_out`）。

## protocol_version

当前冻结版本：**v1**（与 design §7.1、§7.5、§7.7 对齐）。
