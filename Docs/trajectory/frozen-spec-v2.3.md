# UEnv 轨迹保存规范（冻结 v2.3）

> **状态**：冻结、实机在用  
> **适用范围**：SWE Runtime Gateway 路径（含 OpenHands driver）、VeRL native DispatchEpisode 路径、**非 SWE 通用 episode 路径（v2.3 新增）**  
> **Canonical 存储**：Server `8.130.75.157:8077`（HTTP）+ SQLite 索引；Worker 本地为 seal + 上传 spool  
> **代码真源**：`uenv-worker/src/swe/trajectory.rs`、`uenv-common/src/trajectory.rs`、`uenv-server/src/trajectory.rs`、`uenv-worker/src/episode/executor.rs`
>
> v2.3 相对 v2.2 的增量：**所有任务类型的轨迹都进集中存储**——非 SWE 通用 episode（math/code 等
> plugin step 循环）在 episode 终态（completed / failed / timeout 全部）seal 成同一个
> `TrajectoryBundle` 并复用同一上传旁路；`StepAction` 新增 `generic` kind；artifact 根目录新增
> `UENV_TRAJECTORY_ARTIFACT_DIR`（回退 `UENV_SWE_ARTIFACT_DIR`）；run_id 增加 metadata 传播约定。
> SWE 既有行为（seal、upload、ref 字段、spool 语义）不变。

---

## 1. 设计目标

1. **逐步可追溯**：Gateway 每次 exec/read/write 与 provision reset 形成 `steps[]`；通用 episode 每次 obs/action/reward 形成 `generic` 步骤。
2. **评测可关联**：`instance_id`、`reward`、`resolved`、`artifact.test_results` 与 SWE 评测对齐。
3. **作业可聚合**：`run_id`（一次 OpenHands/评测作业）、`batch_id` / `correlation_id`（VeRL 训练批次）。
4. **上传不阻断 reward**：seal 后异步 POST Server；失败写 spool，submit 仍返回 reward。
5. **Server 统一查询**：LIST/GET 只暴露 `upload_status=acked` 且 `body_present=1` 的轨迹。
6. **全任务类型覆盖（v2.3）**：非 SWE episode 同样 seal + 上传；**失败与超时终态也 seal 部分轨迹**，不丢训练数据。

---

## 2. 存储分层

```text
┌─────────────────────────────────────────────────────────────┐
│  Worker（ephemeral）                                           │
│  ${UENV_TRAJECTORY_ARTIFACT_DIR:-UENV_SWE_ARTIFACT_DIR}/      │
│    bodies/{trajectory_id}.json     ← TrajectoryBundle 正文   │
│    index/by-id/{trajectory_id}.json ← TrajectoryRef 轻量索引 │
│    spool/pending|failed/{id}.json  ← 上传重试 marker          │
└───────────────────────────┬─────────────────────────────────┘
                            │ POST /control/v1/trajectories (gzip)
┌───────────────────────────▼─────────────────────────────────┐
│  Server（canonical）                                           │
│  ${data_dir}/                                                 │
│    trajectory.db                   ← SQLite 索引（可 LIST 过滤）│
│    bodies/{trajectory_id}.json     ← 与 Worker 上传正文一致     │
└─────────────────────────────────────────────────────────────┘
```

### 2.1 环境变量

| 变量 | 侧 | 含义 |
|------|-----|------|
| `UENV_TRAJECTORY_ARTIFACT_DIR` | Worker | **v2.3 新增**：本地 `bodies/`、`index/`、`spool/` 根目录；未设置时回退 `UENV_SWE_ARTIFACT_DIR` |
| `UENV_SWE_ARTIFACT_DIR` | Worker | artifact 根目录（v2.2 起；现为回退项，既有部署零改动） |
| `UENV_TRAJECTORY_ENDPOINT` | Worker | Server 轨迹 HTTP 根，如 `http://8.130.75.157:8077`；未配置时仅本地 seal 不上传 |
| `UENV_TRAJECTORY_TOKEN` | Worker / Server | 请求头 `X-Trajectory-Token` |
| `trajectory.data_dir` | Server | SQLite + bodies 根（YAML 或部署配置） |

