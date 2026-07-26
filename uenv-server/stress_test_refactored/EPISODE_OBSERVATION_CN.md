# Episode 观测结构说明

## 1. 文档目的

本文介绍重构后压测代码中的统一 Episode 观测结构
`EpisodeObservation v1`。

本文聚焦逐 Episode 原始事实。导师关注的每数据集、每并行模式、
每 Worker、Worker×数据集、负载分布、replay 命中率、提交频率偏差、
资源采样和清理结果，统一由第二层 `SuiteMetrics v1` 负责，详见
`SUITE_METRICS_CN.md`。

该结构同时服务于两套测试：

1. 规模压测：观察不同 Worker 数量、Worker 容量和并行模式下的吞吐、
   调度、时延、错误及训练轨迹完整性。
2. 正式稳定性验收：观察长时间运行中的 Episode 生命周期、超时、迟到、
   重复终态、UEnv 错误和结果完整性。

两套测试的负载时序和验收标准仍然不同，但每个 Episode 的观测字段、
字段含义和失败分类保持一致。这样可以直接对比两套测试的结果，也可以
在同一套分析程序中计算吞吐、时延、失败率、数据集分布和 Worker 负载。

统一结构只位于压测代码目录：

```text
/home/uenv-scale-bench-merge/uenv-server/stress_test_refactored
```

它没有修改 UEnv Server、Worker、Adapter 协议或生产服务代码。

## 2. 核心设计原则

### 2.1 每个已提交 Episode 对应一条观测

观测分母以实际提交的 `SampleEnvelope` 为准。

- 正常返回：记录 SampleResult 的状态、奖励、轨迹和错误。
- RPC 失败：仍然为批次中的每个已提交 Episode 生成一条记录。
- 没有返回结果：生成 `missing_result` 记录。
- 返回重复终态：保留一条 Episode 记录，并用 `terminal_count` 和
  `duplicate_terminal_result` 标记。
- 返回未知 request ID：记入压测结果文档的协议异常列表，但不伪造
  Episode 观测，因为它无法与任何已提交 SampleEnvelope 建立绑定。

因此，正常情况下应满足：

```text
EpisodeObservation 行数 = submitted Episode 数
```

### 2.2 三种存储格式使用同一组字段

结构采用扁平字段设计，可以无损写入：

- JSONL：规模压测的逐 Episode 结果；
- SQLite：稳定性验收的权威持久化账本；
- CSV：需要人工检查或使用表格工具分析时导出。

场景特有字段统一放入 `extensions_json`，避免为不同数据集建立不同表。

### 2.3 事实缺失时留空，不推测

当前 Adapter `SampleResult` 不返回 Worker ID、Worker 主机、
dispatch lease 或 attempt ID。统一结构保留了这些字段，但当前值为空，
同时设置：

```text
worker_attribution = unavailable_in_adapter_result
```

这表示“当前协议结果无法归属”，而不是“没有 Worker 执行”。当前的
Worker 覆盖率和每台 Worker 完成量仍需从隔离 Server 日志中统计。

## 3. 总体结构

`EpisodeObservation v1` 共 51 个字段，可分为八组：

| 分组 | 作用 |
|---|---|
| 契约与场景 | 标识结构版本、测试套件、阶段、数据集和并行模式 |
| Episode 身份 | 将 Episode、请求、批次和数据集样本绑定起来 |
| 生命周期与时延 | 记录计划、提交、截止、终态时间及派生时延 |
| 结果与失败 | 记录状态、奖励、终止原因、错误和失败分类 |
| Worker 归属 | 预留 Worker、主机、lease 和 attempt 归属 |
| 轨迹回放 | 记录回放策略以及可用时的轨迹选择信息 |
| 训练轨迹与校验 | 记录步数、token、训练轨迹完整性和结果校验和 |
| 扩展字段 | 保存不属于公共结构的场景信息 |

## 4. 字段详细说明

### 4.1 契约与场景字段

