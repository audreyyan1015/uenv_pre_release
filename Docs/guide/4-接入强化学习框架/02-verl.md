# 以 VeRL 为例接入 UEnv

UEnv 已经提供 VeRL 适配器。使用当前支持的 VeRL v0.7.1 时，你不需要修改 VeRL 源码，也不需要自己处理 UEnv 协议。`uenv train run-task` 会把 VeRL 选出的训练样本交给 UEnv 执行环境交互，并把模型输出对应的 token、mask、reward 和轨迹信息转换回 VeRL 可直接训练的数据。

开始训练前，你只需要准备一个已运行的 UEnv 服务、目标环境、Hugging Face 格式的模型目录、训练 JSONL，以及可运行 GPU 容器的主机。下面先用发布包中的两条 QA 数据跑通一次模型更新，再说明如何替换为自己的数据。

## VeRL 适配器做了什么

VeRL 选好一批 sample 后，UEnv 适配器会在本地 rollout 开始前接管这个批次。它将 sample 转成 UEnv Episode，让 UEnv Worker 调用当前策略模型、运行环境并计分，然后将结果转回 VeRL rollout 输出。VeRL 随后按原流程计算 advantage、loss 并更新模型。

`uenv train run-task` 会自动准备匹配版本的 VeRL 与 UEnv Bridge、转换数据、配置 AgentLoop、向 Worker 暴露当前策略模型并启动训练。这些步骤属于适配器内部实现，使用者无需配置 gRPC 消息、request ID、token 对齐或 VeRL hook。

## 准备运行环境

本页的示例假定 UEnv Server 和 Worker 都在 GPU 主机上，且 Worker 已安装 `qa` 环境。先检查 UEnv、GPU 和容器运行时：

```bash
uenv doctor
uenv environments
nvidia-smi -L
docker info >/dev/null
```

四条命令都应成功。如果使用 Podman，将后续命令中的 `docker` 换成 `podman`，并向 `uenv train run-task` 传入 `--runtime podman`。

## 运行发布包中的示例

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

## 替换为自己的数据

普通 QA 训练的 JSONL 每行写一条 sample。下面是最小示例：

```json
{"id":"example-1","env_type":"qa","dataset":"gsm8k","question":"1+1=? End with `#### number`.","target":"2","max_steps":1}
```

`id` 建议在数据集内唯一，`env_type`、`dataset` 和 `max_steps` 必须与运行命令一致。只有环境确实需要额外输入或自定义奖励配置时，才增加 `env_config` 或 `reward_config`。

将这些数据保存为 JSONL 后，仅需把上一节命令的 `--input` 改为新文件。如果更换环境或数据集，同时修改 `--env-type` 和 `--dataset`。更完整的数据示例见[数学问答训练](../3-运行任务/08-training-gsm8k-verl.md)和[代码生成训练](../3-运行任务/09-training-code-verl.md)。

## 查看结果

转换后的数据位于 `$WORK_DIR/episode-data/`，VeRL 配置、指标和训练产物位于 `$WORK_DIR/.uenv-verl/output/`。运行：

```bash
test -d "$WORK_DIR/episode-data"
test -d "$WORK_DIR/.uenv-verl/output"
find "$WORK_DIR/.uenv-verl/output" -maxdepth 2 -type f -print
```

本次接入成功应同时满足：训练完成了设定的更新步数，训练样本收到了 response token、mask 和 reward，且日志中没有未处理的 UEnv Episode 失败。需要查询环境轨迹时，继续阅读[获取轨迹](../3-运行任务/12-trajectory.md)。

## UEnv 与 GPU 主机分离时

当 UEnv Server 和 GPU 训练主机不在同一台机器时，先导出并安装训练客户端，再使用 GPU 主机上的 `uenv-train run-task`。还需要打通 GPU 主机到 UEnv Server 的 50051/TCP，以及 UEnv Worker 到 GPU 主机的 18080/TCP。

完整导出、安装和运行参数见[强化学习训练指南](../3-运行任务/07-post-training.md#uenv-server-与-gpu-主机分离)。特别注意，model gateway 的公开地址必须从 UEnv Worker 访问，不能使用 GPU 主机自身的 `127.0.0.1`。

## 当前范围

- 当前发布固定 VeRL v0.7.1，其他 VeRL 版本需要重新验证。
- 普通 `run-task` 当前只支持 `--max-steps 1`。
- `--rollouts` 必须至少为 2；训练批次、rollout 数和 GPU 数需要满足命令行校验的整除关系。
- 软件工程修复使用 `uenv train run-swe`，请按[代码修复训练](../3-运行任务/10-training-swe-smith-verl.md)操作。

如果你需要把 VeRL 之外的框架接入 UEnv，请阅读[自定义强化学习框架接入](./01-custom-framework.md)。
