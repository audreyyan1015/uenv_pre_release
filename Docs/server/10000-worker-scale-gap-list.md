# 万 worker 控制面规模化差距与落地验收方案

更新时间：2026-07-16

评估基线：远端 `/home/uenv`，源码 HEAD `229700b`。本文档描述的是 UEnv server 控制面从当前单进程内存调度架构演进到 `10000` 个 worker / agent、`10000+` 并发 episode 的差距、改造边界和验收门槛。

## 结论摘要

当前实现已经具备动态 admission、worker/agent 负载上报、dispatch lease、幂等/late-result 语义缓存、Agent pool admission、结构化日志和部分 trajectory 指标，但**不能直接宣称支持生产万 worker**。

本文档中的 P0 不是“建议优化项”，而是万 worker 上线前必须关闭的正确性、容量和运维门槛。只有在以下条件同时成立后，才能宣布“UEnv 控制面支持万 worker”：

1. P0 项全部完成，并有代码、配置和设计说明。
2. native 与 SWE AgentJob 两条路径都完成 `10000` 实例规模压测。
3. 稳态、超载、注册/重连风暴、批量掉线、server 重启、存储变慢和取消风暴测试全部通过。
4. 每项验收都有原始指标、日志、压测配置和复现命令，不能只以“请求成功”作为证据。

本文档证明的范围仅是 UEnv 控制面。它不覆盖训练通信、GPU/NPU 拓扑、集合通信、模型推理服务、checkpoint、训练数据读取等数据面能力，因此“控制面支持 10000 worker”不等于“完整万卡训练系统已经通过验收”。

## 当前自动化入口

`/home/uenv/uenv-server/stress_test/run_stress_suite.py` 是统一入口，场景定义在同目录的 `stress_suite.json`。默认执行全部场景的严格预检，不启动任何服务；严格预检失败时会自动追加仅忽略二进制时间戳的诊断预检，以便一次性汇总代码版本和环境依赖问题。

```bash
cd /home/uenv/uenv-server/stress_test

# 一次完成全部场景预检，不启动服务
python3 run_stress_suite.py

# 所有严格预检通过后，一次顺序执行整套真实环境压测
python3 run_stress_suite.py --execute --confirm-execute --confirm-swe-run
```

suite 自动保存运行前后主机快照、配置、命令日志、每个 runner 的 manifest/results/config 和总 `summary.json`。超过本地 worker 安全阈值的场景仍需独立压测集群和 `--confirm-large-run`；该确认不会绕过真实二进制、OpenHands SDK、容器运行时或其他环境门禁。

## 当前实现基线：已经具备什么、还缺什么

