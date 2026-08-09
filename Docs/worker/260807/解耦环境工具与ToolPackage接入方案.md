# 解耦环境工具与 ToolPackage 接入方案

> 日期：2026-08-07  
> 范围：记录关于“把环境依赖、浏览器、轨迹采集、reward 计算、agent 工具等能力从 EnvPackage 中解耦为可复用工具”的方案讨论。  
> 状态：规划方案。当前代码已具备动态 EnvPackage / OpenEnv HTTP 首轮接入，但还没有完整 ToolPackage 协议和工具调度闭环。

## 1. 目标效果

用户可以把一个自定义栈级环境注册到 Hub，例如：

```text
EnvPackage: custom-browser-task@0.1.0
  - env_type = custom_browser_task
  - runtime_api = openenv_http_v1 或 process
  - required_tools = browser.playwright@1, artifact_store@v2, reward_adapter@v1
  - trajectory_schema = browser_trace_bundle_v1
  - agent_defaults = openhands / custom-agent scaffold
```

Server 收到 episode 后，如果当前 Worker 未支持该 `env_type`，应能：

1. 从 Hub resolve EnvPackage 和依赖工具。
2. 判断 Worker 是否具备或可准备所需 ToolPackage。
3. 在 Worker 上安装或启动共享工具服务。
4. 准备 env instance 并放入实例池。
5. 调度 episode 到该 env instance。
6. 把轨迹、截图、HAR、DOM dump、reward 结果作为标准 artifact / trajectory 返回。

## 2. 当前已具备的基础

260806 已落地的动态环境接入能力包括：

| 模块 | 当前能力 |
|------|----------|
| Hub / Worker | Worker 可按 `env_type` 从 Hub 拉取 manifest，首轮支持 `process` 和 `openenv_http` / `generic_openenv_plugin` |
| Server | 无 ready Worker 时，可选择具备 `hub_dynamic_env` 的 Worker 下发 prepare，再重新 reserve / dispatch |
| Worker capability | 注册和心跳可上报 `platform_features`、`backend_kinds`、`trajectory_schemas`、`tool_schemas`、`package_states`、`pool_summary`、`pool_slots` |
| OpenEnv HTTP shim | `uenv-openenv-plugin` 可把 `/reset`、`/step`、`/close` 映射到现有 episode/reward/trajectory 结果 |
| artifact 接入边界 | OpenEnv step 可通过 `info.artifact_uri` 或等价字段携带附件 URI |

这些能力解决的是“未知环境如何先跑起来”。但它还没有把浏览器、reward adapter、轨迹采集、agent scaffold 等做成可声明、可版本化、可部署、可观测的独立工具包。

## 3. 推荐抽象

### 3.1 EnvPackage

EnvPackage 只描述环境本体和接入契约：

- `env_type`
- 环境镜像、process plugin 或 OpenEnv HTTP 入口
- action / observation / state schema
- env config schema
- artifact layout
- 默认 agent / reward / tool 依赖引用
- 需要的 Worker feature

EnvPackage 不应直接改写 Server / Worker 核心代码。

### 3.2 ToolPackage

ToolPackage 描述可复用配套设施：

- 浏览器工具：Playwright service、Chrome profile、HAR capture、DOM dump、screenshot
- 轨迹工具：trace recorder、artifact bundler、checksum/seal、upload client
- reward 工具：reward adapter、grader runner、test evaluator、rubric scorer
- agent 工具：agent scaffold、driver entrypoint、tool bridge adapter
- 其他依赖服务：database、proxy、文件服务、沙箱 sidecar

ToolPackage 应具有：

- `tool_id`
- `version`
- `kind`
- `runtime`，例如 process、container、sidecar service
- `capabilities`
- `ports` / endpoint 暴露方式
- `artifact_schema`
- `healthcheck`
- `resource_spec`
- `session_isolation`

### 3.3 RuntimeProfile

RuntimeProfile 是 EnvPackage 和 ToolPackage 的组合解析结果：

