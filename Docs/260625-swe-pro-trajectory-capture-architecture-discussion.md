# SWE-bench Pro 轨迹捕获 — 冻结方案（Worker 本地真值）

> **文档版本**：v1.1（**已实现并联调**）  
> **日期**：2026-06-25  
> **状态**：7143 验收通过 — gold reward=1.0 + LLM 真实跑通 + 轨迹 GET 可查  
> **关联**：`260618-swe-bench-env-hub-worker-plan.md` §1.7 / §5.3、`260624-swe-bench-pro-7143-联调报告.md`、`secrets/README.md`

---

## 0. 背景

7143 已跑通 **OpenHands → Runtime Gateway → Pro grader**（gold reward=1.0），但 **step 级轨迹未持久化**。本文冻结 **一种** 直观可用的方案：**轨迹真值、索引、查询 API 全部落在执行该 session 的 Worker 本机**；**不**做 Server/Hub 聚合层，**不**分 Phase。

---

## 1. 冻结方案总览

```text
OpenHands / UEnvRuntime
  → POST /runtime/v1/sessions/{id}/exec|read|write   （每步追加 StepTrace）
  → POST /runtime/v1/sessions/{id}/submit            （seal + 落盘 + 返回 TrajectoryRef）
  → GET  /runtime/v1/trajectories/{id}               （按 ID 拉完整 bundle）
  → GET  /runtime/v1/trajectories?instance_id=&limit=  （本 Worker 本地列表）

落盘目录（Worker 本机，环境变量 UENV_SWE_ARTIFACT_DIR）：
  index/by-id/{trajectory_id}.json    ← 轻量索引（即「索引服务」）
  bodies/{trajectory_id}.json         ← 完整 TrajectoryBundle（含逐步详情）

Server / Hub：不参与轨迹存储与查询。
WAL：不参与 Gateway 轨迹（见 §5.4）。
OpenHands 客户端：不强制本地 copy；持 TrajectoryRef 即可回查 Worker。
```

**分布式含义**：哪台 Worker 跑的 session，轨迹就在哪台 Worker 的磁盘上。上层模块必须保存 **`TrajectoryRef` 全字段**（含 `worker_id`、`gateway_base_url`、`trajectory_id`），需要 body 时向 **对应 Worker 的 Gateway** 发起 GET。跨 Worker 检索 = 调用方自行汇总已知的 ref 列表，或依次问各 Worker 的 list API（无中心索引）。

---

## 2. 数据模型

### 2.1 StepTrace（逐步）

```rust
struct StepTrace {
    step_index: u32,
    kind: StepKind,              // Exec | Read | Write | ProvisionReset | Submit
    action: StepAction,
    observation: StepObservation,
    timestamp_ms: u64,
    duration_ms: u64,
}

enum StepAction {
    Exec { command: String },
    Read { path: String },
    Write { path: String, content: String },   // 冻结：存完整 content，见 §4.1
}

struct StepObservation {
    stdout: String,
    stderr: String,
    exit_code: Option<i32>,
    truncated: bool,
    read_content: Option<String>,   // Read 步骤的文件内容
    write_ok: Option<bool>,
}
```

### 2.2 TrajectoryBundle（episode 级）

```rust
struct TrajectoryBundle {
    trajectory_id: String,
    session_id: String,
    instance_id: String,
    benchmark_variant: String,
    worker_id: String,
    gateway_base_url: String,    // 例 http://219.147.100.43:28999
    steps: Vec<StepTrace>,
    artifact: EpisodeArtifact,   // git_diff, test_results, reward（终局）
    sealed_at_ms: u64,
}
```

### 2.3 TrajectoryRef（submit 响应 + 索引文件内容）

```json
{
  "trajectory_id": "trj-worker-7143-pro-1719300000-00042",
  "worker_id": "worker-7143-pro",
  "gateway_base_url": "http://219.147.100.43:28999",
  "instance_id": "instance_NodeBB__...",
  "benchmark_variant": "pro",
  "session_id": "sess-...",
  "step_count": 3,
  "reward": 1.0,
  "resolved": true,
  "sealed_at_ms": 1719300123456
}
```

ID 格式：`trj-{worker_id}-{unix_ms}-{seq}`。`worker_id` 来自 Worker 配置（`worker.id` 或 hostname 派生）。

### 2.4 落盘布局

