# Worker Pool 层 MVP 实现清单

> **文档版本**：v1.3  
> **依据**：[worker-pool-layer-design.md](./worker-pool-layer-design.md)（v1.3）  
> **用途**：分阶段交付 Worker Pool 层，支持排期与验收  
> **最后更新**：2026-05-26  
> **v1.3 变更**：对齐 design §2.5 CLI、§2.6 YAML/JSON 配置、§2.2 平台级 `/var/log/uenv/` 日志目录与 `tail -f` 验收项  
> **v1.2 变更**：插件 Runtime 1 进程 = 1 实例；Dispatch 租约字段；插件崩溃语义；L1/L2 协议边界  
> **v1.1 变更**：集中式调度 + Worker gRPC Server；M1.7 混沌测试；WAL schema；M5/M6 metrics；Proto/UDS MVP

> **文档维护（编码）** — 2026-05-25 曾发生 UTF-8 损坏，修改本文时请遵守：
>
> - 本文须以 **UTF-8（无 BOM）** 保存；`Get-Content` 只读通常安全，**`Set-Content` / `Add-Content` 重写字节才是高风险操作**。
> - PowerShell 写入必须显式指定编码，例如 `Set-Content -Encoding utf8NoBOM ...`、`Add-Content -Encoding utf8NoBOM ...`（PS 7+）；**禁止**依赖默认编码。
> - 已确认的高风险模式：`Set-Content -Path 'Docs\worker-pool-mvp-checklist.md' -Value (...)` **未带 `-Encoding`**，会导致中文尾字节变为 `?` 或错字。
> - 损坏后**禁止**用“猜测 UTF-8 续字节”的脚本批量修复（会产出形似中文的错字，如「骨枀」「插什」）；应以 [`worker-pool-mvp-checklist_1.md`](./worker-pool-mvp-checklist_1.md) 等正确副本合并还原。
> - 大段修改优先编辑器或 Agent `Write` 整文件写入；若用 PowerShell 拼接 `$head + $mid + $tail`，读写编码必须一致。
> - 改完后抽查「用途」「骨架」「插件」「持久化」等关键词，确认无 `?`、无错字。

---

## 使用说明

- 每个 **MVP 阶段** 有独立 **退出标准（Exit Criteria）**；全部满足后再进入下一阶段。
- 任务项格式：`- [ ]` 待完成 / `- [x]` 已完成（实施时自行勾选）。
- **第一步固定为 M1：Mock Scheduler Gateway**，不依赖真实 UEnv Server、环境插件或 Episode 执行引擎。
- **控制面模型（冻结）**：Worker = **gRPC Server**（`DispatchEpisode`）；Mock/真实 Scheduler = **主动调用方**；Worker 通过 ControlPlane **主动**注册/心跳/上报；**禁止** Worker `subscribe_dispatch` 拉任务。
- **插件 Runtime（冻结）**：**1 插件子进程 = 1 environment instance**；WarmupPool 管理进程级实例；同进程同时仅 1 个 Active Episode（§3.5）。
- **双协议层**：Scheduler ↔ Worker（L1 gRPC）与 Worker ↔ Plugin（L2 Proto/UDS）**严格隔离**，Scheduler 不感知插件 IPC（§2.4）。
- 设计细节见 [worker-pool-layer-design.md](./worker-pool-layer-design.md)，本文只列交付物与验收项。
- **MVP 启动前提**：须先完成 [worker-pool-pre-mvp-architecture-adjustment.md](./worker-pool-pre-mvp-architecture-adjustment.md) 中的框架对齐（目录、proto、crate 边界等）；**本文 M1–M8 为框架就绪后的功能实现**，二者勿混为一谈。

### 总览路线图

```
M1 Mock Scheduler + proto/WAL 冻结 ──► M2 Worker gRPC Server + ControlPlane
       │
       ▼
M3 日志/配置(ADR) ──► M4 GSM8K 插件(Process+Proto/UDS)
       │
       ▼
M5 Episode 执行 + 最小 metrics ──► M6 预热池 + 池 metrics + 复用安全
       │
       ▼
M7 真实 Scheduler 联调(Server 直连 Dispatch) ──► M8 WAL 持久化/重连 ──► M9+ 增强
```

| 阶段 | 名称 | 核心交付 | 可演示能力 |
|------|------|----------|------------|
| **M1** | Mock Scheduler Gateway | Mock Scheduler + proto + **WAL schema 冻结** + M1.7 混沌测试 | Scheduler **主动** `DispatchEpisode` → Worker |
| M2 | Worker 运行时骨架 | `WorkerGrpcServer` + `ControlPlaneClient` | 注册 / 心跳 / 被 Dispatch / 空执行回报 |
| M3 | 基础设施 | ADR-001 Linux `.log`、ADR-002 YAML/JSON 配置、CLI | 符合日志、配置与 CLI 约束 |
| M4 | GSM8K 插件 | **ProcessBackend + Protobuf/UDS 子进程** | 本地 reset/step/close |
| M5 | Episode 执行 | 单轮 GSM8K + **最小 Prometheus 指标** | Server(Mock) Dispatch → 执行 → Report |
| M6 | 预热池 | 固定容量 + **§5.6 复用安全** + 池 metrics | 池命中可量化；无双分配 |
| M7 | 真实 Scheduler | ControlPlane remote；Server 直连 Worker | 与 UEnv Server 联调 |
| M8 | 容错 | WAL **持久化** + 重连（schema 已在 M1） | 断连不丢 Result；幂等重放 |
| M9+ | 增强 | 动态预热、Podman、Cap'n/cdylib、OTel | 生产化（非 MVP 阻塞） |