| 字段 | 类型 | 含义 |
|---|---|---|
| `schema_version` | 整数 | 当前固定为 `1`，用于控制结构演进 |
| `observation_type` | 字符串 | 当前固定为 `episode` |
| `suite` | 字符串 | `scale` 或 `stability` |
| `run_id` | 字符串 | 本次压测或验收运行的唯一 ID |
| `phase` | 字符串 | 规模压测通常为并行模式；稳定性为 selfcheck、reference、stability、capacity 或 burst 等阶段 |
| `task` | 字符串 | 统一任务名 |
| `dataset` | 字符串 | 数据集名 |
| `environment` | 字符串 | UEnv 环境类型，例如 code、swe、math 或 swe_openhands |
| `parallel_mode` | 字符串 | `sync`、`one_step_off_policy` 或 `fully_async` |
| `arrival_mode` | 字符串 | 稳定性负载到达方式，例如 constant、poisson 或 batch；不适用时为空 |

`task` 和 `dataset` 当前通常相同，但保留两个字段是为了区分“测试任务”
和“数据来源”。例如未来同一数据集可以形成多个验收任务，而不用修改结构。

### 4.2 Episode 身份字段

| 字段 | 类型 | 含义 |
|---|---|---|
| `episode_id` | 字符串 | Episode 唯一 ID |
| `request_id` | 字符串 | Adapter SampleEnvelope 的 request ID |
| `batch_id` | 字符串 | Episode 所属 ExecuteBatch 批次 |
| `sample_index` | 整数 | Episode 在批次中的样本序号 |
| `sequence` | 整数 | Episode 在本次任务流中的全局序号；未知时为 `-1` |
| `dataset_item_id` | 字符串 | DSCodeBench problem ID 或 Math 数据集条目 ID |
| `instance_id` | 字符串 | SWE-bench Pro instance ID |

当前约束为：

```text
episode_id = request_id
```

这使普通 Code/Math 回放请求和 OpenHands 回放请求都可以使用同一个 ID
绑定整条轨迹。

### 4.3 生命周期与时延字段

| 字段 | 类型 | 单位 | 含义 |
|---|---|---|---|
| `planned_at` | 浮点数 | Unix 秒 | 负载生成器创建并登记 Episode 的时间 |
| `dispatch_started` | 布尔值 | - | 是否已经开始向 Adapter 提交 |
| `dispatched_at` | 浮点数 | Unix 秒 | Episode 成功写入请求流或开始批次 RPC 的时间 |
| `deadline` | 浮点数 | Unix 秒 | `dispatched_at + timeout_seconds` |
| `terminal_at` | 浮点数 | Unix 秒 | 收到结果、确认 RPC 失败或超时对账的时间 |
| `timeout_seconds` | 浮点数 | 秒 | 单个 Episode 的超时配置 |
| `end_to_end_ms` | 浮点数 | 毫秒 | 从提交到终态的 Episode 端到端时间 |
| `batch_rpc_latency_ms` | 浮点数 | 毫秒 | Episode 所属 ExecuteBatch RPC 的整体耗时 |

`end_to_end_ms` 优先从 `dispatched_at` 开始计算；没有提交时间时才回退到
`planned_at`。

规模压测使用批量 `ExecuteBatch`，因此同一批次内多个 Episode 的
`batch_rpc_latency_ms` 相同。稳定性验收使用流式提交，主要使用
`end_to_end_ms` 分析单 Episode 时延。

### 4.4 结果与失败字段

| 字段 | 类型 | 含义 |
|---|---|---|
| `status` | 字符串 | 当前生命周期或 SampleResult 状态 |
| `done` | 布尔值 | 环境是否已完成 |
| `reward` | 浮点数 | Episode 奖励 |
| `termination_reason` | 字符串 | Episode 终止原因 |
| `error_code` | 字符串 | UEnv、协议、RPC 或对账错误码 |
| `error_message` | 字符串 | 错误详情 |
| `failure_class` | 字符串 | 跨两套测试统一的失败分类 |
| `terminal_count` | 整数 | 收到的终态结果数量 |

常见 `status` 值如下：

