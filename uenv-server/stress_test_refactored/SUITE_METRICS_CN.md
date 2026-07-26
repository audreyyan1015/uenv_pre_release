# 统一测试汇总指标 SuiteMetrics 说明

## 1. 为什么还需要 SuiteMetrics

`EpisodeObservation` 解决的是单个 Episode 的原始事实记录，例如：

- 提交和终态时间；
- 成功、失败和错误；
- 轨迹、步数和训练字段；
- 数据集、并行模式和批次归属。

导师和验收人员真正关心的是系统层面的汇总结论，例如：

- 每个数据集表现如何；
- 三种并行模式分别表现如何；
- 每台 Worker 实际处理了多少 Episode；
- 每台 Worker 在各数据集上处理了多少 Episode；
- Worker 负载是否均衡；
- replay 是否全部命中；
- 实际提交频率是否符合计划；
- 资源采样是否完整；
- 测试资源是否清理干净。

因此当前压测代码采用两层结构：

```text
EpisodeObservation v1
  └─ 每个已提交 Episode 一条原始记录

SuiteMetrics v1
  └─ 从 Episode、Server 日志、replay、资源和清理证据汇总
```

两套测试都生成同名产物：

```text
suite-metrics.json
```

## 2. SuiteMetrics 的统一结构

顶层结构如下：

| 字段 | 含义 |
|---|---|
| `schema_version` | 当前固定为 `2` |
| `metrics_type` | 固定为 `suite_metrics` |
| `suite` | `scale` 或 `stability` |
| `run_id` | 本次测试运行 ID |
| `phase` | 稳定性阶段；规模压测可不填 |
| `overall` | 全部 Episode 的总体指标 |
| `by_dataset` | 每数据集指标 |
| `by_parallel_mode` | 每并行模式指标 |
| `by_dataset_parallel_mode` | 数据集×并行模式交叉指标 |
| `by_worker` | 每 Worker 指标 |
| `by_worker_dataset` | 每 Worker×数据集指标 |
| `worker_load_distribution` | Worker 负载分布 |
| `replay` | replay 调用、命中和未命中统计 |
| `submission_rate` | 总体计划/实际提交频率 |
| `resources` | 资源采样结果 |
| `cleanup` | 清理和残留检查结果 |
| `data_quality` | 各类指标的数据来源与完整性 |
| `complete` | 导师关注的必需指标是否全部有可靠证据 |

当必需指标缺失时，不能用 `0` 冒充真实值。统一使用：

```json
{
  "available": false,
  "reason": "缺少该指标的可靠数据来源"
}
```

完整套件执行时，如果 `complete=false`，套件不能报告为正式通过。

## 3. 每数据集指标

`by_dataset` 固定包含五个数据集：

```text
dscodebench
swebench_pro
olymmath
scitab
pubmedqa
```

每个数据集使用相同指标块：

| 指标 | 含义 |
|---|---|
| `planned_episodes` | 已计划并写入观测的 Episode 数 |
| `dispatched_episodes` | 实际开始提交的 Episode 数 |
| `terminal_episodes` | 已形成终态的 Episode 数 |
| `successful_episodes` | `failure_class=none` 的 Episode 数 |
| `failed_episodes` | 非 pending、非 none 的失败数 |
| `success_rate` | 成功 Episode / 已提交 Episode |
| `failure_rate` | 失败 Episode / 已提交 Episode |
| `throughput_eps` | 成功 Episode / 测量时间，保留为兼容字段 |
| `throughput` | 提交、完成、成功三种吞吐率及其测量窗口 |
| `average_reward` | 平均奖励，仅作观测 |
| `actual_steps` | 总步数、平均步数和最大步数 |
| `end_to_end_latency_ms` | Episode 端到端时延统计 |
| `failure_classes` | 各失败分类的数量 |
| `submission_rate` | 计划与实际提交频率 |

规模压测中，同一数据集的三个并行模式是依次执行的，因此该数据集的汇总
测量时间为三个模式执行时间之和。

稳定性验收中，五个数据集并发混合提交，因此每个数据集使用同一阶段的
完整测量时长。

### 3.1 吞吐率不能只看一个数

统一的 `throughput` 结构为：

| 字段 | 含义 |
|---|---|
| `measurement_seconds` | 吞吐率的测量窗口 |
| `submission_eps` | 已提交 Episode / 测量时间 |
| `completion_eps` | 已形成终态 Episode / 测量时间 |
| `successful_eps` | 成功 Episode / 测量时间 |
| `source` | 指标证据来源 |
| `complete` | 三种计数和测量窗口是否全部可用 |
| `reason` | 不完整时明确说明缺少什么 |