```json
{
  "env_type": "custom_browser_task",
  "env_package": "custom-browser-task@0.1.0",
  "backend": "openenv_http",
  "required_tools": [
    "browser.playwright@1",
    "trajectory.browser_trace_bundle@1",
    "reward.env_endpoint@1"
  ],
  "worker_features": [
    "hub_dynamic_env",
    "tool_package_v1",
    "artifact_uri",
    "trajectory_v2_2"
  ],
  "pool": {
    "warmup_target": 4,
    "max_parallel_episodes": 4
  }
}
```

Server 调度时使用 RuntimeProfile 做准备和准入，而不是只看 `env_type` 字符串。

## 4. 浏览器轨迹类环境的推荐接入

以“环境里执行任务，并需要保存 Playwright trace、截图、HAR、DOM dump”为例，推荐流程为：

1. EnvPackage 声明需要 `browser.playwright@1` 和 `trajectory.browser_bundle@1`。
2. Worker prepare 时检查本机是否已有相同版本 ToolPackage。
3. 若没有，从 Hub 拉取 ToolPackage，启动共享 browser tool service 或按 session 启动 sidecar。
4. Episode 开始时，Worker 为 env instance 分配 `tool_session_id`。
5. Env instance 通过标准 tool endpoint 调用浏览器能力。
6. Tool service 在 session 结束时产出：
   - `trace.zip`
   - `network.har`
   - `screenshots/*.png`
   - `dom/*.html` 或 `dom/*.json`
   - `tool-events.jsonl`
7. Worker trajectory recorder 把这些路径上传到 artifact store，写入 `artifact_uri` 列表。
8. EpisodeResult 的 trajectory 只保存索引、摘要、校验和和 artifact URI，不内联大文件。

这可以是非容器侵入式的：浏览器可以在共享 sidecar 或独立工具容器中运行，EnvPackage 只通过协议调用它。但如果环境本身必须控制浏览器进程，也允许浏览器在环境容器内部运行，只要最后按 artifact 协议输出标准 bundle。

## 5. Reward 计算的两种模式

### 5.1 平台通用 reward adapter

适用于奖励逻辑可以由框架统一承载的情况，例如：

- OpenEnv `/score`
- `tests_passed / tests_total`
- rubric scorer
- diff / patch / compile / unit test 增量
- artifact 检查

EnvPackage 只声明 reward contract：

```json
{
  "reward": {
    "kind": "adapter",
    "adapter_id": "python_test_delta",
    "version": "1",
    "inputs": ["trajectory", "artifact_uri", "env_result"]
  }
}
```

Worker 调用平台内置或 Hub 拉取的 reward adapter。

### 5.2 EnvPackage 自带 reward 实现

适用于强业务逻辑、私有 grader 或环境内部状态才能评分的情况。EnvPackage 声明：

```json
{
  "reward": {
    "kind": "env_endpoint",
    "endpoint": "/score",
    "schema": "reward/v1"
  }
}
```

Worker 不需要理解内部评分逻辑，只校验返回结构并记录 artifact。

## 6. 是否类似 Docker

注册和打包方式可以借鉴 Docker/OCI，但不应退化为“一个巨大镜像包一切”。推荐分层为：

| 层 | 类似 OCI 的部分 | UEnv 需要补充的部分 |
|----|----------------|--------------------|
| EnvPackage | 内容寻址、manifest、镜像或制品引用 | env_type、OpenEnv contract、reward/trajectory/tool schema |
| ToolPackage | 可复用镜像/二进制/sidecar | tool capability、session API、artifact schema |
| RuntimeProfile | 多包组合解析 | Server 调度、Worker prepare、租约、实例池 |
| ArtifactBundle | blob + digest | trajectory index、reward 输入、UI 可浏览 |

这样多个 env instance 可以复用同一浏览器工具服务、artifact uploader、reward runner，而不必每个实例重复打包浏览器和所有依赖。

## 7. 需要补齐的代码能力

