# Worker 与环境资源池概念命名决策

> **日期**：2026-08-13  
> **状态**：已固定，作为后续设计与开发的术语基线  
> **适用范围**：Worker 执行层、Server 资源聚合、用户侧前端展示  
> **相关现状文档**：[面向用户的Worker前端设计](../260805/面向用户的Worker前端设计.md)、[Worker Pool 层设计说明](../../older/worker-pool-layer-design.md)

---

## 1. 决策摘要

当前系统存在两个不同作用域的“池”概念：

1. Worker 内部按环境类型维护的本地实例集合。
2. 面向用户、跨所有服务器按环境类型聚合的资源集合。

二者不能继续只使用一个不带作用域的 `pool` 名称。最终采用以下术语：

| 当前概念 | 对外中文名 | 对外英文名 | 作用域 |
|---|---|---|---|
| `worker` | 执行节点 | Execution Node | 单台服务器上的可调度执行节点 |
| `env_instance` | 隔离环境运行时 | Isolated Environment Runtime | 单个独立生命周期和故障边界 |
| `WarmupPool` | Worker 本地环境运行时池 | Worker-local Environment Runtime Pool | 单个执行节点、单种环境资源 |
| 新增聚合概念 | 环境资源池 | Environment Resource Pool | 跨所有执行节点的用户视图 |
| 新增内部聚合概念 | 全局环境运行时池 | Global Environment Runtime Pool | 跨节点聚合的逻辑资源集合 |

代码和协议字段暂不整体重命名。现有 `worker_id`、`env_instances`、`pool_summary`、`pool_slots` 等字段继续作为协议兼容名称；用户界面和新设计文档使用本文件确定的展示名称。

---

## 2. Worker 的概念定位

### 2.1 正式定义

**执行节点（Execution Node）**是与一台服务器资源绑定的、可被 UEnv Server 调度的运行节点，负责：

- 向 UEnv Server 注册自身能力、端点和容量；
- 接收并执行 Server 下发的 Episode；
- 托管一种或多种环境类型的本地运行时池；
- 上报心跳、负载、实例池状态和执行结果。

英文定义：

> An Execution Node is a schedulable runtime node associated with one server. It hosts environment runtime pools, reports capacity and health, and executes Episodes dispatched by UEnv Server.

### 2.2 与 Worker 的关系

`Worker` 是当前协议、代码和部署中的稳定技术名称；`执行节点` 是面向用户和架构文档的准确产品名称。

```text
Worker（代码/协议名称）
    = Execution Node（概念/展示名称）
```

不将 Worker 改名为 `Server`，因为系统已有 `uenv-server`，且 Worker 的 gRPC Server 只是通信角色，不是资源模型名称。不将其直接改名为 `Runtime`，因为 Runtime 更适合描述节点内部的执行运行时。

### 2.3 一个 Worker 对应一台服务器的边界

当前部署约定下，一个 Worker 对应一台服务器上的一个 Worker 服务进程及其受管资源。该表述是部署拓扑约束，不等同于把 Worker 命名成 Server：

```text
Server Host
└── Worker Service / Execution Node
    ├── control-plane endpoint
    ├── Episode executor
    └── local environment runtime pools
```

如果未来一台服务器部署多个 Worker，`Execution Node` 仍然比“服务器”更准确；多个节点可以共享同一 Host，但每个节点仍是独立的调度和生命周期实体。

---

## 3. 环境实例的概念定位

### 3.1 正式定义

**隔离环境运行时（Isolated Environment Runtime）**是一个独立管理的环境执行单元，具有独立的：

- 生命周期；
- 环境状态机；
- 健康状态；
- 资源句柄；
- 故障边界；
- Episode 绑定关系。

英文定义：

> An Isolated Environment Runtime is an independently managed environment execution unit with its own lifecycle, state, health, resource handle, and failure boundary.

MVP 中通常满足：

```text
1 plugin process = 1 environment runtime
```

后续实现可以由不同 Backend 承载：

```text
ProcessBackend  → 插件子进程
PodmanBackend   → 容器
其他 Backend    → 其他隔离载体
```

因此不将所有环境实例直接命名为 `container`。容器是某种物理/部署载体，不是稳定的概念层名称。

### 3.2 与 `env_instance` 的关系

```text
env_instance（代码/协议名称）
    = Isolated Environment Runtime（概念名称）
    = 隔离环境运行时（展示名称）
```

`environment instance` 仍可在兼容性文档、历史接口和迁移说明中使用；新代码、界面和架构说明优先使用 `Environment Runtime`。

---

## 4. 两种池的作用域

### 4.1 Worker 本地环境运行时池

当前 `WarmupPool` 实际表示的是一个执行节点内、按环境类型维护的本地运行时集合：

```text
Worker / Execution Node
└── Worker-local Environment Runtime Pool
    ├── Warm Runtime
    ├── Active Runtime
    ├── Warming Runtime
    └── Cooling / Failed Runtime
```

推荐定义：

> A Worker-local Environment Runtime Pool is the set of environment runtimes of a compatible environment type and configuration managed by one Execution Node.

推荐代码方向：

```text
WarmupPool
    → WorkerLocalEnvironmentPool
    → WorkerLocalEnvironmentRuntimePool
```

