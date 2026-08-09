# Hub 注册未知环境的 Worker 动态接入补齐方案

> 日期：2026-08-06  
> 状态：已完成首轮动态接入闭环；本文同时保留剩余改进要点  
> 关联：[SWE WarmupPool 与用户前端 — 一次性实施规划](./SWE-GRPO预热池与Agent-Worker调度讨论.md)  
> 目标：汇总“Hub 已注册但 Worker 未支持的环境按需接入”与“SWE WarmupPool / Agent-Worker 调度 / 前端观测”两条规划，形成当前所有相关调整的一次性实现清单。

---

## 0. 2026-08-06 实施落地核验

本轮已经把“Hub 已注册但 Worker 本地未声明的环境”从规划推进到可运行闭环，覆盖 Server 调度、Worker prepare、Hub manifest 拉取、通用 OpenEnv HTTP shim、心跳上报和用户前端展示。

### 0.1 已落地事实

| 模块 | 已落地能力 |
|------|------------|
| Proto / Control Plane | `RegisterWorker`、`HeartbeatRequest`、`WorkerInfo` 增加 `platform_features`、`backend_kinds`、`trajectory_schemas`、`tool_schemas`、`package_states`、`pool_summary`、`pool_slots`；Worker gRPC 增加 `PrepareEnvironment` |
| Server Scheduler | 仍优先调度 ready Worker；当 `env_type` / package 不匹配时，选择具备 `hub_dynamic_env` 的可准备 Worker，调用 `PrepareEnvironment`，成功后刷新 Worker capability 并重新 reserve/dispatch |
| Worker | `WarmupPool::prepare_env` 可按 env_type 触发 Hub 拉取；`EnvResolver` 只把 process/OpenEnv HTTP 兼容 manifest 注册进 `PluginHost`；container-only manifest 不再误注册为 process plugin |
| Hub manifest 映射 | `openenv_http` / `generic_openenv_plugin` / HTTP entrypoint 会生成本地 `plugins/{env_type}/run.sh` shim，shim 调用 `uenv-openenv-plugin` 转接 `/reset`、`/step`、`/close` |
| 配套设施协议 | Worker 心跳声明 `trajectory_v2_2`、`artifact_uri`、`reward_adapter_v1`、`runtime/v1`、`browser-tools/v1`；OpenEnv step 可通过 `info.artifact_uri` 携带轨迹/截图/HAR/DOM dump 等附件 URI，并通过返回 `reward` 或 reward adapter 信息接入奖励 |
| 前端 | `/fleet/workers` 和 Worker 详情页展示支持环境类型、平台能力、backend、轨迹/工具协议、已准备 EnvPackage、实例池 summary/slots |

### 0.2 已验证结果

| 验收 | 结果 |
|------|------|
| 隔离 smoke | Worker 初始只支持 `math`；Server 收到 `dyn-openenv` 后触发 prepare；Worker 从 mock Hub 拉取并生成 shim；Episode `completed`，reward=1 |
| 正式 Hub / Server / Worker | 在真实 Hub 发布 `dyn-openenv-prod@0.1.2`；正式 Server `8.130.75.157:8088` 提交 `env_type=dyn-openenv-prod`；Worker 7143 动态拉取、准备、入池并执行完成，reward=1 |
| 正式 admin/fleet | `http://127.0.0.1:50052/status` 可见 `supported_env_types=["code","dyn-openenv-prod","qa","swe"]`，且 `package_states`、`pool_summary`、`pool_slots` 含动态环境 |
| 用户前端 | `http://8.130.75.157:8888/server/worker?worker=worker-7143-pro` 已显示 `dyn-openenv-prod`、SWE EnvPackage、平台能力、轨迹/工具协议和实例池快照 |

### 0.3 当前边界

当前“动态未知环境”已支持两类路径：