```text
${UENV_SWE_ARTIFACT_DIR}/                    # 7143 建议 /var/lib/uenv/swe-artifacts
  index/by-id/{trajectory_id}.json           # TrajectoryRef（索引条目）
  bodies/{trajectory_id}.json                # TrajectoryBundle（正文）
```

**索引服务首站**：就是 **Worker 本机 `index/by-id/` 目录 + Gateway list/get API**。不另起 Server/Hub 服务；查询 API 与写 session 共用同一 Gateway 进程与鉴权。

---

## 3. Gateway API（冻结）

| 方法 | 路径 | 行为 |
|------|------|------|
| POST | `/runtime/v1/sessions` | 创建 session；可选在 step 0 记 Reset observation |
| POST | `/runtime/v1/sessions/{id}/exec` | 执行命令 → 追加 StepTrace(Exec) |
| POST | `/runtime/v1/sessions/{id}/read` | 读文件 → 追加 StepTrace(Read) |
| POST | `/runtime/v1/sessions/{id}/write` | 写文件 → 追加 StepTrace(Write)，**content 写入 trace** |
| POST | `/runtime/v1/sessions/{id}/submit` | evaluate → seal bundle → 写 index + body → 响应含 `trajectory_ref` |
| GET | `/runtime/v1/trajectories/{id}` | 返回完整 `TrajectoryBundle` JSON |
| GET | `/runtime/v1/trajectories` | 查询参数：`instance_id`、`since_ms`、`limit`；扫描 `index/by-id/` |
| DELETE | `/runtime/v1/sessions/{id}` | 释放容器（轨迹已 submit 则保留落盘文件） |

**鉴权（冻结）**：除 `/health` 外，**所有路由**（含 trajectories GET）使用与 session **相同的 `X-API-Key`**。不单独发 read-only token。理由见 §4.3。

---

## 4. 设计决策说明（原「待决事项」已冻结）

### 4.1 step 里是否存完整 `FileWriteAction.content`？

**冻结：存完整 `content`。**

#### 该字段从哪来

在 OpenHands 语义里，`FileWriteAction` 表示 Agent **向沙箱内某路径写入一段文本**。UEnv 侧对应 Gateway：

```http
POST /runtime/v1/sessions/{id}/write
{ "path": "/tmp/agent.patch", "content": "<整段 unified diff 或源文件正文>" }
```

典型来源：

| 场景 | content 是什么 |
|------|----------------|
| **Gold 回放**（`run_swebench.py --gold`） | catalog 里该 instance 的 **完整 gold patch**（unified diff，可能含多文件 hunks） |
| **真实 LLM Agent** | 模型生成的单文件内容、或 agent 拼出的 patch 片段；也可能是 `str_replace` 后的整文件 |
| **非 patch 写文件** | 临时脚本、配置、测试夹具等 Agent 自行创建的文件正文 |

数据流：

```text
OpenHands FileWriteAction.path / .content
  → UEnvRuntime.write(action)
  → UEnvGatewayClient.write(session_id, path, content)
  → Worker write_file：host 临时文件 + docker cp 进容器
  → （冻结后）StepTrace.action Write { path, content } 追加到 session buffer
```

因此 **`content` 是 Agent 编辑意图的原始载荷**，不是 Worker 事后从容器里 `cat` 出来的（虽然结果等价）。轨迹要回答的问题是：「Agent 这一步 **打算写什么**」，而不仅是「容器里最后有什么」。

#### 为什么会有「是否全量存」的讨论

1. **体积**：多文件 SWE patch、长日志型 stdout 可能让单条 trajectory JSON 变大；若每步 write 都存全文，磁盘按 episode 线性增长。  
2. **重复**：同一份 patch 可能在 OpenHands 进程内存、catalog JSON、Worker trace 各存一份。  
3. **安全**：content 偶发含 token、路径、内网地址（Agent 误写进文件）。  
4. **可替代方案**：只存 `path + sha256`，正文另存 blob 文件或要求客户端凭 hash 自证——实现更重，且离线 replay 要多一次 indirection。

#### 为何冻结为「全量存 content」

- SWE-bench / Pro 的 **patch 体量通常 KB 级**（7143 gold 实例远小于 256KB），全量存最简单、replay 最直观。  
- **Worker 已是唯一 canonical 存储**（§1），不在 OpenHands 侧强制双写，重复问题可控。  
- **stdout/stderr 仍受 `CommandPolicy.max_output_bytes` 截断**（已有 `truncated` 标志）；write 的 content 是 action 输入，不走过 exec 截断逻辑，但 SWE 场景下 write 体积可预期。  
- 若将来出现超大 write，可在实现层加 **单步 content 上限**（超限拒绝或截断并标 `content_truncated`），**不改变**「以 content 字段为主体」的模型。