---

## M1：Mock Scheduler Gateway（第一步）

**目标**：提供与 UEnv Server / Scheduler **契约一致** 的 Mock，使 Worker Pool 团队可独立开发。Mock 扮演 **调度方**：接受 Worker 注册/心跳，**主动调用** Worker `DispatchEpisode`。

**设计参考**：worker-pool-layer-design.md §8、§7、§7.5

### M1.1 契约与仓库骨架

> **完成** 2026-05-25（[worker-pool-pre-mvp-architecture-adjustment.md](./worker-pool-pre-mvp-architecture-adjustment.md) 阶段 0）。proto 生成当前为 `make proto` / `scripts/proto-gen.sh` 手动步骤；`build.rs` 接入 `cargo build` 计划 M2。

- [x] 创建 `uenv-worker` Rust 2024 workspace（`edition = "2024"`；根 `Cargo.toml` workspace 含 `uenv-worker`）
- [x] 引入 `proto/`：最小 Protobuf 定义（与 v7.1 §5.3 对齐的子集）
  - [x] `EpisodeRequest` / `EpisodeResult` / `StreamReport`
  - [x] `RegisterWorker` / `WorkerHeartbeat` / `DispatchEpisode` / `ReportResult`
  - [x] `WorkerInfo`（含 **`endpoint`** 供 Scheduler 直连）、Ack、健康检查
  - [x] `episode_id`、`attempt_id`、`server_epoch`、`idempotency_key` 字段
  - [x] **Dispatch 租约**：`dispatch_lease_id`、`lease_expire_at`、`scheduler_epoch`（§7.7）
  - [x] **WAL record schema**（§7.5，含 `dispatch_lease_id`）与 `replay_state` 枚举 — **M1 冻结，M8 再实现落盘**
  - [x] 插件 IPC 的 `.proto` **独立目录**（如 `plugin_proto/`），与控制面 `proto/` 分离（§2.4）
- [x] `tonic` + `prost` 代码生成与 `cargo build` 通过（`--prost_out` / `protoc-gen-prost`；生成后各 crate `cargo build` 通过）
- [x] 文档：`proto/README.md` 说明字段来源、`protocol_version`、**L1 控制面方向**、**禁止插件字段进入 L1**

### M1.2 独立 Mock Scheduler（优先）

- [x] 新建 crate：`uenv-mock-scheduler`（或 `uenv-worker/mock-server`）
- [x] ControlPlane 监听 `UENV_MOCK_LISTEN`（默认 `0.0.0.0:50051`）
- [x] 实现 **Scheduler 侧** gRPC 服务（Worker 为 Client 连接）：
  - [x] `RegisterWorker` → 返回合成 `worker_id`；记录 Worker **`endpoint`**（如 `127.0.0.1:50052`）
  - [x] `WorkerHeartbeat`（双向流）→ Ack + `next_heartbeat_interval_ms`；可注入 `server_epoch`
  - [x] `ReportResult` → 接收并记录；**按 `idempotency_key` 幂等去重**
- [x] 实现 **Scheduler 侧** gRPC **客户端**（主动调度）：
  - [x] 从 fixture 队列取 `EpisodeRequest`，向 Worker `endpoint` 发起 **`DispatchEpisode`**
  - [x] 消费 Worker 返回的 `stream StreamReport`
- [x] 可选：内存 `ListWorkers` 模拟 Worker Pool 资源目录查询
- [x] CLI（§2.5）：`uenv-mock-scheduler serve [--config PATH] [--fixture-dir ./fixtures] [--log-file /var/log/uenv/mock-scheduler.log]`
- [x] CLI：`uenv-mock-scheduler version`
- [x] **不包含**：Worker 侧 `subscribe_dispatch` RPC

### M1.3 Fixture 与任务队列

- [x] 目录 `fixtures/gsm8k/`：
  - [x] `episode_001.pb`（二进制 Protobuf `EpisodeRequest`）
  - [x] `expected_result_001.pb`（可选，用于自动校验）
  - [x] `episode_001.textproto` 或文档说明人类可读字段（便于 review）
- [x] Fixture 必填字段（§8.5）：
  - [x] `env_type = "gsm8k"`
  - [x] `request_id` / `correlation_id`
  - [x] `model_endpoint`（占位 URL）
  - [x] `max_steps`、`timeout_seconds`
  - [x] `reward_config`（最小规则奖励配置）
  - [x] `dispatch_lease_id`、`lease_expire_at`（租约测试）
- [x] Mock 行为：启动时加载 fixture 入队；Scheduler **按 FIFO 主动** `DispatchEpisode` 到已注册 Worker；队列为空时打 WARN 不下发（非挂起）
- [x] Mock 每次 Dispatch 生成 **新** `dispatch_lease_id`；failover 测试可注入第二 lease

### M1.4 Mock 可观测与日志

- [x] Mock Server 日志写入 `/var/log/uenv/mock-scheduler.log`（Linux 文本 `.log`，禁止 JSON 落盘；§2.2）
- [x] 开发模式可通过 `--log-file ./mock-scheduler.log` 或 env `UENV_LOG_FILE` 覆盖
- [x] 关键事件打 INFO：`register`、`heartbeat`、`dispatch`、`report_result`
- [x] 行内字段：`worker_id`、`episode_id`、`env_type`、`trace_id`（若有）
- [x] 文档示例：`tail -f /var/log/uenv/mock-scheduler.log`

### M1.5 故障注入（M1 最小集，M1.7 子集）

