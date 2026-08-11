# SWE-smith 官方镜像与 OpenHands Agent 链路验收报告

> 日期：2026-08-11
> 关联说明：`Docs/adapter/20260811-SWE-smith镜像命名空间不一致Worker处理说明.md`
> 核验主机：7143 Worker、8.130.75.157 Server、8.130.208.77 OpenHands Agent 池

## 1. 结论

本次验收得到以下结论：

1. SWE-smith 镜像 namespace 不一致问题已经解决。7143 上旧 `jyangballin/swesmith.*` tag 已清除，222 个本地 Smith 镜像均使用官方 `swebench/swesmith.*` tag。
2. 7143 上全量 Smith catalog 和 `images.manifest.json` 均已切换到官方 namespace，59136 条 instance 没有旧 namespace 残留。
3. Worker、Server、Trajectory 服务、OpenHands runner、Agent poller 和 Gateway SSH tunnel 均在线。
4. 已在 8.130.208.77 执行 OpenHands + SWE gold 端到端验收，得到 `resolved=true`、`reward=1.0`、`tests_passed=56/56`、`server_verified=true`。
5. Server 日志中的 OlymMATH `HTTP/2 CANCEL` 记录不是当前仍在增长的错误。该日志文件最后修改于 2026-07-15，记录对应 2026-07-15 的旧批次；当前 Server 进程已运行至 2026-08-11，当前核验未发现新的 OlymMATH CANCEL 记录。

## 2. SWE-smith 镜像验收

### 2.1 Docker 镜像

7143 实机核验结果：

```text
swebench/swesmith.*: 222
jyangballin/swesmith.*: 0
```

此前失败样例现在可以使用官方 tag：

```text
swebench/swesmith.x86_64.hypermodeinc_1776_ristretto.da570116:latest
```

该镜像 `docker image inspect` 成功。

### 2.2 EnvPackage 与 catalog

当前 Smith package：

```text
/var/lib/uenv/envs/swe-bench-smith/0.1.0-local
```

核验结果：

| 项目 | 结果 |
|---|---:|
| catalog instance | 59136 |
| catalog 官方 namespace 引用 | 59136 |
| catalog 旧 namespace 引用 | 0 |
| images.manifest 官方 namespace 引用 | 59136 |
| images.manifest 旧 namespace 引用 | 0 |

Worker 日志确认：

```text
package_id=swe-bench-smith
images=59136
count=59136
msg="swe_catalog_loaded_from_env_package"
```

### 2.3 Worker 状态

7143 当前 Worker 进程为：

```text
uenv-worker --config config/uenv-worker.deploy-7143-swe-pro.yaml serve
```

服务状态：

| 服务 | 验收结果 |
|---|---|
| `28777/health` | `ok` |
| `28097/runtime/v1/health` | `ok` |
| `28888` | LISTEN |
| `28777` | LISTEN |
| `28097` | LISTEN |
| Worker -> Server heartbeat | 持续正常 |
| `UENV_SWE_SMITH_EVAL_CMD` | 已设置 |
| `UENV_SWESMITH_REPO` | 已设置 |

Runtime Gateway 的正确健康路径是 `/runtime/v1/health`。旧路径 `/health` 返回 404，不代表 Gateway 故障。

## 3. Server 与 Agent 池验收

### 3.1 Server

8.130.75.157 上运行单一 `uenv-adapter-core` 进程，监听：

```text
127.0.0.1:50052
0.0.0.0:8077
0.0.0.0:8088
```

Trajectory 服务健康检查：

```json
{"data_dir":"/home/uenv/trajectory-data","db":"ok"}
```

### 3.2 AgentControlService

`GET http://8.130.75.157:50052/agents` 返回成功。当前 OpenHands pool：

```text
agent_pool_id: openhands-default
total_capacity: 4
total_load: 0
pending_jobs: 0
running_jobs: 0
```

当前有 `openhands-20877-main` 和三个 autoscaled OpenHands Agent 实例处于非 stale 状态。历史 stale 注册记录仍保留在查询结果中，但不占用当前容量，Server 已执行 stale job 回收。

