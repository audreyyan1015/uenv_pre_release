# Server Obs 真实联调状态收口修改说明

> 日期：2026-07-26
> 面向对象：Server / 可视化观测链路维护者
> 关联提交：`4cb5a61 fix(obs): 修正真实联调观测状态收口`

## 1. 当前现象与证据

Adapter 侧已经用真实 `qa` Episode 驱动了 Server Obs 与前端页面，链路本身是通的。

本次证据：

- 运行日志：`/data/ronghao/uenv/uenv-bridge/temp/logs/frontend_obs_smoke/bridge-smoke-evidence-20260726-210249.log`
- Obs state：`/data/ronghao/uenv/uenv-bridge/temp/logs/frontend_obs_smoke/bridge-smoke-evidence-20260726-210249.state.json`
- 前端 URL：`http://8.130.75.157:8888/?run=bridge-smoke-evidence-20260726-210249`

运行结果：

| 项 | 结果 |
|----|------|
| `gsm8k` | `status=completed, reward=1` |
| `pubmedqa` | `status=completed, reward=1` |
| `scitab` | `status=completed, reward=1` |
| `olymmath-easy` | `status=completed, reward=1` |

Obs state 中已经能看到：

| 字段 | 当前值 |
|------|--------|
| `run_state` | `CLOSED` |
| `episode_count` | 4 |
| `episode_statuses` | `DONE` |
| `workflow` | `SUBMIT/DISPATCH/EXECUTE/REPORT/DONE` 全部 `DONE` |

但对象树仍有两个状态没有收口：

| 字段 | 当前值 | 期望值 |
|------|--------|--------|
| tree root `run:*` | `PENDING` | `CLOSED` |
| tree `step:*` | `ACTIVE` | `DONE` 或 `FAILED` |

这说明真实链路与前端页面都已经能运行展示，但 Server Obs 的对象树投影存在状态不一致。

## 2. 为什么需要 Server 侧修改

这个问题不应只在前端修。

前端展示的数据来自 Server Obs 的 `GET /obs/api/v1/runs/{run_id}/state`。当前 state API 本身已经返回了互相矛盾的状态：

- 顶层 `run_state=CLOSED`
- workflow 全部 `DONE`
- episode 全部 `DONE`
- 但 tree root 仍是 `PENDING`
- tree step 仍是 `ACTIVE`

如果只在前端做显示兜底，其他消费 Obs state 的工具仍会读到不一致状态。因此应该在 Server 的 Obs merge/projection 层把状态收口做好。

根因是：

1. `RUN_CLOSED` 事件只更新了 `ChainState.run_state`，没有同步更新对象树根节点 `run:{training_run_id}` 的 `status`。
2. `EPISODE_COMPLETED/EPISODE_FAILED/EPISODE_CLOSED` 只更新了 episode 节点，没有把已创建的 step 节点从 `ACTIVE` 收口。
3. 对于部分 Worker/插件路径，Server 会收到 `STEP_STARTED` 或流式进度事件，但终态结果未必伴随完整的 `STEP_COMPLETE` 事件；此时应该由 episode 终态事件兜底关闭 step。

## 3. 需要修改的 Server 文件

### 3.1 `uenv-server/src/obs/merge.rs`

需要在 Obs 投影层做两类状态同步。

第一类：run 生命周期同步到对象树根节点。

建议逻辑：

| 事件 | `ChainState.run_state` | tree root `status` |
|------|------------------------|--------------------|
| `RUN_STARTED` | `RUNNING` | `ACTIVE` |
| `RUN_STOPPED` | `STOPPING` | `ACTIVE` |
| `RUN_CLOSED` | `CLOSED` | `CLOSED` |

第二类：episode 终态收口 step 节点。

当收到：

- `EPISODE_COMPLETED`
- `EPISODE_FAILED`
- `EPISODE_CLOSED`

时，除了更新 episode 节点状态，还需要将该 episode 下仍为 `ACTIVE` 的 step 节点同步为：

- episode 成功：`DONE`
- episode 失败：`FAILED`