| 状态 | 含义 |
|---|---|
| `planned` | 已登记但尚未提交 |
| `dispatched` | 已提交，等待结果 |
| `completed` / `success` | Adapter 返回成功终态 |
| `missing_result` | 批次返回，但该 Episode 没有对应结果 |
| `rpc_error` | 批次 RPC 失败 |
| `timeout` | 稳定性对账时仍没有终态 |

统一 `failure_class` 的主要取值如下：

| 失败分类 | 含义 |
|---|---|
| `pending` | 尚未形成终态 |
| `none` | 正常成功 |
| `uenv_error` | UEnv 返回错误状态或结果不完整 |
| `test_config_error` | 请求、配置或协议输入错误，正式验收应判为无效运行 |
| `late_result` | 结果在 Episode deadline 之后到达 |
| `no_terminal_result` | 对账宽限期结束后仍无终态 |
| `duplicate_terminal_result` | 同一 request ID 收到多个终态 |
| `rpc_error` | ExecuteBatch RPC 失败 |

成功状态并不自动等于结果完全有效。稳定性验收还会检查轨迹和结果校验和；
非同步训练模式还会检查训练轨迹字段。

### 4.5 Worker 归属字段

| 字段 | 类型 | 含义 |
|---|---|---|
| `worker_id` | 字符串 | 执行 Episode 的 Worker ID |
| `worker_host` | 字符串 | Worker 所在主机 |
| `dispatch_lease_id` | 字符串 | 调度 lease ID |
| `attempt_id` | 字符串 | 重试或执行 attempt ID |
| `worker_attribution` | 字符串 | Worker 归属信息的来源或不可用原因 |

当前五个数据集的 Adapter 结果中，这四个归属值通常为空。若未来 Adapter
协议直接返回这些字段，或实现经过校验的 Server 日志关联，只需填充值，
不需要升级观测结构。

当前阶段不能只依靠 EpisodeObservation 计算“每台 Worker 处理多少
Episode”。正式报告应同时引用 Server 日志生成的 Worker dispatch
coverage 统计，避免把未知归属误写为均匀分配。

### 4.6 轨迹回放字段

| 字段 | 类型 | 含义 |
|---|---|---|
| `replay_strategy` | 字符串 | 当前五个数据集统一为 `round_robin_episode` |
| `trace_id` | 字符串 | 分配给该 Episode 的轨迹 ID |
| `trace_slot` | 整数 | 轨迹在数据集 corpus 中的位置；未知时为 `-1` |
| `trace_corpus_size` | 整数 | 对应数据集的轨迹总数；未知时为 `0` |

当前结构已经统一记录回放策略。实际的 `trace_id`、`trace_slot` 和
`trace_corpus_size` 只有在 Envelope 上下文或结果链路提供逐 Episode
选择信息时才能填写；否则保持空值，并由 replay 统计产物记录 corpus
大小、轮转次数和每条轨迹使用次数。

回放语义为：

1. 五个数据集分别维护独立游标。
2. 每个新 Episode 取得下一条已采集轨迹。
3. 到达 corpus 末尾后回到第一条轨迹。
4. DSCodeBench 和 SWE-bench Pro 在一个 Episode 内固定使用同一条多轮轨迹。
5. OlymMATH、SciTab 和 PubMedQA 为单轮轨迹，每个 Episode 推进一次游标。

### 4.7 训练轨迹与结果校验字段

| 字段 | 类型 | 含义 |
|---|---|---|
| `actual_steps` | 整数 | trajectory 中记录的实际环境步数 |
| `response_tokens` | 整数 | 汇总后的 response token ID 数量 |
| `training_trace_valid` | 布尔值 | 训练轨迹是否满足完整性要求 |
| `training_trace_errors_json` | JSON 字符串 | 训练轨迹校验错误列表 |
| `rollout_param_version` | 字符串 | rollout 参数版本 |
| `rollout_policy_version` | 字符串 | rollout 策略版本 |
| `result_checksum` | 字符串 | 结果校验和，使用 SHA-256 |
| `result_checksum_valid` | 布尔值 | 是否存在可校验轨迹且校验和格式有效 |

`training_trace_valid` 的当前检查包括：

