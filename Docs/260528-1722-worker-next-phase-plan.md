# Worker 层下一阶段补齐规划（基于 design + v7.2 对照）

> **文档版本**：v3.0  
> **更新时间**：2026-05-30  
> **定位**：在「Worker 层 MVP 已完成」的前提下，按 [worker-pool-layer-design.md](./worker-pool-layer-design.md) 与 [uenv-design-prd-v7.2.md](./uenv-design-prd-v7.2.md) 明确下一阶段功能缺口与补齐计划。  
> **适用范围**：仅覆盖 Worker 层与其直接联调边界（Server/Hub 的跨层依赖单列说明）。

---

## 1. 现状判定（本规划起点）

- **已完成（MVP）**：控制面链路（Register / Heartbeat / Dispatch / Report）、预热池固定容量、WAL 持久化与重放、最小 Prometheus 指标、单轮 GSM8K Episode 执行、本机预联调回归。
- **骨架在、语义未落地**：心跳双向流已通，但 `load` 恒为 0、`ResourceSpec` 未填、`next_heartbeat_interval_ms` 未生效、`DrainCommand` 未处理；负载画像六维未上报；`WorkerPoolRegistry` 仍为占位。
- **尚未开始**：真实跨机 Server 联调验收、Hub 真实接入、多环境并行、Podman 后端、多步 Episode / StreamReport 逐步上报、步调感知（PACING）、MCP/Agent 执行路径。
- **规划目标**：从「单环境 MVP 可用」升级到「**Server 可感知调度** + 多环境可负载 + 跨层可验收 + 通用环境可扩展 + 生产化后端」。

---

## 2. 设计对照总表（重点章节）

| 设计章节 | 设计要求（摘要） | 当前状态 | 下一步补齐 |
|---|---|---|---|
| §1.1/§7.0/§7.1 | Server 直连 Worker Dispatch，Pool 不二次转发 | 已实现 | P0：真实跨机联调证据并收口 |
| §7.4 / PRD §5.2 | 心跳保活、epoch fencing、自适应间隔、Drain 指令 | 循环已通，语义极简 | **P0：§3.2 心跳语义落地** |
| PRD §5.1 / §5.5 | Worker 六维负载画像 → 驱动调度 | 未上报 | **P0/P1：§3.2 负载画像最小集** |
| §7.1 / PRD 资源匹配 | Register 上报 `ResourceSpec`；调度侧资源过滤 | `resource: None` | **P0：§3.2 资源注册** |
| §4.1 / §7.0 | `WorkerPoolRegistry` 资源目录（ListWorkers / capacity / warm 状态） | 占位 struct | **P1：§3.9 资源目录** |
| §3.5/§5.3/§5.6 | 1 进程=1 实例、状态机、无双分配 | 已实现（MVP） | 扩展到多 `env_type` 同等约束 |
| §5.4 / PRD §6.4 | 预热池动态容量策略 | 固定参数 | P2：§3.10 WarmupSizer |
| §6.2 / PRD F-15 | Process→Podman 两阶段后端 | 仅 Process；Podman 占位 | **P1：§3.6 Podman 后端** |
| §3.6 / PRD F-21 | 插件注册发现与 Hub 元数据协同 | 本地 manifest；Hub 未接入 | P1：§3.8 Hub 协同 |
| §4.2 / PRD F-14 | 多步 Episode 主循环；StreamReport 逐步上报 | 仅 `execute_single_round` | **P1：§3.4 多步执行与流式上报** |
| PRD §4.4 | 步调感知：capacity≥80% 降速；StreamReport PACING | 未实现 | **P1：§3.4 步调/背压** |
| §7.2 / PRD §6.8 | Worker 生命周期：Ready/Busy/Draining/Terminated | 未显式建模 | **P1：§3.5 运行时状态机** |
| §7.3 / PRD §7.6 | 断连 WAL、重连重放、派发策略 | WAL 已实现 | P1：与 Server epoch 变更联动验收 |
| §11.1 / PRD §7.2 | 禁止默认 step 重试；Episode 级由 Scheduler 重投 | 单轮已遵守 | 多步/副作用环境保持边界 |
| §10.2/§10.3 / PRD §5.3–5.4 | 最小 metrics + 追踪演进 | M5/M6 最小集 | P2：§3.11 指标与 OTel 对齐 |
| PRD F-10/F-11 | MCP 路由、可组合 Reward | Reward 仅 rule 判分 | P2：§3.12 MCP/Reward 前置 |
| PRD F-19 / §9.5 | Worker OOM 重启、混沌容错 | 未实现进程管理器 | P2：§3.13 Worker 级容错 |
| PRD §3.2 / F-12 | CodeEnv 沙箱、执行级别分级 | 未实现 | P1：§3.7 沙箱与执行级别 |

