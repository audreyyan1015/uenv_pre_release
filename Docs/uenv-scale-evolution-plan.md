# UEnv 大规模演进方案

> **版本**: v1.0 | **日期**: 2026-06-21 | **范围**: 4台 → 百台 → 千台 → 万台
>
> 本文基于对当前代码库（bridge-alignment 分支）的完整审查，针对超大规模训练仿真目标（文本+多模态、万台机器）提出分阶段演进路线。每个阶段有明确的边界条件、工程改动点和验收指标。

---

## 一、演进原则

1. **增量而非重写**：每个阶段在现有代码基础上做最小必要改动，保证当前功能不回退。
2. **先解决已知故障模式**：2026-06-16 Worker 假活事件暴露的问题类型，在 Phase 1 全部封堵。
3. **协议向前兼容**：protobuf 字段只增不删，新字段用 optional，旧 Worker 可与新 Server 共存。
4. **可观测性先行**：每个阶段的扩展能力建立在可以看到系统状态的基础上，而不是盲目扩规模。

---

## 二、现状问题地图

```
问题类别            当前代码位置                            触发规模
────────────────────────────────────────────────────────────────────
Server全局写锁      scheduler/mod.rs Vec<WorkerInfo>        >200台开始明显
状态无持久化        state.rs DashMap(纯内存)                 任何规模下重启即丢
心跳不等于业务存活  control_plane.rs / scheduler/mod.rs     已触发(4台)
无背压/无界队列     service.rs sleep(500ms)轮询              >50并发Episode
Model Gateway       model_gateway.py ThreadingHTTPServer     >100并发推理请求
Bridge每批重建      verl_batch_agent_loop_patch.py           >200台训练规模
WAL仅本地           config/uenv-worker.yaml /tmp/wal         机器宕机即丢
gRPC Payload无界    episode.proto bytes payload              多模态场景(图/视频)
Plugin=进程粒度     backend/process.rs                       >500台x4并发
WorkerPool空占位    registry/worker_pool.rs 3行代码          >1000台需分层
ResourceSpec简陋    common.proto 4个字段                      多模态/异构资源
可观测性单机拉取    metrics.rs :19090                        >500台scrape崩溃
```

---

## 三、分阶段演进路线

### Phase 1：基础加固（4台 → 100台）

**目标**：消灭已知故障模式，让现有架构在百台规模下稳定运行。
**周期估计**：2-4周

#### 1.1 心跳与业务存活解耦 ✅ 已实现

> **已实现**：`scheduler/mod.rs` 的 `is_worker_degraded` 函数已包含双维度判断：
> - 维度1：`last_heartbeat_at` 超过 `heartbeat_timeout_secs`（默认 30s）→ 连接断开
> - 维度2：`current_load > 0` 但 `last_report_at` 超过 `degraded_threshold_secs`（默认 400s）无上报 → 业务假活
>
> 调度时已排除 degraded worker（`schedule` 方法内 `!is_worker_degraded` 过滤）。
> `touch_worker_report` 在每次 `report_result` 时刷新 `last_report_at`；心跳 load=0 时也刷新防误判。
>
> 与计划方案的差异：无 `consecutive_episode_failures` 计数器，但核心功能已覆盖。

**问题**：Server 用 `last_report_at`（来自 ReportResult）判断 Worker 健康，但 heartbeat 正常不等于 Episode 在正常处理。这是 2026-06-16 事故的根因。

**改动**（`uenv-server/src/scheduler/traits.rs` + `scheduler/mod.rs`）：