- `trajectory_json` 存在且可以解析；
- 存在 `response_ids`；
- 存在 `response_mask`；
- `response_ids` 与 `response_mask` 长度一致；
- 存在 `rollout_log_probs`；
- token 数量与 log probability 数量一致。

`result_checksum` 对以下内容的稳定 JSON 表示计算 SHA-256：

```text
status + error_code + trajectory
```

它用于发现重复终态、结果变化和持久化损坏，不用于评价模型答案质量。

### 4.8 扩展字段

| 字段 | 类型 | 含义 |
|---|---|---|
| `extensions_json` | JSON 字符串 | 不属于公共契约的 SampleContext 字段 |

可能进入扩展区的内容包括：

- `max_steps`；
- 历史 gate 标记；
- 规模证据标记；
- SWE/OpenHands 并发参数；
- 场景特有的执行参数。

`extensions_json` 和 `training_trace_errors_json` 在 JSONL 中仍然是字符串，
这是为了保证 JSONL、SQLite 和 CSV 三种格式使用相同的扁平列结构。

## 5. 两套测试如何写入

### 5.1 规模压测

DSCodeBench、SWE-bench Pro 和 Math 规模客户端都调用同一个
`observe_episode_batch`：

```text
SampleEnvelope 列表
        +
SampleResult 列表或 RPC 异常
        ↓
每个已提交 Episode 一条 EpisodeObservation
        ↓
*.episode-observations.jsonl
```

五数据集对应关系如下：

| 数据集 | 规模客户端 |
|---|---|
| DSCodeBench | DSCodeBench pressure client |
| SWE-bench Pro | SWE/OpenHands pressure client |
| OlymMATH | Math rule pressure client |
| SciTab | Math rule pressure client |
| PubMedQA | Math rule pressure client |

每个规模结果 JSON 还包含：

```json
{
  "episode_observations": {
    "schema_version": 1,
    "artifact_path": "...episode-observations.jsonl",
    "local_artifact": ".../episode-observations-*.jsonl",
    "row_count": 10240,
    "submitted_count": 10240,
    "complete": true,
    "worker_attribution": "unavailable_in_adapter_result"
  }
}
```

`complete=true` 只表示每个已提交 Episode 都有观测行，不表示压测通过。
是否通过仍需同时检查失败数、协议错误、Worker 覆盖、容量波次和轨迹回放。

### 5.2 正式稳定性验收

稳定性验收不把长时间运行的全部记录保存在内存中，而是逐条写入：

```text
episode.sqlite
└── episode 表：51 个 EpisodeObservation 字段
```

其生命周期为：

```text
plan → dispatched → terminal
                    └→ reconcile timeout
```

- `plan`：创建完整结构并写入计划信息。
- `dispatched`：记录提交时间和 deadline。
- `terminal`：写入结果、轨迹、校验和和失败分类。
- `reconcile`：为没有终态的 Episode 写入 timeout/no_terminal_result。

使用 `--export-episode-csv` 时，运行结束后同时导出：

```text
episode.csv
episode-observations.jsonl
```

对于 72 小时运行，`episode.sqlite` 是权威数据；JSONL/CSV 导出是可选的，
避免不必要地重复占用大量磁盘空间。

## 6. JSONL 示例

下面是一条成功 Episode 的示例。为强调实际存储格式，两个扩展字段展示为
JSON 字符串：