---

## 3. 下一阶段功能缺口与补齐计划

### 3.1 P0：真实链路验收收口（M7 收尾）

#### 缺口

- 目前仅完成本机预联调，缺真实跨机 `uenv-server` 验收闭环。
- 缺少双侧日志交叉证据（Register / **Heartbeat（含 load 变化）** / Dispatch / Report）。
- Unix 环境下 `proto-uds` 插件全链路尚未与真实 Server 一并验收。

#### 计划

- 在真实网络路径完成至少 1 轮 GSM8K 与 1 轮 MathEnv 请求联调（Unix 环境）。
- 固化联调记录模板（request ID、Worker endpoint、Server/Worker 日志定位键、**心跳 load 快照**）。
- 与 Server 团队对齐：联调期间 Worker 至少上报真实 `active_episodes` 负载。

#### 验收标准

- 关闭 checklist 中 M7 最后一项待办。
- 输出「跨机联调证据记录」文档（含时间、环境、结果、异常与处置）。
- Server 监测侧可读到 Worker 注册信息与心跳负载非零变化。

---

### 3.2 P0：心跳·负载·资源感知补齐

> 对应架构图 Worker 内 **「心跳 / 负载 / 资源」** 三块；PRD §5.1–5.2、design §7.1/§7.4。  
> **说明**：MVP 心跳仅证明「链路通」；下一阶段须让 Server 调度与监测服务获得可用数据。

#### 设计依据

- design §7.4：`server_epoch` fencing、`next_heartbeat_interval_ms`、超时重注册。
- PRD §5.1：Worker 负载画像六维（资源 / Episode / 延迟 / 实例池 / 可靠性 / 环境亲和）。
- PRD §5.2：心跳为负载主数据源；负载新鲜度 ≤ 10s；自适应间隔（Server 下发，Worker 遵从）。
- PRD §7.7：心跳与 Episode 执行隔离，避免长 step 阻塞保活。

#### 当前缺口（代码事实）

| 项 | MVP 现状 |
|---|---|
| 心跳 `load` | 恒为 `0`，未反映 `active_episode_count` |
| `RegisterWorker.resource` | 恒为 `None` |
| `next_heartbeat_interval_ms` | Server/Mock 已下发，Worker 未持久应用 |
| `DrainCommand` | proto 已有，Worker 未处理 |
| `server_epoch` 变更 | 更新本地 epoch，未触发强制 re-register |
| 负载画像 | 无 warm 池分布、无分 env 统计、无 pacing 状态 |
| `uenv_heartbeat_lag_ms` | 指标已定义，未与心跳循环联动 |
| 心跳隔离 | 与 Tokio 主循环同任务，未独立 |

#### 计划

**阶段 A（P0，与 M7 联调并行）**

1. **心跳语义**  
   - `load` = 当前活跃 Episode 数；`max_load` = `max_concurrent`。  
   - 应用 Server 下发的 `next_heartbeat_interval_ms`。  
   - `server_epoch` 变化时触发 `RegisterWorker` 重注册。  
   - 处理 `DrainCommand`：进入 Draining，完成在途 Episode 后拒绝新 Dispatch（见 §3.5）。

2. **资源注册**  
   - 启动时采集本机 `ResourceSpec`（cpu_cores、memory_mb；gpu 可声明 0 或探测值）。  
   - 插件 manifest 聚合 `resource_requirements`，写入 Register 或心跳扩展字段（与 Server proto 对齐后实施）。

3. **负载画像最小集（经心跳上报）**  
   - Episode 维：`active_episodes`、`available_slots`。  
   - 实例池维：分 `env_type` 的 warm/active 计数、近期 hit/miss（或比率）。  
   - 可靠性维：滑动窗口内完成/失败计数（可先本地累计，心跳携带快照）。

