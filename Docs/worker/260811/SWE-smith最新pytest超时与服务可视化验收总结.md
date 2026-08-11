# SWE-smith 最新 pytest 超时与服务可视化验收总结

> 日期：2026-08-11
> 范围：7143 Worker、8.130.75.157 Server、8099 可视化界面、208.77 OpenHands Agent 池

## 1. 结论

1. 最新 pytest 长时间运行问题已经确认，根因是超长测试列表分批后，单个 pytest batch 没有超时保护，导致容器内 `xargs -> pytest` 长时间等待并占满 Worker/Agent 资源。
2. Worker 已增加两级超时保护并完成 7143 release build 和重启。
3. Server 已修复“Worker 注册被拒绝后仍把 heartbeat 返回为成功”的状态失真问题，当前 Worker 可以重新注册并出现在 Server registry 中。
4. 当前服务器可视化页面和核心模块均已通过本次在线验收，8099 前端不再显示错误的离线状态。

## 2. pytest 超时问题

### 2.1 现象

受影响样本为 pydicom 等包含大量 pytest node id 的 SWE-smith 任务。此前 4 个 episode 在 Worker 上运行约 40 多分钟，容器状态仍为运行中，内部进程为：

```text
xargs -> python -m pytest
```

典型进程状态：

```text
WCHAN=wait_woken / do_wait
CPU 约为 0%
```

Server 侧同时显示 OpenHands Agent pool 的 4 个容量全部占用，导致后续任务无法调度。

### 2.2 根因

超长测试列表已经从 `docker exec` argv 移到容器内文件，并使用 `xargs` 分批执行，但原先没有给每个 batch 设置独立超时：

```bash
xargs -r -0 -n 100 python -m pytest ...
```

因此即使单个 pytest batch 因 fixture、参数化测试或测试本身长时间等待，也不会主动结束。Worker 的 episode 超时也没有可靠地终止容器内完整进程树，最终形成：

```text
pytest 长时间等待
  -> Worker 容器持续占用
  -> OpenHands runner 不回 CompleteAgentJob
  -> Server AgentJob 持续 in-flight
  -> Agent pool 容量被占满
```

## 3. 修复方式

### 3.1 测试 batch 超时

长测试列表现在使用：

```bash
xargs -r -0 -n 100 \
  timeout --kill-after=30s 600s \
  python -m pytest -rA -v -p no:cacheprovider \
  < /tmp/uenv-pytest-nodeids
```

当前单个 batch 阈值：

```text
600 秒
```

超时后：

```text
先发送终止信号
等待 30 秒
仍未退出则强制 kill
```

### 3.2 Docker exec 总超时

Worker `exec_raw()` 现在使用外部 `timeout` 包裹 Docker exec：

```bash
timeout --kill-after=30s 900s \
  docker exec <container> bash -lc <command>
```

配置项：

```text
UENV_WORKER_EXEC_TIMEOUT_SECS
```

默认值：

```text
900 秒
```

当前 7143 进程未显式设置该变量，因此使用默认值 900 秒。

### 3.3 Worker episode 超时

7143 重启脚本当前设置：

```text
UENV_WORKER_EPISODE_TIMEOUT_SECS=600
```

此外 dispatch heartbeat 周期为：

```text
UENV_WORKER_DISPATCH_HEARTBEAT_SECS=15
```

该值是 heartbeat 周期，不是测试超时。

## 4. Worker 注册状态修复

### 4.1 原状态失真

Worker 重启时曾出现：

```text
worker_reregister_rejected_active_lease
accepted=false
```

但后续 heartbeat 仍被 Server 返回 `ok=true`，造成：

```text
Worker 本地 heartbeat 正常
Server scheduler 没有 Worker 记录
/fleet/workers 返回 worker_count=0
前端显示离线
```

### 4.2 修复

Server：

- 未注册 Worker 的 heartbeat 返回 `ok=false`。
- 增加 `heartbeat_from_unregistered_worker` 日志。

Worker：

- 收到 `HeartbeatResponse.ok=false` 后自动重新注册。
- `RegisterWorkerResponse.accepted=false` 时明确返回注册错误并触发重试。

涉及代码：

```text
uenv-server/src/control_plane.rs
uenv-worker/src/control_plane/client.rs
```

## 5. 服务器在线验收

### 5.1 Server

Server `uenv-adapter-core` 已使用最新构建重新启动，当前状态：

```text
0.0.0.0:8088 -> LISTEN
0.0.0.0:8077 -> LISTEN
127.0.0.1:50052 -> LISTEN
```

Server admin 状态接口返回：

```json
{
  "ready": true,
  "accepting": true,
  "total_capacity": 4,
  "worker_count": 1,
  "workers": [
    {
      "worker_id": "worker-7143-pro",
      "endpoint": "219.147.100.43:28888",
      "capacity": 4,
      "load": 0,
      "status": "ready"
    }
  ]
}
```

### 5.2 Worker

7143 当前检查：

```text
28777/health -> ok
28097/runtime/v1/health -> ok
28888 -> LISTEN
```

Worker 日志持续输出：

```text
worker_id=worker-7143-pro
msg="heartbeat"
```

Worker 已重新加载：

```text
swe-bench-pro: 731 instances
swe-bench-smith: 59136 instances
runtime gateway catalog: 59867
```

### 5.3 Trajectory 服务

```text
GET /control/v1/trajectories/health -> HTTP 200
{"data_dir":"/home/uenv/trajectory-data","db":"ok"}
```

### 5.4 OpenHands Agent 池

Agent 状态接口返回正常：

```text
openhands-default total_capacity=4

当前 OpenHands Agent 实例均为非 stale，poller 和 gateway tunnel 服务正常。

### 5.5 8099 可视化界面

```text
GET http://8.130.75.157:8099/server?run=null -> HTTP 200
GET http://8.130.75.157:8099/fleet/workers -> HTTP 200
```

`/fleet/workers` 当前返回 `worker_count=1`，前端页面的 Worker 状态数据源已恢复，不再是空列表。

8099 由 Vite 前端服务监听：

```text
0.0.0.0:8099 -> node vite dev
```

## 6. 当前风险与后续建议

当前 `episode timeout=600s`、`exec timeout=900s` 的层级仍不完全一致。为了给错误封装、容器清理和结果上报预留时间，生产环境建议将：

```text
UENV_WORKER_EXEC_TIMEOUT_SECS=540
```

使 `docker exec` 先于 episode 总超时结束。

此外，本次验证确认服务健康和进程清理链路已恢复，但没有再次运行完整 pydicom 全量测试集。下一次长测试回归应重点检查：

- 是否在 600 秒 batch 超时后退出；
- 容器内 `xargs` 和 pytest 子进程是否全部消失；
- Worker 是否释放容器和实例池槽位；
- OpenHands 是否发送 `CompleteAgentJob`；
- Server `/fleet/workers` 和 `/fleet/agents` 是否恢复空闲容量。
