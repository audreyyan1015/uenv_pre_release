# UEnv 评测示例

`evaluate.sh` 只提供两个明确入口：

- `run-task`：QA、Code 和自定义 process plugin；
- `run-swe`：需要容器、Runtime Gateway 和 Agent 的 SWE 任务。

脚本不会代填任务参数。`run-task` 必须指定 Adapter Core、环境、dataset、输入、输出和步数：

```bash
uenv evaluate run-task \
  --endpoint '127.0.0.1:50051' \
  --env-type qa \
  --dataset gsm8k \
  --input /opt/uenv/current/examples/evaluation/qa-gsm8k.jsonl \
  --output "$PWD/results/qa-gsm8k.jsonl" \
  --max-steps 1
```

## 同一入口迁移到其他任务

Code：

```bash
uenv evaluate run-task \
  --endpoint '127.0.0.1:50051' \
  --env-type code \
  --dataset dscodebench \
  --input /opt/uenv/current/examples/evaluation/code-custom.jsonl \
  --output "$PWD/results/code.jsonl" \
  --max-steps 1
```

自定义 process plugin：

```bash
uenv evaluate run-task \
  --endpoint '127.0.0.1:50051' \
  --env-type my-environment \
  --dataset my-dataset \
  --input "$HOME/uenv-envs/my-environment/example.jsonl" \
  --output "$PWD/results/my-environment.jsonl" \
  --max-steps 1
```

这三条命令的形状相同。迁移时修改：

| 任务变化 | 修改的位置 |
|---|---|
| 换环境实现 | `--env-type` |
| 换环境内的数据路由 | `--dataset` |
| 换样本和环境配置 | `--input` 指向的 JSONL |
| 换结果位置 | `--output` |
| 换交互步数 | `--max-steps` |
| 换 UEnv 服务 | `--endpoint` |

命令行中的 `env-type`、`dataset` 和 `max-steps` 是整个批次的权威值。JSONL 中可以重复相同值以便文件自描述，但如果某一行与命令不一致，评测会报错。一次命令只运行一种任务路由。

## JSONL 契约

公共字段是：

- `id`：样本标识；
- `question`：模型看到的任务说明；
- `env_config`：交给环境的初始化配置；
- `reward_config`：交给环境的判分配置；
- `target`：有静态标准答案时使用。

QA 可以只用 `question + target`；Code 需要在 `env_config` 中提供 harness；自判分插件可以使用 `env_config + reward_config`。

新任务如果只是数据变了，复用已有 `env_type` 并更换 dataset/JSONL。如果 `reset`、`step`、observation、action 或 reward 语义变了，则创建新插件：

```bash
bash /opt/uenv/current/examples/environment/plugin.sh \
  create my-environment --dataset my-dataset
```

## 模型和首次运行

QA、Code 和普通 process plugin 由 Worker 调用模型。运行前要在每台候选 Worker 上用 `uenv evaluate configure-model` 配置 OpenAI-compatible endpoint、模型名和密钥。

`run-task` 首次执行会在当前用户目录创建隔离 Python venv，并安装 UEnv Bridge 客户端依赖。它不会修改系统 Python，也不会下载任务数据、模型或容器镜像。

## SWE

启用 SWE Worker 时必须明确 bundle、部署角色、容器运行时、镜像策略和 Gateway：

```bash
sudo uenv evaluate prepare-swe \
  --bundle /home/uenv-install/uenv-linux-x86_64.tar.gz \
  --profile single-node \
  --runtime docker \
  --image-policy allow_public \
  --gateway '127.0.0.1:28999'
```

然后显式选择模型提供方、catalog、benchmark variant 和 instance：

```bash
sudo uenv evaluate run-swe \
  --provider local \
  --model 'Qwen/Qwen2.5-Coder-7B-Instruct' \
  --base-url 'http://10.0.0.30:8000/v1' \
  --gateway 'http://127.0.0.1:28999' \
  --catalog /opt/uenv/current/share/swe/verified.json \
  --benchmark-variant verified \
  --instance astropy__astropy-7166 \
  --output-dir "$PWD/results/swe-model-api-astropy-7166" \
  --max-iterations 30
```

完整说明见 [`Docs/deployment/UEnv评测指南.md`](../../Docs/deployment/UEnv评测指南.md)。