| 模块 | 当前真实状态 | 结论 |
|---|---|---|
| server 架构 | `ServerState`、worker/agent registry、active episode、pending result、AgentJob 主要在单进程内存中。 | 单机重启和多副本场景缺少完整状态恢复与 ownership 协议。 |
| worker scheduler | `Arc<RwLock<RoundRobinScheduler>>` 包含 `Vec<WorkerInfo>`；调度、注册、心跳、release 等路径均需要线性查找或扫描。 | 万 worker 下存在 O(N) 调度成本和全局写锁竞争。 |
| Agent registry | 单个 `RwLock<Vec<AgentInfo>>`；pool admission 使用 `DashMap`，但注册、心跳、路由、reserve 仍访问全量 Agent 列表。 | 多 pool 与高频 heartbeat/poll 会争用全局锁。 |
| dynamic admission | 远端 `config/server.yaml` 设置 `episode.queue_dynamic: true`，容量跟随 worker 注册/心跳上报容量变化。 | 已有执行并发闸门，但没有等待队列硬上限；worker 仅因心跳超时变为 degraded 时，容量不会自动从 admission 中回收。 |
| adapter 背压 | 默认限制 `64` 个 pending batch；stream 路径默认最多 `64` 个并行 sample。unary batch 没有 sample 数和总字节上限。 | 只能算间接、局部背压；大 unary batch 仍可通过 `join_all` 一次创建大量 future。 |
| async submit | `submit_episode_async` 直接 `tokio::spawn`。 | 没有有界执行队列和全局 task 数上限。 |
| AgentJob | pending 为每 pool `VecDeque`，in-flight 为内存 `DashMap`；poll 通过 `position(...)` 线性匹配 bridge。 | pending/in-flight 无硬容量上限、不可恢复、混合 bridge 队列下匹配成本高。 |
| 结果缓存 | `completed_async` 默认 TTL `3600s`、最大 `10000` 条；idempotency、cancel/result outcome 有 TTL sweeper。 | 不是完全无界，但 TTL 窗口内的幂等/outcome 条数没有硬上限；`completed_async` 超限时全量收集并排序，且 10000 条可能不足以承载 10000+ 并发结果等待。 |
| 完成广播 | `episode_broadcast` 默认/当前容量为 `1024`。 | 慢订阅者可能 lag；必须明确它是可丢观测流还是可靠结果通道。 |
| worker 控制面连接 | worker heartbeat 和 ReportResult 当前每次重新 `connect()`；server dispatch/cancel worker 也每次建立 tonic channel。 | 万 worker 下会产生连接建立风暴，不能只按 heartbeat QPS 估算。 |
| Agent poll | OpenHands runner 默认 `3s` 空轮询一次，heartbeat 默认 `10s`。 | 10000 个空闲 Agent 约产生 `3333 PollAgentJob QPS + 1000 heartbeat QPS`，应改成长轮询/stream 或抖动退避。 |
| trajectory store | SQLite WAL + 单 connection `Mutex`；body 写文件、`fsync`、rename；结果落库通过每条记录一次 `spawn_blocking`。HTTP body 已限制为 16 MiB。 | 写入串行且缺少有界写队列、批处理、队列水位和明确的存储反压语义。 |
| metrics | trajectory HTTP 已有少量 Prometheus 指标。scheduler、admission、RPC、worker/agent、AgentJob、结果延迟等业务指标缺失。 | 不能描述为完全没有 metrics，但核心调度链路不可观测。 |
| admin 查询 | `/status`、`/agents`、ListWorkers 会构造并序列化全量 worker/agent/episode/job 快照。 | 万实例下需要摘要、分页、过滤、缓存和限流。 |
| 压测 | 已有共享主机安全的真实 Rust worker runner，覆盖 math/code 与 SWE/OpenHands 环境入口以及 sync/one-step/fully-async 模式；仍没有分布式万实例、重启和长稳覆盖。 | 当前结果仍不能证明万 worker。 |

### 与真实训练场景的主要差距（2026-07-19）

当前 Gate3/Gate4 的定位是隔离环境中的调度、Worker、插件、Agent 和容器链路验证。它们可以证明部分控制面和执行链路可运行，但还不能等同于真实 veRL 训练，也不能作为“已支持万 Worker / 百万 Episode”的验收证据。使用压测结果时必须明确以下边界。

