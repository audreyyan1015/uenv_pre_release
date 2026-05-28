# Worker Pool：MVP 清单完成后与完整设计的差距分析

> **文档版本**：v1.0  
> **生成时间**：2026-05-26 19:27  
> **对照依据**：
> - [UEnv — 下一代分布式训练环境框架方案 v7.1](./UEnv%20—%20下一代分布式训练环境框架方案-v7.1.pdf)
> - [worker-pool-layer-design.md](./worker-pool-layer-design.md)（v1.3）
> - [worker-pool-mvp-checklist.md](./worker-pool-mvp-checklist.md)（v1.3）
>
> **阅读对象**：完成 MVP 清单（M1–M8）后的研发排期、架构评审、与 UEnv Server 联调前的预期管理。

---

## 1. 摘要

[worker-pool-mvp-checklist.md](./worker-pool-mvp-checklist.md) 将 Worker Pool 层拆为 **M1–M8（可交付 MVP+）** 与 **M9+（生产化增强）**。按清单 **全部勾选完成** 后，将得到：

- 与 UEnv Server **契约对齐** 的 Worker 运行时（gRPC Server + ControlPlane Client）；
- **GSM8K 单环境、单轮 Episode、ProcessBackend + Proto/UDS 子进程** 的可演示闭环；
- **固定容量预热池**、最小 Prometheus 指标、WAL schema 冻结并实现落盘、Mock/真实 Scheduler 联调路径。

这与 [worker-pool-layer-design.md](./worker-pool-layer-design.md) 及 v7.1 所描述的 **完整 Layer 2** 相比，仍有 **能力广度、动态性、多环境、生产隔离、Reward/执行语义、控制面协同** 等多维差距。其中一部分为 **清单刻意推迟（M9+）**，一部分为 **M5–M8 范围内尚未完全落地的设计子集**，还有一部分为 **相对 v7.1 全文愿景的有意收敛（ADR、控制面模型）**。

**核心结论（三条）**：

1. **MVP 完成 ≠ 设计文档 W6+ 完成**：动态预热（`WarmupSizer`）、`PodmanBackend`、Cap'n/cdylib、OpenTelemetry、多 `env_type` 等明确在 M9+，需在排期中单独立项。
2. **环境实例扩展路径已冻结但只走通一条**：设计为「1 子进程 = 1 instance → 多进程并发 →（Phase 1+）单进程多 session / 容器」；MVP 仅实现 **Process + 固定池 + GSM8K**，距 v7.1 的 AgentEnv/CodeEnv/容器生产态差距大。
3. **参数与容量以静态配置为主**：`UENV_WARMUP_POOL_SIZE`、`max_concurrent` 等为默认值/配置文件；完整设计要求的 **按 QPS、命中率、Job 启动事件动态调参** 依赖 M6 指标 + M9+ `WarmupSizer`，且需 Scheduler 侧预测预热协同。

---

## 2. 对照基准说明

### 2.1 「MVP 完成」在本分析中的定义

| 口径 | 范围 | 说明 |
|------|------|------|
| **清单完整交付** | M1–M8 退出标准全部满足 | 本文主对比基准；含 WAL 持久化、真实 Server 联调 |
| **推荐 MVP+** | M1–M6 | 清单定义：Mock 下 GSM8K + 预热池可量化 |
| **最小 MVP** | M1–M5 | 清单定义：Mock 下全链路，可无预热池 |
| **当前实现快照（2026-05-26）** | M1–M6 主体完成，M5 两项未勾选，M7–M8 未开始 | 见 §7；不影响本文「清单完成后」的差分结论，但影响 **当下** 验收表述 |

### 2.2 「完整设计」在本分析中的定义

- **Layer 2 设计权威**：[worker-pool-layer-design.md](./worker-pool-layer-design.md)（含 ADR-001/002、§3.5 实例模型、§5.4 动态池、§6 Podman、§7 控制面、§11 容错）。
- **平台愿景与路线图**：v7.1 PDF（多环境类型、Reward v4.0、Agent 多轮、扩缩容、训练 Job 预热协同、场景 F-11–F-28 等）。

### 2.3 与 v7.1 的「有意差异」（完成 MVP 后仍保持）

设计文档 §15 已冻结、MVP **不应** 在补齐时改回 v7.1 旧叙述的方向：