```rust
// scheduler/traits.rs — WorkerInfo 增加两个维度的健康时间戳
pub struct WorkerInfo {
    // ... 现有字段 ...
    pub last_heartbeat_at: Option<Instant>,     // 心跳时间（连接存活）
    pub last_episode_done_at: Option<Instant>,  // 最后完成 Episode 的时间（业务存活）
    pub consecutive_episode_failures: u32,      // 连续失败次数
}

// 新增双维度健康判断，替换现有 is_worker_degraded
pub fn is_worker_healthy(w: &WorkerInfo) -> bool {
    let now = Instant::now();
    // 维度1: 30s 无心跳 -> 连接断开
    let heartbeat_ok = w.last_heartbeat_at
        .map(|t| now.duration_since(t).as_secs() < 30)
        .unwrap_or(false);
    // 维度2: 有活跃 Episode 但 5min 无任何完成 -> 疑似假活
    let business_ok = if w.current_load > 0 {
        w.last_episode_done_at
            .map(|t| now.duration_since(t).as_secs() < 300)
            .unwrap_or(true)  // 刚启动还没完成过，给一次机会
    } else {
        true
    };
    heartbeat_ok && business_ok && w.consecutive_episode_failures < 5
}
```

`control_plane.rs` 中各字段更新点如下：

- **`last_heartbeat_at`**：在 `update_worker_load()` 中更新（该方法已由 heartbeat stream 调用），无需另建新方法。
- **`last_episode_done_at` + `consecutive_episode_failures`**：将现有的 `touch_worker_report(worker_id)` 改为带成功/失败语义的新接口：

```rust
// scheduler/mod.rs — 替换现有 touch_worker_report
pub fn record_episode_result(&mut self, worker_id: &str, success: bool) {
    if let Some(w) = self.workers.iter_mut().find(|w| w.worker_id == worker_id) {
        w.last_episode_done_at = Some(std::time::Instant::now());
        if success {
            w.consecutive_episode_failures = 0;
        } else {
            w.consecutive_episode_failures = w.consecutive_episode_failures.saturating_add(1);
        }
    }
}
```

`control_plane.rs` 的 `report_result` 中，解析 `EpisodeResult.status` 字段（`"completed"` 为成功，`"failed"` / `"timeout"` 为失败），调用 `record_episode_result(worker_id, result.status == "completed")`。调度时同时检查两个维度。

#### 1.2 Server 状态持久化（SQLite 最小可行版）

**问题**：Server 重启丢失所有 Worker 注册信息和在途 Episode 状态。

**方案**：引入 SQLite（`rusqlite` crate），零外部依赖，100台规模绰绰有余：

```
uenv-server/
  src/
    persist/
      mod.rs       -- PersistenceLayer trait
      sqlite.rs    -- SQLite 实现
```

```sql
-- 三张核心表
CREATE TABLE IF NOT EXISTS workers (
    worker_id       TEXT PRIMARY KEY,
    endpoint        TEXT NOT NULL,
    env_types       TEXT NOT NULL,   -- JSON array
    capacity        INTEGER NOT NULL,
    registered_at   INTEGER NOT NULL,
    last_heartbeat  INTEGER,
    draining        INTEGER DEFAULT 0
);

CREATE TABLE IF NOT EXISTS active_episodes (
    episode_id  TEXT PRIMARY KEY,
    attempt_id  INTEGER NOT NULL,
    worker_id   TEXT NOT NULL,
    batch_id    TEXT,
    started_at  INTEGER NOT NULL,
    timeout_at  INTEGER NOT NULL     -- 用于重启后清理超时 Episode
);

-- 带 TTL 的幂等键，解决当前无界 HashSet 的 OOM 隐患
CREATE TABLE IF NOT EXISTS idempotency_keys (
    key         TEXT PRIMARY KEY,
    created_at  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_idempotency_ts ON idempotency_keys(created_at);
```

Server 启动时从 SQLite 恢复 Worker 注册（Worker 重连后立即可调度）；`idempotency_keys` 定期删除 24h 前记录；`active_episodes` 中超过 `timeout_at` 的记录在重启后标记失败。

#### 1.3 有界待调度队列 + 背压信号 ✅ 已实现

