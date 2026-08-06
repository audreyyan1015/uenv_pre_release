# Hub 注册未知环境的 Worker 动态接入补齐方案

> 日期：2026-08-06  
> 状态：设计备忘录 / 待实现  
> 背景：当前 Hub 已能注册任意 `env_type` 和分发 EnvPackage，但 Worker 执行能力仍主要来自本地已部署的插件或专用后端。本文记录要实现“Server 收到 Hub 已注册但 Worker 未支持的环境时，可自动将受调度 Worker 调整为可用状态”的缺口与实施方案。

---

## 1. 当前结论

Hub 层已经具备：

- 任意 `env_type` 的 registry manifest 发布；
- `InterfaceSchema`（Action / Observation / State）与 `config_schema` 登记；
- EnvPackage 的内容寻址分发、`image_tar` 托管、`uenv env sync`、`docker load`；
- AgentBridge / Episode Stack / rubric 等组合建模。

Worker 层当前不具备完整的“任意未知环境动态执行”能力：

- Worker 可以上报 `supported_env_types` 与 `synced_env_packages`；
- Worker 可同步 SWE EnvPackage 并消费本地目录；
- 普通 L2 插件路径仍依赖本地 `plugins/{env_type}/manifest.yaml` 和本地 plugin binary；
- Server 调度逻辑主要选择已经具备能力的 Worker，而不是先把 Worker reconcile 成具备能力再 dispatch；
- 没有通用 OpenEnv/Container backend 能直接根据 Hub manifest 启动任意标准环境容器。

因此，仅把 `browser-task`、`robot-sim` 等新环境注册到 Hub，当前不能保证 Worker 自动变成支持该 `env_type`。

---

## 2. 目标行为

目标是把 Episode 调度前置为“解析 + 准备 + 调度”三段：

```text
EpisodeRequest(env_type, env_package_id/version, stack_id?)
  -> Server 查询 Hub resolve
  -> 得到 Task Env / EnvPackage / Agent Scaffold / Runtime Gateway / Trajectory 要求
  -> Server 检查 Worker 与 Agent capabilities
  -> 若无 ready Worker，但有可准备 Worker：
       下发 PrepareEnvironment
       Worker sync EnvPackage + load image + 启动 backend/plugin + 更新 capability
       Agent sync scaffold + 更新 agent capability
  -> Worker / Agent 均 ready
  -> DispatchEpisode 或 AgentJob
```

目标不是让旧 Worker 自动理解任意新平台协议。EnvPackage 可以声明需要 `browser_trajectory_v1`、`runtime_gateway`、`openenv_http_v1` 等能力，但旧二进制不具备时只能拒绝或等待升级。

---

## 3. 需要补齐的能力

### 3.1 Episode Stack resolve 成为调度前置步骤

Server 不应只根据 `env_type` 做本地判断。对于带 `stack_id` 或 `env_package_id` 的请求，应先从 Hub 获取解析结果：

- task env manifest；
- EnvPackage sync plan；
- required worker features；
- required agent scaffold；
- runtime gateway requirement；
- trajectory schema requirement；
- stack digest / package bundle digest。

调度使用 resolved 坐标，不使用未解析的 `latest`。

### 3.2 Worker capability 模型补全

Worker 心跳需要稳定表达：

| 字段 | 含义 |
|------|------|
| `supported_env_types` | 当前可执行的环境类型 |
| `synced_env_packages` | 已同步且校验过的 EnvPackage |
| `platform_features` | Worker 二进制/运行时具备的平台能力，如 `runtime_gateway`、`trajectory_v2_2`、`generic_container_backend` |
| `trajectory_schemas` | 可 seal 的轨迹 schema，如 `v2.2`、未来 `browser_trajectory_v1` |
| `tool_schemas` | 可路由的工具协议，如 `runtime/v1`、`browser-tools/v1` |
| `backend_kinds` | 可执行后端，如 `plugin_uds`、`swe_gateway`、`openenv_http_container` |
| `package_states` | 每个包的 `missing/syncing/ready/failed` 状态和 digest |