- **process plugin**：Hub manifest 声明 `supported_backends:["process"]`，Worker 本地存在或可生成兼容 `run.sh`；
- **OpenEnv HTTP**：Hub manifest 声明 `openenv_http` / `generic_openenv_plugin`，或 `entrypoint` 为 HTTP URL；Worker 生成 shim 并接入统一 episode/reward/trajectory artifact 协议。

当前没有把任意 `container`/`docker` manifest 自动转成通用容器运行时。纯 `container` manifest 会被保留给已有专用路径，例如 SWE `swe_instance_pool` / Runtime Gateway / EnvPackage。若自定义 Docker 环境要走动态未知环境闭环，首轮规范要求它暴露 OpenEnv HTTP 接口，并在 Hub manifest 中声明 `openenv_http` 或 `worker_overlay.openenv.base_url`；否则需要继续补齐 generic container backend。

---

## 1. 当前事实边界

### 1.1 已存在的事实

Hub 侧已经具备环境注册和制品分发基础：

- 任意 `env_type` 的 registry manifest 发布；
- `InterfaceSchema`（Action / Observation / State）与 `config_schema` 登记；
- EnvPackage 的内容寻址分发、`image_tar` 托管、`uenv env sync`、`docker load`；
- `platform.features`、`contracts`、`worker_overlay`、`agent_defaults` 等声明字段；
- AgentBridge、Episode Stack、rubric/scorer 等组合建模。

Worker / Server 侧已经具备部分调度基础：

- Worker 注册时上报 `supported_env_types`、`synced_env_packages`、`gateway_public_url`、`load/max_load`；
- Server `RoundRobinScheduler` 会按 `env_type`、资源、容量、`env_package_id/version` 过滤 Worker；
- Worker 有 L2 `WarmupPool`，可管理本地插件实例；
- Worker 有 SWE `SweInstancePool`、Runtime Gateway、SWE EnvPackage 读取、image tar load、trajectory seal/upload；
- Server 有 Worker registry、AgentRegistry、AgentJobQueue、trajectory store、Obs/fleet 基础视图。

### 1.2 当前没有实现的事实

当前还没有完整实现：

- Server 调度失败后主动命令 Worker 从 Hub 拉取 EnvPackage 并变成支持该 `env_type`；
- Worker 远程 `PrepareEnvPackage` / `PrepareEnvironment` RPC；
- 任意标准容器环境的 generic OpenEnv/container backend；
- Agent host 按 Episode Stack 自动同步 Agent scaffold；
- Worker 心跳携带完整平台能力、池快照、包准备状态；
- SWE 资源统一纳入 Worker WarmupPool 账本并按 ready/busy 槽调度；
- 浏览器类轨迹 schema 原生支持；
- 前端以 fleet live 数据展示真实 Worker 实例池。

因此，当前真实行为仍是：

```text
Worker 先启动并声明/准备好能力
Server 只在已有能力的 Worker 中调度
EnvPackage 需要提前 sync 到 Worker
WarmupPool 管理的是 Worker 已支持的本地环境实例
```

目标行为才是：

```text
Episode/Stack 到达
  -> Server 从 Hub resolve
  -> 检查 Worker / Agent capability
  -> 必要时下发 prepare
  -> Worker sync package + load image + 启动 backend + 更新能力
  -> Agent sync scaffold + 更新能力
  -> Worker/Agent ready 后再 dispatch
```

---

## 2. 总体原则

### 2.1 平台设施与环境制品分层

框架应提供通用平台设施：

- Worker prepare / reconcile；
- EnvPackage installer；
- generic container lifecycle；
- runtime gateway / tool bridge；
- trajectory recorder / uploader；
- artifact store；
- reward adapter runner；
- AgentJob / AgentRegistry；
- WarmupPool 账本、租约、capacity admission；
- Server resolve / prepare / dispatch 状态机。

EnvPackage 应携带环境与适配声明：