`submission_eps` 反映压测器把请求送入系统的速度，`completion_eps`
反映系统完成请求的速度，`successful_eps` 才是有效吞吐率。三者一起看
可以区分“提交不够快”“系统积压”和“虽然完成但失败很多”。

## 4. 每并行模式指标

`by_parallel_mode` 固定包含：

```text
sync
one_step_off_policy
fully_async
```

每个模式使用与 `by_dataset` 完全相同的指标块。这样可以直接回答：

- 三种模式分别提交多少 Episode；
- 成功率和失败率是否一致；
- 哪种模式吞吐更高；
- 哪种模式产生更多协议或训练轨迹错误；
- 各模式端到端时延是否存在明显差异。

规模压测要求五个数据集分别覆盖三个并行模式。

当前正式稳定性配置使用 `fully_async`。另外两个模式仍保留统一字段，
其计划量和实际量为零。这表示本阶段未使用该模式，不表示该模式已经通过
稳定性验收。

## 5. 数据集×并行模式指标

`by_dataset_parallel_mode` 使用以下 key：

```text
dscodebench|sync
dscodebench|one_step_off_policy
dscodebench|fully_async
...
pubmedqa|fully_async
```

共有 5×3=15 个固定组合。

该结构可以防止只给出“数据集总成绩”或“模式总成绩”而掩盖局部问题。
例如：

- DSCodeBench 总体成功，但 fully_async 模式存在训练轨迹错误；
- SWE-bench Pro sync 正常，但 fully_async 吞吐下降；
- PubMedQA 在某个模式下出现集中超时。

## 6. 每 Worker 指标

`by_worker` 中每条记录至少包含：

| 字段 | 含义 |
|---|---|
| `run_id` | Worker 所属测试运行 |
| `worker_id` | Worker 唯一 ID |
| `started_episodes` | Server 日志观察到的开始/分配数量，日志不提供时为 0 |
| `completed_episodes` | Server 日志观察到的完成数量 |
| `completion_share` | 该 Worker 完成量占本次运行总完成量的比例 |
| `throughput.completion_eps` | 该 Worker 完成量 / 对应测量时间 |
| `datasets` | 该 Worker 实际处理过的数据集 |
| `parallel_modes` | 该 Worker 实际处理过的并行模式 |
| `first_completion_timestamp` | 首次完成时间，日志提供时记录 |
| `last_completion_timestamp` | 最后完成时间，日志提供时记录 |
| `source` | 当前为 `server_log` |

当前 Adapter `SampleResult` 不返回 Worker ID，因此每 Worker 指标不能从
EpisodeObservation 推测，而是从隔离 Server 的 `episode_completed`
日志中统计。

这也意味着当前每 Worker 指标的权威内容是“实际完成量、完成吞吐率和负载份额”。
Worker 级成功率、失败率和端到端时延只有在日志能可靠提供
`request_id + worker_id` 的逐 Episode 关联时才能进一步计算；当前不会
把全局成功率平均分配给每台 Worker。因此 Worker 级
`throughput.submission_eps` 和 `throughput.successful_eps` 为 `null`，
而不是用全局数据伪造。

## 7. 每 Worker×数据集指标

`by_worker_dataset` 为每个 Worker 和每个数据集建立交叉记录：

| 字段 | 含义 |
|---|---|
| `worker_id` | Worker ID |
| `dataset` | 五个数据集之一 |
| `started_episodes` | 观察到的开始量 |
| `completed_episodes` | 观察到的完成量 |
| `completion_share_within_dataset` | 该 Worker 占此数据集完成量的比例 |
| `throughput.completion_eps` | 该 Worker 在此数据集上的完成量 / 测量时间 |
| `parallel_modes` | 对此数据集执行过的并行模式 |
| `source` | Server 日志 |

稳定性验收固定输出：

```text
Worker 数量 × 5
```

条记录，包括完成量为零的 Worker×数据集组合。因此不会因为省略零值行
而掩盖某台 Worker 从未处理某个数据集的问题。

规模压测在 Worker coverage 通过时，每个 Worker 都应有完成记录；
汇总器按测试 run ID 区分不同规模层级的 Worker，避免把两个独立隔离
运行中同名的 Worker 混为一台。

