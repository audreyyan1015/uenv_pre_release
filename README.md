# UEnv — 面向强化学习的分布式环境执行系统

UEnv 为大模型评测和后训练提供统一的 Episode 接口。评测程序或训练框架通过 UEnv Bridge 提交任务样本。Adapter 将每个 Episode 分配给 UEnv Worker；UEnv Worker 执行环境，计算得分（reward）并返回交互轨迹（trajectory）。同一环境可用于评测和训练采样。

Adapter 由 `uenv-adapter-core.service` 运行。该服务内部使用 UEnv Server（`uenv-server`）模块完成 UEnv Worker 注册、Episode 调度和状态管理。用户指南统一使用组件名“Adapter”；`--server`、`server.endpoint` 和 `uenv logs server` 中的 `server` 是保留的代码名。

## 核心组件

```text
评测程序 / 训练框架
        │
        ▼
uenv-bridge / UEnv Bridge（任务样本 ↔ Episode）
        │ EpisodeRequest / EpisodeResult
        ▼
uenv-adapter-core / Adapter（内部使用 UEnv Server 模块）
        │
        ▼
uenv-worker / UEnv Worker（环境执行、模型 API 调用与得分计算）
        │
        ├── qa / code / 自定义 process plugin（进程插件）
        └── SWE Runtime Gateway / OpenHands Agent

uenv-hub / UEnv Hub（可选：环境注册、版本与 EnvPackage（环境包）分发）
```

| 组件 | 主要职责 |
|---|---|
| `uenv-bridge`（UEnv Bridge） | 将训练框架的任务样本转换为 Episode，并将得分和交互轨迹返回框架；当前提供 VeRL AgentLoop 适配 |
| `uenv-adapter-core`（Adapter） | 接收 UEnv Worker 注册、分配 Episode，并保存状态和结果；内部使用 `uenv-server` 模块 |
| `uenv-worker`（UEnv Worker） | 调用模型 API、执行环境、计算并返回得分 |
| `uenv-hub`（UEnv Hub） | 可选的环境注册、版本管理与 EnvPackage 分发服务 |

## 已有环境类型与数据集 ID