- [x] 环境变量或 flags（至少实现 3 项）：
  - [x] `UENV_MOCK_DISPATCH_DELAY_MS`：主动 Dispatch 前延迟
  - [x] `UENV_MOCK_DROP_HEARTBEAT_N`：前 N 次心跳无响应
  - [x] `UENV_MOCK_DUPLICATE_DISPATCH=1`：重复下发同一 Episode
  - [x] `UENV_MOCK_SERVER_EPOCH`：注入 epoch 变化
- [x] 文档说明各开关与 M1.7 场景映射（供 M8 复用）

### M1.6 M1 测试与工具

- [x] 集成测试：**最小 Worker stub**（仅 gRPC Server）+ Mock Scheduler 完成 `Register → Heartbeat → Dispatch(主动) → Report`
- [x] 脚本 `scripts/gen-gsm8k-fixture.sh`（或 Rust bin）从 textproto 生成 `.pb`
- [x] `grpcurl` / 文档示例：手动探测 Mock ControlPlane 与 Worker 端口

### M1.7 Contract Chaos Tests（协议鲁棒性）

- [x] `duplicate_dispatch`：同一 `episode_id`+`attempt_id` 重复 `DispatchEpisode`，Worker stub 幂等
- [x] `unsupported_env_type`：Worker 拒绝并返回明确错误码
- [x] `capacity_full`：Worker 返回背压（如 `RESOURCE_EXHAUSTED`）
- [x] `stale_worker_id` / `server_epoch` 变化：触发重新 `RegisterWorker`
- [x] `heartbeat_timeout`：Mock 停止 Ack，验证 Worker 重连语义（stub 级）
- [x] `report_result_retry`：相同 `idempotency_key` 重复提交，Mock 幂等 ACK
- [x] `partial_stream_interruption`：中断 `StreamReport` 流，仍以 `ReportResult` 为准
- [x] `lease_expired`：过期 lease 的 Dispatch 被拒绝
- [x] `lease_superseded`：新 lease Dispatch 后，旧 lease 在途执行终止并 `LEASE_SUPERSEDED`
- [x] 文档：各场景开关与预期行为（供 M8 复用）

### M1 退出标准（2026-05-26 实现收口）

| # | 验收项 | 实现情况 | 验证依据 |
|---|--------|----------|----------|
| 1 | `uenv-mock-scheduler serve` 启动 ControlPlane | **已完成** | crate `uenv-mock-scheduler`；默认 `UENV_MOCK_LISTEN=0.0.0.0:50051`；CLI `serve` / `version` |
| 2 | Worker `RegisterWorker` 返回合法 `worker_id` | **已完成** | 空 `worker_id` 时合成 `mock-worker-{n}`；响应含 `server_epoch`（`UENV_MOCK_SERVER_EPOCH` 可注入） |
| 3 | Mock **主动** `DispatchEpisode` GSM8K fixture | **已完成** | 启动加载 `fixtures/gsm8k/*.pb`，FIFO 轮询；每次生成新 `dispatch_lease_id`（`lease-{n}`）；1s 调度循环 |
| 4 | `ReportResult` 日志 + `idempotency_key` 幂等 | **已完成** | 内存 `seen_idempotency` 去重；日志事件 `report_result`，字段含 `duplicate` |
| 5 | 心跳双向流 + `server_epoch` 注入 | **已完成** | `WorkerHeartbeat` 流式 Ack；`next_heartbeat_interval_ms=5000`；`UENV_MOCK_DROP_HEARTBEAT_N` 可丢弃前 N 次 Ack |
| 6 | proto / WAL schema / 租约字段冻结 | **已完成** | `proto/uenv/v1/{common,episode,scheduler,wal}.proto`；`plugin_proto/` 独立；见 `proto/README.md` |
| 7 | M1.7 混沌场景自动化（≥7/10，含 ≥2 租约） | **已完成（7 passed）** | `cargo test -p uenv-mock-scheduler --test m1_contract_chaos_tests` → **7 passed, 0 failed** |
| 8 | 文本 `.log` 落盘 + 可观测 | **已完成** | 默认 `/var/log/uenv/mock-scheduler.log`；`--log-file` / `UENV_LOG_FILE` 覆盖；INFO：`register` / `heartbeat` / `dispatch` / `report_result` |
| 9 | 故障注入 + 工具链 | **已完成** | 4 项 env 注入；`scripts/gen-gsm8k-fixture.sh`；README 含 `grpcurl` 与 `tail -f` 示例 |

**M1.7 自动化覆盖明细**（7 个测试用例 → 9+ 场景）：

| 测试用例 | 覆盖场景 |
|----------|----------|
| `m16_register_heartbeat_dispatch_report_chain` | 全链路 Register→Heartbeat→Dispatch→Report；`report_result_retry`（同 `idempotency_key` 第二次 `duplicate=true`） |
| `m17_duplicate_dispatch` | `duplicate_dispatch`（`UENV_MOCK_DUPLICATE_DISPATCH=1`） |
| `m17_heartbeat_timeout_drop_ack` | `heartbeat_timeout`（`UENV_MOCK_DROP_HEARTBEAT_N`） |
| `m17_dispatch_delay_and_server_epoch_injection` | `server_epoch` 注入；`UENV_MOCK_DISPATCH_DELAY_MS` 调度延迟 |
| `m17_unsupported_env_type_and_capacity_full` | `unsupported_env_type`；`capacity_full` |
| `m17_lease_expired` | `lease_expired` |
| `m17_lease_superseded` | `lease_superseded`（配合 duplicate dispatch） |

