# 面向用户的 Worker 前端设计

> **日期**：2026-08-05  
> **状态**：已落地首版（Worker 列表跳转 + 详情页）  
> **受众**：训练/评测用户（非管理员、非联调开发者）  
> **继承**：[260802 规划稿](../260802/面向用户的Worker前端设计规划.md)  
> **实现入口**：
> - 任务总览：`frontend/src/components/episode-journey.tsx`（路由 `/server`）
> - Worker 舰队：`frontend/src/components/worker-status-overview.tsx`
> - Worker 详情：`frontend/src/components/worker-detail.tsx`（路由 `/server/worker`）

---

## 1. 结论先行

| 项 | 决策 |
|----|------|
| 产品定位 | 在管理员观测台（`/`）之外，**用户任务台** 的 Worker 执行面（只读观测） |
| 路由 | `/server?run=` 任务进度；`/server/worker?run=&worker=` Worker 详情 |
| 跳转 | Server 页 Worker 列表项 **可点击**，进入对应 Worker 详情 |
| 数据入口 | 仍只连 **Obs REST + SSE**；不直连 Worker gRPC / admin HTTP |
| 详情内容 | 运行状态、负载、环境实例、活跃 Episode、模块配置（环境类型 / 端点） |
| 本期不做 | 日志面板、step 树、drain/重启等运维控制 |

---

## 2. 信息架构

```text
/server?run={training_run_id}                    Episode 进度（用户任务台）
├── A. Episode 阶段统计 + 列表
└── B. Worker 舰队（worker-status-overview）
         │
         │ 点击某 Worker
         ▼
/server/worker?run={id}&worker={worker_id}       Worker 详情
├── 1. 运行状态（健康 / 负载 / 心跳 / 本 run 统计）
├── 2. 环境实例（树投影 + env_instances[]）
├── 3. 活跃 Episode（本 run、落点该 Worker、未终态）
└── 4. 模块配置（supported_env_types、endpoint、实例计数）
```

阅读顺序：**任务进度 → 舰队忙闲 → 单台 Worker 详情**。详情页提供返回 `/server` 的导航。

---

## 3. 跳转契约

### 3.1 列表 → 详情

| 参数 | 必填 | 说明 |
|------|------|------|
| `run` | 是 | `training_run_id`，与 Server 页一致 |
| `worker` | 是 | `WorkerView.worker_id` |
| `status` | 否 | 列表侧 operational status，用于详情首屏徽章回显 |

前端工具：

- `WORKER_DETAIL_ROUTE` = `/server/worker`
- `buildWorkerDetailSearch({ runId, workerId, status })` → TanStack Router `search`
- 列表仅在 `runId` 存在时渲染可点击 `Link`；无 run 时保持静态行

### 3.2 详情 → 列表

详情页顶栏与页脚 `Link to="/server" search={{ run }}` 回到任务进度，保留 run 上下文。

---

## 4. Worker 详情页界面细则

### 4.1 运行状态区

| 元素 | 数据来源 |
|------|----------|
| 健康态徽章 | `classifyWorkerStatus(WorkerView)` → busy / idle / offline / attention |
| 状态原因 | `status_reason` → 中文映射（与列表一致） |
| 心跳 | `last_heartbeat_ts` → 相对时间 |
| 负载条 | `current_load` 或 `active_episodes.length` / `capacity` |
| 本 run 统计 | `episodes` 中 `worker_id` 匹配的总数与完成数 |

### 4.2 环境实例区

合并两路数据（`projectWorkerDetail`）：

1. **树投影**：`tree` 中 `worker → env_instance → episode` 子树（`meta.label` 作展示名）
2. **Worker 视图**：`WorkerView.env_instances[]` 补全树中尚未出现的实例 id

每张实例卡片展示：标签、实例 id、状态点、关联 Episode 列表（env_type、step、状态）。

### 4.3 活跃任务区

`episodes` 过滤条件：