| 主题 | v7.1（部分章节） | MVP / design v1.3（冻结） |
|------|------------------|---------------------------|
| Worker 任务获取 | 易被理解为 Pool 转发 | **Scheduler 直连 Worker `DispatchEpisode`**；Pool 仅资源目录 |
| 运行日志 | §14.1 倾向 JSON 结构化 | **ADR-001：单行文本 `.log`**，`/var/log/uenv/` |
| 服务配置 | 未强调文件格式 | **ADR-002：YAML + JSON** |
| 本地 step 重试 | 未强调 | **禁止默认 `env.step()` 重试** |
| 插件 IPC | 多协议并行 | **MVP 仅 Proto/UDS 子进程** |

以下差距分析 **默认保留上表右侧**，除非产品决策修订 ADR。

---

## 3. 差距总览矩阵

图例：**MVP 清单** = M1–M8 完成后；**完整设计** = design v1.3 + v7.1 Layer 2 目标；**当前** = 仓库 2026-05-26 状态。

| 能力域 | 完整设计 / v7.1 目标 | MVP 清单（M8 后） | 差距类型 | 建议阶段 |
|--------|----------------------|-------------------|----------|----------|
| 环境类型 | 多种（GSM8K/MATH/Code/Agent…） | 仅 `gsm8k` | 范围裁剪 | M9+ / 按环境立项 |
| 实例拓扑 | 1 进程=1 实例；Phase 1+ 可多 session | 仅 1 进程=1 实例 | 路径未走通 | Phase 1+ |
| 预热池容量 | `WarmupSizer` 动态 + Job 预测预热 | 固定 `UENV_WARMUP_POOL_SIZE` | 清单推迟 + 协同 | M9+ + Server |
| 后端 | Process → **Podman** 生产 | 仅 ProcessBackend | 清单推迟 | M9+ |
| 插件 IPC | Proto；后续 Cap'n/cdylib | 仅 Proto/UDS | 清单推迟 | M9+ |
| Episode 模式 | 单轮 + **多轮 Agent** + 工具/MCP | 单轮 GSM8K | 执行语义 | Phase 1+ |
| Reward | **v4.0 链**（Rule/RM/WeightedSum/TrajectoryReward） | 插件内规则分 + Mock 答案 | 架构简化 | Phase 1+ |
| 模型回调 | 真实 vLLM/Ray/HTTP | Mock 读 `reward_config` | 依赖外部 | 联调期 |
| 并发 Episode | `max_concurrent` 可 >1 压测 | Semaphore，M5 常 serial=1 | 未验证扩展 | M9+ |
| Worker 状态机 | Created→Ready→**Busy**→Draining | 枚举存在，驱动不完整 | 实现缺口 | M7–M8 或 M9+ |
| WorkerPoolRegistry | 独立/侧车资源目录 | Mock 内存；worker 侧占位 | 职责分散 | M7 + Server |
| WAL | 落盘 + 重放 + 指标 | M8 实现（schema M1 冻结） | 清单内待做 | **M8** |
| 租约安全 | `dispatch_token` HMAC（可选→强制） | `dispatch_lease_id` + 过期 | 安全加固 | Phase 1+ |
| 可观测性 | Prometheus + **OTel** + 直方图 | M5/M6 最小 counter/sum | 清单推迟 | M9+ |
| 扩缩容 | KEDA Worker / HPA Server / 池预测 | 无 | 平台级 | Server + 运维 |
| 配置调参 | 动态策略覆盖静态默认 | 静态 YAML/env | 运行时演进 | M9+ |

---

## 4. 分域差距与改进建议

### 4.1 环境实例扩展（用户关注点）

#### 4.1.1 设计目标（三层扩展）

```text
Layer A（MVP 已冻结）     1 插件子进程 = 1 environment instance
Layer B（MVP 部分）       多子进程并行 ≈ 多实例并发（受 UENV_MAX_CONCURRENT）
Layer C（Phase 1+）       1 进程 → N sessions；Podman 容器边界；cdylib 进程内
```

v7.1 场景（AgentEnv、CodeEnv）依赖 **Layer B 以上 + 多轮 step 循环 + MCP**；MVP 仅在 Layer A 上为 GSM8K 打通 **单步** 路径。