| 模块 | 需要新增或规范化 |
|------|------------------|
| Hub | ToolPackage manifest、版本解析、依赖图、兼容性校验 |
| Proto | `ToolPackageState`、`RuntimeProfile`、`PrepareToolPackage`、工具 session 和 artifact bundle schema |
| Server | Episode 到 RuntimeProfile resolve；按 env + tool + agent capability 调度；prepare 状态机 |
| Worker | ToolPackage installer；sidecar lifecycle；tool session manager；artifact bundler；reward adapter runner |
| Trajectory | 标准 `artifact_uri[]`、bundle manifest、checksum、大小限制、保留策略 |
| Frontend | Worker tool states、tool sessions、artifact bundle 和实例池关联展示 |

## 8. 边界结论

当前框架已经能把 Hub 注册的 `process` / `openenv_http` 未知环境动态拉到 Worker 并执行。要完整支持“环境依赖和浏览器等工具解耦复用”，还需要引入 ToolPackage / RuntimeProfile 作为一等协议对象。

不建议要求用户修改框架代码来接入自定义环境。用户应提供按规范构建的 EnvPackage，并可选提供 ToolPackage 或引用平台内置 ToolPackage。Worker 负责按声明准备、启动、连接和回收这些能力。

## 9. 分布式按需工具调用的目标架构

2026-08-08 进一步讨论的目标是：在某类环境上进行并行测试时，不再为了并行度复制多个完整大环境实例，而是把重型依赖拆成分布式 ToolPackage 服务，由多个 thin env session 按需租用。

推荐把运行时拆成三层：

| 层 | 职责 | 示例 |
|----|------|------|
| Thin Env Session | 只保留任务状态、reset/step/score 契约、workspace/session 隔离 | OpenEnv HTTP env、process plugin、轻量 sandbox |
| Distributed Tool Runtime | 承载可复用重型能力，支持本地 sidecar、远端 service、K8s Deployment/Job 或容器池 | Playwright、grader、artifact uploader、trace recorder、DB/proxy |
| Resource Broker / Tool Scheduler | 为 episode 同时申请 env、tool、agent、artifact 等多类资源租约 | `WorkerSlotLease`、`ToolLease`、`ToolSessionLease`、`AgentLease` |

并行 episode 的调度不应只检查 Worker 是否有空闲 env slot，而应对 RuntimeProfile 做多资源准入：

```text
EpisodeRequest
  -> resolve RuntimeProfile
  -> acquire scheduling group permit
  -> acquire WorkerSlotLease
  -> acquire required ToolLease / ToolSessionLease
  -> acquire AgentLease
  -> dispatch episode
  -> collect trajectory/artifacts
  -> release all leases
```

工具调用路径可以先从 Worker 本地代理开始，后续再演进为远端或 K8s 服务：

```text
Env / Agent
  -> Worker ToolProxy / Gateway
  -> Tool Router
  -> local sidecar 或 remote ToolService
```

这样 EnvPackage 不需要知道工具运行在哪台机器，也不需要把浏览器、reward runner、artifact store client 等全部打进同一个环境镜像。

## 10. ToolPackage 资源模型

ToolPackage manifest 需要从“声明依赖”升级为“可调度资源声明”。建议补充以下字段：

```json
{
  "tool_id": "browser.playwright",
  "version": "1.0.0",
  "runtime": "k8s_service | container_pool | worker_sidecar | process",
  "api_schema": "uenv.tool.browser.v1",
  "session_model": "stateless | shared_readonly | leased | exclusive | pooled",
  "isolation": {
    "profile": "per_session",
    "filesystem": "ephemeral",
    "network": "policy_per_session"
  },
  "resource_spec": {
    "cpu": 2,
    "memory_mb": 4096,
    "gpu": 0,
    "max_sessions_per_replica": 4
  },
  "pool": {
    "min_ready": 2,
    "max_replicas": 16,
    "queue_timeout_ms": 30000,
    "scale_metric": "pending_leases"
  },
  "healthcheck": {
    "path": "/health",
    "interval_ms": 5000
  },
  "artifacts": {
    "schema": "browser_trace_bundle_v1"
  }
}
```

