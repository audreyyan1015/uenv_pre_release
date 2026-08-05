# UEnv 评测指南

UEnv 不是某一个 benchmark 的评测脚本。它把每条样本组装为 Episode，交给指定环境执行，再返回 reward 和 trajectory。

每次评测都必须明确这 6 项：

| 参数 | 回答的问题 |
|---|---|
| `--endpoint` | Episode 提交到哪个 UEnv Adapter Core？ |
| `--env-type` | 用哪种交互和判分实现？ |
| `--dataset` | 该环境内使用哪个数据路由？ |
| `--input` | 本次要跑哪些样本？ |
| `--output` | 每条 Episode 的结果写到哪里？ |
| `--max-steps` | 每条 Episode 最多与环境交互几步？ |

`uenv evaluate run-task` 不会猜测任务。缺少任何一项都会报错，因此用户能从命令本身看出如何替换任务。

命令行中的 `env-type`、`dataset` 和 `max-steps` 是这一批任务的权威值。JSONL 可以重复写入相同字段以便文件自描述，但任何一行与命令不一致都会直接报错。不同环境、dataset 或步数的样本应拆成不同批次，不会在同一个输入中静默切换路由。

## 1. 开始前的状态

先完成 [UEnv 基础部署指南](./UEnv基础部署指南.md) 或 [UEnv 多机部署指南](./UEnv多机部署指南.md)。Adapter Core 必须能连到至少一台支持目标 `env_type` 的 Worker。

release 中包含的环境入口如下：

| `env_type` | 已有 `dataset` | 执行方式 |
|---|---|---|
| `qa` | `gsm8k`、`pubmedqa`、`scitab`、`olymmath*` | 单轮问答、分类或结果匹配 |
| `code` | `dscodebench` | 生成代码并运行任务提供的测试 |
| `swe` | Verified、Pro、Smith 等 catalog | 容器、Runtime Gateway 和 Agent |

`env_type` 是执行能力，`dataset` 是该能力内的数据和判分路由。一个新数据集不一定需要新 `env_type`。

## 2. 为 Worker 配置模型

QA、Code 和普通 process plugin 通常由 Worker 调用 OpenAI-compatible Chat Completions API 生成 action。模型服务不在 UEnv 中启动；应先由用户启动本地模型服务，或准备一个可用的 API。

无鉴权的本地 vLLM/SGLang 示例：

```bash
sudo uenv evaluate configure-model \
  --endpoint 'http://10.0.0.30:8000/v1' \
  --model 'Qwen/Qwen2.5-7B-Instruct' \
  --no-api-key
```

使用火山引擎方舟时，`--model` 填推理接入点 ID。命令会隐藏地读取 API Key，不要把密钥写进命令行：

```bash
sudo uenv evaluate configure-model \
  --endpoint 'https://ark.cn-beijing.volces.com/api/v3' \
  --model 'ep-xxxxxxxx'
```

这一配置要在每台可能承接任务的 Worker 上执行。参数发生变化时脚本会重启 Worker，因为 systemd 只在进程启动时读取模型环境文件；只改文件不会更新已运行进程。配置未变时不会重启，也不需要每次评测前重复配置。

`configure-model` 中的 `--endpoint` 是模型 API；下文 `run-task` 中的 `--endpoint` 是 UEnv Adapter Core。两者不是同一个地址。

## 3. QA 评测

以 release 中的 GSM8K 样例为例：

```bash
uenv evaluate run-task \
  --endpoint '127.0.0.1:50051' \
  --env-type qa \
  --dataset gsm8k \
  --input /opt/uenv/current/examples/evaluation/qa-gsm8k.jsonl \
  --output "$PWD/results/qa-gsm8k.jsonl" \
  --max-steps 1
```

QA JSONL 每行是一条样本：

```json
{"id":"case-1","env_type":"qa","dataset":"gsm8k","question":"What is 3 + 5? End with #### number.","target":"8","max_steps":1}
```

换成 PubMedQA 或自己的问答数据时，保持命令形状不变，只替换：

- `--dataset`：环境已注册的数据路由键；
- `--input`：新 JSONL；
- `--output`：新结果路径；
- 如果不再是单轮任务，同时修改 `--max-steps`。

如果新数据仍是“给定问题，抽取答案，与目标匹配”，应扩展 `qa` 的 dataset/backend，而不是另造一套评测脚本。

## 4. Code 评测

Code 使用与 QA 完全相同的入口，任务差异全部显式出现在参数和 JSONL 中：

```bash
uenv evaluate run-task \
  --endpoint '127.0.0.1:50051' \
  --env-type code \
  --dataset dscodebench \
  --input /opt/uenv/current/examples/evaluation/code-custom.jsonl \
  --output "$PWD/results/code.jsonl" \
  --max-steps 1
```

Code 记录用 `env_config` 携带执行代码所需的 harness：

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

换成其他代码任务时，修改问题、入口函数、测试或测试文件以及超时。如果交互仍是“生成代码后运行 harness”，保持 `env-type=code`；如果 observation/action/reward 都变了，应创建新环境。

Code 环境会执行模型产生的代码。不可信任务应使用隔离 Worker、受限文件系统和网络，或容器化 harness。

## 5. 自定义 process plugin 评测

假设已安装一个 `warehouse` 环境，其数据路由为 `warehouse-v1`：

```bash
uenv evaluate run-task \
  --endpoint '127.0.0.1:50051' \
  --env-type warehouse \
  --dataset warehouse-v1 \
  --input "$HOME/uenv-envs/warehouse/cases.jsonl" \
  --output "$PWD/results/warehouse.jsonl" \
  --max-steps 4
```

