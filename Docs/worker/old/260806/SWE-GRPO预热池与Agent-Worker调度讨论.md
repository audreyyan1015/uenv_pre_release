# SWE WarmupPool 与用户前端 — 一次性实施规划

> **日期**：2026-08-06（由讨论稿整理为实施序）  
> **状态**：实施规划（按下列顺序一次性推进，不再按讨论时间线组织）  
> **关联**：
> - [SWE-smith 联调规划](../260801/SWE-smith环境支持与OpenHands-Rollout联调规划.md)
> - [AgentJob catalog 契约](../260802/AgentJob正统catalog注入契约固定.md)
> - [面向用户的 Worker 前端](../260805/面向用户的Worker前端设计.md)
> - [7143 SWE 容器残留与 exec 无超时诊断](./7143-SWE容器残留与exec无超时诊断报告.md)（2026-08-06 实机；运维已回收，代码项见 §9）
> - 拓扑：`secrets/README.md`（Worker 7143 / Agent 208.77 / Server 157）

---

## 0. 目标与非目标

### 0.1 目标（必须同时达成）

1. **SWE Pro + SWE-smith** 环境供给统一纳入 Worker **WarmupPool 账本**（内部可委派 `SweInstancePool`）。
2. **Server 按池状态调度**：选机 / 占槽后再建 Gateway session、投递 AgentJob。
3. **单机多实例并行**：同一 Worker 上最多 **K** 个环境槽，对应最多 **K** 个并行 Episode（1 Episode ↔ 1 session ↔ 1 容器）。
4. **用户 Worker 前端**：近实时展示池内实例（ready/busy），消除长期「环境实例 = 0」；与 Worker 真值在时效预算内一致。

### 0.2 非目标

- 新建 **Agent WarmupPool**（Agent 只用现有 `AgentRegistry` 准入；容量配到 ≥ K）。
- 取消 Gateway / 改回纯 plugin step 循环。
- 前端大改版式（接数 + 状态摘要 + 陈旧提示即可）。
- ToolEnv / Agent 本机工具环境（不在本规划「Worker 环境实例」范围内）。
- 对 Smith **全量 catalog** 启动时全量预开容器。

### 0.3 一句话

> 一台 Worker 持有 K 个 SWE 槽（Warmup 账本）→ Server 按槽并行调度 → Agent+Gateway 执行 → 心跳把池快照推到 Obs/舰队 → 用户详情页显示真实 ready/busy。

---

## 1. 冻结决策

| ID | 决策 |
|----|------|
| D1 | 对外统一 **WarmupPool 账本**；对内 SWE 仍用 Docker/`SweInstancePool` 实现 |
| D2 | **Warm 分层**：① 镜像缓存；② K 个可租用槽（ready）；③ 绑定 `instance_id` 后 provision → busy session。不以「预开 K 个匿名空容器」为主方案 |
| D3 | 池键：`env_type=swe` + **`benchmark_variant`（pro/smith）** 分桶；绑定阶段带 `instance_id` |
| D4 | 并行度 **K** 全链路对齐：`gateway.capacity = SweInstancePool.capacity = Warmup 可 busy 槽 ≤ worker.max_concurrent`；Agent 准入总和 ≥ K |
| D5 | **租约**：Server reserve → 下发 `lease_id` → Worker/Gateway 校验 → complete/cancel/fail **必 release** |
| D6 | Agent / native `DispatchEpisode(swe)` / Gateway for-episode **共用同一账本与租约** |
| D7 | 实时字段：Worker 真值 → Server last-known → 前端短轮询；**禁止**用户面直连 Worker；允许 `/fleet` 同源代理 |
| D8 | `env_instances` 展示语义 = 池内 ready+busy（机器级）；文案标明「本机池，不限本 run」 |
| D9 | 有 fleet live 时 **全量覆盖** Obs 的实例列表；心跳陈旧则标「延迟」，未升级 Worker 显示「未上报」而非「0 已加载」 |

**首期建议 K=2 冒烟 → 再升到与 `max_concurrent` 一致（如 4）。**

---

## 2. 目标架构（终态）