| 维度 | 当前实现 | 真实训练要求 | 差距与风险 |
|---|---|---|---|
| 构建与部署基线 | 分布式脚本仍硬编码旧的生产 PID、可执行文件路径以及 `/home/uenv/target/...` 下的 Server/Worker/插件二进制。 | 每次运行必须显式指定源码工作树和二进制，记录 Git SHA、binary SHA、配置、PID、启动时间及端口归属；压测前后动态核对被保护的正式进程。 | 可能测到 WIP 或旧二进制；硬编码 PID 失效后只能失败退出，无法形成可复现证据。 |
| 端口与网络拓扑 | Gate3 为每个 Worker 使用 `20000+`，并预留 `21000+` OBS 端口；模型模拟器使用 `18001`。 | 当前允许的隔离链路应使用 `8099`（Server）、`8000`（Worker）、`8777`（Runtime Gateway）和必要时的 `8888`（模型端点）；`8077/8088` 属于正式服务，不得占用或重启。多独立 Worker 必须有明确的私网端口范围或单端口复用/主动出站协议。 | 现有 Gate3 端口方案不满足当前云主机开放范围，禁止直接运行。仅靠现有端口不能证明 32 个或更多独立 Worker。 |
| 训练闭环 | Gate3 通过 `ExecuteBatch` 直接提交 Episode，并在 payload 中填写 `framework=verl`；没有实际 Trainer。 | 真实 veRL Trainer 应参与 batch 生成、rollout 消费、梯度/优化器更新、checkpoint、策略发布和流控，至少形成“提交 → rollout → reward/trace → Trainer 消费 → policy version 推进”的闭环。 | 填写 framework 字段不代表真实训练；当前无法验证 Trainer backlog、更新节奏、checkpoint 或训练侧反压。 |
| 策略版本与异步语义 | 确定性模型模拟器返回固定的 policy/version，one-step-off-policy 和 fully-async 主要检查字段存在。 | 训练过程中 policy version 应持续推进，并记录 rollout 版本、当前训练版本、staleness 分布、丢弃/重放策略和最大允许落后窗口。 | 当前无法证明 off-policy 数据是否按预期被接受、拒绝或降权，也无法验证异步训练中的版本滞后。 |
| 模型和任务负载 | Gate3 使用固定的 `add(a, b)` 小题，模型前若干次返回固定错误、随后返回固定正确代码，token 数、延迟和 reward 高度稳定。 | 应使用真实模型服务或来自真实训练的负载分布，覆盖 prompt/response token 长度、step 数、模型延迟、reward、超时和错误的长尾；任务应包含多个数据集和难度档位。 | 当前适合测调度吞吐，不代表真实模型排队、动态 batching、网络传输、上下文增长或复杂工具调用成本。 |
| 多轮真实性 | Gate3 已设置 `max_steps` 并让模拟器先错后对；Gate4 可配置 OpenHands `max_iterations`。结果文件尚未逐 Episode 记录和断言实际 step/模型调用次数。 | 必须保存每个 Episode 的 step 数、每步模型请求、环境反馈、终止原因和最终 reward，并断言至少发生预期轮数，而不是只检查 `max_steps` 配置存在。 | 即使脚本配置为多轮，也可能因提前终止、Task ID 识别或执行器行为而退化为单轮，当前结果无法排除该情况。 |
| Rollout 与训练字段 | Gate3 为减小干扰关闭 trajectory/OBS；Gate4 结果主要检查 status/reward，没有强制 `trajectory_id`、response IDs、mask、logprobs 和时间戳完整。 | 用于训练的成功 Episode 必须产生完整、可关联、顺序正确的 trajectory/rollout 字段；异步路径还要检查 policy/version、时间戳和去重语义。 | 当前能证明任务完成，但不能证明产物可以被 Trainer 正确消费，也不能发现 trace 静默缺失或错配。 |
| Gate4 真实 LLM 与任务覆盖 | Gate4 代码支持 `mode=llm`，但当前 Worker 主机没有已验证的 OpenHands LLM 配置；并发仅为 1/2，重复同一个 `astropy__astropy-7166` 实例。 | LLM 配置必须通过 schema、鉴权和端点连通性检查；应使用多个不同仓库/镜像/难度的 SWE 实例，覆盖多轮、长尾时延和失败任务。 | 现有 `gold` 产物只能证明容器和 Agent 基线；没有真实 LLM 运行产物。单实例和极小并发也不能代表训练工作负载。 |
| 验收口径 | Gate4 将平均 reward `< 0.999` 视为失败。 | 基础设施验收与模型质量必须拆分：基础设施看任务收敛、协议、trace、资源回收和错误率；模型质量单独报告 reward、成功率、step/token 和成本。 | 真实 LLM 未解题不等于基础设施故障；反之 reward 为 1 也不能替代协议、轨迹和资源回收验证。 |
| 规模、故障与长稳 | 当前最高实测为 32 个真实 Worker、128 个并发槽位，每种模式约 30 秒；Gate4 仅并发 1/2。 | 应逐级覆盖 64/128/256 及更高真实 Worker、多机网络、百万 Episode 完整闭环、超载、断连、重启、取消、慢存储和至少 24 小时 soak。 | 当前只能算小规模功能与短时吞吐验证，不能证明万 Worker、百万 Episode、资源无泄漏或故障恢复。 |

真实训练场景的最低验证闭环如下：

1. 从本次目标分支构建 Server、Worker、插件和 Agent 组件，运行清单记录源码 SHA、二进制 SHA、配置及端口；正式 adapter core 只做前后身份快照，不得停止、替换或重启。
2. 使用隔离的 `8099` Server、`8000` Worker、`8777` Runtime Gateway；真实模型若需跨主机服务，使用获准的 `8888`。多独立 Worker 扩容前必须先解决端口范围或协议复用问题。
3. Gate3 必须记录并断言每个 Episode 的实际 step 数；使用真实或可回放的训练负载分布，并让 policy version 随训练轮次推进。
4. Gate4 必须使用经过校验的真实 LLM 配置和多个 SWE 实例；结果必须包含可验证的 trajectory、response IDs/mask、logprobs、时间戳和终止原因。
5. 接入真实 veRL Trainer，证明 rollout 被消费并触发策略更新；分别测 sync、one-step-off-policy 和 fully-async 下的吞吐、版本滞后、队列水位与反压。
6. 基础设施通过标准与模型 reward 分开统计；任何“训练通过”结论都必须同时给出调度、trace、Trainer 消费、策略更新、资源回收和模型质量证据。
7. 完成功能 Gate 后再扩大 Worker 和 Episode 数，并加入过载、Worker/Agent 掉线、Server 重启、存储变慢、网络抖动和 24 小时长稳测试。

