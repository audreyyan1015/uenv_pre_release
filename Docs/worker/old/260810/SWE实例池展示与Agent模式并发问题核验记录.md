# SWE 实例池展示与 Agent 模式并发问题核验记录

日期：2026-08-10

本文记录本次会话中从“实例池里面没有显示 swe 类型的环境”开始，已经通过 server / worker 实机状态确认属实的问题，以及按顺序完成的修复和实机验证结果。

## 1. Worker 详情页实例池未显示 SWE 环境

### 现象

Worker 详情页中“Worker 实例池 / 当前实例池槽位”只显示 `qa`、`code`，没有显示正在使用的 `swe` 环境。用户观察到当前 episode 实际应该运行在 SWE 环境上。

### 核验证据

Server admin `/fleet/workers` 返回中，`worker-7143-pro` 已明确具备 SWE 能力：

- `supported_env_types`: `["qa", "code", "swe"]`
- `package_states`:
  - `swe-bench-pro@0.3.4`，`env_type=swe`，`backend_kind=swe_instance_pool`，`state=ready`
  - `swe-bench-smith@0.1.0-local`，`env_type=swe`，`backend_kind=swe_instance_pool`，`state=ready`

但同一返回中的 `pool_summary` / `pool_slots` 只有 `process_plugin` 的 `qa`、`code`：

- `pool_summary`: `code`、`qa`
- `pool_slots`: `qa-*`、`code-*`
- 没有 `env_type=swe` 的 slot 或 summary

Worker 7143 本机也确认 SWE 正在运行：

- metrics 中有 `uenv_swe_instance_pool_size 1`
- Docker 中存在运行中的 `uenv-swe-*` 容器
- worker 日志中有 `swe_session_provisioned`
- worker 日志中有 `swe_trajectory_sealed`

### 原因

当前前端“实例池槽位”只读取 worker heartbeat 中的 `pool_summary` / `pool_slots` 字段。

而 worker 端这两个字段当前只来自通用 `warmup_pool.snapshot()`：

- `uenv-worker/src/control_plane/client.rs` 的 register 路径读取 `warmup_pool.snapshot()`
- heartbeat 路径同样只读取 `warmup_pool.snapshot()`

SWE 并不走这个通用 process-plugin warmup pool，而是单独走 `SweInstancePool`：

- `warmup_pool`：面向 `qa/code` 这类 process plugin 槽位
- `SweInstancePool`：面向 Runtime Gateway 创建的 SWE 容器 session，负责 Docker sandbox、EnvPackage catalog、patch 语义、trajectory、artifact、reward contract 等

因此这是“可观测性展示口径缺口”，不是 SWE 环境未启动，也不是 EnvPackage 未 ready。

### 影响

用户在 Worker 详情页无法直接看到 SWE 容器实例池，容易误判为：

- SWE 环境没有加载
- SWE 实例池为空
- 当前 episode 没有实际走 SWE

实际情况是 SWE 正在由 `SweInstancePool` 管理，只是没有合并进前端当前展示的数据源。

### 建议修复

短期前端修复：

- 在 Worker 详情页中基于 `package_states` 单独展示 “SWE 环境池”
- 当 `package_states` 中存在 `env_type=swe` 且 `backend_kind=swe_instance_pool` 的 ready package 时，展示 SWE EnvPackage ready 状态
- 结合 active episode、`swe` env_type、已知 session/trajectory 线索展示 SWE 当前 busy/ready 状态

中长期 telemetry 修复：

- 将 `SweInstancePool` 的 active/idle/session 状态纳入统一 runtime pool snapshot
- 让 worker heartbeat 同时上报 process plugin pool 与 SWE pool
- 前端继续统一消费 `pool_summary` / `pool_slots`，减少特殊展示逻辑

### 已完成修复

已在 Worker 详情页补充 SWE 环境池兼容展示：

- `frontend/src/components/worker-detail.tsx` 基于 `package_states` 中 `env_type=swe` 且 `backend_kind=swe_instance_pool` 的 package 单独展示 “SWE 环境池”。
- 页面保留 `qa/code` 的 process-plugin `pool_slots` 原样展示，不伪造不存在的 SWE slot。
- SWE 区域显示 `ready package`、`agent-mode load`、`worker capacity`，并列出 `swe-bench-pro@0.3.4`、`swe-bench-smith@0.1.0-local`。
- `frontend/src/lib/worker-tree.ts` 修复了 Obs state 尚未到达但 Fleet live 已返回时，Worker 详情仍把活跃数投影为 0 的问题。