- 镜像 / image tar；
- catalog / dataset；
- eval spec；
- worker overlay；
- interface / config schema；
- contracts；
- required platform features；
- tool schema；
- Agent scaffold 引用或脚本制品；
- rubric/scorer/evaluator 制品；
- artifact layout 声明。

EnvPackage 不应直接替换：

- `uenv-server` 调度核心；
- `uenv-worker` 平台后端实现；
- Server trajectory indexer；
- runtime gateway 协议核心；
- sandbox/security enforcement。

### 2.2 失败必须早失败

如果 EnvPackage / Episode Stack 声明了旧 Worker 不具备的平台能力，例如 `generic_container_backend`、`browser_trajectory_v1`、`reward_adapter_v1`，Server/Worker 应在 prepare 前或 prepare 中明确失败，而不是进入运行期半启动。

### 2.3 所有资源占用必须有租约

Worker 环境槽、Gateway session、AgentJob、容器实例必须能以同一个 episode/run 维度关联。任何完成、取消、超时、Agent 失败、Gateway destroy 都必须 release。

---

## 3. 模块改造总表

### 3.1 Hub / Registry / Episode Stack

需要补齐：

| 项 | 更改 |
|----|------|
| `platform.features` 标准化 | 固定 feature 枚举，如 `runtime_gateway`、`trajectory_v2_2`、`generic_container_backend`、`openenv_http_container`、`reward_adapter_v1`、`browser_trajectory_v1`、`swe_warmup_pool` |
| `contracts` 标准化 | 明确 `env_runtime_api`、`runtime_gateway_api`、`trajectory_bundle_schema`、`tool_bridge_schema`、`reward_adapter_schema` |
| `adapters` 字段 | 新增或规范化 reward / trajectory / artifact / tool 的适配声明 |
| Episode Stack resolve | 调度前返回 task env、EnvPackage sync plan、Agent scaffold、runtime gateway requirement、required worker features、stack digest |
| 引用校验 | 校验 env package、agent scaffold、consumer role、runtime gateway、feature 要求、dataset/config_schema |
| Sync plan digest | 继续以 `bundle_digest` 作为 Worker prepare 的幂等输入 |

推荐声明形状：

```json
{
  "platform": {
    "uenv_worker_min": "0.2.0",
    "features": [
      "generic_container_backend",
      "runtime_gateway",
      "trajectory_v2_2",
      "reward_adapter_v1"
    ],
    "consumers": ["worker", "openhands-agent"]
  },
  "contracts": {
    "env_runtime_api": "openenv_http_v1",
    "runtime_gateway_api": "runtime/v1",
    "trajectory_bundle_schema": "v2.2",
    "tool_bridge_schema": "browser-tools/v1",
    "reward_adapter_schema": "reward/v1"
  },
  "adapters": {
    "trajectory": {
      "mode": "artifact_uri",
      "artifact_paths": ["trace.zip", "network.har", "screenshots/"]
    },
    "reward": {
      "kind": "env_endpoint",
      "endpoint": "/score"
    }
  }
}
```

事实核对：Hub types 已有 `PackagePlatform.features`、`PackageContracts`、`EnvPackageManifest.worker_overlay/agent_defaults/interface`、`EpisodeStackManifest.required_worker_features/runtime_gateway/env_packages`，但 `adapters`、feature 枚举和调度侧消费尚未完整落地。

### 3.2 Proto / Control Plane

需要补齐：

| 项 | 更改 |
|----|------|
| Worker capability | 注册/心跳上报 `platform_features`、`backend_kinds`、`trajectory_schemas`、`tool_schemas`、`package_states` |
| Pool snapshot | 心跳或独立 RPC 上报 `pool_summary` 和 slots，供 Server 调度与前端观测 |
| Prepare RPC | 新增 Server -> Worker 的 prepare/reconcile 通道 |
| Lease 字段 | for-episode、DispatchEpisode、Gateway session、AgentJob 统一携带 `lease_id` / `run_id` |
| Drain 语义 | drain 时停止新租约，busy 槽归还后再下线 |

