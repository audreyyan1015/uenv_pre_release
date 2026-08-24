# UEnv 评测任务样本

`uenv evaluate` 提供两个评测命令：

- `run-task`：执行 QA、Code 和自定义 process plugin（进程插件）的任务样本。
- `run-swe`：使用 SWE Runtime Gateway、OpenHands Agent 和 SWE catalog 执行 SWE 任务样本。

下文的 Adapter 接收并分配 Episode。Adapter 由 `uenv-adapter-core.service` 运行；其内部使用 UEnv Server（`uenv-server`）模块完成 UEnv Worker 注册、Episode 调度和状态管理。

## 通用 `run-task`

QA 示例：

```bash
uenv evaluate run-task \
  --endpoint '127.0.0.1:50051' \
  --env-type qa \
  --dataset gsm8k \
  --input /opt/uenv/current/examples/cases/evaluation/qa-gsm8k.jsonl \
  --output "$PWD/results/qa-gsm8k.jsonl" \
  --max-steps 1
```

Code 使用同一命令结构：

```bash
uenv evaluate run-task \
  --endpoint '127.0.0.1:50051' \
  --env-type code \
  --dataset dscodebench \
  --input /opt/uenv/current/examples/cases/evaluation/code-custom.jsonl \
  --output "$PWD/results/code.jsonl" \
  --max-steps 1
```

已安装的自定义 process plugin 也使用 `run-task`：

```bash
uenv evaluate run-task \
  --endpoint '127.0.0.1:50051' \
  --env-type my-environment \
  --dataset my-dataset \
  --input "$HOME/uenv-envs/my-environment/example.jsonl" \
  --output "$PWD/results/my-environment.jsonl" \
  --max-steps 1
```

迁移时按任务替换：

| 任务变化 | 参数或文件 |
|---|---|
| Adapter 地址 | `--endpoint` |
| 环境类型 | `--env-type` |
| 数据集 ID | `--dataset` |
| 任务样本和环境配置 | `--input` 指向的 JSONL |
| 结果位置 | `--output` |
| 最大交互步数 | `--max-steps` |

命令行中的 `--env-type`、`--dataset` 和 `--max-steps` 适用于本次输入的所有任务样本。JSONL 可以重复对应的 `env_type`、`dataset` 和 `max_steps` 字段；字段值与命令行不一致时，评测命令会报告对应的任务样本。

### JSONL 公共字段

- `id`：任务样本 ID。
- `question`：模型看到的任务说明。
- `env_config`：环境初始化配置。
- `reward_config`：环境判分配置。
- `target`：静态标准答案。

QA 可以使用 `question + target`；Code 在 `env_config` 中提供测试配置；process plugin 自行判分时使用 `env_config + reward_config`。

新任务的初始化、环境返回内容、模型动作格式或得分方式与已有环境不同时，创建 process plugin：

```bash
mkdir -p "$HOME/uenv-envs"
cd "$HOME/uenv-envs"
uenv env plugin create my-environment --dataset my-dataset
```

QA、Code 和 process plugin 由 UEnv Worker 调用模型 API。运行前，在每台可能执行任务的 UEnv Worker 上使用 `uenv evaluate configure-model` 配置模型 API 地址、模型名与密钥。

## 批量 SWE 评测

先按 [UEnv 评测指南 7.1](../../../Docs/deployment/UEnv评测指南.md#71-为-uenv-worker-启用-swe-runtime) 选择单机或多机部署，并为 UEnv Worker 启用 SWE Runtime。

SWE 输入 JSONL 每行选择 SWE catalog 中的一个实例：

```json
{"id":"astropy-7166","instance_id":"astropy__astropy-7166"}
{"id":"requests-1142","instance_id":"psf__requests-1142"}
```

在启用了 SWE Runtime 的 UEnv Worker 主机上，调用本地模型 API 批量执行：

```bash
RUN_ID="verified-$(date +%Y%m%d-%H%M%S)"
sudo uenv evaluate run-swe \
  --provider local \
  --model 'Qwen/Qwen2.5-Coder-7B-Instruct' \
  --base-url 'http://10.0.0.30:8000/v1' \
  --gateway 'http://127.0.0.1:28999' \
  --catalog /opt/uenv/current/share/swe/verified.json \
  --benchmark-variant verified \
  --input /opt/uenv/current/examples/cases/evaluation/swe-verified.jsonl \
  --output "$PWD/results/$RUN_ID.jsonl" \
  --artifacts-dir "/var/lib/uenv/evaluation-runs/$RUN_ID" \
  --max-iterations 30 \
  --batch-size 2
```

`--batch-size` 控制并发数，结果 JSONL 保持输入顺序。`--artifacts-dir` 每次使用 `/var/lib/uenv/evaluation-runs` 下一个新的评测运行文件目录。火山引擎方舟模型 API、完整 SWE catalog、离线 SWE 实例镜像与结果字段见 [UEnv 评测指南](../../../Docs/deployment/UEnv评测指南.md)。