`TrajectoryStore` 与 `TrajectoryUploader` 用同一解析函数取根目录，保证 spool 与 bodies 始终同根。

---

## 3. 标识符

### 3.1 `trajectory_id`

- **格式**：`trj-{worker_id}-{unix_ms}-{seq5}`  
  例：`trj-worker-7143-pro-1783244550494-00001`
- **生成**：Worker `TrajectoryStore::next_trajectory_id()`，进程内单调递增 seq。
- **唯一性**：全局 PK（Server SQLite + Worker 文件名）。

### 3.2 `run_id`

- **含义**：一次完整评测作业 ID（OpenHands driver 启动时生成，或 native 路径用 `correlation_id`）。
- **注入（SWE / Gateway）**：HTTP 头 `X-UEnv-Run-Id` → Gateway session → 写入 bundle。
- **注入（非 SWE 通用 episode，v2.3）**：adapter-core 把 sample context 的 `training_run_id` /
  `batch_id` / `run_id` 写入 `EpisodeRequest.metadata`（proto 字段 21），Server dispatch 原样透传；
  Worker seal 时按以下优先级解析：  
  `metadata["run_id"]` → `metadata["training_run_id"]` → `metadata["batch_id"]` → `run-{episode_id}`（兜底）。
- **run-task 评测路径**：`uenv-bridge` Python `evaluate.py` → adapter-core `ExecuteBatch`；
  `batch_id` 为 envelope 必填字段且必入 metadata，故该路径至少落到 `batch_id` 一级，无需额外补键。
- **Server 入库**：**必填**（POST 校验）；LIST 可按 `run_id` 过滤。

---

## 4. 文件类型与字段

### 4.1 `TrajectoryBundle`（`bodies/{trajectory_id}.json`）

完整 episode 轨迹，JSON 对象。顶层字段：

| 字段 | 类型 | 必填 | 含义 |
|------|------|------|------|
| `trajectory_id` | string | ✓ | 与文件名一致 |
| `run_id` | string | ✓（上传 Server 时） | 一次评测/作业 ID |
| `batch_id` | string \| null | | VeRL batch；OpenHands 常为 null |
| `correlation_id` | string \| null | | 训练 run / 可视化关联 ID |
| `episode_id` | string \| null | | Server 分配的 platform episode；Gateway 路径常为 null |
| `session_id` | string | ✓ | Gateway SWE session；**通用 episode（v2.3）填 warmup 环境实例句柄** |
| `instance_id` | string | ✓ | SWE-bench 实例 ID；通用 episode 同 session_id |
| `benchmark_variant` | string | ✓ | 如 `pro`、`verified`；**通用 episode 填 `env_type`**（如 `math`、`code`） |
| `worker_id` | string | ✓ | 注册 Worker ID |
| `gateway_base_url` | string | ✓ | 产生轨迹的 Gateway 基址（溯源）；**通用 episode 无 gateway，留空** |
| `steps` | array | ✓ | 逐步轨迹，见 §4.2 |
| `artifact` | object | ✓ | Episode 产物，见 §4.3 |
| `reward` | number | ✓（seal 后） | 评测得分 0.0–1.0；通用 episode 为累计 reward |
| `resolved` | boolean | ✓（seal 后） | 是否通过 SWE 评测；**通用 episode = 环境自然终止（terminated）** |
| `sealed_at_ms` | integer | ✓ | 封存 UTC 毫秒时间戳 |

### 4.2 `steps[]` — `StepTrace`

| 字段 | 类型 | 含义 |
|------|------|------|
| `step_index` | uint | 从 0 递增（通用 episode 取 proto step_index，从 1 起） |
| `action` | object | 见下表（`kind` 区分类型） |
| `observation` | object | 执行结果 |
| `timestamp_ms` | uint | 步开始时间（通用 episode 的 proto StepRecord 无时间戳，填 0） |
| `duration_ms` | uint | 步耗时 |