不同工具的并行语义不同，不能统一当成无状态 HTTP 调用：

| 类型 | 示例 | 并行策略 |
|------|------|----------|
| `stateless` | schema validator、轻量 parser、format checker | 直接并发调用，按 QPS / CPU 限流 |
| `shared_readonly` | 只读数据集、镜像缓存、静态 fixture | 多 episode 共享，无需 session |
| `leased` | browser context、DB schema、workspace mount | 每个 episode 租一个 session，结束回收 |
| `exclusive` | GPU grader、真实设备、单 license 软件 | 独占 lease，其他 episode 排队等待 |
| `pooled` | Playwright browser、test runner、sandbox pod | 预热池 + 按 pending leases 扩缩容 |

因此“工具解耦”不等于简单远程调用。核心是让工具成为可声明、可租约、可池化、可观测、可回收的资源。

## 11. 资源排队、池化与故障处理

该设计会引入资源等待和多资源调度问题，尤其是浏览器、GPU grader、数据库、真实设备、代理出口、商业 license 工具等。处理原则如下：

1. 对资源重的工具显式建池，而不是让 episode 直接抢进程或端口。
2. `min_ready` 控制常驻预热容量，`max_replicas` 控制峰值成本。
3. Server / Broker 在 episode 开始前拿齐 env、tool、agent leases，避免运行中途才发现关键资源不可用。
4. pending lease 队列需要 timeout、priority、fairness 和 backpressure。
5. 工具池按 `pending_leases`、queue wait p95、busy ratio 等指标扩容，按 idle TTL 缩容。
6. 任何 dispatch、tool session、agent job、reward、artifact 上传失败，都必须补偿释放已持有的租约。
7. 租约状态应持久化到 Server DB 或 Redis 之类共享状态中，支持 Worker / ToolService 崩溃后的 orphan lease 回收。
8. artifact、trajectory、reward 输入必须携带 `episode_id`、`tool_session_id`、tool package digest 和 checksum，避免并发串线。

推荐把当前 Worker WarmupPool 从“完整环境实例池”逐步演进为：

```text
thin env pool
  + tool pool(browser / grader / artifact / proxy / db)
  + agent job pool
  + global resource broker
```

这样可以同时保留预热降低冷启动的优势，又避免为每个并行 episode 复制完整依赖栈。

## 12. 微软 Orchard 对照

微软开源的 Orchard 与本方案的方向有明显对齐点：它把环境管理做成 thin、Kubernetes-native、harness-agnostic 的服务层，支持大规模隔离 sandbox，目标是让训练、评测和不同 agent harness 复用同一套环境基础设施。

可以借鉴的点：

| Orchard 思路 | 对 UEnv 的启发 |
|--------------|----------------|
| K8s-native sandbox lifecycle | ToolService / thin env 可用 K8s Deployment、Job、Pod pool 承载 |
| orchestrator 与 sandbox 分离 | UEnv Server / Worker / ToolService 也应保持控制面和执行面分离 |
| 多 orchestrator 共享状态和分布式锁 | Resource Broker 可用 DB / Redis 保存租约、队列和 orphan 回收状态 |
| harness-agnostic environment service | UEnv RuntimeProfile 应让 OpenHands、自研 agent、VeRL adapter 共用环境与工具 |
| 热路径绕开重控制面 | 高频 tool call 不应每步都经 Server，Server 负责 lease 和审计，Worker/ToolProxy 负责调用热路径 |

但 Orchard 不能直接覆盖 UEnv 的全部需求。Orchard 更像通用 sandbox / environment service；UEnv 还需要处理 EnvPackage、ToolPackage、RuntimeProfile、reward、trajectory、artifact、训练 run scheduling policy、Worker pool telemetry、Hub package resolution 等更贴近 RL/benchmark 平台的协议对象。

结论：Orchard 证明了“thin environment service + K8s 承载 + 分布式状态”的路线可行。UEnv 应借鉴其底层环境服务化方式，但继续保留并强化面向训练和评测的 ToolPackage / RuntimeProfile / 多资源调度层。
