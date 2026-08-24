# UEnv + VeRL 训练任务样本

Adapter 是接收 Episode 的 UEnv 接口，由 `uenv-adapter-core.service` 运行；其内部使用 UEnv Server（`uenv-server`）模块完成 UEnv Worker 注册、Episode 调度和状态管理。

`uenv train` 提供 `run-task` 和 `run-swe` 两个训练命令。命令明确填写环境类型、数据集 ID、任务样本、模型、Adapter 地址和训练参数。替换这些参数后，同一命令可以执行其他任务。

## 通用 `run-task`

QA 示例：

```bash
uenv train run-task \
  --model /data/models/Qwen2.5-3B-Instruct \
  --work-dir /data/uenv-runs/qa-gsm8k \
  --uenv-endpoint '127.0.0.1:50051' \
  --env-type qa \
  --dataset gsm8k \
  --input /opt/uenv/current/examples/cases/training/qa-gsm8k.jsonl \
  --max-steps 1 \
  --gpus 1 \
  --steps 1 \
  --rollouts 2 \
  --train-batch-size 1 \
  --runtime docker \
  --image 'docker.io/verlai/verl:vllm017.latest'
```

Code 使用同一命令结构：

```bash
uenv train run-task \
  --model /data/models/Qwen2.5-Coder-3B-Instruct \
  --work-dir /data/uenv-runs/code-dscodebench \
  --uenv-endpoint '127.0.0.1:50051' \
  --env-type code \
  --dataset dscodebench \
  --input /opt/uenv/current/examples/cases/training/code-dscodebench.jsonl \
  --max-steps 1 \
  --gpus 1 \
  --steps 1 \
  --rollouts 2 \
  --train-batch-size 1 \
  --runtime docker \
  --image 'docker.io/verlai/verl:vllm017.latest'
```

自定义 process plugin（进程插件）也使用 `run-task`：

```bash
uenv train run-task \
  --model /data/models/Qwen2.5-3B-Instruct \
  --work-dir /data/uenv-runs/my-environment \
  --uenv-endpoint '127.0.0.1:50051' \
  --env-type my-environment \
  --dataset my-dataset \
  --input /opt/uenv/current/examples/cases/training/process-plugin.jsonl \
  --max-steps 1 \
  --gpus 1 \
  --steps 100 \
  --rollouts 4 \
  --train-batch-size 8 \
  --runtime docker \
  --image 'docker.io/verlai/verl:vllm017.latest'
```

使用其他任务时，替换环境类型（`--env-type`）、数据集 ID（`--dataset`）、任务样本 JSONL（`--input`）和工作目录（`--work-dir`）。JSONL 的公共字段是 `id`、`question`、`env_config`、`reward_config` 和可选的 `target`。数据转换程序将这些字段写入 VeRL `extra_info`，UEnv Bridge 再将任务样本提交给 Adapter。

当前通用 `run-task` 使用 `--max-steps 1`，每个 Episode 包含一次模型动作。多步训练需要先在 UEnv Bridge 中实现该环境的专用数据转换。

## 追加 VeRL/Hydra 参数

通用命令直接列出常用训练参数。其他 VeRL/Hydra 选项通过配置文件或 `--set` 设置：

```bash
cp /opt/uenv/current/examples/cases/training/verl-grpo-overrides.conf \
  /data/config/my-grpo.conf
```

在 `run-task` 或 `run-swe` 命令末尾增加：

```text
--verl-config /data/config/my-grpo.conf
--set actor_rollout_ref.actor.optim.lr=1e-6
--set trainer.save_freq=20
```

`--set` 可重复。增加 `--print-effective-config` 时，命令只显示最终 Hydra 配置覆盖项并结束。哪些配置由 UEnv 任务参数统一生成，详见 [UEnv 训练指南第 7 节](../../../Docs/deployment/UEnv训练指南.md#7-配置更多-verl-参数)。

## UEnv 主机与 VeRL GPU 主机分开运行

在 UEnv 主机导出训练客户端：

```bash
uenv train export-client \
  --output "$HOME/uenv-training-client.tar.gz"
```

把客户端包、SHA-256 文件和安装脚本复制到 VeRL GPU 主机后，使用：

```bash
"$HOME/uenv-training-client/bin/uenv-train" run-task \
  --uenv-release "$HOME/uenv-training-client" \
  --model /data/models/Qwen2.5-3B-Instruct \
  --work-dir /data/uenv-runs/qa-gsm8k \
  --uenv-endpoint '10.0.0.10:50051' \
  --gateway-public-url 'http://10.0.0.20:18080/v1' \
  --gateway-bind '10.0.0.20' \
  --env-type qa \
  --dataset gsm8k \
  --input "$HOME/uenv-training-client/examples/cases/training/qa-gsm8k.jsonl" \
  --max-steps 1 \
  --gpus 1 \
  --steps 1 \
  --rollouts 2 \
  --train-batch-size 1 \
  --runtime docker \
  --image 'docker.io/verlai/verl:vllm017.latest'
```

`10.0.0.10` 是运行 Adapter 和 UEnv Worker 的 UEnv 主机，`10.0.0.20` 是 VeRL GPU 主机。`--gateway-public-url` 是 VeRL GPU 主机上的 VeRL 模型 API 地址，UEnv Worker 通过该地址调用当前正在训练的模型。

## SWE `run-swe`

先按 [UEnv 训练指南第 5 节](../../../Docs/deployment/UEnv训练指南.md#5-为-swe-训练准备-uenv) 为 UEnv Worker 启用 SWE Runtime。SWE 训练使用 SWE Runtime Gateway 和 OpenHands Agent，并从 SWE catalog 中选择实例：

```bash
uenv train run-swe \
  --model /data/models/Qwen2.5-Coder-7B-Instruct \
  --work-dir /data/uenv-runs/swe-smith \
  --uenv-endpoint '127.0.0.1:50051' \
  --catalog /opt/uenv/current/share/swe/smith-sample-catalog.json \
  --benchmark-variant smith \
  --instance oauthlib__oauthlib.1fd52536.combine_file__09vlzwgc \
  --max-iterations 30 \
  --gpus 1 \
  --steps 1 \
  --rollouts 2 \
  --train-batch-size 1 \
  --runtime docker \
  --image 'docker.io/verlai/verl:vllm017.latest'
```

`--instance` 可重复；按 SWE catalog 顺序取前 N 条时使用 `--limit N`。`--image` 是 VeRL GPU 主机使用的 VeRL 训练镜像，SWE catalog 中的 `image_cache_key` 是 UEnv Worker 使用的 SWE 实例镜像。完整单机与多机命令见 [UEnv 训练指南](../../../Docs/deployment/UEnv训练指南.md)。