**`action`（`kind` 为 snake_case tag）**

| kind | 字段 | 含义 |
|------|------|------|
| `exec` | `command` | Shell 命令 |
| `read` | `path` | 读文件路径 |
| `write` | `path`, `content` | 写文件 |
| `provision_reset` | `issue_text` | 容器 provision 后 issue 摘要 |
| `generic`（v2.3） | `action`, `action_encoding` | 非 SWE 通用 episode 的动作（不透明字节）：UTF-8 可读按文本存储（encoding 省略），否则 hex 编码且 `action_encoding="hex"` |

**`observation`**

| 字段 | 类型 | 含义 |
|------|------|------|
| `stdout` | string | 标准输出 |
| `stderr` | string | 标准错误 |
| `exit_code` | int \| omit | 命令退出码 |
| `truncated` | bool | SWE：输出是否截断；**v2.3 通用 episode 复用该字段表示单步 episode 截断标记** |
| `read_content` | string \| omit | read 动作内容 |
| `write_ok` | bool \| omit | write 是否成功 |
| `raw` | string \| omit | **v2.3**：通用 episode 的原始观测（UTF-8 文本或 hex） |
| `raw_encoding` | string \| omit | **v2.3**：`"hex"` 或省略（=文本） |
| `reward` | number \| omit | **v2.3**：通用 episode 的单步 reward |
| `terminated` | bool \| omit | **v2.3**：通用 episode 的单步终止标记 |

新增 action kind 需同步修改 `uenv-worker/src/swe/trajectory.rs` 中 `StepAction` 枚举及对应记录逻辑。

### 4.3 `artifact` — `EpisodeArtifact`

| 字段 | 类型 | 含义 |
|------|------|------|
| `episode_id` | string | 常与 session 或 platform episode 对应 |
| `instance_id` | string | 实例 ID |
| `patch` | string \| omit | 候选 patch 文本 |
| `git_diff` | string \| omit | 容器内 git diff |
| `stdout_log` | string[] | 聚合 stdout |
| `stderr_log` | string[] | 聚合 stderr |
| `test_results` | object \| omit | 见下 |
| `reward` | number \| omit | 产物内 reward（可与顶层重复）；通用 episode 填累计 reward |
| `artifact_uri` | string \| omit | 外部存储 URI（预留） |

**`test_results`**

| 字段 | 类型 | 含义 |
|------|------|------|
| `passed` | bool | 全部测试是否通过 |
| `raw_output` | string | 原始测试输出 |
| `per_test` | [name, bool][] | 单测名 → 是否通过 |

### 4.4 `TrajectoryRef`（`index/by-id/{trajectory_id}.json`）

轻量索引，与 `uenv-common::TrajectoryRef` 一致；Gateway submit 响应、`GET /trajectories` LIST 项同构。

| 字段 | 类型 | 含义 |
|------|------|------|
| `trajectory_id` | string | |
| `worker_id` | string | |
| `gateway_base_url` | string | |
| `instance_id` | string | |
| `benchmark_variant` | string | |
| `session_id` | string | |
| `run_id` | string | |
| `storage_url` | string \| omit | Server 基址（上传成功后填） |
| `storage_kind` | `"worker"` \| `"server"` \| omit | 存储位置 |
| `step_count` | uint | steps 长度 |
| `reward` | number | |
| `resolved` | bool | |
| `sealed_at_ms` | uint | |
| `upload_status` | `"pending"` \| `"acked"` \| `"failed"` | Worker spool / Server 入库状态 |

### 4.5 Spool marker（`spool/pending|failed/{trajectory_id}.json`）

| 字段 | 类型 | 含义 |
|------|------|------|
| `attempts` | uint | 已重试次数（上限 10） |
| `last_error` | string | 最近一次失败原因 |

正文仍引用 `bodies/{trajectory_id}.json`，spool **不复制**大 JSON。

---

## 5. Server 索引字段（SQLite `trajectories` 表）

