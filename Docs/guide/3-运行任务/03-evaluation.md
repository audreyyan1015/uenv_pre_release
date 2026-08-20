# 通用评测流程

本页说明普通 Episode 评测（`run-task`）的完整链路：提交样本、逐条获得终态结果。需要实例容器的 SWE 评测见[代码修复](./06-evaluation-swe-verified.md)。两条路径的共同结果都是“一条输入对应一条终态结果”；`status=completed` 表示基础设施完整执行，reward、测试或 `resolved` 才表示任务质量。

## 先确定执行路径

| 输入是什么 | 是否使用 Agent | 使用入口 | 命令在哪台主机执行 |
|---|---|---|---|
| 问答、代码生成、自定义 process plugin 的 JSONL | 否，UEnv Worker 直接调用模型完成单步执行 | `uenv evaluate run-task` | 任意安装了 `uenv` 且能访问 UEnv Server 的客户端主机 |
| catalog 中的代码修复实例 | 是，固定版本的 OpenHands Agent 在实例容器中多步修改并运行测试 | `sudo uenv evaluate run-swe` | 已启用 SWE Runtime 的 UEnv Worker 主机；完整流程见[代码修复](./06-evaluation-swe-verified.md) |

`run-task` 覆盖输入、执行和结果的最小链路，本页后续内容只讲这条路径；当任务涉及仓库、容器、patch 与测试时，SWE Runtime、catalog、variant 和 Agent 运行细节统一在[代码修复](./06-evaluation-swe-verified.md)中说明。SWE 当前通过固定版本的 OpenHands Agent 执行修改，它是 SWE 的运行实现。

三类内置任务的本质区别：

| 维度 | 问答（`qa`） | 代码生成（`code`） | 代码修复（SWE） |
|---|---|---|---|
| 任务内容 | 回答一个问题（数学、生物医学、表格问答等，由 `dataset` 路由） | 按题目生成一段代码 | 在真实代码仓库中修复一个问题 |
| 谁调用模型 API | UEnv Worker 进程，使用 `configure-model` 持久化在 Worker 上的模型配置 | UEnv Worker 进程，同样使用 `configure-model` 持久化在 Worker 上的模型配置 | 实例容器内的 OpenHands Agent，使用 `run-swe` 命令行传入的模型参数，只作用于当次运行 |
| 交互步数 | 单步：模型一次性给出答案 | 单步：模型一次性给出代码 | 多步：Agent 反复查看代码、修改、运行测试，最多 `--max-iterations` 轮 |
| 执行环境 | Worker 上的 process plugin | Worker 上的 process plugin，会实际执行模型生成的代码（生产环境应配置隔离） | 每个实例一个容器，镜像由 catalog 指定 |
| 输入怎么写 | JSONL 每行一条样本：`question` + `target` | JSONL 每行一条样本：`question` + `env_config.test_code` | JSONL 每行选择一个实例：`instance_id` 指向 catalog |
| 怎么判分 | 规则比对最终答案与 `target`（如 `#### 数字` 格式） | 运行 `test_code` 中的断言，通过与否 | 在仓库中应用补丁并运行测试，产出 `resolved` |
| 需要的组件 | UEnv Server + UEnv Worker | UEnv Server + UEnv Worker，建议为 Worker 配置受限执行环境 | UEnv Server + UEnv Worker + SWE Runtime（Runtime Gateway、catalog、实例镜像） |

## 检查共同前置条件