建议新增请求：

```text
PrepareEnvironment {
  env_type
  env_package_id
  env_package_version
  expected_bundle_digest
  required_features[]
  contracts
  consumer = worker
}

PrepareEnvironmentResponse {
  ready: bool
  env_type
  backend_kind
  package_state
  message
}
```

Pool snapshot 形状：

```json
{
  "pool_summary": [
    {"env_type":"swe","variant":"pro","ready":2,"busy":1,"warming":0,"capacity":3}
  ],
  "slots": [
    {"slot_id":"swe-pro-0","status":"ready","env_type":"swe","variant":"pro"},
    {"slot_id":"swe-pro-1","status":"busy","env_type":"swe","variant":"pro","episode_id":"ep-1","session_id":"sess-1"}
  ]
}
```

事实核对：`scheduler.proto` 当前只有 `RegisterWorker.supported_env_types`、`synced_env_packages`、`gateway_public_url` 和心跳 `load/max_load`，无 prepare RPC、无 pool snapshot、无平台 feature 列表。

### 3.3 Worker EnvPackage Installer

需要补齐 Worker 可被 Server 触发的安装器：

```text
PrepareEnvPackage(package_id, version, consumer=worker, expected_bundle_digest)
```

职责：

1. 拉 Hub package manifest / sync plan；
2. 下载 inline artifacts；
3. 校验每个 sha256 与 bundle digest；
4. 对 `image_tar` 执行 docker/podman load；
5. 读取 `manifest.json`、`worker_overlay`、`contracts`、`interface`、`adapters`；
6. 校验 Worker platform feature 是否满足；
7. 如果 backend 已支持，注册 `env_type -> backend`；
8. 更新本地 package state；
9. 下一次 heartbeat 上报 ready。

幂等规则：

- 相同 `package_id@version@bundle_digest` 已 ready 直接返回；
- digest 不一致必须拒绝，不做覆盖；
- prepare 中断后可重试；
- sync 中状态必须可观测，避免 Server 重复派发多个并发 prepare。

事实核对：当前 `uenv env sync` 是 CLI 侧能力；Worker 只读取已经同步到本地的 SWE EnvPackage 目录。Worker 内没有远程触发的 installer。

### 3.4 Worker Generic Container / OpenEnv Backend

这是突破 `qa/code/swe` 固定类型的核心。

需要新增 backend：

```text
backend_kind = openenv_http_container
```

读取：

- image / image_tar；
- entrypoint；
- health_check_path；
- resources；
- interface；
- config_schema；
- contracts.env_runtime_api；
- adapters.reward / adapters.trajectory / adapters.artifact。

推荐运行协议：

| 方法 | 路径 | 含义 |
|------|------|------|
| GET | `/health` | 健康检查 |
| POST | `/reset` | 创建/重置 episode |
| POST | `/step` | 执行动作，返回 observation/reward/done/info |
| GET | `/state` | 读取状态 |
| POST | `/close` | 释放 episode |
| POST | `/score` | 可选 reward endpoint |

Worker generic backend 负责：

- 启动容器；
- 等待 health；
- reset/step/state/close；
- 将 EpisodeRequest payload 映射为 reset config；
- 将 Agent/LLM action 映射到 `/step`；
- 汇总 reward / terminated / artifact；
- 记录 trajectory；
- 回收容器。

事实核对：当前普通 L2 路径依赖 `plugins/{env_type}`；SWE 路径是专用 `SweInstancePool`。没有任意标准 HTTP 容器 backend。

### 3.5 Worker WarmupPool / SWE Pool 统一账本

需要并入 `SWE-GRPO预热池与Agent-Worker调度讨论.md` 的全部关键点：