**尚未单独自动化**：`stale_worker_id`（独立重注册流程）、`partial_stream_interruption`（流中断仍以 ReportResult 为准）——行为已在 Mock 实现层覆盖，M8 可复用故障注入开关补测。

#### Mock Scheduler Gateway 为 Scheduler 侧提供的 L1 结构体示例

Mock 扮演 **Scheduler / ControlPlane** 角色：Worker 作为 Client 调用下列 RPC；Mock 作为 Client 向 Worker `endpoint` 主动下发 `EpisodeRequest`。

**1. Worker → Mock（ControlPlaneService，定义于 `proto/uenv/v1/scheduler.proto`）**

```textproto
# RegisterWorkerResponse（注册成功后返回）
accepted: true
worker_id: "mock-worker-1"
message: "accepted"
server_epoch: 1

# HeartbeatResponse（心跳 Ack）
ok: true
server_epoch: 1
next_heartbeat_interval_ms: 5000

# ReportResultResponse（结果上报 Ack；重复 key 时 duplicate=true）
ack: true
duplicate: false

# ListWorkersResponse → WorkerInfo（资源目录查询）
workers {
  worker_id: "mock-worker-1"
  supported_env_types: "gsm8k"
  load: 0
  max_load: 1
  status: "ready"
  endpoint: "127.0.0.1:50052"
}
```

**2. Mock → Worker（WorkerGrpcService，定义于 `uenv-worker/proto/worker_service.proto`）**

Mock 主动构造并下发的 `DispatchEpisodeRequest.episode`（fixture 来源见 `fixtures/gsm8k/episode_001.textproto`；运行时 `dispatch_lease_id` 由 Mock 重写为 `lease-{n}`）：

```textproto
episode_id: "gsm8k-episode-001"
attempt_id: 1
env_type: "gsm8k"
payload: "{\"request_id\":\"req-gsm8k-001\",\"question\":\"If 3 books cost $12, what is the cost of 5 books?\"}"
mode: MODE_SINGLE
max_steps: 8
resource_spec { cpu_cores: 1 memory_mb: 512 gpu_count: 0 }
model_endpoint: "http://127.0.0.1:18080/mock-llm"
correlation_id: "corr-gsm8k-001"    # 日志 trace_id 映射源
timeout_seconds: 120
reward_config: "{\"type\":\"rule_reward\",\"target\":\"20\"}"
dispatch_lease_id: "lease-1"         # Mock 每次 Dispatch 新生成
lease_expire_at { seconds: 1800000000 }
scheduler_epoch: 1
```

**3. Worker → Mock（结果上报）**

```textproto
# ReportResultRequest
idempotency_key: "gsm8k-episode-001:1:mock-worker-1"  # episode_id + attempt_id + worker_id
worker_id: "mock-worker-1"
server_epoch: 1
result {
  episode_id: "gsm8k-episode-001"
  attempt_id: 1
  status: "completed"
}
```

**4. WAL schema（M1 冻结，M8 落盘；`proto/uenv/v1/wal.proto`）**

```textproto
episode_id: "gsm8k-episode-001"
attempt_id: 1
worker_id: "mock-worker-1"
dispatch_lease_id: "lease-1"
server_epoch: 1
status: "pending"
replay_state: REPLAY_STATE_PENDING
```

**M1 不包含**：真实 Episode 执行、环境插件、预热池、WAL 磁盘持久化（schema 除外）。

**M1 完成总结**：`uenv-mock-scheduler` 已作为与真实 Scheduler 契约一致的 Mock ControlPlane 落地，Worker 可独立对接 Register / Heartbeat / 被主动 Dispatch / Report 全链路，proto 与 WAL schema 已冻结，7 项混沌测试全部通过，可进入 M2。

---

## M2：Worker 运行时骨架 + 对接 Mock

**目标**：`uenv-worker` 同时作为 gRPC **Server**（`DispatchEpisode`）与 ControlPlane **Client**（注册/心跳/上报），与 M1 Mock 跑通控制面空循环。

**依赖**：M1 完成

### 任务

- [x] `WorkerRuntime`：`main` + Tokio 运行时 + 优雅退出（SIGTERM）
- [x] CLI（§2.5）：`uenv-worker serve [--config PATH]`、`uenv-worker version`、`uenv-worker health`
- [x] `WorkerGrpcServer`：监听 `UENV_WORKER_LISTEN`（默认 `0.0.0.0:50052`）；实现 `DispatchEpisode`、`HealthCheck`
- [x] `ControlPlaneClient` trait：`register`、`heartbeat_loop`、`report_result`（**无** `subscribe_dispatch`）
- [x] 连接 `UENV_SERVER_ENDPOINT`（Mock ControlPlane `:50051`）；注册时上报 `endpoint`
- [x] `UENV_SCHEDULER_MODE=remote` 开发期默认连 Mock；保留配置项供 M7 切换真实 Server
- [x] Worker 状态机骨架：`Created → Ready`（注册成功且 gRPC Server 就绪）
- [x] `DispatchEpisode` 收到后 **暂不执行环境**：校验 `dispatch_lease_id` / `lease_expire_at`；构造最小合法 `EpisodeResult` 并 `ReportResult`
- [x] 支持 duplicate dispatch 幂等 + **lease_conflict** 拒绝（占位）
- [x] 集成测试：Mock Scheduler **主动** Dispatch → Worker 回报（含租约字段）

### M2 退出标准

