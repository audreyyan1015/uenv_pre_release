# 三、UENV Worker

> **版本**：2026-06-09  
> **依据**：四端实机联调（7142 Python Adapter → `8.130.86.71:8088` adapter-core+Server → 7143 Worker → Hub `8.130.95.176:8088`）  
> **日志包**：[`logs/e2e-full-chain-20260609T102437Z/`](../logs/e2e-full-chain-20260609T102437Z/)（`test_pass=true`，`reward=1.0`）  
> **相关**：[worker-pool-layer-design.md](./worker-pool-layer-design.md)、[260608-verl-gsm8k-real-testing-adjustments.md](./260608-verl-gsm8k-real-testing-adjustments.md)



## 1. MVP 阶段 Worker 实现情况

### 1.1 已实现功能

| 模块 | 实现要点 | 代码位置 |
| **运行时** | 读 YAML 配置、加载插件目录、启动 gRPC + 可观测性 HTTP | `runtime.rs`, `main.rs` |
| **控制面 Client** | 向 Server 注册、双向流心跳、上报 Episode 结果 | `control_plane/client.rs` |
| **数据面 Server** | 接收 `DispatchEpisode`，流式返回 `StreamReport` | `grpc_server/worker_service.rs` |
| **Episode 执行** | 单轮：`acquire → reset → infer_action → step → release → 判分` | `episode/executor.rs` |
| **预热池** | 按 `env_type` 维护 Warm 实例；命中/未命中指标 | `pool/warmup_pool.rs` |
| **插件宿主** | `ProcessBackend` 子进程 + Proto/UDS；`plugins/math/` | `plugin/host.rs`, `backend/process.rs` |
| **Hub 元数据** | 启动 pull manifest；`EnvResolver` 缺实例前校验 | `hub/mod.rs`, `hub/env_resolver.rs` |
| **WAL** | 结果持久化 + 断连重放 `ReportResult` | `wal/mod.rs` |
| **可观测性** | Prometheus 文本指标 + `/health` | `metrics.rs`, `runtime.rs` |
| **Lease 校验** | `dispatch_lease_id` 必填、过期/冲突拒绝 | `worker_service.rs` |
| **并发控制** | `Semaphore(max_concurrent)` | `worker_service.rs` |

7143 实机关键环境变量（见日志 `03-worker-7143.log`）：

```bash
UENV_MATH_PLUGIN_BIN=/root/UEnv/target/release/uenv-math-plugin
UENV_PLUGIN_DIR=/root/UEnv/plugins
UENV_HUB_TOKEN=<Bearer token>
UENV_PREWARM_ON_STARTUP=true
```

### 1.2 仍为 Mock / Stub / 占位

| 位置 | 现状 | 影响 |
| **`uenv-math-plugin`** | `reset` 写死固定数学题，答案恒为 `"20"`；不读 Episode `payload` | E2E 得 `reward=1.0` 依赖 fixture 与 stub 对齐，**非真实 GSM8K** |
| **`ModelClient`** | 若 `reward_config.type=rule_reward` 且有 `target`，**直接把 target 当 action**，不调 LLM | 联调捷径；VeRL 路径应改用 `response_text` |
| **`RewardEngine`** | 仅识别 `rule_reward`；Bridge 来的 `rubric_config` 未映射时 fallback 插件 step reward | 与 Bridge payload 格式未完全打通 |
| **心跳 `load`** | 恒为 `0`（未上报真实活跃 Episode 数） | Server 调度看不到 Worker 负载 |
| **`RegisterWorker.resource`** | 发送 `None` | `ResourceSpec` 未参与注册 |
| **`StreamReport`** | 主要填 `phase`；`report_type` 等 PRD 扩展字段多为默认 | 流式进度语义不完整 |
| **Hub 集成** | 仅 HTTP 拉 manifest 元数据；**不下载**镜像/插件包 | 仍依赖本地 `plugins/` + `UENV_MATH_PLUGIN_BIN` |
| **Episode 步数** | 仅 `execute_single_round`（1 step） | 多轮 Agent 未实现 |
| **Podman 后端** | 代码存在，7143 使用 `process` | 容器化插件未验收 |
| **`registry/worker_pool.rs`** | 占位注释 | 内存 Registry 未用于热路径 |

  

### 1.3 注意事项：本次 Worker 规模

**本次全链路联调只拉起并注册了一个 Worker 进程**（`uenv-worker` 实例 1 个，Worker ID `5e96910f-6dac-4700-bc58-80de28cbb7a7`，部署在 A100 **7143** 主机上）。Server 调度清单中仅一条 `RegisterWorker` 记录。