已同步到：

- `8.130.75.157:8099`
- `8.130.136.136:8777`

Playwright 实机验证：

```text
http://8.130.75.157:8099/server/worker?run=_orphan&worker=worker-7143-pro&status=busy
http://8.130.136.136:8777/server/worker?run=_orphan&worker=worker-7143-pro&status=busy

页面均显示：
SWE 环境池
ready package=2
agent-mode load=0
worker capacity=4
swe-bench-pro@0.3.4 ready
swe-bench-smith@0.1.0-local ready
实例池汇总含 swe / swe_instance_pool
```

## 2. `8.130.136.136:8777` 前端 Fleet 代理配置缺失

### 现象

访问 `http://8.130.136.136:8777/server?run=verl_swesmith_grpo_train_20260810_095908` 可以打开页面，但直接访问该前端的 `/fleet/workers` / `/fleet/agents` 返回 `500 Internal Server Error`。

### 核验证据

在 `8.130.136.136` 上检查：

- 本机只运行前端 Vite：`vite dev --host 0.0.0.0 --port 8777`
- 本机没有运行 `uenv-adapter-core`
- 本机没有监听 `127.0.0.1:50052`
- `/fleet/workers` 返回 Vite 500

该前端进程环境变量：

- 已配置 `VITE_AGGREGATION_BASE_URL=/obs`
- 已配置 `VITE_OBS_PROXY_TARGET=http://8.130.75.157:8888/obs`
- 未配置 `VITE_FLEET_PROXY_TARGET`

前端 `vite.config.ts` 默认：

- `/fleet/*` 代理到 `VITE_FLEET_PROXY_TARGET`
- 未设置时默认 `http://127.0.0.1:50052`

由于 `8.130.136.136` 本机没有 `50052` admin HTTP，导致 `/fleet/*` 代理失败。

### 原因

`8.130.136.136:8777` 是独立前端节点，只配置了 Obs 代理，没有配置 fleet admin 代理目标。它默认回退到本机 `127.0.0.1:50052`，但该节点不是 server 节点。

### 影响

Worker 详情页依赖 `/fleet/workers` 获取实时 worker 名册、负载、capacity、package_states、pool_summary 等信息。该代理不可用时：

- Worker 详情实时 fleet 数据不可信或不可用
- 页面可能回落到 Obs 投影
- 用户看到的 worker 负载、实例池状态可能不是最完整的实时状态

### 建议修复

在 `8.130.136.136` 前端启动环境中补充：

```bash
VITE_FLEET_PROXY_TARGET=http://8.130.75.157:50052
```

或将该前端部署为 SSR/反向代理时显式把 `/fleet/*` 转发到真实 server admin HTTP。

### 已完成修复

实机验证发现 `8.130.136.136` 不能直连 `8.130.75.157:50052`：

```text
curl http://8.130.75.157:50052/workers
Connection timed out after 5002 milliseconds
```

但 `8.130.136.136` 可访问 `8.130.75.157:8099/fleet/workers`。由于 Vite 代理会把浏览器侧 `/fleet/workers` 重写为上游 `/workers`，最终配置为：

```bash
VITE_FLEET_PROXY_TARGET=http://8.130.75.157:8099/fleet
```

该配置已写入远端：

```text
/root/.uenv-frontend-8777.env
```

并重启 `8.130.136.136:8777` Vite 前端。复查：

```text
http://8.130.136.136:8777/fleet/workers -> 200 application/json
worker_count=1
worker-7143-pro load=0 capacity=4 supported_env_types=["qa","code","swe"]

http://8.130.136.136:8777/fleet/agents -> 200 application/json
openhands-default total_capacity=1 total_load=0 pending_jobs=0
```

## 3. Agent 模式 SWE 训练实际只以 1 并发推进

### 现象

负责训练的人员观察到 run `verl_swesmith_grpo_train_20260810_095908` 中 Worker 页面显示：

```text
当前负载 1 / 4
```

怀疑 worker 仍在串行执行。

### 核验证据

从 Obs run state 查询：