Server 从 bundle **解析** `TrajectoryHeader`（见 `uenv-common/src/trajectory.rs`），写入可查询列：

`trajectory_id`, `worker_id`, `instance_id`, `benchmark_variant`, `session_id`,  
`episode_id`, `run_id`, `batch_id`, `correlation_id`, `gateway_base_url`,  
`step_count`, `reward`, `resolved`, `sealed_at_ms`,  
`body_path`, `body_sha256`, `body_bytes`, `upload_status`, `body_present`, `created_at_ms`

**未进入索引的字段**（仅存在于 body JSON）：`steps` 全文、`artifact` 详情、以及下文所述扩展字段。

---

## 6. 生命周期

### 6.1 Gateway / OpenHands 路径（SWE）

```text
create_session（绑定 run_id）
  → exec / read / write（append StepTrace）
  → submit（评测 + seal）
       TrajectoryStore::seal → bodies + index
       TrajectoryUploader::enqueue → POST Server（gzip）
  → SubmitResponse { reward, trajectory_ref }
```

**OpenHands / Server 编排路径**：driver 经 Gateway submit；`CompleteAgentJob` 回填 `trajectory_id` 字符串；**不写** Server `episode_results` 表（无 platform `episode_id`）。

### 6.2 非 SWE 通用 episode 路径（v2.3）

```text
DispatchEpisode（env_type != "swe"）
  → acquire（登记 inflight 部分轨迹）
  → reset / step 循环（逐步累积 proto StepRecord 到 inflight）
  → 终态（全部 seal，失败/超时同等待遇）：
       completed            → seal（resolved = terminated）+ enqueue + 回填 EpisodeResult.trajectory_id
       failed（reset/step/model/reward/release 错误、async 校验失败）
                            → seal 已收集的部分轨迹；async failed result 同样回填 trajectory_id
       timeout（tokio 超时丢弃执行 future）
                            → worker_service 从 executor inflight 表取部分轨迹 seal + enqueue
       cancel               → 丢弃 inflight（server 已取消，不留存）
```

要点：

- inflight 部分轨迹保存在 `EpisodeExecutor` 的共享表中（key = `{episode_id}:{attempt_id}`），
  timeout 时执行 future 已被丢弃，只有该表能取到已收集 steps。
- 通用 seal 复用 SWE 同一个 `TrajectoryStore` / `TrajectoryUploader`（同一 drainer 线程、同一 spool）。
- `UENV_TRAJECTORY_ENDPOINT` 未配置时仅本地 seal：`EpisodeResult.trajectory_id` 照填，
  `trajectory_storage_url` 留空。
- SWE native DispatchEpisode 路径仍复用 pool submit 的 seal 结果，**不重复 seal**。
- Worker 回填的 `trajectory_id` / `trajectory_storage_url` 经 `ReportResult` 上报；Server
  `result_finalizer::persist_episode_result` 对所有 env_type 无差别写入 `episode_results` 表。
  注意：同步失败（gRPC 错误返回）与 timeout 终态没有 Worker 上报的 EpisodeResult，轨迹指针不进
  `episode_results`，但 bundle 已进集中存储，可按 `episode_id` / `run_id` 关联。

---

## 7. HTTP API（Server `:8077`）

| 方法 | 路径 | 说明 |
|------|------|------|
| POST | `/control/v1/trajectories` | 上传 bundle（可 gzip） |
| GET  | `/control/v1/trajectories/{id}` | 取正文 JSON |
| GET  | `/control/v1/trajectories` | LIST（Query: run_id, instance_id, …） |
| GET  | `/control/v1/trajectories/health` | 健康检查 |

POST 成功条件：必填字段齐全、`run_id` 非空、blob 落盘 + INSERT 成功 → `upload_status=acked`。

---

## 8. 扩展性说明

### 8.1 当前是否需要改代码

**不需要。** v2.3 已冻结并在实机使用；§8.2 所列能力由现有实现提供。**不必**为「预留扩展」提前改 Worker / Server 代码。

