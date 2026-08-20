# 以 VeRL 为例接入 UEnv

UEnv 已经提供 VeRL 适配器 `UEnvAgentLoop`。使用当前发布版的同步入口时，不需要修改 VeRL 源码，可以直接运行 `uenv train run-task`。

为了说明这层适配如何实现，本页仍按照[自定义强化学习框架接入](./01-custom-framework.md)中的顺序展开：先确定 rollout 接入位置，再把 VeRL sample 转成 UEnv 请求、调用 UEnv，最后把结果转回 VeRL。同步与异步训练使用相同的两次数据转换，区别在于 VeRL 如何调度 rollout 和模型更新，以及调用 UEnv 时如何接收结果。

## VeRL 中的接入位置

自定义框架应在“sample 已经确定，本地 rollout 尚未开始”的位置调用 UEnv。VeRL 中的这个位置是 AgentLoop。UEnv 将 `uenv_agent` 注册为 VeRL AgentLoop，由 `UEnvAgentLoop` 在模型生成前接管 sample：

`VeRL sample → UEnvAgentLoop → UEnv rollout → AgentLoopOutput → advantage / loss / update`

`UEnvAgentLoop` 只替换 rollout。VeRL 仍然负责加载数据、计算 advantage 和 loss、更新模型及保存 checkpoint。适配器把当前策略模型的 OpenAI-compatible 地址写入 `model_endpoint`；公开同步入口会通过 Model Gateway 将这个地址暴露给 UEnv Worker。

`uenv train run-task` 会自动配置 VeRL 使用 `uenv_agent`，使用者不需要自己注册 AgentLoop。

## 同步训练：按照三个步骤接入

当前公开 release 使用同步训练流程。下面三步分别对应自定义框架文档中的请求转换、UEnv 调用和结果转换。

### 1. 将 VeRL sample 转成 UEnv 请求

`uenv train run-task` 接受 Episode JSONL，并先将它转换成 VeRL 可读取的数据。最小输入如下：