> **已实现**：`service.rs` + `state.rs` 已实现动态 Semaphore 队列（2026-06-21）。
> - `queue_dynamic: true` → `Semaphore::new(0)`，随 worker 注册 `add_permits(capacity)`
> - `queue_max_in_flight: N` → `Semaphore::new(N)`，静态容量
> - 等待用 `tokio::time::timeout(deadline - now, sem.acquire_owned()).await`
> - 超时直接返回失败，不忙等
> - worker 注销时后台 `acquire_many(cap) + forget()` 回收 permits
>
> 压测验证（64 workers × 8 = 512 并发，30s LLM latency，30min）：16640 episodes，0 失败，avg_reward=0.697 ✓

**问题**：`service.rs` 中 `sleep(500ms)` 轮询无界等待，N 个并发 Episode 同时等待时 OOM。

```rust
// state.rs 新增
pub struct ServerState {
    // ... 现有字段 ...
    pub dispatch_semaphore: Arc<tokio::sync::Semaphore>,  // 最大并发 Episode 数
}

// service.rs submit_episode 改为：
async fn submit_episode(&self, req: EpisodeRequest) -> Result<EpisodeResult> {
    // 超时获取令牌，满了立即返回 RESOURCE_EXHAUSTED，而不是无限等待
    let _permit = tokio::time::timeout(
        Duration::from_secs(req.timeout_seconds.max(1) as u64),
        self.state.dispatch_semaphore.acquire()
    ).await
    .map_err(|_| anyhow!("dispatch queue full (capacity={})", MAX_QUEUE_DEPTH))??;
    // ... 原有调度逻辑不变 ...
}
```

客户端收到 RESOURCE_EXHAUSTED 后指数退避重试，而不是把 Server 压垮。

#### 1.4 Model Gateway 替换为 nginx 反向代理

当前 Python `ThreadingHTTPServer` 在约 100 并发推理请求时线程耗尽，且不支持流式输出（`response.read()` 把整个响应缓存内存）。

**最快修法**：在 Worker 所在机器上用 nginx 做实际代理，Python 只负责写配置和 reload：

```nginx
# /etc/nginx/conf.d/uenv-model-gateway.conf（由 model_gateway.py 生成）
upstream uenv_llm {
    least_conn;
    server 127.0.0.1:8000;
    server 127.0.0.1:8001;
    keepalive 32;            # TCP 连接复用
}
server {
    listen 18080;
    location / {
        proxy_pass         http://uenv_llm;
        proxy_buffering    off;       # 支持流式（SSE/chunked）
        proxy_read_timeout 600s;
        proxy_http_version 1.1;
        proxy_set_header   Connection "";
    }
}
```

`ModelGateway.start()` 改为渲染上述配置并执行 `nginx -s reload`。外部接口（`public_url`、`set_upstreams()`）完全不变。

**Phase 1 验收指标**：
- Server 重启后，已注册 Worker 在 10s 内恢复可调度状态（无需 Worker 重启）
- 满载时新 Episode 请求返回 RESOURCE_EXHAUSTED，Server 内存不上涨
- 单 Worker 假活时 Server 在 5min 内自动标记降级，不再分发给它
- Model Gateway 支持 200 并发推理请求，支持流式输出

---

### Phase 2：水平扩展（100台 → 1000台）

**目标**：Server 不再是单点，调度器支持分层，Worker 支持异构资源声明。
**周期估计**：1-2个月

#### 2.1 调度器数据结构升级（消除全局写锁）

当前问题：`Vec<WorkerInfo>` 的线性扫描 + `Arc<RwLock<>>` 在高并发 dispatch 下写锁争用严重。