#### 4.1.2 MVP 完成后的状态

- ✅ `PluginHost` spawn/kill、崩溃回收、预热池进程级复用（design §3.5、§6.4）。
- ✅ 固定 `warmup_size` 预创建与 hit/miss 指标（M6）。
- ❌ 第二 `env_type`、Hub manifest 拉取与能力协商（design §3.6）。
- ❌ `PodmanBackend`（~2s 启动、rootless 隔离，v7.1 §7.3）。
- ❌ 单进程多 session（明确 **MVP 不支持**）。
- ❌ 训练 Job 启动前的 **Scheduler 侧预测预热**（v7.1 §6.5；Worker 侧仅有静态预创建）。

#### 4.1.3 建议补齐顺序

1. **M9+**：第二环境插件（如 `math`）验证 `env_type` 路由与池分队列。
2. **M9+**：`PodmanBackend` 与 `ProcessBackend` 共用 `Backend` trait，配置 `env.backend=podman`。
3. **Phase 1+**：评估是否引入 session 多路复用（需同步改 WarmupPool 语义与 §3.5 不变量）。
4. **联调**：与 Server 约定 Job 级 `PrewarmRequest(env_types, count)`，避免 Worker 静态池与全局调度脱节。

---

### 4.2 Worker 参数：默认值 → 动态调整（用户关注点）

#### 4.2.1 设计中的「静态默认」与「动态覆盖」

| 参数 | Phase 0 默认（design §5.5 / §12） | 完整设计期望 |
|------|-----------------------------------|--------------|
| `pool.warmup_size` | 2（固定） | `WarmupSizer::target_pool_size()` 按 QPS、P95 创建延迟、命中率、活跃 Episode 调整 |
| `worker.max_concurrent` | 4 | 与 KEDA 自定义指标、池大小联动 |
| `pool.max_idle_time` / `cool_timeout` / `max_episode_count` | 固定秒数/次数 | 可按 env_type 画像分档 |
| 模型回调重试 | 可配置 | 与 SLA 联动；**仍不** 重试 `env.step()` |

MVP 清单：**M6 采集** `warmup_pool_hit/miss`、`instance_pool_size`；**M9+** 才实现 `WarmupSizer`（checklist §M9+、design §5.4）。

#### 4.2.2 MVP 完成后的差距

- 配置加载已支持 YAML/JSON + env 覆盖（M3），但 **无运行时根据指标写回配置** 的闭环。
- `warmup_sizer.rs` 在仓库中为 **占位结构体**，无算法实现。
- v7.1 **扩缩容**（Worker Pod KEDA、Server HPA、池预测）不在 Worker Pool MVP 范围内。

#### 4.2.3 建议实现路径

1. **短期（M8 后、M9 前）**：文档化「推荐静态调参」— 根据 `uenv_warmup_pool_hit_total / (hit+miss)` 人工调整 `warmup_size`。
2. **M9+**：实现 `WarmupSizer` trait：滑动窗口 QPS（可从 `episode_total` 派生）、hit rate、池空等待时间；输出目标池大小并 **渐进调整**（避免抖动）。
3. **中期**：Scheduler 下发 `warmup_hint` 或 Job 级预热指令；Worker 仅执行，不替代调度决策（保持 design §1.1）。

---

### 4.3 Episode 执行与 Reward

| 项 | 完整设计 | MVP（M5 目标） | 差距 |
|----|----------|----------------|------|
| 执行循环 | `max_steps` 多步 + deadline + `StreamReport` 每步 | 单轮：1×reset + 1×step | 无多轮主循环 |
| `RewardEngine` | 按 `RewardConfig` 构建 v4.0 链 | 清单要求最小 `RuleReward`；**当前** 奖励在 GSM8K 插件 + `ModelClient` | 模块未独立 |
| `TrajectoryReward` | Episode 末延迟奖励 | 未实现 | v7.1 F-28 |
| MCP / 工具 | 插件内路由，step 间异步 | 无 | Agent 场景 |
| 模型 | `ModelClient` 调真实 endpoint | Mock 解析 JSON | 联调项 |

**改进**：将 `RewardEngine` 从插件中抽出为 `episode/reward_engine.rs`；执行器改为 **通用 step 循环**，GSM8K 仍可作为 `max_steps=1` 的特例；多轮与 TrajectoryReward 单独立项。