§8.3、§8.6 描述的是 **将来真要增加轨迹内容时** 的设计与改码范围，属于后续演进指南，非当前必做项。

### 8.2 现状：不改代码已具备的能力

| 能力 | 是否需要改代码 | 说明 |
|------|----------------|------|
| Worker `seal` → 写入标准 `TrajectoryBundle` | **否** | 按 `trajectory.rs` / `artifact.rs` 现有 struct 序列化即可 |
| Server POST → **整段 JSON 原样落盘** | **否** | `insert()` 按 HTTP body 字节写 `bodies/{id}.json` |
| Server 索引 → 解析 `TrajectoryHeader` 已知列 | **否** | LIST/过滤仅用 §5 索引字段 |
| GET body → 返回完整 JSON 文件 | **否** | 含上传时带入、但未进索引的顶层字段 |
| 不扩展新字段、维持 v2.3 契约 | **否** | 保持现状即可 |

### 8.3 后续扩展：何时需要改代码

| 扩展意图 | 是否要改代码 | 改哪里 |
|----------|--------------|--------|
| 在 `artifact` / `observation` 增加**可选**字段（检索仍靠现有索引列） | **要** | `artifact.rs` / `trajectory.rs` 增加字段 + `#[serde(default)]`；同步更新 `Docs/trajectory/` |
| 增加正式 `extensions: { ... }` 容器 | **要** | `TrajectoryBundle` 增加字段；若需 LIST 按扩展内容过滤，另改 `TrajectoryHeader` + SQLite schema |
| 新增 `action.kind`（新步类型） | **要** | `StepAction` 枚举 + 对应记步逻辑 |
| POST body 临时带顶层未知字段（**不经过** Worker `seal`） | **可不改 Worker** | 自定义上传客户端；索引/LIST 仍不可见这些字段 |
| 经 Worker `seal` 持久化顶层自定义字段 | **要** | 必须在 `TrajectoryBundle`（或嵌套 struct）中声明，否则 `seal` 不会写出 |
| 按新字段做 Server LIST / retention / 监控 | **要** | `TrajectoryHeader`、`trajectories` 表及 POST 校验 |

**原则**：仅 **GET 可读、不参与查询** 的扩展，可考虑只写入 body（或 POST 自定义 body）；凡 **Worker 正常 seal 路径** 或 **Server 索引/LIST** 要用到的字段，都必须改代码并升文档版本。

### 8.4 扩展方式与支持程度（摘要）

| 扩展方式 | 是否支持 | 说明 |
|----------|----------|------|
| 在 bundle **顶层**增加额外 JSON 字段 | **部分支持** | Server **原样保存** body 字节；**不会**进入 SQLite 索引与 LIST 过滤 |
| 在 `artifact` 内增加可选字段 | **推荐（后续）** | Rust 类型加 `#[serde(default)]`；旧读者忽略；**实施时需改代码** |
| 在 `observation` 内增加可选字段 | **推荐（后续）** | 同上，需改 `StepObservation` |
| 新增 `action.kind` | **需协议升级** | 扩展 `StepAction` 枚举 + 写步逻辑 |
| 仅 Worker 本地 seal 路径写扩展字段 | **不支持（不改代码时）** | `TrajectoryStore::seal` 只序列化已声明字段 |
| 自定义上传 POST body（绕过 Worker seal） | **支持存根** | Server 存完整 JSON；索引仍只认 `TrajectoryHeader` 已知列 |

### 8.5 机制细节

**Worker 侧（强类型）**

- `TrajectoryBundle` / `EpisodeArtifact` / `StepTrace` 为 Rust struct，`serde` 默认 **反序列化忽略未知字段**，但 **序列化只输出已声明字段**。
- 因此：经 Gateway seal 的路径 **不能** 在不变更代码的情况下持久化额外顶层字段。

**Server 侧（blob 优先）**

- `insert()` 将 HTTP body **按字节**写入 `bodies/{id}.json`，再用 `TrajectoryHeader::deserialize` 提取索引列。
- `TrajectoryHeader` 含 `steps: Vec<IgnoredAny>`，只计数、不解析步内容；**顶层未知字段被 serde 忽略**，但 **仍保留在文件中**。
- GET `/trajectories/{id}` 返回 **完整文件内容**，含扩展字段。

