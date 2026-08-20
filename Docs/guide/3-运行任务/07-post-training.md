# 强化学习训练指南

UEnv 负责强化学习训练中的环境 rollout：UEnv Worker 调用当前策略模型、执行环境并返回 reward 与 trajectory；强化学习框架负责采样策略、advantage、loss、参数更新和 checkpoint。主流程不依赖某个框架的内部配置。

## 通用数据流

| 阶段 | 负责方 | 输入 | 输出 |
|---|---|---|---|
| 选择训练样本 | 强化学习框架 | 数据集与采样策略 | sample、batch ID |
| 构造 Episode | 框架接入代码 | sample、模型端点、环境与判分配置 | 带唯一 request ID 的标准数据包 |
| 调度与执行 | UEnv Server / UEnv Worker | Episode | response/action、reward、trajectory |
| 还原 rollout | 框架接入代码 | 可能乱序的 Episode 结果 | token、mask、reward、轨迹引用 |
| 模型更新 | 强化学习框架 | 可训练 rollout | loss、指标、新模型版本 |
| 保存与追溯 | 强化学习框架与 UEnv | 运行 ID、版本和产物 | checkpoint、日志、轨迹 |

UEnv 不接管优化器或 checkpoint。一次环境执行失败不能静默变成普通低 reward；基础设施错误和模型任务低分必须分开。

## 当前框架能力

通用流程适用于任何满足[强化学习接入契约](../4-接入强化学习框架/02-contract.md)的框架。当前发布状态以[支持矩阵](../4-接入强化学习框架/05-support-matrix.md)为准：

- VeRL v0.7.1 有正式用户入口。
- ROLL 只有实验实现，不能作为生产入口。
- NexRL 仍是规划状态，没有可执行的发布命令。

下面的 `uenv train` 是当前发布包对正式入口的封装。它会准备固定版本的训练接入实现、转换后的数据和 GPU 容器；这些属于当前实现细节，通用契约以接入契约为准。

## 选择任务入口