```text
                         Server
         ┌───────────────┼────────────────┐
         │ 选 Worker：ready[variant]>0    │
         │ 且 load < capacity(=K)         │
         │ reserve → lease_id             │
         │ Agent 准入 ≥1                  │
         ▼                                ▼
   AgentRegistry                    Worker WarmupPool(SWE)
   (max_concurrent 总和≥K)           分桶 pro|smith
         │                          slots[0..K): ready|busy|warming
         │ AgentJob + lease               │
         └──────────┬─────────────────────┘
                    ▼
            Gateway for-episode（校验 lease）
            → session/container → Episode
            → 结束/取消：release 槽 + Agent load--

Heartbeat(pool_snapshot) → Server last-known
        → WorkerStatusObservation / WorkerView
        → GET /fleet/workers（含 pool）
        → 前端 Obs + useWorkerFleetLive overlay
```

---

## 3. 现状差距（实施对照）

| 项 | 现状 | 目标 |
|----|------|------|
| SWE 执行 | 绕开 WarmupPool；`SweInstancePool` MVP **无预热** | 入 Warmup 账本；有 K 槽 + 镜像缓存 |
| Server 选机 | 主要看 worker load/capacity + gateway URL | 再看 **variant ready** |
| Heartbeat proto | 仅 `load/max_load/...`，**无池字段** | 带 `pool_summary` + 实例列表 |
| Obs / fleet | 无 `env_instances` 可靠填充 | 同源投影；fleet 可 overlay |
| 并行 | 配置 `gateway.capacity:1` + Agent 默认 1 → 体感单飞 | K≥2 可同机多 Episode |
| 前端 | 详情常显示 0；fleet 无池字段 | ready/busy 可见且近实时 |

---

## 4. 按序实施（一次性完成）

以下阶段 **按编号顺序** 推进；后一阶段依赖前一阶段的契约，不要跳过契约先改 UI。

### 阶段 1 — 契约与并行度 K（先定数字与协议）

**产出**：书面冻结 K、Warm 定义、lease 字段、Heartbeat/admin JSON 形状。

| 序号 | 工作 | 代码/文件触点（预期） |
|------|------|------------------------|
| 1.1 | 选定首期 **K=2**（联调再升 4） | 部署：`config/uenv-worker.deploy-7143-swe-pro.yaml` 等 |
| 1.2 | 扩展 `HeartbeatRequest`（或等价可靠通道） | `proto/uenv/v1/scheduler.proto`；regen bridge/server/worker |
| 1.3 | 定义 snapshot JSON：`pool_summary{variant,ready,busy,warming,capacity}` + `slots[{id,status,variant,instance_id?,episode_id?}]` | 文档冻结；前后端同一 schema |
| 1.4 | 定义 `lease_id` 在 for-episode / Dispatch / AgentJob 的传递 | proto `AgentJob` 或 gateway 请求头/体；Server `submit_swe_agent_episode` |

**疏漏补丁（本阶段必须写进契约）**：

- Heartbeat **今天没有扩展字段**——P3 不能假装「只改 Obs」；**必须改 proto 或另建控制面 RPC**。
- `max_load` 今日上报的是 `metrics.active_episode_count` 与 `max_concurrent`，与 **池 busy** 可能不一致——契约要求：**调度用的 capacity/load 与池 busy/K 同源或可推导**。
- Drain：心跳已有 `DrainCommand`——契约写明 drain 时 **停止新租约、等 busy 归还**，快照仍上报。

---

### 阶段 2 — Worker：SWE 入 Warmup 账本 + 多槽

**产出**：同机可持有 K 个 SWE 槽；Gateway/native 持租约才能开 session。