**阶段 B（P1，Server 消费就绪后）**

4. **延迟画像**：心跳携带近期 `avg_episode_duration_ms`、`avg_model_latency_ms`（滚动均值即可，分位数可后续加）。  
5. **环境亲和**：`supported_env_types` + 各类型预热/活跃实例分布。  
6. **步调状态**：本地 capacity 利用率 ≥ 80% 时在心跳或 StreamReport 标记 `pacing_state=SLOW_DOWN`（PRD §4.4）。  
7. **心跳隔离**：独立 Tokio task 或专用线程，payload 保持极简；写入 `uenv_heartbeat_lag_ms`。

#### 验收标准

- 压测下 Server `ListWorkers` / 心跳处理可看到 `load` 随 Episode 增减变化。  
- epoch 切换后 Worker 自动 re-register，旧 epoch Dispatch 被拒绝（与 Server 联测）。  
- Drain 指令下 Worker 优雅排空，无新 Dispatch 进入。  
- 心跳间隔随 Server 下发值变化；长 Episode 执行期间心跳不中断（lag 指标可观测）。

#### 跨层依赖

- Server 侧 Worker 注册表须消费扩展心跳字段（与 Worker 团队对齐 proto 增量，避免 Worker 单边上报无效字段）。

---

### 3.3 P0：MathEnv 与通用数学环境能力

#### 设计依据

- design §3（插件化多语言实例）、§9.1（MathEnv 扩展路径）、§4.2（Episode 主循环）。
- PRD F-06、§8.1（ROLL + MathEnv）。

#### 当前缺口

- 当前稳定能力仍以 `gsm8k` 为主，缺少通用 `env_type=math` 的能力分层与可扩展输入输出约定。
- 缺「数学计算类环境」统一承载模型（不同数据集/题型/求解器仍未抽象到统一环境能力）。

#### 计划

1. 定义 `math` 环境能力边界：  
   - 统一请求字段（题目、上下文、约束、目标类型）  
   - 统一结果字段（答案、解释、判分依据、状态码）
2. 将 `math` 纳入 Worker 支持环境类型并接入预热池：  
   - Worker 启动可初始化 `math` 预热实例  
   - Dispatch 按 `env_type` 路由到 `math` 实例池执行
3. 支持「数学计算类环境实例」负载：  
   - 在同一 Worker 中可并行承载 `gsm8k` 与 `math`  
   - 统计分环境命中率、失败率与耗时（**并接入 §3.2 负载画像**）

#### 验收标准

- `math` 请求可从 Dispatch 到 Report 完整跑通。
- `gsm8k` / `math` 混合负载下无实例串扰、无双分配。
- 文档中给出 `math` 配置样例与最小回归矩阵。
- 心跳/指标中可区分两类环境的池状态与执行统计。

---

### 3.4 P1：多步 Episode 与 StreamReport 流式上报

#### 设计依据

- design §4.2（reset → step 循环 → StreamReport → ReportResult）。
- PRD F-14、§4.2/§4.6：`report_type` 含 PROGRESS / STEP_COMPLETE / REWARD_SIGNAL / **PACING**。
- PRD §4.4：Worker 端 Episode 步调自适应。

#### 当前缺口

- `EpisodeExecutor` 仅 `execute_single_round`（固定 1 step）；无 `max_steps` 主循环。
- `DispatchEpisode` 流仅回传 1 条 StreamReport，无逐步上报。
- StreamReport 字段未对齐 v7.2 完整语义（`report_type`、`model_latency_ms`、`estimated_remaining` 等）。
- 无 PACING 类型上报。

#### 计划

1. 实现通用 Episode 主循环：`reset` →（model callback → `env.step` → reward → StreamReport）× N → `close`/归还池。  
2. 每 step 经 `DispatchEpisode` 流推送 StreamReport（至少 STEP_COMPLETE；Agent 场景加 PROGRESS / REWARD_SIGNAL）。  
3. 实现 PACING 上报：capacity 高水位或推理 pending 超阈值时推送 PACING 帧。  
4. 保持 §11.1：**禁止**对 `env.step()` 默认自动重试；Episode 级重试仅由 Scheduler 新 `attempt_id` 触发。