```rust
// scheduler/mod.rs 重构
pub struct CapacityAwareScheduler {
    workers: DashMap<String, WorkerInfo>,           // O(1) 查改，无全局锁
    env_type_index: DashMap<String, Vec<String>>,   // env_type -> [worker_id] 索引
    counter: AtomicUsize,
}

fn schedule(&self, request: &EpisodeRequest) -> Result<WorkerAssignment, ScheduleError> {
    // 1. O(1) 按 env_type 查候选列表
    let candidates = self.env_type_index
        .get(&request.env_type)
        .ok_or(ScheduleError::NoWorkerForEnvType)?;
    // 2. 从候选中选最小负载的健康 Worker（Least Connections）
    let best = candidates.iter()
        .filter_map(|id| self.workers.get(id))
        .filter(|w| w.current_load < w.capacity && is_worker_healthy(&w))
        .filter(|w| resource_matches(&w, &request.resource_spec))  // 资源过滤
        .min_by_key(|w| w.current_load);
    // ...
}

// 新增资源匹配（为多模态调度铺垫）
fn resource_matches(worker: &WorkerInfo, spec: &ResourceSpec) -> bool {
    if spec.gpu_count > 0 && worker.resource.gpu_count < spec.gpu_count { return false; }
    if spec.gpu_memory_mb > 0 && worker.resource.gpu_memory_mb < spec.gpu_memory_mb { return false; }
    for cap in &spec.required_capabilities {
        if !worker.capabilities.contains(cap) { return false; }
    }
    true
}
```

#### 2.2 Server 主备 HA

引入 etcd 做 Leader Election，两个 Server 实例共享 PostgreSQL 状态存储：

```
Workers/Agents ──► VIP:50051 ──► Server-A (主)  ◄──etcd Leader Election──► Server-B (备)
                                      │                                           │
                                      └──────── PostgreSQL (共享状态) ────────────┘
```

Worker 注册地址配置为 VIP，主备切换时 VIP 漂移，Worker 感知重连后重新注册即可。Server-B 从 PostgreSQL 恢复全量状态，无需从 Worker 重新注册。

#### 2.3 Region Agent 层（Rack-Aware 分层调度）

1000台 Worker 需要在 Server 和 Worker 之间加一层聚合，每个机架部署一个 Region Agent：

```
uenv-server（全局调度，处理跨 Rack 决策）
    ├── Region-Agent-A（Rack-01，管理 50 台 Worker）
    │       ├── Worker-001 ... Worker-050
    ├── Region-Agent-B（Rack-02，管理 50 台 Worker）
    │       ├── Worker-051 ... Worker-100
    └── Region-Agent-C（...）
```

**Region Agent 核心职责**：
- 聚合心跳：50 个 Worker → 1 个聚合流到 Server（减少 Server 连接数 50x）
- 本地快速重试：Worker 失败时在本 Rack 内选另一台重试，不经 Server 往返
- 能力聚合：向 Server 汇报本 Rack 的总容量和已用量，Server 只做 Rack 级调度

```proto
// 新增 region_agent.proto
service RegionAgentService {
    rpc RegisterRegion(RegisterRegionRequest) returns (RegisterRegionResponse);
    // 聚合心跳：50个Worker的心跳聚合成1个到Server
    rpc RegionHeartbeat(stream RegionHeartbeatRequest) returns (stream RegionHeartbeatResponse);
    // Server 向 Region 分发，由 Region 内部选择具体 Worker
    rpc DispatchToRegion(EpisodeRequest) returns (stream StreamReport);
}

message RegionHeartbeatRequest {
    string region_id       = 1;
    string rack_id         = 2;
    int32 total_capacity   = 3;
    int32 current_load     = 4;
    repeated WorkerStatus worker_statuses = 5;  // 各 Worker 状态摘要
}
```

#### 2.4 ResourceSpec 扩展（异构资源和多模态调度基础）

```proto
// common.proto — 向后兼容扩展
message ResourceSpec {
    int32  cpu_cores     = 1;
    int32  memory_mb     = 2;
    int32  gpu_count     = 3;
    string gpu_type      = 4;
    // Phase 2 新增
    int32  gpu_memory_mb = 5;   // 单卡显存需求（8B模型 vs 70B模型差距巨大）
    string affinity_rack = 6;   // 期望调度到的机架（数据局部性）
    int32  network_bw_gbps = 7; // 网络带宽需求（大 Payload 传输）
    repeated string required_capabilities = 8;  // 能力标签: ["vision","audio","lammps","physics_sim"]
}
```