不要求立即修改已有类型名。`WarmupPool` 更适合作为内部预热策略组件名，而不是用户可见的池概念名。

### 4.2 全局环境资源池

新增的用户侧概念是跨所有执行节点聚合的逻辑资源集合：

```text
Global Environment Runtime Pool
└── env_type = math
    ├── worker-01 local pool
    ├── worker-02 local pool
    └── worker-03 local pool
```

推荐对外名称：

```text
环境资源池
Environment Resource Pool
```

推荐内部名称：

```text
Global Environment Runtime Pool
GlobalEnvironmentPool
```

它是聚合统计和导航视图，不直接持有运行时，也不承担 Episode 二次调度或数据转发职责。

### 4.3 两种池的关系

```text
Global Environment Resource Pool
    = aggregate(all Worker-local Environment Runtime Pools)
```

必须始终明确：

```text
Worker-local pool ≠ Global environment resource pool
Worker 本地池 ≠ 全局环境资源池
```

---

## 5. 聚合键和统计边界

用户首页可以先按 `env_type` 展示，但系统内部不能永久只按 `env_type` 聚合。建议全局池的逻辑聚合键为：

```text
PoolKey = (
  env_type,
  variant,
  package_id,
  package_version,
  backend_kind
)
```

原因是不同变体、环境包版本或 Backend 不一定可以安全合并统计。

用户侧推荐的展开层级：

```text
环境资源池
└── 环境类型：math
    ├── 总容量 / ready / busy / warming / failed
    ├── 覆盖执行节点数
    ├── 环境包和 Backend 变体
    └── 执行节点明细
        └── 本地运行时槽位
```

---

## 6. 前端信息架构决策

### 6.1 保留的机器级路径

继续保留：

```text
/server/worker?run={run_id}&worker={worker_id}
```

该页面的产品名称调整为：

```text
执行节点详情
Execution Node Details
```

它继续展示单个 Worker/执行节点的：

- 在线状态、负载、心跳；
- 本地环境运行时池；
- 运行时槽位；
- 当前活跃 Episode；
- 支持的环境类型和环境包。

页面中的“Worker 实例池”应理解并逐步展示为“本地环境运行时池”，不得让用户误认为是跨服务器池。

### 6.2 新增的用户级路径

建议新增：

```text
/server/pools
```

页面名称：

```text
环境资源池
Environment Resource Pools
```

页面用于查看：

- 当前所有执行节点支持的环境类型；
- 每种环境类型跨节点的总容量；
- ready、busy、warming、failed 等状态统计；
- 覆盖的执行节点数量；
- 从环境类型下钻到执行节点和本地运行时。

### 6.3 系统拓扑入口

系统拓扑页应提供“环境资源池”快捷入口，并将拓扑中的资源池统计标注为跨节点聚合统计。拓扑中的 Worker 节点仍可进入单节点详情。

建议区分两个入口：

| 拓扑入口 | 路径 | 语义 |
|---|---|---|
| 执行节点 | `/server/worker?...` | 单节点机器级详情 |
| 环境资源池 | `/server/pools` | 跨节点环境资源聚合 |

---

## 7. API 演进方向

当前接口：

```text
GET /fleet/workers
```

继续保持 Worker/执行节点级快照，其中：

- `pool_summary` 表示单 Worker 本地池汇总；
- `pool_slots` 表示单 Worker 本地运行时槽位；
- `episodes` 表示该 Worker 当前活跃 Episode。

建议新增聚合接口：

```text
GET /fleet/environment-pools
```

其返回应提供全局池的聚合结果，并可包含按 Worker 拆分的明细。该接口的职责是聚合和展示，不改变 Scheduler 的 Episode 调度路径，也不引入 `Server → Pool → Worker` 的二次转发。

短期可以由前端基于 `/fleet/workers` 的 `pool_summary` 聚合；长期建议由 Server 统一聚合，以统一 stale/offline 处理、兼容性判断和统计时间点。

---

## 8. 迁移原则

### 8.1 立即执行

- 新文档和用户界面使用“执行节点”“隔离环境运行时”“环境资源池”。
- `/server/worker` 保留，但页面标题和说明改为执行节点语义。
- 新增 `/server/pools` 作为跨节点用户视图。
- 明确 `pool_summary` 和 `pool_slots` 是 Worker 本地数据。

### 8.2 暂不执行

- 不立即修改 Protobuf 字段名。
- 不立即修改已有 JSON 字段名。
- 不立即删除 `worker`、`env_instance`、`WarmupPool` 等已有代码标识。
- 不把所有实例统一改成 `container`。

### 8.3 后续新代码优先名称

```text
ExecutionNode
EnvironmentRuntime
IsolatedEnvironmentRuntime
WorkerLocalEnvironmentPool
GlobalEnvironmentPool
EnvironmentResourcePoolOverview
```

协议兼容字段和展示名称的完整对照见：[概念对照表-代码协议字段与对外展示名称](./概念对照表-代码协议字段与对外展示名称.md)。

---

## 9. 一句话基线

> Worker 是代码和协议中的稳定名称，产品上称为执行节点；`env_instance` 是代码和协议中的稳定名称，产品上称为隔离环境运行时；Worker 内的池是本地环境运行时池，跨 Worker 的统计才称为环境资源池。
