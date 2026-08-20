# UEnv Bridge

UEnv Bridge 是评测程序或训练框架一侧的适配层。当前主线实现把 VeRL v0.7.1 的 sample 在 pre-rollout 阶段交给 UEnv，并把 response token、mask、reward 和 trajectory 还原成 `AgentLoopOutput`。

## 组件边界

```text
VeRL → UEnv Bridge → UEnv Server → UEnv Worker
```

- Bridge：本目录的 Python 包 `src/uenv/bridge/`。
- Server：中心服务，接收 Bridge 请求并选择 Worker。
- Worker：执行模型生成、环境 step 和 reward。

本页面向 Bridge 维护者。`core/`、`AdapterCoreService` 和 `UENV_ADAPTER_CORE_ENDPOINT` 是现有源码与协议名称；普通用户只需把它们理解为 UEnv Server 的实现入口或连接地址。

## 主流程

```text
VeRL AgentLoop sample
  -> UEnvAgentLoop
  -> SampleEnvelope / EpisodeRequest
  -> Server 调度 Worker
  -> EpisodeResult / SampleResult
  -> AgentLoopOutput
  -> VeRL logprob / advantage / update
```

Bridge 在 Server 乱序返回时按 `request_id` 对齐结果。未知、重复或缺失 ID 都是协议错误。

## 代码结构

| 路径 | 作用 |
|---|---|
| `src/uenv/bridge/verl_agent_loop.py` | VeRL pre-rollout hook 与字段映射 |
| `src/uenv/bridge/clients.py` | Bridge 到 Server 的 gRPC 客户端 |
| `src/uenv/bridge/protocol.py` | Python 内部 Episode 类型 |
| `src/uenv/bridge/model_gateway.py` | 向 Worker 暴露当前训练模型 |
| `configs/uenv-agent-loop.yaml` | VeRL AgentLoop 配置 |
| `core/` | Server 可执行程序的 Rust 源码（现有代码名 adapter-core） |

## VeRL 配置

```text
actor_rollout_ref.rollout.agent.default_agent_loop=uenv_agent
actor_rollout_ref.rollout.agent.agent_loop_config_path=/path/uenv-bridge/configs/uenv-agent-loop.yaml
```

正式部署的最小连接配置：

```bash
export UENV_AGENT_LOOP_CLIENT=rust_core
export UENV_ADAPTER_CORE_ENDPOINT='SERVER_HOST:50051'
export UENV_ADAPTER_CORE_AUTO_START=false
```

每条训练数据都应显式提供 `env_type`。不要用全局默认值掩盖数据缺失。

GPU 主机和 Worker 分开时，还需要向 Worker 暴露当前策略模型：

```bash
export UENV_MODEL_GATEWAY_ENABLED=true
export UENV_MODEL_GATEWAY_BIND_HOST='GPU_HOST'
export UENV_MODEL_GATEWAY_PORT=18080
export UENV_MODEL_GATEWAY_PUBLIC_URL='http://GPU_HOST:18080/v1'
```

`PUBLIC_URL` 必须从 Worker 可达。密钥不进入 sample payload、请求记录或轨迹。

## 结果要求

训练结果至少提供：

| UEnv 字段 | VeRL 用途 |
|---|---|
| `request_id` | 回填原 sample |
| terminal `status` | 判断是否可进入训练 |
| response IDs / mask | 构造训练 token 与 loss mask |
| reward | `AgentLoopOutput.reward_score` |
| trajectory / trajectory ID | 调试与可追溯 |
| policy version / logprobs（异步时） | 新鲜度和 off-policy 处理 |

Bridge 优先使用 Worker 返回的原始 token。只有通用兼容路径可以从 response text 重新编码；SWE 训练默认要求完整 response trace。

## 用户入口

用户通过 release 命令运行，不直接调用仓库内部脚本：

```text
uenv train run-task ...
uenv train run-swe ...
```

完整文档：

- [Bridge 通用契约](../Docs/guide/integration/contract.md)
- [VeRL 接入指南](../Docs/guide/integration/verl.md)
- [强化学习训练指南](../Docs/guide/usage/post-training.md)
- [强化学习训练案例](../Docs/guide/cases/README.md#强化学习训练)

## 开发验证

修改 Bridge 或 Server 映射后，验证以下边界：

1. `tests/test_verl_agent_loop.py` 的 sample/request 和 result/output 映射。
2. `cargo test -p uenv-adapter-core` 的批次、ID 与 Episode 转换。
3. Bridge 连接真实 Server/Worker 后的 response、reward、trajectory 闭环。
4. 目标 VeRL 作业完成计划的模型更新，并写出指标和 checkpoint。

不要使用 `fake` client 作为真实接入验收；它只用于孤立的 Python 单元测试。
