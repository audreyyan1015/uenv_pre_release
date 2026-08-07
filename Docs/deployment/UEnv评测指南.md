# UEnv 评测指南

本指南说明如何使用已部署的 UEnv 执行评测。前六节适用于 QA、Code 和自定义 process plugin（进程插件）；第 7 节适用于 SWE 任务。每个示例都会说明准备内容、命令的执行主机、输入文件和输出文件。

## 1. 开始前的准备

### 1.1 确认 UEnv 已经可用

先完成 [UEnv 基础部署指南](./UEnv基础部署指南.md) 或 [UEnv 多机部署指南](./UEnv多机部署指南.md)。本指南将接收和分配 Episode 的组件称为 Adapter。Adapter 由 `uenv-adapter-core.service` 运行；其内部使用 UEnv Server（`uenv-server`）模块完成 UEnv Worker 注册、Episode 调度和状态管理。

评测开始前，UEnv 需要满足以下条件：

1. Adapter 正在运行。
2. 至少一台 UEnv Worker 已注册到 Adapter，状态为 `ready`。
3. UEnv Worker 支持本次评测使用的环境。
4. 用于生成模型动作的模型 API 已经可访问。

QA、Code 和 process plugin 评测可在安装了 `uenv` 命令的主机上执行，该主机需要能访问 Adapter。SWE 评测命令在启用了 SWE Runtime 的 UEnv Worker 主机上执行。

### 1.2 本指南使用的名称

| 名称 | 含义 |
|---|---|
| Adapter | 接收 Episode，并将 Episode 分配给 UEnv Worker；由 `uenv-adapter-core.service` 运行 |
| UEnv Worker | 运行环境，返回得分（reward）和交互轨迹（trajectory） |
| Episode | 一条任务样本的一次完整执行 |
| 模型 API | 提供兼容 OpenAI Chat Completions 协议的模型调用接口，可以位于本机或远程主机 |
| process plugin | 以独立进程方式运行的 UEnv 环境插件 |
| SWE Runtime | UEnv Worker 上负责启动、管理和评测 SWE 实例镜像的运行组件 |
| SWE Runtime Gateway | 启用了 SWE Runtime 的 UEnv Worker 上用于创建和操作 SWE 容器环境的 HTTP 服务 |
| SWE catalog | 按 `instance_id` 记录代码仓库、提交、测试和 SWE 实例镜像的 JSON 文件 |
| SWE 实例镜像 | 某个 SWE 实例执行测试时使用的容器镜像 |
| OpenHands Agent | 读取 SWE 问题、调用模型 API 并修改代码的执行程序 |

### 1.3 选择评测命令

| 任务类型 | 命令 | 输入 | 输出 |
|---|---|---|---|
| QA、Code、process plugin | `uenv evaluate run-task` | 每行一条任务样本的 JSONL | 每行一条 Episode 结果的 JSONL |
| SWE | `sudo uenv evaluate run-swe` | 每行选择一个 SWE 实例的 JSONL | 每行一个 SWE 实例结果的 JSONL，以及每个实例的评测运行文件目录 |

### 1.4 内置环境类型和数据集 ID

环境类型（`env_type`）选择环境实现，数据集 ID（`dataset`）选择该环境内的数据格式和判分方式。

