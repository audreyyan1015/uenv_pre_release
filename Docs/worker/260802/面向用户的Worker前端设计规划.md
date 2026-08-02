# 面向用户的 Worker 前端设计规划

> **日期**：2026-08-02  
> **状态**：规划稿（仅 Worker 观测面；不改控制面 gRPC）  
> **受众**：训练/评测用户（非管理员、非联调开发者）  
> **继承现状**：
> - [260612-前端完整设计](../../discussions/可视化前端相关/260612-前端完整设计.md)（管理员观测台：工作流 + 对象树 + 日志/Metrics）
> - [2026-07-15-Server侧聚合与前端接入规划](../../discussions/可视化前端相关/2026-07-15-Server侧聚合与前端接入规划.md)（Obs 内嵌 Server；前端只消费 REST/SSE）
> - [前端观测面与系统能力差距](../260726/前端观测面与系统能力差距-待补齐.md)（现网 `TrainingConsole` 能力基线）
> - 实现入口：`frontend/src/components/training-console.tsx`、`frontend/src/lib/types/chain-state.ts`

---

## 1. 结论先行

| 项 | 决策 |
|----|------|
| 产品定位 | 在现有 **管理员观测台** 之外，新增 **用户任务台** 的 **Worker 执行面**（只读观测） |
| 相对现状的精简原则 | **砍诊断细节**（原始日志、事件刷屏、step 级树、运维 ID 堆砌）；**保留调度与进度语义** |
| 用户要一眼看懂的三件事 | ① 任务分到了哪些 Worker；② 各 Worker 忙闲与排队；③ Episode / Step 推进到哪、还要多久 |
| 数据入口 | 仍只连 **Obs REST + SSE**；不直连 Worker gRPC / `admin_http` / 文本日志 |
| 本期范围 | **仅规划 Worker 相关展示与所需投影字段**；Adapter 开战、Hub 管理、全链路日志 Tab **不在本期** |

---

## 2. 为什么要单独做「用户面」

现有前端（`TrainingConsole`）目标是 **联调与排障**：

- 主视觉：`SUBMIT → DISPATCH → EXECUTE → REPORT → DONE` 工作流图  
- 侧栏：`run → worker → env_instance → episode → step` 全量对象树  
- 底栏：日志 / 指标 / 事件流 / 快照 / 搜索（日志与 Metrics 仍多为占位）  
- 详情：`correlation_id`、`attempt_id`、心跳时间戳、失败分支哈希等运维字段优先

这对管理员/开发者合理，但对 **提交评测或训练任务的用户** 过载：

| 用户真正关心 | 当前界面倾向暴露 |
|--------------|------------------|
| 我的任务分到哪台执行节点、是否卡住 | 全链路模块名、内部 stage 枚举 |
| 整体完成了多少、还剩多少 | 关联实体计数、事件流派生表 |
| 某条样本卡在调度还是执行 | step 树、原始日志、heartbeat 细节 |
| 失败了怎么办（可理解的原因） | `FAILED` 状态 + 技术 ID，缺用户可读摘要 |

因此用户面应 **换信息架构**，而不是简单隐藏几个 Tab：以 **Worker 舰队 + Episode 进度** 为主叙事，调度信息上提为可读状态，日志下沉为「管理员入口」或不提供。

---

## 3. 目标与非目标

### 3.1 目标（Worker 部分）

1. **调度可见**：展示「已提交 → 排队/待分发 → 已派往 Worker → 执行中 → 已回传」的用户语义，对应现有工作流阶段，但用自然语言与进度条表达，而非运维画布。  
2. **Worker 负载可见**：每台 Worker 的健康态、容量占用、活跃 Episode 数、支持的环境类型（若有）。  
3. **执行进度可见**：单条 Episode 的 `current_step / total_steps`、预估剩余时间、所属 Worker；批量任务的完成率。  
4. **失败可理解**：失败条数 + 用户可读原因摘要（超时 / 无可用 Worker / 环境错误），**不**展开堆栈与 stdout。  
5. **与管理员面共存**：同一 Obs `ChainState` 可驱动两套视图；用户面是 **投影与裁剪**，不是第二套事实源。

### 3.2 非目标（明确不做）

