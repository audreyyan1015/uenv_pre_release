# VeRL 强化学习接入

本页介绍 UEnv 对 VeRL 的已有适配。如果你只想运行训练，请先阅读[强化学习训练指南](../3-运行任务/07-post-training.md)；如果你正在维护 VeRL 适配代码，再按本页核对接入点、字段映射和配置。

UEnv 当前固定支持 **VeRL v0.7.1**。通用的 UEnv 请求和结果字段见[接口与数据契约](./02-contract.md)；批次、重试、超时、背压和异步策略见[生产运行语义](./06-runtime-semantics.md)。本页只说 VeRL 特有的映射。

## VeRL 的接入点

VeRL 在 AgentLoop 中已选定训练 sample、但还没有在本地执行 rollout 时，调用 `UEnvAgentLoop`。这个 hook 将 sample 交给 UEnv：UEnv Worker 调用当前策略模型、与环境交互并计算 reward，然后适配代码把结果还原为 VeRL 的 `AgentLoopOutput`。

| 作用 | 实现位置 |
|---|---|
| VeRL hook 与双向映射 | `uenv-bridge/src/uenv/bridge/verl_agent_loop.py` 中的 `UEnvAgentLoop` |
| AgentLoop 配置 | `uenv-bridge/configs/uenv-agent-loop.yaml` |
| `AdapterCoreService` 客户端 | `uenv-bridge/src/uenv/bridge/clients.py` |
| 映射测试 | `uenv-bridge/tests/test_verl_agent_loop.py` |

`UEnvAgentLoop` 内部先构造 Python `EpisodeRequest`，客户端再将它编码成公开协议的 `SampleEnvelope`。这是两层实现细节；修改 VeRL 适配时，最终仍应核对 `SampleEnvelope` 与 `SampleResult` 的公开契约。

## 从 VeRL sample 到 `SampleEnvelope`

`UEnvAgentLoop` 从 `raw_prompt`、sampling parameters 和 sample metadata 取值。新的训练数据至少应显式提供 `extra_info.env_type` 和 `extra_info.dataset`；不要依赖文件名、数据集名或全局默认值猜测环境。

| VeRL 中的来源 | `SampleEnvelope` 字段 | 说明 |
|---|---|---|
| 适配代码为每次 rollout 生成的 UUID | `request_id` | 每个 rollout 唯一；同一次传输重试保持不变 |
| `extra_info.batch_id` 或 batch hook 传入的 ID | `batch_id` | 关联同一 VeRL 批次 |
| sample 在批次中的位置 | `sample_index` | 用于乱序结果还原 |
| 固定值 `verl` | `framework` | 标识请求来源 |
| `extra_info.env_type` | `env_type` | 选择 UEnv 环境 |
| AgentLoop 的 `parallel_mode` | `parallel_mode` | 记录当前执行模式 |
| `raw_prompt`、`task_name`、`data_source`、`extra_info.dataset` 和 `extra_info.env_config` | `env_config_json` | 环境输入；SWE 还需要 `instance_id` 和 `benchmark_variant` |
| `max_steps`、`seed` | `episode_config_json` | 当前 UEnv Server 实际读取的通用字段；其他键不会自动参与环境执行 |
| `extra_info.reward_config` 或 `reward_model` | `reward_config_json` | 环境计分配置；SWE 检查问题是否已解决 |
| 运行时 model gateway 或显式模型 URL/名称 | `model_endpoint` | URL 必须能被 UEnv Worker 访问 |
| AgentLoop timeout 与 batch/sample metadata | `timeout_seconds`、`correlation_id`、`sample_context_json` | 用于超时和跨组件追踪 |

`prompt_ids` 是 **VeRL 输入的保留字段**，不是 UEnv 的输出。VeRL 适配代码把它保存在本地 Python 请求的 `initial_observation` 中，生成 `AgentLoopOutput` 时再从同一请求取回；当前 UEnv Server 不读取这个字段。不要把“Bridge 本地保留”误解为“UEnv 返回 prompt token”。

## 从 `SampleResult` 到 `AgentLoopOutput`

UEnv 结果可以乱序到达。VeRL 适配代码先按 `request_id` 恢复请求顺序，再为每条 sample 生成 `AgentLoopOutput`。

| `AgentLoopOutput` 字段 | 数据来源 |
|---|---|
| `prompt_ids` | 原 VeRL sample 经 tokenizer 生成并随请求保留的 token；**不是 `SampleResult` 字段** |
| `response_ids` | `trajectory_json.steps[*].rollout_trace.response_ids`；非 SWE 兼容路径可对 response text 重新编码 |
| `response_mask` | `trajectory_json.steps[*].rollout_trace.response_mask`；缺失时现有兼容路径按 response 长度补 1 |
| `response_logprobs` | `SampleResult.rollout_log_probs`，并按 response token 数量校验；结果缺失时默认为 `None` |
| `reward_score` | `SampleResult.reward` |
| `num_turns` | trajectory step 数转成 VeRL 需要的 turn 计数 |
| `extra_fields.uenv_*` | `request_id`、status、termination reason、trajectory ID/body 和 response 来源 |
| `extra_fields.global_steps` 等 | rollout policy/parameter version 与请求中的训练 step |