| 项 | 更改 |
|----|------|
| SWE 入 Warmup 账本 | 对外统一 WarmupPool 账本；对内 SWE 可委派 `SweInstancePool` |
| K 槽 | 一台 Worker 持有 K 个 ready/busy/warming 槽；`gateway.capacity = SweInstancePool.capacity = Warmup 可 busy 槽 <= worker.max_concurrent` |
| 分桶 | 池键至少包含 `env_type=swe` + `benchmark_variant`；generic backend 也应支持按 env_type/package/config 分桶 |
| 租约 | Server reserve -> Worker/Gateway 校验 -> complete/cancel/fail release |
| Gateway/native 共用 | Agent、native `DispatchEpisode(swe)`、Gateway for-episode 共用同一账本 |
| Snapshot | Worker 真值 -> Server last-known -> fleet/admin/Obs/frontend |
| Drain | drain 停止新租约，等待 busy 归还 |

对未知环境的泛化：

- `slot_key = env_type + env_package_id/version + backend_kind + runtime_config_hash`；
- SWE 的 `variant` 是 slot key 的特例；
- generic container 的 ready 槽可以是 image 已加载 + container template ready，也可以是 warm container，必须在 schema 中区分。

事实核对：当前 L2 `WarmupPool` 和 SWE `SweInstancePool` 并列，SWE 尚未统一进入 Warmup 账本；心跳也未带 slots/pool summary。

### 3.6 Server Scheduler / Prepare-Reconcile 状态机

Server 需要从“只挑现成 Worker”变成“resolve -> prepare -> dispatch”。

状态机：

```text
PendingResolve
  -> Resolved
  -> SelectingReadyWorker
  -> PreparingWorker
  -> PreparingAgent
  -> ReadyToDispatch
  -> Dispatching
  -> Running
  -> Completed / Failed
```

调度条件：

- env_type 匹配；
- package digest 匹配；
- required worker features 满足；
- backend kind 满足或可 prepare；
- pool ready/busy/capacity 满足；
- resource_spec 满足；
- runtime gateway required 时 `gateway_public_url` 可用；
- Agent stack required 时 AgentRegistry 有匹配 scaffold 和容量。

错误分类：

| 错误 | 含义 |
|------|------|
| `NO_REGISTERED_ENV` | Hub 不存在 env 或 stack |
| `NO_CAPABLE_PLATFORM` | 没有 Worker 具备所需平台 feature |
| `NO_PREPARABLE_WORKER` | 有 Worker 资源，但缺 backend 或 prepare 条件 |
| `PREPARE_FAILED` | sync/load/backend 启动失败 |
| `NO_MATCHING_ENV_PACKAGE` | 没有 Worker ready 且无法准备对应 digest |
| `NO_AGENT_SCAFFOLD` | 要求 agent，但 AgentBridge 未发布或 Agent host 未 sync |
| `NO_READY_WORKER` | 能力满足但容量不足 |

事实核对：当前 `RoundRobinScheduler` 能分类 `NoMatchingEnvType`、`NoMatchingEnvPackage`、`AllWorkersAtCapacity`，但不会触发 prepare。

### 3.7 Agent Scaffold Reconcile

需要补齐：

- Server 从 Episode Stack resolve 出 `agent_scaffold`；
- Agent host 按需同步 `uenv-agent-xxx@version`；
- Agent 注册/心跳上报 `synced_agent_bridges`、`agent_kind`、labels、容量；
- Server 只向匹配 scaffold 且未满载的 Agent 投递 AgentJob；
- AgentJob 携带 `env_package_id/version`、`agent_bridge_id/version`、`lease_id`、`run_id`；
- Worker 与 Agent 均 ready 后再 dispatch。

并入 SWE 规划事实：

- 不新增 Agent WarmupPool；
- 继续使用现有 AgentRegistry admission；
- Agent 总并发必须 >= Worker K；
- OpenHands runner / systemd env 要与 K 对齐；
- Agent 失败、取消、超时必须释放 Worker 槽和 Agent load。