本地已经实现的关键函数：

```rust
fn set_run_tree_status(state: &mut ChainState, status: &str) {
    let node_id = format!("run:{}", state.training_run_id);
    if let Some(n) = state.tree.nodes.iter_mut().find(|n| n.node_id == node_id) {
        n.status = status.into();
    }
}

fn close_tree_steps_for_episode(state: &mut ChainState, episode_id: &str, status: &str) {
    let parent = format!("episode:{episode_id}");
    for n in state.tree.nodes.iter_mut().filter(|n| n.parent_id == parent) {
        if n.kind == "step" && n.status == "ACTIVE" {
            n.status = status.into();
        }
    }
}
```

### 3.2 `uenv-server/src/obs/tests.rs`

需要新增单测覆盖如下事件序列：

```text
RUN_STARTED
EPISODE_SUBMITTED
EPISODE_DISPATCHED
STEP_STARTED
EPISODE_COMPLETED
EPISODE_CLOSED
RUN_CLOSED
```

期望断言：

- `state.run_state == "CLOSED"`
- tree root `run:{run_id}` 的 `status == "CLOSED"`
- tree episode 节点 `status == "DONE"`
- tree step 节点 `status == "DONE"`

本地新增的测试名：

```rust
terminal_events_close_run_and_step_tree_nodes
```

## 4. 部署要求

Server 侧需要拉取并部署包含 `4cb5a61` 的 `feature/verl-bridge-adapter` 分支。

高层步骤：

```bash
cd /root/UEnv   # 以 Server 实际部署目录为准
git fetch origin
git checkout feature/verl-bridge-adapter
git pull origin feature/verl-bridge-adapter

# 重新构建 Server/adapter-core 二进制，具体命令按 Server 当前部署方式执行。
# 之前部署文档中使用的是 uenv-adapter-core 作为 8.130.75.157:8088 的入口。
cargo build --release -p uenv-adapter-core

# 替换线上二进制并重启服务，服务名以 Server 机器实际 systemd/nohup 配置为准。
```

如果当前 Server 机器仍是手动二进制方式，需要确认正在监听 `8.130.75.157:8088` 与 Obs `:50053` / 前端 `/obs` 代理的进程确实换成了新二进制。

## 5. 验收方式

部署后重新跑 Adapter 侧 smoke：

```bash
RUN_ID=bridge-smoke-server-obs-$(date +%Y%m%d-%H%M%S)
python3 /data/ronghao/uenv/uenv-bridge/scripts/smoke_qa_datasets_grpcurl.py \
  8.130.75.157:8088 \
  --run-id "$RUN_ID" \
  --obs-url http://8.130.75.157:8888/obs

curl -sS "http://8.130.75.157:8888/obs/api/v1/runs/$RUN_ID/state" | python3 -m json.tool
```

需要同时满足：

| 检查项 | 期望 |
|--------|------|
| smoke 四个 case | 全部 `completed/reward=1` |
| `run_state` | `CLOSED` |
| workflow | 全部 `DONE` |
| episode 状态 | 全部 `DONE` |
| tree root 状态 | `CLOSED` |
| tree step 状态 | `DONE` 或失败场景下 `FAILED` |

前端检查：

```text
http://8.130.75.157:8888/?run=<RUN_ID>
```

页面应展示该真实 run，而不是 `_orphan` 或 fixture/mock 回落；顶部状态应与 state API 中的 `run_state` 一致，对象树不应再出现 run 已关闭但根节点仍 `PENDING`、step 仍 `ACTIVE` 的矛盾状态。

## 6. Adapter 侧已经完成的配套改动

Adapter 侧 smoke 脚本已支持：

- `--run-id`：把 `training_run_id` 写入 `sampleContextJson`，供 Server Obs 投影使用；
- `--obs-url`：向 Obs 上报 `RUN_STARTED/RUN_CLOSED`；
- 保留原有 `qa` 四数据集 smoke 逻辑。

因此 Server 部署完成后，不需要再改前端或 Adapter 脚本即可复验。
