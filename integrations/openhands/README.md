# UEnv 与 OpenHands

本目录实现 OpenHands SWE Agent 到 UEnv Worker Runtime Gateway 的适配。Agent 的 shell、文件读取和文件写入在 UEnv 管理的 SWE 实例容器中执行；submit 返回测试、reward、patch 和 trajectory。

## 运行关系

```text
OpenHands Agent -> UEnvRuntime Bridge -> Worker Runtime Gateway -> SWE instance
```

批量评测和训练仍由 UEnv Server 管理任务与 Agent job。Runtime Gateway 只负责 SWE 实例中的操作，不负责 Worker 注册或任务调度。

## 实现边界

当前 `UEnvRuntime` 是独立实现，不 import 或 subclass OpenHands 的经典 `Runtime`：

- action 通过 `.command`、`.path`、`.content` 或同名 dict 字段 duck type；
- observation 以 OpenHands 形态的普通 dict 返回；
- Runtime Gateway HTTP 契约不绑定某个 OpenHands Python 包结构；
- benchmarks 与 SDK 使用 `PIN.md` 中的固定 commit，不使用浮动分支。

这支持已验证的 UEnv SWE 执行链路，但不表示对任意 OpenHands 版本都可作为 drop-in Runtime。

## 目录

| 路径 | 作用 |
|---|---|
| `uenv_runtime/client.py` | session、exec/read/write、submit、trajectory HTTP 客户端 |
| `uenv_runtime/runtime.py` | action/observation 适配 |
| `uenv_runtime/agent_client.py` | Adapter 内部 AgentControlService 客户端 |
| `run_swebench*.py` | release `run-swe` 使用的执行器与开发驱动 |
| `tests/` | action、client、trace 和 workspace 测试 |
| `PIN.md` | 固定的 OpenHands 仓库 commit |

## 用户入口

最终用户使用：

```text
sudo uenv evaluate run-swe ...
uenv train run-swe ...
```

这些命令要求显式提供模型/provider、Gateway、catalog、variant、实例、输出与并发。仓库脚本属于内部实现和开发诊断入口，不应另写一套生产流程。

文档：

- [SWE-bench Verified 评测案例](../../Docs/guide/cases/evaluation-swe-verified.md)
- [轨迹采集与查询](../../Docs/guide/usage/trajectory.md)

## 开发驱动

需要直接验证 Runtime action 与 grader 契约时，可以对固定 catalog 实例执行：

```bash
python3 integrations/openhands/run_swebench.py \
  --gateway '127.0.0.1:28999' \
  --instance 'astropy__astropy-7166' \
  --instances fixtures/swe/swe_instances.json \
  --benchmark-variant verified \
  --save-ref "$PWD/trajectory_ref.json" \
  --fetch-trajectory
```

该开发驱动默认应用 catalog 中的 gold patch，用于验证 create/action/submit/trajectory 契约，不用于报告模型能力。模型评测必须走 `uenv evaluate run-swe`，由实际 Agent 调用目标模型。

## 接入要求

- catalog、Gateway 实例和 variant 必须一致。
- 每个 Agent job 使用独立 session/workspace，完成、超时或取消后释放容器。
- Gateway API Key 通过受保护配置传入，不写入日志或轨迹。
- complete 必须回填原 `job_id` / `episode_id` 和 `trajectory_id`。
- 并发受 Agent poller、Gateway session、容器和模型服务共同限制。

修改后应通过 `python3 -m pytest integrations/openhands/tests -q`，并用真实模型、Server 和 Worker 完成目标 SWE 作业，确认没有 workspace/session/result 串线。
