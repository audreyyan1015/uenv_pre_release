# Worker Pool 层 — MVP 前置架构调整说明

> **文档版本**：v1.0  
> **依据**：[worker-pool-layer-design.md](./worker-pool-layer-design.md)（v1.3）、[worker-pool-mvp-checklist.md](./worker-pool-mvp-checklist.md)（v1.3）  
> **用途**：记录在 **正式推进 MVP 代码实现之前**，对当前仓库目录、crate 边界、命名与契约所做的 **框架级调整**  
> **最后更新**：2026-05-25（前置调整已完成）

---

## 1. 本文档的定位

### 1.1 写什么

本文档只记录 **「当前项目骨架」与「设计文档 v1.3 目标框架」之间的差异**，以及 **进入 MVP 实施前必须完成的结构调整与契约对齐**。

完成本文所列调整后，再以 [worker-pool-mvp-checklist.md](./worker-pool-mvp-checklist.md) 为执行清单，从 **M1** 起逐项实现业务逻辑。

### 1.2 不写什么（与 MVP 清单的边界）

| 类别 | 归属文档 | 示例 |
|------|----------|------|
| **MVP 前置架构调整** | **本文档** | 重组 `uenv-worker/src/` 目录；proto 契约重写；新建 `uenv-mock-scheduler` crate 空壳；配置 TOML→YAML；README 与控制面叙述对齐 |
| **MVP 阶段功能实现** | [worker-pool-mvp-checklist.md](./worker-pool-mvp-checklist.md) | Mock 主动 Dispatch 逻辑；EpisodeExecutor 执行循环；GSM8K 插件 reset/step；WAL 落盘；Prometheus 指标；M1.7 混沌测试用例 |
| **Phase 1+ / 非 MVP** | design §14 W6+、checklist M9+ | WarmupSizer 动态容量；PodmanBackend；Cap'n Proto；OpenTelemetry |

**判断标准**：若调整的是 **目录是否存在、模块归属哪个 crate、proto/配置/CLI 契约、命名约定**，属于本文档；若是在 **已对齐的框架上编写运行时代码与测试**，属于 MVP 清单。

### 1.3 权威来源

发生冲突时，以以下两份文档为准：

1. [worker-pool-layer-design.md](./worker-pool-layer-design.md) — 架构与契约  
2. [worker-pool-mvp-checklist.md](./worker-pool-mvp-checklist.md) — 交付顺序与验收（Mock 独立 crate 等实施细节以 checklist 为准）

---

## 2. 当前 vs 目标：一览

### 2.1 仓库顶层（As-Is）

```
UEnv/
├── Docs/
├── config/                    # worker.example.toml 等（TOML）
├── scripts/                   # proto-gen.sh（路径与 Makefile 不一致）
├── Makefile
├── README.md                  # uenv-* start；server 中心化叙述
├── uenv-bridge/               # Python 训练适配
├── uenv-server/               # scheduler + registry + pool + wal + backend
├── uenv-worker/               # 扁平 src/ + python/ 内嵌环境
└── uenv-hub/
```

### 2.2 目标框架（To-Be，MVP 启动前应对齐）

```
UEnv/
├── Docs/
├── config/
│   ├── uenv-worker.yaml       # ADR-002 示例（可并存 .json）
│   └── uenv-mock-scheduler.yaml
├── proto/                     # 可选：L1 共享 proto 根（见 §3.4）
│   └── README.md              # L1/L2 边界说明
├── plugin_proto/              # L2 插件 IPC（与 L1 严格分离）
├── plugins/gsm8k/             # 子进程插件占位（manifest.yaml）
├── fixtures/gsm8k/            # Mock fixture 占位目录
├── uenv-mock-scheduler/       # 独立 crate（checklist M1.2）
├── uenv-worker/               # 按 design §13 模块化
├── uenv-server/               # 职责收敛（见 §3.1）
├── uenv-bridge/               # 命名与文档对齐（见 §3.6）
└── uenv-hub/                  # 不在 Worker Pool MVP 阻塞范围内
```

---

## 3. 前置调整项（按优先级）

以下每一项均需在 **M1 业务代码开发之前** 或 **与 M1 proto 冻结同步** 完成。表格中 **「调整后状态」** 表示框架就绪、可开始填 MVP 实现，而非功能已完成。

---

### A. Crate 与模块职责边界（P0）

