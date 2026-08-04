# SWE 训练 Obs Episode 事件缺失 Server 侧核验说明

> 日期：2026-08-04
> 面向对象：Server / adapter-core / Worker / 可视化观测链路维护者
> 关联 run：`verl_swesmith_grpo_train_20260803_213856`、`verl_swesmith_grpo_train_20260804_102356`

## 1. 背景

Adapter 侧运行 `Qwen3.6-35B-A3B + VeRL + UEnv + SWE-smith GRPO` 训练后，前端页面能看到对应 run，但工作流视图中的 episode 关联实体计数仍为 0。

初步判断：这不是前端布局或页面渲染问题，而是前端正在读取的 Obs state 中没有 episode 级事件。Adapter 的 run 生命周期事件已经进入 Obs，但 Server / adapter-core / Worker 侧 episode 事件没有进入同一个 run 的 Obs 聚合状态。

## 2. 当前证据

本轮 run：

```text
run_id = verl_swesmith_grpo_train_20260803_213856
```

Adapter 侧本地日志目录：

```text
/data/ronghao/uenv/uenv-bridge/temp/logs/layer4_distributed/verl_swesmith_grpo_train_20260803_213856/
```

当前本地复核结果：

| 文件 | 行数 / 状态 |
|---|---:|
| `agent-loop-requests.jsonl` | 40 |
| `agent-loop-results.jsonl` | 40 |
| `model-gateway.jsonl` | 475 |
| `agent-loop-results.jsonl` 状态 | 15 completed / 25 failed |

这说明 Adapter 侧确实产生了 episode 请求与结果记录，gateway 也有模型调用记录。

前端 Obs state 接口：

```bash
curl http://8.130.75.157:8888/obs/api/v1/runs/verl_swesmith_grpo_train_20260803_213856/state
```

当前返回的关键字段：

| 字段 | 当前值 |
|---|---|
| `run_state` | `CLOSED` |
| `global_event_seq` | `15` |
| `episodes` | `{}` |
| `workflow.nodes[].payload_summary` | `null` |

这说明 Obs 已经收到 run 生命周期事件，并且 run 已能收口到 `CLOSED`；但 episode 级事件仍没有进入该 run 的 state。

## 3. 20260804 run 补充复核

新一轮 run：

```text
run_id = verl_swesmith_grpo_train_20260804_102356
```

前端 URL：

```text
http://8.130.75.157:8888/?run=verl_swesmith_grpo_train_20260804_102356
```

本地 Adapter 结果文件显示：

| 文件 | 行数 / 状态 |
|---|---:|
| `agent-loop-requests.jsonl` | 112 |
| `agent-loop-results.jsonl` | 112 |
| `agent-loop-results.jsonl` 状态 | 108 failed / 4 completed |

失败原因集中为：

```text
swe instance_id ... not in catalog (size=736)
```

这类错误发生在 Worker 查 SWE-smith catalog 的早期阶段。Server / Worker 已经把失败结果返回给 Adapter，因此本地 `agent-loop-results.jsonl` 能看到 `failed`；但前端 Obs state 没有体现这些失败。

当前前端 Obs state 关键字段为：

```text
run_state = CLOSED

episodes:
  ACTIVE = 108
  DONE = 4

workflow:
  SUBMIT = 112
  REPORT = 4
  DONE = 4
```

这说明前端确实收到了 112 条提交事件，但 108 条失败 episode 没有进入 `FAILED` 终态，而是一直停在 `ACTIVE`。因此页面看起来不像“报错”，只是大量任务卡在进行中 / 调度中。

Server 的 Obs merge 逻辑本身支持失败终态；只要收到下面事件，前端就会显示失败：

```text
EPISODE_FAILED
EPISODE_CLOSED
```

当前缺失的是：`not in catalog` 这类早期失败没有被投递成 Obs 的 `EPISODE_FAILED` / `EPISODE_CLOSED`。