## 容量目标合同：改代码前必须先确定

容量目标必须写入压测配置和验收报告。缺少下列数字时，不允许用“支持万 worker”作为完成标准。

| 维度 | 必须明确的目标或测试档位 |
|---|---|
| worker / agent 数量 | 稳态 `10000`；容量模型同时覆盖 `30000` 和 `100000` registry 条目；10000 实例在 `60s` 内注册/重连的风暴场景。 |
| episode 并发 | 至少 `10000` in-flight；超载测试至少提交 `30000`，验证有界排队和稳定拒绝。native 与 SWE 分开统计。 |
| batch 边界 | 单 unary batch 最大 sample 数、最大反序列化字节数；全局 pending batch/sample/bytes；每 client/pool 的配额。 |
| 提交吞吐 | SubmitEpisode、ExecuteBatch、stream sample 的目标 QPS、batch 分布和 admission/queue/dispatch P50/P95/P99。 |
| 完成吞吐 | ReportResult、CompleteAgentJob 的目标 QPS；至少按小结果、64 KiB、1 MiB 和协议允许的最大 payload 分档。 |
| 心跳与 poll | worker 5s 心跳约 `2000 QPS`；Agent 当前默认空轮询约 `3333 QPS`，必须纳入改造前基线；记录连接建立速率而不只记录 RPC QPS。 |
| 轨迹写入 | 每秒 trajectory/result 数、body 大小分布、写队列上限、存储允许延迟和降级策略。 |
| 恢复目标 | 明确 RTO、RPO；server 重启、多 shard 故障时允许丢多少 pending、result 和 event，是否允许重复执行。 |
| 可用性目标 | 单实例是否可接受；若要求 HA，明确副本数、路由、故障切换时间和重复/乱序语义。 |
| 资源预算 | 每 worker、agent、active episode、pending result、AgentJob、cache item、连接的内存/FD/CPU 预算，按 10k/30k/100k 三档估算。 |

性能阈值必须由真实训练需求与目标机器共同确定。至少要单独定义：`admission P99`、`dispatch P99`、`ReportResult ACK P99`、`CompleteAgentJob ACK P99`、队列等待 P99、错误率、最大 RSS、FD 安全余量和网络带宽余量。

## P0：正式万 worker 前必须补齐

### P0-0：部署与配置基线

**当前风险**

- 2026-07-15 检查时，`uenv-server.service` 处于重复启动失败状态，实际监听 `8088/8077/50052` 的是已脱离 unit 管理的旧 `uenv-adapter-core` 进程。
- unit 配置的 `LimitNOFILE=1048576` 不等于游离进程实际生效值；当时游离进程 nofile 为 `65535`。
- 在服务不是唯一、可控、版本可确认的前提下，重启恢复和万连接压测结果没有可信度。

**2026-07-16 状态更新**

- 已停止游离旧进程，重新编译并部署当前代码，`uenv-server.service` 已恢复为 systemd 唯一托管；检查时为 `active (running)`、`NRestarts=0`。
- 当前运行进程的 nofile 为 `1048576`，目标端口由同一个 unit 主进程提供监听；native `math` 与 SWE gold 实链路冒烟均已通过。
- LLM Agent 多轮验证被模型网关 `backend_ready=false` 阻塞，这是模型数据面环境问题，不影响 synthetic worker 控制面基准，但必须在最终全链路验收中补测。
- 部署运行态已恢复，但 systemd unit 尚需纳入仓库，且工作树仍有未提交代码改动，因此 Gate 0 的“可复现版本冻结”尚未完成。

**必须实现**