#### 验收标准

- 多步 fixture（≥3 step）在 Mock/真实 Server 下流式可见逐步进度。  
- 单轮 GSM8K 回归不退化（单步路径仍可用）。  
- PACING 帧在人工压满并发时可被 Server/日志观测。

---

### 3.5 P1：Worker 生命周期与运行时管理

#### 设计依据

- design §7.2：Created → Ready → Busy ⇄ Ready → Draining → Terminated。
- PRD §6.8 Worker 运行时管理；PRD §7.6 断连与在途 Episode 策略。

#### 当前缺口

- 无显式 Worker 运行时状态机；Shutdown 未与心跳 Drain 联动。
- 断连期间 `reject|queue` 策略已有，但未与 Draining / epoch 失效组合验收。

#### 计划

1. 引入 `WorkerRuntimeState` 状态机，与心跳 `DrainCommand`、SIGTERM 优雅退出联动。  
2. Draining：拒绝新 Dispatch，等待在途 Episode + WAL 重放完成后退出。  
3. Busy/Ready 与 metrics、心跳 load 一致。  
4. 文档化断连 + Draining + epoch 变更的组合行为矩阵。

#### 验收标准

- 发送 Drain 后无新 Episode 进入，在途任务正常 Report。  
- 状态转换可在日志与 metrics 中追踪。

---

### 3.6 P1：Podman 后端与运行环境管理

#### 设计依据

- design §6.2–§6.4：Process（开发）→ Podman（生产）；1 容器 = 1 实例。
- PRD F-15、§3.2.3：rootless + seccomp + AppArmor；PRD 非功能「容器隔离」验收。

#### 当前缺口

- `PodmanBackend` 仅为占位；所有插件经 `ProcessBackend` 启动。
- 架构图「运行环境管理 Process/Podman」尚未落地。

#### 计划

1. 实现 `PodmanBackend` 最小路径：按 manifest 拉取/启动 rootless 容器，UDS/aRPC 与 Process 路径复用。  
2. 配置切换：`UENV_BACKEND=process|podman`，同一 `gsm8k` 插件双路径回归。  
3. 与 §3.7 沙箱策略联动：`isolated` 级别默认走 Podman。  
4. 记录启动延迟差异（metrics），供预热池与调度参考。

#### 验收标准

- `gsm8k` 在 Podman 后端完成 1 条完整 Episode。  
- Process / Podman 切换不需改插件代码。  
- 容器异常退出时 Worker 存活、实例销毁且不回 Warm 池（§6.4）。

---

### 3.7 P1：沙箱与通用环境执行能力（Code/Agent 场景前置）

#### 设计依据

- PRD F-12 CodeEnv、§8.2 VeRL + CodeEnv；design §11.1、§4.4 资源隔离。

#### 当前缺口

- 尚无可运营的沙箱执行能力规范化接入（尤其是有副作用工具调用场景）。
- 缺少通用环境分级：只读工具、受限写工具、高风险外部调用。

#### 计划

1. 定义 Worker 可识别的环境执行级别：  
   - `read_only`（可安全重试的只读调用）  
   - `side_effecting`（有副作用调用，禁止自动 step 重试）  
   - `isolated`（必须在受限沙箱或容器边界执行，默认 Podman）
2. 明确通用环境接入准入条件：能力声明、失败语义。  
3. 与 Podman 后端联合验收；为 CodeEnv / AgentEnv 插件预留 manifest 字段。

#### 验收标准

- 有副作用 step 在默认策略下不被自动重试（保持 design §11.1）。  
- 通用环境请求可按执行级别进入对应策略路径并可观测。

---

### 3.8 P1：Hub 协同接入（环境发现与能力同步）

#### 设计依据

- design §3.6；PRD §3.2.4 四级注册、F-21 UEnvHub。

#### 当前缺口

- 依赖本地静态 manifest；Worker 启动未从 Hub pull 环境定义。

#### 计划

1. 与 Hub 层对齐最小字段：`env_type`、版本、能力标签、后端要求、资源画像、镜像地址。  
2. Worker 启动 pull / 增量更新 / 失败回退本地 manifest。  
3. pull 结果驱动 `LocalRegistry` 与预热池 `env_types` 列表。  
4. Hub 不可用时不阻断 Worker 启动。