| 序号 | 工作 | 代码/文件触点（预期） |
|------|------|------------------------|
| 2.1 | Warmup 门面接管 SWE：`acquire/release/snapshot`；内委 `SweInstancePool` | `uenv-worker/src/pool/warmup_pool.rs` 或新建 `swe_warmup` 适配；`swe/instance_pool.rs` |
| 2.2 | 容量：`gateway.capacity` **直接等于** 池 capacity（去掉「仅靠 max() 隐式抬升」的运维歧义） | `runtime.rs`（今日 `swe_capacity = gateway.max(max_concurrent)`）；`config` |
| 2.3 | 分桶 pro/smith；禁止仅 `acquire("swe")` 无 variant | `executor.rs`、`runtime_gateway` for-episode |
| 2.4 | for-episode / `create_session`：**校验 lease**；无租约拒绝 | `runtime_gateway/mod.rs` |
| 2.5 | native `execute_swe_episode`：**同一 acquire/release** | `episode/executor.rs`（今日 swe 提前 return 绕开 Warmup） |
| 2.6 | 镜像预热策略：catalog 子集 / 最近使用；**禁止**全量 Smith 容器预开 | `SweInstancePool::prewarm_images`、配置 `swe.prewarm` |
| 2.7 | 异常路径 release：cancel、submit fail、gateway destroy、超时 | gateway destroy、Server 取消回调触发的 destroy、native drop |
| 2.8 | Heartbeat 携带全量 snapshot | `control_plane/client.rs` `heartbeat_once` |
| 2.9 | 指标：ready/busy/leased、泄漏告警 | `metrics.rs` |

**疏漏补丁**：

- **会话与槽的 1:1**：busy 槽必须对应唯一 `session_id`；destroy session 必须 release 槽（今日只有 session map，无 Warmup lease 层）。
- **instance 换绑**：同一 ready 槽绑定不同 `instance_id` 时要 destroy 旧容器再 provision，避免串镜像。
- **WAL / 重启**：Worker 重启后槽与 Docker 残尸——启动时 reconcile（扫容器或记租约表），否则 Server 以为有 ready、实际撞满。**2026-08-06 实机已证实**：tenacity/teleport/ansible 容器可存活数天且无 destroy 日志（见 §9 / 诊断报告）。
- **与 plugin Warmup 共存**：qa/code 继续按 `env_type` 进程池；SWE 分桶不要 `fill_pool("swe")` 去 `plugin_host.spawn`。
- **seccomp / trajectory_upload / catalog JSON**：并行 session 下已有逻辑需确认无全局可变单例互踩（submit 状态 map 已 per-session，保持）。
- **`exec` 超时与杀进程**（§9 P0）：租约/多槽落地前必须先堵住「无超时 docker exec → 活挂死占满 CPU」；否则 K↑ 会放大泄漏。

---

### 阶段 3 — Server：按池调度 + 租约

**产出**：无 ready 不派发；同机可并行最多 K 个 SWE reservation。

| 序号 | 工作 | 代码/文件触点（预期） |
|------|------|------------------------|
| 3.1 | 解析 Heartbeat 池快照写入 WorkerInfo last-known | `control_plane.rs`、`scheduler/mod.rs` WorkerInfo 扩字段 |
| 3.2 | `reserve`：SWE 请求要求 `ready[variant]>0`（或可借）且 `load<capacity` | `scheduler/mod.rs`、`submit_swe_agent_episode` |
| 3.3 | 生成并持久化 `lease_id`；传入 for-episode 与（如需要）AgentJob | `service/episode.rs`、`ports` gateway client |
| 3.4 | complete / cancel / timeout：**release worker reservation + 通知 Worker 释槽**（若 session 未建也要释） | episode 取消分支、gateway destroy |
| 3.5 | Admin `/fleet/workers`（及 status JSON）输出 pool 字段 | `admin_http.rs`、`admin_query.rs` |
| 3.6 | Obs：`WorkerStatusObservation` + merge → `WorkerView.env_instances` + `pool_summary` | `obs/event.rs`、`worker_status.rs`、`merge.rs`、`project.rs` |

**疏漏补丁**：

- **占坑早于 session**：今日先 `scheduler.reserve` 再 `create_session`；session 失败必须 **两边回滚**（已有部分错误路径，租约层要纳入）。
- **Server 重启**：`server_epoch` 已触发 Worker re-register；需规定 **租约表清空或租约带 epoch**，避免幽灵 lease。
- **多 Worker**：按 variant ready 选最空闲机；避免只钉 `worker-7143-pro`。
- **Admission / adapter 层**：若存在 Σ worker capacity 的动态 admission，K 变更要同步（`config.rs` / control_plane capacity 同步）。
- **金标 / 非 Agent SWE**：凡占 Worker SWE 资源的路径都走 3.2–3.4，不只 GRPO。

---

### 阶段 4 — Agent 与部署对齐 K

**产出**：Agent 侧接得住 K 个并行 Job。