| 不做 | 原因 |
|------|------|
| 日志面板 / 原始事件刷屏 | 面向排障；用户面不提供 |
| Step 级对象树展开 | 噪声过高；进度用条形/百分比即可 |
| 直连 Worker、改 capacity、drain、重启 | 属运维控制面 |
| Episode / Step 级遥控 | 现规划仅 run 级控制，用户面本期只读 |
| Prometheus/Grafana 替代大盘 | 用户面只做任务上下文内的调度与进度 |
| Hub 注册、环境包管理 | 非 Worker 执行观测 |

---

## 4. 角色对比：管理员面 vs 用户面（Worker）

| 维度 | 管理员 / 开发者面（现状） | 用户面（本规划） |
|------|---------------------------|------------------|
| 主入口叙事 | 全链路工作流图 | **任务进度总览 + Worker 舰队** |
| Worker 呈现 | 树节点 + `WorkerView` 摘要 | **Worker 卡片墙 / 列表**（负载条为主） |
| 调度细节 | DISPATCH 节点、lease、attempt | **排队深度、派发耗时、落点 Worker、重试次数（可读）** |
| 进度 | step 树节点、事件流派生 | **进度条 + step 分数 + ETA** |
| 环境实例 | `env_instance` 树层级 | 折叠为「池化命中 / 冷启动」一句摘要（有数据才显示） |
| 日志 / 事件 | 底栏核心排障工具 | **默认不出现**；可选「复制诊断 ID」给管理员 |
| ID 暴露 | 完整 `worker_id` / `episode_id` | 默认短码；详情抽屉可复制完整 ID |
| 快照 / 回放 | 联调抓拍 | 可选「进度书签」；非 P0 |
| 控制 | 开战/停战按钮（多 disabled） | 本期只读；开战仍走脚本/Bridge |

---

## 5. 信息架构（仅 Worker 相关）

用户任务台中与 Worker 相关的推荐结构（可落在独立路由，如 `/tasks/:runId`，或作为现有 console 的「用户模式」切换）：

```text
┌─────────────────────────────────────────────────────────────┐
│ 顶栏：任务名 / run 短码 · 状态 · 完成率 · 连接态              │
├───────────────────────────┬─────────────────────────────────┤
│ A. 调度与进度总览           │ B. Worker 舰队                   │
│  · 阶段漏斗 / 进度条        │  · Worker 卡片（负载/健康）       │
│  · 排队 · 执行中 · 完成/失败 │  · 点击 → 该 Worker 上的 Episode │
├───────────────────────────┴─────────────────────────────────┤
│ C. Episode 列表（默认表格式，按状态筛选）                      │
│  · 样本摘要 · 落点 Worker · 进度 · ETA · 可读失败原因          │
└─────────────────────────────────────────────────────────────┘
│ （无日志/事件底栏；可选「诊断信息」折叠条：仅 ID 与时间）        │
```

阅读顺序固定为：**总进度 → 舰队忙闲 → 单条 Episode**。不把工作流画布放在第一屏主视觉。

---

## 6. 界面设计细则

### 6.1 A 区：调度与进度总览

把管理员面的五阶段工作流 **压缩为用户漏斗 + 一条总进度**：

| 用户标签 | 对应 Obs / 控制面语义 | 展示 |
|----------|----------------------|------|
| 已提交 | `SUBMIT` / 入队 | 计数 |
| 排队中 | 已提交未派发（pending） | 计数 + 可选「等待可用槽位」提示 |
| 执行中 | `DISPATCH` 已落点且未终态 / `EXECUTE` | 计数 + 平均进度 |
| 已完成 | `DONE` 且成功 | 计数 |
| 失败 | `FAILED` / 终态失败 | 计数；可点开列表过滤 |

**总进度条**（主指标）：

```text
progress = (done + failed) / max(total_submitted, 1)
```

旁注：`执行中 N · 排队 M · 失败 K`。  
若 Obs 已维护 `workflow.nodes[].payload_summary.count`（见 07-27 口径说明），优先用服务端计数，避免前端各自推断。

**调度细节（精简但仍可见）**——用一行「调度健康」表达，不要画 lease 图：