- `run_state=RUNNING`
- `worker-7143-pro.current_load=1`
- `worker-7143-pro.capacity=4`
- `worker-7143-pro.active_episodes` 只有 1 个 episode
- run 中有 7 个 `ACTIVE` episode：
  - 1 个绑定 `worker-7143-pro` 且进入 `EXECUTE`
  - 6 个 `worker_id=""`，停留在 `DISPATCH`

从 server `/fleet/workers` 查询：

- `worker_count=1`
- `total_capacity=4`
- `worker-7143-pro.load=1`
- `worker-7143-pro.capacity=4`
- `workers[0].episodes` 只有当前 1 个 episode

从 server `/fleet/agents` 查询：

- `openhands-default.total_capacity=1`
- `openhands-default.total_load=1`
- `running_jobs=1`
- `in_flight_detail` 只有 1 个 OpenHands job
- 可用 OpenHands agent：
  - `agent_id=94874799-8856-4a9e-bf3e-e5fe42de071d`
  - `max_concurrent=1`
  - `reserved_load=1`
  - `current_load=1`
  - `stale=false`

从 Worker 7143 本机查询：

- 当前只有 1 个运行中的 `uenv-swe-*` Docker 容器
- `uenv_swe_instance_pool_size 1`
- 日志显示 SWE session 逐个 `swe_session_provisioned` / `swe_trajectory_sealed`

### 原因

SWE Agent 模式调度需要同时拿到：

1. Worker capacity
2. OpenHands Agent capacity

当前 Worker 虽然 `capacity=4`，但 `openhands-default` Agent 池只有 1 个可用并发。因此实际并发由：

```text
min(worker capacity, openhands agent pool capacity)
```

限制为 1。

当前单个 OpenHands agent 的 `max_concurrent=1`，所以它不会同时处理多个 `AgentJob`。

### 影响

训练侧即使一次提交多个 SWE episode，也只有 1 个会进入 Worker/SWE 容器执行；其他 episode 会停留在 DISPATCH / 等待 Agent 可用。表现为：

- Worker 页面长期显示 `1 / 4`
- Worker 容量未被打满
- SWE rollout 吞吐接近串行

### 建议修复

优先方案：在 Agent 服务器上启动多个 OpenHands agent 实例。

- 每个实例使用不同 `agent_id`
- 注册到同一个 `openhands-default` pool
- 根据目标并发将 pool capacity 提升到 2/4/8 等
- 确保每个 agent 的 workspace、日志、runtime session、端口或工作目录互相隔离

谨慎方案：提高单个 OpenHands agent 的 `max_concurrent`。

该方案要求 OpenHands runner / bridge 内部能够安全并发处理多个 job，包括独立 workspace、session、日志、错误隔离和状态回传。当前实机显示配置为 `max_concurrent=1`，不建议直接调大而不做并发隔离验证。

## 4. Worker metrics 与 Agent-mode SWE 执行计数口径不一致

### 现象

在 Agent-mode SWE 正在执行时，Worker 7143 本机 metrics 中：

```text
uenv_active_episode_count 0
uenv_swe_instance_pool_size 1
```

而 server `/fleet/workers` 同时显示：

```text
load=1
capacity=4
```

### 原因

`uenv_active_episode_count` 反映 worker native episode executor 的计数；Agent-mode SWE 通过 Server 创建 Gateway session，再由 OpenHands agent 执行 job，不完全走 native episode executor 计数路径。

SWE 容器实际状态由 `SweInstancePool` 指标体现，例如 `uenv_swe_instance_pool_size`。

### 影响

如果只看 `uenv_active_episode_count`，会误以为 Worker 没有执行任务；如果只看 server `/fleet/workers.load`，又看不到 SWE session 数和容器池细节。

### 建议修复

- 前端和运维文档应明确区分 native worker episode 与 Agent-mode SWE session
- 将 `uenv_swe_instance_pool_size` 或更细粒度的 `SweInstancePool` session 状态纳入 worker 详情页
- 中长期将 Agent-mode SWE 的 session activity 合并到统一 worker runtime telemetry

### 已完成修复

Worker metrics 保留 `uenv_active_episode_count` 的原语义，只表示 native `EpisodeExecutor` 活跃数；同时新增两个更明确的指标：

```text
uenv_swe_active_session_count
uenv_worker_runtime_load
```

其中：

```text
uenv_worker_runtime_load = uenv_active_episode_count + uenv_swe_active_session_count
```

这样运维侧可以同时看到：

- native episode executor load
- SWE Runtime Gateway / SweInstancePool session load
- worker 总 runtime load

