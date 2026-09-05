# 原生 VeRL SWE AgentLoop 远端 OpenHands 执行说明

## 1. 背景

为了构造 “原生 VeRL + SWE/OpenHands AgentLoop” 对比组，训练进程不经过 UEnv Adapter Core / Server 提交 episode，但仍需要远端 OpenHands 执行环境。为了避免和 UEnv 的通信形态产生额外差异，正式对比路径不采用 per-episode SSH，而是采用和 UEnv 一致的长驻 agent 模式：

```text
VeRL native_swe_agent
  -> 本地轻量 AgentControl gRPC 服务
  <- 远端 OpenHands runner 注册、轮询、回填
  -> Worker runtime gateway 执行 SWE 环境
```

这样，原生 VeRL 对比组只绕过 UEnv Adapter Core / Server 的 episode 调度链路，不额外引入 “每条 episode 临时 ssh 一次” 的执行差异。

## 2. 本次新增能力

本地 `native_swe_agent` 新增 `grpc` 执行后端：

- VeRL 侧在训练进程内启动轻量 `AgentControlService`。
- 每条 SWE episode 被转换为 `AgentJob` 后放入本地队列。
- 远端 OpenHands runner 长驻运行，通过 `RegisterAgent` / `PollAgentJob` / `CompleteAgentJob` 与 VeRL 侧通信。
- runner 回填 `reward`、`trajectory_id`、`response_ids`、`response_mask`、`rollout_log_probs` 等训练消费字段。
- `ssh` 后端仍保留，但只作为单条 smoke/fallback，不作为正式对比实验路径。

## 3. 远端机器处理

agent 机器：`8.130.208.77`

本次没有覆盖 `/root/UEnv` 下的既有文件，只新增了最终使用的 integration bundle：

```text
/root/uenv-native-swe-agentloop-20260823_231433/integrations/openhands
```

该目录由本地当前 `integrations/openhands` 拷贝而来，并包含兼容远端旧版 `protobuf/grpcio` 的 Python gRPC stubs。

新增远端启动脚本：

```text
/root/uenv-native-swe-agentloop-20260823_231433/start-native-agent-poller.sh
```

该脚本复用既有 runner 和 wrapper：

```text
/root/UEnv/scripts/openhands/openhands_runner.py
/root/UEnv/scripts/run-openhands-pro-20877.sh
```

## 4. 网络连接方式

当前 208.77 没有到训练机 `10.10.20.142` 网段的路由，实测无法直连 `10.10.20.142:19051`。同时，远端 OpenHands runner 也需要访问训练侧 model gateway，因此正式运行时需要从训练机同时建立两个反向 SSH 隧道：

```bash
SSHPASS=dev@BDW2026 sshpass -e ssh \
  -N \
  -o ExitOnForwardFailure=yes \
  -R 19051:127.0.0.1:19051 \
  -R 18088:127.0.0.1:18088 \
  root@8.130.208.77
```

然后在 208.77 上启动 poller：

```bash
UENV_SERVER_ENDPOINT=127.0.0.1:19051 \
  /root/uenv-native-swe-agentloop-20260823_231433/start-native-agent-poller.sh
```

训练侧 `native_swe_agent` 默认在本机 `19051` 端口启动轻量 AgentControl 服务，并在 `18088` 端口启动 adapter model gateway。远端通过隧道访问：

```text
127.0.0.1:19051 -> 训练机 AgentControl
127.0.0.1:18088 -> 训练机 model gateway
```

因此 native preset 默认把 `UENV_MODEL_GATEWAY_PUBLIC_URL` 设置为 `http://127.0.0.1:18088/v1`。这个地址是从 208.77 视角看的地址，不是训练机本地服务的真实绑定地址。

## 5. 本地代码改动范围

- `uenv-bridge/src/uenv/bridge/native_agent_control_server.py`
  - 新增轻量 `AgentControlService`，支持 register / heartbeat / poll / complete。
  - 支持把 native `AgentJob` dict 转成 UEnv `AgentJob` proto。

- `uenv-bridge/src/uenv/bridge/native_swe_agent_loop.py`
  - 新增 `grpc` 后端，把 episode 放入本地 AgentControl 队列并等待远端回填。
  - 保留 `ssh` / `local` 后端作为调试路径。

- `uenv-bridge/configs/native-swe-agent-loop.yaml`
  - 增加 native AgentControl host / port / public endpoint 配置。

- `uenv-bridge/scripts/train/launchers/swe/native/swe_smith_native_verl_grpo_train.sh`
  - 默认切到 `NATIVE_SWE_EXECUTION_BACKEND=grpc`。
  - 默认指向最终远端 bundle。

- `uenv-bridge/src/uenv/v1/*_pb2.py` 与 `integrations/openhands/uenv_runtime/gen/uenv/v1/*_pb2.py`
  - 移除生成代码里的 protobuf runtime 硬版本检查。
  - 移除 `agent_pb2_grpc.py` 中新 grpc 版本才支持的 `_registered_method` 参数。
  - 仅为兼容当前训练镜像与 208.77 运行环境，不改变 proto schema。

## 6. 验证结果

已完成以下验证：

- 本地 Python 环境可导入 `uenv.v1.agent_pb2` 和 `NativeAgentControlServer`。
- VeRL 训练镜像内可导入 `NativeAgentControlServer` 并构造 `AgentJob`。
- 单测覆盖 `RegisterAgent`、`PollAgentJob`、`CompleteAgentJob`、unknown job ack=false。
- 208.77 新 bundle 可在系统 Python 下导入 `AgentControlClient`。
- 208.77 poller 通过反向 SSH 隧道可成功注册到本地轻量 AgentControl 服务：

```text
[agent-poll] registered agent_id=native-openhands-20877 pool=openhands-default max_concurrent=1
```

## 7. 注意事项

正式训练前需要先保持反向 SSH 隧道和远端 poller 常驻，并确认 Worker runtime gateway 可访问。否则 VeRL 侧会启动 AgentControl 服务并等待远端领取任务，或 OpenHands 无法连接环境 / 模型，最终 episode 会失败或超时。

该方案仍使用 Worker runtime gateway 执行 SWE 沙箱交互与判分，因此它适合对比 “VeRL 是否经过 UEnv Adapter Core / Server 调度链路”，不适合作为完全不依赖 UEnv Worker 的环境实现对照组。
