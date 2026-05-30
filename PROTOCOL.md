# UEnv 通信协议与数据结构规范

> **版本**：v1（2026-05-30）  
> **依据**：[Docs/uenv-design-prd-v7.2.md](Docs/uenv-design-prd-v7.2.md) §4、[Docs/worker-pool-layer-design.md](Docs/worker-pool-layer-design.md) §7  
> **Proto 权威路径**：[proto/](proto/)（L1）、[uenv-worker/proto/](uenv-worker/proto/)（Worker gRPC）、[plugin_proto/](plugin_proto/)（L2）

本文档描述当前仓库 **已统一并落地** 的 gRPC 服务边界、消息结构与层间约束。实现以 `proto/` 目录为准；若代码与本文冲突，以 `proto/` 为准并提 PR 修正实现。

---

## 1. 架构与链路

```
训练框架 (Python)
    │  EpisodeRequest / EpisodeResult
    ▼
uenv-bridge (Adapter) ──gRPC──► uenv-server (UEnvService)
                                    │
                    ControlPlaneService ◄──► uenv-worker (Client)
                    WorkerGrpcService   ──► uenv-worker (Server)
                                    │
                                    ▼
                              plugins/* (L2 UDS)
```

| 链路 | 协议 | 序列化 | 说明 |
|------|------|--------|------|
| Bridge ↔ Server | gRPC `UEnvService` | Protobuf | 训练侧提交 Episode |
| Worker ↔ Server/Mock | gRPC `ControlPlaneService` | Protobuf | 注册、心跳、结果上报 |
| Server/Mock → Worker | gRPC `WorkerGrpcService` | Protobuf | **主动** `DispatchEpisode` |
| Worker ↔ 插件 | Protobuf over UDS | Protobuf | L2，与 L1 隔离 |
| Hub ↔ CLI/Server | HTTP REST | JSON | 环境元数据，非 Episode 热路径 |
| Worker ↔ 推理服务 | HTTP/gRPC | 框架自定 | 不经 Server |

**冻结原则**（PRD §4.1）：

- 不使用 Redis/Kafka 等消息中间件做 Episode 转发
- 控制面与数据面均使用 **gRPC + Protobuf**，训练热路径不用 JSON
- Scheduler **主动**调用 Worker `DispatchEpisode`；Worker **禁止** `subscribe_dispatch` 拉任务

---

## 2. Proto 文件与 Package

### 2.1 L1 共享（`proto/uenv/v1/`）

| 文件 | Package | 内容 |
|------|---------|------|
| `common.proto` | `uenv.v1` | `ErrorCode`、`ResourceSpec`、`ExecutionMode` |
| `episode.proto` | `uenv.v1` | `EpisodeRequest`、`EpisodeResult`、`Trajectory`、`StreamReport` |
| `wal.proto` | `uenv.v1` | `WalRecord`、`ReplayState`（Worker WAL，§7.5 冻结） |
| `scheduler.proto` | `uenv.scheduler.v1` | `ControlPlaneService` 及注册/心跳/上报消息 |
| `server.proto` | `uenv.v1` | `UEnvService`、`AdminService` 及批量/运维消息 |

### 2.2 Worker gRPC Server（`uenv-worker/proto/worker_service.proto`）

| Package | Service | 方法 |
|---------|---------|------|
| `uenv.worker.v1` | `WorkerGrpcService` | `DispatchEpisode`、`HealthCheck` |

### 2.3 L2 插件（`plugin_proto/uenv/plugin/v1/plugin.proto`）

| Package | 说明 |
|---------|------|
| `uenv.plugin.v1` | `reset` / `step` / `close` / `health_check`；**不得**被 L1 crate import |

---

## 3. gRPC Service 规范

### 3.1 UEnvService（训练侧 → Server）

**定义**：`proto/uenv/v1/server.proto`  
**实现**：`uenv-server`  
**PRD 对照**：§4.2 UEnvService