**因此本次测试验证的是**：单 Worker 上「Server → DispatchEpisode → 预热池 → math 插件 → ReportResult」链路可达；**不能**据此推断多 Worker 并行训练、跨节点负载均衡或 PRD §8.5 大规模并行场景已验收。

---

## 2. 测试内容与 Worker 内通信流程

### 2.1 本次测试验证了什么

> **范围说明**：以下均在 **单 Worker 进程** 前提下验证（7143 主机上仅 1 个 `uenv-worker`）；Server 无第二候选 Worker，调度等价于「唯一进程接单」。见 §1.3。

1. **7143 Worker 存活**：`/health` 返回 `ok`，进程与日志正常  
2. **Hub 连通**：启动时 `hub_manifest_pulled`（math `1.0.0`）  
3. **Server 控制面**：`register` + 持续 `heartbeat`（`server_epoch=1`）  
4. **全链路 Episode**：Python → adapter-core → Server 调度 → **唯一 Worker** `DispatchEpisode` → 返回 `reward=1.0` → `report_result`  


### 2.2 请求进入 Worker 后的完整链路

```text
                    ┌─────────────────────────────────────────┐
                    │  Server（Scheduler）主动 gRPC 调用       │
                    │  WorkerGrpcService.DispatchEpisode      │
                    └──────────────────┬──────────────────────┘
                                       │
                                       ▼
┌──────────────────────────────────────────────────────────────────┐
│ 1. 准入：lease 校验 / 并发 Semaphore / 控制面连接策略              │
│ 2. 预热池 acquire(env_type=math) → 命中 math-2 或 spawn 新实例    │
│ 3. 插件 reset(seed) → UDS 调 uenv-math-plugin（返回 observation） │
│ 4. ModelClient 得 action（rule_reward 捷径或 HTTP LLM）            │
│ 5. 插件 step(action) → reward / terminated                       │
│ 6. RewardEngine 规则判分 → 最终 reward                             │
│ 7. 预热池 release → 实例归还 Warm 队列                             │
│ 8. 同步返回 StreamReport（step_complete）                         │
│ 9. 异步：WAL 持久化 → ControlPlane ReportResult → Server          │
└──────────────────────────────────────────────────────────────────┘
```

**与 Server 的双通道关系**：

- **Server → Worker（数据面）**：`DispatchEpisode` 下发任务，Worker 流式回 `StreamReport`  
- **Worker → Server（控制面）**：Worker 主动 `RegisterWorker` / `WorkerHeartbeat` / `ReportResult`  

Hub **不参与 Episode 热路径**；仅在 Worker **启动**或 **spawn 前**拉 manifest 做元数据对齐。

---

## 3. 协议与接口结构

本节汇总 Worker 对外暴露与主动调用的全部接口及共享数据结构：与 Server 的 gRPC 数据面/控制面、Hub HTTP manifest、进程内 L2 插件 IPC，以及可观测性 HTTP 端点。

### 3.1 Worker 为 Server 提供的 gRPC 接口（Worker 作 Server）

> Proto：`uenv-worker/proto/worker_service.proto`  
> Package：`uenv.worker.v1`  
> **调用方向**：UEnv Server / Scheduler 作为 **Client**，Worker 作为 **Server**

#### 3.1.1 Service：`WorkerGrpcService`

| RPC | 类型 | 说明 |
| `DispatchEpisode` | Unary → **Server stream** | 下发单个 Episode，执行中/完成后推送 `StreamReport` |
| `HealthCheck` | Unary | Worker 探活 |

#### `DispatchEpisode`

**Request：`DispatchEpisodeRequest`**

| 字段 | 类型 | 必填 | 说明 |
| `episode` | `uenv.v1.EpisodeRequest` | 是 | 完整 Episode 规格（见 §3.3） |

**Response：stream `uenv.v1.StreamReport`**

Worker MVP 行为：执行完单轮后发送 **一条** `StreamReport`（`phase=step_complete`），然后关闭流；`ReportResult` 在后台异步上报。

#### `HealthCheck`

**Request：`HealthCheckRequest`** — 空消息

**Response：`HealthCheckResponse`**

| 字段 | 类型 | 说明 |
| `ok` | `bool` | 恒 `true`（MVP） |
| `status` | `string` | 如 `"ok"` |

**HTTP 等价**：`GET http://<worker>:28777/health` → 文本 `ok`

---

### 3.2 Worker 作为 Client 连接 Server 的控制面