**问题**：当前 `uenv-server` 聚合了调度、Worker 注册表、实例池、WAL、Backend；design 要求 **Worker 侧** 承担执行、插件、预热池、Worker WAL、Worker gRPC Server，**Scheduler/Mock** 承担 ControlPlane 与主动 Dispatch。

| 能力 / 模块 | 当前位置 | 目标位置（design） | 前置调整动作 |
|-------------|----------|-------------------|--------------|
| `DispatchEpisode` / `HealthCheck`（被调方） | 无（`worker.proto` 混在 `WorkerService`） | `uenv-worker/src/grpc_server/` | 拆分 proto 服务；预留目录与空模块 |
| `RegisterWorker` / `Heartbeat` / `ReportResult`（Worker 主动） | `worker.proto` 的 `WorkerService` | Worker 侧 **Client** → Server/Mock ControlPlane | proto 拆为 Scheduler 服务 + Worker 服务；`control_plane/client.rs` 占位 |
| `WalWriter` / 断连重放 | `uenv-server/src/wal.rs` | `uenv-worker/src/wal/` | **从 server 移除或标记 deprecated**；worker 侧新建 `wal/` 目录（M8 再实现逻辑，M1 冻结 schema） |
| `WarmupPool` / 环境实例生命周期 | `uenv-worker/warmpool.rs`（空） | `uenv-worker/src/pool/warmup_pool.rs` | 重命名 `WarmPool`→`WarmupPool`；迁入 `pool/` |
| `Backend`（Process/Podman 启插件） | `uenv-server/src/backend.rs` | `uenv-worker/src/backend/` | server 侧 backend 若指「环境实例」则迁出或改语义；避免双份 Backend |
| `WorkerPoolRegistry` / `ListWorkers` | `uenv-server/registry.rs` + `pool.rs` | Scheduler/Mock **只读查询**；design §4 可选 worker 侧 `registry/worker_pool.rs` | 明确：**M1–M6 Mock 阶段** 由 `uenv-mock-scheduler` 内存实现 `ListWorkers`；`uenv-server` 在 M7 前不阻塞 MVP |
| `EpisodeExecutor` / `ModelClient` | `executor.rs` / `inference.rs` | `episode/executor.rs` / `episode/model_client.rs` | 目录重组；`inference` 重命名为 `model_client` |
| Mock Scheduler | 不存在 | **独立** `uenv-mock-scheduler/`（checklist 优先于 design §13 内嵌 mock） | 新建 crate 骨架 + `Cargo.toml` + `src/main.rs` CLI 占位 |

**验收（框架级）**：crate 职责表写入 design 或本文附录；`uenv-server` 与 `uenv-worker` 无重复 WAL/Backend/预热池模块；目录与 design §13 一致（可为空实现）。

> **不属于本文、属于 MVP 清单**：M2 实现 `WorkerGrpcServer` 逻辑；M1 实现 Mock 主动 Dispatch；M8 实现 WAL 持久化。

---

### B. `uenv-worker` 源码目录重组（P0）

**问题**：当前为扁平 7 模块；design §13 要求分层目录。

| 当前路径 | 目标路径 | 动作 |
|----------|----------|------|
| `src/main.rs`（占位 println） | `src/main.rs` + `src/cli/` | 预留 clap 结构；子命令名 **serve**（非 start） |
| `src/config.rs` | `src/config/mod.rs` | 目录化；后续 M3 接 YAML/JSON |
| `src/executor.rs` | `src/episode/executor.rs` | 移动 |
| `src/inference.rs` | `src/episode/model_client.rs` | 移动并重命名 |
| `src/warmpool.rs` | `src/pool/warmup_pool.rs` | 移动并重命名类型 |
| `src/grpc.rs` + `grpc/client.rs` | `src/control_plane/client.rs` + `src/grpc_server/` | 拆分 Client/Server |
| — | `src/runtime.rs` | 新建占位 |
| — | `src/plugin/`、`src/backend/`、`src/wal/`、`src/logging/` | 新建空模块 |
| `src/state.rs` | `src/state.rs` 或并入 `runtime` | 状态枚举对齐 design §7.2（见 §3.5） |
| `uenv-worker/python/` | **不作为 MVP 主路径** | 保留可标记 `legacy/` 或文档注明 Phase 1+；MVP 环境走 `plugins/gsm8k/` 子进程 |

**验收**：`cargo build` 通过；`lib.rs` 模块树与 design §13 一致。

---

### C. Proto 与双协议层（P0）

