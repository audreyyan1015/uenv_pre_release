# SWE-bench-Pro UEnv 联调依赖说明

## 1. Adapter 侧边界

Adapter 侧负责把 SWE-bench-Pro 样本构造成 UEnv `EpisodeRequest`，通过 Rust adapter core 提交给 Server，并记录 `EpisodeResult`、请求日志和评测指标。对于 SWE-bench-Pro，当前推荐走 `env_type=swe`、`execution_mode=agent`、`mode=llm` 的 UEnv Agent 路线，由 Server/Worker/Agent 侧完成环境创建、OpenHands 执行、模型调用和结果回填。

Adapter 不应直接修改 Server、Worker、OpenHands Agent 的实现逻辑；如果联调发现非 adapter 问题，应先记录依赖项并交由对应模块处理。

## 2. 当前全量测试的非 Adapter 前置依赖

### 2.1 Worker 需要覆盖全量 SWE-bench-Pro 环境 catalog

全量 SWE-bench-Pro 测试集包含多条 instance。Adapter 可以逐条提交请求，但 Worker 侧必须能识别这些 `instance_id`，并能找到对应环境包、镜像、仓库状态和测试入口。

如果 Worker catalog 只预置少量样例，则全量运行会在 Server/Worker 侧返回类似 `instance_id not in catalog` 的错误。这不是 adapter payload 格式本身能解决的问题。

期望 Worker/Server 侧提供：

- 全量 SWE-bench-Pro instance catalog。
- 每个 instance 对应的环境包版本、镜像或按需拉取策略。
- catalog 覆盖率与缺失 instance 的可观测日志。

### 2.2 Agent 完成回调需要与 Server 协议一致

Server 对 Agent job completion 有身份校验时，OpenHands Agent 侧需要使用与 Server 一致的 proto/stub，并在 `CompleteAgentJob` 时回填注册后的 `agent_id`。否则可能出现 Agent 已经跑完任务，但 Server 不 ack，导致 job 无法被正确释放或计入结果。

期望 Agent/Server 侧对齐：

- `AgentJobCompleteRequest` 的字段定义。
- Agent 注册后使用 Server 返回的 `agent_id`。
- completion ack 失败时输出明确日志，包括 expected/report agent id。

### 2.3 模型服务需要支持 thinking 与较大输出长度

SWE-bench-Pro 的修复任务通常需要较长上下文和多轮工具调用。为了按“thinking 开启、max tokens 较大或参考官方值”的口径测试，Agent 使用的 LLM 配置需要明确：

- thinking 未被关闭。
- `max_output_tokens` / `max_tokens` 设置为较大值，例如 32768。
- 模型 endpoint 稳定可用，不应长期返回 backend starting / unavailable。
- OpenHands 的总运行超时、单次 LLM 请求超时与 max tokens 匹配。

### 2.4 并发容量和超时策略

全量 SWE-bench-Pro 运行耗时较长，Worker 与 Agent pool 需要明确容量限制。Adapter 可以设置 batch size 和 client timeout，但实际并发上限由 Server/Worker/Agent pool 决定。

期望 Server/Worker/Agent 侧提供：

- 当前 agent pool 容量。
- Worker 可并发 episode 数。
- 每个 instance 的运行超时和失败重试策略。

## 3. Adapter 侧可以继续做的工作

在不修改非 adapter 代码的前提下，Adapter 侧可以继续完善：

- SWE-bench-Pro UEnv 提交脚本。
- 请求与结果 JSONL 记录。
- `resolved`、reward、status、错误分布等指标汇总。
- 单样例 smoke 与全量运行命令文档。
- 对非 adapter 依赖的错误分类和证据记录。