> Proto：`proto/uenv/v1/scheduler.proto`  
> Package：`uenv.scheduler.v1`  
> **调用方向**：Worker 作为 **Client**，Server / adapter-core 内嵌 `ControlPlaneService` 作为 **Server**

#### Service：`ControlPlaneService`

| RPC | 类型 | Worker 是否实现 Client |
| `RegisterWorker` | Unary | √ 启动时一次 |
| `WorkerHeartbeat` | **Client stream → Server stream** | √ 后台循环 |
| `ReportResult` | Unary | √ 每个 Episode 完成后 |
| `ListWorkers` | Unary | × Worker 不调用（Admin/Server 侧） |

---

#### 3.2.1 `RegisterWorker`

**Request：`RegisterWorkerRequest`**

| 字段 | 类型 | 必填 | Worker 实填示例 |
| `worker_id` | `string` | 是 | 配置 `auto` 则 Server 分配；实机 `5e96910f-...` |
| `supported_env_types` | `repeated string` | 是 | `["math"]` |
| `resource` | `uenv.v1.ResourceSpec` | 否 | MVP 发 `None` |
| `endpoint` | `string` | 是 | `advertise_endpoint`，如 `219.147.100.43:28888` |
| `max_concurrent` | `uint32` | 是 | 如 `4` |

**Response：`RegisterWorkerResponse`**

| 字段 | 类型 | 说明 |
| `accepted` | `bool` | 是否接受注册 |
| `worker_id` | `string` | 确认/分配的 Worker ID |
| `message` | `string` | 人类可读信息 |
| `server_epoch` | `uint64` | Server 纪元，后续心跳/上报需携带 |

---

#### 3.2.2 `WorkerHeartbeat`

**Request（Client stream）：`HeartbeatRequest`**

| 字段 | 类型 | Worker MVP 行为 |
| `worker_id` | `string` | 当前 Worker ID |
| `load` | `int32` | **固定 0**（待改进） |
| `max_load` | `int32` | `max_concurrent` |
| `timestamp_ms` | `int64` | 当前 Unix 毫秒 |
| `server_epoch` | `uint64` | 本地缓存的 Server epoch |

**Response（Server stream）：`HeartbeatResponse`**

| 字段 | 类型 | 说明 |
| `ok` | `bool` | 心跳是否接受 |
| `drain` | `DrainCommand` | 可选 drain 指令 |
| `server_epoch` | `uint64` | 更新后的 epoch |
| `next_heartbeat_interval_ms` | `int32` | 建议下次心跳间隔 |

**`DrainCommand`**

| 字段 | 类型 | 说明 |
| `drain` | `bool` | 是否进入 drain |
| `grace_period_sec` | `int32` | 优雅退出宽限秒数 |

Worker MVP：每 ~5s 发一次心跳；日志 `msg=heartbeat`。

---

#### 3.2.3 `ReportResult`

**Request：`ReportResultRequest`**

| 字段 | 类型 | 说明 |
| `idempotency_key` | `string` | `{episode_id}:{attempt_id}:{worker_id}` |
| `worker_id` | `string` | Worker ID |
| `server_epoch` | `uint64` | 注册/心跳同步的 epoch |
| `result` | `uenv.v1.EpisodeResult` | 完整结果（见 §3.4） |

**Response：`ReportResultResponse`**

| 字段 | 类型 | 说明 |
| `ack` | `bool` | Server 是否确认 |
| `duplicate` | `bool` | 是否重复上报 |

失败时写入 WAL，后台 `spawn_replay_loop` 重试。

---

### 3.3 共享 Episode 数据结构（Server ↔ Worker）

> Proto：`proto/uenv/v1/episode.proto`、`proto/uenv/v1/common.proto`  
> Package：`uenv.v1`

#### `EpisodeRequest`（Server 填入后经 `DispatchEpisode` 下发）

| 字段 | 类型 | 说明 |
| `episode_id` | `string` | Episode 唯一 ID |
| `attempt_id` | `uint32` | 重试序号，从 1 起 |
| `env_type` | `string` | Phase 0：`"math"` |
| `payload` | `bytes` | 环境配置 JSON（MVP 多为 `env_config` 子集） |
| `mode` | `ExecutionMode` | 如 `MODE_MULTI` |
| `max_steps` | `int32` | 最大步数 |
| `resource_spec` | `ResourceSpec` | 资源需求 |
| `model_endpoint` | `string` | 模型回调 URL（可选） |
| `seed` | `optional int32` | 随机种子 |
| `correlation_id` | `string` | 全链路 trace，如 `e2e-chain-smoke-0` |
| `timeout_seconds` | `int32` | 超时 |
| `reward_config` | `bytes` | 判分配置 JSON |
| `dispatch_lease_id` | `string` | **必填**，调度租约 ID |
| `lease_expire_at` | `google.protobuf.Timestamp` | 租约过期时间 |
| `scheduler_epoch` | `uint64` | 调度器 epoch |
| `dispatch_token` | `bytes` | 可选 dispatch 令牌 |