**查询与可视化**

- LIST / SQL 只能按 §5 索引列过滤；扩展字段 **不可检索**，除非升级规范并迁移 DB schema。
- 可视化 `events.db` 与轨迹 body **独立**；扩展字段不会自动进入 events。

### 8.6 推荐扩展做法（后续演进时采用）

1. **向后兼容字段（首选）**  
   在 `TrajectoryBundle` 或 `EpisodeArtifact` 增加 `Option<T>` / 带 `default` 的字段 → 发版 Worker + 更新本文档版本号。

2. **命名空间式扩展（中期）**  
   增加正式字段如 `extensions: { "openhands": { ... } }`（需在 Rust 中显式建模为 `serde_json::Value` 或 nested struct），避免与顶层索引字段冲突。

3. **非规范数据**  
   放入 `artifact`（如 `artifact_uri` 指向对象存储）或 `stdout_log` / `test_results.raw_output`，不破坏索引契约。

4. **禁止**  
   在未升级 `TrajectoryHeader` 的情况下依赖自定义顶层字段做 Server 端 LIST 过滤或 retention 策略。

### 8.7 版本演进

- 当前 **无** bundle 内 `schema_version` 字段；版本由仓库文档 + Git 管理。
- 若未来引入 `schema_version`，建议：`"schema_version": "2.3"` 顶层字段，Server POST 校验兼容范围。

---

## 9. 与 proto `Trajectory` 的关系

VeRL native 路径在 gRPC `ReportResult` 中另有一套 `Trajectory` / `StepRecord`（bytes observation/action，见 `proto/uenv/v1/episode.proto`）。  
SWE Gateway 路径 **不使用** proto Trajectory 落盘，而使用本文 **JSON TrajectoryBundle**。  
native 路径在 report 前将轨迹 seal 为 bundle 并 upload，再填 `EpisodeResult.trajectory_id`：SWE 复用
pool submit 的 seal 结果；**非 SWE 通用 episode（v2.3）由 executor 把 proto StepRecord 映射为
`generic` StepTrace 后 seal**（失败/超时终态 seal 部分轨迹）。

---

## 10. 参考实现位置

| 组件 | 路径 |
|------|------|
| Bundle / Step 类型 | `uenv-worker/src/swe/trajectory.rs` |
| Artifact | `uenv-worker/src/swe/artifact.rs` |
| 共享 Ref / Header | `uenv-common/src/trajectory.rs` |
| 本地 seal + list | `TrajectoryStore` |
| 上传 spool | `uenv-worker/src/swe/trajectory_upload.rs` |
| Server 入库 / GET | `uenv-server/src/trajectory.rs` |
| Session seal 入口（SWE） | `uenv-worker/src/swe/session.rs` → `seal_trajectory` |
| 通用 episode seal（v2.3） | `uenv-worker/src/episode/executor.rs` → `seal_inflight` |
| run_id metadata 传播 | `uenv-bridge/core/src/core.rs` → `sample_to_episode_request` |

---

## 11. 变更记录

| 版本 | 日期 | 说明 |
|------|------|------|
| v2.2 | 2026-06-25 | 冻结：Server 聚合、run_id、spool 上传、SQLite 索引 |
| v2.2-doc | 2026-07-05 | 从 `260625` 抽离为本目录可读规范；补充扩展性章节 |
| v2.2-doc.1 | 2026-07-05 | §8 补充「当前不必改代码 / 后续扩展改码范围」要点 |
| v2.3 | 2026-08-20 | 全任务类型轨迹进集中存储：`generic` action kind；非 SWE 通用 episode 在 completed/failed/timeout 全终态 seal + 上传；`UENV_TRAJECTORY_ARTIFACT_DIR`（回退 `UENV_SWE_ARTIFACT_DIR`）；run_id 的 metadata 传播约定与优先级 |
