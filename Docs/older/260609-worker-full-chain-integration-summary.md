# 三、UENV Worker

> **版本**：2026-06-16（§1～§3 已同步代码与 2026-06-13 AgentLoop 实机结论）  
> **依据**：四端实机联调（7142 VeRL `UEnvAgentLoop` → `8.130.86.71:8088` adapter-core+Server → 7143 Worker → Hub `8.130.95.176:8088`）  
> **历史日志包**：[`logs/e2e-full-chain-20260609T102437Z/`](../logs/e2e-full-chain-20260609T102437Z/)（stub 链路，`reward=1.0`；**非** AgentLoop 真实 GSM8K）  
> **相关**：[worker-pool-layer-design.md](./worker-pool-layer-design.md)、[260608-verl-gsm8k-real-testing-adjustments.md](./260608-verl-gsm8k-real-testing-adjustments.md)（AgentLoop 全栈验收详见其 §5.4）



## 1. MVP 阶段 Worker 实现情况

### 1.1 已实现功能

模块	实现要点	代码位置
**运行时**	读 YAML 配置、加载插件目录、启动 gRPC + 可观测性 HTTP	`runtime.rs`, `main.rs`
**控制面 Client**	向 Server 注册、双向流心跳、上报 Episode 结果	`control_plane/client.rs`
**数据面 Server**	接收 `DispatchEpisode`，流式返回 `StreamReport`	`grpc_server/worker_service.rs`
**Episode 执行**	多步主循环（GSM8K 单步终止）：`acquire → reset → (infer → step)* → release`	`episode/executor.rs`
**ModelClient**	`response_text` 优先；Episode `model_endpoint`/`model_name`/`generation_config`；OpenRouter/vLLM HTTP	`episode/model_client.rs`, `llm.rs`
**RewardEngine**	默认采信插件 `step.reward`；仅 `scorer=worker` 时平台精确比对	`episode/reward_engine.rs`
**Payload 规范化**	reset 前合并 `question`/`dataset`/`target` 至 `{uds}.episode.json`	`episode/payload.rs`
**预热池**	按 `env_type` 维护 Warm 实例；spawn 后 `wait_plugin_ready`；命中/未命中指标	`pool/warmup_pool.rs`
**插件宿主**	`ProcessBackend` 子进程 + Proto/UDS；`plugins/math/`	`plugin/host.rs`, `backend/process.rs`
**Hub 元数据**	启动 pull manifest；`EnvResolver` 缺实例前校验	`hub/mod.rs`, `hub/env_resolver.rs`
**WAL**	结果持久化 + 断连重放 `ReportResult`	`wal/mod.rs`
**可观测性**	Prometheus 文本指标 + `/health`	`metrics.rs`, `runtime.rs`
**Lease 校验**	`dispatch_lease_id` 必填、过期/冲突拒绝	`worker_service.rs`
**并发控制**	`Semaphore(max_concurrent)`	`worker_service.rs`
**断连策略**	控制面断开时 `reject`（默认）或 `queue`（`UENV_DISPATCH_ON_DISCONNECT`）	`worker_service.rs`, `main.rs`

7143 实机关键环境变量：

```bash
# LLM（AgentLoop 全栈必须；默认 OpenRouter，见 config/uenv-worker-llm.env.example）
cp config/uenv-worker-llm.env.example config/uenv-worker-llm.env
# 编辑 UENV_LLM_API_KEY（勿提交仓库）

UENV_MATH_PLUGIN_BIN=/root/UEnv/target/release/uenv-math-plugin
UENV_PLUGIN_DIR=/root/UEnv/plugins
UENV_HUB_TOKEN=<Bearer token>
# 可选：启动时预暖池（deploy-7143.yaml 默认 false，按需拉起）
UENV_PREWARM_ON_STARTUP=true
# 可选：异构资源上报
UENV_WORKER_GPU_COUNT=1
UENV_WORKER_GPU_TYPE=A100
```

### 1.2 代码现状 vs 仍待实机验收（2026-06-16 更新）

> **说明**：2026-06-09 实机为 **stub / rule_reward 捷径** 链路；2026-06-13 已在同拓扑完成 **AgentLoop + OpenRouter + math 插件** 单条 GSM8K 全栈验收（见 §2.1.2）。下表以当前代码为准。