#### `ExecutionMode`（enum）

| 值 | 名称 |
| 0 | `MODE_UNSPECIFIED` |
| 1 | `MODE_SINGLE` |
| 2 | `MODE_MULTI` |
| 3 | `MODE_MODEL_CALLBACK` |
| 4 | `MODE_CUSTOM` |

#### `ResourceSpec`

| 字段 | 类型 |
| `cpu_cores` | `int32` |
| `memory_mb` | `int32` |
| `gpu_count` | `int32` |
| `gpu_type` | `string` |

#### `StepRecord`

| 字段 | 类型 |
| `step_index` | `int32` |
| `observation` | `bytes` |
| `action` | `bytes` |
| `reward` | `double` |
| `terminated` | `bool` |
| `truncated` | `bool` |
| `info` | `map<string,string>` |
| `duration_ms` | `int64` |

#### `Trajectory`

| 字段 | 类型 |
| `steps` | `repeated StepRecord` |
| `total_reward` | `double` |
| `total_steps` | `int32` |

#### `EpisodeResult`（Worker 经 `ReportResult` 上报）

| 字段 | 类型 | 说明 |
| `episode_id` | `string` | 与 Request 一致 |
| `attempt_id` | `uint32` | 与 Request 一致 |
| `status` | `string` | `"completed"` / `"failed"` / `"timeout"` |
| `trajectory` | `Trajectory` | 完整轨迹 |
| `summary` | `Summary` | 汇总 |
| `error_code` | `optional ErrorCode` | 失败时 |
| `error_message` | `string` | 错误描述 |
| `trajectory_checksum` | `string` | SHA256(hex) |
| `integrity_verified` | `bool` | MVP 为 `true` |

**`EpisodeResult.Summary`**

| 字段 | 类型 |
| `total_reward` | `double` |
| `total_steps` | `int32` |
| `total_duration_ms` | `int64` |
| `terminate_reason` | `string` | MVP：`single_round_done` |

#### `StreamReport`（`DispatchEpisode` 流式响应）

| 字段 | 类型 | MVP 填充情况 |
| `episode_id` | `string` | √ |
| `attempt_id` | `uint32` | √ |
| `current_step` | `int32` | √（单轮为 1） |
| `total_steps` | `int32` | √ |
| `current_reward` | `double` | √ |
| `phase` | `string` | √ `step_complete` |
| `last_step` | `optional StepRecord` | √ |
| `report_type` | `ReportType` enum | × 默认 UNSPECIFIED |
| `step_latency_ms` | `int64` | 未填 |
| `model_latency_ms` | `int64` | 未填 |
| `estimated_remaining_seconds` | `double` | 未填 |
| `worker_active_episodes` | `int32` | 未填 |
| `worker_capacity` | `int32` | 未填 |
| `correlation_id` | `string` | 未填 |
| `worker_id` | `string` | 未填 |

**`ReportType` enum**：`UNSPECIFIED` | `PROGRESS` | `STEP_COMPLETE` | `REWARD_SIGNAL` | `LOG` | `PACING`

#### `ErrorCode`（enum，节选）

| 值 | 名称 | 场景 |
| 1001 | `ERR_INVALID_REQUEST` | 请求非法 |
| 1002 | `ERR_UNKNOWN_ENV_TYPE` | 不支持 env_type |
| 2001 | `ERR_NO_AVAILABLE_WORKER` | Server 侧 |
| 3002 | `ERR_ENV_INIT_FAILED` | 插件 reset 失败 |
| 3003 | `ERR_ENV_STEP_FAILED` | 插件 step 失败 |
| 3004 | `ERR_MODEL_CALL_FAILED` | ModelClient 失败 |
| 3007 | `ERR_LEASE_EXPIRED` | 租约过期 |

---

### 3.4 WAL 记录结构（Worker 内部，供 Server 重放语义）

> Proto：`proto/uenv/v1/wal.proto`