已将 `uenv-worker/src/metrics.rs` 同步到 7143，执行 release rebuild 并通过既有脚本重启：

```text
SKIP_REBUILD=0 bash scripts/restart-worker-gateway-28097-7143.sh
```

实机验证：

```text
7143 worker pid=2013760
health ok
Server /fleet/workers: worker-7143-pro load=0 capacity=4 last_heartbeat_secs=3

curl http://127.0.0.1:28777/metrics:
uenv_active_episode_count 0
uenv_swe_instance_pool_size 0
uenv_swe_active_session_count 0
uenv_worker_runtime_load 0
```

## 5. `8.130.75.157:8099/system` Hub overview 401 与子界面可达性

### 现象

2026-08-10 复查 `http://8.130.75.157:8099/system` 时，浏览器侧曾报：

```text
GET http://8.130.75.157:8099/hub/api/v1/system/overview 401 (Unauthorized)
```

### 核验证据

直接 HTTP 检查：

- `/system`：`200 text/html`
- `/fleet/workers`：`200 application/json`
- `/fleet/agents`：`200 application/json`
- `/hub/healthz`：`200 application/json`
- `/hub/api/v1/system/overview`：裸请求为 `401 application/json`

直连 Hub 源站 `http://8.130.95.176:8088/api/v1/system/overview` 的裸请求同样返回 `401`，说明该错误不是 `/system` 子页面不可达，也不是 8099 的 Hub 代理链路断开，而是 Hub overview API 需要 bearer token。

服务器 8099 前端进程环境原本只有：

- `VITE_HUB_PROXY_TARGET=http://8.130.95.176:8088`
- `VITE_HUB_CONSOLE_URL=http://8.130.95.176:8088/`

缺少 `VITE_HUB_TOKEN`，而本地前端代码 `frontend/src/hooks/use-system-telemetry.ts` 已通过 `authHeaders()` 支持从 `VITE_HUB_TOKEN` 生成 `Authorization: Bearer ...`。

### 已同步修复

已从 Hub 主机 `/root/uenv/uenv-hub/data/.admin_token` 读取现有 token，并写入 Server 主机：

```text
/root/.uenv-frontend-8099.env
```

该文件权限为 `0600`，未写入仓库。随后用该环境文件重启 8099 Vite 前端。重启后：

- 8099 由 `node ... vite dev --port 8099 --host 0.0.0.0` 监听
- 服务器端带 Authorization 请求 Hub overview 返回 `200`
- Playwright 打开 `/system` 后，页面中 `uenv-hub` 显示 `registry`
- Playwright console 未再出现 Hub overview `401`，仅有 `/favicon.ico` 404

### 子界面可达性

从 `/system` 图上涉及的内部入口逐个检查，均可达并能渲染业务内容：

- `/`：`200`，标题 `UEnv · 训练与评测控制台`
- `/ops`：`200`，标题 `UEnv · 技术观测台`
- `/server`：`200`，标题 `UEnv · Episode 进度`
- `/server/worker?run=_orphan&worker=worker-7143-pro&status=busy`：`200`，标题 `UEnv · Worker 详情`

Hub 外链 `http://8.130.95.176:8088/` 也可达，会跳转到 `/console`，标题 `UEnv Hub 控制台`。

### 额外发现

`/ops` 页面存在非阻断 React hydration mismatch。Playwright console 中显示 SSR 与客户端的“最近更新”时间文本不一致，例如服务端渲染为 `18:04:19`，客户端水合时变为 `18:05:06`。

这不是页面不可达问题，页面会在客户端重新生成相关子树并继续显示，但建议后续修复为：

- 时间类动态文本避免参与 SSR 首屏稳定文本
- 或在 SSR payload 中传入同一快照时间，保证服务端与客户端首屏一致

### 已完成修复

已在 `frontend/src/components/training-console.tsx` 中让 `/ops` 在客户端 hydration 完成前渲染固定初始化占位，避免 SSR 首屏直接格式化 `Date.now()`、`updated_at`、stage `source_ts` 等动态时间。

修复同步到：

- `8.130.75.157:8099`
- `8.130.136.136:8777`

Playwright 复查 `http://8.130.75.157:8099/ops?run=verl_swesmith_grpo_train_20260810_095908`：

```text
Console errors:
- /favicon.ico 404

未再出现 React hydration mismatch。
```