SWE 训练默认必须收到类型化 response trace；缺失 token trace 时直接失败，不使用文本重新 tokenization 的结果训练。

## 配置 VeRL AgentLoop

普通训练用户应由 `uenv train run-task` 或 `uenv train run-swe` 准备配置。只有调试 hook 或适配代码时，才需要手工指定 AgentLoop：

```bash
export UENV_REPO_ROOT="$PWD"
export UENV_AGENT_LOOP_CONFIG="$UENV_REPO_ROOT/uenv-bridge/configs/uenv-agent-loop.yaml"
export UENV_AGENT_LOOP_CLIENT='rust_core'
export UENV_ADAPTER_CORE_ENDPOINT='127.0.0.1:50051'
```

VeRL 配置中启用注册名 `uenv_agent`：

```text
actor_rollout_ref.rollout.agent.default_agent_loop=uenv_agent
actor_rollout_ref.rollout.agent.agent_loop_config_path=${oc.env:UENV_AGENT_LOOP_CONFIG}
```

常用适配配置如下：

| 配置 | 默认值 | 什么时候修改 |
|---|---:|---|
| `UENV_ADAPTER_CORE_ENDPOINT` | `127.0.0.1:50051` | UEnv Server 不在同一主机时，改为实际 gRPC 地址 |
| `UENV_AGENT_LOOP_TIMEOUT_SECONDS` | `300` | Episode 的正常执行时间超过 300 秒时 |
| `UENV_ROLLOUT_MODEL_ENDPOINT` / `UENV_ROLLOUT_MODEL_NAME` | 空 | 不启用本地 model gateway、而是直接调用 Worker 可达的模型服务时 |
| `UENV_MODEL_GATEWAY_ENABLED` | `false` | 当前策略模型位于 VeRL GPU 主机、需要暴露给 Worker 时设为 `true` |
| `UENV_MODEL_GATEWAY_PUBLIC_URL` | 空 | 启用 gateway 时，设为 Worker 实际可访问的 URL，不能对远程 Worker 使用 `127.0.0.1` |
| `UENV_REQUIRE_SWE_RESPONSE_TRACE` | `true` | 建议保持；它防止 SWE 在缺失原始 token trace 时继续训练 |
| `UENV_MISSING_LOGPROBS_AS_ZERO` | `false` | 仅在明确需要兼容缺失 logprob 的旧流程时改为 `true` |
| `UENV_AGENT_LOOP_FAILED_EPISODE_POLICY` | `raise` | 仅当任务定义明确允许失败占位时才改为 `zero_reward` |
| `UENV_AGENT_LOOP_BATCH_SIZE` | `0` | 需要将 VeRL batch 进一步分块时设置；`0` 表示不额外分块 |

`UENV_DEFAULT_ENV_TYPE` 应保持为空。每条新 sample 显式填写 `extra_info.env_type`，可以避免错误数据被静默路由到其他环境。

## 已知限制

- 当前发布只固定 VeRL v0.7.1，其他 VeRL 版本需要重新验证 AgentLoop API。
- 普通 QA/code 训练目前是单步 Episode；SWE 训练目前只有 Smith 发布入口。
- 非 SWE 任务在缺少类型化 response token 时仍保留文本重新 tokenization 兼容路径；该结果可能与原 rollout token 不完全相同。
- `recorded` 仍被作为历史状态兼容；新实现应以 `completed` 为正常成功状态。
- 当前 VeRL 适配使用同一个 timeout 配置构造 Episode 超时和 gRPC 等待时间；需要分别控制两者时，应先扩展适配配置并重新验收。
- UEnv 负责 rollout 和 reward；VeRL 仍负责 logprob/advantage 使用、参数更新和 checkpoint。

## 测试 VeRL 映射

在仓库根目录运行：

```bash
python -m pytest -q uenv-bridge/tests/test_verl_agent_loop.py
```

该测试应至少覆盖：

1. `raw_prompt`、`env_type`、dataset、reward 和模型信息能正确写入 `SampleEnvelope`。
2. `prompt_ids` 保留自 VeRL 请求，`response_ids` / `response_mask` / reward 来自对应的 UEnv 结果。
3. 乱序结果可按 `request_id` 恢复，重复、缺失或未知 ID 会失败。
4. SWE 缺少 response trace 时拒绝训练，失败 Episode 符合配置的失败策略。

将 VeRL 版本保持为“支持”还需要完成真实 UEnv Server/Worker 闭环和 VeRL 训练作业验收，统一通过标准见[支持状态与接入验收](./05-support-matrix.md)。
