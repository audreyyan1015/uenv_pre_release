# UEnv 训练指南

UEnv 在训练中负责执行环境、计算 reward 和返回 trajectory；VeRL 负责生成 rollout、计算 advantage 和更新模型。当前 release 提供 VeRL 适配入口。

一次通用训练必须明确三组信息：

| 类别 | 必须明确的内容 |
|---|---|
| 任务 | `env-type`、`dataset`、`input`、`max-steps` |
| UEnv 连接 | `uenv-endpoint` |
| 训练 | `model`、`work-dir`、`gpus`、`steps`、`rollouts`、`train-batch-size`、`runtime`、`image` |

`uenv train run-task` 不会代替用户选择数据、环境或训练规模。命令中每项参数都是迁移到其他任务时的明确修改点。

命令行中的 `env-type`、`dataset` 和 `max-steps` 是这一批训练数据的权威值。JSONL 可以重复写入相同字段用于自描述，但如果任何一行与命令不一致，数据转换会立即失败。不同环境或 dataset 应分开启动训练，不会在同一批数据中静默改路由。

## 1. 开始前需要什么

- UEnv Adapter Core 和至少一台 Worker 已部署。
- 目标 `env_type + dataset` 已按 [UEnv 评测指南](./UEnv评测指南.md) 成功运行过 Episode。
- GPU 主机已安装 NVIDIA 驱动、Docker 或 Podman，以及 NVIDIA Container Toolkit。
- GPU 主机的当前用户能运行 GPU 容器。
- GPU 主机有 Git、Python 3.10+ 和 `python3-venv`。
- 待训练模型是 GPU 主机上的 Hugging Face 格式目录。

`run-task` 和 `run-swe` 会在 `--work-dir` 中准备 release 锁定的 VeRL 源码和 UEnv Bridge，并用 `--image` 指定的 CUDA 容器运行训练。它们不下载模型权重。如果主机上没有数据转换依赖，脚本会在工作目录创建隔离 venv 并安装 `pandas` 和 `pyarrow`，不修改系统 Python。

联网主机需要访问 Git 源、Python 包索引和镜像仓库。离线训练应先准备 VeRL 源码、Bridge 依赖、数据转换依赖和指定的 CUDA 镜像。

## 2. 单机：QA/GSM8K 训练

本节的 UEnv 和 VeRL 在同一台 GPU 主机。下面的命令把任务、UEnv 连接和训练配置全部展开：

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

这条命令的参数并非一组隐藏的“GSM8K 模式”：

| 要改的东西 | 对应参数 |
|---|---|
| 换模型 | `--model` |
| 换 UEnv 服务 | `--uenv-endpoint` |
| 换环境实现 | `--env-type` |
| 换该环境中的数据路由 | `--dataset` |
| 换训练数据 | `--input` |
| 换结果与中间文件目录 | `--work-dir` |
| 改训练规模 | `--gpus`、`--steps`、`--rollouts`、`--train-batch-size` |
| 换容器运行时和 VeRL CUDA 镜像 | `--runtime`、`--image` |

正式执行前可以先加 `--dry-run`：只准备数据和 Bridge 资产、打印将要执行的容器命令，不连接 UEnv、不校验 GPU，也不会拉取 `--image` 指定的 CUDA 镜像。无 GPU 的主机可用它检查参数与路径，确认无误后去掉该参数再真正运行。

命令中的 1 个 step 用于确认训练链路可执行，不能证明模型能力提升。正式训练应根据数据量和 GPU 数重新设置步数、rollout 数和 batch size。`train-batch-size × rollouts` 必须不小于 GPU 数且能被 GPU 数整除；`rollouts` 至少为 2。

## 3. 换成 Code 或自定义环境

### 3.1 Code

Code 和 QA 使用同一个 `run-task` 入口。环境、模型、数据和工作目录都在命令中明确指定：

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

Code JSONL 的 `env_config` 必须提供入口函数、测试代码或测试文件、测试数量和超时。对不可信代码进行训练时，Worker 需要额外的文件、进程和网络隔离。

### 3.2 自定义 process plugin

先复制 release 中的插件数据样例，把任务名、环境配置和 reward 配置改成已安装的 `warehouse` 插件所需内容：

```bash
cp /opt/uenv/current/examples/training/process-plugin.jsonl \
  /data/tasks/warehouse-train.jsonl
```

然后显式选择 `warehouse + warehouse-v1`：

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

自定义数据仍使用相同 JSONL 契约：

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

数据转换器会把命令中的 `env_type`、`dataset`、`max_steps` 和每行的 `env_config`、`reward_config`、`target` 写入 VeRL `extra_info`，Bridge 再把它们交给环境。因此迁移任务时不需要复制 Shell 脚本，需要修改的是命令中的任务参数和 JSONL 的环境配置。

当前通用 `run-task` 只支持 `--max-steps 1`。这不是为任务挑选的数字：单步 process plugin 只需要一次 model action。多步训练还要把 observation 和 action 交错编码为带 mask 的 token trace，使 VeRL 能按实际生成上下文重算 log probability。多步环境可先用评测入口验证；要训练则需要环境专用 Bridge trace 适配，不能只把 `--max-steps` 改大。

## 4. 双机：CPU/UEnv 与 GPU/VeRL

这个拓扑中，CPU 主机运行 Adapter Core 和 Worker，GPU 主机运行 VeRL 和模型网关。GPU 主机不安装 UEnv systemd 服务，只需要一个训练客户端包。

### 4.1 在 CPU/UEnv 主机导出客户端

```bash
uenv train export-client \
  --output "$HOME/uenv-training-client.tar.gz"
```

把产生的 `.tar.gz`、`.sha256` 和 `install-training-client.sh` 复制到 GPU 主机。该包包含 UEnv Bridge wheel、VeRL 配置和训练入口，不包含 Adapter Core、Worker、Hub 或 systemd 服务。