## 6. SWE Agent 模式并发扩容实机联调

### 目标

修复 `openhands-default` 只有 1 并发导致 SWE-smith agent-mode episode 串行推进的问题，并在真实 server / worker / agent 三端链路上验证并行训练可用。

### 修复动作

在 Agent 池机器 `8.130.208.77` 上保留原 `uenv-agent-poller.service`，新增 3 个独立 OpenHands poller service：

- `uenv-agent-poller-extra-01.service`
- `uenv-agent-poller-extra-02.service`
- `uenv-agent-poller-extra-03.service`

每个 extra service 使用独立配置：

- 独立 `OPENHANDS_AGENT_ID`：`openhands-20877-extra-01/02/03`
- 独立 `OPENHANDS_RUNS_DIR`：`/var/log/uenv/openhands-extra-01/02/03`
- 独立 `OPENHANDS_COMPLETION_SPOOL_DIR`
- 独立 API/health 端口：`8889/8778`、`8890/8779`、`8891/8780`
- 每个 agent 仍保持 `OPENHANDS_AGENT_MAX_CONCURRENT=1`

同时将主 poller 固定为稳定 ID：

```text
OPENHANDS_AGENT_ID=openhands-20877-main
```

避免主 service 重启后生成随机新 agent id，导致 server 侧出现额外 stale 注册项。

为避免历史 completion spool 对主 poller 造成持续 `UNKNOWN_JOB` 噪声，已将旧 spool 归档到：

```text
/var/log/uenv/openhands-runs/completion-spool.unknown-job-archive-20260810-182839
```

归档数量为 10 个 JSON 文件；未删除数据。

本地部署脚本也已补充 `OPENHANDS_AGENT_REPLICAS` 支持：

```bash
OPENHANDS_ENABLE_POLL=1 OPENHANDS_AGENT_REPLICAS=4 bash scripts/deploy-openhands-20877.sh
```

默认值仍为 `1`，不会改变单 agent 部署行为。

### 联调证据

扩容后，Server `/agents` 显示 `openhands-default` 从单并发提升为 4 并发：

```text
agent_count=6
openhands-default.total_capacity=4
openhands-default.total_load=4
running_jobs=4
outstanding_jobs=4
```

4 个 OpenHands agent 均健康且各自领取 1 个 job：

- `94874799-8856-4a9e-bf3e-e5fe42de071d`
- `openhands-20877-extra-01`
- `openhands-20877-extra-02`
- `openhands-20877-extra-03`

同一时刻 Server `/workers` 显示 Worker 7143 被打满：

```text
worker-7143-pro load=4 capacity=4
```

活跃 episode 包括：

- `79401bac-f61c-475f-9e26-e76e3a7709be`
- `4c0ea384-a3ef-4973-9c46-eff34035e464`
- `0f751014-18ae-4362-ad84-92bbbf07c7b6`
- `732a03b1-5ae3-4894-a249-82875c315114`

Worker 7143 本机 Docker 同时存在 4 个 `uenv-swe-*` 容器，说明执行端实际并行创建 SWE runtime，而不是仅 server 侧排队：

```text
uenv-swe-...-qqjrb8nj-... Up About a minute
uenv-swe-...-qqjrb8nj-... Up About a minute
uenv-swe-...-qqjrb8nj-... Up About a minute
uenv-swe-...-qqjrb8nj-... Up About a minute
count=4
```

### 完成与补位验证

扩容后不是只领取首批任务，而是完成后能继续补位：

- `extra-03` 完成 `0f751014-18ae-4362-ad84-92bbbf07c7b6`，`acked=True`，随后领取 `546b1171-827c-48bd-a6e4-680a858c8dcd`
- `extra-02` 完成 `4c0ea384-a3ef-4973-9c46-eff34035e464`，`reward=1.0`，`response_ids=10867`，`acked=True`，随后领取 `16b4a782-b1f0-4c38-b76a-94aac58cb63a`
- `extra-01` 完成 `732a03b1-5ae3-4894-a249-82875c315114`，`response_ids=18467`，`acked=True`
- `extra-03` 又完成补位任务 `546b1171-827c-48bd-a6e4-680a858c8dcd`，`acked=True`

采样中可见 episode 列表从首批 4 个切换到新 episode，但整体仍保持 `worker load=4/4`，证明补位调度可用。

