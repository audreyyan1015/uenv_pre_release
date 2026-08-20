# VeRL 强化学习接入

本页面向维护 VeRL 接入实现的开发者。普通训练用户使用 `uenv train run-task` 或 `uenv train run-swe`，完整流程见[强化学习训练指南](../3-运行任务/07-post-training.md)。

当前发布固定 VeRL v0.7.1。接入点是 AgentLoop 的 pre-rollout 阶段：VeRL 提供 prompt/sample，UEnv Worker 负责模型生成、环境交互和 reward，接入实现返回 `AgentLoopOutput` 供 VeRL 计算 logprob、advantage 与模型更新。

## 实现映射

| VeRL / UEnv 项 | 仓库实现 |
|---|---|
| VeRL hook | `uenv-bridge/src/uenv/bridge/verl_agent_loop.py` 中的 `UEnvAgentLoop` |
| 接入配置 | `uenv-bridge/configs/uenv-agent-loop.yaml` |
| gRPC 客户端 | `uenv-bridge/src/uenv/bridge/clients.py` |
| 协议 | `proto/uenv/v1/adapter_core.proto` |
| UEnv Server 兼容二进制 | `uenv-adapter-core` |
| 环境执行 | UEnv Server 调度 UEnv Worker |

`uenv-bridge/core/` 是历史源码布局；当前运行时由接入实现通过 gRPC 连接独立 UEnv Server。

## 用户入口与开发入口

用户由 runner 准备固定 VeRL、AgentLoop 配置、数据转换和容器参数：

```text
uenv train run-task ...
uenv train run-swe ...
```

只有调试接入实现或修改 VeRL hook 时才手工启用 AgentLoop。在源码仓库根目录设置真实路径：

```bash
export UENV_REPO_ROOT="$PWD"
export UENV_AGENT_LOOP_CONFIG="$UENV_REPO_ROOT/uenv-bridge/configs/uenv-agent-loop.yaml"
export UENV_AGENT_LOOP_CLIENT='rust_core'
export UENV_ADAPTER_CORE_ENDPOINT='127.0.0.1:50051'

test -r "$UENV_AGENT_LOOP_CONFIG"
```

VeRL 配置引用：

```text
actor_rollout_ref.rollout.agent.default_agent_loop=uenv_agent
actor_rollout_ref.rollout.agent.agent_loop_config_path=${oc.env:UENV_AGENT_LOOP_CONFIG}
```

`UENV_ADAPTER_CORE_ENDPOINT` 是兼容变量名，值是 UEnv Server gRPC 地址。直接开发时这些变量由启动 VeRL 的 shell/container 读取，不写入 UEnv Server 的 systemd 环境。

不要设置全局 `UENV_DEFAULT_ENV_TYPE=qa`。每条训练 sample 必须显式提供 `extra_info.env_type` 与 dataset；通用默认值会把错误数据静默路由到错误环境。

## 训练模型位于远程 GPU 主机

UEnv Worker 必须能访问当前策略 model gateway：

```bash
export UENV_MODEL_GATEWAY_ENABLED=true
export UENV_MODEL_GATEWAY_BIND_HOST='10.0.0.30'
export UENV_MODEL_GATEWAY_PORT=18080
export UENV_MODEL_GATEWAY_PUBLIC_URL='http://10.0.0.30:18080/v1'
```

`BIND_HOST` 使用 GPU 主机的可路由接口地址，`PUBLIC_URL` 使用 UEnv Worker 实际能访问的地址。只向 UEnv Worker 网段开放 18080/TCP；远程 UEnv Worker 不能使用 `127.0.0.1`。

## sample 到 Episode

`UEnvAgentLoop` 从 VeRL sample 读取：

- `raw_prompt` 或 prompt tokens；
- `data_source`、`extra_info.env_type`、`extra_info.dataset`；
- ground truth / reward model；
- sampling parameters、seed 与 max steps；
- run、batch、sample 和策略版本关联信息。

接入代码构造类型化请求：环境字段进入 `env_config_json`，步数与 seed 进入 `episode_config_json`，判分进入 `reward_config_json`，当前策略 API 进入 `model_endpoint`。每个 rollout 生成唯一 `request_id`，同一传输重试复用该 ID。

## Episode 到 AgentLoopOutput

| UEnv 结果 | VeRL 输出 |
|---|---|
| prompt token | `prompt_ids` |
| response token | `response_ids` |
| token 训练掩码 | `response_mask` |
| rollout token logprob | `response_logprobs` |
| 总 reward | `reward_score` |
| trajectory step 数 | `num_turns` |
| 状态、终止原因、轨迹 ID/body | `extra_fields.uenv_*` |

接入实现优先读取每个 step 的类型化 response trace。只有显式兼容模式才能用 tokenizer 对最终文本重新编码，并必须记录 token 来源。SWE 训练要求完整 response trace；缺失时失败，不能制造不可验证 token。

验收时至少检查：

1. `response_ids` 与 `response_mask` 等长且存在有效 mask。
2. token logprob 与对应 token 对齐。
3. reward 来源为环境结果本身。
4. `request_id` 能关联 VeRL、UEnv Server 与 UEnv Worker 日志。
5. rollout policy/version 满足训练的 staleness 约束。

## 批次、失败与并发

接入实现为每个 sample 生成唯一 ID、共享 batch ID，并在 UEnv Server 乱序返回时按 ID 重排。重复、未知或缺失结果立即报协议错误。

| 配置 | 作用 |
|---|---|
| `UENV_AGENT_LOOP_BATCH_SIZE` | 每个提交 chunk 的样本数；0 表示不额外切分 |
| `UENV_AGENT_LOOP_BATCH_RETRY_ATTEMPTS` | 传输重试尝试数 |
| `UENV_AGENT_LOOP_BATCH_RETRY_DELAY_SECONDS` | 重试退避 |
| `UENV_AGENT_LOOP_FAILED_EPISODE_POLICY` | `raise` 或 `zero_reward` |
| `UENV_MAX_EPISODE_CONCURRENCY` | 本次 run 的 Episode 并发 hint |
| `UENV_MAX_IN_FLIGHT_BATCHES` | run 级积压批次 hint |

默认使用 `raise`。只有任务定义明确允许时使用 `zero_reward`，同时保存原始错误并把失败 response 标为不可训练。并发 hint 不能绕过 UEnv Worker 硬容量。

## Hydra 覆盖的维护位置

可版本管理的覆盖项写入独立文件，每行一个 `KEY=VALUE`。仓库示例为：

```text
examples/cases/training/verl-grpo-overrides.conf
```

runner 的 `--verl-config FILE` 加载该文件，可重复的 `--set KEY=VALUE` 提供更高优先级覆盖。调试最终列表时使用 `--print-effective-config`；它只打印配置并退出，不执行训练。通用训练流程不重复维护 Hydra 细节。

## 开发验收

在仓库根目录执行映射测试：

```bash
python -m pytest -q uenv-bridge/tests/test_verl_agent_loop.py
```

然后依次完成：

1. UEnv Server 协议/批次测试。
2. 接入实现连接真实 UEnv Server/Worker，验证 ID、token、reward 与 trajectory 闭环。
3. 真实 VeRL 作业完成计划更新并写出指标/checkpoint。

只有三步与映射测试全部通过，支持矩阵才能保持“支持”。案例入口见[强化学习训练案例](../3-运行任务/02-cases.md#强化学习训练)。