---

### 4.4 控制面、Worker Pool 角色与联调（M7–M8）

#### 4.4.1 设计冻结 vs v7.1 表述

- design v1.1 / checklist：**禁止** Worker `subscribe_dispatch`；**禁止** Pool 二次转发 Episode。
- v7.1 部分流程图仍含「通过 gRPC 转发」字样 — 实施上以 **Server → Worker endpoint 直连** 为准（design §7.0）。

#### 4.4.2 MVP 完成后仍缺的控制面能力

| 能力 | 说明 |
|------|------|
| **M7 真实联调** | 同一二进制 Mock/Remote；Server 读 Pool 后直连 Dispatch |
| **M8 WAL** | `WalWriter` 落盘、断连排队/拒绝新 Dispatch、重连重放、`uenv_wal_pending_records` |
| **`WorkerPoolRegistry`** | worker 侧为占位；生产由 Server/Mock `ListWorkers` — 需明确 **是否独立 `uenv-pool-registry` 进程**（v7.1 日志表有 `pool-registry.log`） |
| **`dispatch_token` 验签** | design §7.7 Phase 1+；MVP 仅 lease 字段 |
| **`CancelEpisode` / 抢占** | design §7.7.2 Phase 1+；MVP 靠 lease 过期与 superseded |
| **Worker 状态机驱动** | `Busy`/`Draining` 与并发、排空策略绑定 — 当前以指标与日志为主 |

---

### 4.5 容错与一致性

MVP（M1.7 + M8）覆盖：幂等 Dispatch/Report、租约过期/取代、心跳 epoch、WAL schema。

**完整设计额外要求**：

- 插件 crash 后 `failure_reason=PLUGIN_CRASH` 等 **枚举级** 上报（MVP 有语义，需与 proto 枚举对齐）。
- `ReportResult` 携带 `dispatch_lease_id` 且 Server 拒绝非权威 lease 的成功结果（design §7.7.4）— 需 M7 proto/Server 同步。
- Worker OOM 后池重建 — 依赖进程管理器，非 Worker 代码单独交付。

---

### 4.6 可观测性与运维

| 项 | MVP（M5–M7 补充） | 完整设计 / v7.1 |
|----|-------------------|-----------------|
| 日志 | ADR-001 文本 `.log`，`tail -f` | v7.1 曾建议 JSON — **不采纳** |
| 指标 | counter/sum 为主；`/metrics` HTTP | 直方图 bucket、更多标签维度 |
| 追踪 | `trace_id` 行内字段 | gRPC metadata + **OpenTelemetry**（M9+） |
| 混沌测试 | M1.7 子集 | M8 复用；`stale_worker_id`、流中断等需补自动化 |

**改进**：在保持 ADR-001 前提下，用 OTel Collector 从日志或 gRPC 拦截器导出 span；Prometheus 增加 histogram（`episode_duration_ms` bucket）。

---

### 4.7 仓库与平台边界（v7.1 全栈视角）

完成 Worker Pool MVP **不** 等于 v7.1 Phase 0 端到端全部就绪：

| 组件 | v7.1 角色 | MVP 关系 |
|------|-----------|----------|
| UEnv Server / Scheduler | 调度、租约、WAL 去重、Pool 查询 | **M7 依赖**；`uenv-server` 侧 WAL/backend 已标 deprecated，需 Server 团队对齐 |
| Training Adapter | ROLL/VERL/NeMo 协议转换 | Layer 4，不阻塞 Pool MVP |
| UEnvHub | 环境元数据与 manifest | Worker 可选缓存；MVP 本地 `manifest.yaml` |
| 模型推理服务 | vLLM 等 | Worker `ModelClient` 需真实 endpoint |

---

## 5. MVP 清单阶段与 design W0–W6 映射（完成后仍余 W6+）

| design §14 | 内容 | MVP 清单 | M8 后状态 |
|------------|------|----------|-----------|
| W0 | CLI、日志、YAML/JSON | M1 schema + M3 | ✅ |
| W1 | PluginHost + GSM8K + gRPC Server | M2 + M4 | ✅ |
| W2 | EpisodeExecutor + 混沌 | M5 + M1.7 | ✅（单轮） |
| W3 | WarmupPool + metrics | M6 | ✅（固定容量） |
| W4 | 真实 Server 联调 | M7 | ✅（清单内） |
| W5 | WAL 持久化 | M8 | ✅（清单内） |
| **W6+** | WarmupSizer、Podman、Cap'n | **M9+** | ❌ 未交付 |