另外，这个 run 在前端里已经是 `CLOSED`，但训练日志仍显示跑到 step 13/50。这说明 Adapter 当前可能按 rollout batch 发 `RUN_STARTED` / `RUN_STOPPED` / `RUN_CLOSED`，而不是按整个训练进程发一次 run 生命周期事件；因此前端的 run 状态暂时不能完全等同于训练进程状态。

建议处理顺序：

1. Server / Worker 侧先确认：`not in catalog` 这种早期失败是否会发 `EPISODE_FAILED` 和 `EPISODE_CLOSED` 到 Obs。
2. 如果 Server / Worker 不方便覆盖所有失败路径，Adapter 可以在收到 `EpisodeResult.status=failed` 后补发失败事件作为 fallback。
3. 后续整理 run 生命周期：`RUN_STARTED` 对应训练开始，`RUN_CLOSED` 对应训练真正结束，batch / step 用单独事件表示。

## 4. Adapter 侧当前行为

Adapter 当前主动上报的是 run 边界事件：

- `RUN_STARTED`
- `RUN_STOPPED`
- `RUN_CLOSED`

相关代码：

- [uenv-bridge/src/uenv/bridge/verl_agent_loop.py](../../uenv-bridge/src/uenv/bridge/verl_agent_loop.py)
- [uenv-bridge/src/uenv/bridge/obs_client.py](../../uenv-bridge/src/uenv/bridge/obs_client.py)

Adapter 在 `run_batch` 边界调用 `obs_client.run_started/run_stopped/run_closed`。因此前端能看到 run，并不代表 episode 事件已经进入 Obs。

workflow 的 episode 计数依赖 Server Obs merge 层收到 episode 事件后更新。相关代码：

- [uenv-server/src/obs/merge.rs](../../uenv-server/src/obs/merge.rs)

因此当前现象更像是：run 事件到达了 `8.130.75.157:8888/obs` 背后的 Obs 实例，但 Server / adapter-core / Worker 的 episode 事件没有进入同一个 `training_run_id` 的 Obs state。

## 5. 需要 Server / Worker 侧核验的问题

请优先检查 `8.130.75.157:8088` 上处理 `ExecuteBatch` 的进程。

建议核验项：

| 核验项 | 需要确认的问题 |
|---|---|
| Obs 是否启用 | adapter-core / Server 处理 `ExecuteBatch` 时是否启用了 Obs 上报。 |
| Obs URL 是否一致 | episode 事件是否投递到 `http://8.130.75.157:8888/obs` 背后的同一个 Obs 实例。 |
| run id 是否透传 | `training_run_id` / `run_id` 是否保持为 `verl_swesmith_grpo_train_20260803_213856`。 |
| 事件是否上报成功 | Server / Worker 日志里是否有 Obs HTTP 上报失败、鉴权失败、连接失败或超时。 |
| 事件类型是否被 merge 识别 | episode started/completed/failed/closed 等事件类型是否符合 `uenv-server/src/obs/merge.rs` 识别的事件名。 |
| 字段是否完整 | episode 事件里是否包含 `episode_id`、`training_run_id`、状态、payload summary 所需字段。 |

建议直接 grep：

```bash
grep -R "verl_swesmith_grpo_train_20260803_213856" /path/to/server/logs /path/to/worker/logs
grep -R "obs" /path/to/server/logs /path/to/worker/logs
grep -R "EPISODE" /path/to/server/logs /path/to/worker/logs
```

如果 server/worker 侧日志中能看到 episode 执行完成，但 Obs state 中 `episodes={}`，则重点检查 Obs 投递地址、run id 透传和 merge 事件类型。

## 6. 期望行为

同一 run 下，Obs state 应至少能看到：

```text
run_state = CLOSED
episodes 非空
workflow SUBMIT / DISPATCH / EXECUTE / REPORT / DONE 节点有 episode 关联实体计数
workflow.nodes[].payload_summary 非空或至少包含 episode 计数摘要
tree 中出现 run / episode / step 节点
```

前端 URL：

```text
http://8.130.75.157:8888/?run=verl_swesmith_grpo_train_20260803_213856
```

页面上应能看到该训练 run 的 episode 流转，而不是只有 run 生命周期状态。