Worker 注册时通过 `nvidia-smi` / ROCm 接口自动填写实际 GPU 显存；`required_capabilities` 由 Worker 配置文件声明，由调度器在候选过滤时检查。

#### 2.5 Prometheus PushGateway（解决 500台 scrape 问题）

当前：Server 每隔 15s 拉取 N 个 Worker 的 `:19090`，500台时 scrape 时间超过采集间隔。

改为 Push 模式：
```
Worker → PushGateway (1个) → Prometheus → Grafana
```

Worker 侧 `metrics.rs` 改为每 15s 主动 push，Prometheus 只需拉取一个端点。Region Agent 可以在推送前做初步聚合（各 Worker 按 rack 分组）。

**Phase 2 验收指标**：
- 500台 Worker，Server 调度延迟 p99 < 10ms
- Server 主备切换 < 30s，无 Episode 丢失
- Grafana 可查每台 Worker 的 Episode 吞吐率和 GPU 利用率
- 同 Rack 内 Worker 故障时本 Rack 重试，不上报 Server（减少 Server 流量 80%）

---

### Phase 3：分布式架构（1000台 → 万台）

**目标**：跨数据中心 Federation、多模态大 Payload 流式传输、全链路 Trace。
**周期估计**：季度级

#### 3.1 Server Federation（跨 DC 联邦调度）

```
Global Scheduler（跨 DC 路由，一致性哈希按 batch_id 分配）
    ├── DC-A Server（管理 Region-Agent × 50，约 2500 台 Worker）
    ├── DC-B Server（管理 Region-Agent × 50，约 2500 台 Worker）
    └── DC-C Server（管理 Region-Agent × 100，约 5000 台 Worker）
```

训练框架提交 Episode 到任意 Global Scheduler 入口；按 `batch_id` 一致性哈希路由到固定 DC，保证同 batch 内所有 Episode 在同一 DC 处理（减少跨 DC 状态同步）。DC 内 Server 做具体 Rack 级调度，不感知其他 DC。

#### 3.2 大 Payload 旁路传输（多模态核心）

当前 gRPC 默认消息大小限制 4MB，一张图片 / 一帧视频就会超限。

**方案**：Object Store 旁路（S3 / MinIO / Ceph），Payload 走存储，gRPC 只传引用：

```
训练框架 ──上传 payload──► Object Store (MinIO)
    │
    │ EpisodeRequest.payload_ref = "s3://uenv/episodes/batch-xxx/ep-001.pb"
    ↓
uenv-server → Region-Agent → Worker
    │
    │ Worker 从 Object Store 下载 payload
    ↓
Plugin 执行（可能产生大体积 observation：图像序列、仿真轨迹）
    │
    │ observation 上传到 Object Store
    │ StepRecord.observation_ref = "s3://uenv/obs/ep-001/step-003.jpg"
    ↓
训练框架
```

```proto
// episode.proto 扩展（向后兼容）
message EpisodeRequest {
    // ... 现有字段 ...
    optional string payload_ref      = 20;  // Object Store URI，与 payload 字段互斥
    optional string payload_checksum = 21;  // SHA256，完整性校验
    optional PayloadFormat payload_format = 22;
}

enum PayloadFormat {
    PAYLOAD_FORMAT_UNSPECIFIED = 0;
    JSON        = 1;
    PROTOBUF    = 2;
    IMAGE_JPEG  = 3;
    IMAGE_PNG   = 4;
    VIDEO_MP4   = 5;
    AUDIO_WAV   = 6;
    POINT_CLOUD_PLY = 7;  // 3D 仿真点云
    NUMPY_NPY   = 8;      // 科学计算数组
}

// 同样扩展 StepRecord
message StepRecord {
    // ... 现有字段 ...
    optional string observation_ref    = 20;
    optional string observation_format = 21;
}
```

Worker 的 `episode/executor.rs` 在 `execute_episode` 开头：优先读 `payload_ref`，fallback 到内嵌 `payload`。

#### 3.3 WAL 升级为分布式（跨机恢复）

当前 WAL 存在 Worker 本地 `/tmp/uenv/wal`，Worker 宕机即丢。