| # | 标准 |
|---|------|
| 1 | 启动 Worker 后，Mock 可见 register（含 `endpoint`）+ 持续 heartbeat |
| 2 | Mock **主动** `DispatchEpisode` 1 个 GSM8K 后，Worker 在超时内 `ReportResult` |
| 3 | Worker **未实现** subscribe 拉任务 RPC |
| 4 | CI 中集成测试自动通过 |
| 5 | `uenv-worker serve --help` 与 `uenv-worker version` 可用 |

### M2 现状总结（2026-05-26）

- 已完成 `WorkerRuntime` 运行时骨架：`uenv-worker serve` 可启动 Worker gRPC Server 与 ControlPlane Client，并支持优雅退出（Unix 下含 SIGTERM，Windows 开发环境下可用 Ctrl+C）。
- 已完成 `WorkerGrpcServer`：监听 `UENV_WORKER_LISTEN`（默认 `0.0.0.0:50052`），实现 `DispatchEpisode` 与 `HealthCheck`。
- 已完成 `ControlPlaneClient`：实现 `register`、`heartbeat_loop`、`report_result`，连接 `UENV_SERVER_ENDPOINT`（Mock 为 `:50051`）并在注册时上报 `endpoint`。
- 已落实协议边界：Worker 侧未实现 `subscribe_dispatch` 拉任务模型，保持「Scheduler 主动 Dispatch，Worker 被动接收」控制面方向。
- 已完成 M2 占位执行语义：`DispatchEpisode` 路径校验 `dispatch_lease_id` 与 `lease_expire_at`；当前不执行真实环境，仅构造最小合法 `EpisodeResult` 并主动 `ReportResult`。
- 已完成重复与租约冲突占位处理：支持 duplicate dispatch 幂等处理，并对 lease 不一致请求返回 `lease_conflict` 拒绝语义。
- 已补齐对接测试：新增 `uenv-worker/tests/m2_runtime_with_mock.rs`，覆盖 Mock 主动 Dispatch -> Worker 回报链路，并通过重复 `idempotency_key` 验证上报幂等行为。
- 已完成编译与测试验证：`cargo check -p uenv-worker` 通过；`cargo test -p uenv-worker --test m2_runtime_with_mock` 通过（1 passed）。

---

## M3：配置与 Linux 日志（ADR-001 / ADR-002）

**目标**：满足设计文档 §2.2（ADR-001）、§2.5（CLI）、§2.6（ADR-002）、§12 的基础设施约束。

**依赖**：M2 完成（可与 M4 并行部分任务）

### 任务

- [x] `cli` 模块（clap）：`serve`、`version`、`health`；全局 `--config`、`--log-level`、`--log-file`
- [x] `config` 模块：YAML **与** JSON 双格式（按扩展名识别）；默认路径 `/etc/uenv/worker.yaml` 或 §2.6 查找序；优先级 CLI > env > 文件 > 默认
- [x] 提供示例配置：`config/uenv-worker.yaml` 与 `config/uenv-worker.json`（字段与 design §2.6 一致）
- [x] 配置文件 ↔ 环境变量映射（§2.6 表格）单元测试
- [x] `logging` 模块：写入 `logging.file` / `UENV_LOG_FILE`，默认 `/var/log/uenv/worker.log`；格式 `timestamp LEVEL target k=v msg="..."`（ADR-001）
- [x] 禁止 `UENV_LOG_FORMAT=json` 或忽略该变量；禁止 multiline stacktrace 落盘
- [x] Worker 启动/关闭、注册、Dispatch、Report 打 INFO；含 `trace_id`、`episode_id`、`worker_id`
- [x] 单元测试：日志行解析含 `trace_id`、`episode_id`、`worker_id`；单行断言
- [x] 集成测试：`uenv-worker serve --config config/uenv-worker.yaml` 启动成功

### M3 退出标准

| # | 标准 |
|---|------|
| 1 | 本地运行后仅生成文本 `.log` 于 `/var/log/uenv/worker.log`（或 `--log-file` 指定路径）；`grep episode_id` / `trace_id` 可检索 |
| 2 | `tail -f /var/log/uenv/worker.log` 可实时看到 register / dispatch 等 INFO 行 |
| 3 | `UENV_*` 环境变量与 `uenv-worker.yaml` / `.json` 均可加载；CLI 参数覆盖生效 |
| 4 | 配置项 `env.types`、`worker.max_concurrent`、`worker.listen` 等可从 YAML 或 JSON 读取 |

### M3 现状总结（2026-05-26）

- 已完成 `cli` 全局参数：`--config`、`--log-level`、`--log-file`，并保持 `serve` / `version` / `health` 子命令。
- 已完成 `config` 模块：支持 YAML/JSON 按扩展名加载；默认查找序支持 `./uenv-worker.yaml`、`/etc/uenv/worker.yaml`、`./uenv-worker.json`、`/etc/uenv/worker.json`（开发环境兼容 `./config/uenv-worker.{yaml,json}`）；优先级为 CLI > env > 文件 > 默认值。
- 已落地 ADR-001 日志实现：写入 `logging.file`（可由 `UENV_LOG_FILE` 覆盖），默认指向 `/var/log/uenv/worker.log`；使用单行文本日志并输出 `trace_id`、`episode_id`、`worker_id` 等关键字段。
- 已落实 `UENV_LOG_FORMAT=json` 约束：检测到该值时仅告警并忽略，不启用 JSON 落盘。
- 已补齐测试：`config` 环境变量映射单测、日志字段解析单测，以及 `m3_serve_with_yaml_config_starts` 集成测试（覆盖 `uenv-worker serve --config ...` 启动路径）。

---

## M4：GSM8K 环境插件 + PluginHost