| 序号 | 工作 | 触点 |
|------|------|------|
| 4.1 | `OPENHANDS_AGENT_MAX_CONCURRENT≥K` 或 K 个 poller | `scripts/openhands/openhands_runner.py`、208.77 systemd/env |
| 4.2 | Worker 部署：`runtime_gateway.capacity=K`，`max_concurrent≥K` | `config/uenv-worker.deploy-7143-swe-pro.yaml` |
| 4.3 | LLM/网关 QPS 与 K 匹配（避免 Agent 并行后全卡模型） | 7142 llm-gateway / 限流配置 |
| 4.4 | 资源画像：磁盘、内存、并行容器数（Pro/Smith 分开测） | 运维记录进本规划附录或联调报告 |

**疏漏补丁**：

- 工作区 / 日志目录按 `episode_id` 隔离，防并行串文件。
- CompleteAgentJob / 失败回收与 `_active_jobs` 已有逻辑，压测 K>1 时回归「卡死不再 poll」（runner 注释已警示）。

---

### 阶段 5 — 用户前端近实时展示

**产出**：有预热时详情 ≠ 0；同机多实例可见；陈旧可辨。

| 序号 | 工作 | 触点 |
|------|------|------|
| 5.1 | 类型：`pool_summary`、实例 status；兼容旧 `env_instances: string[]` | `frontend/src/lib/types/chain-state.ts`、normalize |
| 5.2 | `useWorkerFleetLive` 解析 fleet 池字段；`WorkerLiveOverlay` 增加 instances | `use-worker-fleet-live.ts`、`worker-tree.ts` |
| 5.3 | `projectWorkerDetail`：**live 全量覆盖**实例列表；无 live 回落 Obs | `worker-tree.ts` |
| 5.4 | 详情 UI：ready/busy 计数 + 列表；文案「本机池」；心跳延迟提示 | `worker-detail.tsx` |
| 5.5 | （可选）舰队列表行显示 `busy/K` | `worker-status-overview.tsx` |
| 5.6 | Fixture / 单测更新 | `fixture.ts` |

**疏漏补丁**：

- 今日详情计数优先 `worker?.env_instances?.length`，**未用 live 池**——5.3 必须改，否则舰队有数 UI 仍读 Obs 空数组。
- 260805「只连 Obs」与 `/fleet`：在前端设计文档补一句 **实时例外**，避免规范回摆。
- SSE/poll 重连后先画 Obs 再被 live 覆盖——允许一帧闪烁；不要并集追加导致幽灵 id。

---

### 阶段 6 — 联调验收（一次过检）

| ID | 场景 | 通过标准 |
|----|------|----------|
| T1 | Worker 空闲预热 | 时效预算内 UI ready=K（或配置的 ready 水位） |
| T2 | 同机并行 | **≥2** Episode 同时 ACTIVE、≥2 session、UI busy≥2 |
| T3 | Pro + Smith 各至少 1 条 | 分桶不互抢到饿死；均可 acquire |
| T4 | 占用→归还 | busy↓ ready↑；无租约泄漏（指标对账） |
| T5 | 取消 / Agent 失败 | 槽释放；可再派发 |
| T6 | 停心跳 | UI 标延迟/离线，不把旧 N 当新鲜「已加载」 |
| T7 | 对照 | Worker 本地 snapshot vs `/fleet` vs UI，差 ≤ 一个刷新周期 |
| T8 | native swe（若启用） | 与 Gateway 同一账本 |

**时效预算**：\(H+S+P\) ≤ **10–15s**（Heartbeat + Server 同步 + 前端 3s poll）。

---

## 5. 代码触点总表（防漏改）