**分级方案**：
- **千台级**：WAL 写入 Redis Stream（`XADD uenv:wal:<worker_id> ...`），TTL 24h 自动清理，零运维
- **万台级**：WAL 写入 Kafka（按 `worker_id` 分区），Consumer Group 负责 replay，支持跨机房

Worker 宕机时，Region Agent 从 Redis/Kafka 中读取该 Worker 的未完成 Episode，转发给同 Rack 其他 Worker 重试。

#### 3.4 全链路分布式 Trace（OpenTelemetry）

在现有 `correlation_id` 基础上，加 OTLP 导出到 Jaeger/Tempo：

```rust
// 所有 Rust crate 新增
[dependencies]
opentelemetry = "0.27"
opentelemetry-otlp = "0.27"
tracing-opentelemetry = "0.28"

// worker/episode/executor.rs 关键 Span
let span = tracer.start_with_context("episode.execute", &parent_ctx);
span.set_attribute(KeyValue::new("episode_id", episode.episode_id.clone()));
span.set_attribute(KeyValue::new("env_type", episode.env_type.clone()));
span.set_attribute(KeyValue::new("worker_id", ctx.worker_id.clone()));
span.set_attribute(KeyValue::new("warmup_hit", lease.warmup_hit.to_string()));
// ... 执行各阶段加子 Span: acquire / reset / step / model_call / reward ...
```

完整链路：`verl(batch) → uenv-bridge → adapter-core → server → region-agent → worker → plugin → llm-call`，全程一个 trace，在 Jaeger 中可以看到每个阶段耗时。

**Phase 3 验收指标**：
- 10000台 Worker，Global Scheduler 调度延迟 p99 < 50ms
- 单个 Episode payload 支持最大 1GB（通过 Object Store 旁路）
- 全链路 Trace 覆盖 verl batch → Plugin 执行 → LLM 回调
- Worker 宕机后未完成 Episode 在 60s 内自动转移重试

---

## 四、多模态专项设计

多模态与纯文本有三点本质差异，需要专项对待：

### 4.1 各模态 Episode 的数据量级

| 模态 | 单次 Episode 数据量 | 关键瓶颈 |
|------|-------------------|---------|
| 纯文本（数学/代码） | <64KB | 无，当前架构够用 |
| 图像 VLM | 1-10MB（每步 observation 为图像） | gRPC 4MB 限制 |
| 视频理解 | 10-500MB（视频片段） | 传输带宽、存储 |
| 3D 仿真（LAMMPS/MuJoCo） | 100MB-10GB（仿真轨迹） | 存储、计算时间 |
| 多模态 Agent（图+文+动作） | 可变，多 turn 累积 | 多 turn 状态管理 |

Phase 1/2 聚焦文本。Phase 3 的 Object Store 旁路是多模态数据传输的基础设施。

### 4.2 异构 Worker 资源矩阵

不同 env_type 对 Worker 硬件需求差距极大：

| env_type | CPU | GPU | 内存 | capability 标签 |
|----------|-----|-----|------|----------------|
| math / code | 2-4核 | 无 | 4GB | — |
| vision (VLM 推理) | 8核 | 1x A100/H100 | 40GB+ | vision |
| lammps（分子动力学）| 32-64核 | 可选 | 64-256GB | lammps, hpc |
| mujoco（物理仿真）| 8核 | 1x GPU (渲染) | 16GB | physics_sim |
| multi-modal agent | 16核 | 4x GPU | 80GB+ | vision, audio |

调度器在 Phase 2 引入 `required_capabilities` 后，Episode 请求中带上所需标签，调度器只把任务分配给能力匹配的 Worker，避免类型错配导致 Episode 失败。

### 4.3 Model Gateway 多路由扩展

随着多模态引入，一个训练 Job 可能同时需要多种模型：
- 语言模型（LLM）：处理文本推理
- 视觉语言模型（VLM）：处理图像 observation
- 奖励模型（Reward Model）：评判 trajectory 质量

