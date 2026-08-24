# Server Obs 摘要与时间轴接口修改说明

## 1. 修改背景

前端进展页需要分页展示训练 run 列表，并在不拉取完整 `ChainState` 的情况下展示任务总数、运行状态、episode 统计和任务时间轴。旧接口只能返回 run id 列表，前端只能显示“已加载任务 5”，无法显示全局任务总数。

## 2. 修改内容

本次 server 侧修改已提交为：

```text
b571915 feat(server): 增加 Obs 摘要与时间轴接口
```

主要改动如下：

| 路径 | 内容 |
| --- | --- |
| `uenv-server/src/obs/http.rs` | 为 `/api/v1/runs` 增加 `limit` / `offset` 分页参数；新增 `/api/v1/runs/summary` 和 `/api/v1/runs/{run_id}/timeline` |
| `uenv-server/src/obs/mod.rs` | 新增 `RunSummary`、`RunTimelineItem`，从 Obs 状态和事件历史生成摘要与时间轴 |
| `uenv-server/src/obs/store.rs` | 新增按 `training_run_id` 读取 accepted events 的方法 |
| `uenv-server/src/obs/tests.rs` | 补充 summary 与 timeline 单元测试 |

补充修改：Obs 支持 run 级计划量字段 `planned_episode_total` 和 `planned_step_total`。Adapter 会在 `RUN_STARTED` 的 payload 中上报计划总量，Server Obs 保存后通过 `ChainState` 和 `/runs/summary` 返回，前端用该计划量展示整体进度。

进一步补充：Obs 支持 run 级显式生命周期字段。Adapter 在训练期间上报 `RUN_HEARTBEAT`，训练脚本在 VeRL 进程退出后上报 `RUN_COMPLETED`、`RUN_TERMINATED` 或 `RUN_FAILED`。Server Obs 聚合为 `run_status`、`terminal_reason`、`last_heartbeat_ts` 和 `heartbeat_state`，前端直接使用这些字段展示运行中、已完成和已终止状态。

## 3. 新增接口

### 3.1 Run 摘要分页

```text
GET /api/v1/runs/summary?limit=5&offset=0
```

用于返回分页 run 摘要和全局状态计数。前端会用其中的 `total` 显示“任务总数”。

摘要字段中新增：

| 字段 | 含义 |
| --- | --- |
| `planned_episode_total` | 该 run 计划发送的 episode 总数；为 0 或缺失表示未知 |
| `planned_step_total` | 该 run 计划执行的训练 step 总数；为 0 或缺失表示未知 |
| `run_status` | run 级状态：`running`、`stopping`、`completed`、`terminated`、`failed` 等 |
| `terminal_reason` | run 结束原因；未结束时为空 |
| `last_heartbeat_ts` | 最近一次 run heartbeat 时间戳 |
| `heartbeat_state` | heartbeat 状态；当前使用 `alive`、`closed`、`unknown` |

### 3.2 Run 时间轴

```text
GET /api/v1/runs/{run_id}/timeline
```

用于返回单个 run 的阶段时间轴，包括提交、调度、环境执行、结果回传、完成或失败收口等阶段。

## 4. 部署注意

前端如果仍显示“已加载任务 5”，通常说明当前 `/obs` 反代背后的 Obs 服务还没有更新到包含上述接口的 server 版本。部署侧需要确认：

1. `/obs` 反代指向的是目标 `uenv-server` Obs 实例；
2. `uenv-server` 已更新到包含提交 `b571915` 的版本；
3. 更新后已重启 server/Obs 进程；
4. 以下接口返回 200：

```bash
curl -i 'http://<obs-base-url>/api/v1/runs/summary?limit=5&offset=0'
curl -i 'http://<obs-base-url>/api/v1/runs/<run_id>/timeline'
```

## 5. 验证结果

本地已执行：

```bash
cargo test -p uenv-server obs --lib
```

结果为 `16 passed`。