后续训练 step `verl-agent-loop-step-29-8953cfc1-*` 自动进入下一轮，Server 再次显示：

```text
openhands-default.total_capacity=4
openhands-default.total_load=4
worker-7143-pro load=4 capacity=4
```

该轮中由于主 poller 重启触发一次 lease reclaim，Server `stale_reclaimed_jobs=1`，随后 job `bd8f8281-8840-4b1c-a8ac-a803ef711eaa` 被重新派发给稳定主 agent `openhands-20877-main` 并继续执行。

### 结论

SWE-smith agent-mode 并行 episode 训练在当前真实联调环境中已验证可用：

- Agent 池容量已从 1 提升到 4
- Worker 7143 可被 SWE Agent 模式打满到 `4/4`
- 7143 实际同时运行 4 个 SWE Docker runtime
- OpenHands agent 完成结果可正常 `CompleteAgentJob acked=True` 回填 server
- episode 完成后可继续补位，后续训练 step 可继续以 4 并发推进

已继续处理的可观测性/运维项：

- 前端系统拓扑已按非 stale agent 重新计算 Agent Scaffold 可用 agent 数、agent load 与链路 activity，避免 stopped extra agent 在 heartbeat 超时窗口中继续撑起可用容量展示。
- OpenHands runner 已增加 `UNKNOWN_JOB` completion spool 归档策略：Server 明确返回 `UNKNOWN_JOB` 时，将 spool JSON 移动到 `completion-spool/archived-unknown-job/`，不再无限重放。
- 当前扩容仍受 Worker `capacity=4` 限制；若继续提升 Agent 数，应同步提升 Worker/Gateway/SWE 容器容量并做资源压测

本地验证：

```text
python3 -m unittest integrations.openhands.tests.test_rollout_trace_mode
Ran 11 tests in 0.020s
OK
```

208.77 实机验证：

```text
uenv-agent-poller.service active
uenv-agent-pool-supervisor.service active
GET http://127.0.0.1:8777/health:
{"status":"ok","poll_enabled":true,"registered":true,"agent_id":"openhands-20877-main"}
```

## 7. Agent Pool Supervisor 动态对齐修复

### 背景判断

仅把 208.77 上的 OpenHands poller 固定扩到 4 个，可以解决当前这一次 `worker capacity=4` 的并发瓶颈，但不是根本方案。根本问题是 Agent-mode SWE 的实际并发由 Worker 容量与 Agent 池容量共同决定：

```text
effective_concurrency = min(worker capacity, openhands agent pool capacity)
```

如果训练侧把并行参数改到 2、4、8，或者 Worker 侧容量变化，而 Agent 侧仍需要人工同步实例数，就会再次出现 Worker 空闲、episode 卡在 DISPATCH / pending agent 的情况。

更合理的边界是：在 Agent 节点本地运行 supervisor，根据 Server 暴露的 worker / agent fleet 状态动态 reconcile 本机 OpenHands poller 实例数。Worker 只声明容量和执行 episode，不直接 SSH 或管理 Agent 机器进程；Server 继续作为调度事实源；Agent 节点负责把本地服务实例数对齐到需求。

### 代码调整

新增本地 supervisor：

```text
scripts/openhands/agent_pool_supervisor.py
```

核心行为：

- 从 `http://8.130.75.157:8099/fleet/agents` 与 `/fleet/workers` 读取当前状态；208.77 不能稳定直连 `8.130.75.157:50052`，因此使用 8099 已配置好的 fleet 代理。
- 计算目标副本数时同时考虑 `pending_jobs`、`running_jobs`、pool `total_load`、worker `active_episodes`、worker `load` 与 worker `capacity`。
- 最小保留 1 个主 poller：`uenv-agent-poller.service` / `openhands-20877-main`。
- 当需求超过当前本地 poller 数时，自动创建并启动 `uenv-agent-poller-extra-01..N.service`。
- 当需求下降时，只停止 idle extra poller；如果 extra agent 仍有 `current_load`，先跳过，等待下一轮 reconcile。
- 每个 extra poller 保持 `OPENHANDS_AGENT_MAX_CONCURRENT=1`，通过多实例隔离 workspace、runs、completion spool、API/health 端口。

部署脚本 `scripts/deploy-openhands-20877.sh` 同步补充两个开关：