## 8. Worker 负载分布

`worker_load_distribution` 以每台 Worker 的
`completed_episodes` 为统计样本。

统一记录：

| 字段 | 含义 |
|---|---|
| `minimum` | Worker 完成量最小值 |
| `mean` | Worker 完成量平均值 |
| `p95` | Worker 完成量 P95 |
| `maximum` | Worker 完成量最大值 |
| `standard_deviation` | 总体标准差 |
| `coefficient_of_variation` | 变异系数 |
| `total` | 所有 Worker 完成量之和 |
| `configured_workers` | 应参与统计的 Worker 数 |
| `observed_workers` | 日志中观察到完成量的 Worker 数 |

P95 使用 nearest-rank 口径：

```text
排序后取 ceil(N×0.95) 对应的值
```

变异系数定义为：

```text
CV = Worker 完成量总体标准差 / Worker 完成量平均值
```

解释：

- CV 越接近 0，Worker 负载越均衡；
- CV 越大，负载偏斜越明显；
- 若平均值为 0，CV 记为 0，但 `available` 和 `total` 会表明没有有效负载。

统计总体必须包含配置中存在但完成量为零的 Worker。不能只统计“至少完成
一个 Episode”的 Worker，否则最小值、平均值和 CV 都会被人为美化。

## 9. Replay 命中率

`replay` 同时提供总体和每数据集统计：

```text
replay.overall
replay.by_dataset.<dataset>
```

字段如下：

| 字段 | 含义 |
|---|---|
| `calls` | replay 调用数 |
| `hits` | 成功匹配并返回采集轨迹的调用数 |
| `misses` | 找不到任务、Episode 绑定缺失、轨迹耗尽等未命中数 |
| `hit_rate` | `hits / (hits + misses)` |
| `assigned_episodes` | 已分配轨迹的 Episode 数 |
| `sampling_strategies` | 实际使用的回放策略 |

正式轨迹回放应满足：

```text
hit_rate = 100%
misses = 0
sampling_strategy = round_robin_episode
```

规模压测从各自 replay simulator 的统计接口读取 hit/miss。

稳定性 replay server 的 `/health` 现在直接返回每数据集
`calls/hits/misses/hit_rate`，稳定性运行结束前将其保存为：

```text
replay-health.json
```

再汇总进 `suite-metrics.json`。

## 10. 计划提交频率与实际提交频率偏差

统一字段位于：

```text
submission_rate
by_dataset.<dataset>.submission_rate
by_parallel_mode.<mode>.submission_rate
```

字段如下：

| 字段 | 含义 |
|---|---|
| `submission_kind` | `configured_rate` 或 `backlog` |
| `measurement_seconds` | 频率测量时间 |
| `planned_rate_eps` | 配置中的目标 Episode/s |
| `actual_rate_eps` | 实际 dispatched Episode / 测量时间 |
| `absolute_deviation_eps` | `actual_rate_eps - planned_rate_eps` |
| `relative_deviation` | `absolute_deviation_eps / planned_rate_eps` |
| `available` | 频率偏差是否具有定义 |
| `reason` | 不可计算时的原因 |

### 10.1 稳定性验收

稳定性测试是目标速率负载，计划值来自：

```text
phase_rate(task_config, phase)
```

因此可以计算每数据集和总体的计划/实际频率偏差。

例如：

```text
计划：2.4 Episode/s
实际：2.36 Episode/s
绝对偏差：-0.04 Episode/s
相对偏差：-1.67%
```

该指标可以发现负载生成器自身跟不上计划的情况，避免把“实际压力没有打满”
误认为系统稳定。

### 10.2 规模压测

规模压测采用：

```text
提交指定数量 backlog，再异步收集结果
```

它的计划是 Episode 总数、批次数和容量波次，不是目标 EPS。因此：

```text
submission_kind = backlog
planned_rate_eps = null
absolute_deviation_eps = null
relative_deviation = null
available = false
```

这不是漏记，而是明确表示该场景没有“计划 EPS”这一数学定义。规模压测仍
记录实际提交速率、提交总量、批次数、client submit seconds 和 backlog
比例。不能用实际提交速率反过来冒充计划速率。

如果导师要求规模压测也检查目标 EPS，需要另设 rate-controlled 规模场景，
不能改变当前 backlog 压测的语义。

## 11. 资源采样结果

`resources` 使用统一外壳：