| 位置 | 代码现状 | 实机验收 |
|------|----------|----------|
| **`uenv-math-plugin`** | ✅ reset 读 `{uds}.episode.json`；`dataset=gsm8k` + `answers_match`（`plugins/math/.../gsm8k/`） | ✅ 单条 Natalia 样本（`target=72`）；仍待 ≥2 **不同** 题 |
| **`ModelClient`（W-1）** | ✅ 首步优先 `payload.response_text` | ✅ AgentLoop 路径由 Worker 调 LLM |
| **`ModelClient`（W-2）** | ✅ 优先 Episode `model_endpoint` / `model_name` / `generation_config`；API Key 来自 `uenv-worker-llm.env` | ✅ 2026-06-15 代码；7143 OpenRouter 实机 |
| **`ModelClient`（grpcurl）** | ✅ 仅无 LLM、无 `question` 时 `rule_reward` 短路 | ✅ 2026-06-09 stub 链路 |
| **`RewardEngine`** | ✅ 默认透传插件 `step.reward`；`scorer=worker` 为通用精确比对（不含 GSM8K `####`） | ✅ 判分权威在 math 插件 |
| **Bridge `core.rs`** | ✅ `question` / `dataset=gsm8k` / `rule_reward.target`；AgentLoop 完整 prompt（W-13） | ✅ 2026-06-13 实机 |
| **心跳 `load`** | ✅ `metrics.active_episode_count()` | 多 Worker 负载调度 E2E |
| **`RegisterWorker.resource`** | ✅ `detect_resource_spec()`（CPU/内存/GPU 可 env 覆盖） | 异构调度 E2E |
| **`StreamReport`** | ✅ 每步 `step_complete` + 末条 `episode_complete`；填 `report_type`、延迟、`correlation_id`、`worker_id` 等 | ✅ 2026-06-13 |
| **`llm.rs` + LLM env** | ✅ 默认 OpenRouter；支持 vLLM 无 Key 端点 | ✅ 7143 `uenv-worker-llm.env` |
| **Hub 集成** | 仅启动拉 manifest 元数据 | H-6 热路径拉制品 |
| **Episode 步数** | ✅ `execute_episode` 多步循环；math GSM8K 第一步 `terminated=true` | 多轮 Agent 环境（非 GSM8K）待验 |
| **Podman 后端** | 代码存在，7143 用 `process` | W-10 可选验收 |
| **`registry/worker_pool.rs`** | 占位 | P2 |

### 1.3 注意事项：实机 Worker 规模

历次 A100 四端联调均 **只注册一个 Worker 进程**（`uenv-worker` 单实例，部署在 **7143**；2026-06-09 Worker ID `5e96910f-6dac-4700-bc58-80de28cbb7a7`）。Server 调度清单中仅一条 `RegisterWorker` 记录。

**已验证**：单 Worker 上「AgentLoop / grpcurl → adapter-core → Server → DispatchEpisode → 预热池 → math 插件 → OpenRouter（或 rule_reward）→ ReportResult」可达；2026-06-13 另验收 1-step GRPO 与 Worker `reward` 一致。

**未验证**：多 Worker 并行、跨节点负载均衡、≥100 样本 benchmark、Hub 热路径拉制品、PRD §8.5 大规模并行。

---

## 2. 测试内容与 Worker 内通信流程

### 2.1 实机验收记录

> **范围说明**：均在 **单 Worker 进程** 前提下（7143 仅 1 个 `uenv-worker`）；Server 无第二候选 Worker。见 §1.3。

#### 2.1.1 2026-06-09：stub / rule_reward 捷径（历史）

1. **7143 Worker 存活**：`/health` 返回 `ok`  
2. **Hub 连通**：启动时 `hub_manifest_pulled`（math `1.0.0`）  
3. **Server 控制面**：`register` + 持续 `heartbeat`（`server_epoch=1`）  
4. **全链路 Episode**：Python → adapter-core → Server → Worker `DispatchEpisode` → **`reward=1.0`（rule_reward 捷径）** → `report_result`  
5. **日志**：[`logs/e2e-full-chain-20260609T102437Z/`](../logs/e2e-full-chain-20260609T102437Z/) — **非** 真实 LLM rollout

#### 2.1.2 2026-06-13：AgentLoop + OpenRouter + GSM8K 判分（当前主路径）