现有 `supported_env_types` / `synced_env_packages` 是基础，但不足以表达“可通过 prepare 变成 ready”。

### 3.3 Server Prepare/Reconcile 状态机

Server 需要在 dispatch 前增加准备态：

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

失败需要区分：

| 错误 | 含义 |
|------|------|
| `NO_REGISTERED_ENV` | Hub 不存在该 env 或 stack |
| `NO_CAPABLE_PLATFORM` | 没有 Worker 具备所需平台 feature，准备也无意义 |
| `PREPARE_FAILED` | Worker 同步包、load 镜像或启动 backend 失败 |
| `NO_AGENT_SCAFFOLD` | 要求 agent，但 AgentBridge 未发布或 Agent host 未 sync |
| `NO_READY_WORKER` | 有能力但容量不足 |

### 3.4 Worker EnvPackage installer

Worker 需要一个可由 Server 调用的安装器：

```text
PrepareEnvPackage(package_id, version, consumer=worker, expected_bundle_digest)
```

执行步骤：

1. 拉 Hub package manifest / sync plan；
2. 下载 inline artifacts；
3. 校验每个 sha256 与 bundle digest；
4. 对 `image_tar` 执行 docker/podman load；
5. 读取 `manifest.json`、`worker_overlay`、`contracts`、`interface`；
6. 如果 backend 已支持，注册 `env_type -> backend`；
7. 更新本地 package state；
8. 下一次 heartbeat 上报为 ready。

安装器必须幂等。相同 `package_id@version@digest` 已 ready 时直接返回 ready。

### 3.5 通用 OpenEnv/Container backend

这是打破 `qa/code/swe` 限制的核心。Worker 需要一个 generic backend，能根据 Hub manifest/EnvPackage 启动符合标准 HTTP 契约的容器：

```text
image / image_tar
entrypoint
health_check_path
interface.action / observation / state
runtime_api = openenv_http_v1 或 uenv_env_http_v1
```

推荐运行协议：

| 方法 | 路径 | 含义 |
|------|------|------|
| GET | `/health` | 健康检查 |
| POST | `/reset` | 创建/重置 episode |
| POST | `/step` | 执行动作，返回 observation/reward/done/info |
| GET | `/state` | 读取状态 |
| POST | `/close` | 释放 episode |

Worker generic backend 负责：

- 启动容器；
- 等待 health；
- 将 EpisodeRequest payload 映射成 reset config；
- 将 Agent/LLM action 映射到 `/step`；
- 汇总 reward / terminated / artifact；
- seal trajectory；
- 回收容器。

没有这个 backend，每个新 `env_type` 仍需要写本地插件或专用 Rust 路径。

### 3.6 Agent scaffold reconcile

Agent 机也需要被纳入准备闭环：

- Server 从 Episode Stack resolve 出 `agent_scaffold`；
- Agent host 同步 `uenv-agent-xxx@version`；
- Agent 注册/心跳上报 `synced_agent_bridges`、`agent_kind`、labels、容量；
- Server 只在匹配 scaffold 且未满载的 Agent 上投递 AgentJob。

现有 Agent 注册和 `synced_agent_bridges` 已具备基础，需要补齐自动 sync/reconcile 与 stack resolve 的硬校验。

### 3.7 平台能力与环境制品分层

EnvPackage 可以携带：

- 镜像 / image tar；
- catalog / dataset；
- eval spec；
- worker overlay；
- Agent scaffold 引用或脚本制品；
- rubric/scorer；
- tool schema；
- interface / config schema。

EnvPackage 不应携带并替换：

- `uenv-server` 调度核心；
- `uenv-worker` generic backend 实现；
- Server trajectory indexer；
- runtime gateway 协议实现；
- 安全沙箱策略核心。

如果包声明了旧 Worker 不具备的平台能力，Server/Worker 应拒绝准备并给出可诊断错误，而不是半启动。