| 字段 | 类型 | 说明 |
| `episode_id` | `string` | |
| `attempt_id` | `uint32` | |
| `worker_id` | `string` | |
| `dispatch_lease_id` | `string` | |
| `server_epoch` | `uint64` | |
| `request_checksum` | `string` | |
| `result_checksum` | `string` | |
| `status` | `string` | |
| `protobuf_payload` | `bytes` | 序列化 `EpisodeResult` |
| `created_at` | `Timestamp` | |
| `replay_state` | `ReplayState` | PENDING / SENT / ACKED |

幂等键：`idempotency_key = episode_id + attempt_id + worker_id`

---

### 3.5 Worker 与 Hub 的 HTTP 接口（Worker 作 Client）

> Worker **仅消费** Hub Registry 的只读 manifest API；不调用 Publish/Admin。  
> 权威文档：[uenv-hub/docs/api.md](../uenv-hub/docs/api.md)

#### 3.5.1 Worker 实际调用的接口

#### `GET /api/v1/envs/{env_type}/versions/latest`

| 项 | 值 |
| 方法 | `GET` |
| 路径参数 | `env_type` — 如 `math` |
| 认证 | `Authorization: Bearer <UENV_HUB_TOKEN>`（reader 角色） |
| 超时 | 10s（Worker 硬编码） |

**Worker 解析的 JSON 子集（`HubEnvManifest`）**

| 字段 | 类型 | 必填 | 说明 |
| `env_type` | `string` | 是 | 须与请求路径一致 |
| `version` | `string` | 是 | 如 `1.0.0` |
| `entrypoint` | `string` | 否 | Hub 元数据；Worker **优先本地** `plugins/{env_type}/manifest.yaml` 的 `./run.sh` |
| `supported_backends` | `string[]` | 否 | 默认 `["process"]` |

Hub 返回的完整 `FullManifest` 还包含（Worker **当前忽略**，不下载）：

| 字段 | 说明 |
| `changelog` | 变更说明 |
| `dependencies` | Python 依赖等 |
| `min_uenv_version` | 最低 UEnv 版本 |
| `base_image` / `image` | OCI 镜像 URL/digest |
| `health_check_path` | 容器健康检查路径 |
| `interface` | action/observation/state JSON Schema |
| `examples` | 示例请求 |
| `config_schema` / `default_config` | 环境配置 Schema |
| `resources` | CPU/内存/GPU |
| `is_yanked` / `published_at` | 发布元数据 |

**成功响应示例（Hub 完整体，节选）**

```json
{
  "env_type": "math",
  "version": "1.0.0",
  "entrypoint": "uenv-worker math",
  "supported_backends": ["process", "podman"],
  "interface": {
    "action": { "type": "object", "properties": { "answer": { "type": "string" } } },
    "observation": { "type": "object", "properties": { "question": { "type": "string" } } }
  },
  "resources": { "cpu": 2.0, "memory_mb": 4096, "gpu": 0 }
}
```

**Worker 处理逻辑**

1. 启动时 `sync_env_types_from_hub` 对每个 `env.types` pull  
2. `EnvResolver.apply_hub_summary` 合并版本/backend 信息  
3. spawn 前 `ensure_env_ready`：本地 `plugins/math/` 必须存在  
4. **不**拉取 `image.url` 或替换二进制  

**失败降级**：Hub 不可达时 `hub_pull_failed_using_local_manifest`，继续用本地插件。

---


### 3.6 Worker 内部 L2 插件 IPC（Execution 必读）

> Proto：`plugin_proto/uenv/plugin/v1/plugin.proto`  
> 传输：Protobuf over Unix Domain Socket（仅 Worker 进程内）

#### Service：`PluginService`

| RPC | Request | Response 要点 |
| `Reset` | `optional int32 seed` | `observation` bytes, `info` map |
| `Step` | `action` bytes | `observation`, `reward`, `terminated`, `truncated`, `info` |
| `Close` | 空 | `ok` |
| `HealthCheck` | 空 | `ok`, `message` |

math 插件启动：`plugins/math/run.sh` → `exec $UENV_MATH_PLUGIN_BIN --uds-path <path>`

---

### 3.7 Worker 可观测性端点（非 gRPC）

| 端点 | 端口（7143） | 说明 |
| `GET /health` | 28777 | 文本 `ok` |
| `GET /metrics` | 28777 | Prometheus 文本格式 |

主要指标名：`uenv_episode_total`、`uenv_episode_duration_ms_sum`、`uenv_warmup_pool_hit_total`、`uenv_warmup_pool_miss_total`、`uenv_active_episode_count`、`uenv_wal_pending_records`、`uenv_instance_pool_size_*`

---