三类内置任务（`qa`、`code`、SWE）在执行方式、模型调用方和数据形态上的区别见[通用评测流程](./03-evaluation.md#先确定执行路径)的对比表。

| 训练任务 | 当前命令 | 当前限制 |
|---|---|---|
| 问答、代码生成、自定义 process plugin | `uenv train run-task` | 公共入口要求 `--max-steps 1` |
| 代码修复 | `uenv train run-swe` | 只接受 `--benchmark-variant smith` |

`run-swe` 训练只能用 SWE-smith 的 catalog：SWE-smith 是面向训练生成的任务集，verified/lite/pro 是评测基准，不应用于训练；且当前训练 runner 只实现了 smith 这一条链路。SWE 训练使用固定版本的 Agent 与 Runtime Gateway 产生多步修改轨迹；它们是软件工程环境的运行细节。

## 可执行的训练前检查

先完成[UEnv 使用前检查](./01-usage.md#开始前的可执行检查)。然后在 GPU 主机执行：

```bash
nvidia-smi -L
docker info >/dev/null
python3 --version
```

使用 Podman 时把 `docker info` 改为 `podman info`。三条命令必须成功，当前用户必须能启动 GPU 容器。

设置实际地址和模型目录并校验：

```bash
export UENV_SERVER_ENDPOINT='127.0.0.1:50051'
export MODEL_DIR='/absolute/path/to/huggingface-model'

test -d "$MODEL_DIR"
python3 -c 'import os,socket; h,p=os.environ["UENV_SERVER_ENDPOINT"].rsplit(":",1); socket.create_connection((h,int(p)),5).close(); print("UEnv Server reachable")'
```

多机部署还要满足两个方向：GPU 主机到 UEnv Server 的 50051/TCP，以及 UEnv Worker 到 GPU 主机 model gateway 的 18080/TCP。后一个端口由训练 runner 启动后才监听，因此先在防火墙中只向 UEnv Worker 网段放行，运行时再从 UEnv Worker 检查连接。

## 准备训练输入

普通训练 JSONL 每行至少包含：

- 唯一 `id`；
- 显式 `env_type`、`dataset` 和 `max_steps`；
- `question` 与 `target`，或任务需要的 `env_config` / `reward_config`。

例如下面是一条自拟字段示例：

```json
{"id":"example-1","env_type":"qa","dataset":"gsm8k","question":"Return the answer as #### number: 1+1=?","target":"2","max_steps":1}
```

接入实现不从文件名或默认环境猜测任务。命令与每行的环境、数据路由和步数必须一致。SWE 输入从 catalog 显式选择实例；`--instance ID` 与 `--limit N` 不能同时使用。

检查安装包中的示例输入：

```bash
export UENV_RELEASE_ROOT='/opt/uenv/current'
export TRAIN_INPUT="$UENV_RELEASE_ROOT/examples/cases/training/qa-gsm8k.jsonl"

test -r "$TRAIN_INPUT"
jq -e -c . "$TRAIN_INPUT" >/dev/null
```

该文件包含两条自拟数学问答，仅用于展示训练字段与端到端数据流。

## 执行一次完整训练

先设置实际模型、镜像和唯一工作目录。发布示例镜像标签可能随上游变化；需要复现时，把 `TRAIN_IMAGE` 换成团队验证过的不可变 digest。

```bash
export UENV_SERVER_ENDPOINT='127.0.0.1:50051'
export UENV_RELEASE_ROOT='/opt/uenv/current'
export MODEL_DIR='/absolute/path/to/huggingface-model'
export TRAIN_IMAGE='docker.io/verlai/verl:vllm017.latest'
export RUN_ID="rl-train-$(date +%Y%m%d-%H%M%S)"
export WORK_DIR="$PWD/uenv-runs/$RUN_ID"
export TRAIN_INPUT="$UENV_RELEASE_ROOT/examples/cases/training/qa-gsm8k.jsonl"
export TRAIN_CONFIG="$UENV_RELEASE_ROOT/examples/cases/training/verl-grpo-overrides.conf"

test -d "$MODEL_DIR"
test -r "$TRAIN_INPUT"
test -r "$TRAIN_CONFIG"
test ! -e "$WORK_DIR"
mkdir -p "$(dirname "$WORK_DIR")"
docker image inspect "$TRAIN_IMAGE" >/dev/null || docker pull "$TRAIN_IMAGE"
```

确认变量后执行目标训练，这就是本轮训练：

```bash
uenv train run-task \
  --model "$MODEL_DIR" \
  --work-dir "$WORK_DIR" \
  --uenv-endpoint "$UENV_SERVER_ENDPOINT" \
  --env-type qa \
  --dataset gsm8k \
  --input "$TRAIN_INPUT" \
  --max-steps 1 \
  --gpus 1 \
  --steps 20 \
  --rollouts 4 \
  --train-batch-size 2 \
  --runtime docker \
  --image "$TRAIN_IMAGE" \
  --verl-config "$TRAIN_CONFIG"
```

约束：`--rollouts` 至少为 2；训练 batch、rollout 和 GPU 并行配置必须可整除；`--steps` 控制模型更新步数，与 Episode 最大步数相互独立。

当前 runner 的工作目录包含：

```text
WORK_DIR/
  episode-data/            # 接入实现转换后的训练数据
  .uenv-verl/
    output/                # 指标和 checkpoint
```

验收数据转换与训练产物：

```bash
test -d "$WORK_DIR/episode-data"
test -d "$WORK_DIR/.uenv-verl/output"
find "$WORK_DIR/.uenv-verl/output" -maxdepth 4 -type f -print | head -50
```

配置示例在第 20 步保存；日志必须显示计划更新完成，每个参与更新的 sample 都有可对齐的 response token/mask 和 reward，且失败 Episode 没有被无提示地转成低分。

## 代码修复训练

软件工程训练增加 catalog、实例容器、Agent 迭代和完整 response trace，但仍遵循同一六阶段数据流。当前入口只支持 Smith variant，完整变量、命令和验收见[代码修复训练](./10-training-swe-smith-verl.md)。不要只替换普通 `run-task` 的 `env_type` 来模拟 SWE 训练。

## UEnv Server 与 GPU 主机分离

在安装了完整 UEnv release 的服务主机导出客户端。命令会在归档旁同时生成 checksum 和安装脚本：

```bash
mkdir -p "$PWD/uenv-training-client"
uenv train export-client \
  --output "$PWD/uenv-training-client/uenv-training-client.tar.gz"
ls -l "$PWD/uenv-training-client/"
```

安全复制以下三个文件到 GPU 主机：归档、同名 `.sha256`、`install-training-client.sh`。然后在 GPU 主机安装：

```bash
bash ./install-training-client.sh \
  --archive ./uenv-training-client.tar.gz \
  --checksum ./uenv-training-client.tar.gz.sha256 \
  --target "$HOME/uenv-training-client"
test -x "$HOME/uenv-training-client/bin/uenv-train"
```

远程执行时使用该 `uenv-train`，并在命令中增加实际值：

```text
--uenv-release "$HOME/uenv-training-client"
--uenv-endpoint "10.0.0.10:50051"
--gateway-public-url "http://10.0.0.30:18080/v1"
--gateway-bind "10.0.0.30"
```

`gateway-public-url` 必须是 UEnv Worker 实际可达的 URL；不能使用只对 GPU 主机自身有效的 `127.0.0.1`。`gateway-bind` 使用 GPU 主机的可路由接口地址，或按本机安全策略绑定后由反向代理暴露。

## 失败、重试和并发

| 现象 | 先检查 | 处理原则 |
|---|---|---|
| GPU 容器不能启动 | `nvidia-smi`、容器 runtime、镜像 | 先修 GPU 环境，不提交 Episode |
| UEnv Worker 无法调用训练模型 | 18080/TCP、public URL、bind 地址 | 从 UEnv Worker 检查，不用 loopback |
| 没有可调度环境 | `uenv workers` 的 capability/容量 | 安装环境或降低并发 |
| response token/mask 缺失 | UEnv Worker 模型响应与接入 trace | 不用最终文本静默重编码代替可验证 trace |
| 单个 Episode 失败 | error code、UEnv Worker 日志、trajectory | 默认 fail fast |
| GPU OOM | rollout、batch、上下文和并行配置 | 同步调整并保持整除关系 |

传输重试保持原 `request_id`，同一个逻辑 Episode 不能因重试变成两条训练样本。只有任务定义明确允许时才可使用 `zero_reward`；基础设施失败不能作为模型能力信号。

## 完成标准与下一步

一次强化学习训练完成至少满足：

1. 转换后样本数与选择范围一致。
2. 每个参与更新的 sample 都有可对齐 token/mask、reward 和 ID。
3. 框架完成计划更新，没有未处理的失败 Episode。
4. 指标、checkpoint、模型版本、数据版本、环境版本与命令可追溯。
5. 需要审计时，训练结果能通过 `trajectory_id` 关联环境动作。

接着阅读[轨迹采集指南](./12-trajectory.md)，先查看本轮已有结果，再决定是否启用集中存储。框架内部 hook、字段映射和配置见[VeRL 接入](../4-接入强化学习框架/04-verl.md)；案例见[训练案例](./02-cases.md#强化学习训练)。