| RPC | 模式 | 状态 |
|-----|------|------|
| `SubmitEpisode` | unary | ✅ 已实现 |
| `SubmitEpisodeStream` | bidi stream | Phase 2+（unimplemented） |
| `SubmitBatch` | unary | Phase 2+（unimplemented） |
| `SubmitEpisodeAsync` | unary | Phase 2+（unimplemented） |
| `GetEpisodeResult` | unary | Phase 2+（unimplemented） |
| `WatchEpisodes` | server stream | Phase 2+（unimplemented） |

### 3.2 ControlPlaneService（Worker → Server/Mock）

**定义**：`proto/uenv/v1/scheduler.proto`  
**实现**：`uenv-server`、`uenv-mock-scheduler`  
**PRD 对照**：§4.2 DispatcherService + WorkerDirectService（合并为单一控制面服务）

| RPC | 方向 | 说明 |
|-----|------|------|
| `RegisterWorker` | Worker → Server | 上报 `worker_id`、`endpoint`、`supported_env_types`、`max_concurrent`、`resource` |
| `WorkerHeartbeat` | 双向流 | Worker 上报 `load`；Server 回复 `server_epoch`、`next_heartbeat_interval_ms`、`DrainCommand` |
| `ReportResult` | Worker → Server | 幂等键：`episode_id:attempt_id:worker_id` |
| `ListWorkers` | 查询 | 只读资源目录（Admin 亦复用此消息） |

### 3.3 WorkerGrpcService（Server/Mock → Worker）

**定义**：`uenv-worker/proto/worker_service.proto`  
**实现**：`uenv-worker`  
**PRD 对照**：§4.2 `DispatchEpisode` 服务端流

| RPC | 模式 | 说明 |
|-----|------|------|
| `DispatchEpisode(DispatchEpisodeRequest)` | **server stream** `StreamReport` | 请求体为完整 `EpisodeRequest`（含租约字段） |
| `HealthCheck` | unary | Worker 探活 |

### 3.4 AdminService（运维 → Server）

**定义**：`proto/uenv/v1/server.proto`

| RPC | 说明 |
|-----|------|
| `ListWorkers` | 复用 `scheduler.proto` 的 `ListWorkersRequest/Response` |
| `DrainWorker` | 排空 Worker |
| `CancelEpisode` | 取消在途 Episode |
| `GetServerStatus` | 返回 `server_epoch`、Worker/Episode 计数 |

---

## 4. 核心消息结构

### 4.1 EpisodeRequest

```protobuf
message EpisodeRequest {
    string episode_id = 1;
    uint32 attempt_id = 2;
    string env_type   = 3;              // Phase 0: "gsm8k"
    bytes payload     = 4;                // 环境特定 JSON（如题目、配置）
    ExecutionMode mode = 5;
    int32 max_steps   = 6;
    ResourceSpec resource_spec = 7;
    string model_endpoint = 8;
    optional int32 seed = 9;
    string correlation_id = 10;           // 日志 trace_id 映射
    int32 timeout_seconds = 11;
    bytes reward_config = 12;             // JSON，如 rule_reward target

    // 派发租约（design §7.7）
    string dispatch_lease_id = 13;
    google.protobuf.Timestamp lease_expire_at = 14;
    uint64 scheduler_epoch = 15;
    bytes dispatch_token = 16;
}
```

**约定**：

- `payload`、`reward_config` 为 **bytes 承载的 UTF-8 JSON**，类型约束由 Hub `interface` schema 或本地 manifest 描述
- Server 在 `DispatchEpisode` 前 MUST 填充 `dispatch_lease_id`、`lease_expire_at`、`scheduler_epoch`
- Episode 级重试由 Scheduler 递增 `attempt_id` 触发；Worker **禁止**对 `env.step()` 默认自动重试

### 4.2 EpisodeResult

```protobuf
message EpisodeResult {
    string episode_id     = 1;
    uint32 attempt_id     = 2;
    string status         = 3;            // "completed" | "failed" | "timeout"
    Trajectory trajectory = 4;
    Summary summary       = 5;
    optional ErrorCode error_code = 6;
    string error_message  = 7;
    string trajectory_checksum = 8;       // SHA-256 hex
    bool integrity_verified = 9;
}
```