- 任意时刻只有一个被 systemd/容器编排器托管的 server 实例占用目标端口。
- 启用生产 strict config；配置文件缺失、解析失败或非法时 fail fast。
- 增加 readiness；未完成状态恢复、队列/存储初始化或 shard ownership 获取时不得接收业务流量。
- 支持 graceful shutdown：先停止新提交，再 drain active 请求、flush 有界写队列，最后退出。
- 构建产物记录 git SHA、配置摘要和协议版本，运行时可查询。

**验收**

- 连续执行 20 次 start/stop/restart，无端口残留、游离进程和 restart loop。
- PID、监听端口、二进制 SHA、配置路径一致；restart 后 readiness 按恢复状态变化。

### P0-1：显式、有界、分层的背压

**当前风险**

- episode 在等待 admission 前已经进入 active map；等待请求和 async task 可以持续增长。
- pending batch 上限不能限制单 batch sample 数，也不能限制总 payload 字节。
- unary batch 使用 `join_all`，会同时创建整个 batch 的 future；async submit 直接 spawn。

**必须实现**

- 在创建执行 task 和大对象之前申请 pending slot；episode ID 去重与执行 admission 分离，避免用 active map 同时承担去重和无限等待队列。
- 设置全局、每 client、每 pool 的 `max_pending_episodes`、`max_pending_bytes` 和 `max_in_flight`。
- 设置 `max_batch_samples`、`max_batch_bytes`、单 request/message/metadata 大小。
- unary batch 使用有界 `buffer_unordered`/worker queue，不一次激活全部 sample。
- async submit 进入 bounded mpsc/调度队列；队列满时不再 `tokio::spawn`。
- 超限统一返回 gRPC `RESOURCE_EXHAUSTED`，业务 code 使用 `QUEUE_FULL`，并携带 `retry_after_ms`、限制作用域和当前水位。
- 排队等待受 request deadline、queue timeout 和 cancel token 共同约束。

**验收**

- 10000 容量下提交 30000 请求，RSS/task 数/队列长度稳定在配置上限附近。
- 拒绝码、重试建议和队列水位稳定；停止上游流量后队列能够回落，无泄漏、OOM 或长期悬挂 task。

### P0-2：worker scheduler 与生命周期

**当前风险**

- 每次 reserve 都扫描全量 worker；heartbeat/update/release/register 也在线性列表上持有全局写锁。
- 心跳超时只影响 eligible 判断，没有后台 stale 移除和 dynamic admission 容量回收。

**必须实现**

- worker 主表使用 `worker_id -> WorkerState` 的 O(1) 索引。
- 按 `env_type`、`env_package@version`、资源类别和 shard 维护可服务候选索引；ready/full/draining/stale 状态转换只更新相关索引。
- registry 分片；heartbeat 只锁目标 worker/目标 shard，不扫描所有 worker。
- 使用 generation/server_epoch 防止旧连接、旧 heartbeat 和重注册覆盖新状态。
- 增加 stale sweeper 和明确状态机：`ready -> draining/stale -> removed -> recovered`。
- dynamic admission 必须满足不变量：

  `admission_capacity = Σ(capacity of live, accepted, non-draining workers)`

  每次注册、容量变化、drain、心跳超时、移除和恢复只能精确增减一次。

**验收**

- 10000 worker、2000 heartbeat QPS、持续 submit/report 下，记录 scheduler lock wait、索引更新和 reserve P99。
- 同时断开 30% worker，规定时间内 registry、候选索引和 admission 容量一致收缩；恢复后精确回补，不多发/少发 permit。
- 重复注册、旧 generation 心跳、乱序心跳不能污染新状态。

### P0-3：Agent registry、空轮询和 AgentJob 队列

**当前风险**

- Agent registry 是全局 `RwLock<Vec>`。
- 默认 3 秒空 poll 在 10000 Agent 时约 3333 QPS；poll 会扫描 pool pending queue，混合 bridge 时成本进一步放大。
- AgentJob pending/in-flight 没有容量和持久化；Agent 注册不携带 active job/lease 与 server epoch。

**必须实现**

- 按 `agent_pool_id` 分片 registry，并建立 `agent_id`、bridge、label、ready capacity 索引。
- 将短轮询改为带 deadline 的长轮询、server stream/watch 或通知队列；空闲时 RPC QPS 不应与 Agent 数量按 `N / 3s` 线性增长。
- 注册/重连带 `server_epoch`、generation、active job/lease 列表；server 对 Agent 和 AgentJob 做 reconcile。
- pending 按 pool + bridge/version 建索引队列，避免每次 poll 线性扫描混合队列。
- 增加全局、每 pool、每 bridge pending 上限、TTL、优先级和公平性。
- AgentJob pending/in-flight 使用持久化队列或外部队列；明确 ack、重新投递、重复 complete、超时和 abandon 语义。

