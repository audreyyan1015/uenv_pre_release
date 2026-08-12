# SWE-smith Gateway 执行超时与残留 Episode 修复报告

> 日期：2026-08-11
> 范围：7143 Worker Runtime Gateway、OpenHands Agent、Server active episode 清理
> 关联 episode：`23fe3d1b-e108-4960-96dd-d8a0ebeab4ab`、`fb66aa51-5b04-4b6e-a97c-33ab2af96325`

## 1. 问题

上述两个 episode 通过 OpenHands Agent 的 Runtime Gateway 执行 pydicom 测试。Worker
进程和 Server heartbeat 正常，但容器内 pytest 长时间等待，episode 一直保持 active。

此前的 pytest batch timeout 和 native `DispatchEpisode` timeout 没有覆盖 Gateway 的
`session.exec` / `session.submit` 路径；OpenHands 传入的 `timeout` 也只是客户端 HTTP
读取超时，不会终止远端容器进程。

## 2. 根因

Gateway 原路径为：

```text
POST /sessions/{id}/exec -> pool.exec -> SweSession::exec
POST /sessions/{id}/submit -> spawn_blocking(pool.submit)
```

`submit` 后台任务没有服务端 deadline，`exec` 只使用了底层默认路径。客户端的
`urllib` timeout 断开 HTTP 等待后，远端 pytest 仍可继续运行。

## 3. 修复

### 3.1 Gateway exec 服务端 timeout

`SweSession::exec` 现在使用独立的 `timeout --kill-after=30s docker exec`，默认 180 秒，
可通过 `UENV_WORKER_GATEWAY_EXEC_TIMEOUT_SECS` 调整。

### 3.2 Gateway submit 服务端 timeout

`POST /submit` 的后台任务增加 1800 秒服务端 deadline，可通过
`UENV_WORKER_GATEWAY_SUBMIT_TIMEOUT_SECS` 调整。超时后记录
`gateway_submit_timeout`，并从 session pool 移除 session，触发容器释放。

### 3.3 与既有超时分层

```text
Agent 单命令 / Gateway exec       180s
单个 pytest batch                  900s
Gateway submit                    1800s
native docker exec                1200s
native episode                    1800s
```

## 4. 验收要求

1. Worker 二进制包含新的 Gateway timeout 实现并重启。
2. 7143 上设置并记录 Gateway timeout 环境变量。
3. 清理本次遗留的 pydicom pytest、xargs、timeout 进程和容器。
4. Server `/status` 中两个 episode 不再 active，Worker load 恢复。
5. 新建一个短时测试 session，验证 Gateway exec 正常；超时测试验证服务端返回/释放。
6. 后续长测试确认 `pytest`、`xargs`、容器和 Server episode 均能在 deadline 后退出。

## 5. 本次现场处置结果

7143 已同步 `runtime_gateway.rs`、`session.rs` 和 `instance_pool.rs`，并在服务器完成：

```text
bash scripts/gen-worker-proto.sh
cargo build -p uenv-worker --release
```

release build 成功。Worker 使用以下超时启动：

```text
UENV_WORKER_GATEWAY_EXEC_TIMEOUT_SECS=180
UENV_WORKER_GATEWAY_SUBMIT_TIMEOUT_SECS=1800
UENV_WORKER_EPISODE_TIMEOUT_SECS=1800
UENV_WORKER_EXEC_TIMEOUT_SECS=1200
```

同时清理了遗留的 pydicom 容器。Server 在线状态已恢复：

```json
{
  "active_episodes": 0,
  "pending_results": 0,
  "worker_count": 1,
  "load": 0,
  "status": "ready"
}
```

Worker 当前 `28097`、`28777`、`28888` 均监听，health 返回正常，Server heartbeat
持续正常。

本次没有再次启动完整 pydicom 长测，避免重新占用生产评测槽位；因此“600 秒后自动
超时”的在线耗时回归仍需在隔离 episode 上执行。代码构建和残留资源清理已完成。

## 6. 限制

Rust 的 `spawn_blocking` 超时会停止等待并执行 session cleanup，但无法强制取消已经
进入阻塞系统调用的线程。因此 `SweSession::exec` 自身也必须使用外部 `timeout`；本次
修复已同时覆盖该层。后续如需更强的任务取消语义，应将每次评测放入独立可杀进程组。

## 7. 增量更新：终止状态与联调复核（2026-08-11）

针对上一节指出的 `spawn_blocking` 无法强制取消阻塞线程风险，Worker 的
`SweSession` 增加了显式终止状态：

```text
terminated: AtomicBool
```

`terminate()` 现在先设置终止标记，再执行：

```text
docker rm -f <container>
```

`evaluate()` 在评测开始、依赖安装、test patch、pre-test、pytest 返回和 reward
组装等关键阶段调用 `ensure_active()`。超时清理后，即使原阻塞任务稍后从 Docker CLI
返回，也不能继续将已终止 session 作为有效结果完成。

### 7.1 目标机重新构建与重启

7143 已重新执行：

```text
cargo build -p uenv-worker --release
```

release build 成功。新 Worker 进程为 `3785839`，实际环境变量保持为：

```text
UENV_WORKER_GATEWAY_EXEC_TIMEOUT_SECS=180
UENV_WORKER_GATEWAY_SUBMIT_TIMEOUT_SECS=1800
UENV_WORKER_EXEC_TIMEOUT_SECS=1200
UENV_WORKER_EPISODE_TIMEOUT_SECS=1800
UENV_WORKER_DISPATCH_HEARTBEAT_SECS=15
```

### 7.2 联调结果

```text
28777 health -> ok
28097 Runtime Gateway -> ok
28888 -> listening
```

Server `/status` 复核：

```json
{
  "ready": true,
  "accepting": true,
  "active_episodes": 0,
  "pending_results": 0,
  "worker_count": 1,
  "load": 0
}
```

Worker 已重新注册并持续 heartbeat，主机上未发现残留的 `pytest`、`xargs` 或
`timeout ... pytest` 进程。

本次增量更新未启动完整 pydicom 长测；已完成目标机 release build、Worker 重启、
Gateway/health/gRPC 联调、Server 状态复核和残留进程检查。完整超时耗时回归仍应在
隔离 session 中执行，避免占用生产评测槽位。
