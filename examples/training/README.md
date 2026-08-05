# UEnv + VeRL 训练示例

`train_verl.sh` 的用户入口是 `run-task` 和 `run-swe`。两者都要显式提供任务、模型、UEnv 连接和训练规模，不会选择内置任务。

## `run-task`

QA：

```bash
uenv train run-task \
  --model /data/models/Qwen2.5-3B-Instruct \
  --work-dir /data/uenv-runs/qa-gsm8k \
  --uenv-endpoint '127.0.0.1:50051' \
  --env-type qa \
  --dataset gsm8k \
  --input /opt/uenv/current/examples/training/qa-gsm8k.jsonl \
  --max-steps 1 \
  --gpus 1 \
  --steps 1 \
  --rollouts 2 \
  --train-batch-size 1 \
  --runtime docker \
  --image 'docker.io/verlai/verl:vllm017.latest'
```

Code 使用同形命令：

```bash
uenv train run-task \
  --model /data/models/Qwen2.5-Coder-3B-Instruct \
  --work-dir /data/uenv-runs/code-dscodebench \
  --uenv-endpoint '127.0.0.1:50051' \
  --env-type code \
  --dataset dscodebench \
  --input /opt/uenv/current/examples/training/code-dscodebench.jsonl \
  --max-steps 1 \
  --gpus 1 \
  --steps 1 \
  --rollouts 2 \
  --train-batch-size 1 \
  --runtime docker \
  --image 'docker.io/verlai/verl:vllm017.latest'
```

自定义 process plugin 仍使用同一入口：

```bash
uenv train run-task \
  --model /data/models/Qwen2.5-3B-Instruct \
  --work-dir /data/uenv-runs/my-environment \
  --uenv-endpoint '127.0.0.1:50051' \
  --env-type my-environment \
  --dataset my-dataset \
  --input /opt/uenv/current/examples/training/process-plugin.jsonl \
  --max-steps 1 \
  --gpus 1 \
  --steps 100 \
  --rollouts 4 \
  --train-batch-size 8 \
  --runtime docker \
  --image 'docker.io/verlai/verl:vllm017.latest'
```

换任务时，明确替换 `env-type + dataset + input + work-dir`。JSONL 的公共字段是 `id`、`question`、`env_config`、`reward_config` 和可选的 `target`。数据转换器会把这些字段写入 VeRL `extra_info`，Bridge 再交给目标环境。

命令行中的 `env-type`、`dataset` 和 `max-steps` 是批次权威值。JSONL 可以重复相同值用于自描述，但与命令不一致会立即报错，不会在同一批中静默切换任务。

当前通用 `run-task` 只接受 `--max-steps 1`。多步环境训练需要专用 token-trace Bridge 适配，不能只修改 JSONL 或步数。

## CPU/UEnv 与 GPU/VeRL 双机

在 UEnv 主机上运行 `uenv train export-client --output FILE`，再把客户端包、SHA-256 文件和解压脚本复制到 GPU 主机。GPU 主机使用客户端目录中的 `train_verl.sh run-task`，并在上述参数基础上增加：

```bash
--uenv-release "$HOME/uenv-training-client" \
--uenv-endpoint '10.0.0.10:50051' \
--gateway-public-url 'http://10.0.0.20:18080/v1' \
--gateway-bind '10.0.0.20'
```

`10.0.0.10` 是 CPU/UEnv 主机，`10.0.0.20` 是 GPU/VeRL 主机。GPU 主机不安装 UEnv systemd 服务，也不需要完整 release bundle。

## `run-swe`

SWE 使用 catalog 选择任务：

```bash
uenv train run-swe \
  --model /data/models/Qwen2.5-Coder-7B-Instruct \
  --work-dir /data/uenv-runs/swe-smith \
  --uenv-endpoint '127.0.0.1:50051' \
  --catalog /opt/uenv/current/share/swe/smith-example.json \
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

`--instance` 可重复以选择多条；如需按 catalog 顺序取前 N 条，可改用 `--limit N`。`--image` 是 GPU 主机的 VeRL CUDA 镜像，catalog 中的 `image_cache_key` 是 UEnv Worker 上的 SWE 任务镜像，两者不可混淆。

完整的单机、双机、SWE Worker 准备和结果说明见 [`Docs/deployment/UEnv训练指南.md`](../../Docs/deployment/UEnv训练指南.md)。