```python
# model_gateway.py 扩展（纯 Python 层改动）
@dataclass
class ModelGatewayConfig:
    enabled: bool = False
    bind_host: str = "0.0.0.0"
    port: int = 18080
    # Phase 3 新增：多路由组
    routes: dict = field(default_factory=lambda: {
        "default": [],   # 默认 LLM
        "vision": [],    # VLM 端点
        "reward": [],    # 奖励模型
    })
```

EpisodeRequest 的 `model_endpoint` 支持 `gateway://vision` 格式，Gateway 按路由组分发。

### 4.4 多模态 Plugin 容器化策略

| 方案 | 启动延迟 | 隔离性 | 适用场景 |
|------|---------|--------|---------|
| Process（当前） | <1s | 无 | 文本/数学，无副作用 env |
| Podman 容器 | 5-30s | 强 | LAMMPS、物理仿真、需要特定依赖 |
| 预热容器池 | <1s（命中时）| 强 | 高频调用的多模态 env |

Phase 2 完善 `backend/podman.rs`（当前代码存在但未经生产验证），支持容器预热池（warmup_pool 现有机制可直接复用，改为容器 ID 而不是进程 PID）。

---

## 五、关键架构决策

### Q1：为什么不用 Kubernetes 做 Episode 调度？

K8s 调度粒度是 Pod（整机或整 GPU），UEnv 调度粒度是 Episode（一台机器同时跑 N 个 Episode）。两层 scheduler 会产生不一致（K8s 认为 Pod 可用，UEnv 发现 max_concurrent 已满）。

**建议**：K8s 只用于 Worker 进程的部署和自动重启（DaemonSet），Episode 调度完全由 UEnv Server/Region Agent 负责。

### Q2：要不要引入消息队列（Kafka/Pulsar）？

Phase 1/2 **不引入**。理由：
- 当前 Episode dispatch 是同步 RPC（Server 直连 Worker），MQ 会把同步变异步，调用链追踪复杂度大幅上升
- WAL + 幂等重试机制已覆盖 MQ 要解决的可靠性和幂等问题
- Phase 3 WAL 存储层可以用 Kafka，但**不暴露到 Episode dispatch 主路径**

### Q3：Region Agent 是必须的吗？

- 100台以内：**不需要**，Server 直连 100 个 Worker 完全可控
- 200-500台：**建议引入**，主要解决心跳连接数问题
- 1000台以上：**必须**，否则 Server 端的心跳处理成为瓶颈

### Q4：SQLite 够用到什么规模？

理论上够用到单 Server 处理的任何规模（SQLite WAL 模式支持高并发读，写操作序列化但 Server 写频率不高）。SQLite 的上限不是性能，而是**高可用**——Phase 2 切换到 PostgreSQL 的主要驱动是 HA 需求，而不是性能。

---

## 六、改动优先级总表

| 优先级 | 改动 | 当前代码位置 | Phase | 解决问题 |
|--------|------|------------|-------|---------|
| ✅ 已实现 | 心跳/业务存活双维度检测 | scheduler/mod.rs `is_worker_degraded` | 1 | 假活 Worker 继续接收任务 |
| ✅ 已解决 | idempotency_keys 清理 | server/control_plane.rs（已有） | 1 | 现有代码在 report_result 处理后立即 remove，不存在无界增长，此条无需改动 |
| ✅ 已实现 | episode_broadcast 容量提升 | server/config.rs `broadcast_capacity` 字段（默认 1024，可配置） | 1 | 高并发事件丢失 |
| P1 | Server 状态 SQLite 持久化 | server/persist/（新增） | 1 | 重启状态丢失 |
| ✅ 已实现 | 有界待调度队列 + 背压 | server/service.rs + state.rs（动态 Semaphore，2026-06-21） | 1 | 无界等待 OOM |
| P1 | Model Gateway -> nginx | uenv-bridge/model_gateway.py | 1 | 100+ 并发 / 流式输出 |
| P2 | 调度器 Vec->DashMap + 索引 | server/scheduler/mod.rs | 2 | 全局写锁争用 |
| P2 | ResourceSpec 扩展 | proto/common.proto | 2 | 多模态异构调度 |
| P2 | Server 主备 HA | server/ + etcd + pg | 2 | Server 单点 |
| P2 | Region Agent 层 | 新增 uenv-region-agent/ | 2 | 心跳风暴 / 分层调度 |
| P2 | Prometheus PushGateway | worker/metrics.rs | 2 | 500台 scrape 崩溃 |
| P2 | WAL 写入 Redis Stream | worker/wal/ | 2 | Worker 宕机数据丢失 |
| P3 | Object Store 大 Payload 旁路 | proto/episode.proto + worker/executor | 3 | 多模态 4MB 限制 |
| P3 | OpenTelemetry 全链路 Trace | 所有 Rust crate | 3 | 万台可观测性 |
| P3 | Server Federation | 新增 uenv-global-scheduler/ | 3 | 跨 DC 万台扩展 |
| P3 | Plugin 容器化预热池 | worker/backend/podman.rs | 3 | 复杂 env 快速启动 |