集成路径：**VeRL `UEnvAgentLoop`** → `RustCoreEpisodeClient` → adapter-core → Server → Worker；Worker **必须** 配 `config/uenv-worker-llm.env`（OpenRouter），由 `ModelClient` 生成 action，math 插件 `answers_match` 判分。

| 项 | 结果 |
|----|------|
| smoke（`verify_pre_rollout_rust_core_loop.py`） | `uenv_status=completed`；首次可能 `reward=0`（模型未按 `####` 答对，**非链路失败**） |
| 1-step GRPO（`max_response_length=32`） | 训练 `1/1`；Worker **`reward=0.0`**、VeRL `critic/rewards/mean=0.0` |
| 对比跑（`DATA_MAX_RESPONSE_LENGTH=256` + 完整 prompt） | Worker **`reward=1.0`**、VeRL **`critic/rewards/mean=1.0`**（Natalia 样本，`target=72`） |
| 样本 | GSM8K train 第 1 条；题干含 `Let's think step by step... ####` 指令 |
| 7142 日志 | `single-gsm8k-grpo-gpu4-v3.log`（reward=0）、`single-gsm8k-compare-256.log`（reward=1） |

仍待：≥2 道 **不同** GSM8K 题；≥100 样本 acc 可复现（见 [260608 §5](./260608-verl-gsm8k-real-testing-adjustments.md#5-验收标准)）。

### 2.2 请求进入 Worker 后的完整链路

```text
                    ┌─────────────────────────────────────────┐
                    │  Server（Scheduler）主动 gRPC 调用       │
                    │  WorkerGrpcService.DispatchEpisode      │
                    └──────────────────┬──────────────────────┘
                                       │
                                       ▼
┌──────────────────────────────────────────────────────────────────┐
│ 1. 准入：lease 校验 / 并发 Semaphore / 控制面断连策略（reject）    │
│ 2. 预热池 acquire(env_type=math) → 命中 Warm 或 spawn + ready 轮询│
│ 3. build_reset_config → 插件 reset → observation（按题注入）      │
│ 4. ModelClient.infer_action：                                      │
│      首步 response_text → Episode model_endpoint/name/gen_cfg    │
│      → OpenRouter/vLLM HTTP →（仅 headless）rule_reward 短路       │
│ 5. 插件 step(action) → reward / terminated；GSM8K 在插件内判分      │
│ 6. RewardEngine 透传插件 reward（默认不二次判分）                   │
│ 7. 每步推送 StreamReport（step_complete）；末条改 episode_complete │
│ 8. 预热池 release → 实例归还 Warm 队列                             │
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

RPC	类型	说明
`DispatchEpisode`	Unary → **Server stream**	下发单个 Episode，执行中/完成后推送 `StreamReport`
`HealthCheck`	Unary	Worker 探活

#### `DispatchEpisode`

**Request：`DispatchEpisodeRequest`**

字段	类型	必填	说明
`episode`	`uenv.v1.EpisodeRequest`	是	完整 Episode 规格（见 §3.3）

**Response：stream `uenv.v1.StreamReport`**

Worker 行为：每步发送一条 `StreamReport`（`phase=running` 或 `step_complete`，`report_type=STEP_COMPLETE`）；Episode 结束后将 **最后一条** 改为 `phase=episode_complete`。GSM8K 单步环境通常共 **2 条**（一步 `step_complete` + 末条 `episode_complete`）。`ReportResult` 在流关闭后异步上报。

#### `HealthCheck`

**Request：`HealthCheckRequest`** — 空消息

**Response：`HealthCheckResponse`**

字段	类型	说明
`ok`	`bool`	恒 `true`（MVP）
`status`	`string`	如 `"ok"`

**HTTP 等价**：`GET http://<worker>:28777/health` → 文本 `ok`

---

### 3.2 Worker 作为 Client 连接 Server 的控制面

> Proto：`proto/uenv/v1/scheduler.proto`  
> Package：`uenv.scheduler.v1`  
> **调用方向**：Worker 作为 **Client**，Server / adapter-core 内嵌 `ControlPlaneService` 作为 **Server**

#### Service：`ControlPlaneService`

RPC	类型	Worker 是否实现 Client
`RegisterWorker`	Unary	√ 启动时一次
`WorkerHeartbeat`	**Client stream → Server stream**	√ 后台循环
`ReportResult`	Unary	√ 每个 Episode 完成后
`ListWorkers`	Unary	× Worker 不调用（Admin/Server 侧）

---

#### 3.2.1 `RegisterWorker`

**Request：`RegisterWorkerRequest`**

字段	类型	必填	Worker 实填示例
`worker_id`	`string`	是	配置 `auto` 则 Server 分配
`supported_env_types`	`repeated string`	是	`["math"]`
`resource`	`uenv.v1.ResourceSpec`	否	`detect_resource_spec()`（`cpu_cores`/`memory_mb`/`gpu_count`/`gpu_type`）
`endpoint`	`string`	是	`advertise_endpoint`，如 `219.147.100.43:28888`
`max_concurrent`	`uint32`	是	如 `4`

**Response：`RegisterWorkerResponse`**

字段	类型	说明
`accepted`	`bool`	是否接受注册
`worker_id`	`string`	确认/分配的 Worker ID
`message`	`string`	人类可读信息
`server_epoch`	`uint64`	Server 纪元，后续心跳/上报需携带

---

#### 3.2.2 `WorkerHeartbeat`

**Request（Client stream）：`HeartbeatRequest`**

字段	类型	Worker 行为
`worker_id`	`string`	当前 Worker ID
`load`	`int32`	**`metrics.active_episode_count()`**（执行中 Episode 数）
`max_load`	`int32`	`max_concurrent`
`timestamp_ms`	`int64`	当前 Unix 毫秒
`server_epoch`	`uint64`	本地缓存的 Server epoch

**Response（Server stream）：`HeartbeatResponse`**

字段	类型	说明
`ok`	`bool`	心跳是否接受
`drain`	`DrainCommand`	可选 drain 指令
`server_epoch`	`uint64`	更新后的 epoch
`next_heartbeat_interval_ms`	`int32`	建议下次心跳间隔

**`DrainCommand`**

字段	类型	说明
`drain`	`bool`	是否进入 drain
`grace_period_sec`	`int32`	优雅退出宽限秒数

Worker MVP：每 ~5s 发一次心跳；日志 `msg=heartbeat`。

---

#### 3.2.3 `ReportResult`

**Request：`ReportResultRequest`**

字段	类型	说明
`idempotency_key`	`string`	`{episode_id}:{attempt_id}:{worker_id}`
`worker_id`	`string`	Worker ID
`server_epoch`	`uint64`	注册/心跳同步的 epoch
`result`	`uenv.v1.EpisodeResult`	完整结果（见 §3.4）

**Response：`ReportResultResponse`**

字段	类型	说明
`ack`	`bool`	Server 是否确认
`duplicate`	`bool`	是否重复上报

失败时写入 WAL，后台 `spawn_replay_loop` 重试。

---

### 3.3 共享 Episode 数据结构（Server ↔ Worker）

> Proto：`proto/uenv/v1/episode.proto`、`proto/uenv/v1/common.proto`  
> Package：`uenv.v1`

#### `EpisodeRequest`（Server 填入后经 `DispatchEpisode` 下发）

字段	类型	说明
`episode_id`	`string`	Episode 唯一 ID
`attempt_id`	`uint32`	重试序号，从 1 起
`env_type`	`string`	Phase 0：`"math"`
`payload`	`bytes`	环境配置 JSON（MVP 多为 `env_config` 子集）
`mode`	`ExecutionMode`	如 `MODE_MULTI`
`max_steps`	`int32`	最大步数
`resource_spec`	`ResourceSpec`	资源需求
`model_endpoint`	`string`	模型回调 URL（可选）
`seed`	`optional int32`	随机种子
`correlation_id`	`string`	全链路 trace，如 `e2e-chain-smoke-0`
`timeout_seconds`	`int32`	超时
`reward_config`	`bytes`	判分配置 JSON
`dispatch_lease_id`	`string`	**必填**，调度租约 ID
`lease_expire_at`	`google.protobuf.Timestamp`	租约过期时间
`scheduler_epoch`	`uint64`	调度器 epoch
`dispatch_token`	`bytes`	可选 dispatch 令牌

**`payload` JSON（AgentLoop / Bridge 映射后，节选）**

| 字段 | 说明 |
|------|------|
| `question` | GSM8K 题干（2026-06-13 起含 `####` 格式指令） |
| `dataset` | `"gsm8k"`（math 环境内 benchmark 路由） |
| `response_text` | 可选；首步若存在则 **优先** 作 action（后 rollout 路径） |
| `model_endpoint` | LLM 基址；优先于 proto 顶层 `model_endpoint` |
| `model_name` | 模型 slug；优先于 `uenv-worker-llm.env` |
| `generation_config` | `temperature` / `max_new_tokens` 等 |

**`reward_config` JSON（节选）**

```json
{"type": "rule_reward", "target": "<#### 后标准答案>"}
```

GSM8K 判分在 math 插件内完成；`target` 注入 reset 配置供插件比对。`scorer=worker` 时平台才二次精确比对（headless 单测用，**不含** `####` 提取）。

#### `ExecutionMode`（enum）

值	名称
0	`MODE_UNSPECIFIED`
1	`MODE_SINGLE`
2	`MODE_MULTI`
3	`MODE_MODEL_CALLBACK`
4	`MODE_CUSTOM`

#### `ResourceSpec`

字段	类型
`cpu_cores`	`int32`
`memory_mb`	`int32`
`gpu_count`	`int32`
`gpu_type`	`string`

#### `StepRecord`

字段	类型
`step_index`	`int32`
`observation`	`bytes`
`action`	`bytes`
`reward`	`double`
`terminated`	`bool`
`truncated`	`bool`
`info`	`map<string,string>`
`duration_ms`	`int64`

#### `Trajectory`

字段	类型
`steps`	`repeated StepRecord`
`total_reward`	`double`
`total_steps`	`int32`

#### `EpisodeResult`（Worker 经 `ReportResult` 上报）

字段	类型	说明
`episode_id`	`string`	与 Request 一致
`attempt_id`	`uint32`	与 Request 一致
`status`	`string`	`"completed"` / `"failed"` / `"timeout"`
`trajectory`	`Trajectory`	完整轨迹
`summary`	`Summary`	汇总
`error_code`	`optional ErrorCode`	失败时
`error_message`	`string`	错误描述
`trajectory_checksum`	`string`	SHA256(hex)
`integrity_verified`	`bool`	MVP 为 `true`

**`EpisodeResult.Summary`**

字段	类型
`total_reward`	`double`
`total_steps`	`int32`
`total_duration_ms`	`int64`
`terminate_reason`	`string`	`terminated` / `truncated` / `max_steps_reached`

#### `StreamReport`（`DispatchEpisode` 流式响应）

字段	类型	填充情况
`episode_id`	`string`	√
`attempt_id`	`uint32`	√
`current_step`	`int32`	√
`total_steps`	`int32`	√（来自 `EpisodeRequest.max_steps`）
`current_reward`	`double`	√（累计 reward）
`phase`	`string`	√ `running` / `step_complete`；末条 `episode_complete`
`last_step`	`optional StepRecord`	√
`report_type`	`ReportType` enum	√ `STEP_COMPLETE`（末条 `PROGRESS`）
`step_latency_ms`	`int64`	√ 本步 env step 耗时
`model_latency_ms`	`int64`	√ 累计 model 回调耗时
`estimated_remaining_seconds`	`double`	未填
`worker_active_episodes`	`int32`	√
`worker_capacity`	`int32`	√
`correlation_id`	`string`	√
`worker_id`	`string`	√

**`ReportType` enum**：`UNSPECIFIED` | `PROGRESS` | `STEP_COMPLETE` | `REWARD_SIGNAL` | `LOG` | `PACING`

#### `ErrorCode`（enum，节选）

值	名称	场景
1001	`ERR_INVALID_REQUEST`	请求非法
1002	`ERR_UNKNOWN_ENV_TYPE`	不支持 env_type
2001	`ERR_NO_AVAILABLE_WORKER`	Server 侧
3002	`ERR_ENV_INIT_FAILED`	插件 reset 失败
3003	`ERR_ENV_STEP_FAILED`	插件 step 失败
3004	`ERR_MODEL_CALL_FAILED`	ModelClient 失败
3007	`ERR_LEASE_EXPIRED`	租约过期

---

### 3.4 WAL 记录结构（Worker 内部，供 Server 重放语义）

> Proto：`proto/uenv/v1/wal.proto`

字段	类型	说明
`episode_id`	`string`	
`attempt_id`	`uint32`	
`worker_id`	`string`	
`dispatch_lease_id`	`string`	
`server_epoch`	`uint64`	
`request_checksum`	`string`	
`result_checksum`	`string`	
`status`	`string`	
`protobuf_payload`	`bytes`	序列化 `EpisodeResult`
`created_at`	`Timestamp`	
`replay_state`	`ReplayState`	PENDING / SENT / ACKED

幂等键：`idempotency_key = episode_id + attempt_id + worker_id`

---

### 3.5 Worker 与 Hub 的 HTTP 接口（Worker 作 Client）

> Worker **仅消费** Hub Registry 的只读 manifest API；不调用 Publish/Admin。  
> 权威文档：[uenv-hub/docs/api.md](../uenv-hub/docs/api.md)

#### 3.5.1 Worker 实际调用的接口

#### `GET /api/v1/envs/{env_type}/versions/latest`

项	值
方法	`GET`
路径参数	`env_type` — 如 `math`
认证	`Authorization: Bearer <UENV_HUB_TOKEN>`（reader 角色）
超时	10s（Worker 硬编码）

**Worker 解析的 JSON 子集（`HubEnvManifest`）**

字段	类型	必填	说明
`env_type`	`string`	是	须与请求路径一致
`version`	`string`	是	如 `1.0.0`
`entrypoint`	`string`	否	Hub 元数据；Worker **优先本地** `plugins/{env_type}/manifest.yaml` 的 `./run.sh`
`supported_backends`	`string[]`	否	默认 `["process"]`

Hub 返回的完整 `FullManifest` 还包含（Worker **当前忽略**，不下载）：

字段	说明
`changelog`	变更说明
`dependencies`	Python 依赖等
`min_uenv_version`	最低 UEnv 版本
`base_image` / `image`	OCI 镜像 URL/digest
`health_check_path`	容器健康检查路径
`interface`	action/observation/state JSON Schema
`examples`	示例请求
`config_schema` / `default_config`	环境配置 Schema
`resources`	CPU/内存/GPU
`is_yanked` / `published_at`	发布元数据

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

RPC	Request	Response 要点
`Reset`	`optional int32 seed`	`observation` bytes, `info` map
`Step`	`action` bytes	`observation`, `reward`, `terminated`, `truncated`, `info`
`Close`	空	`ok`
`HealthCheck`	空	`ok`, `message`

math 插件启动：`plugins/math/run.sh` → `exec $UENV_MATH_PLUGIN_BIN --uds-path <path>`

---

### 3.7 Worker 可观测性端点（非 gRPC）

端点	端口（7143）	说明
`GET /health`	28777	文本 `ok`
`GET /metrics`	28777	Prometheus 文本格式

主要指标名：`uenv_episode_total`、`uenv_episode_duration_ms_sum`、`uenv_env_step_duration_ms_sum`、`uenv_model_callback_duration_ms_sum`、`uenv_warmup_pool_hit_total`、`uenv_warmup_pool_miss_total`、`uenv_active_episode_count`、`uenv_heartbeat_lag_ms`、`uenv_wal_pending_records`、`uenv_instance_pool_size{status=...}`（creating/warm/active/idle/cooling/evicting/destroyed）

---

### 3.8 Worker 侧 LLM 配置（`ModelClient`）

> 代码：`uenv-worker/src/llm.rs`；启动时由 `runtime.rs` 加载 `config/uenv-worker-llm.env`（`UENV_WORKER_LLM_ENV` 可覆盖）。

| 变量 | 默认 / 说明 |
|------|-------------|
| `UENV_LLM_PROVIDER` | `openrouter` |
| `UENV_LLM_ENDPOINT` | `https://openrouter.ai/api/v1` |
| `UENV_LLM_MODEL_NAME` | `qwen/qwen-2.5-7b-instruct` |
| `UENV_LLM_API_KEY` | OpenRouter **必填**；vLLM 本地端点可无 Key |
| `UENV_LLM_MAX_TOKENS` / `UENV_LLM_TEMPERATURE` | 默认 512 / 1.0；可被 payload `generation_config` 覆盖 |

**端点优先级**（`model_client.rs`）：`payload.model_endpoint` → `EpisodeRequest.model_endpoint` → `uenv-worker-llm.env`。API Key **仅** 来自 Worker 本地 env，不进 Bridge。

**AgentLoop 全栈**：Episode 通常 **不含** `response_text`；Worker 必须配 LLM，由 OpenRouter 生成 action。grpcurl / headless 无 LLM 且无 `question` 时走 `rule_reward` 短路。

---