| 字段（用户文案） | 数据来源建议 | 说明 |
|------------------|--------------|------|
| 平均排队等待 | Server 投影：`enqueue → dispatch` 时差 | 无数据则隐藏 |
| 当前无可用 Worker | 调度拒绝/capacity 类事件摘要 | 用警告条，不刷日志 |
| 重试中 | `attempt_id > 1` 的 episode 数 | 点击过滤 Episode 列表 |
| 步调 | `NORMAL` / `减速` / `暂停分发`（若有 pacing） | 用户文案，不暴露枚举原名也可 |

### 6.2 B 区：Worker 舰队

每台 Worker 一张卡片（或紧凑行），信息预算严格控制：

**必须展示**

| 元素 | 说明 |
|------|------|
| 显示名 | 短码或别名（完整 `worker_id` 悬停/复制） |
| 健康态 | 在线 / 繁忙 / 排空中 / 离线（由心跳与 status 映射） |
| 负载条 | `active / capacity`（容量缺失时用 active 数 +「容量未知」） |
| 本任务贡献 | 本 run 上该 Worker 承接的 episode 数、完成数 |

**建议展示（有字段才显示）**

| 元素 | 说明 |
|------|------|
| 环境类型标签 | `math` / `swe` / `code` 等，避免技术变体刷屏（`benchmark_variant` 可次级显示） |
| 实例池摘要 | 「预热命中」或「冷启动」计数一句 |
| 最近活跃 | 相对时间（如「12s 前有进度」），不展示原始 heartbeat unix |

**明确不展示**

- 心跳 seq、gRPC 地址、资源规格原数字段堆砌  
- `env_instances[]` 原始 ID 列表  
- Worker 侧 stdout / Prometheus 链接（可留给管理员面）

**交互**

- 点击卡片 → C 区自动过滤「落在该 Worker 的 Episode」  
- 卡片上不提供 drain / 重启等运维操作

### 6.3 C 区：Episode 进度列表

默认表格（移动端可改为卡片列表），列定义：

| 列 | 用户可见内容 | 管理员面对照 |
|----|--------------|--------------|
| 样本 | 短标题或 `episode` 短码 | 完整 id 树节点 |
| 状态 | 排队 / 调度中 / 执行中 / 完成 / 失败 | `NodeStatus` 英文枚举可映射中文 |
| 落点 | Worker 短名 | `worker_id` |
| 进度 | `step 3/10` + 迷你进度条 | step 子树 |
| 预估 | ETA 或「—」 | `estimated_remaining_seconds` |
| 说明 | 失败时一行可读原因 | 日志 Tab |

行展开（可选）：仅展示 **调度时间线摘要**（提交时刻 → 派发时刻 → 首个进度 → 终态），**不**展开逐步 log。

筛选：全部 / 执行中 / 排队 / 失败 / 某 Worker。  
排序：默认「进行中优先」，其次最近更新。

### 6.4 详情抽屉（轻量）

选中一条 Episode 或一台 Worker 时，右侧或底部抽屉：

**Episode 抽屉**

- 进度条、step 分数、ETA  
- 调度摘要：派往哪台、第几次尝试、是否重调度  
- 「复制诊断信息」：`training_run_id` / `episode_id` / `correlation_id` / `worker_id`（给管理员用）  
- **无**日志流、**无**逐步工具调用原文（SWE/Agent 路径尤其如此）

**Worker 抽屉**

- 负载条、本 run 贡献统计  
- 当前活跃 Episode 列表（短码 + 进度）  
- 健康说明（如「心跳超时，可能离线」）——一句话，不贴原始事件

---

## 7. 状态与文案映射

### 7.1 Worker 健康态

| 内部信号（示意） | 用户文案 | 颜色语义 |
|------------------|----------|----------|
| 近期有心跳且 load &lt; capacity | 空闲 / 可接任务 | 中性成功 |
| 近期有心跳且 load ≥ capacity | 繁忙 | 强调进行中 |
| `draining` | 排空中（不再接新任务） | 警告 |
| 心跳超时 / 失联 | 离线 | 危险 |
| degraded（长时间无 report） | 可能卡住 | 警告 |

### 7.2 Episode 用户态

| 内部 | 用户文案 |
|------|----------|
| 已提交未派发 | 排队等待调度 |
| 已派发未出首个进度 | 已分配，准备执行 |
| 有 PROGRESS / step 推进 | 执行中 |
| 成功终态 | 已完成 |
| 失败终态 | 失败 |
| attempt 递增 | 失败后重试中 |