| 层 | 路径 | 改什么 |
|----|------|--------|
| Proto | `proto/uenv/v1/scheduler.proto`（+ 可能 `agent.proto`） | Heartbeat 池快照；lease 传递 |
| Worker 池 | `pool/warmup_pool.rs`、`swe/instance_pool.rs` | SWE 账本、snapshot、分桶 |
| Worker 执行 | `episode/executor.rs`、`swe/session.rs` | swe 走租约；**§9：exec 超时杀进程、destroy 日志** |
| Worker Gateway | `runtime_gateway/mod.rs` | lease 校验；容量=K；取消杀 exec |
| Worker 控制面 | `control_plane/client.rs`、`runtime.rs` | 心跳带池；capacity 语义 |
| Worker 配置 | `config/mod.rs`、`deploy-7143-swe-pro.yaml` | K、预热策略 |
| Server 调度 | `scheduler/mod.rs`、`control_plane.rs` | last-known 池；按 ready reserve |
| Server Episode | `service/episode.rs`、gateway ports | lease 生命周期；失败回滚 |
| Server Admin | `admin_http.rs`、`admin_query.rs` | fleet 池字段 |
| Server Obs | `obs/event.rs`、`worker_status.rs`、`merge.rs` | env_instances / pool_summary |
| Agent | `openhands_runner.py` + 部署 env | max_concurrent≥K |
| Frontend | `chain-state`、`worker-tree`、`use-worker-fleet-live`、`worker-detail` | live 覆盖 + 状态展示 |
| 文档 | `260805` 前端设计 | 允许 fleet 实时例外 |

---

## 6. 追加疏漏清单（整理时对照代码仍须防）

实施中除上文「疏漏补丁」外，再核对：

1. **Proto 兼容**：旧 Worker 无池字段时 Server 不得误判 ready=0 永拒发；灰度用「未知=仅看 load」或版本门闸。  
2. **Heartbeat 载荷大小**：slots 上限按 K（个位数），勿把整份 Smith catalog 塞进心跳。  
3. **reported_load vs reserved_load**：并行下以 reservation+池 busy 为准，避免心跳 load 偏低导致超卖。  
4. **Gateway API key / 公网 URL**：多 session 无额外问题，但限流与连接数要按 K 评估。  
5. **Obs 按 training_run 存 WorkerView**：机器级池写入某个 run 的 ChainState 时，其他 run 打开详情也应能看到——确认 merge/广播是否所有活跃 run 都能刷到同一 worker 池视图，或详情主要靠 **fleet（与 run 无关）**（推荐：实例数以 fleet 为准，规避 run 作用域坑）。  
6. **权限**：`/fleet` 仅同源代理，勿暴露到公网无鉴权。  
7. **测试**：Worker/Server 单测加「双 lease 并行」「session 失败回滚」「心跳快照」；前端 fixture 含 ready/busy。  
8. **回滚开关**：feature flag「SWE 强制租约」便于现网 GRPO 空窗切换。

---

## 7. 实施顺序一览（执行清单）

```text
① 契约：K、Warm 分层、lease、Heartbeat/admin/Obs schema
② Worker：SWE Warmup 账本 + 分桶 + lease 校验 + snapshot 心跳
   （含 §9：exec 超时杀进程、destroy 可观测、启动 reconcile）
③ Server：按 ready 调度 + lease + fleet 池字段 + Obs 投影
④ Agent/部署：准入与 K 对齐；资源画像；（§9 P2：retry 类命令 timeout 包装）
⑤ 前端：fleet overlay 优先；ready/busy；陈旧提示
⑥ 验收：T1–T8（先 K=2 再升 K）+ §9 泄漏回归
```

- [ ] 阶段 1 契约冻结（含 proto）  
- [ ] 阶段 2 Worker（**含 §9 P0/P1**）  
- [ ] 阶段 3 Server  
- [ ] 阶段 4 Agent/部署（**含 §9 P2**）  
- [ ] 阶段 5 前端  
- [ ] 阶段 6 T1–T8 + §9 泄漏回归通过  

---

## 8. 修订记录

| 日期 | 说明 |
|------|------|
| 2026-08-06 | 讨论稿：Warmup / Agent / 前端 / 审查 / 多实例并行 |
| 2026-08-06 | **整理为按序一次性实施规划**；合并决策；补 proto/租约/灰度/run 作用域等代码疏漏 |
| 2026-08-06 | 实机发现 7143 tenacity/`exec` 无超时活挂死；运维已回收；**增补 §9** 代码缺陷项并入阶段 2/4/6 |

---

## 9. 追加：SWE `exec` 超时缺失与容器泄漏（2026-08-06 实机）