先完成[使用前检查](./01-usage.md#开始前的可执行检查)。然后确认至少有一台 UEnv Worker 支持本次任务的环境类型（如 `qa`）。在 UEnv Server 主机执行：

```bash
uenv workers
curl -fsS http://127.0.0.1:50052/status | \
  jq -r '.workers[] | [.endpoint, .status, (.supported_env_types | join(","))] | @tsv'
```

第一条命令确认有 `ready` 且负载未满的 UEnv Worker；第二条命令每台 Worker 输出一行，第三列是它声明支持的环境类型，例如：

```text
10.0.0.21:50054	ready	qa,math,code
10.0.0.22:50054	ready	qa,my-environment
```

要运行 `env_type` 为 `qa` 的任务，至少一台 `ready` Worker 的第三列必须包含 `qa`。若没有，先修复 UEnv Worker 注册、安装相应 EnvPackage 或 process plugin；不要靠重复提交绕过调度失败。

## 配置并检查模型 API

`run-task` 默认由 UEnv Worker 调用 OpenAI-compatible 模型 API。本地部署和云端 API 两种模型来源都支持，在每台可能接单的 UEnv Worker 上配置一次即可。

本地部署的模型服务（vLLM、SGLang 等，通常无鉴权）：

```bash
export MODEL_API='http://10.0.0.30:8000/v1'
export MODEL_NAME='Qwen/Qwen2.5-7B-Instruct'

curl -fsS "$MODEL_API/models" >/dev/null
sudo uenv evaluate configure-model \
  --endpoint "$MODEL_API" \
  --model "$MODEL_NAME" \
  --no-api-key
sudo systemctl is-active uenv-worker.service
```

云端模型 API（以火山引擎方舟为例，`--model` 填方舟的推理接入点 ID）：

```bash
export MODEL_API='https://ark.cn-beijing.volces.com/api/v3'
export MODEL_NAME='ep-xxxxxxxx'

sudo uenv evaluate configure-model \
  --endpoint "$MODEL_API" \
  --model "$MODEL_NAME" \
  --api-key-file ./ark-api-key.txt
sudo systemctl is-active uenv-worker.service
```

云端 API 需要密钥：用 `--api-key-file` 从权限为 `0600` 的单行文件读取，或省略该参数按提示交互输入。密钥写入 UEnv Worker 的受保护配置，不进入 JSONL、命令历史或 trajectory。`configure-model` 在配置变化时会自动重启 UEnv Worker；配置相同则保持不变、不重启。并非所有兼容服务都实现 `GET /models`；此时以该服务自己的健康请求替换 `curl`，但必须从 UEnv Worker 主机发起。

`configure-model --endpoint` 是模型 HTTP API 地址；后文 `run-task --endpoint` 是 UEnv Server gRPC 地址，二者分别对应两个不同端口。

SWE 的模型调用方不是 UEnv Worker 主进程，而是实例容器中的 OpenHands Agent，因此模型配置不经过 `configure-model`，在 `run-swe` 命令行上直接声明 provider；两种模型来源的完整命令见[代码修复](./06-evaluation-swe-verified.md#配置模型-api)。

## 准备输入

输入是 UTF-8 JSONL，每个非空行是一个 JSON 对象：

| 字段 | 是否必需 | 含义 |
|---|---|---|
| `id` | 是 | 输入文件内唯一的业务样本 ID |
| `env_type` | 是 | 环境类型，必须与命令行一致 |
| `dataset` | 是 | 环境内的判分路由，必须与命令行一致 |
| `question` | 按任务 | 给模型的任务说明 |
| `target` | 至少一项 | 静态答案；与 `reward_config` 至少提供一种 |
| `env_config` | 按任务 | 环境初始化、测试代码或任务配置 |
| `reward_config` | 至少一项 | 规则、rubric 或 plugin 判分配置 |
| `max_steps` | 是 | Episode 最大步数，必须与命令行一致 |

安装包提供的数学问答文件是用于说明字段和执行链路的自拟示例：

```bash
export UENV_RELEASE_ROOT='/opt/uenv/current'
export INPUT="$UENV_RELEASE_ROOT/examples/cases/evaluation/qa-gsm8k.jsonl"

test -r "$INPUT"
jq -e -c . "$INPUT" >/dev/null
wc -l "$INPUT"
```

一次 `run-task` 只运行一种 `env_type`、一种 `dataset` 和一种 `max_steps`。不同任务拆成不同输入与运行 ID。

## 执行评测

下面的命令直接运行安装包中的两条数学问答示例。把 `UENV_SERVER_ENDPOINT` 改成实际地址；单机部署保留 `127.0.0.1`：

```bash
export UENV_SERVER_ENDPOINT='127.0.0.1:50051'
export UENV_RELEASE_ROOT='/opt/uenv/current'
export INPUT="$UENV_RELEASE_ROOT/examples/cases/evaluation/qa-gsm8k.jsonl"
export RUN_ID="evaluation-$(date +%Y%m%d-%H%M%S)"
export OUTPUT="$PWD/results/$RUN_ID/results.jsonl"

test -r "$INPUT"
mkdir -p "$(dirname "$OUTPUT")"

uenv evaluate run-task \
  --endpoint "$UENV_SERVER_ENDPOINT" \
  --env-type qa \
  --dataset gsm8k \
  --input "$INPUT" \
  --output "$OUTPUT" \
  --max-steps 1 \
  --batch-size 2 \
  --streaming
```

正常结束时终端打印 `cases`、`completed`、`failed`、`mean_reward` 和输出路径。`--streaming` 允许结果按完成顺序返回，因此用 ID 关联输入，不按行号假设顺序。

验收基础设施结果：

```bash
jq -e -s '
  length == 2 and
  (map(.case_id) | unique | length) == 2 and
  all(.[]; .status == "completed" and (.reward | type) == "number")
' "$OUTPUT" >/dev/null && echo 'evaluation completed'
```

这条命令确认两条 Episode 完整执行且有数值 reward；模型得分高低取决于模型本身。查看任务质量：

```bash
jq -c '{case_id,status,reward,answer:(.steps[-1].action // ""),error_message}' "$OUTPUT"
```

## 结果字段与完成标准

`run-task` 每行稳定包含 `case_id`、`request_id`、`env_type`、`status`、`error_code`、`error_message`、`reward`、`total_steps`、`terminate_reason`、`trajectory_id` 和内联 `steps`。SWE 的结果字段见[代码修复](./06-evaluation-swe-verified.md#结果与验收)。

一次评测完成必须同时满足：

1. 每个输入 `id` 恰好有一条终态结果。
2. 没有未知、重复或缺失的请求/实例 ID。
3. 基础设施状态与任务得分分别统计。
4. 输入、结果和逐实例产物可通过同一个 `RUN_ID` 追溯。

## 失败定位

| 现象 | 首个检查 | 日志或下一步 |
|---|---|---|
| 无法连接 UEnv Server | 客户端到 50051/TCP | `uenv logs server -n 200` |
| 没有可调度 UEnv Worker | `uenv workers` 中的状态、环境能力和容量 | `journalctl -u uenv-worker.service` |
| 模型调用失败 | 从 UEnv Worker 到模型 API 的网络、模型名和密钥 | UEnv Worker journal 与模型服务日志 |
| `env_type` / `dataset` 不匹配 | 命令与每行 JSON 声明 | 修正输入，不使用默认值掩盖 |
| Episode completed 但低分 | target、判分配置、测试和 trajectory | 不要重装服务 |
| SWE 相关问题 | 见[代码修复](./06-evaluation-swe-verified.md#失败定位) | — |

## 继续到案例库

- [数学问答评测](./04-evaluation-gsm8k.md)
- [代码生成评测](./05-evaluation-code.md)
- [代码修复评测](./06-evaluation-swe-verified.md)

实现自定义环境应使用发行包中的 process plugin 模板；只有接入新的强化学习框架时才阅读[自定义强化学习框架接入](../4-接入强化学习框架/01-custom-framework.md)。