### 7.3 失败原因（用户可读，枚举化）

优先由 Obs 投影 `failure_category` + `failure_summary`（短句），建议类别：

| category | 用户摘要示例 |
|----------|--------------|
| `NO_WORKER` | 暂无可用执行节点（容量已满或无匹配环境） |
| `TIMEOUT` | 执行超时 |
| `ENV_ERROR` | 环境启动或评测失败 |
| `AGENT_ERROR` | Agent / 模型侧错误 |
| `CANCELLED` | 已取消 |
| `UNKNOWN` | 执行失败（请联系管理员并提供诊断 ID） |

**禁止**把 stderr 全文推到用户面。

---

## 8. 数据契约：在现有 ChainState 上增量投影

原则：**不新建第二套观测协议**；在 Obs `ChainState` / `WorkerView` / `EpisodeView` 上扩展用户面所需字段，管理员面可忽略未知字段。

### 8.1 现有可用字段（已够搭骨架）

| 结构 | 字段 | 用户面用途 |
|------|------|------------|
| `ChainState` | `run_state`, `updated_at`, `workflow`, `episodes`, `workers` | 总览与列表 |
| `WorkerView` | `worker_id`, `active_episodes[]`, `status`, `last_heartbeat_ts` | 舰队卡片 |
| `EpisodeView` | `episode_id`, `worker_id`, `step_index`, `status`, `attempt_id` | 进度行 |
| `WorkflowNode.payload_summary.count` | 阶段计数 | 漏斗 |

### 8.2 建议增量（用户面 P0 / P1）

#### `WorkerView` 扩展

| 字段 | 类型 | 优先级 | 说明 |
|------|------|--------|------|
| `capacity` | number | P0 | 与 admin `/workers` 同源；算负载比 |
| `load` | number | P0 | 当前占用；可与 `active_episodes.length` 对齐校验 |
| `supported_env_types` | string[] | P1 | 卡片标签 |
| `display_name` | string | P1 | 可选友好名 |
| `pool_summary` | `{ warm?, hit_rate? }` | P2 | 一句池化摘要 |
| `health` | enum | P0 | Obs 投影后的用户健康态，避免前端各自猜心跳阈值 |

#### `EpisodeView` 扩展

| 字段 | 类型 | 优先级 | 说明 |
|------|------|--------|------|
| `total_steps` | number | P0 | 与 StreamReport 对齐 |
| `progress_ratio` | number | P0 | `current/total`，缺 total 时可为 null |
| `estimated_remaining_seconds` | number | P0 | ETA |
| `queue_wait_ms` | number | P1 | 调度等待 |
| `dispatch_latency_ms` | number | P1 | 派发耗时 |
| `failure_category` | string | P0 | 见 §7.3 |
| `failure_summary` | string | P0 | ≤ 80 字中文/英文摘要 |
| `label` | string | P1 | 样本短标题（来自 payload 投影，禁止泄题可只显示序号） |

#### Run 级汇总（可放 `ChainState` 顶层或 `payload_summary`）

| 字段 | 说明 |
|------|------|
| `submitted_count` / `queued_count` / `running_count` / `succeeded_count` / `failed_count` | 漏斗权威计数 |
| `scheduler_hint` | 可选短句：「3 台 Worker 满载，排队 12」 |

### 8.3 数据来源分工（保持观测面原则）

```text
Worker 心跳 / Register     ──┐
StreamReport(PROGRESS…)    ──┼─► Server 控制面 hook / project ──► Obs ChainState
调度排队与派发事实         ──┘              │
                                           ▼
                              用户面 / 管理员面 各自渲染
```

- Worker **P0 仍无**独立 Obs HTTP 通道（与 07-15 规划一致）：进度靠 **StreamReport → Server 转译**。  
- `capacity` / 全局排队若 Obs 暂缺，可由 Server 在 project 时并入 `WorkerView`，**不要**让用户面前端直连 `:50052/admin`。

### 8.4 前端消费裁剪

用户面 store 可复用 `applyStateDelta`，渲染层遵守：

- 忽略 `tree` 中 `kind === "step"`（或根本不渲染树）  
- 不订阅/不展示日志与原始 events Tab  
- `env_instance` 不作为独立导航层

---

## 9. 与现有 TrainingConsole 的落地关系