#### 验收标准

- Worker 能识别并加载来自 Hub 的环境能力信息。  
- Hub 不可用时自动降级回本地配置。

---

### 3.9 P1：WorkerPoolRegistry 资源目录

#### 设计依据

- design §1.4、§7.0：`ListWorkers` / `GetCapacity` 只读查询；返回 endpoint / load / warm 状态。
- 架构图：Worker Pool 作为 Resource Registry，**不转发 Episode**。

#### 当前缺口

- `uenv-worker/src/registry/worker_pool.rs` 为空占位；资源查询仅在 Mock Scheduler 侧内存实现。

#### 计划

1. 明确部署形态：单 Worker 进程内 registry vs 侧车聚合多 Worker（先实现前者，文档预留后者）。  
2. 暴露与 ControlPlane `ListWorkers` 一致的内存视图：本节点 endpoint、load、supported_envs、warm 池快照。  
3. 与 §3.2 心跳/load 共用数据源，避免双写不一致。

#### 验收标准

- Mock/真实 Server 查询到与心跳一致的 load 与 warm 状态。  
- 无 Episode 经 Registry 转发（架构红线不变）。

---

### 3.10 P2：预热池动态化与容量治理

#### 设计依据

- design §5.4（`WarmupSizer`）、§10.2；PRD §6.4–§6.5。

#### 当前缺口

- 固定参数；无法根据负载与历史请求动态收敛。

#### 计划

- 实现 `WarmupSizer`：基于命中率、请求速率、活跃执行量、分 env_type 统计调节目标池大小。  
- 参数护栏：扩缩容步长、冷却窗口、最大/最小池界限。  
- 与 §3.2 实例池画像、Server 预热策略 eventual 合并（联调后）。

#### 验收标准

- 波动负载下，命中率与启动延迟相较固定池策略可量化改善。

---

### 3.11 P2：可观测性与分布式追踪增强

#### 设计依据

- design §10.2–§10.3；PRD §5.3–§5.4、F-22。

#### 当前缺口

- 指标为 M5/M6 最小集（counter/ gauge 简化实现），缺 histogram、缺 PRD 列出的调度侧指标对齐。  
- gRPC metadata `x-uenv-trace-id` 未全链路透传；无 OTLP exporter。

#### 计划

1. 指标升级：`uenv_episode_duration_ms` 等改为 histogram；补齐 `uenv_worker_load_ratio`、分 env 失败率等（与 PRD 命名对齐）。  
2. trace_id 从 `EpisodeRequest` 贯穿 Dispatch 流、ReportResult、日志行。  
3. 可选接入 OpenTelemetry SDK（Jaeger/OTLP），与 Server 追踪体系对齐。  
4. 告警基线：心跳丢失、Episode 失败率、WAL 积压（Worker 侧 exporter 提供输入）。

#### 验收标准

- `/metrics` 可 scrape PRD 核心 Worker 指标子集。  
- 单次 Episode 可通过 trace_id 关联 Dispatch 流与 Report 日志。

---

### 3.12 P2：MCP 路由与 Reward 系统扩展（Worker 侧前置）

#### 设计依据

- PRD F-10 MCPEnvironment、F-11 Reward 组合容器；design §4.2 Reward 链。

#### 当前缺口

- `RewardEngine` 仅 rule 判分；无 WeightedSum / Gate 等组合。  
- 插件内无 MCP list_tools / call_tool 标准路由；Worker 未识别 MCP 型 env。

#### 计划

1. 扩展 `RewardEngine`：支持 manifest 声明的 Reward 链配置（最小：WeightedSum + RuleReward）。  
2. 定义 MCP 环境插件协议约定（工具调用仍发生在插件子进程，Worker 负责超时/隔离/不重试边界）。  
3. 为 AgentEnv（PRD §8.3）预留 `env_type` 与多步循环联调入口。

#### 验收标准

- 至少 1 个组合 Reward 配置在 Worker 内可运行。  
- MCP 型插件在 Process 后端完成 list_tools + call_tool 冒烟（单 Worker 本地）。

---

### 3.13 P2：Worker 级容错与生产化验证

#### 设计依据

- PRD F-19、§9.5 混沌测试；design §6.4、§11.2。

#### 当前缺口