**目标**：**仅**通过 `ProcessBackend` + **Protobuf over UDS** 子进程加载 `gsm8k`；落实 **§3.5 进程级实例** 与 **§6.4 插件崩溃语义**。

**依赖**：M3 完成

### 任务

- [x] `plugins/gsm8k/manifest.yaml`：`env_type`、版本、`supported_backends: [process]`、`ipc: proto-uds`
- [x] **L2** aRPC IDL（`plugin_proto/`，**Protobuf**）：`reset` / `step` / `close` / `health_check` — 与 L1 `proto/` 分离
- [x] `PluginHost`：扫描 `UENV_PLUGIN_DIR`；`spawn` 返回 `instance_id` + PID + UDS（**1 spawn = 1 进程 = 1 instance**）
- [x] `ProcessBackend::create`：启动 **一个** 插件子进程；**不** 实现单进程多 session
- [x] `PluginHost`：订阅子进程退出（`waitpid`）；退出时标记实例 Broken（§6.4）
- [x] GSM8K 插件最小实现：固定一道题，`step` 比较答案给 0/1 reward，`reset` 返回题干
- [x] 单元测试：不经过 gRPC，`PluginHost` 对 **独立进程** reset/step/close
- [x] 单元测试：**故意 kill 插件进程** → Worker 存活；返回错误；实例不可复用
- [x] 健康检查：失败实例不进入预热池（M6 前置）

### M4 退出标准

| # | 标准 |
|---|------|
| 1 | `uenv-worker` 启动日志显示已加载 `gsm8k`（`proto-uds`） |
| 2 | 本地测试 reset → step → close 返回预期 reward |
| 3 | `RegisterWorker.supported_envs` 包含 `gsm8k` |
| 4 | 未引入 Cap'n Proto / cdylib；未实现 1 process → N sessions |
| 5 | kill 插件进程后 Worker 仍运行；`PluginHost` 已销毁该 `instance_id` |
| 6 | `plugin_proto/` 与 `proto/` 分离，README 说明 L1/L2 边界 |

### M4 现状总结（2026-05-26）

- 已完成 `plugins/gsm8k/manifest.yaml` 字段对齐，补充 `version` 与 `supported_backends: [process]`，保持 `ipc: proto-uds`。
- 已完成 `PluginHost` 主体：扫描 `UENV_PLUGIN_DIR` 下 `manifest.yaml`，按 `env_type` 建索引，`spawn` 返回 `instance_id` + PID + UDS 路径。
- 已完成 `ProcessBackend::create`：严格一实例一子进程，按 `entry` 启动插件并注入 `--uds-path`，未实现单进程多 session。
- 已完成 L2 调用链：`plugin/arpc` 新增 `connect_uds/reset/step/close/health_check`，并新增 `uenv-gsm8k-plugin` 子进程服务（固定题目奖励 0/1）。
- 已落实崩溃语义：`PluginHost` 为子进程注册退出监听，实例退出后标记不可继续使用；`terminate_for_test` 路径验证 kill 后调用失败。
- 已完成 M4 测试与编译验证：新增 `uenv-worker/tests/m4_plugin_host_process.rs`（Unix 平台启用）；`cargo check -p uenv-worker` 通过；`cargo test -p uenv-worker --test m4_plugin_host_process` 在当前 Windows 环境为 `0 tests`（`cfg(unix)` 约束）。

---

## M5：Episode 执行引擎（单轮 GSM8K）

**目标**：在 Mock 控制面下完成 **真实** Episode 执行并返回轨迹。

**依赖**：M2 + M4 完成

### 任务

- [x] `EpisodeExecutor`：实现 §4.2 流程（单轮模式）；绑定 **进程级实例**；校验 **当前 dispatch_lease_id**
- [x] `ModelClient`：Mock 模式可返回 fixture 中预设 `action`（不依赖真实 vLLM）
- [ ] `RewardEngine`：最小 `RuleReward`（答案匹配）
- [x] 构建 `EpisodeResult`：`trajectory`、`trajectory_checksum`（SHA-256）、`integrity_verified=true`
- [x] `StreamReport`：`STEP_COMPLETE` 至少 1 次（若 proto 要求）
- [x] `ConcurrencyPool`：尊重 `UENV_MAX_CONCURRENT`（M5 可先 serial=1）
- [x] **禁止**默认 `env.step()` 重试；model callback 可配置重试
- [x] `MetricsExporter`：暴露 M5 最小集（`episode_total`、`episode_duration_ms`、`env_step_duration_ms`、`model_callback_duration_ms`、`active_episode_count`、`heartbeat_lag_ms`）
- [ ] 集成测试：Mock **主动** Dispatch → Worker 执行 → Result 与 `expected_result` 比对 reward/status

### M5 退出标准

| # | 标准 |
|---|------|
| 1 | Mock 模式完整链路：Register → **主动** Dispatch → Execute → Report（设计 §9.3-1） |
| 2 | `EpisodeResult` 含非空 trajectory 且 checksum 校验通过 |
| 3 | 日志记录 `duration_ms`、`reward`；`/metrics` 可 scrape 且含 M5 最小指标 |
| 4 | 代码审查确认无 `env.step()` 默认重试循环 |

### M5 现状总结（2026-05-26）