**验收**

- 10000 Agent、1000 pool、混合 bridge、持续 heartbeat/poll/complete 压测。
- 空队列稳态、任务突发和 Agent 批量重连分别记录 QPS、CPU、锁等待和领取延迟。
- server 重启后 pending/running job 可 reconcile；同一 job 不会被两个有效 owner 同时执行。

### P0-4：连接复用、gRPC 与网络保护

**当前风险**

- worker heartbeat、ReportResult、server dispatch/cancel 当前均重复建连；SWE gateway 每次创建 HTTP client。
- server 主要依赖 tonic/h2 默认参数；大 payload、慢客户端、连接风暴会放大 FD、内存和 runtime task。

**必须实现**

- worker 持有并复用到 server 的 tonic Channel；register、heartbeat、report 共享连接或明确的少量连接池。
- server 按 worker endpoint 缓存 Channel，处理 endpoint/generation 变化、失效和 idle eviction。
- Runtime Gateway 复用全局 reqwest Client 与连接池。
- 配置并验证 HTTP/2 keepalive、max concurrent streams、stream/window、message size、request deadline、连接/请求并发、load shedding。
- 注册、heartbeat、poll、report 重试使用指数退避、随机抖动和 server `retry_after`，避免同步重连。
- listener backlog、nofile、conntrack、端口范围和负载均衡器参数纳入部署检查。

**验收**

- 分别验证 10000 稳定连接和当前“短连接风暴”基线；改造后连接建立速率显著下降且 FD 有明确余量。
- server 重启时 10000 实例在 60 秒窗口内抖动重连，不能形成尖峰雪崩。
- 大小 payload 与慢客户端下，单 RPC 的内存、deadline 和错误码受控。

### P0-5：结果、trajectory 与可靠事件通道

**当前风险**

- trajectory/result SQLite 写入受单 connection mutex 串行化；每条结果独立 spawn_blocking，没有写队列水位和上限。
- `completed_async` 最大 10000 条，在 10000+ 并发下可能淘汰尚未读取的结果；超限清理包含全量扫描和排序。
- broadcast 容量 1024，不能作为可靠结果总线使用。

**必须实现**

- 结果/轨迹写入进入有界队列；批量事务提交，暴露 queue depth、wait、flush latency、drop/retry/error。
- 明确存储慢或不可用时的策略：阻塞、返回失败、只保留结果、采样/丢轨迹；不得无限创建 blocking task。
- 用压测决定 SQLite 单写是否足够；不足时拆分 shard、外部数据库/对象存储或独立持久化服务。
- `completed_async` 容量按完成吞吐 × 最大读取延迟估算；使用 O(1)/分桶/LRU 类淘汰，避免每次超限全表排序。
- 幂等、cancel/result outcome 同时设置 TTL 和最大条数，并统计命中、淘汰和拒绝。
- 将 broadcast 明确定义为 best-effort 观测流；若客户端依赖可靠完成事件，使用持久化 event log/queue，支持 offset、重放和 consumer lag。

**验收**

- 按小/中/大 payload 分档压测，并注入慢盘、磁盘满、数据库锁等待和对象存储错误。
- 已 ACK 的结果不能静默丢失；可靠事件可重放；best-effort 流发生 lag 时有明确指标和告警。
- 24 小时 soak 无 blocking task、缓存、文件、FD 或数据库 WAL 无界增长。

### P0-6：重启恢复、多副本与 shard ownership

**必须实现**

- 持久化 dispatch lease、pending result、AgentJob、结果幂等语义和必要的 episode 状态。
- worker/agent 重连上报 active lease/job、generation 和真实 load；server 执行 reconcile。
- 明确 late result、旧 server epoch、重复 report、重复 complete、取消后回报的稳定协议语义。
- 若单机基准无法满足容量，按 episode/pool/env/shard key 做一致路由；取消、查询、结果上报必须路由到 lease owner。
- 多副本下需要 fencing token/lease ownership，防止两个 server 同时认领同一 episode/job。
- 明确 shard 扩缩容、再均衡、灰度和回滚时的状态迁移方式。