---

## 4. 浏览器类环境的轨迹接入建议

浏览器专有数据（Playwright trace、截图、HAR、DOM dump）第一版建议作为 artifact 保存，不直接扩展主索引：

```text
Agent / Env container 生成:
  trace.zip
  network.har
  screenshots/*.png
  dom/final.html

Worker seal:
  TrajectoryBundle.artifact.artifact_uri -> artifact 目录或对象存储 URI
  test_results.raw_output / stdout_log -> 摘要
```

如果要在标准轨迹中原生表达：

- `browser.navigate`
- `browser.click`
- `browser.type`
- `browser.screenshot`
- `browser.dom_snapshot`

则需要新增轨迹 schema，修改 Worker `StepAction` / `StepObservation` / `TrajectoryBundle`，并升级 Server 索引或前端读取逻辑。

---

## 5. 分阶段落地

### 阶段 A：只做 resolve + 静态能力校验

目标：

- Server 调度前调用 Hub Episode Stack resolve；
- 对 Worker heartbeat capability 做硬校验；
- 不支持时报明确错误，不再进入运行期失败。

产出：

- `NO_CAPABLE_WORKER` / `NO_CAPABLE_PLATFORM` 错误；
- resolved stack digest 进入 episode metadata；
- 文档化 Worker 必须上报的 capability。

### 阶段 B：Worker 自动同步 EnvPackage

目标：

- Server 可下发 prepare；
- Worker 自动 `sync + docker load + overlay apply`；
- 同步完成后 heartbeat 更新 `synced_env_packages`。

仍不解决：

- Worker 不懂该 env_type 如何执行的问题。

### 阶段 C：Generic OpenEnv/Container backend

目标：

- 新环境只要符合标准 HTTP 契约，即可由 Worker 启动；
- `qa/code/swe` 不再是唯一可执行集合；
- Hub registry + EnvPackage 成为真正的动态环境来源。

新增：

- `openenv_http_container` backend；
- container lifecycle manager；
- reset/step/state/close client；
- generic trajectory bridge；
- generic reward/artifact mapping。

### 阶段 D：Agent scaffold 自动准备

目标：

- Server 根据 Episode Stack 自动要求 Agent host sync scaffold；
- Agent 注册能力与 stack 引用强校验；
- Worker 与 Agent 均 ready 后再 dispatch。

### 阶段 E：浏览器轨迹原生化（可选）

目标：

- 新增 `browser_trajectory_v1`；
- 支持浏览器 step 类型和可视化；
- Server 可按浏览器字段索引或前端按 body 渲染。

---

## 6. 最小可行实施建议

近期建议先做 A+B+C 的最小闭环：

1. Server 以 Episode Stack resolve 为调度入口；
2. Worker 支持 `PrepareEnvPackage` 并能把 package 变为 ready；
3. 新增 generic OpenEnv HTTP container backend；
4. 浏览器 trace 先作为 artifact URI 保存；
5. Agent scaffold 自动同步放到 D 阶段。

这样可以先实现“Hub 注册的新容器化环境可被 Worker 执行”，再逐步把 Agent 与浏览器轨迹做成一等公民。

---

## 7. 与当前代码的关系

当前已有基础：

- proto 中 Worker register/heartbeat 已有 `supported_env_types`、`synced_env_packages`；
- Server 有 worker registry、AgentRegistry、AgentJobQueue；
- Hub 有 EnvPackage sync plan、AgentBridge、Episode Stack 设计；
- Worker 有 SWE EnvPackage 读取、image tar load、runtime gateway、trajectory seal/upload。

当前主要缺口：

- 缺 Server 调度前的 Hub resolve/reconcile 状态机；
- 缺 Worker 侧可远程触发的 EnvPackage installer；
- 缺任意标准容器环境的 generic backend；
- 缺 Agent host 的自动 scaffold reconcile；
- 缺浏览器原生轨迹 schema。