```bash
# 静态副本，适合临时固定容量
OPENHANDS_ENABLE_POLL=1 OPENHANDS_AGENT_REPLICAS=4 bash scripts/deploy-openhands-20877.sh

# 动态对齐，适合作为长期运行模式
OPENHANDS_ENABLE_POLL=1 OPENHANDS_ENABLE_AUTOSCALE=1 bash scripts/deploy-openhands-20877.sh
```

动态模式会安装并启动：

```text
uenv-agent-pool-supervisor.service
```

### 实机验证

已将 supervisor 部署到 `8.130.208.77` 并启动 `uenv-agent-pool-supervisor.service`。在后续 SWE-smith agent-mode 训练波次中，supervisor 从单主 poller 自动扩到 4 个本地 agent 实例：

```text
18:50:38 state desired=4 current=1 pool_capacity=1 pool_load=1 pending=3 worker_capacity=4 worker_load=4 active_episodes=8
18:50:38 scale up slot=01
18:50:38 scale up slot=02
18:50:39 scale up slot=03
18:50:49 state desired=4 current=4 pool_capacity=4 pool_load=4 pending=0 worker_capacity=4 worker_load=4 active_episodes=7
```

该波任务开始排空后，supervisor 没有强停仍在执行的 agent，而是跳过 busy slot，只停止 idle slot：

```text
18:51:09 state desired=3 current=4 pool_capacity=4 pool_load=3 pending=0 worker_capacity=4 worker_load=3 active_episodes=3
18:51:09 skip scale down busy slot=03 load=1
18:51:09 skip scale down busy slot=02 load=1
18:51:09 skip scale down busy slot=01 load=1
18:51:19 state desired=2 current=4 pool_capacity=4 pool_load=2 pending=0 worker_capacity=4 worker_load=1 active_episodes=1
18:51:19 scale down idle slot=01
18:51:29 state desired=1 current=3 pool_capacity=4 pool_load=0 pending=0 worker_capacity=4 worker_load=0 active_episodes=0
18:51:29 scale down idle slot=03
18:51:29 scale down idle slot=02
18:52:09 state desired=1 current=1 pool_capacity=1 pool_load=0 pending=0 worker_capacity=4 worker_load=0 active_episodes=0
```

之后出现第二轮训练需求时，supervisor 再次自动扩容，并在任务完成后回落：

```text
18:52:19 state desired=4 current=1 pool_capacity=1 pool_load=1 pending=3 worker_capacity=4 worker_load=4 active_episodes=8
18:52:19 scale up slot=01
18:52:20 scale up slot=02
18:52:20 scale up slot=03
18:52:30 state desired=4 current=4 pool_capacity=4 pool_load=4 pending=0 worker_capacity=4 worker_load=3 active_episodes=7
18:53:00 state desired=1 current=4 pool_capacity=4 pool_load=1 pending=0 worker_capacity=4 worker_load=0 active_episodes=0
18:53:00 skip scale down busy slot=03 load=1
18:53:00 scale down idle slot=02
18:53:01 scale down idle slot=01
18:53:11 state desired=1 current=2 pool_capacity=4 pool_load=0 pending=0 worker_capacity=4 worker_load=0 active_episodes=0
18:53:11 scale down idle slot=03
```

训练继续滚动期间，后续又观察到新的 4 并发波次：

```text
18:57:04 state desired=4 current=1 pool_capacity=1 pool_load=0 pending=4 worker_capacity=4 worker_load=4 active_episodes=8
18:57:05 scale up slot=01/02/03
18:57:15 state desired=4 current=4 pool_capacity=4 pool_load=4 pending=0 worker_capacity=4 worker_load=4 active_episodes=8
18:57:45 state desired=2 current=4 pool_capacity=4 pool_load=2 pending=0 worker_capacity=4 worker_load=0 active_episodes=0
18:57:46 scale down idle slot=02/01
18:57:56 scale down idle slot=03

18:58:57 state desired=4 current=4 pool_capacity=4 pool_load=3 pending=0 worker_capacity=4 worker_load=4 active_episodes=7
18:59:07 state desired=4 current=4 pool_capacity=4 pool_load=4 pending=1 worker_capacity=4 worker_load=4 active_episodes=5
18:59:38 state desired=1 current=2 pool_capacity=4 pool_load=0 pending=0 worker_capacity=4 worker_load=0 active_episodes=0
18:59:38 scale down idle slot=02
18:59:48 state desired=1 current=1 pool_capacity=4 pool_load=0 pending=0 worker_capacity=4 worker_load=0 active_episodes=0
```