- 无 Worker 进程管理器（OOM / panic 自动拉起）。  
- 混沌场景仅 Mock 开关覆盖，未做 Worker OOM、磁盘满等系统级注入。

#### 计划

1. 文档化 + 可选实现：systemd/k8s 重启策略与 Worker 本地状态重建（池重建、WAL 重放）。  
2. 扩展 M1.7 混沌矩阵：Worker OOM、磁盘满（WAL 失败降级）、长分区。  
3. 与 Server 验证：Worker 重启后在途 Episode 由 Scheduler 重分配，WAL 结果仍可 ack。

#### 验收标准

- Worker 被 kill 后进程可恢复；注册与预热池重建成功。  
- 至少 2 项混沌场景有自动化或脚本化回归记录。

---

## 4. 推荐实施顺序（下一阶段）

1. **P0 并行收口**：真实跨机 M7（§3.1）+ 心跳/负载/资源语义（§3.2 阶段 A）。  
2. **P0 扩环境**：MathEnv + 多 env 预热池（§3.3），联调验证负载画像分 env 维度。  
3. **P1 执行能力**：多步 Episode + StreamReport（§3.4）→ 生命周期/Drain（§3.5）→ Podman（§3.6）。  
4. **P1 生态与目录**：沙箱分级（§3.7）+ Hub 协同（§3.8）+ WorkerPoolRegistry（§3.9）。  
5. **P1 负载闭环**：§3.2 阶段 B（延迟/亲和/步调）与 Server 调度联调。  
6. **P2 生产化**：动态预热（§3.10）→ 观测/追踪（§3.11）→ MCP/Reward（§3.12）→ 容错混沌（§3.13）。

---

## 5. 下一阶段完成定义（DoD）

- Worker 可稳定负载 `gsm8k + math` 两类环境请求，且**心跳 load / 池状态可被 Server 用于调度感知**。  
- 完成真实 `uenv-server` 跨机链路验收并留痕（含 Heartbeat 负载变化证据）。  
- Register 携带有效 `ResourceSpec`；`ListWorkers` 可返回与心跳一致的 load / warm 快照。  
- 至少 1 个多步 Episode 流式 StreamReport 跑通；PACING 信号可观测。  
- Podman 后端完成最小回归；`isolated` 执行级别有明确策略路径。  
- 形成「通用环境执行级别 + 沙箱策略」文档并落到回归测试。  
- Hub 协同最小闭环可用（可降级）。  
- （P2）WarmupSizer、OTel 追踪、Worker 混沌项按里程碑分批达标，不阻塞 P0/P1 DoD。

---

## 6. v7.2 功能矩阵对照（Worker 层责任摘选）

| PRD 编号 | 功能 | Worker 层 MVP | 本规划章节 |
|---|---|---|---|
| F-03 | Worker 基础框架 | ✅ | §3.1 跨机验收 |
| F-07 | ProcessBackend | ✅ | — |
| F-13 | 预热池 | ✅ 固定容量 | §3.10 动态化 |
| F-14 | StreamReport 流式上报 | ⚠️ 单帧 | §3.4 |
| F-15 | PodmanBackend | ❌ 占位 | §3.6 |
| F-19 | Worker 级容错 | ❌ | §3.13 |
| F-22 | Prometheus 监控 | ⚠️ 最小集 | §3.2、§3.11 |
| — | 负载画像 / 心跳语义 | ❌ | §3.2 |
| — | 步调感知 PACING | ❌ | §3.4、§3.2 |
| — | Worker 生命周期 / Drain | ❌ | §3.5 |
| — | Hub pull 环境定义 | ❌ | §3.8 |
| F-10/F-11 | MCP / Reward 组合 | ❌ | §3.12 |
| F-12/F-18 | CodeEnv / AgentEnv | ❌ | §3.7 + §3.4 前置 |

---

## 7. 参考文档

- [worker-pool-layer-design.md](./worker-pool-layer-design.md)
- [uenv-design-prd-v7.2.md](./uenv-design-prd-v7.2.md)
- [worker-pool-mvp-checklist.md](./worker-pool-mvp-checklist.md)
- [260526-1927-worker-pool-mvp-vs-full-design-gap.md](./260526-1927-worker-pool-mvp-vs-full-design-gap.md)