| 字段 | 含义 |
|---|---|
| `available` | 是否存在有效资源样本 |
| `sample_count` | 实际样本数 |
| `expected_samples` | 按时长和采样周期计算的计划样本数 |
| `coverage` | `sample_count / expected_samples`，最大为 1 |
| `records` 或 `p95` | 原始摘要或统一分位数 |
| `events` | OOM、FD/线程耗尽、Worker 退出、UEnv crash 和人工重启 |
| `artifact` | 原始资源文件路径 |
| `reason` | 资源证据不可用的原因 |

稳定性验收直接读取 `resource.csv`，统一计算：

- RSS P95；
- open FD P95；
-线程数 P95；
-运行容器数 P95；
-资源采样覆盖率；
- OOM、FD 耗尽、线程耗尽、Worker 退出、UEnv crash、人工重启计数。

规模压测保留各专用 runner 已有的：

- Worker 主机资源摘要；
- fleet resource metrics；
-内存可用量下降；
-峰值 RSS、进程数、FD 数；
-容器并发和主机负载。

这些数据都放在统一 `resources.records` 下，并同时给出统一
`sample_count` 和 `available`。

## 12. 清理结果

`cleanup` 统一记录：

| 字段 | 含义 |
|---|---|
| `available` | 是否执行并记录清理检查 |
| `attempted` | 是否尝试清理 |
| `passed` | 所有清理检查是否通过 |
| `records` | 每个场景或稳定性 fleet 的清理详情 |
| `reason` | 未执行时的原因 |

规模压测的清理记录包括：

- 本次拥有的 Server、Worker、replay、agent 和资源监控进程是否停止；
- 隔离端口是否释放；
- 测试新建容器是否删除；
- 容器集合是否恢复到测试前基线；
- 生产进程和保护端口是否保持不变；
- 清理异常列表。

稳定性清理探针统一记录：

```text
remaining_workers
remaining_containers
remaining_processes
```

三项必须全部为 0，且不能有 cleanup error。

清理结果不是附属日志，而是正式 SuiteMetrics 的必需组成部分。

## 13. 数据完整性和 fail-closed

`data_quality` 至少包含：

### 13.1 Episode 观测完整性

检查：

```text
EpisodeObservation 行数 = declared row count = submitted count
```

### 13.2 Worker 归属完整性

记录：

- 是否有隔离 Server 日志；
- 匹配到多少 run-owned `episode_completed` 行；
- Worker coverage 记录数量；
- 数据来源和缺失原因。

正式稳定性执行的 fleet manifest 现在必须提供：

```json
{
  "server_log_path": "/absolute/path/to/isolated-server.log"
}
```

没有此字段就无法形成每 Worker 和每 Worker×数据集指标，执行前直接拒绝。

### 13.3 SuiteMetrics 完整性

正式完整套件要求：

- Episode 观测完整；
- Worker 指标可用；
- replay hit/miss 可用；
-资源采样可用；
-清理检查通过。

任一项缺失：

```text
complete = false
```

完整规模套件或正式稳定性执行不能报告为通过。

## 14. 产物位置

### 14.1 规模压测

顶层运行目录：

```text
<scale-run>/suite-metrics.json
```

顶层 `summary.json` 同时包含：

```text
suite_metrics_contract
suite_metrics
```

### 14.2 稳定性验收

阶段运行目录：

```text
<stability-run>/episode.sqlite
<stability-run>/resource.csv
<stability-run>/replay-health.json
<stability-run>/suite-metrics.json
<stability-run>/manifest.json
```

`manifest.json` 中的 `suite_metrics_contract` 记录聚合产物路径、完整性和
data quality 摘要。

## 15. 代码位置

| 内容 | 路径 |
|---|---|
| SuiteMetrics 定义与聚合 | `uenv_stress/core/suite_metrics.py` |
| 规模套件汇总接入 | `uenv_stress/cli/run_scale_suite.py` |
| 稳定性汇总接入 | `uenv_stress/cli/run_formal_stability_suite.py` |
| 稳定性 replay hit/miss | `uenv_stress/stability/replay_server.py` |
| DSCodeBench Worker 负载日志 | `uenv_stress/scale/dscodebench_pressure.py` |
| SWE-bench Pro Worker 负载日志 | `uenv_stress/scale/swebench_pro_pressure.py` |
| Math Worker 负载日志 | `uenv_stress/scale/rule_task_pressure.py` |
| 统一指标测试 | `tests/test_suite_metrics.py` |
