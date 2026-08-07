# UEnv 训练指南

本指南说明如何使用 UEnv 进行 VeRL 训练。UEnv 执行任务环境，返回得分（reward）和交互轨迹（trajectory）；VeRL 生成模型输出并更新模型。

当前 UEnv 版本支持 VeRL v0.7.1，并提供 QA、Code、自定义 process plugin（进程插件）和 SWE 的训练命令。

一条任务样本的一次环境执行称为 Episode。Adapter 是接收 Episode 的 UEnv 接口，由 `uenv-adapter-core.service` 运行；其内部使用 UEnv Server（`uenv-server`）模块完成 UEnv Worker 注册、Episode 调度和状态管理。UEnv Worker 执行 Episode 并返回结果。VeRL 模型 API 是 UEnv Worker 访问当前训练模型的 HTTP 接口。

首先确认两类主机的职责：

| 主机 | 运行的内容 | 保存的内容 |
|---|---|---|
| UEnv 主机 | Adapter 和 UEnv Worker。Adapter 接收 Episode，UEnv Worker 执行环境并计算得分 | 环境配置、process plugin、Episode 记录；SWE 任务还包括 SWE catalog 和 SWE 实例镜像 |
| VeRL GPU 主机 | VeRL、当前训练模型和 VeRL 模型 API | 模型权重、训练数据、训练检查点（checkpoint）和训练结果 |

两类职责可以放在同一台 VeRL GPU 主机，也可以分布在不同主机。UEnv Hub 是环境版本的发布和同步服务。基础训练直接连接 Adapter；当团队需要多台 UEnv Worker 统一环境版本时，再使用 UEnv Hub。

本指南按以下顺序组织：

1. 先用第 2 节的 QA/GSM8K 命令完成第一次单机训练。
2. 需要 Code 或自定义 process plugin 时，按第 3 节替换任务参数和训练数据。
3. UEnv 主机与 VeRL GPU 主机分开时，按第 4 节安装训练客户端。
4. 执行 SWE 训练时，按第 5 节准备 UEnv Worker，再执行第 6 节。
5. 基础命令运行成功后，在第 7 节配置更多 VeRL 参数。

## 1. 训练前准备

执行训练命令前，完成以下准备：

- 按 [UEnv 基础部署指南](./UEnv基础部署指南.md) 或 [UEnv 多机部署指南](./UEnv多机部署指南.md) 部署 Adapter 和至少一台 UEnv Worker。
- 按 [UEnv 评测指南](./UEnv评测指南.md) 用目标环境类型（`env_type`）和数据集 ID（`dataset`）成功执行一条 Episode。
- VeRL GPU 主机已安装 NVIDIA 驱动、Docker 或 Podman、NVIDIA Container Toolkit、Git、Python 3.10+ 和 `python3-venv`。
- VeRL GPU 主机的当前用户可以运行 GPU 容器。
- 待训练模型已以 Hugging Face 格式保存在 VeRL GPU 主机的本地目录中。
- 训练数据已保存为 JSONL 文件，每行表示一条任务样本。

UEnv Bridge 是训练程序连接 Adapter 的 Python 组件。它把 VeRL 训练数据转换成 Episode，并把 Episode 结果交回 VeRL。

`uenv train run-task` 和 `uenv train run-swe` 会在 `--work-dir` 中准备与当前 UEnv 版本匹配的 VeRL 源码、UEnv Bridge 和数据转换环境。训练在 `--image` 指定的 GPU 容器中运行。

联网主机需要访问 Git 源、Python 包索引和容器镜像仓库（OCI Registry）。离线主机需要由管理员提前准备对应的源码、Python 安装包（wheel）和 VeRL 训练镜像。

## 2. 第一次训练：单机 QA/GSM8K