**问题**：L1 控制面契约与 v1.3 不符；无 L2 `plugin_proto/`；构建脚本路径不一致。

#### C.1 L1 控制面（Scheduler ↔ Worker）

| 项 | 当前 | 目标（design §7.1、§1.4） | 前置调整 |
|----|------|---------------------------|----------|
| `DispatchEpisode` 返回值 | `DispatchEpisodeResponse` | `stream StreamReport` | 重写 `worker.proto` / 拆分 control plane proto |
| 结果上报 | `ReportStream`（方向相反） | Worker 主动 `ReportResult` | 新增 RPC；删除或废弃 `ReportStream` |
| 探活 | 无 | `HealthCheck` | 新增 |
| 字段 | `request_id`；无租约/WAL | `episode_id`、`attempt_id`、`server_epoch`、`idempotency_key`、`dispatch_lease_id`、`lease_expire_at` | 扩展 `episode.proto` 或 L1 专用 message |
| 服务归属 | 全在 `WorkerService` | ControlPlane 在 Scheduler 侧；Dispatch/Health 在 Worker 侧 | 拆成 `scheduler.proto`（或 mock/server）+ `worker_service.proto` |
| `env_type` Phase 0 | `"math"\|"code"\|"agent"` | `"gsm8k"` | 注释与示例改为 gsm8k |
| WAL schema | 无 | §7.5 message + `replay_state` enum | M1 随 proto 一并冻结（**实现**在 M8） |

#### C.2 L2 插件 IPC（Worker ↔ Plugin）

| 项 | 当前 | 目标 | 前置调整 |
|----|------|------|----------|
| IDL 目录 | 无 | `plugin_proto/`（与 `proto/` 分离） | 新建目录 + 占位 `.proto`（reset/step/close/health_check） |
| 文档 | 无 | `proto/README.md` 或 `docs/proto-boundary.md` | 说明 L1/L2 边界、import 禁止规则 |

#### C.3 Proto 物理布局与构建

| 项 | 当前 | 建议目标 | 前置调整 |
|----|------|----------|----------|
| Episode 类型 | `uenv-server/proto/uenv/v1/episode.proto` | 保持为 **canonical 共享** 或迁至 repo 根 `proto/uenv/v1/` | 在 README 中 **写死唯一路径**，避免双份拷贝 |
| 代码生成 | Makefile 可用；`scripts/proto-gen.sh` 引用不存在的 `$ROOT/proto`、`uenv-adapter` | 单一入口 | 修正 `proto-gen.sh` 与 Makefile 一致；各 crate 增加 `build.rs` 或文档化 `make proto` |
| 生成物 | 无 committed `src/gen/` | 构建时生成 | 选定策略并在 README 说明 |

**验收**：proto 文件与设计 §7.1、§7.5、§2.4 字段级对齐；`make proto && cargo build` 通过；L1/L2 目录分离。

> **不属于本文、属于 MVP 清单**：M1.2 Mock 实现 Dispatch 客户端；M1.7 混沌测试；M4 插件 UDS 通信实现。

---

### D. 顶层目录与 crate 新增（P0）

| 路径 | 当前 | 目标 | 前置调整 |
|------|------|------|----------|
| `uenv-mock-scheduler/` | 无 | 独立 crate | 新建；CLI：`serve` / `version`；默认日志路径约定 |
| `plugins/gsm8k/` | 无 | manifest + 入口占位 | 新建 `manifest.yaml`（env_type=gsm8k, ipc=proto-uds） |
| `fixtures/gsm8k/` | 无 | Mock 测试数据目录 | 新建空目录 + `.gitkeep` 或 README |
| `plugin_proto/` | 无 | L2 IDL | 新建 |
| `config/uenv-worker.yaml` | 无（仅有 `worker.example.toml`） | ADR-002 示例 | 新增 YAML/JSON 示例；TOML 标记 **deprecated** |
| `config/uenv-mock-scheduler.yaml` | 无 | Mock 配置示例 | 新增 |

**验收**：目录存在且被 `.gitignore` / 文档引用正确；无 MVP 业务逻辑要求。

---

### E. 配置、CLI、日志约定（P1）

**问题**：仓库约定与 design §2.2、§2.5、§2.6 不一致。