- `worker_id === 当前 worker`
- `status` 为 `ACTIVE` 或 `PENDING`

展示：episode id、环境类型、step、重试次数、最近更新时间。

### 4.4 模块配置区

| 字段 | 说明 |
|------|------|
| `supported_env_types` | 标签展示；无则「尚未上报」 |
| `endpoint` | 等宽字体；用户面只读展示 |
| `active_episodes.length` | 活跃任务数 |
| `env_instances.length` | 环境实例数 |

**不展示**：心跳 seq、原始 gRPC 调试字段、Prometheus 链接、stdout。

---

## 5. 与 260802 规划的对应关系

| 260802 模块 | 260805 落地 |
|-------------|-------------|
| B 区 Worker 舰队 | `worker-status-overview`（已有） |
| Worker 抽屉 / 点击下钻 | 独立路由 `/server/worker` 替代抽屉 |
| 环境实例折叠摘要 | 详情页「环境实例」区块 |
| Episode 过滤某 Worker | 详情页「活跃任务」+ 可回到 Server 页按 Worker 筛选（后续） |
| 日志 / 事件底栏 | 未做（符合非目标） |

---

## 6. 数据契约

仍基于 `ChainState`，无第二套协议。详情页复用 `useRunStream`（poll 模式，与 Server 页一致）。

| 结构 | 详情页用途 |
|------|------------|
| `workers[worker_id]` | 主实体 |
| `tree` | env_instance 层级与 meta |
| `episodes` | 按 worker_id 过滤进度 |
| `WorkerView.env_instances` | 实例 id 补全 |
| `WorkerView.supported_env_types` | 模块配置 |

建议后续 Obs 增量（与 260802 §8.2 一致，非阻塞首版）：

- `EpisodeView.total_steps` / `progress_ratio` / `estimated_remaining_seconds`
- `WorkerView.display_name` / `pool_summary`
- `failure_category` + `failure_summary` 用于失败 Episode 可读摘要

---

## 7. 文件清单

| 文件 | 职责 |
|------|------|
| `frontend/src/lib/worker-status.ts` | 状态分类、`buildWorkerDetailSearch`、`WORKER_DETAIL_ROUTE` |
| `frontend/src/lib/worker-tree.ts` | `projectWorkerDetail` 树 + episodes 投影 |
| `frontend/src/components/worker-status-overview.tsx` | 舰队列表 + `Link` 跳转 |
| `frontend/src/components/worker-detail.tsx` | 详情页 UI |
| `frontend/src/routes/server_.worker.tsx` | 路由与 search 校验；`server_` 前缀使 `/server/worker` 与 `/server` 平级，避免嵌套进无 `<Outlet />` 的 Episode 页 |

---

## 8. 验收（首版）

| ID | 项 | 验收 |
|----|----|------|
| U-WD-0 | Server 页 Worker 行可点击 | 有 `run` 时跳转 `/server/worker` |
| U-WD-1 | 详情展示运行状态与负载 | fixture / 实机 run 可见 |
| U-WD-2 | 环境实例列表 | fixture 树 + `env_instances` 可见 |
| U-WD-3 | 活跃 Episode | 执行中 episode 出现在对应 Worker |
| U-WD-4 | 模块配置 | `supported_env_types` / `endpoint` 有则展示 |
| U-WD-5 | 返回导航 | 回到 `/server?run=` 保留 run |

---

## 9. 分期后续

| 阶段 | 内容 |
|------|------|
| P1 | 详情页 Episode 进度条与 ETA；失败可读摘要 |
| P1 | Server 页与详情页双向深链（详情中点 Episode 回 Server 并选中） |
| P2 | `display_name`、池化一句摘要；管理员面深链（仅运维角色） |

---

## 10. 一句话边界

> **用户面 Worker = 从舰队点进单台执行节点，看清环境实例与在跑任务；不是日志站，也不是运维控制台。**