| `env_type` | `dataset` | 对应官方数据集 | 执行内容 |
|---|---|---|---|
| `qa` | `gsm8k` | [GSM8K（Grade School Math 8K）](https://huggingface.co/datasets/openai/gsm8k) | 数学问答与结果匹配 |
| `qa` | `pubmedqa` | [PubMedQA: A Dataset for Biomedical Research Question Answering](https://github.com/pubmedqa/pubmedqa) | 生物医学问答与分类 |
| `qa` | `scitab` | [SCITAB: A Challenging Benchmark for Compositional Reasoning and Claim Verification on Scientific Tables](https://github.com/XinyuanLu00/SciTab) | 科学表格声明验证 |
| `qa` | `olymmath`、`olymmath-easy`、`olymmath-hard` | [OlymMATH: Challenging the Boundaries of Reasoning: An Olympiad-Level Math Benchmark for Large Language Models](https://huggingface.co/datasets/RUC-AIBOX/OlymMATH) | 奥数问题与答案匹配 |
| `code` | `dscodebench` | [DSCodeBench: A Realistic Benchmark for Data Science Code Generation](https://github.com/ShuyinOuyang/DSCodeBench) | 生成代码并运行测试 |

SWE 任务通过 SWE 变体和 SWE catalog 选择。命令参数为 `--benchmark-variant` 和 `--catalog`：

| `--benchmark-variant` 的值 | 官方数据集 | UEnv 安装包中的内容 |
|---|---|---|
| `verified` | [SWE-bench Verified](https://huggingface.co/datasets/SWE-bench/SWE-bench_Verified) | 10 条用于检查安装的样例 |
| `lite` | [SWE-bench Lite](https://huggingface.co/datasets/SWE-bench/SWE-bench_Lite) | 按需生成 SWE catalog |
| `pro` | [SWE-bench Pro](https://huggingface.co/datasets/ScaleAI/SWE-bench_Pro) | 按需生成 SWE catalog |
| `smith` | [SWE-smith](https://huggingface.co/datasets/SWE-bench/SWE-smith) | 5 条训练样例 |

## 2. 配置模型 API

本节配置用于 `run-task`。QA、Code 和 process plugin 由 UEnv Worker 调用模型 API 生成模型动作。以下命令需要在每台执行评测任务的 UEnv Worker 主机上运行一次。

SWE 评测在 `run-swe` 命令中直接填写模型 API 参数。

配置无鉴权的本地 vLLM 或 SGLang 模型 API：

```bash
sudo uenv evaluate configure-model \
  --endpoint 'http://10.0.0.30:8000/v1' \
  --model 'Qwen/Qwen2.5-7B-Instruct' \
  --no-api-key
```

配置火山引擎方舟模型 API：

```bash
sudo uenv evaluate configure-model \
  --endpoint 'https://ark.cn-beijing.volces.com/api/v3' \
  --model 'ep-xxxxxxxx'
```

第二条命令会提示输入 API Key。`--model` 填写方舟的推理接入点 ID。

命令把模型 API 配置写入 UEnv Worker 的服务配置。UEnv Worker 在启动时读取该文件，因此配置发生变化时，命令会自动重启 UEnv Worker；配置内容相同时，服务继续运行。

`configure-model --endpoint` 填写模型 API 地址。后文 `run-task --endpoint` 填写 Adapter 地址。

## 3. 执行 QA 评测

本节命令可在安装了 `uenv` 命令、且能访问 Adapter 的主机上执行。下面的示例假设命令与 Adapter 在同一台主机上执行。

在其他主机执行时，将 `127.0.0.1:50051` 换成 Adapter 的内网地址。示例输入文件由 UEnv 安装包提供，结果写入当前目录的 `results/qa-gsm8k.jsonl`。

```bash
uenv evaluate run-task \
  --endpoint '127.0.0.1:50051' \
  --env-type qa \
  --dataset gsm8k \
  --input /opt/uenv/current/examples/cases/evaluation/qa-gsm8k.jsonl \
  --output "$PWD/results/qa-gsm8k.jsonl" \
  --max-steps 1
```

`run-task` 使用以下六个参数确定整批任务样本：

| 参数 | 填写内容 |
|---|---|
| `--endpoint` | Adapter 地址 |
| `--env-type` | 环境类型 |
| `--dataset` | 数据集 ID |
| `--input` | 输入 JSONL 路径 |
| `--output` | 输出 JSONL 路径 |
| `--max-steps` | 每个 Episode 的最大交互步数 |

QA JSONL 每行包含一条问题和标准答案：

```json
{"id":"case-1","env_type":"qa","dataset":"gsm8k","question":"What is 3 + 5? End with #### number.","target":"8","max_steps":1}
```

JSONL 中的 `env_type`、`dataset` 和 `max_steps` 需要与命令行参数一致。使用其他 QA 数据集时，替换 `--dataset`、`--input` 和 `--output`。多轮任务同时调整 `--max-steps`。

## 4. 执行 Code 评测

Code 评测的执行主机和输出方式与 QA 评测相同。以下命令读取 UEnv 安装包中的 Code 样例：

```bash
uenv evaluate run-task \
  --endpoint '127.0.0.1:50051' \
  --env-type code \
  --dataset dscodebench \
  --input /opt/uenv/current/examples/cases/evaluation/code-custom.jsonl \
  --output "$PWD/results/code.jsonl" \
  --max-steps 1
```

Code JSONL 每行使用 `env_config` 说明要运行的代码测试：

```json
{
  "id": "custom-code-1",
  "env_type": "code",
  "dataset": "dscodebench",
  "question": "Return only Python code defining add(a, b).",
  "target": "custom-code-1",
  "env_config": {
    "task_id": "custom-code-1",
    "library": "python",
    "entry_point": "add",
    "test_code": "assert add(2, 3) == 5",
    "num_tests": 1,
    "timeout_secs": 30
  },
  "max_steps": 1
}
```

使用其他代码数据时，替换 `question`、`entry_point`、测试内容和超时时间。Code 环境会执行模型生成的代码，因此生产环境应为 UEnv Worker 配置受限文件系统、受限网络或容器隔离。

## 5. 执行自定义 process plugin 评测

以已安装的 `warehouse` process plugin 为例。它的数据集 ID 是 `warehouse-v1`。在能访问 Adapter 的主机上执行：

```bash
uenv evaluate run-task \
  --endpoint '127.0.0.1:50051' \
  --env-type warehouse \
  --dataset warehouse-v1 \
  --input "$HOME/uenv-envs/warehouse/example.jsonl" \
  --output "$PWD/results/warehouse.jsonl" \
  --max-steps 4
```

`uenv env plugin create` 生成的示例输入文件固定命名为 `example.jsonl`；使用自定义输入文件时替换 `--input` 的路径。

输入 JSONL 每行是一条任务样本。一条 `run-task` 命令会为文件中的每条任务样本执行一个 Episode：

```json
{
  "id": "warehouse-1",
  "env_type": "warehouse",
  "dataset": "warehouse-v1",
  "question": "Move the package to shelf B.",
  "env_config": {"map": "warehouse-a", "start": "A"},
  "reward_config": {"goal": "B", "success_reward": 1.0},
  "max_steps": 4
}
```

| 字段 | 填写内容 |
|---|---|
| `id` | 任务样本 ID，用于查找对应结果 |
| `question` | 发送给模型的任务说明 |
| `env_config` | process plugin 启动该 Episode 时使用的初始配置 |
| `reward_config` | process plugin 计算得分时使用的配置 |
| `target` | 静态标准答案；process plugin 自行判分时可以使用 `reward_config` |

## 6. 接入新任务

先根据新任务的交互和得分计算方式选择扩展方式：

| 新任务的需求 | 扩展方式 |
|---|---|
| 环境返回内容、模型动作格式和得分方式与已有环境相同，只有数据不同 | 在已有环境中增加 `dataset`、数据转换和判分实现 |
| 任务是生成代码并运行测试 | 在 `code` 环境中增加 `dataset` 和测试实现 |
| 任务初始化、环境返回内容、模型动作格式或得分方式与已有环境不同 | 创建新的 process plugin |
| 任务需要容器、外部执行程序或较长的执行过程 | 实现该任务专用的运行组件和执行程序；UEnv 的 SWE 实现可作为代码参考 |

下面创建一个名为 `warehouse`、数据集 ID 为 `warehouse-v1` 的 process plugin：

```bash
mkdir -p "$HOME/uenv-envs"
cd "$HOME/uenv-envs"
uenv env plugin create warehouse --dataset warehouse-v1
```

命令生成目录后，修改下列文件：

| 文件 | 需要填写的内容 |
|---|---|
| `environment.py` | 任务初始状态、模型动作处理、环境返回内容和得分计算 |
| `example.jsonl` | 至少一条可运行的任务 |
| `requirements.txt` | process plugin 使用的第三方 Python 依赖 |

`plugin.py`、`run.sh`、`uenv_plugin_api.py` 和 `generated/` 是 UEnv 的通信文件，保持生成时的内容。修改完成后，在 process plugin 所在主机执行：

```bash
uenv env plugin test "$HOME/uenv-envs/warehouse"
sudo uenv env plugin install-local "$HOME/uenv-envs/warehouse"
```

`install-local` 将 process plugin 安装到当前 UEnv Worker。需要向多台 UEnv Worker 发布固定版本时，使用 [UEnv Hub 使用指南](./UEnv%20Hub使用指南.md) 中的发布、同步和激活流程。

## 7. 执行 SWE 评测

SWE catalog 确定本次评测的 SWE 实例，并提供代码仓库、提交和 SWE 实例镜像信息。启用了 SWE Runtime 的 UEnv Worker 启动对应镜像。

OpenHands Agent 调用模型 API，并通过 SWE Runtime Gateway 操作镜像中的代码和测试。

开始前准备：

1. 与当前 UEnv 版本一致的 UEnv 安装包，例如 `/home/uenv-install/uenv-linux-x86_64.tar.gz`。`prepare-swe` 会从该安装包取得同版本的 SWE 运行组件。
2. UEnv Worker 主机上已安装并启动 Docker 或 Podman。
3. UEnv Worker 能访问模型 API。
4. 本次评测的 SWE catalog 和输入 JSONL。

### 7.1 为 UEnv Worker 启用 SWE Runtime

两种准备方式提供相同的 SWE 功能。Adapter 与 UEnv Worker 同机时执行“单机”命令；二者位于不同主机时执行“多机”命令。每次部署只选择对应的一组命令。

#### 单机：Adapter 和 UEnv Worker 在同一台主机

在该主机执行。基础部署使用 `single-node` 安装模式（profile）时：

```bash
sudo uenv evaluate prepare-swe \
  --bundle /home/uenv-install/uenv-linux-x86_64.tar.gz \
  --profile single-node \
  --runtime docker \
  --image-policy allow_public \
  --gateway '127.0.0.1:28999'
```

当前主机安装了 Adapter、UEnv Worker 和 UEnv Hub，即安装时使用 `full` 安装模式，执行：

```bash
sudo uenv evaluate prepare-swe \
  --bundle /home/uenv-install/uenv-linux-x86_64.tar.gz \
  --profile full \
  --runtime docker \
  --image-policy allow_public \
  --gateway '127.0.0.1:28999'
```

这两条命令都会为 UEnv Worker 启用 SWE Runtime Gateway，并安装 OpenHands Agent 的运行依赖。`full` 会保留已安装的 UEnv Hub。

#### 多机：Adapter 和 UEnv Worker 在不同主机

本示例使用以下地址：

| 主机 | 地址 |
|---|---|
| Adapter 主机 | `10.0.0.10` |
| UEnv Worker 主机 | `10.0.0.21` |

第一步，在 Adapter 主机执行：

```bash
sudo uenv evaluate prepare-swe \
  --bundle /home/uenv-install/uenv-linux-x86_64.tar.gz \
  --profile control-plane \
  --shared-key-file /home/uenv-install/uenv-swe-shared.key
```

这条命令会生成 `/home/uenv-install/uenv-swe-shared.key`。通过团队规定的受保护文件传输方式，将该文件复制到 UEnv Worker 主机。在 UEnv Worker 主机将文件权限设为 `0600`：

```bash
sudo install -o root -g root -m 0600 \
  ./uenv-swe-shared.key \
  /home/uenv-install/uenv-swe-shared.key
```

第二步，在 UEnv Worker 主机执行：

```bash
sudo uenv evaluate prepare-swe \
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

| 参数 | 填写内容 |
|---|---|
| `--server` | UEnv Worker 访问 Adapter 的 gRPC 地址；`server` 是此参数的固定名称 |
| `--advertise` | Adapter 访问这台 UEnv Worker 的 gRPC 地址 |
| `--shared-key-file` | Adapter 和所有 UEnv Worker 共用的 SWE 共享密钥文件 |
| `--gateway` | SWE Runtime Gateway 在本机的监听地址 |
| `--gateway-public` | OpenHands Agent 可访问的 SWE Runtime Gateway URL；UEnv Worker 会把该 URL 登记到 Adapter |
| `--trajectory-endpoint` | 用于提交交互轨迹的 Adapter URL |

在基础多机端口之外，还需要为 SWE 开放以下内网访问：

| 来源 | 目标 | 端口 | 用途 |
|---|---|---:|---|
| UEnv Worker | Adapter | `50051/TCP`、`8077/TCP` | 注册、状态报告、结果和交互轨迹 |
| Adapter | UEnv Worker | `50054/TCP`、`28999/TCP` | 发送 Episode、访问 SWE Runtime Gateway |
| OpenHands Agent 所在主机 | UEnv Worker | `28999/TCP` | 操作 SWE 实例镜像 |

每增加一台 UEnv Worker，将 SWE 共享密钥复制到新 UEnv Worker，再重复 UEnv Worker 主机命令。每台 UEnv Worker 都要使用自己的 `--advertise` 和 `--gateway-public` 地址。

`--image-policy allow_public` 允许 UEnv Worker 在首次执行某个 SWE 实例时从容器镜像仓库（OCI Registry）拉取 SWE 实例镜像。

离线 UEnv Worker 需要先导入本次评测的 SWE 实例镜像，再将参数改为 `--image-policy local_only`。

`prepare-swe` 还会安装固定版本的 OpenHands 依赖（uv 创建独立环境，下载量约 1 GiB 以上）。两点注意：

- 依赖版本由 uv.lock 固定内容哈希。**不要**用 `UV_INDEX_URL` 指向与 PyPI 文件不一致的镜像，否则会因哈希校验失败；慢网络只需调大 `UV_HTTP_TIMEOUT`（默认 120 秒）。离线主机可在同架构、同 Python 版本的联网主机完成一次 `prepare-swe` 后，把 `/opt/uenv/agent/openhands-benchmarks` 和 `/var/lib/uenv/agent/.cache/uv` 原样复制过来再重跑。
- `prepare-swe` 不会创建常驻的 OpenHands Agent 服务；`run-swe` 会按需拉起 Agent。训练场景的常驻 Agent（`uenv-swe-agent.service`）由 `uenv train prepare-swe` 安装，见训练指南。

### 7.2 准备 SWE 输入

SWE 输入是 JSONL 文件。每行的 `instance_id` 选择 SWE catalog 中的一个实例：

```json
{"id":"astropy-7166","instance_id":"astropy__astropy-7166"}
{"id":"requests-1142","instance_id":"psf__requests-1142"}
```

UEnv 安装包中的示例文件是：

```text
/opt/uenv/current/examples/cases/evaluation/swe-verified.jsonl
```

使用自定义输入时，确认每个 `instance_id` 都存在于本次使用的 SWE catalog。

### 7.3 调用本地模型 API

在已启用 SWE Runtime 的 UEnv Worker 主机上执行。以下命令读取 Verified 样例输入，并写入两类结果：

- `results/$RUN_ID.jsonl`：每个 SWE 实例的汇总结果。
- `/var/lib/uenv/evaluation-runs/$RUN_ID`：每个 SWE 实例的评测运行文件。

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

`--batch-size 2` 表示最多同时执行两个 SWE 实例。单个实例失败时，其他实例继续执行；结果 JSONL 保持与输入相同的顺序。

`sudo` 用于读取 SWE 共享密钥和管理评测运行文件目录。命令会自动以 `uenv-agent` 账户运行 OpenHands Agent。

### 7.4 调用火山引擎方舟模型 API

仍然在启用了 SWE Runtime 的 UEnv Worker 主机上执行。`--model` 填写方舟推理接入点 ID：

```bash
RUN_ID="verified-ark-$(date +%Y%m%d-%H%M%S)"
sudo uenv evaluate run-swe \
  --provider volcengine \
  --model 'ep-xxxxxxxx' \
  --base-url 'https://ark.cn-beijing.volces.com/api/v3' \
  --gateway 'http://127.0.0.1:28999' \
  --catalog /opt/uenv/current/share/swe/verified.json \
  --benchmark-variant verified \
  --input /opt/uenv/current/examples/cases/evaluation/swe-verified.jsonl \
  --output "$PWD/results/$RUN_ID.jsonl" \
  --artifacts-dir "/var/lib/uenv/evaluation-runs/$RUN_ID" \
  --max-iterations 30 \
  --batch-size 2
```

命令会提示输入 API Key。在无人值守任务中，可以把 API Key 保存在权限受控的单行文件中，并在命令末尾增加：

```bash
--api-key-file /path/to/key
```

### 7.5 使用其他 SWE 数据集

完整流程包含四步。以 SWE-bench Pro 为例：

1. 从 [SWE-bench Pro 官方页面](https://huggingface.co/datasets/ScaleAI/SWE-bench_Pro) 获取 JSON 或 JSONL 数据。
2. 在存放数据文件、且已安装 `uenv` 命令的主机生成 SWE catalog：

   ```bash
   uenv evaluate build-swe-catalog \
     --variant pro \
     --input /data/official/swe-bench-pro.jsonl \
     --output /data/catalogs/swe-bench-pro.json
   ```

   JSON 和 JSONL 可以直接转换。Parquet 输入需要在执行命令的 Python 环境中安装 `pyarrow`。

3. 在每台启用了 SWE Runtime 的 UEnv Worker 主机安装 SWE catalog：

   ```bash
   sudo install -d -o root -g uenv -m 0750 /etc/uenv/catalogs
   sudo install -o root -g uenv -m 0640 \
     /data/catalogs/swe-bench-pro.json \
     /etc/uenv/catalogs/swe-bench-pro.json
   ```

   使用 `sudoedit /etc/uenv/swe.env`，找到现有的 `UENV_SWE_EXTRA_CATALOG` 行，并把它改为：

   ```text
   UENV_SWE_EXTRA_CATALOG=/etc/uenv/catalogs/swe-bench-pro.json
   ```

   旧版本配置中没有该字段时，只添加一次。

   然后重启 UEnv Worker：

   ```bash
   sudo systemctl restart uenv-worker.service
   ```

4. 创建输入 JSONL，其中的 `instance_id` 来自新 SWE catalog。执行 `run-swe` 时使用以下参数：

   ```text
   --catalog /etc/uenv/catalogs/swe-bench-pro.json
   --benchmark-variant pro
   --input /path/to/pro-input.jsonl
   ```

每台 UEnv Worker 可以配置一个主 SWE catalog 和一个额外 SWE catalog。需要使用多个自定义 SWE catalog 时，先按 `instance_id` 将它们合并为一个 JSON 文件。

## 8. 读取评测结果

### 8.1 `run-task` 结果

`run-task` 的输出 JSONL 与输入 JSONL 逐行对应：

| 字段 | 含义 |
|---|---|
| `status` | `completed` 表示 Episode 执行完成 |
| `reward` | 环境返回的得分 |
| `total_steps` | 实际交互步数 |
| `terminate_reason` | Episode 结束原因 |
| `steps` | 每步的环境返回内容（`observation`）、模型动作（`action`）、得分（`reward`）和附加信息（`info`） |
| `error_code` / `error_message` | 执行失败的阶段和错误内容 |

`status=completed` 且 `reward=0` 表示 Episode 已执行完成，但模型结果未通过环境判分。

### 8.2 `run-swe` 结果

| 字段 | 含义 |
|---|---|
| `case_id` / `instance_id` | 输入记录和 SWE catalog 实例 |
| `status` | 该 SWE 实例的执行状态 |
| `resolved` / `reward` | 仓库修复结果和得分 |
| `tests_passed` / `tests_total` | 通过的测试数和测试总数 |
| `artifact_dir` / `trajectory_id` | 该 SWE 实例的评测运行文件目录和交互轨迹 ID |
| `exit_code` / `error` | OpenHands Agent 退出状态和错误内容 |

某个 SWE 实例失败时，先在汇总 JSONL 查看 `error`，再使用 `artifact_dir` 查找该实例的日志。

## 9. 离线主机需要额外准备的文件

`run-task` 使用 UEnv Bridge 向 Adapter 提交 Episode。UEnv Bridge 是评测程序连接 Adapter 的 Python 组件。

首次执行时，脚本会在当前用户的目录中创建独立 Python 虚拟环境（venv），并安装 UEnv Bridge 的 Python 依赖。在可访问 Python 包索引的主机上，该过程自动完成。

离线主机需要事先准备与该主机的 Linux、CPU 架构和 Python 版本匹配的 Python 离线依赖目录（wheelhouse）。执行前设置 `UENV_EVAL_WHEELHOUSE` 为该目录，并在 `run-task` 命令末尾增加 `--offline`。

SWE 离线评测还需要事先导入本次评测需要的 SWE 实例镜像，并在 `prepare-swe` 命令中使用 `--image-policy local_only`。