| 项 | 当前 | 目标 | 前置调整 |
|----|------|------|----------|
| 配置格式 | TOML（`config/worker.example.toml`） | YAML **与** JSON（ADR-002） | 新增示例文件；worker config 模块接口按嵌套 schema 设计（实现可空） |
| 配置路径 | `config/worker.example.toml` | `config/uenv-worker.yaml`；默认查找 `/etc/uenv/worker.yaml` | 文件名与 design §2.6 对齐 |
| CLI 子命令 | README：`start` / `status` | `serve` / `version` / `health` | 更新根 README、`uenv-worker/README.md`；二进制占位 CLI 使用 `serve` |
| 全栈 CLI | `uenv-server start`、`uenv-hub start` | 统一 `serve`（跨层约定） | README 表格更新 |
| 日志路径 | 未约定 | `/var/log/uenv/worker.log` 等 | README 与 config 示例写入 `logging.file` |
| 环境变量 | 部分 | design §12 映射表 | config 示例与 env 键一一对应 |

**验收**：文档与示例配置一致；CLI 帮助文本使用 `serve`；**不要求** ADR-001 日志落盘已实现（属 M3）。

> **不属于本文、属于 MVP 清单**：M3 实现 config 加载、LogSink、CLI 完整参数解析。

---

### F. 命名与文档对齐（P1）

| 项 | 当前 | 目标 | 前置调整 |
|----|------|------|----------|
| 训练适配 crate | `uenv-bridge` | design 跨层称 `uenv-adapter` | 在 design 或 README 增加 **术语对照表**（bridge = adapter，暂不强制重命名目录） |
| 预热池 | `WarmPool` | `WarmupPool` | 重命名源文件与类型 |
| 推理客户端 | `InferenceClient` | `ModelClient` | 重命名 |
| Worker 状态机 | `Starting, Ready, Busy, Draining, Offline` | `Created, Ready, Busy, Draining, Terminated`（§7.2） | 调整 enum 命名 |
| Mock 位置 | design §13 写 `uenv-worker/mock/` | checklist：**独立 crate** | **以 checklist 为准**；design §13 后续修订指向 `uenv-mock-scheduler` |

**验收**：代码与 design 术语一致；两份 design 文档间 Mock 位置无歧义。

---

### G. `uenv-server` 与全栈 README（P1）

**问题**：根 README 描述「Bridge → Server SubmitEpisode → Worker」与 Worker Pool 文档「Scheduler 查 Pool → 直连 Worker DispatchEpisode」在 **路径叙述** 上需共存但不混淆。

| 调整项 | 说明 |
|--------|------|
| 根 `README.md` | 增加 **Layer 2 Worker Pool** 小节，指向 `Docs/worker-pool-layer-design.md`；控制面箭头改为与 §1.1 一致 |
| `uenv-server/README.md` | 标明 M7 前 Worker Pool MVP 不依赖完整 server；server 模块与 Worker 侧职责边界 |
| `uenv-worker/README.md` | 删除「内嵌 Python 环境为主路径」叙述；改为 ProcessBackend + 插件子进程；CLI `serve` |
| Worker Pool 与 Server 关系 | 在 design 增补「与现有 uenv-server crate 映射表」（建议后续 patch design v1.4） |

**验收**：新贡献者读 README 不会按旧 proto/旧 CLI 实现；Worker Pool 文档为 Layer 2 权威。

> **不属于本文、属于 MVP 清单**：M7 与真实 `uenv-server` 联调。

---

### H. 构建与工程卫生（P2）

| 项 | 当前 | 动作 |
|----|------|------|
| 根 `Cargo.toml` workspace | 无 | 可选：将 `uenv-worker`、`uenv-mock-scheduler` 纳入 workspace 便于联调 |
| `scripts/proto-gen.sh` | 路径错误 | 与 Makefile 对齐或删除冗余脚本 |
| `uenv-worker/python/` | 与 MVP 插件路径冲突 | 文档标注非 MVP 主路径 |

---

## 4. 建议执行顺序

```
阶段 0（本文档范围 — 框架对齐）  ✅ 已完成 2026-05-25
│
├─ 0.1  冻结 crate 职责表（server vs worker vs mock-scheduler）     [x]
├─ 0.2  新建 uenv-mock-scheduler、plugins/、fixtures/、plugin_proto/ 骨架  [x]
├─ 0.3  重组 uenv-worker/src/ 目录 + 重命名（WarmupPool、ModelClient、状态机）  [x]
├─ 0.4  重写/拆分 L1 proto + 新建 L2 plugin_proto 占位 + proto/README.md  [x]
├─ 0.5  修正构建脚本；proto-gen + cargo build 全 crate 通过  [x]
├─ 0.6  配置示例 TOML→YAML/JSON；CLI/README 统一 serve  [x]
└─ 0.7  更新 uenv-server 模块归属（WAL/Backend 迁出或 deprecated 标记）  [x]
│
▼
阶段 1（MVP 清单 — 业务实现）  ← 当前可进入
└─ 从 checklist M1 开始：Mock 逻辑、Worker gRPC、插件、Episode…
```