---

## 七、本周可立即落地的最高价值改动

以下改动不破坏现有功能，代码量小，但收益高：

**1. ~~`seen_idempotency` 加过期清理~~（已解决，无需改动）**

> **⚠️ 勘误**：方案原文将此列为 P0，但实际代码已解决此问题。
> `control_plane.rs` 的 `report_result` 底部已有：
>
> ```rust
> // 结果已处理完毕，从幂等集合中删除该 key，避免长期运行内存无限增长。
> { let mut seen = self.state.seen_idempotency.lock(); seen.remove(&req.idempotency_key); }
> ```
>
> key 在每次处理后**立即删除**（即时删除，而非 TTL 过期），不存在无界增长问题。
> 此条改动**不需要做**。

**2. ~~Worker 健康检测加业务存活维度~~（已实现）**

> **⚠️ 勘误：已实现**，无需改动。
> `scheduler/mod.rs` 的 `is_worker_degraded` 已有双维度判断：
> - 心跳超时（`last_heartbeat_at` > `heartbeat_timeout_secs`）
> - 业务假活（`current_load > 0` 且 `last_report_at` > `degraded_threshold_secs`）
>
> 与本节方案唯一区别是没有 `consecutive_episode_failures` 字段，但核心功能已覆盖。

**3. ~~`episode_broadcast` 容量改为配置驱动~~（已实现）**

> **⚠️ 勘误：已实现**，无需改动。
> `config.rs` 已有 `broadcast_capacity: usize`（默认 1024），`state.rs` 已使用 `config.episode.broadcast_capacity.max(1)` 创建 channel。
> 如需调整，在 `server.yaml` 加 `episode.broadcast_capacity: 4096` 即可。

> **⚠️ 勘误（原方案）**：原方案 `(worker_count * 32).max(1024)` 存在两个问题：
> 1. `ServerState::new()` 初始化时尚无 Worker 注册（worker_count = 0），表达式恒等于 `.max(1024)` 即保持现状，无实际效果
> 2. `broadcast::channel` 创建后容量固定，Tokio 不支持运行时调整
>
> **正确做法**：从配置文件读取广播容量，启动时传入：
>
> ```rust
> // config/server.toml 新增
> [episode]
> broadcast_capacity = 4096   // 按预期最大 Worker 数 x 32 估算后写入配置
>
> // server/state.rs 修改 new() 签名
> pub fn new(scheduler: Arc<RwLock<RoundRobinScheduler>>, broadcast_capacity: usize) -> Self {
>     let (episode_broadcast, _) = broadcast::channel(broadcast_capacity.max(1024));
>     // ... 其余不变
> }
> ```

**4. Model Gateway 日志改为异步批量写（约 40 行代码）**

把 `_record()` 方法的同步 `file.open/write/close` 改为 `mpsc::channel` + 后台线程批量 flush，高并发下日志不再成为锁点。