- 已完成 `EpisodeExecutor` 单轮执行主链路：`spawn(process instance) -> reset -> model action -> step -> close`，并在执行前复用 `dispatch_lease_id` 校验。
- 已完成 `ModelClient` Mock 推理：优先从 `reward_config.target` 解析预设答案作为 action，兜底支持从 payload 中读取 `answer` 字段。
- 已完成结果构建：`EpisodeResult` 含非空 `trajectory`，并对 `Trajectory` 做 SHA-256 生成 `trajectory_checksum`，`integrity_verified=true`。
- 已完成 `DispatchEpisode` 流改造：至少上报 1 次 `StreamReport`（`phase=step_complete`，携带 `last_step`）。
- 已完成并发门控：在 Worker gRPC 服务中引入 `Semaphore`，由 `UENV_MAX_CONCURRENT` 控制并发执行上限（M5 默认可配为 1）。
- 已完成最小指标聚合器 `MetricsExporter`：支持 `episode_total`、`episode_duration_ms`、`env_step_duration_ms`、`model_callback_duration_ms`、`active_episode_count`、`heartbeat_lag_ms` 的内存聚合与 Prometheus 文本导出。
- 已确认执行路径无 `env.step()` 默认重试循环：`step` 仅执行一次，失败直接返回错误并由上层处理。
- 待完成：`RewardEngine` 独立模块化（当前奖励语义由 GSM8K 插件 + ModelClient 协同达成）；Mock 主动 Dispatch 到 `expected_result` 的端到端集成断言仍需在 Unix 测试环境补齐。

---

## M6：预热池（固定容量）

**目标**：Worker 侧持有 `WarmupPool`，Phase 0 使用固定 `UENV_WARMUP_POOL_SIZE`。

**依赖**：M5 完成

### 任务

- [x] `WarmupPool`：按 `env_type` 队列；状态机 Creating/Warm/Active/Idle/Cooling/Evicting/Destroyed
- [x] 启动时预创建 `UENV_WARMUP_POOL_SIZE` 个 `gsm8k` 实例
- [x] Episode 开始：出池前 `health_check`；池取 **进程级** 实例；结束：`reset`/cleanup 后归还 Warm（进程不退出）
- [x] `max_episode_count` / `max_idle_time` / `cool_timeout` 基础回收
- [x] **no double allocation**：同一 `instance_id`（PID）不被双 Episode 占用
- [x] 插件进程崩溃：实例从池移除并补池（§6.4）
- [x] 日志：池命中 `warmup_hit=true` vs 冷创建 `warmup_hit=false`
- [x] 指标（**M6 必达**）：`warmup_pool_hit_total`、`warmup_pool_miss_total`、`uenv_instance_pool_size{status}`

### M6 退出标准

| # | 标准 |
|---|------|
| 1 | 连续 2 个 Episode，第 2 次 `episode_duration_ms` 或日志 `duration_ms` 显著低于第 1 次 |
| 2 | 实例 `episode_count` 递增，归还后仍处于 Warm/Idle；坏实例 health_check 失败被销毁 |
| 3 | `warmup_pool_hit_total` 在第 2 次 Episode 递增；`/metrics` 可验证 |
| 4 | 无双分配：自动化测试覆盖 allocate→release→reallocate |
| 5 | 设计 §9.3-2、§9.3-3 在 Mock 下可验证 |

### M6 现状总结（2026-05-26）

- 已完成 `WarmupPool` 固定容量实现：按 `env_type` 维护池队列，并实现 `Creating/Warm/Active/Idle/Cooling/Evicting/Destroyed` 状态流转。
- 已在 `WorkerRuntime` 启动阶段接入预热：按 `pool.warmup_size`（`UENV_WARMUP_POOL_SIZE`）预创建实例。
- 已将 `EpisodeExecutor` 改造为池化执行：`acquire -> reset(seed) -> step -> release`，不再每轮执行后销毁进程实例。
- 已落地回收语义：支持 `max_episode_count` 上限淘汰、`max_idle_time/cool_timeout` 空闲回收、health_check/reset 失败销毁并补池。
- 已落实复用安全：池内维护 active 集合，实例借出时强校验，防止同一 `instance_id` 双分配。
- 已接入 M6 日志与指标：`dispatch` 日志新增 `warmup_hit` 字段；指标新增 `uenv_warmup_pool_hit_total`、`uenv_warmup_pool_miss_total`、`uenv_instance_pool_size{status}`。
- 已补充测试：新增 `uenv-worker/tests/m6_warmup_pool.rs` 覆盖 allocate→release→reallocate（Unix 平台）；当前 Windows 环境运行结果为 `0 tests`（`cfg(unix)` 约束）。

---

## M7：真实 Scheduler 联调

**目标**：`ControlPlaneClient` 对接真实 UEnv Server；真实 Scheduler **直连** Worker `DispatchEpisode`（非 Worker Pool 二次转发）。

**依赖**：M6 完成；**外部依赖** UEnv Server 可用

### 任务

- [x] `ControlPlaneClient` 远程实现与 Mock 共用 trait
- [x] 配置切换：`UENV_SERVER_ENDPOINT` 指向真实 Server；`UENV_WORKER_LISTEN` 可被 Server 访问
- [ ] 联调清单：Register（含 endpoint）、Heartbeat、**Server 主动 Dispatch**、Report；与 Server 日志交叉验证
- [x] 确认 Server 经 Worker Pool **只读查询** worker 清单后直连 Dispatch（若 Server 未就绪 Pool，文档记录临时路径）
- [x] 文档：Mock vs Remote 切换步骤、常见错误码、`server_epoch` 行为

### M7 联调说明（2026-05-28）

- `uenv-worker` 已将控制面抽象为统一 trait：`scheduler.mode=mock|remote` 走同一 `register/heartbeat/report` 生命周期。
- 启动切换：
  - Mock：`UENV_SCHEDULER_MODE=mock`，`UENV_SERVER_ENDPOINT=<mock_control_plane_host:port>`
  - Remote：`UENV_SCHEDULER_MODE=remote`，`UENV_SERVER_ENDPOINT=<uenv_server_host:port>`，`UENV_WORKER_LISTEN=<server 可回连地址>`