推荐两种实现路径（二选一，产品拍板）：

| 方案 | 做法 | 适用 |
|------|------|------|
| **A. 模式切换** | 同一应用顶栏增加「用户视图 / 运维视图」；共享 SSE，切换布局组件 | 联调期快速验证 |
| **B. 独立路由** | `/ops` 保留现 console；`/tasks/:runId` 新用户台 | 长期产品边界清晰 |

本规划建议 **B 为默认目标**，A 可作为过渡。  
无论哪条路径，**Worker 相关 UI 组件应新建**（如 `WorkerFleetPanel`、`TaskProgressOverview`、`EpisodeProgressTable`），避免在运维画布上叠 `hidden` 造成双用途腐化。

---

## 10. 交互与体验约束

1. **默认只读**：无开战/停战；连接态与「数据更新于」保留。  
2. **一屏主路径**：首屏只有总进度 + 舰队 + 列表；详情用抽屉。  
3. **少卡片装饰**：舰队可用紧凑列表；进度用条与数字，避免运维风多指标磁贴墙。  
4. **移动端**：舰队可横滑卡片；Episode 改列表；不追求运维面的三栏拖拽。  
5. **空态**：无 Worker → 「尚无执行节点接入」；有排队无派发 → 「等待可用容量」并提示联系管理员扩容。  
6. **安全**：用户面默认不展示可能含题目原文/补丁全文的字段；仅进度与状态。

---

## 11. 分期与验收

### P0 — 可演示的用户 Worker 面

| ID | 项 | 验收 |
|----|----|------|
| U-W0 | 任务总进度条 + 五态计数 | 真实 run 下计数随 Episode 推进 |
| U-W1 | Worker 舰队（健康 + 负载条） | ≥1 Worker；点击过滤 Episode |
| U-W2 | Episode 表：状态 / Worker / step 进度 | StreamReport 驱动 step 变化可见 |
| U-W3 | 无日志/事件底栏 | 页面无日志入口 |
| U-W4 | 失败摘要或至少失败计数 + 诊断 ID 复制 | 失败行可区分于成功 |

### P1 — 调度细节增强

| ID | 项 | 验收 |
|----|----|------|
| U-W5 | 排队等待 / 派发耗时 | 列表或总览可见 |
| U-W6 | `failure_category` 映射文案 | 常见失败类可理解 |
| U-W7 | ETA | 有 `estimated_remaining_seconds` 时展示 |
| U-W8 | 无 Worker / 满载警告条 | 与调度拒绝语义对齐 |

### P2 — 体验

| ID | 项 |
|----|----|
| U-W9 | Worker / Episode 友好名 |
| U-W10 | 实例池一句摘要 |
| U-W11 | 与运维面深链（「在运维视图中打开」仅管理员角色可见） |

---

## 12. 开放问题（需产品/后端拍板）

1. **容量字段**：`WorkerView.capacity` 由 Obs 投影，还是短期允许 BFF 聚合 admin 快照？（倾向 Obs 投影，保持「前端只连 Obs」。）  
2. **样本标题**：评测场景是否允许展示题面短标题，还是仅序号？  
3. **多 run / 多租户**：用户面是否先假设「一次只看一个 `training_run_id`」（与现状 `?run=` 一致）？  
4. **Agent 池路径**：无传统 Worker gateway 的 CodeAgent / ToolEnv，舰队区是展示 **Agent Pool 卡片** 还是统一抽象为「执行单元」？（建议统一「执行单元」模型，Worker 与 Agent Pool 同属 B 区。）

---

## 13. 文档关系

| 文档 | 关系 |
|------|------|
| 本规划 | 定义 **用户面 Worker 模块** 的信息架构、裁剪原则与字段增量 |
| 260612 / 07-15 前端规划 | 继续约束 **管理员观测台** 与 Obs 通道；本规划不推翻 SSE/`ChainState` |
| 260726 差距文档 | 管理员面 P1 日志/Metrics **不自动变成用户面需求** |
| 后续实现清单 | 落地时应另开 `面向用户的Worker前端-实现清单.md`（API 字段 PR + 前端路由） |

---

## 14. 一句话边界

> **用户面 Worker 模块 = 调度结果可见 + 执行进度可见；不是日志工作站，也不是运维控制台。**