**验收**

- 在持续流量下 kill server/shard，验证 RTO/RPO、重复执行、结果丢失、late result 和队列恢复。
- 扩容、缩容、切流时取消/结果不会串 shard；旧 owner 不能继续接受有效写入。

### P0-7：可观测性、日志与管理接口

**必须实现**

- server `/metrics` 至少覆盖：
  - worker/agent ready、draining、stale、capacity、load；
  - active、pending、in-flight、queue bytes、queue wait；
  - admission capacity/used/wait/reject；
  - register/heartbeat/poll/dispatch/report/complete/cancel QPS、P50/P95/P99、error code；
  - scheduler candidate size、scan/index latency、lock wait；
  - connection、FD、runtime task、blocking task、RSS；
  - cache size/hit/evict、broadcast lag、result/trajectory write queue。
- 高频成功日志降为 debug、采样或聚合；当前每条 worker heartbeat 的 info 日志必须处理。
- correlation_id、episode_id、attempt、lease、worker/agent、pool/shard 保留，但避免将高基数字段直接做 Prometheus label。
- `/status`、`/agents`、ListWorkers 改为 summary + 分页/过滤；限制调用 QPS 和最大响应。
- 建立 Grafana dashboard、容量告警、SLO burn-rate 告警和故障 runbook。

**验收**

- 每个压测瓶颈都能由指标解释，不能只依赖日志猜测。
- 高频稳态下日志磁盘和序列化 CPU 占用在容量预算内；管理查询不会长时间持有 registry 锁。

### P0-8：安全、取消风暴与资源治理

**必须实现**

- worker/agent 注册鉴权，生产环境启用 mTLS 或可轮换 token；注册身份与 worker_id/agent_id 绑定。
- admin 端点强制认证并限制网络暴露；对注册、查询、取消和结果上报限流。
- 批量取消使用有界并发、批处理和重试预算；late-result 缓存设 TTL + 最大容量。
- 定义 per-client/project/pool QPS、并发、pending、结果/轨迹字节配额，防止单租户耗尽全局资源。

**验收**

- 10000 episode 同时取消时 CPU、连接、RPC task、late cache 保持有界。
- 非法注册、伪造 lease、旧 token、重复/乱序结果均被拒绝且不会污染容量状态。

## P1：P0 通过后补齐的增强能力

| 模块 | 需要补充的内容 |
|---|---|
| 调度公平性 | 按 client / project / benchmark / pool 使用公平队列，避免单一大 batch 饿死其他任务。 |
| 优先级与抢占 | 高优先级 episode、worker drain、pending 抢占；已执行任务是否允许抢占需单独定义。 |
| 自适应背压 | 根据 queue wait、存储延迟、worker/agent capacity 动态调整上游并发和 `retry_after_ms`。 |
| 配置热更新 | 队列上限、日志采样、限流和路由支持安全 reload；需要版本、校验、回滚和审计。 |
| 灰度发布 | shard 灰度、worker/agent 分批切流、协议兼容检查、自动回滚。 |
| 数据治理 | trajectory retention、压缩、冷热分层、按 run/project 清理和配额计费。 |

## 推荐落地顺序与阶段 Gate

1. **Gate 0：恢复部署基线。** 修复唯一托管进程、strict config、readiness、graceful shutdown；否则禁止开始容量测试。
2. **Gate 1：先有测量。** 完成容量模型、synthetic 10k worker/agent 工具、核心 metrics、dashboard，并保存当前基线。
3. **Gate 2：先封住无界增长。** 完成 pending/batch/bytes/task/write queue 上限、稳定错误码和上游重试协议。
4. **Gate 3：修正确性与热点。** 完成 stale 容量回收、worker/Agent 状态机、连接复用、scheduler/registry 索引和 Agent 长轮询。
5. **Gate 4：恢复与可靠性。** 完成结果/AgentJob 持久化、reconcile、可靠事件语义和故障注入测试。
6. **Gate 5：决定单机或分片。** 用单机基准判断是否需要多 shard；如果可用性目标要求 HA，则无论单机性能是否够，都必须实现 ownership/fencing。
7. **Gate 6：最终验收。** 10000 实例、真实 payload、native/SWE、过载、重启、网络抖动和 24h soak 全部通过。

任何 Gate 未通过，不进入下一阶段；不能以单元测试通过替代容量和故障验收。