| 环境类型（`env_type`） | 数据集 ID（`dataset`）/ SWE 变体 | 数据集 | 执行方式 |
|---|---|---|---|
| `qa` | `gsm8k` | [GSM8K（Grade School Math 8K）](https://huggingface.co/datasets/openai/gsm8k) | 单轮问答与结果匹配 |
| `qa` | `pubmedqa` | [PubMedQA: A Dataset for Biomedical Research Question Answering](https://github.com/pubmedqa/pubmedqa) | 生物医学问答与分类 |
| `qa` | `scitab` | [SCITAB: A Challenging Benchmark for Compositional Reasoning and Claim Verification on Scientific Tables](https://github.com/XinyuanLu00/SciTab) | 科学表格声明验证 |
| `qa` | `olymmath`、`olymmath-easy`、`olymmath-hard` | [OlymMATH: Challenging the Boundaries of Reasoning: An Olympiad-Level Math Benchmark for Large Language Models](https://huggingface.co/datasets/RUC-AIBOX/OlymMATH) | 奥数问题与答案匹配 |
| `code` | `dscodebench` | [DSCodeBench: A Realistic Benchmark for Data Science Code Generation](https://github.com/ShuyinOuyang/DSCodeBench) | 生成代码并运行任务测试 |
| `swe` | `verified` | [SWE-bench Verified](https://huggingface.co/datasets/SWE-bench/SWE-bench_Verified) | SWE Runtime Gateway 与 OpenHands Agent |
| `swe` | `lite` | [SWE-bench Lite](https://huggingface.co/datasets/SWE-bench/SWE-bench_Lite) | SWE Runtime Gateway 与 OpenHands Agent |
| `swe` | `pro` | [SWE-bench Pro](https://huggingface.co/datasets/ScaleAI/SWE-bench_Pro) | SWE Runtime Gateway 与 OpenHands Agent |
| `swe` | `smith` | [SWE-smith](https://huggingface.co/datasets/SWE-bench/SWE-smith) | SWE Runtime Gateway 与 OpenHands Agent；当前提供 VeRL 训练适配 |

环境类型（`env_type`）确定环境的交互和判分实现，数据集 ID（`dataset`）确定该环境使用的数据格式和判分方式。`math` 是 `qa` 的历史兼容名，新任务使用 `qa`。

按以下条件选择新任务的实现方式：

- 环境返回内容、模型动作格式和得分方式与已有环境相同：在已有环境中增加数据集 ID 和判分实现。
- 任务初始化、环境返回内容、模型动作格式或得分方式与已有环境不同：用 `uenv env plugin create` 创建 process plugin。
- 任务需要容器和外部执行程序：实现任务专用的运行组件；UEnv 安装包中的 SWE 实现可作为代码参考。

## 使用指南

公开手册按用户的实际操作顺序拆为五份：

1. [UEnv 基础部署指南](./Docs/deployment/UEnv基础部署指南.md)：从 GitHub 源码或预构建包安装单机 Adapter 与 UEnv Worker。
2. [UEnv 多机部署指南](./Docs/deployment/UEnv多机部署指南.md)：将 Adapter 与 UEnv Worker 安装到不同主机，并增加 UEnv Worker。
3. [UEnv Hub 使用指南](./Docs/deployment/UEnv%20Hub使用指南.md)：部署可选的 UEnv Hub，管理环境版本、EnvPackage 分发与回滚。
4. [UEnv 评测指南](./Docs/deployment/UEnv评测指南.md)：执行 QA、Code、process plugin 和 SWE 评测，并说明新任务的接入方法。
5. [UEnv 训练指南](./Docs/deployment/UEnv训练指南.md)：使用 UEnv、VeRL 模型 API 和 UEnv Bridge 执行 QA、Code、process plugin 与 SWE 训练。

基础部署后，评测命令明确填写环境类型、数据集 ID、输入与输出。以 `qa + gsm8k` 为例：

```bash
sudo uenv evaluate configure-model \
  --endpoint 'http://10.0.0.30:8000/v1' \
  --model 'Qwen/Qwen2.5-3B-Instruct' \
  --no-api-key

uenv evaluate run-task \
  --endpoint '127.0.0.1:50051' \
  --env-type qa \
  --dataset gsm8k \
  --input /opt/uenv/current/examples/cases/evaluation/qa-gsm8k.jsonl \
  --output "$PWD/results/qa-gsm8k.jsonl" \
  --max-steps 1
```

训练使用同样的环境类型、数据集 ID 和任务样本，并填写模型、工作目录、GPU 和 VeRL 训练参数：

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

创建新 process plugin：

```bash
mkdir -p "$HOME/uenv-envs"
cd "$HOME/uenv-envs"
uenv env plugin create my-environment --dataset my-dataset
```

命令中的 `--env-type`、`--dataset` 和 `--input` 分别填写环境类型、数据集 ID 和任务样本 JSONL。JSONL 中的 `env_config` 和 `reward_config` 描述每条任务样本的初始配置和判分配置。SWE 使用 `--catalog`、`--benchmark-variant` 和 `--input` 选择仓库修复实例。

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

安装、系统要求与验收命令见 UEnv 基础部署指南。

## 源码布局

| 目录 | 用途 |
|---|---|
| `examples/cases/` | 面向用户的可修改任务样本 JSONL 和配置示例 |
| `templates/` | 创建新 process plugin 的模板 |
| `libexec/uenv/` | `uenv` CLI 调用的内部自动化实现 |
| `tools/` | 管理员按需调用的运维工具 |

用户按指南使用 `uenv evaluate ...`、`uenv train ...` 和 `uenv env plugin ...`。`libexec` 内的脚本是 CLI 实现细节，为发布包和自动化测试提供稳定的内部路径。

## 协议

| 通信双方 | 协议 |
|---|---|
| UEnv Bridge ↔ Adapter | `proto/` 中的 L1 gRPC |
| Adapter ↔ UEnv Worker | Episode 分配、定期状态报告与结果上报 |
| UEnv Worker ↔ process plugin | [`plugin_proto/`](./plugin_proto/) 中的 L2 gRPC over UDS |
| UEnv Worker ↔ 模型 API | 兼容 OpenAI Chat Completions 协议的 HTTP |

协议和数据结构说明见 [PROTOCOL.md](./PROTOCOL.md)。

## 许可

Apache-2.0
