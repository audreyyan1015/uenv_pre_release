# Worker 与环境资源池前端改造实施规划

> **日期**：2026-08-13  
> **依据**：[Worker与环境资源池概念命名决策](./Worker与环境资源池概念命名决策.md)  
> **目标**：在不改变现有协议字段和后端调度语义的前提下，完成用户侧概念统一，并新增跨执行节点的环境资源池视图。  
> **执行规则**：按阶段顺序实施；每个阶段完成后必须更新本文件的状态、改动范围和验收结果，再进入下一阶段。

---

## 1. 总体边界

### 1.1 本轮实施范围

- 用户可见的 `Worker` 统一展示为“执行节点”。
- 用户可见的 `env_instance` 统一展示为“环境运行时”；在需要强调隔离边界时展示“隔离环境运行时”。
- 单 Worker 的 `pool_summary` / `pool_slots` 统一展示为“本地环境运行时池”和“本地运行时槽位”。
- 保留 `/server/worker`，将其定位为单执行节点机器级详情页。
- 新增 `/server/pools`，展示跨所有执行节点聚合的环境资源池。
- 从 `/system` 拓扑页直接进入环境资源池页面。
- 第一版全局聚合暂基于已有 `/fleet/workers` 数据，后续再评估 Server 聚合接口。

### 1.2 本轮不做

- 不修改 `worker_id`、`env_instances`、`pool_summary`、`pool_slots` 等协议或 JSON 字段。
- 不修改数据库字段、Protobuf 字段或 Worker/Server 调度逻辑。
- 不将环境运行时统一命名为 `container`。
- 不把 Worker 本地池改造成全局调度池。
- 不新增 Server 聚合 API 作为本轮前端交付的前置条件。

---

## 2. 阶段清单

| 阶段 | 内容 | 状态 | 验收状态 |
|---|---|---|---|
| P0 | 建立规划、盘点入口和验证命令 | 已完成 | 已验收 |
| P1 | 统一已有前端用户侧概念展示 | 已完成 | 已验收 |
| P2 | 新增全局环境资源池聚合模型和页面 | 已完成 | 已验收 |
| P3 | 系统拓扑增加资源池入口并明确统计作用域 | 已完成 | 已验收 |
| P4 | 全局搜索、类型检查、Lint、构建和文档收口 | 已完成 | 已验收 |

---

## 3. P0：规划和盘点

### 3.1 目标

确认前端入口、数据来源、路由方式和可执行的验证命令，避免改名时遗漏用户侧文案或混淆协议字段。

### 3.2 重点文件

```text
frontend/src/routes/server_.worker.tsx
frontend/src/components/worker-detail.tsx
frontend/src/components/worker-status-overview.tsx
frontend/src/components/system-topology.tsx
frontend/src/hooks/use-system-telemetry.ts
frontend/src/hooks/use-worker-fleet-live.ts
frontend/src/lib/types/chain-state.ts
```

### 3.3 验收标准

- 已确认 `/server/worker` 为单执行节点详情路径。
- 已确认 `/fleet/workers` 可提供第一版全局聚合所需的 Worker 级池快照。
- 已确认前端脚本至少包括 `lint`、`build`，并记录实际执行结果。

### 3.4 实施记录

- 状态：已完成。
- 结论：第一版全局资源池可基于 `FleetStatusPayload.workers[].pool_summary` 聚合，按 `env_type + variant + package_id + package_version + backend_kind` 形成逻辑池键。
- 验收：已确认 `frontend/package.json` 提供 `lint` 和 `build`；仓库当前没有现成前端测试脚本。

---

## 4. P1：统一已有前端用户侧概念展示

### 4.1 目标

只修改用户可见标题、描述、导航、空状态、说明文字和必要的注释，不修改协议字段和内部数据结构名称。

### 4.2 预定改动

- `/server/worker`：`Worker 详情` → `执行节点详情`。
- Worker 列表：`Worker 舰队` / `Worker 列表` → `执行节点舰队` / `执行节点列表`。
- Worker 详情：`Worker 实例池` → `本地环境运行时池`。
- Worker 详情：`实例池槽位` → `本地运行时槽位`。
- Worker 详情：`环境实例数` → `环境运行时数`。
- 所有本地池说明明确“仅代表当前执行节点”。
- 技术拓扑保留必要的 `Worker` 英文技术名，但补充“执行节点”概念名。

### 4.3 验收标准

- 用户侧页面不再把单 Worker 本地池描述成全局池。
- 用户侧主标题、区块标题和空状态统一使用新概念。
- `worker_id`、`env_instances`、`pool_summary`、`pool_slots` 等字段和调用保持不变。
- `npm run lint` 通过。

### 4.4 实施记录

- 状态：已完成。
- 改动文件：`frontend/src/routes/server_.worker.tsx`、`frontend/src/components/worker-detail.tsx`、`frontend/src/components/worker-status-overview.tsx`、`frontend/src/components/episode-journey.tsx`、`frontend/src/components/user-launch-console.tsx`、`frontend/src/components/agent-pool-status.tsx`、`frontend/src/routes/server_.agents.tsx`、`frontend/src/routes/system.tsx`、`frontend/src/components/system-topology.tsx`。
- 结果：用户侧标题、说明、空状态、任务落点、Agent 对齐和导航已改为“执行节点”“环境运行时”“本地环境运行时池”等术语；技术字段和内部数据结构未修改。
- 验收命令及结果：全量 `npm run lint` 已通过（0 errors，6 个既有 Fast Refresh warnings）；`npm run build` 已通过。