事实核对：当前 Agent 注册和 `synced_agent_bridges` 已有基础，自动 scaffold sync/reconcile 尚未完成。

### 3.8 Trajectory / Artifact / Reward Adapter

需要把“框架通用设施 + 环境声明适配”固定下来。

#### Trajectory

通用模式：

| 模式 | 含义 |
|------|------|
| `tool_steps` | 动作经 Worker/Gateway，Worker 记录 step |
| `env_emitted_events` | 容器按标准事件流输出，Worker 转为 TrajectoryBundle |
| `artifact_uri` | 大文件 trace/HAR/截图外置，bundle 中挂 URI |
| `custom_schema` | 要求 Worker 支持新 schema，如 `browser_trajectory_v1` |

当前 SWE v2.2 是 `tool_steps`，且 action kind 只有 `exec/read/write/provision_reset`。浏览器 trace、截图、HAR、DOM dump 第一版应走 `artifact_uri`；若要 `browser.click` 等一等公民，需要升级 Worker `StepAction` / `StepObservation` / Server 索引或前端读取逻辑。

#### Reward

受控模式：

| 模式 | 含义 |
|------|------|
| `env_returned` | `/step` 或 `/submit` 返回 reward/resolved |
| `env_endpoint` | Worker 调容器内 `/score` |
| `rubric_scorer_ref` | Hub 托管 scorer 文件，digest pin，受控 runner 执行或 auditor 复算 |
| `worker_builtin` | Worker 内置通用 exact/trim 等简单 scorer |

禁止默认让 EnvPackage 注入任意 Worker 核心代码。需要代码执行时，应在容器内、Agent scaffold runner、或受控 scorer runner 中执行，并由 manifest digest pin。

事实核对：当前 SWE trajectory seal/upload 已存在；math/code plugin 走 proto `EpisodeResult.trajectory`，未统一 JSON bundle；rubric/scorer Hub 分发已有设计，generic reward adapter runner 尚未实现。

### 3.9 Worker Runtime Safety / Container Lifecycle

必须把 2026-08-06 SWE 容器残留诊断一并纳入：

| 项 | 更改 |
|----|------|
| exec 超时 | `exec_raw` / Gateway exec 强制落实 timeout，超时 kill `docker exec` 进程树 |
| destroy 可靠性 | complete/cancel/fail/drop 均明确 destroy 或归还槽，打可检索日志 |
| 启动 reconcile | 扫描 `uenv-*` 残尸，按账本重挂或清理 |
| orphan 指标 | 暴露超龄容器、孤儿 exec、exec timeout 次数 |
| Agent 命令纪律 | retry 类探测命令强制 timeout 包装 |
| harness timeout | 数据集测试命令统一可配置上限 |

这些不是 SWE-only 的边角问题；generic backend 若引入任意容器环境，也必须同样具备 timeout、kill、destroy、reconcile。

### 3.10 Server Admin / Obs / Frontend

需要并入 SWE 规划的观测改造：

- Server 保存 Worker last-known pool snapshot；
- Admin `/fleet/workers` 输出 pool summary、slots、capability、package states；
- Obs `WorkerStatusObservation` / `WorkerView` 支持 `env_instances`、`pool_summary`；
- 前端 `useWorkerFleetLive` 以 fleet live 全量覆盖实例列表；
- Worker 详情显示 ready/busy/warming、package state、capability、陈旧提示；
- 旧 Worker 未上报池字段时显示“未上报”，不能显示成“0 已加载”。

事实核对：现有前端/Obs 有 WorkerView 基础，但 fleet live 池字段和实例覆盖尚未按 SWE 规划落地。

---

## 4. 一次性落实清单（不按时间拆分）

### Hub / Manifest

- [ ] 固定 `platform.features` 枚举和语义。
- [ ] 固定 `contracts` 字段版本语义。
- [ ] 增加或规范 `adapters`：trajectory、reward、artifact、tool。
- [ ] Episode Stack resolve 返回调度可直接消费的完整 resolved plan。
- [ ] cross_check 校验 env package、agent scaffold、runtime gateway、worker features、consumer role。