## 必跑压测与故障矩阵

| 场景 | 负载 | 必看结果 |
|---|---|---|
| 注册风暴 | 10000 worker 在 60s 内注册；Agent 单独执行同规模测试。 | 注册成功率/延迟、连接建立率、RSS、FD、锁等待、admission 容量一致性。 |
| 稳态心跳 | 10000 worker、5s 心跳；持续 submit/report。 | heartbeat P99、scheduler lock/index latency、日志量、CPU、连接复用效果。 |
| Agent 空闲与突发 | 10000 Agent 空闲 10 分钟，再突发 10000 job。 | 空闲 poll QPS、领取延迟、pool/bridge 公平性、队列水位。 |
| native 满载 | 10000+ in-flight，按目标结果大小持续运行。 | admission/dispatch/report P99、结果正确性、worker 分配公平性。 |
| SWE 满载 | 10000+ AgentJob，1000 pool、混合 bridge。 | pool admission、worker+agent 双资源获取、poll/complete、gateway session 和存储延迟。 |
| 超载 | 容量 10000，提交 30000 或更高。 | 内存/task 有界、QUEUE_FULL 稳定、retry_after 有效、队列可排空。 |
| 批量掉线 | 同时断开 30% worker/Agent。 | stale 检测、permit 回收、重新调度、恢复后容量精确回补。 |
| server/shard 重启 | 持续流量中 kill -9，再恢复。 | RTO/RPO、lease fencing、reconcile、重复/丢失/late result。 |
| 取消风暴 | 同时取消 10000 episode。 | RPC/task/连接有界、worker cancel、AgentJob abandon、late cache。 |
| 存储降速/故障 | 慢盘、SQLite 锁等待、磁盘满、对象存储错误。 | 写队列、业务返回、重试/丢弃语义、无 blocking task 爆炸。 |
| 24h soak | 真实负载分布，周期性扩缩容和网络抖动。 | RSS/FD/WAL/缓存/队列无持续增长，P99 和错误率无漂移。 |

每个场景都必须分别标注：native / SWE、sync batch / stream / async submit、mock payload / 真实 payload。报告必须说明哪些字段被实际 dump 验证、哪些仅由断言覆盖、哪些受环境限制未执行。

## 最小验收标准

以下是不可放宽的规模与正确性门槛；业务延迟阈值需在“容量目标合同”中另外填写。

- 10000 worker 或 Agent 注册后，registry 数、ready/stale 状态、总容量和 admission permit 完全一致。
- 10000+ in-flight 下无重复有效 lease、无超额 reservation、无已 ACK 结果静默丢失。
- 容量为 10000 时提交 30000 请求，pending/task/RSS 保持有界，超限返回稳定 `RESOURCE_EXHAUSTED/QUEUE_FULL`。
- 30% worker/Agent 批量掉线后，规定检测窗口内容量正确收缩；重连后不重复增加 permit。
- server/shard 重启后，worker/agent active lease/job 能 reconcile；旧 owner 被 fencing，late result 语义稳定。
- native 与 SWE AgentJob 两条路径都完成万实例测试；sync batch、stream、async submit 分别有覆盖记录。
- 结果/轨迹存储变慢时不会无限创建 blocking task，不会拖垮调度控制面。
- metrics、dashboard、alert 能解释队列堆积、锁/索引热点、连接风暴、worker 掉线、result 延迟和存储变慢。
- 24 小时 soak 无 RSS、FD、runtime task、缓存、队列、日志、SQLite WAL 或磁盘占用持续无界增长。
- 压测期间的配置、git SHA、二进制 SHA、命令、原始指标和报告均归档，可重复执行。

## 支持等级判定

| 等级 | 判定 |
|---|---|
| Red：不支持 | 任一 P0 未完成；只有小规模单元/冒烟测试；存在无界队列、容量漂移或不可控重启。 |
| Yellow：实验支持 | 10000 synthetic worker/agent 可运行，但真实 payload、SWE、重启恢复、超载或 24h soak 尚未全部通过。仅允许实验环境使用。 |
| Green：生产支持 | P0 全部关闭，目标 SLO 明确，完整矩阵通过，容量余量、HA/RTO/RPO、告警和 runbook 均有证据。 |

只有达到 Green，才能对外宣称“UEnv 控制面支持万 worker”。完整“万卡训练”还需要训练与模型数据面团队分别完成对应验收。
