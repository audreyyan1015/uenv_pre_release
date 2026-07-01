# UEnv — 训练框架无关的分布式环境执行框架

[![Rust](https://img.shields.io/badge/language-Rust-orange)](https://www.rust-lang.org)
[![Python](https://img.shields.io/badge/language-Python-blue)](https://www.python.org)
[![gRPC](https://img.shields.io/badge/communication-gRPC-brightgreen)](https://grpc.io)

UEnv 是一个**训练框架无关的分布式环境执行框架**，为 LLM 后训练（Post-Training）提供统一的 Environment 接口。

---

## 架构总览

### 全栈路径（Bridge → Server → Worker → Hub）

```
训练集群 (ROLL / VeRL / NeMo-RL / TRL / OpenRLHF)
       │
       ▼
┌──────────────────────────┐
│  uenv-bridge              │  训练框架适配器（design 称 uenv-adapter）
└──────────┬───────────────┘
           │ gRPC EpisodeRequest / EpisodeResult
           ▼
┌──────────────────────────┐
│  uenv-server              │  全栈调度控制面（M7+ Worker Pool 联调）
└──────────┬───────────────┘
           │ DispatchEpisode / ReportResult
           ▼
┌──────────────────────────┐
│  uenv-worker              │  Layer 2 Worker Pool 执行层
└──────────┬───────────────┘
           │ plugins/math 子进程（L2 Proto/UDS；dataset=gsm8k 在 payload）
           ▼
┌──────────────────────────┐
│  uenv-hub                 │  环境注册（非 Worker Pool MVP 阻塞项）
└──────────────────────────┘
```

### Layer 2 Worker Pool 控制面（design §1.1）

Worker Pool MVP 使用 **Scheduler 查 Pool → 直连 Worker DispatchEpisode** 路径：

```
uenv-mock-scheduler (或 uenv-server M7+)
       │ RegisterWorker / Heartbeat / ReportResult  ◄── Worker Client
       │ DispatchEpisode (stream StreamReport)       ──► Worker gRPC Server
       ▼
  uenv-worker
       │ L2 plugin_proto / UDS
       ▼
  plugins/math/
```

权威文档：[Docs/worker-pool-layer-design.md](./Docs/worker-pool-layer-design.md)

---

## 四大部分

| 目录 | 语言 | 职责 | CLI 入口 |
|:-----|:-----|:-----|:---------|
| [`uenv-bridge`](./uenv-bridge/) | Python | 协议转换（= design 的 uenv-adapter） | 嵌入训练进程 |
| [`uenv-server`](./uenv-server/) | Rust | 全栈调度（M7 前 Worker Pool 不阻塞） | `uenv-server serve` |
| [`uenv-worker`](./uenv-worker/) | Rust | Worker Pool 执行 + 插件 + 预热池 | `uenv-worker serve` |
| [`uenv-mock-scheduler`](./uenv-mock-scheduler/) | Rust | MVP Mock ControlPlane | `uenv-mock-scheduler serve` |
| [`uenv-hub`](./uenv-hub/) | Rust | 环境元数据注册 | `uenv-hub serve` |

---

## Worker Pool MVP 快速开始

```bash
# 1. 生成 proto
make proto

# 2. 启动 Mock Scheduler（M1 实现完整逻辑）
cd uenv-mock-scheduler && cargo run -- serve

# 3. 启动 Worker
cd uenv-worker && cargo run -- serve --config ../config/uenv-worker.yaml

# 4. 训练框架侧
pip install ./uenv-bridge
```

配置示例：`config/uenv-worker.yaml`（YAML/JSON，ADR-002）。

---

## 通信协议

| 链路 | 协议 | 说明 |
|:-----|:-----|:------|
| Bridge ↔ Server | `UEnvService` (L1 gRPC) | 提交 Episode |
| Scheduler ↔ Worker | `ControlPlaneService` + `WorkerGrpcService` (L1) | 注册/心跳/上报 + **主动 Dispatch** |
| Worker ↔ Plugin | `plugin_proto/` (L2 UDS) | reset/step/close/health_check |
| Worker ↔ 推理服务 | HTTP/gRPC | 模型回调，不经 Server |

Proto 权威路径：`proto/`（L1）、`plugin_proto/`（L2）。见 [proto/README.md](./proto/README.md) 与 **[PROTOCOL.md](./PROTOCOL.md)**（通信协议与数据结构规范）。

---

## 构建

```bash
make proto              # 生成各 crate src/gen/
make build              # server + worker + mock-scheduler + hub
make build-worker
make build-mock-scheduler

# 或使用 workspace
cargo build -p uenv-worker -p uenv-mock-scheduler
```

---

## 许可

Apache-2.0