---

## 6. 建议优先级路线图（M8 之后）

```text
P0（生产阻断）
  └─ M7 真实 Server 联调闭环
  └─ M8 WAL + 断连策略 + uenv_wal_pending_records
  └─ M5 收尾：RewardEngine 模块化 + 端到端 expected_result 测试（Unix CI）

P1（能力补齐，仍属 design 正文）
  └─ 多步 Episode 循环（仍为 GSM8K）
  └─ max_concurrent > 1 压测与 Busy/Draining 状态机
  └─ failure_reason / lease 字段与 Server proto 对齐
  └─ 第二 env_type 插件样板

P2（M9+ / Phase 1+）
  └─ WarmupSizer 动态 pool.warmup_size
  └─ PodmanBackend
  └─ OpenTelemetry + Prometheus histogram
  └─ Cap'n Proto / cdylib（评估 ROI 后再做）

P3（v7.1 场景扩展）
  └─ AgentEnv 多轮 + MCP
  └─ Reward v4.0 全链 + TrajectoryReward
  └─ CodeEnv / 沙箱
  └─ KEDA/HPA 与 Job 级预测预热（跨团队）
```

---

## 7. 当前实现快照（2026-05-26，供对照）

在 **清单未全部完成** 时，与「§2.1 清单完整交付」的额外距离：

| 阶段 | 状态 | 备注 |
|------|------|------|
| M1–M4 | 已完成 | 混沌 7/7；插件 Unix 测试在 Windows CI 为 0 tests |
| M5 | 部分 | `RewardEngine`、Mock→`expected_result` 集成测试未勾选 |
| M6 | 已完成 | 预热池与指标已接入 |
| M7 | 未开始 | Worker 侧 observability/日志已做「联调前补充」 |
| M8 | 未开始 | `WalWriter` 仍为占位 |
| M9+ | 未开始 | `WarmupSizer`、`PodmanBackend` 占位 |

因此：**当前** 谈「MVP 完成」应使用 **M6 推荐 MVP+** 口径；**完整清单 M8** 仍是与完整设计对比的合理假设基准。

---

## 8. 结论

按 [worker-pool-mvp-checklist.md](./worker-pool-mvp-checklist.md) **做到 M8**，Worker Pool 层将在 **控制面模型、GSM8K 单环境、进程级实例与固定预热池、最小可观测性、容错 schema 与落盘** 上与 [worker-pool-layer-design.md](./worker-pool-layer-design.md) 的 **Phase 0 执行路径** 对齐，并与 v7.1 在 **直连 Dispatch、Episode 粒度、预热池归属 Worker** 等关键原则上保持一致（含已知 ADR 差异）。

与 **完整设计 + v7.1 全景** 相比，主要缺口集中在：

1. **扩展性**：多环境、多轮 Agent、容器后端、单进程多 session；  
2. **动态性**：预热池与并发参数从静态配置升级为指标驱动 + Scheduler 协同；  
3. **生产化**：Podman、OTel、直方图、验签租约、独立 Pool Registry；  
4. **语义完整性**：Reward v4.0 链、TrajectoryReward、真实模型推理。

建议将 **M9+ 清单项** 拆为独立 Epic，并在 M7 联调时与 Server 团队书面确认 **「不经过 Pool 转发 Episode」** 与 **租约/WAL 字段** 的双向实现，避免 v7.1 旧图与 design v1.3 混用导致集成返工。

---

## 9. 参考资料

- [worker-pool-mvp-checklist.md](./worker-pool-mvp-checklist.md)
- [worker-pool-layer-design.md](./worker-pool-layer-design.md)
- [worker-pool-pre-mvp-architecture-adjustment.md](./worker-pool-pre-mvp-architecture-adjustment.md)
- [UEnv 方案 v7.1 PDF](./UEnv%20—%20下一代分布式训练环境框架方案-v7.1.pdf)
- [更新日志.md](./更新日志.md)（实施进度）