### 3.3 OpenHands 208.77

208.77 核验结果：

```text
uenv-agent-poller.service: active
uenv-gateway-tunnel.service: active
:8777: LISTEN
:8888: LISTEN
127.0.0.1:28097: SSH tunnel LISTEN
```

Runner 健康检查返回：

```json
{
  "agent_id": "openhands-20877-main",
  "agent_pool_id": "openhands-default",
  "poll_enabled": true,
  "registered": true,
  "server_endpoint": "8.130.75.157:8088",
  "status": "ok"
}
```

## 4. OpenHands + SWE 端到端验收

在 208.77 执行：

```text
scripts/verify-openhands-trajectory-e2e-20877.sh
```

预检结果：

```text
runner_ok
server_trj_ok
```

gold run 结果：

```json
{
  "resolved": true,
  "reward": 1.0,
  "tests_passed": 56,
  "tests_total": 56,
  "run_id": "run-oh-20260811-112339-pro-gold",
  "trajectory_id": "trj-worker-7143-pro-1786418629274-00001",
  "upload_status": "pending",
  "server_verified": true
}
```

该结果覆盖以下实际链路：

```text
OpenHands Agent
  -> Agent poller
  -> Server AgentControlService
  -> AgentJob
  -> OpenHands runner
  -> SSH tunnel 127.0.0.1:28097
  -> 7143 Runtime Gateway
  -> SWE Worker
  -> SWE container / tests / reward
  -> trajectory upload
  -> Server verification
```

## 5. OlymMATH HTTP/2 CANCEL 复核

### 5.1 日志时间判断

Server 日志文件核验结果：

```text
/var/log/uenv/adapter-core.log
last modified: 2026-07-15 22:11:08 +0800
```

当前核验时间为 2026-08-11。日志中的 OlymMATH CANCEL 记录集中在 2026-07-15，例如：

```text
2026-07-15T14:00:40Z ... OlymMATH-EASY-49-ZH ... h2 protocol error ... CANCEL
2026-07-15T14:07:24Z ... OlymMATH-EASY-52-ZH ... h2 protocol error ... CANCEL
2026-07-15T14:10:03Z ... OlymMATH-EASY-52-ZH ... h2 protocol error ... CANCEL
```

这些记录属于旧批次 `olymmath-uenv-20260714_223910`，不是当前日期产生的实时日志。当前 `uenv-adapter-core` 进程自 2026-08-06 起运行，旧日志没有继续追加。

### 5.2 当前是否仍存在

本次通过以下现象判断当前问题没有继续发生：

- Server 日志文件自 2026-07-15 后没有新的 OlymMATH 记录。
- 7143 Worker 在 2026-08-11 持续发送 heartbeat。
- Worker 当前没有新的 `image ... not present locally` 错误。
- OpenHands + SWE gold 端到端测试完成并由 Server 验证成功。
- Agent pool 当前无 pending、running 或 outstanding job。

因此，旧 OlymMATH `HTTP/2 CANCEL` 记录可以认定为历史告警，不是当前仍在持续的故障。

### 5.3 仍需保留的工程风险

历史记录所揭露的根因曾与长耗时 Math episode 的 gRPC/HTTP2 transport 稳定性有关，仓库已有相关修复和长 episode keepalive 配置。由于当前没有新的 OlymMATH 训练批次可复现，本文不宣称完成一次新的 OlymMATH 压测回归；若重新运行长耗时 OlymMATH 全量批次，仍应关注：

```text
dispatch_failed
execute_episode_failed
h2 protocol error
CANCEL
```

当前结论是“旧错误未继续发生”，不是“通过新一轮 OlymMATH 全量压测证明永久不会复现”。

## 6. 代码与测试

本次提交包含 SWE-smith 官方 namespace 的活动代码、catalog 构建逻辑、测试 fixture 和 Worker namespace 校验调整。

本地通过：

```text
python3 -m unittest tests.test_swe_catalog_tool
5 tests passed
git diff --check
```

本地 `cargo test` 未执行成功，原因是开发机缺少 `protoc`，不是 Rust 测试断言失败：

```text
Could not find `protoc`
```