### 4.2 在 GPU/VeRL 主机解压并运行

```bash
bash ./install-training-client.sh \
  --archive ./uenv-training-client.tar.gz \
  --checksum ./uenv-training-client.tar.gz.sha256 \
  --target "$HOME/uenv-training-client"
```

以 QA 为例，GPU 主机的完整命令为：

```bash
bash "$HOME/uenv-training-client/examples/training/train_verl.sh" run-task \
  --uenv-release "$HOME/uenv-training-client" \
  --model /data/models/Qwen2.5-3B-Instruct \
  --work-dir /data/uenv-runs/qa-gsm8k \
  --uenv-endpoint '10.0.0.10:50051' \
  --gateway-public-url 'http://10.0.0.20:18080/v1' \
  --gateway-bind '10.0.0.20' \
  --env-type qa \
  --dataset gsm8k \
  --input "$HOME/uenv-training-client/examples/training/qa-gsm8k.jsonl" \
  --max-steps 1 \
  --gpus 1 \
  --steps 1 \
  --rollouts 2 \
  --train-batch-size 1 \
  --runtime docker \
  --image 'docker.io/verlai/verl:vllm017.latest'
```

其中 `10.0.0.10` 是 CPU/UEnv 主机，`10.0.0.20` 是 GPU/VeRL 主机。GPU 主机必须能访问 CPU 主机 50051/TCP；CPU 主机的 Worker 必须能访问 GPU 主机 18080/TCP。18080 只应对 UEnv Worker 所在网络放行。

这里的 `--gateway-public-url` 指 GPU 主机上的 VeRL 模型网关，供 UEnv 回调当前训练模型；它不是 SWE Worker 的 Runtime Gateway。SWE Runtime Gateway 在 `prepare-swe` 时配置，由 Worker 注册并随 AgentJob 注入，不能把两个地址互换。

迁移到 Code 或自定义环境时，双机网络参数不变，仍只替换 `env-type + dataset + input + work-dir`。

## 5. SWE 训练的 Worker 准备

SWE 使用容器任务环境、Runtime Gateway 和 OpenHands，因此要先在执行 SWE 环境的 UEnv 主机上启用这些组件。QA、Code 和普通 process plugin 不执行本节。

准备命令显式填写与已安装系统相同版本的 bundle、部署角色、容器运行时、镜像策略和 Gateway：

```bash
sudo uenv train prepare-swe \
  --bundle /home/uenv-install/uenv-linux-x86_64.tar.gz \
  --profile single-node \
  --runtime docker \
  --image-policy allow_public \
  --gateway '127.0.0.1:28999'
```

bundle 只在 UEnv 服务主机上用于启用同版本的 SWE Runtime 和配套 Agent 资产，不表示要在 GPU 客户端重新部署 UEnv。双机时 GPU 主机仍只使用第 4 节的训练客户端包。

`allow_public` 允许 Worker 为实际选中的 SWE 实例拉取任务镜像。离线节点应先导入目标镜像，并使用 `--image-policy local_only`。Gateway 需要被其它主机上的 Agent 访问时，还要显式加上 `--gateway-public 'http://<SWE_WORKER_IP>:28999'`。

## 6. SWE + VeRL 训练

SWE 不使用通用 Episode JSONL，而是从 catalog 中选择实例。当前 VeRL 数据适配器只支持 Smith，因此命令还必须显式写 `--benchmark-variant smith`。用户必须提供 catalog，并用一个或多个 `--instance` 选择实例；如果确实要按 catalog 顺序取前 N 条，可以用 `--limit N` 取代 `--instance`。

在 UEnv 和 VeRL 同机时：

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

在 CPU/UEnv 与 GPU/VeRL 双机时，在 GPU 主机执行：

```bash
bash "$HOME/uenv-training-client/examples/training/train_verl.sh" run-swe \
  --uenv-release "$HOME/uenv-training-client" \
  --model /data/models/Qwen2.5-Coder-7B-Instruct \
  --work-dir /data/uenv-runs/swe-smith \
  --uenv-endpoint '10.0.0.10:50051' \
  --gateway-public-url 'http://10.0.0.20:18080/v1' \
  --gateway-bind '10.0.0.20' \
  --catalog "$HOME/uenv-training-client/share/swe/smith-example.json" \
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

这里有两类不同的镜像：

- `--image` 是 GPU 主机上运行 VeRL 的 CUDA 镜像；
- catalog 中的 `image_cache_key` 是 UEnv Worker 上运行 SWE 任务的环境镜像。

下载数据记录不会自动得到任务镜像，拉取任务镜像也不会得到模型权重。三类资产应分别管理。

## 7. 结果、Hub 与其他训练框架

训练结果和 UEnv Episode 记录位于：

```text
<work-dir>/.uenv-verl/output/
<work-dir>/.uenv-verl/output/agent-loop-requests.jsonl
<work-dir>/.uenv-verl/output/agent-loop-results.jsonl
```

VeRL 完成 optimizer step 仅说明连接、环境和数据契约可执行。正式训练还需要独立的训练/验证集、checkpoint 与恢复测试、reward 分布、失败率和训练指标监控。

Hub 不是训练热路径的必需组件。需要固定环境版本、让多台 Worker 同步同一插件或镜像、验证 digest 和回滚时，再按 [UEnv Hub 使用指南](./UEnv%20Hub使用指南.md) 管理环境制品。

其他训练框架需要实现自己的 Bridge：把框架样本映射为 `EpisodeRequest`，再把 `EpisodeResult` 中的 response token、reward 和 trajectory 映射回训练框架。当前 release 只对 VeRL 提供本指南中的自动化入口。