---

## 5. 前置调整完成标准（Gate）

满足以下全部条件后，方可将 [worker-pool-mvp-checklist.md](./worker-pool-mvp-checklist.md) 的 **M1** 标为「进行中」：

| # | 标准 | 状态 |
|---|------|------|
| 1 | `uenv-mock-scheduler/` crate 存在且 `cargo build` 通过 | ✅ |
| 2 | `uenv-worker/src/` 模块树与 design §13 一致（允许空实现） | ✅ |
| 3 | L1 proto 与 design §7.1、§7.5 字段对齐；L2 `plugin_proto/` 已分离 | ✅ |
| 4 | `plugins/gsm8k/manifest.yaml`、`fixtures/gsm8k/` 目录存在 | ✅ |
| 5 | `config/uenv-worker.yaml`（及/或 `.json`）替代 TOML 为主示例 | ✅ |
| 6 | README / CLI 约定统一为 `serve`；控制面叙述与 design §1.1 一致 | ✅ |
| 7 | `uenv-server` 与 `uenv-worker` 无 WAL/Backend/预热池职责冲突（已迁移或 documented deprecated） | ✅ |
| 8 | `make proto` / `scripts/proto-gen.sh`（或等价 `protoc --prost_out`）可重复生成代码 | ✅ |

**Gate 结论**：2026-05-25 已全部满足，可开始 MVP checklist **M1** 业务实现。

---

## 8. 前置调整执行清单（§3 分项状态）

| 章节 | 调整项 | 状态 | 备注 |
|------|--------|------|------|
| **A** | Crate 职责边界；mock 独立 crate | ✅ | `uenv-mock-scheduler/`；server WAL/Backend deprecated |
| **B** | `uenv-worker/src/` 目录重组 | ✅ | design §13 模块树；`serve` CLI 占位 |
| **C** | L1 proto 拆分 + L2 `plugin_proto/` | ✅ | `proto/uenv/v1/` canonical；`worker_service.proto` |
| **D** | 顶层目录与 crate 新增 | ✅ | `plugins/gsm8k/`、`fixtures/gsm8k/` |
| **E** | 配置 YAML/JSON、CLI `serve` | ✅ | `config/uenv-worker.yaml`；TOML deprecated |
| **F** | 命名对齐（WarmupPool、ModelClient、状态机） | ✅ | `Created/Ready/Busy/Draining/Terminated` |
| **G** | README 与 Worker Pool 叙述 | ✅ | 根 README Layer 2 小节；各 crate README |
| **H** | workspace、proto-gen 脚本 | ✅ | 根 `Cargo.toml` workspace；`--prost_out` 生成 |

---

## 6. 附录：差异速查表

| 维度 | 当前 | 目标 | 本文档 § |
|------|------|------|----------|
| Mock Scheduler | 无 | `uenv-mock-scheduler/` | A, D |
| Worker 模块树 | 扁平 7 文件 | design §13 分层 | B |
| 插件路径 | `uenv-worker/python/` | `plugins/gsm8k/` 子进程 | B, D |
| L2 proto | 无 | `plugin_proto/` | C, D |
| WAL | server 侧 | worker 侧 | A |
| WarmupPool | `warmpool.rs` | `pool/warmup_pool.rs` | A, B, F |
| 配置 | TOML | YAML/JSON | E |
| CLI | `start` | `serve` | E, F |
| env_type | math/code/agent | gsm8k | C |
| Dispatch RPC | 同步 Response | stream StreamReport | C |
| MVP 功能代码 | 未实现 | M1–M8 清单 | **非本文** |

---

## 7. 相关文档

- [worker-pool-layer-design.md](./worker-pool-layer-design.md) — 架构与 ADR 权威说明  
- [worker-pool-mvp-checklist.md](./worker-pool-mvp-checklist.md) — 框架对齐 **之后** 的分阶段实现与验收  
- [UEnv 方案 v7.1 PDF](./UEnv%20—%20下一代分布式训练环境框架方案-v7.1.pdf) — 上层方案依据