> 诊断全文：[7143-SWE容器残留与exec无超时诊断报告.md](./7143-SWE容器残留与exec无超时诊断报告.md)  
> **运维**：2026-08-06 20:14 CST 已 `docker rm -f` 两个 tenacity + teleport/ansible 残尸；Worker 未重启，当前 GRPO oauthlib 会话未动。  
> **本规划**：下列缺陷与阶段 2（Worker 账本/租约/reconcile）、阶段 4（Agent 命令纪律）、阶段 6（泄漏回归）**一并合入**，不要单独开长期分支遗忘。

### 9.1 实机结论（冻结）

| ID | 结论 |
|----|------|
| F1 | 挂死物是 **`R` 态活进程**（pytest 忙等），不是内核 `Z` 僵尸 |
| F2 | 容器来自 **SWE-smith + OpenHands** 正常 episode（tenacity instance）；trajectory seal + submit `reward=1.0` 后仍泄漏 |
| F3 | Agent 探索性 `pytest … retry_until*` 在错误补丁下形成无限 retry；约每 10min 重试叠加 |
| F4 | Worker `SweSession::exec_raw` 使用同步 `.output()`，**`timeout_sec` 未强制执行、超时不杀进程树** |
| F5 | episode 结束后容器销毁不可靠（日志无 destroy/rm 计数）；与 §2「WAL/残尸 reconcile」同一类债 |

### 9.2 代码改进清单（与改造一并应用）

| 优先级 | ID | 工作 | 触点（预期） | 并入阶段 |
|--------|-----|------|--------------|----------|
| **P0** | X1 | `exec_raw` / Gateway exec：**强制超时**（落实 `CommandPolicyConfig.timeout_sec`，默认 ≥120s 可配）；超时后 **kill `docker exec` 进程树**（含容器内 pytest） | `uenv-worker/src/swe/session.rs`；Gateway 调用链 | **2** |
| **P0** | X2 | episode / AgentJob / Gateway `destroy` / cancel / fail：**必 destroy 容器**；打 `swe_session_destroyed`（或等价）可检索日志；禁止静默 `Drop` 失败 | `session.rs` Drop/`keep`；`runtime_gateway`；`instance_pool` release | **2**（对齐 2.7） |
| **P0** | X3 | Worker **启动 reconcile**：扫描 `uenv-swe-*` 残尸 vs 账本；idle/无租约超时扫尾（与 max_idle / cool 对齐） | `instance_pool` / Warmup 适配；启动钩子 | **2**（对齐 2.x WAL 补丁） |
| **P1** | X4 | HTTP/工具调用 **取消或上游超时** 时，不得留下孤儿 `docker exec`（`kill_on_drop` 或显式 abort） | Gateway handler；若有 blocking threadpool 需可取消 | **2** |
| **P1** | X5 | 指标/告警：`swe_orphan_exec`、超龄容器数、exec 超时次数；心跳或 admin 可观测 | `metrics.rs`；可选 fleet 字段 | **2** + **3** admin |
| **P2** | X6 | Agent/评测侧：对 retry 类探测命令强制 `timeout N …`；或 Runner 包装 shell | `openhands` runner / tool policy；评测 `evaluate` 路径 | **4** |
| **P2** | X7 | 数据集/harness：tenacity 等「无限 retry」F2P 的评测超时上限写死（防 Agent 漏 timeout） | `repo_specs` / evaluate `test_cmd` 包装 | **2** 或 **4** |

### 9.3 验收加项（阶段 6）

| ID | 场景 | 通过标准 |
|----|------|----------|
| T9 | 容器内故意 `while true` / 无限 retry pytest | 达 `timeout_sec` 后 exec 返回错误；**无**残留 docker exec；CPU 回落 |
| T10 | AgentJob 成功 submit 后 | 对应容器在时效内销毁或回到池账本 ready；`docker ps` 无超龄 `uenv-swe-*` |
| T11 | Worker 重启 | reconcile 清掉无主容器，或重新挂账；Server 租约不幽灵占坑 |
| T12 | 取消/客户端断开 | 无孤儿 exec；槽 release |

### 9.4 实施注意

- **不要**用「先升 K」掩盖泄漏：K↑ 会线性放大占核。  
- X1/X2 可先于完整 Warmup 账本合入（热修也可），但仍登记在本规划以免双轨文档。  
- 现网若再出现超龄容器：优先 `docker rm -f` **非当前 episode** 容器；勿误杀正在 GRPO 的 instance。