```json
{"id":"example-1","env_type":"qa","dataset":"gsm8k","question":"1+1=? End with `#### number`.","target":"2","max_steps":1}
```

其中，`question` 成为 VeRL prompt，`target` 成为评分目标；`env_type`、`dataset`、`max_steps` 以及可选的 `env_config`、`reward_config` 会保存在 sample 的附加信息中。

VeRL 将 sample 交给 `UEnvAgentLoop` 后，适配器完成与 `to_uenv_request()` 相同的工作：

- 保留 VeRL 已生成的 prompt token。
- 将 prompt 和 sample 附加信息整理为 `env_config`、`episode_config` 和 `reward_config`。
- 将 VeRL 的采样参数整理为 UEnv 模型生成参数。
- 找到当前策略模型的服务地址，写入 `model_endpoint`。
- 为每次 rollout 生成唯一的 `request_id`，并记录 `batch_id` 和 `sample_index`。

输入 JSONL 中的 `id` 是业务样本 ID；`request_id` 是每次 rollout 的传输 ID。同一条样本生成多条 rollout 时，它们会有不同的 `request_id`。适配器随后将这些信息编码为 UEnv 的 `SampleEnvelope`，使用者不需要自己构造 protobuf 消息。

### 2. 调用 UEnv 执行 rollout

同步入口将 `parallel_mode` 设为 `sync`，并通过 `ExecuteBatch` 调用 UEnv。`ExecuteBatch` 会等待本次提交的所有 Episode 完成后再返回。

UEnv Worker 通过 `model_endpoint` 调用 VeRL 当前策略模型，与环境交互并计算 reward，最后返回 `SampleResult`。适配器使用 `request_id` 将结果放回原 sample；即使以后改为批量或流式返回，也不需要改动这层关联方式。

VeRL 会等本轮所需的 rollout 都转换完成后，才开始这一步的 advantage、loss 和模型更新。因此这里的“同步”描述的是 rollout 阶段与模型更新阶段的先后关系，而不只是 RPC 名称。

### 3. 将 UEnv 结果转成 VeRL 输出

`UEnvAgentLoop` 将 `SampleResult` 转成 `AgentLoopOutput`，这对应自定义框架文档中的 `to_framework_output()`：

- `prompt_ids` 直接使用提交前保存的 VeRL prompt token。
- `response_ids` 和 `response_mask` 优先从 trajectory 中的 token trace 按环境 step 顺序拼接。
- `reward` 写入 VeRL 的 `reward_score`。
- UEnv 返回的逐 token logprob 在存在时写入 `response_logprobs`。
- trajectory、终止原因和 UEnv 请求 ID 放入附加字段，供日志和问题排查使用。

适配器会把 token 与 mask 整理为一致长度，并按 `request_id` 检查结果是否重复或缺失；存在 logprob 时，还会检查它与 `response_ids` 的长度。Episode 失败时，公开入口默认立即报错，不会把失败结果静默当成普通零分样本。转换完成后，VeRL 按原有流程计算 loss 并更新模型。

## 异步训练：同样三个步骤如何变化

下面说明的是源码中的实验接入；`uenv train run-task` 当前仍只提供同步入口，公开 release 暂不提供 rollout 与模型更新解耦的训练命令。

异步接入不会改变 `UEnvAgentLoop` 的两次数据转换。变化的是请求中增加了异步上下文，VeRL 将 rollout 生产端与模型更新端分开调度，并需要根据模型版本处理已经过期的 rollout。

### 1. 输入转换增加异步上下文

`env_config`、`episode_config`、`reward_config`、`model_endpoint` 和样本 ID 与同步训练相同。适配器根据 VeRL 的调度方式设置：

- one-step off-policy 使用 `parallel_mode="one_step_off_policy"`。
- rollout 生产与模型更新持续解耦时使用 `parallel_mode="fully_async"`。

请求会携带 VeRL 当前训练步的上下文。真正生成 token 时使用的 `rollout_param_version`、`rollout_policy_version` 和逐 token logprob，必须由模型服务在同一次生成响应中返回；UEnv 不会推测这些值。

### 2. VeRL 解耦 rollout 与模型更新

one-step off-policy 由 VeRL 将下一步 rollout 与当前模型更新重叠。当前实验脚本默认仍使用 `ExecuteBatch`，这说明异步训练不要求必须使用流式 RPC。

fully async 路径由 VeRL 的 `MessageQueue` 解耦 rollout 生产端和模型更新端。UEnv 传输层在每个有限请求组内使用 `ExecuteBatchStream`，按 Episode 完成顺序返回 `SampleResult`；`UEnvAgentLoop` 收齐这一组结果后，再按 `request_id` 恢复对应关系并交回 VeRL。跨请求组的 producer 与 learner 解耦由 VeRL `MessageQueue` 完成。

`ExecuteBatchStream` 只是 UEnv 批次内的流式传输通道，不是跨批次的训练队列。仅切换 RPC 或修改 `parallel_mode`，都不会自动把 VeRL 改造成异步训练。

### 3. 输出转换增加版本信息

除了同步输出中的 token、mask、reward 和 trajectory，异步结果还必须包含实际 rollout 模型版本和生成时的逐 token logprob。UEnv 校验这些字段后，`UEnvAgentLoop` 将它们写回 VeRL 输出。

VeRL 再根据这些信息决定何时消费结果，以及接受、修正还是丢弃过旧 rollout；off-policy correction、`MessageQueue`、权重同步和消费节奏都属于 VeRL。UEnv 只负责执行 Episode，并返回生成这条 rollout 时的真实结果。

## UEnv 与 VeRL 各自负责什么

- UEnv 负责调用当前策略模型、执行环境交互、计算 reward，并返回 trajectory、token trace 及异步训练所需的 rollout 元数据。
- `UEnvAgentLoop` 负责 VeRL sample 与 UEnv 请求之间的转换，以及 UEnv 结果与 `AgentLoopOutput` 之间的转换。
- VeRL 负责 sample 调度、advantage 和 loss、模型更新、参数同步、异步队列及过旧 rollout 的处理策略。

因此，将 UEnv 接入 VeRL 不需要修改 trainer 的 loss 或更新逻辑；接入点只位于 rollout 前后。

## 直接运行现有的同步接入

### 准备运行环境

本页的示例假定 UEnv Server 和 Worker 都在 GPU 主机上，且 Worker 已安装 `qa` 环境。先检查 UEnv、GPU 和容器运行时：

```bash
uenv doctor
uenv environments
nvidia-smi -L
docker info >/dev/null
```

四条命令都应成功。如果使用 Podman，将后续命令中的 `docker` 换成 `podman`，并向 `uenv train run-task` 传入 `--runtime podman`。

### 运行发布包中的示例

将下列路径改为本机实际值。`WORK_DIR` 应使用一个尚不存在的新目录：

```bash
export UENV_RELEASE='/opt/uenv/current'
export MODEL_DIR='/data/models/Qwen2.5-3B-Instruct'
export WORK_DIR='/data/uenv-runs/qa-gsm8k'
export TRAIN_IMAGE='docker.io/verlai/verl:vllm017.latest'

test -d "$MODEL_DIR"
test -r "$UENV_RELEASE/examples/cases/training/qa-gsm8k.jsonl"
test ! -e "$WORK_DIR"
```

执行一次最小训练：

```bash
uenv train run-task \
  --model "$MODEL_DIR" \
  --work-dir "$WORK_DIR" \
  --uenv-endpoint '127.0.0.1:50051' \
  --env-type qa \
  --dataset gsm8k \
  --input "$UENV_RELEASE/examples/cases/training/qa-gsm8k.jsonl" \
  --max-steps 1 \
  --gpus 1 \
  --steps 1 \
  --rollouts 2 \
  --train-batch-size 1 \
  --runtime docker \
  --image "$TRAIN_IMAGE"