### Proto / API

- [ ] 扩展 Worker register/heartbeat 或新增 capability RPC。
- [ ] 增加 pool snapshot schema。
- [ ] 增加 Worker prepare/reconcile RPC。
- [ ] 统一 lease 字段在 DispatchEpisode、for-episode、Gateway session、AgentJob 中的传递。
- [ ] 定义 prepare / dispatch / release 错误码。

### Worker

- [ ] 实现 EnvPackage installer。
- [ ] 实现 generic OpenEnv/container backend。
- [ ] 将 SWE 纳入统一 WarmupPool 账本。
- [ ] 支持 slot key 分桶：env_type、package、variant/config、backend。
- [ ] 支持 K 槽 ready/busy/warming。
- [ ] Gateway/native/Agent 共用 lease 校验和 release。
- [ ] 心跳上报 capability、package states、pool summary、slots。
- [ ] 轨迹 adapter 支持 `tool_steps` / `artifact_uri`，预留 `env_emitted_events`。
- [ ] reward adapter 支持 `env_returned` / `env_endpoint` / `worker_builtin`，预留 rubric runner。
- [ ] exec timeout、kill-on-timeout、destroy、启动 reconcile、orphan metrics。

### Server

- [ ] 调度入口接入 Hub resolve。
- [ ] 增加 prepare/reconcile 状态机。
- [ ] 支持 ready worker 优先，否则 preparable worker。
- [ ] 按 pool ready/busy/capacity 进行 reserve。
- [ ] 统一 lease 生命周期和失败回滚。
- [ ] Agent scaffold 与 Worker package 均 ready 后再 dispatch。
- [ ] 保存并暴露 Worker last-known capabilities / pool snapshot。
- [ ] Admin/fleet/Obs 输出新字段。

### Agent

- [ ] Agent host 支持 scaffold sync/reconcile。
- [ ] Agent 注册/心跳补全 agent_kind、synced bridge、labels、容量。
- [ ] AgentJob 带 stack resolved 坐标、lease、run_id、package/scaffold 版本。
- [ ] Agent 并发与 Worker K 对齐。
- [ ] Agent 命令执行加 timeout 纪律。

### Frontend / Obs

- [ ] 类型补齐 pool summary、slots、capability、package states。
- [ ] Fleet live 作为 Worker 实例详情的优先真源。
- [ ] Worker detail 展示 ready/busy/warming、package、backend、陈旧状态。
- [ ] 兼容旧 Worker：显示未上报，不误报 0。

### Tests / Validation

- [ ] Hub stack resolve 单测：包、scaffold、feature、consumer、gateway 组合。
- [ ] Worker prepare 幂等、digest mismatch、sync 中断恢复。
- [ ] Generic backend reset/step/score/close 冒烟。
- [ ] Server prepare -> dispatch -> release 状态机测试。
- [ ] 双 lease 并行、session 失败回滚、Agent 失败回滚。
- [ ] SWE K 槽 ready/busy/warming 与 fleet/UI 对账。
- [ ] exec 无限循环 timeout 后无残留容器/进程。
- [ ] 旧 Worker 灰度兼容。

---

## 5. 漏洞检查与约束

### 5.1 不能只补 Hub 声明

仅增加 manifest 字段不会让 Worker 具备执行能力。必须同时有 Worker prepare、generic backend、Server resolve/reconcile。

### 5.2 不能只补 Worker sync

Worker 自动 sync EnvPackage 只能解决“包没在本地”的问题，不能解决“Worker 不知道如何执行该 env_type”。generic backend 或受控 plugin ABI 是必要条件。

### 5.3 不能让 EnvPackage 任意改平台核心

允许包带环境代码、scorer、Agent scaffold、tool schema；不允许包替换 Server 调度、Worker sandbox、trajectory indexer。否则安全和可复现性不可控。