---

### 4.2 索引服务首站放哪？

**冻结：Worker 本机 `UENV_SWE_ARTIFACT_DIR/index/` + Gateway HTTP API。**

在放弃 Server 聚合的前提下，这是唯一自洽的选择：

| 职责 | 位置 |
|------|------|
| 写入索引 | `submit` 时 Worker 写 `index/by-id/{id}.json` |
| 按 ID 查 | `GET /runtime/v1/trajectories/{id}` 读 `bodies/{id}.json` |
| 列表/过滤 | `GET /runtime/v1/trajectories?instance_id=` 扫描本机 `index/by-id/` |

上层模块（OpenHands driver、评测脚本、后续 RL 管线）的 **持久化义务**：保存 `TrajectoryRef` JSON（至少 `trajectory_id` + `gateway_base_url` + `worker_id`）。查询时：

```text
持 ref → HTTP GET {gateway_base_url}/runtime/v1/trajectories/{trajectory_id}
       Header: X-API-Key: <与创建 session 相同>
```

多 Worker 时没有全局 catalog：**谁持有 ref，谁就能查**；要做跨 Worker 报表，由调用方合并多个 ref（或运维脚本轮询各 Worker list API）。

---

### 4.3 OpenHands 是否必须落本地 copy？Gateway 查询 API 鉴权？

#### OpenHands 本地 copy：**不必须**

| 角色 | 职责 |
|------|------|
| **Worker** | canonical：submit 后 bundle + index 已落盘 |
| **OpenHands / driver** | 只需在 run 结束时 **保存 `TrajectoryRef`**（一行 JSON 或写入 run manifest） |

**问题出发点**：早期草案曾考虑「OpenHands 是否也要把逐步 observation 写到本机 `./runs/`」，形成双份存储。动机包括：

- 离线分析时不连 Worker  
- OpenHands 自有 benchmark 框架习惯本地 `output.jsonl`  
- 担心 Worker 磁盘清理后丢失  

在 **Worker 本地真值** 方案下：

- 真源只有 Worker；OpenHands 再存一份 = **可选缓存**，非必需。  
- `run_swebench.py` 可提供 **`--save-ref path.json`**（只写 TrajectoryRef），**不**默认 dump 全量 body。  
- 需要全量时再用 ref 调 Gateway GET。

#### Gateway 查询 API 鉴权：**与 session 相同 `X-API-Key`**

**问题出发点**：trajectory GET 是 **只读**，是否应使用权限更弱的 token（只读 key），与可 `exec`/`write` 的 session key 分离？

**冻结理由（用同一 key）**：

1. **7143 已启用** `runtime_gateway.api_key: swe-pro-secret`；拆两套 key 增加部署与文档成本，收益有限。  
2. trajectory body 含 **patch、命令历史、测试输出**，只读泄露面与「能 exec 的 key」同级，只读 token **不能**当安全降级。  
3. 实现上 Gateway 已有 `require_api_key` 中间件，trajectories 路由 **挂同一 protected router** 即可，零额外配置。  
4. 若未来要公网暴露只读审计，再 **另立** 只读 key 或 mTLS；当前内网联调不前置。

---

### 4.4 与 WAL 的关系

#### 当前 WAL 记什么

WAL（`uenv-worker/src/wal/mod.rs`，schema 见 `proto/uenv/v1/wal.proto`）服务于 **Worker ↔ Server ControlPlane** 的 **`ReportResult` 可靠投递**，与 Gateway HTTP **无关**。

每条 WAL 文件（`{episode_id}__{attempt_id}__{worker_id}.wal`）核心是 `WalRecord`：

| 字段 | 含义 |
|------|------|
| `episode_id` / `attempt_id` / `worker_id` | 幂等键组成部分 |
| `dispatch_lease_id` / `server_epoch` | 调度租约 |
| `request_checksum` | 原始 `EpisodeRequest` 的 SHA-256 |
| `result_checksum` | `EpisodeResult` 的 SHA-256 |
| `status` | completed / failed / … |
| **`protobuf_payload`** | **完整 `EpisodeResult` 序列化 bytes**（含 `Trajectory` proto、`trajectory_checksum` 等） |
| `replay_state` | Pending → 重连 Server 后重放 ReportResult → Acked 删文件 |