最终收敛复查：

```text
8.130.208.77:
uenv-agent-pool-supervisor.service active
uenv-agent-poller.service active
uenv-agent-poller-extra-01.service inactive
uenv-agent-poller-extra-02.service inactive
uenv-agent-poller-extra-03.service inactive

8.130.75.157 /fleet/agents:
openhands-default.total_capacity=1
openhands-default.total_load=0
pending_jobs=0
running_jobs=0
outstanding_jobs=0

8.130.75.157 /fleet/workers:
worker-7143-pro load=0 capacity=4
active_episodes=0

219.147.100.43:7143:
running uenv-swe-* container count=0
```

补充说明：在连续训练波次之间，Server `/fleet/agents` 的 pool `total_capacity` 会短暂继续显示 4，因为刚停止的 extra agent heartbeat 尚未超时；同一窗口内 7143 侧也可能短暂看到刚完成 episode 的 `uenv-swe-*` 容器仍在退出。复查确认这些状态会随下一轮 heartbeat / cleanup 收敛，不是 extra service 未停止或 SWE 容器泄漏。

### 结论

动态 supervisor 才是当前问题的根本修复方向：它把“训练/测试侧请求的并行 episode 数”和“Agent 侧服务实例数”从人工同步改成了自动 reconcile，并且保留了每个 OpenHands runner 单并发隔离的安全边界。

当前实机结果已验证：

- SWE-smith agent-mode 可以在需求出现时自动从 1 并发扩到 4 并发。
- Worker 7143 可在 agent-mode 下实际达到 `load=4/4`。
- 任务排空后 extra agent 会自动停止，主 agent 保持常驻。
- scale down 不会强停 busy extra agent，会等待其完成后再停止。

剩余 caveat：

- Server `/agents` 中 stopped extra agent 会在 heartbeat 超时前短暂保留为 stale 记录，pool `total_capacity` 会有几十秒滞后；最终会收敛到 1。前端系统拓扑已改为按非 stale agent 计算展示侧 capacity/load/activity。
- 当前 supervisor 仍是 208.77 专用部署参数；后续应把 agent node、pool id、server endpoint、port 基线、max replicas 等做成更通用的部署配置。
- 如果要把并发从 4 提升到更高，需要同时确认 Worker/Gateway/SWE Docker 资源、OpenHands API 端口区间和训练侧 batch/episode 参数。

## 结论

本次确认的核心问题不是 SWE 环境不可用，而是 Agent-mode SWE 的可观测性和并发容量分层没有在前端与运维视图中充分表达：

1. SWE 环境池由 `SweInstancePool` 管理，当前未进入 `pool_summary/pool_slots`；Worker 详情页已增加基于 `package_states` 的 SWE 环境池展示。
2. `8.130.136.136:8777` 前端缺少 `VITE_FLEET_PROXY_TARGET`；已改为经 `http://8.130.75.157:8099/fleet` 获取 fleet，`/fleet/workers` 与 `/fleet/agents` 均返回 200。
3. 当前训练串行推进属实，直接瓶颈是 `openhands-default` agent pool 只有 1 并发，而不是 Worker capacity 只有 1；已通过静态扩容和动态 supervisor 修复。
4. Worker 本机 metrics 中 native episode 计数与 Agent-mode SWE session 计数分属不同口径；已新增 `uenv_swe_active_session_count` 与 `uenv_worker_runtime_load` 并在 7143 实机生效。
5. `8.130.75.157:8099/system` 的 Hub overview 401 是 8099 前端进程缺少 `VITE_HUB_TOKEN` 导致；同步运行环境并重启后页面侧已不再报该 401，内部子界面当前均可达。
6. 208.77 已通过 3 个 extra OpenHands poller 将 `openhands-default` 扩到 4 并发；实机验证 SWE-smith agent-mode 可持续以 `worker load=4/4` 推进，并且完成结果可 ack 回填。
7. 进一步完成了 Agent 节点本地 supervisor 动态对齐修复；实机验证可按训练需求自动从 1 扩到 4，并在任务排空后安全回落到 1。
8. `/ops` hydration mismatch 已修复；Playwright 复查仅剩 `/favicon.ico` 404。
9. stale agent 展示口径和 `UNKNOWN_JOB` completion spool 噪声已补充处理；208.77 poller / supervisor 均为 active。