```

这个示例只用于检查 VeRL、UEnv 和模型的完整调用链。它会完成一次参数更新。runner 默认使用 `trainer.save_freq=-1`，因此不会自动保存 checkpoint；需要保存时，在命令末尾增加 `--set trainer.save_freq=1`。

### 替换为自己的数据

普通 QA 训练的 JSONL 使用[输入转换示例](#1-将-verl-sample-转成-uenv-请求)中的格式。`id` 建议在数据集内唯一，`env_type`、`dataset` 和 `max_steps` 必须与运行命令一致。只有环境确实需要额外输入或自定义奖励配置时，才增加 `env_config` 或 `reward_config`。

将这些数据保存为 JSONL 后，仅需把上一节命令的 `--input` 改为新文件。如果更换环境或数据集，同时修改 `--env-type` 和 `--dataset`。更完整的数据示例见[数学问答训练](../3-运行任务/08-training-gsm8k-verl.md)和[代码生成训练](../3-运行任务/09-training-code-verl.md)。

### 查看结果

转换后的数据位于 `$WORK_DIR/episode-data/`，VeRL 配置、指标和训练产物位于 `$WORK_DIR/.uenv-verl/output/`。运行：

```bash
test -d "$WORK_DIR/episode-data"
test -d "$WORK_DIR/.uenv-verl/output"
find "$WORK_DIR/.uenv-verl/output" -maxdepth 2 -type f -print
```

本次接入成功应同时满足：训练完成了设定的更新步数，训练样本收到了 response token、mask 和 reward，且日志中没有未处理的 UEnv Episode 失败。需要查询环境轨迹时，继续阅读[获取轨迹](../3-运行任务/12-trajectory.md)。

### UEnv 与 GPU 主机分离时

当 UEnv Server 和 GPU 训练主机不在同一台机器时，先导出并安装训练客户端，再使用 GPU 主机上的 `uenv-train run-task`。还需要打通 GPU 主机到 UEnv Server 的 50051/TCP，以及 UEnv Worker 到 GPU 主机的 18080/TCP。

完整导出、安装和运行参数见[强化学习训练指南](../3-运行任务/07-post-training.md#uenv-server-与-gpu-主机分离)。特别注意，model gateway 的公开地址必须从 UEnv Worker 访问，不能使用 GPU 主机自身的 `127.0.0.1`。

## 使用源码中的异步实验接入

源码仓库中保留了两条实验接入：

- `uenv-bridge/scripts/onestep_offpolicy/run_verl_grpo_onestep_offpolicy_uenv.sh` 展示 one-step off-policy。
- `uenv-bridge/scripts/fully_async_policy/run_verl_grpo_fully_async_uenv.sh` 展示 VeRL `MessageQueue` 与 UEnv 流式调用的 fully async 组合。

这两个脚本不随 release 打包，也不属于当前稳定入口。它们目前以 QA/GSM8K 为主要实验路径；SWE 等其他环境的 token trace、模型版本和 logprob 链路仍需单独验证。

启动异步训练前，还必须确认 VeRL 的模型服务会返回真实的 rollout 模型版本、token ID 和逐 token logprob。当前实验脚本不保证模型服务已经满足这些前置条件，因此不能把它们当作复制命令即可运行的正式入口。建议先用公开同步入口验证数据转换和 Episode 链路，再单独验证 VeRL 的队列、权重同步与过期 rollout 策略。

## 完成接入前检查

无论使用现成入口还是修改实验接入，开始正式训练前应确认：

- `UEnvAgentLoop` 在 VeRL 本地生成前接管 rollout，结果返回后 VeRL 能继续计算 loss。
- 每条 rollout 使用唯一的 `request_id`，结果能回到正确的 VeRL sample。
- Model Gateway 或模型服务地址可以从 UEnv Worker 实际访问。
- `response_ids`、`response_mask` 以及异步模式下的逐 token logprob 长度一致。
- 失败或超时的 Episode 不会进入正常训练批次。
- 异步训练中可以观察到 rollout 模型版本随参数更新推进，队列、权重同步和过期结果策略均由 VeRL 生效。

## 当前范围

- 当前发布固定 VeRL v0.7.1，其他 VeRL 版本需要重新验证。
- `uenv train run-task` 是当前正式的同步入口；one-step off-policy 和 fully async 仍是源码实验接入。
- 普通 `run-task` 当前只支持 `--max-steps 1`。
- `--rollouts` 必须至少为 2；训练批次、rollout 数和 GPU 数需要满足命令行校验的整除关系。
- 软件工程修复使用 `uenv train run-swe`，请按[代码修复训练](../3-运行任务/10-training-swe-smith-verl.md)操作。

如果你需要把 VeRL 之外的框架接入 UEnv，请阅读[自定义强化学习框架接入](./01-custom-framework.md)。