### 5.4 不能忽略租约回滚

Server reserve 后，任何 create_session、prepare、AgentJob、dispatch 失败都必须释放 Worker reservation 和环境槽。否则 K 槽会逐步泄漏。

### 5.5 不能把浏览器大文件塞进 step observation

Playwright trace、HAR、截图、DOM dump 应先作为 artifact URI。只有前端/索引确实需要浏览器 step 一等公民时，再升级 `browser_trajectory_v1`。

### 5.6 不能把 reported_load 当成唯一调度真值

并发场景中必须用 Server reservation + Worker pool busy 快照共同防超卖。心跳有延迟，单靠 reported load 会竞态。

### 5.7 不能把旧 Worker 的无池字段解释为 ready=0

灰度期间旧 Worker 未上报 pool snapshot，应显示“未上报”或走旧调度兼容路径，不能误判为空池。

### 5.8 不能先扩大 K 再处理 exec/container 泄漏

2026-08-06 实机已证明无限 retry / docker exec 无超时会留下高 CPU 活进程。K 放大会线性放大事故。

---

## 6. 与 SWE Warmup 规划的汇总关系

`SWE-GRPO预热池与Agent-Worker调度讨论.md` 聚焦已知 `swe` 环境的 K 槽、租约、Agent 并发、前端观测和泄漏修复。本文把这些要求泛化为未知 Hub 环境动态接入：

| SWE 规划项 | 泛化后的动态接入要求 |
|------------|----------------------|
| SWE WarmupPool 账本 | 所有 container/plugin backend 都进入统一 Worker slot 账本 |
| pro/smith 分桶 | slot key 泛化为 env_type + package + config hash |
| K 槽并行 | Worker capacity 与 pool capacity 同源 |
| lease 校验 | prepare、gateway、native、AgentJob 全链路租约 |
| Heartbeat pool snapshot | capability + package state + pool summary 通用上报 |
| AgentRegistry 准入 | Agent scaffold resolve/reconcile 后再投递 |
| fleet live UI | 对所有 env_type 显示 ready/busy/warming |
| exec 超时与残尸清理 | 所有容器 backend 必备安全能力 |

结论：两份规划没有冲突。SWE 规划是当前专用路径的资源账本化；本文是在同一账本和租约模型上补 Hub 动态 prepare 与 generic backend。

---

## 7. 最终目标架构

```text
Hub
  env registry / EnvPackage / AgentBridge / EpisodeStack
       |
       | resolve(stack/env/package)
       v
Server
  Resolve -> Capability check -> Prepare Worker/Agent -> Reserve lease -> Dispatch
       |                                      ^
       | PrepareEnvironment                  | heartbeat capabilities/pool/package state
       v                                      |
Worker
  EnvPackage installer
  Generic container backend / plugin backend / SWE backend
  Unified WarmupPool slot ledger
  Runtime Gateway / Tool Bridge
  Trajectory + Artifact + Reward adapters
       |
       | AgentJob / tool calls
       v
Agent host
  scaffold sync/reconcile
  poll/complete with run_id + lease + package/scaffold coordinates
```

规划事实检查：

- Hub 已有必要的 manifest/EnvPackage/EpisodeStack 基础字段，但需标准化 adapters 和 feature 枚举；
- Worker 已有本地插件 WarmupPool 与 SWE 专用池，但需统一 slot ledger；
- Server 已有 worker/agent 注册和调度基础，但需 resolve/prepare 状态机；
- trajectory/reward 已有 SWE 和 plugin 局部实现，但需 generic adapter；
- 前端已有 Worker 视图基础，但需 fleet live 池字段作为实例真源。

按上述模块一次性落实后，才能达到“Hub 注册的新 env_type 可由 Server 按需拉取到 Worker 支持，并连同 Agent、轨迹、reward、配套设施一起变为可运行状态”的目标。