```json
{
  "schema_version": 1,
  "observation_type": "episode",
  "suite": "scale",
  "run_id": "scale-20260726-example",
  "phase": "fully_async",
  "task": "DSCodeBench",
  "dataset": "DSCodeBench",
  "environment": "code",
  "parallel_mode": "fully_async",
  "arrival_mode": "",
  "episode_id": "episode-000001",
  "request_id": "episode-000001",
  "batch_id": "batch-0001",
  "sample_index": 0,
  "sequence": 0,
  "dataset_item_id": "problem-42",
  "instance_id": "",
  "planned_at": 1785031200.100,
  "dispatch_started": true,
  "dispatched_at": 1785031200.120,
  "deadline": 1785031380.120,
  "terminal_at": 1785031202.420,
  "timeout_seconds": 180.0,
  "end_to_end_ms": 2300.0,
  "batch_rpc_latency_ms": 2310.0,
  "status": "completed",
  "done": true,
  "reward": 1.0,
  "termination_reason": "done",
  "error_code": "",
  "error_message": "",
  "failure_class": "none",
  "terminal_count": 1,
  "worker_id": "",
  "worker_host": "",
  "dispatch_lease_id": "",
  "attempt_id": "",
  "worker_attribution": "unavailable_in_adapter_result",
  "replay_strategy": "round_robin_episode",
  "trace_id": "",
  "trace_slot": -1,
  "trace_corpus_size": 0,
  "actual_steps": 3,
  "response_tokens": 96,
  "training_trace_valid": true,
  "training_trace_errors_json": "[]",
  "rollout_param_version": "1",
  "rollout_policy_version": "dscodebench-policy-1",
  "result_checksum": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
  "result_checksum_valid": true,
  "extensions_json": "{\"max_steps\":8}"
}
```

## 7. 推荐统计口径

统一结构可以直接支持以下统计：

### 7.1 Episode 完整性

```text
观测完整率 = EpisodeObservation 行数 / submitted 数
```

该指标应为 100%，否则说明压测客户端自身丢失了观测。

### 7.2 成功率与失败率

```text
成功率 = failure_class=none 的 Episode 数 / 已提交 Episode 数
失败率 = failure_class!=none 且 failure_class!=pending 的 Episode 数 / 已提交 Episode 数
```

应按 `failure_class` 分组报告，不能只给出一个总失败数。

### 7.3 时延

建议分别报告：

- `end_to_end_ms` 的 p50、p95、p99；
- `batch_rpc_latency_ms` 的 p50、p95、p99；
- 按数据集、并行模式和阶段分组的时延；
- 稳定性运行不同时段的时延变化。

### 7.4 Worker 负载

当 `worker_id` 有可靠值时：

```text
每台 Worker 处理量 = 按 worker_id 分组的 Episode 数
```

当前协议下应使用 Server 日志的 Worker dispatch coverage 作为权威来源，
不能根据 Worker 数量平均分摊 Episode。

### 7.5 数据集与轨迹覆盖

可按以下字段统计：

- `dataset`：五个数据集的提交量和成功率；
- `dataset_item_id` / `instance_id`：样本复用次数；
- `trace_id` / `trace_slot`：逐轨迹使用次数，前提是结果链路已提供这些值；
- replay 统计产物：corpus 大小、轮转周期、next slot 和 selection count。

## 8. 当前边界

统一 Episode 观测结构已经解决了“两套测试字段不同、失败分母不同、
结果难以对比”的问题，但仍有三个明确边界：

1. 当前 Adapter 结果没有 Worker/lease/attempt 归属，需继续使用 Server
   日志提供 Worker 负载证据。
2. 当前逐 Episode 的 trace ID 不一定能从结果链路返回，回放轮转仍以
   replay 统计产物为权威证据。
3. `batch_rpc_latency_ms` 是批次级时延；单 Episode 调度和执行耗时需要
   `end_to_end_ms` 或未来更细的 Server/Worker 时间戳。

这些边界均已在结构中预留字段，后续补充数据来源时不需要再次改变
JSONL、SQLite 和 CSV 的列定义。

## 9. 代码位置

| 内容 | 路径 |
|---|---|
| 字段定义、校验和 JSONL 序列化 | `uenv_stress/core/stress_test_common.py` |
| 稳定公共导出接口 | `uenv_stress/core/result.py` |
| 稳定性 SQLite Ledger | `uenv_stress/cli/run_formal_stability_suite.py` |
| DSCodeBench 规模接入 | `uenv_stress/scale/dscodebench_pressure.py` |
| SWE-bench Pro 规模接入 | `uenv_stress/scale/swebench_pro_pressure.py` |
| 三个 Math 数据集规模接入 | `uenv_stress/scale/rule_task_pressure.py` |
| 契约与覆盖测试 | `tests/test_episode_observation.py` |
