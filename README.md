# UEnv — 面向强化学习的分布式环境执行系统

UEnv 为大模型评测和后训练提供统一的 Episode 接口。训练或评测程序提交任务后，
UEnv 负责选择 Worker、执行环境、计算 reward，并返回 trajectory。环境实现与训练框架
彼此解耦：同一个环境可以用于离线评测，也可以接入训练 rollout。

## 组件

```text
评测程序 / 训练框架
        │
        ▼
uenv-bridge（框架适配；当前提供 VeRL 适配）
        │ EpisodeRequest / EpisodeResult
        ▼
uenv-server（控制面、调度、状态与轨迹）
        │
        ▼
uenv-worker（环境执行、模型调用、reward）
        │
        ├── qa / code 等 Proto-UDS 进程插件
        └── 需要专用运行时的复杂环境

uenv-hub（可选：环境注册、版本和制品分发；不参与 Episode 热路径）
```

| 组件 | 主要职责 |
|---|---|
| `uenv-bridge` | 把训练框架样本映射为 Episode；当前提供 VeRL AgentLoop |
| `uenv-server` | Worker 注册、任务调度、状态、结果和轨迹 |
| `uenv-worker` | 调用模型、驱动环境、计算并返回 reward |
| `uenv-hub` | 可选的环境注册表、版本管理和 EnvPackage 分发 |

Hub 与单机/多机是两个独立维度。单机可以使用 Hub 管理环境版本，多机也可以在
没有 Hub 的情况下通过其他方式分发环境。

## 内置环境

| `env_type` | 当前任务 | 执行方式 |
|---|---|---|
| `qa` | GSM8K、PubMedQA、SciTab、OlymMATH | Proto-UDS 进程插件 |
| `code` | DSCodeBench | Proto-UDS 进程插件 |
| `swe` | SWE-bench 系列仓库级修复任务 | 容器、Runtime Gateway 与 Agent |

`env_type` 表示交互和判分能力，`dataset` 表示该能力下的具体任务集。`math` 是
`qa` 的历史兼容名，新接入统一使用 `qa`。

新增任务时：

- 交互和 reward 语义不变：在现有环境中增加 dataset/backend。
- 需要新的交互与判分协议：从 [`templates/process-plugin`](./templates/process-plugin/) 创建插件。
- 需要容器、工具或 Agent：实现专用 Runtime/AgentBridge；`swe` 是现有参考案例。

## 从哪里开始

公开手册按职责拆为五份：

1. [UEnv 基础部署指南](./Docs/deployment/UEnv基础部署指南.md)：单机最小部署。
2. [UEnv 多机部署指南](./Docs/deployment/UEnv多机部署指南.md)：控制面与多个 Worker。
3. [UEnv Hub 使用指南](./Docs/deployment/UEnv%20Hub使用指南.md)：单机或多机的环境管理与分发。
4. [UEnv 评测指南](./Docs/deployment/UEnv评测指南.md)：通用评测、环境扩展和 SWE 案例。
5. [UEnv 训练指南](./Docs/deployment/UEnv训练指南.md)：通用训练接入、VeRL 和 SWE 案例。

安装完成后，评测和训练都要显式声明任务。下面以 `qa + gsm8k` 展示命令形状：

```bash
sudo uenv evaluate configure-model \
  --endpoint 'http://10.0.0.30:8000/v1' \
  --model 'Qwen/Qwen2.5-3B-Instruct' \
  --no-api-key

uenv evaluate run-task \
  --endpoint '127.0.0.1:50051' \
  --env-type qa \
  --dataset gsm8k \
  --input /opt/uenv/current/examples/evaluation/qa-gsm8k.jsonl \
  --output "$PWD/results/qa-gsm8k.jsonl" \
  --max-steps 1

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

bash /opt/uenv/current/examples/environment/plugin.sh \
  create my-environment --dataset my-dataset
```

`env-type + dataset + input` 决定运行什么任务，`env_config + reward_config`
描述每条样本如何初始化和判分。命令行中的 `env-type`、`dataset`、
`max-steps` 是批次权威值；JSONL 可以重复这些字段用于自描述，但不一致会报错。
换成 Code 或自定义环境时，保持命令形状，明确替换任务参数和 JSONL；
SWE 则通过 `run-swe` 显式选择 catalog、benchmark variant 和 instance。具体操作见评测和训练指南。

## 源码构建

构建自包含 Linux 发布包：

```bash
python3 -m unittest tests.test_installation_assets -v
bash ./scripts/build-release.sh --version 0.1.2-trial
```

产物位于 `dist/`：

```text
install.sh
uenv-linux-x86_64.tar.gz
uenv-linux-x86_64.tar.gz.sha256
```

安装和验收命令以基础部署指南为准。

## 协议

| 链路 | 协议 |
|---|---|
| Bridge ↔ Adapter Core / Server | `proto/` 中的 L1 gRPC |
| Server ↔ Worker | 调度、Dispatch、心跳与结果上报 |
| Worker ↔ process plugin | [`plugin_proto/`](./plugin_proto/) 中的 L2 gRPC over UDS |
| Worker ↔ 模型服务 | OpenAI-compatible HTTP |

协议和数据结构说明见 [PROTOCOL.md](./PROTOCOL.md)。

## 许可

Apache-2.0