本节使用一台 VeRL GPU 主机。Adapter、UEnv Worker 和 VeRL 都运行在这台主机上。示例数据来自 [GSM8K（Grade School Math 8K）](https://huggingface.co/datasets/openai/gsm8k)。

一个模型更新步骤（optimizer step）表示 VeRL 使用一批数据更新一次模型参数。采样轨迹数（rollout 数）表示每条任务样本生成的模型输出数。训练批大小（batch size）表示每个模型更新步骤使用的任务样本数。

确认模型目录和工作目录后执行：

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

这条命令使用 1 个模型更新步骤，用于确认数据转换、VeRL、Adapter 和 UEnv Worker 可以完成一次训练。执行成功后，训练结果保存在：

```text
/data/uenv-runs/qa-gsm8k/.uenv-verl/output/
```

以后迁移到其他任务时，按下表替换参数：

| 需要替换的内容 | 参数 | 含义 |
|---|---|---|
| 模型 | `--model` | VeRL GPU 主机上的 Hugging Face 模型目录 |
| 训练结果目录 | `--work-dir` | 保存 VeRL 源码、中间文件和训练结果 |
| Adapter | `--uenv-endpoint` | Adapter 的 `HOST:PORT` |
| 环境类型 | `--env-type` | 例如 `qa`、`code` 或已安装的 process plugin 名称 |
| 数据集 ID | `--dataset` | 例如 `gsm8k`、`dscodebench` 或团队定义的 ID |
| 训练数据 | `--input` | JSONL 文件路径 |
| 环境最大步数 | `--max-steps` | 每条 Episode 允许的最大交互步数 |
| 训练规模 | `--gpus`、`--steps`、`--rollouts`、`--train-batch-size` | GPU 数、模型更新步骤数、每条任务样本的采样轨迹数和训练批大小 |
| VeRL 训练环境 | `--runtime`、`--image` | VeRL GPU 主机上的容器运行时和 VeRL 训练镜像 |

在命令末尾增加 `--dry-run` 可以准备数据和 UEnv Bridge，并显示将要执行的容器命令。正式训练时去掉 `--dry-run`。

示例参数用于确认流程。正式训练时，根据数据量和 GPU 数调整 `--steps`、`--rollouts` 和 `--train-batch-size`。训练批大小与采样轨迹数的乘积需要大于等于 GPU 数且能被 GPU 数整除，`--rollouts` 至少为 2。

## 3. 训练 Code 和自定义 process plugin

`uenv train run-task` 是 QA、Code 和自定义 process plugin 的公共训练命令。迁移时保留训练参数，替换环境类型（`env_type`）、数据集 ID（`dataset`）和 JSONL 文件。

### 3.1 Code 示例

以 [DSCodeBench: A Realistic Benchmark for Data Science Code Generation](https://github.com/ShuyinOuyang/DSCodeBench) 样例为例：

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

Code JSONL 的 `env_config` 记录要测试的函数名、测试代码或测试文件、测试数量和超时时间。UEnv Worker 按部署配置对执行代码设置文件、进程和网络限制。

### 3.2 自定义 process plugin 示例

本示例假设名为 `warehouse` 的 process plugin 已经安装并在 UEnv Worker 上激活。创建目录并复制 JSONL 样例：

```bash
mkdir -p /data/tasks
cp /opt/uenv/current/examples/cases/training/process-plugin.jsonl \
  /data/tasks/warehouse-train.jsonl
```

根据 `warehouse` process plugin 的输入要求修改每行的 `question`、`env_config`、`reward_config` 和 `target`。例如：

```json
{
  "id": "warehouse-1",
  "env_type": "warehouse",
  "dataset": "warehouse-v1",
  "question": "Move the package to shelf B.",
  "env_config": {"map": "warehouse-a", "start": "A"},
  "reward_config": {"goal": "B", "success_reward": 1.0},
  "max_steps": 1
}
```

然后执行：

```bash
uenv train run-task \
  --model /data/models/Qwen2.5-3B-Instruct \
  --work-dir /data/uenv-runs/warehouse-v1 \
  --uenv-endpoint '127.0.0.1:50051' \
  --env-type warehouse \
  --dataset warehouse-v1 \
  --input /data/tasks/warehouse-train.jsonl \
  --max-steps 1 \
  --gpus 1 \
  --steps 100 \
  --rollouts 4 \
  --train-batch-size 8 \
  --runtime docker \
  --image 'docker.io/verlai/verl:vllm017.latest'
```

命令中的 `--env-type`、`--dataset` 和 `--max-steps` 需要与 JSONL 中的同名字段一致。

数据转换器会把这些字段和每行的 `env_config`、`reward_config`、`target` 写入 VeRL `extra_info`，UEnv Bridge 再将它们提交给指定的 process plugin。

当前 `run-task` 公共训练命令按单步模式运行，需要设置 `--max-steps 1`。多步训练需要先为目标环境实现专用的 UEnv Bridge 适配。

## 4. UEnv 主机与 VeRL GPU 主机分开运行

本拓扑使用两台主机：

| 地址 | 职责 |
|---|---|
| `10.0.0.10` | UEnv 主机，运行 Adapter 和 UEnv Worker |
| `10.0.0.20` | VeRL GPU 主机，运行 VeRL、当前训练模型和 VeRL 模型 API |

### 4.1 在 UEnv 主机导出训练客户端

```bash
uenv train export-client \
  --output "$HOME/uenv-training-client.tar.gz"
```

命令会生成三个文件：

- `uenv-training-client.tar.gz`
- `uenv-training-client.tar.gz.sha256`
- `install-training-client.sh`

把这三个文件复制到 VeRL GPU 主机。

### 4.2 在 VeRL GPU 主机安装训练客户端

```bash
bash ./install-training-client.sh \
  --archive ./uenv-training-client.tar.gz \
  --checksum ./uenv-training-client.tar.gz.sha256 \
  --target "$HOME/uenv-training-client"
```

### 4.3 在 VeRL GPU 主机执行 QA 训练

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

双机训练需要两个网络方向：

| 访问方向 | 端口 | 用途 |
|---|---|---|
| VeRL GPU 主机 → UEnv 主机 | `50051/TCP` | VeRL 通过 Adapter 提交 Episode |
| UEnv Worker → VeRL GPU 主机 | `18080/TCP` | UEnv Worker 通过 VeRL 模型 API 请求当前训练模型 |

`--gateway-public-url` 填写 UEnv Worker 可访问的 VeRL 模型 API URL，`--gateway-bind` 填写 VeRL GPU 主机的内网监听地址。仅向 UEnv Worker 所在的内网放行 `18080/TCP`。

迁移到 Code 或自定义 process plugin 时，保留本节的 `--uenv-endpoint`、`--gateway-public-url` 和 `--gateway-bind`。替换 `--env-type`、`--dataset`、`--input` 和 `--work-dir`。

## 5. 为 SWE 训练准备 UEnv

SWE Runtime 是 UEnv Worker 上负责启动、管理和评测 SWE 实例镜像的运行组件。SWE catalog 记录每个 SWE 实例的仓库、提交、问题和 SWE 实例镜像。

SWE Runtime Gateway 向 OpenHands Agent 提供任务环境操作接口。OpenHands Agent 接收 SWE Episode，调用当前模型并操作任务环境。

两种准备方式提供相同的 SWE 功能。Adapter 与 UEnv Worker 同机时执行 5.1 节；二者位于不同主机时执行 5.2 节。每次部署只选择对应的一组命令。VeRL GPU 主机的安装方式保持不变。

`--bundle` 填写当前 UEnv 版本的 UEnv 安装包 `uenv-linux-x86_64.tar.gz`。

`prepare-swe` 会使用该 UEnv 安装包启用 SWE Runtime Gateway 和 OpenHands Agent 所需的服务与 Python 依赖。

### 5.1 Adapter 与 UEnv Worker 在同一台 UEnv 主机

先在 UEnv 主机安装并启动 Docker 或 Podman。基础部署使用 `single-node` 安装模式（profile）时执行：

```bash
sudo uenv train prepare-swe \
  --bundle /home/uenv-install/uenv-linux-x86_64.tar.gz \
  --profile single-node \
  --runtime docker \
  --image-policy allow_public \
  --gateway '127.0.0.1:28999'
```

已使用 `full` 安装模式部署 Adapter、UEnv Worker 和 UEnv Hub 时，将命令中的 `--profile single-node` 改为 `--profile full`。

在这种部署中，SWE Runtime Gateway 和 OpenHands Agent 与 UEnv Worker 同机运行，因此 `--gateway` 使用 `127.0.0.1:28999`。VeRL GPU 主机可以是另一台主机。

### 5.2 Adapter 与 UEnv Worker 分开部署

本示例使用以下地址：

| 地址 | 职责 |
|---|---|
| `10.0.0.10` | Adapter |
| `10.0.0.21` | 启用了 SWE Runtime 的 UEnv Worker、SWE Runtime Gateway 和 OpenHands Agent |

首先在 Adapter 所在的 UEnv 主机执行：

```bash
sudo uenv train prepare-swe \
  --bundle /home/uenv-install/uenv-linux-x86_64.tar.gz \
  --profile control-plane \
  --shared-key-file /home/uenv-install/uenv-swe-shared.key
```

该命令在指定路径生成 SWE 共享密钥。通过团队的受保护文件传输方式，把这个文件原样复制到每台 UEnv Worker。在 UEnv Worker 上设置文件权限：

```bash
sudo install -o root -g root -m 0600 \
  ./uenv-swe-shared.key \
  /home/uenv-install/uenv-swe-shared.key
```

然后在 UEnv Worker 上安装并启动 Docker 或 Podman，执行：

```bash
sudo uenv train prepare-swe \
  --bundle /home/uenv-install/uenv-linux-x86_64.tar.gz \
  --profile worker \
  --server '10.0.0.10:50051' \
  --advertise '10.0.0.21:50054' \
  --shared-key-file /home/uenv-install/uenv-swe-shared.key \
  --runtime docker \
  --image-policy allow_public \
  --gateway '0.0.0.0:28999' \
  --gateway-public 'http://10.0.0.21:28999' \
  --trajectory-endpoint 'http://10.0.0.10:8077'
```

各参数的地址如下：

| 参数 | 填写内容 |
|---|---|
| `--server` | Adapter 的 gRPC 地址；该地址由 `uenv-adapter-core.service` 提供 |
| `--advertise` | Adapter 可访问的 UEnv Worker 地址 |
| `--gateway` | SWE Runtime Gateway 的监听地址 |
| `--gateway-public` | OpenHands Agent 可访问的 SWE Runtime Gateway URL；UEnv Worker 会把该 URL 登记到 Adapter |
| `--trajectory-endpoint` | 用于提交交互轨迹的 Adapter URL |

在基础多机端口之外，还需要为 SWE 训练开放以下内网访问：

| 来源 | 目标 | 端口 | 用途 |
|---|---|---:|---|
| UEnv Worker | Adapter | `50051/TCP`、`8077/TCP` | 注册、状态报告、结果和交互轨迹 |
| Adapter | UEnv Worker | `50054/TCP`、`28999/TCP` | 发送 Episode、访问 SWE Runtime Gateway |
| 每台 OpenHands Agent 主机 | 每台启用了 SWE Runtime 的 UEnv Worker | `28999/TCP` | 操作 SWE 实例镜像 |
| UEnv Worker 和 OpenHands Agent 主机 | VeRL GPU 主机 | `18080/TCP` | 调用 VeRL 模型 API |

`--image-policy allow_public` 允许 UEnv Worker 按 SWE catalog 拉取当前实例的 SWE 实例镜像。离线 UEnv Worker 由管理员先导入所需 SWE 实例镜像，然后使用 `--image-policy local_only`。

增加更多 UEnv Worker 时，在每台 UEnv Worker 重复上述 `worker` 安装模式命令，并为 `--advertise` 和 `--gateway-public` 填写该 UEnv Worker 的内网地址。

所有 UEnv Worker 使用同一个 Adapter 地址和 SWE 共享密钥。

## 6. 执行 SWE 训练

SWE 训练从 SWE catalog 选择实例。当前 VeRL 数据适配器支持 [SWE-smith](https://huggingface.co/datasets/SWE-bench/SWE-smith)，因此命令使用 `--benchmark-variant smith`。

`--instance` 可以重复使用，用于选择多个实例。`--limit N` 按 SWE catalog 顺序选择前 N 个实例。

### 6.1 UEnv 与 VeRL 在同一台 VeRL GPU 主机

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

### 6.2 UEnv 主机与 VeRL GPU 主机分开

在 VeRL GPU 主机执行：

```bash
"$HOME/uenv-training-client/bin/uenv-train" run-swe \
  --uenv-release "$HOME/uenv-training-client" \
  --model /data/models/Qwen2.5-Coder-7B-Instruct \
  --work-dir /data/uenv-runs/swe-smith \
  --uenv-endpoint '10.0.0.10:50051' \
  --gateway-public-url 'http://10.0.0.20:18080/v1' \
  --gateway-bind '10.0.0.20' \
  --catalog "$HOME/uenv-training-client/share/swe/smith-sample-catalog.json" \
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

当 UEnv 使用多台 UEnv Worker 时，`--uenv-endpoint` 仍填写 Adapter 地址。Adapter 根据环境类型（`env_type`）和 UEnv Worker 状态分配 Episode。

SWE 训练使用 VeRL 训练镜像和 SWE 实例镜像：

| 镜像 | 位置 | 用途 |
|---|---|---|
| `--image` | VeRL GPU 主机 | 运行 VeRL 的 VeRL 训练镜像 |
| SWE catalog 中的 `image_cache_key` | 启用了 SWE Runtime 的 UEnv Worker | 运行指定仓库和提交的 SWE 实例镜像 |

模型权重保存在 VeRL GPU 主机，SWE catalog 保存 SWE 实例信息，SWE 实例镜像保存对应的仓库环境。

SWE 训练中的 OpenHands Agent 使用工具调用操作任务环境。Qwen 系列模型可以在命令末尾增加：

```text
--set +actor_rollout_ref.rollout.engine_kwargs.vllm.enable_auto_tool_choice=true
--set +actor_rollout_ref.rollout.engine_kwargs.vllm.tool_call_parser=hermes
```

`hermes` 是 Qwen 工具调用格式的解析器。使用其他模型时，按该模型的工具调用格式设置解析器。

## 7. 配置更多 VeRL 参数

第 2、3 和 6 节已在命令中显式设置 GPU 数、模型更新步骤数、采样轨迹数和训练批大小。VeRL 使用 Hydra 配置系统；其他参数以 Hydra 配置覆盖项（Hydra override）的形式通过 `--verl-config` 或可重复的 `--set` 设置。

### 7.1 使用配置文件

复制 UEnv 安装目录中的配置样例：

```bash
mkdir -p /data/config
cp /opt/uenv/current/examples/cases/training/verl-grpo-overrides.conf \
  /data/config/warehouse-grpo.conf
```

配置文件每行写一个 Hydra 配置覆盖项，格式为 `KEY=VALUE`。编辑完成后，在 `run-task` 或 `run-swe` 命令末尾增加：

```text
--verl-config /data/config/warehouse-grpo.conf
```

### 7.2 使用命令行覆盖

临时调整参数时，可重复使用 `--set`：

```text
--set actor_rollout_ref.actor.optim.lr=1e-6
--set trainer.save_freq=20
```

有效配置的合并顺序是：UEnv 基线配置 < `--verl-config` < 命令行 `--set`。在同一条训练命令末尾增加 `--print-effective-config`，可以显示最终 Hydra 配置覆盖项列表并结束命令。正式训练会把同一份配置记录到：

```text
<work-dir>/.uenv-verl/output/effective-hydra-overrides.txt
```

VeRL v0.7.1 的上游配置位于：

```text
<work-dir>/.uenv-verl/verl/verl/trainer/config/ppo_trainer.yaml
```

以下 VeRL/Hydra 配置由 UEnv 命令参数生成：

| VeRL/Hydra 配置 | UEnv 命令参数 |
|---|---|
| `data.train_files`、`data.val_files` | `--input` 或 SWE catalog 的转换结果 |
| `data.train_batch_size` | `--train-batch-size` |
| `actor_rollout_ref.model.path` | `--model` |
| `actor_rollout_ref.rollout.n` | `--rollouts` |
| `trainer.n_gpus_per_node` | `--gpus` |
| `trainer.total_training_steps` | `--steps` |
| `trainer.nnodes` | 当前命令固定为 1 |
| VeRL 代理循环（AgentLoop）配置 | 由 `run-task` 或 `run-swe` 选择 |

使用上表的 UEnv 命令参数调整数据、模型、GPU 数和采样轨迹数。使用 `--verl-config` 或 `--set` 调整优化器、学习率、保存频率、显存和采样引擎（rollout engine）等 VeRL 选项。

## 8. 查看训练结果

训练结果和 UEnv Episode 请求记录位于：

```text
<work-dir>/.uenv-verl/output/
<work-dir>/.uenv-verl/output/agent-loop-requests.jsonl
<work-dir>/.uenv-verl/output/agent-loop-results.jsonl
<work-dir>/.uenv-verl/output/effective-hydra-overrides.txt
```

VeRL 完成模型更新步骤后，可继续为正式训练配置训练集、验证集、训练检查点和恢复测试，并监控得分分布、Episode 失败率和 VeRL 训练指标。

## 9. UEnv Hub 与其他训练框架

UEnv Hub 用于发布环境版本，并让多台 UEnv Worker 同步相同的 process plugin 和环境文件。训练命令向 Adapter 提交 Episode，Adapter 将 Episode 分配给 UEnv Worker。

UEnv Hub 的部署、发布、同步和回滚步骤见 [UEnv Hub 使用指南](./UEnv%20Hub使用指南.md)。

接入其他训练框架时，实现该框架的 UEnv Bridge：将训练任务样本转换为 `EpisodeRequest`，再将 `EpisodeResult` 中的模型输出、得分和交互轨迹转换回训练框架所需的格式。当前 UEnv 版本中的 VeRL 集成可作为实现参考。