自定义记录的公共契约如下：

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

| 字段 | 作用 |
|---|---|
| `id` | 样本标识，用于对应结果 |
| `question` | 模型看到的任务说明 |
| `env_config` | 交给环境 `reset` 的初始配置 |
| `reward_config` | 交给环境的判分配置 |
| `target` | 有静态标准答案时使用；自身判分的插件可只用 `reward_config` |

上面三条命令的结构完全一致。迁移到新任务时，应先确定“复用已有环境”还是“实现新环境”，然后替换 `env-type + dataset + input`，而不是复制并隐藏整套命令。

## 6. 环境尚未支持时怎么做

| 新任务与已有环境的关系 | 应当做的修改 |
|---|---|
| 交互和 reward 语义相同，只是换数据 | 增加 dataset、数据转换和判分 backend |
| 仍是代码生成与测试 | 为 `code` 增加 dataset/harness |
| `reset/step/observation/action/reward` 语义不同 | 创建新 process plugin |
| 需要容器、外部 Agent 和长流程 | 实现专用 Runtime/Agent 适配，SWE 是现有参考 |

创建新 process plugin：

```bash
mkdir -p "$HOME/uenv-envs"
cd "$HOME/uenv-envs"
bash /opt/uenv/current/examples/environment/plugin.sh \
  create warehouse --dataset warehouse-v1
```

生成目录中，主要修改 `environment.py`；换样本时修改 `example.jsonl`；需要第三方 Python 库时追加 `requirements.txt`。`plugin.py`、`run.sh`、`uenv_plugin_api.py` 和 `generated/` 是 UEnv 通信层，不需要改。

实现后执行：

```bash
bash /opt/uenv/current/examples/environment/plugin.sh \
  test "$HOME/uenv-envs/warehouse"

sudo bash /opt/uenv/current/examples/environment/plugin.sh \
  install-local "$HOME/uenv-envs/warehouse"
```

`install-local` 用于当前 Worker 的开发和临时使用。需要团队共享、固定版本、多 Worker 同步或回滚时，再按 [UEnv Hub 使用指南](./UEnv%20Hub使用指南.md) 发布。Hub 只管理和分发环境制品，不参与 Episode 执行热路径。

## 7. SWE 评测

SWE 需要容器、Runtime Gateway、OpenHands、benchmark catalog 和任务镜像。QA、Code 和普通 process plugin 不需要本节操作。

### 7.1 准备 SWE Worker

先在 SWE Worker 上安装并启动 Docker 或 Podman。然后使用与已安装 UEnv 同版本的 bundle，显式填写部署角色、容器运行时、镜像策略和 Gateway：

```bash
sudo uenv evaluate prepare-swe \
  --bundle /home/uenv-install/uenv-linux-x86_64.tar.gz \
  --profile single-node \
  --runtime docker \
  --image-policy allow_public \
  --gateway '127.0.0.1:28999'
```

`allow_public` 允许在执行选定实例时拉取其镜像。离线节点应先导入目标镜像，并把策略改为 `local_only`。Gateway 需要被其它主机上的 Agent 访问时，改为内网监听地址，并增加 `--gateway-public 'http://<SWE_WORKER_IP>:28999'`。

这一步启用 SWE Runtime 并安装 release 固定的 OpenHands 依赖；它不下载模型、完整 benchmark 数据集或所有任务镜像。

### 7.2 调用本地模型 API

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

### 7.3 调用火山引擎方舟 API

```bash
sudo uenv evaluate run-swe \
  --provider volcengine \
  --model 'ep-xxxxxxxx' \
  --base-url 'https://ark.cn-beijing.volces.com/api/v3' \
  --gateway 'http://127.0.0.1:28999' \
  --catalog /opt/uenv/current/share/swe/verified.json \
  --benchmark-variant verified \
  --instance astropy__astropy-7166 \
  --output-dir "$PWD/results/swe-ark-astropy-7166" \
  --max-iterations 30
```

方舟模式会隐藏地提示输入 API Key。非交互执行可以使用权限受控的密钥文件并加上 `--api-key-file /path/to/key`。

SWE 中的 `catalog + benchmark-variant + instance` 明确决定题目和任务镜像。换用其他 SWE 数据时，必须同时换 catalog、variant 和 instance，不能只改模型名。

## 8. 如何读取结果

`run-task` 的输出 JSONL 与输入逐条对应：

| 字段 | 含义 |
|---|---|
| `status` | `completed` 表示 Episode 执行完成 |
| `reward` | 环境返回的得分 |
| `total_steps` | 实际交互步数 |
| `terminate_reason` | Episode 结束原因 |
| `steps` | 每步 observation、action、reward 和 info |
| `error_code` / `error_message` | 执行失败的阶段和原因 |

`status=completed` 且 `reward=0` 通常表示模型答案未通过环境判分，不是 UEnv 故障。`status` 未完成或出现 `error_code` 时，再根据错误阶段检查模型 API、Worker、插件或 Runtime。

`run-task` 首次执行会在当前用户目录创建隔离的 Python venv，并安装提交 Episode 所需的 Bridge 依赖。这不会修改系统 Python，也不会下载 benchmark 数据、模型或容器镜像。完全离线主机需要事先准备与目标 Linux、CPU 架构和 Python 版本匹配的 wheelhouse，再设置 `UENV_EVAL_WHEELHOUSE` 并加 `--offline`。