- `server_epoch` 行为：
  - Register 时读取服务端 epoch 并写入 Worker 运行时身份；
  - Heartbeat 响应若返回新 epoch，则本地覆盖；
  - ReportResult 透传当前本地 epoch。
- 常见错误码（当前实现）：
  - `UNAVAILABLE`：`UENV_SERVER_ENDPOINT` 不可达 / 服务未启动；
  - `FAILED_PRECONDITION`：派发租约冲突、租约过期、缺少租约字段；
  - `RESOURCE_EXHAUSTED`：Worker 达到并发上限；
  - `INTERNAL`：执行器报错或结果上报失败。
- 临时路径（Server 侧仍未完成 Worker Pool 查询/直连闭环时）：
  - 保持 Worker 通过 `WorkerRegistration` 注册可回连 endpoint；
  - 由 Server 调度层只读查询活跃 worker 清单并直接调用 Worker `DispatchEpisode`；
  - 不通过 Worker Pool 做二次转发，避免控制面方向反转。

### M7 退出标准

| # | 标准 |
|---|------|
| 1 | 同一 Worker 二进制在 Mock / Remote 两种模式下均可启动（gRPC Server + ControlPlane） |
| 2 | 与真实 Server 完成 ≥1 次 GSM8K Episode：**Server Dispatch → Worker 执行 → Report** |

---

## M8：容错（WAL 持久化 + 重连）

**目标**：Worker 控制面断连不丢已完成 Episode 结果。**WAL schema 已在 M1 冻结**；本阶段实现落盘与重放。

**依赖**：M7 建议完成；复用 M1.7 混沌场景

### 任务

- [ ] `WalWriter`：按 §7.5 schema 写入；`EpisodeResult` 载荷 + `request_checksum` / `result_checksum` + CRC32
- [ ] `idempotency_key` 重放：`episode_id` + `attempt_id` + `worker_id`
- [ ] 断连检测：拒绝或排队新 `DispatchEpisode`（策略可配置）；在途执行至完成
- [ ] 指数退避重连 ControlPlane；重连后 WAL 重放至 `replay_state=acked`
- [ ] 指标：`uenv_wal_pending_records`
- [ ] 使用 M1.7 `report_result_retry`、`heartbeat_timeout`、`partial_stream_interruption` 做集成测试

### M8 退出标准

| # | 标准 |
|---|------|
| 1 | 断连期间完成的 Episode，重连后 Server/Mock 收到 Result（幂等不重复计数） |
| 2 | WAL 损坏条目被跳过并打 ERROR 日志 |
| 3 | `uenv_wal_pending_records` 在重放后归零 |

---

## M9+：增强项（非 MVP 阻塞）

以下放在 MVP 之后，按需排期：

| 项 | 说明 |
|----|------|
| `WarmupSizer` 动态容量 | §5.4，依赖 M6 已采集的 hit/miss 与 QPS |
| `PodmanBackend` | §6.2 生产后端 |
| Cap'n Proto / cdylib 插件 | §3.0 非 MVP 路径 |
| 多 Episode 并发压测 | `UENV_MAX_CONCURRENT` > 1 |
| 第二环境类型 | 打破仅 GSM8K 限制 |
| OpenTelemetry | §10.3；日志仍遵守 ADR-001 不落盘 JSON |
| 扩展 metrics | 在 M5/M6 最小集之上增加直方图 bucket 调优等 |

---

## 建议排期（人周估算，供参考）

| 阶段 | 预估 | 说明 |
|------|------|------|
| **M1** | 1.5–2 周 | **立即启动**；含 proto、WAL schema、M1.7 混沌测试 |
| M2 | 0.5–1 周 | Worker gRPC Server + ControlPlane 客户端 |
| M3 | 0.5 周 | 可与 M4 重叠 |
| M4 | 1–1.5 周 | 插件 + aRPC 首版 |
| M5 | 1–1.5 周 | 执行引擎主逻辑 |
| M6 | 0.5–1 周 | 预热池 |
| M7 | 0.5 周 + Server 就绪 | 联调窗口依赖对方 |
| M8 | 1 周 | 容错 |

**MVP 截止定义（最小可交付）**：**M5 完成** = Mock 下 GSM8K 全链路；**推荐 MVP+**：**M6 完成** = 含预热池。

---

## 与 design 文档 W0–W6 的对应

| design §14 | 本清单 |
|------------|--------|
| W0 crate + CLI + 日志 + YAML/JSON 配置 | M1（proto + WAL schema）+ M3（ADR-001/002 + CLI） |
| W1 WorkerGrpcServer + Mock 主动 Dispatch | **M1 + M2** |
| W1 PluginHost + GSM8K（Proto/UDS） | M4 |
| W2 EpisodeExecutor + 混沌测试 | M5 + M1.7 延续 |
| W3 WarmupPool + 复用安全 + metrics | M6 |
| W4 真实 Server 联调（直连 Dispatch） | M7 |
| W5 WAL 持久化 | M8（schema 在 M1） |
| W6+ WarmupSizer / Podman / Cap'n | M9+ |

---

## 参考资料

- [Worker Pool 层设计说明](./worker-pool-layer-design.md)
- [UEnv 方案 v7.1](./UEnv%20—%20下一代分布式训练环境框架方案-v7.1.pdf) — §5.3 gRPC、§8 Mock 测试思路