**回报路径**：

1. `DispatchEpisode` 流推送 `StreamReport`（进度）
2. Worker 经 `ControlPlaneService.ReportResult` 上报完整 `EpisodeResult`（权威结果）
3. Server `SubmitEpisode` 阻塞等待 `ReportResult` 后返回客户端

### 4.3 StreamReport

```protobuf
enum ReportType {
    REPORT_TYPE_UNSPECIFIED = 0;
    PROGRESS       = 1;
    STEP_COMPLETE  = 2;
    REWARD_SIGNAL  = 3;
    LOG            = 4;
    PACING         = 5;
}

message StreamReport {
    string episode_id = 1;
    uint32 attempt_id = 2;
    int32 current_step = 3;
    int32 total_steps = 4;
    double current_reward = 5;
    string phase = 6;                     // MVP 兼容："step_complete" 等
    optional StepRecord last_step = 7;
    ReportType report_type = 8;           // PRD §4.2，新实现应填充
    // … 延迟、步调字段见 episode.proto
}
```

MVP 阶段 Worker 至少发送 1 条 `STEP_COMPLETE`；`PACING` 等为 Phase 1+ 目标。

### 4.4 WalRecord

见 `proto/uenv/v1/wal.proto`。幂等键：`episode_id + attempt_id + worker_id`。

### 4.5 ErrorCode

见 `proto/uenv/v1/common.proto`。gRPC `Status` code 与 `ErrorCode` 映射见各 crate 错误处理模块。

---

## 5. 典型时序（GSM8K 单轮）

```
Worker                          Server/Mock                         Bridge
  |                                 |                                  |
  |-- RegisterWorker -------------->|                                  |
  |<-- server_epoch -----------------|                                  |
  |-- WorkerHeartbeat (stream) ----->|                                  |
  |                                 |<-------- SubmitEpisode ----------|
  |                                 |  (schedule + fill lease)         |
  |<-- DispatchEpisode -------------|                                  |
  |-- StreamReport (step_complete) ->|                                  |
  |-- ReportResult ---------------->|                                  |
  |                                 |-------- EpisodeResult ---------->|
```

---

## 6. 各 Crate 实现对照

| Crate | 实现的 Service | 作为 Client 调用 |
|-------|----------------|------------------|
| `uenv-server` | `UEnvService`、`AdminService`、`ControlPlaneService` | `WorkerGrpcService` |
| `uenv-worker` | `WorkerGrpcService` | `ControlPlaneService` |
| `uenv-mock-scheduler` | `ControlPlaneService` | `WorkerGrpcService` |
| `uenv-bridge` | — | `UEnvService`（Python，待完善） |
| `uenv-hub` | HTTP REST | — |

---

## 7. 配置与环境变量（Worker 侧摘要）

| 变量 | 说明 |
|------|------|
| `UENV_SCHEDULER_MODE` | `remote` \| `mock` |
| `UENV_SERVER_ENDPOINT` | ControlPlane 地址（Server 或 Mock） |
| `UENV_WORKER_LISTEN` | Worker gRPC 对外地址（供 Dispatch 回连） |
| `UENV_ENV_TYPES` | 如 `gsm8k` |
| `UENV_PLUGIN_DIR` | 插件目录 |

完整配置见 `config/uenv-worker.yaml`。

---

## 8. 变更记录

| 日期 | 变更 |
|------|------|
| 2026-05-30 | 统一 `uenv-server` 至共享 proto；删除 `uenv-server/proto/server.proto` 重复定义；`StreamReport` 增加 `ReportType`；Server 实现 `ControlPlaneService` + `WorkerGrpcService` 客户端 |

---

## 参考

- [proto/README.md](proto/README.md)
- [Docs/worker-pool-mvp-checklist.md](Docs/worker-pool-mvp-checklist.md)
- [secrets/README.md](secrets/README.md)（A100 联调，已 gitignore）