触发路径：**仅** `DispatchEpisode`（gRPC）完成后、`ReportResult` 发送前/失败时 `persist_pending`。

对 **math / native swe** 的 `EpisodeResult.trajectory`：是 **proto `Trajectory`**（math 为多步 `StepRecord`；当前 native swe 为 **1 步 stub**），嵌在 WAL 的 `protobuf_payload` 里，目的是 **Server 断连时仍能补报训练侧结果**。

#### Gateway 轨迹要不要进 WAL？

**冻结：不进 WAL；两套存储各管各的。**

| 维度 | WAL | Gateway TrajectoryStore |
|------|-----|-------------------------|
| **触发路径** | Server `DispatchEpisode` | OpenHands HTTP session |
| **是否有 episode_id/attempt_id** | 有（Server 分配） | 无（仅 session_id / trajectory_id） |
| **消费者** | Server / VeRL / adapter-core | OpenHands、评测脚本、人工调试 |
| **内容形态** | proto `EpisodeResult` | JSON `TrajectoryBundle`（逐步 SWE 细节） |
| **重放语义** | 向 Server **ReportResult** | 无 ReportResult；**GET API 即查询** |

**问题出发点**：能否复用 WAL 统一「episode 产物」，避免两套目录？

不合并的原因：

1. OpenHands 路径 **不经 Server**，没有 `episode_id`，WAL 幂等键无法自然构造。  
2. WAL 设计目标是 **控制面结果上报**，不是 **大体积逐步日志库**；把 MB 级 JSON bundle 塞进 `protobuf_payload` 会膨胀 WAL、拖慢重放。  
3. SWE Gateway 轨迹需要 **逐步 exec/write/read**；WAL 里的 proto `Trajectory` 对 swe 目前只是 **1 步占位**，表达力不够。  
4. 职责清晰：**WAL = 训练控制面容错**；**ArtifactDir + Gateway GET = SWE 评测轨迹真值**。

native `DispatchEpisode(env_type=swe)` 若将来也要逐步轨迹，优先 **扩展 Gateway 同款 `TrajectoryBundle` 落盘**，WAL 里仍只放 **精简 `EpisodeResult` + `artifact_uri` / `trajectory_id` 指针**，不把逐步 JSON duplicate 进 WAL。

---

## 5. OpenHands vs VeRL（边界，不变）

```text
VeRL：7142 → adapter-core → DispatchEpisode → Worker（math/swe native）→ WAL + ReportResult
OpenHands Pro：Agent → Gateway HTTP → Worker 沙箱 → TrajectoryStore（本文方案）
```

OpenHands **不依赖** VeRL；Adapter-core **不参与** Gateway 路径。Worker 价值：池化、Pro 镜像/grader、**逐步轨迹落盘与查询**。

---

## 6. 实施清单

1. `SweSession`：`StepTrace` buffer；`exec`/`read`/`write` 追加。  
2. `submit`：seal → 写 `index/` + `bodies/` → `SubmitResp.trajectory_ref`。  
3. Gateway：`GET /trajectories/{id}`、`GET /trajectories`（同 API Key）。  
4. OpenHands：`SubmitResult.trajectory_ref`；`run_swebench.py --save-ref`（可选，不写全量 body）。  
5. 7143：`UENV_SWE_ARTIFACT_DIR=/var/lib/uenv/swe-artifacts`；同步代码；Pro gold 复验 + GET 抽检。

---

## 7. 验收标准

1. Gold Pro：`submit` 返回 `trajectory_id`，`step_count ≥ 2`（write + exec + …）。  
2. `GET /runtime/v1/trajectories/{id}` 含每步 `command`/`path`/`content`/`stdout`/`exit_code`。  
3. `index/by-id/{id}.json` 与 submit 响应一致。  
4. OpenHands 仅保存 ref 即可复现查询；不依赖 Server/Hub。

---

## 8. 7143 拓扑（摘自 secrets）

```text
OpenHands → http://127.0.0.1:28999 (Gateway, X-API-Key: swe-pro-secret)
         → SweInstancePool → Pro 容器 /app
         → /var/lib/uenv/swe-artifacts/   [本方案落盘]
Hub 8.130.95.176 — 仅 Pro catalog，不存轨迹
Server 8.130.86.71 — 不参与 OpenHands 路径
WAL /tmp/uenv/wal-swe — 仅 DispatchEpisode ReportResult，不存 Gateway 轨迹
```