---

## 5. P2：全局环境资源池聚合模型和页面

### 5.1 目标

新增面向用户的跨执行节点环境资源池视图，第一版从已有 Worker 舰队快照聚合，不改变后端协议。

### 5.2 预定数据模型

前端新增独立模型，避免与 `WorkerPoolSummary` 混淆：

```text
GlobalEnvironmentPool
    key
    envType
    variant
    packageId
    packageVersion
    backendKind
    workerCount
    runtimeCount
    ready
    busy
    warming
    failed
    capacity
    workers[]
```

聚合键：

```text
env_type + variant + package_id + package_version + backend_kind
```

### 5.3 预定页面

```text
/server/pools
```

页面名称：

```text
环境资源池
Environment Resource Pools
```

页面至少展示：

- 当前环境类型数量；
- 覆盖执行节点数量；
- 每个环境资源池的容量、就绪、执行中、预热中和异常数量；
- 按执行节点展开的本地池统计；
- 跳转到 `/server/worker?worker=...` 查看单节点详情。

### 5.4 验收标准

- `/server/pools` 可直接访问并正确加载已有 `/fleet/workers` 数据。
- 空数据、接口失败和演示/回退状态有明确反馈。
- 全局聚合页面不把聚合对象误称为某个 Worker 的实际进程池。
- 页面支持桌面和窄屏布局，不依赖固定宽度才能阅读。
- `npm run lint` 和 `npm run build` 通过。

### 5.5 实施记录

- 状态：已完成。
- 改动文件：`frontend/src/lib/environment-pools.ts`、`frontend/src/routes/server_.pools.tsx`。
- 结果：新增 `/server/pools`，基于 `/fleet/workers` 的 `pool_summary` 按环境类型、变体、环境包版本和 Backend 聚合，支持查看全局统计及下钻到执行节点详情；重复上报同一执行节点时会合并本地统计；为空和接口失败状态提供反馈。
- 验收命令及结果：全量 `npm run lint` 已通过（0 errors，6 个既有 Fast Refresh warnings）；`npm run build` 已通过。

---

## 6. P3：系统拓扑入口和作用域说明

### 6.1 目标

从系统拓扑页直接进入环境资源池，并明确拓扑中的资源统计是跨执行节点的聚合统计。

### 6.2 预定改动

- 增加“环境资源池”快捷导航，指向 `/server/pools`。
- Worker 拓扑卡片展示“执行节点 / Worker”。
- Pool 拓扑卡片展示“环境资源池”，说明“跨执行节点聚合”。
- Worker 卡片继续保留单节点详情跳转。

### 6.3 验收标准

- 拓扑页能进入 `/server/pools`。
- 拓扑页能区分执行节点入口和环境资源池入口。
- ready/busy 统计文案明确为跨节点汇总，不与本地池混淆。
- `npm run lint` 和 `npm run build` 通过。

### 6.4 实施记录

- 状态：已完成。
- 改动文件：`frontend/src/components/system-topology.tsx`、`frontend/src/routes/system.tsx`。
- 结果：拓扑页已增加“环境资源池”快捷入口，指向 `/server/pools`；Worker 卡片改为“执行节点 / Worker”；资源池卡片改为“环境资源池 / 跨执行节点聚合”，并直接链接全局资源池页面。
- 验收命令及结果：全量 `npm run lint` 已通过（0 errors，6 个既有 Fast Refresh warnings）；`npm run build` 已通过。

---

## 7. P4：收口验收

### 7.1 全局检查

- 搜索用户可见文案中的旧术语，确认剩余位置均为有意保留的技术名称或兼容提示。
- 检查新页面和旧页面的路由深链。
- 检查协议字段没有被展示层改名误改。
- 检查文档链接和概念对照表仍然准确。

### 7.2 验收命令

在 `frontend` 目录执行：

```bash
npm run lint
npm run build
```

如后续补充测试脚本，再执行：

```bash
npm test
```

### 7.3 最终交付标准

- 用户侧可以区分执行节点、本地环境运行时池和全局环境资源池。
- `/server/worker` 和 `/server/pools` 的作用域边界清楚。
- 系统拓扑提供全局资源池入口。
- 现有协议字段和后端调度行为无变化。
- 本文件已记录每个阶段的实际改动和验收结果。

### 7.4 实施记录

- 状态：已完成。
- 已完成：新增页面已纳入生成的 `frontend/src/routeTree.gen.ts`；全量 `npm run lint` 已通过（0 errors，6 个既有 Fast Refresh warnings）；`npm run build` 已通过；`git diff --check` 已通过。
- 说明：格式化了 `frontend/src/components/agent-pool-status.tsx` 和 `frontend/src/routes/server_.agents.tsx`，未改变业务逻辑；并补齐了 Episode、Agent 状态和启动台中的执行节点展示文案。
- 最终结果：协议字段、后端调度行为未修改；执行节点、本地环境运行时池和全局环境资源池的用户侧语义已落地。
