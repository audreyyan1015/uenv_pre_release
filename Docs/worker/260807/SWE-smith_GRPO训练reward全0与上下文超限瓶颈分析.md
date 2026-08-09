# SWE-smith GRPO 训练 reward 全 0 与上下文超限瓶颈分析

> 日期：2026-08-07  
> 数据来源：7142 服务器训练日志和 `layer4_distributed` JSONL。  
> 服务器连接：`root@219.147.100.43 -p 7142`。  
> 结论：当前 SWE-smith GRPO 配置不适合继续正常训练。核心瓶颈不是单一的“日志有上下文超限”，而是 reward 环境未与官方 SWE-smith harness 对齐、稀疏二值 reward 全 0、上下文预算错误、Agent 产出质量未形成可解任务、slot 超时和训练效率过低共同导致没有有效学习信号。

## 1. 检查的 run

### 1.1 最新运行中 run

```text
Run ID: verl_swesmith_grpo_train_20260807_083330
日志: /data/ronghao/uenv/uenv-bridge/temp/logs/verl_layer4_agent_loop/verl_swesmith_grpo_train_20260807_083330.log
数据: /data/ronghao/uenv/uenv-bridge/temp/logs/layer4_distributed/verl_swesmith_grpo_train_20260807_083330/
检查时间: 2026-08-07
```

检查时该 run 尚未完成：

```text
requests: 48
results: 48
model-gateway records: 1928
Training Progress: 日志已推进到 3/5；结果统计口径为检查时已落盘的 48 条 result
```

### 1.2 前一条完整 run

```text
Run ID: verl_swesmith_grpo_train_20260806_165600
results: 80
Training Progress: 10/10 完成
```

## 2. 关键统计

### 2.1 20260807 最新 run

| 指标 | 数值 |
|------|------|
| results | 48 |
| completed | 37 |
| failed | 11 |
| reward 分布 | 48 条全部 `0.0` |
| completed resolved | 37 条全部 `False` |
| completed git_diff_nonempty | 16 条为 1，21 条为 0 |
| context overflow | 6 条 |
| timeout acquiring agent+worker slot | 4 条 |
| timeout waiting for agent completion | 1 条 |
| vLLM 400 | 6 条 |
| rollout_log_probs_len | n=48, min=0, p50=8145.5, max=32218, mean=8036.8 |

测试通过数集中在：

```text
531/673: 30 条
530/673: 2 条
535/673: 2 条
528/673: 1 条
475/673: 1 条
525/673: 1 条
```

样本分布显示每个 instance 基本采样 4 次，均来自 `oauthlib__oauthlib.1fd52536.combine_file__*`。

### 2.2 20260806 完整 run

| 指标 | 数值 |
|------|------|
| results | 80 |
| completed | 71 |
| failed | 9 |
| reward 分布 | 80 条全部 `0.0` |
| completed resolved | 71 条全部 `False` |
| completed git_diff_nonempty | 27 条为 1，44 条为 0 |
| context overflow | 9 条 |
| Training Progress | 10/10 完成 |

这说明 reward 全 0 不是最新 run 的偶发问题，而是至少连续两次 SWE-smith GRPO run 稳定出现。

## 3. 上下文超限问题

日志中重复出现：

```text
This model's maximum context length is 131072 tokens.
However, you requested 4096 output tokens and your prompt contains at least 126977 input tokens,
for a total of at least 131073 tokens.
```

这个错误非常关键：它不是大幅超限，而是 `126977 + 4096 = 131073`，只比模型上限 `131072` 多 1 token。

这说明当前模型请求的 `max_tokens=4096` 是固定预算，没有根据实际 prompt token 数动态收缩。如果把输出预算动态设为：

```text
max_tokens = max_model_len - prompt_tokens - margin
```

这些边界样本至少不会因为 1 token 被 vLLM 拒绝。需要保留 margin，例如 64 或 128，避免 tokenizer 估算误差。

但上下文超限不是唯一问题。即使排除 6/48 或 9/80 的失败，未超限且 completed 的 episode 仍全部 reward 0。

## 4. Reward 全 0 的核心原因

### 4.0 官方 harness 对照后的新增结论

2026-08-07 追加官方 SWE-smith harness 对照校验后，确认当前 reward 全 0 不能只归因于模型 patch 质量。

同一完整 instance：

```text
oauthlib__oauthlib.1fd52536.combine_file__0fceycuu
```

官方 harness：

```text
gold patch: resolved=true, FAIL_TO_PASS 13/13, PASS_TO_PASS 660/660
empty patch: resolved=false, FAIL_TO_PASS 0/13, PASS_TO_PASS 660/660
```

UEnv Gateway 当前路径：

```text
gold patch: resolved=false, reward=0.0, tests=531/673
empty patch: resolved=false, reward=0.0, tests=518/673
```

也就是说，在当前 UEnv SWE-smith 环境中，gold patch 也会被判为 0。进一步定位发现官方 harness 使用：

```text
swebench/swesmith.x86_64.oauthlib_1776_oauthlib.1fd52536
```

而当前 UEnv catalog / Worker 使用：

```text
jyangballin/swesmith.x86_64.oauthlib_1776_oauthlib.1fd52536:latest
```

因此当前 GRPO reward 全 0 至少包含“判分环境未与官方 harness 对齐”的问题。完整对照记录见：

```text
Docs/worker/260807/SWE-smith官方Harness判分对照校验.md
```

### 4.1 当前 reward 是过稀疏的二值成功信号

completed episode 全部 `resolved=false`，因此 reward 全部为 0。当前训练没有任何正样本：

```text
20260807: 37 completed, resolved=True 为 0
20260806: 71 completed, resolved=True 为 0
```

对于 GRPO，这意味着组内优势也基本为 0。日志中已经体现：

```text
critic/score/mean: 0.0
critic/rewards/mean: 0.0
critic/advantages/mean: 0.0
```

在这种数据下继续训练，策略更新主要来自 KL、数值噪声或无效梯度，不会学习到“如何解 SWE-smith”。

### 4.1.1 进一步分组核验

2026-08-07 追加核验了两类分组：

1. 按 `batch_id`，即每个训练 step 实际提交到 AgentLoop 的 episode 组。
2. 按 `instance_id`，即同一 SWE-smith instance 的多次 rollout 组。

结果显示两类分组内的 `reward_set` 全部都是 `[0.0]`。

旧完整 run `verl_swesmith_grpo_train_20260806_165600`：

| batch | n | reward_set | resolved | 备注 |
|-------|---|------------|----------|------|
| step 1-10 | 每组 8 | `[0.0]` | completed 全部 `False` | 每个 step 内无 reward 差异 |
| instance group | 每个 instance 4 | `[0.0]` | completed 全部 `False` | 同题多 rollout 之间无相对优劣信号 |

最新 run `verl_swesmith_grpo_train_20260807_083330`：

| batch | n | reward_set | resolved | 备注 |
|-------|---|------------|----------|------|
| step 1 | 16 | `[0.0]` | completed 7 条全部 `False` | 其余为失败，也被记 0 |
| step 2 | 16 | `[0.0]` | completed 15 条全部 `False` | 有 4 条非空 diff，但 reward 仍 0 |
| step 3 | 16 | `[0.0]` | completed 15 条全部 `False` | 有 10 条非空 diff，但 reward 仍 0 |
| instance group | 每个 instance 4 | `[0.0]` | completed 全部 `False` | 同题组内 reward 方差为 0 |

训练日志也直接印证了这一点。旧 run 的 10 个 step 全部为：

```text
critic/score/mean = 0.0
critic/score/max  = 0.0
critic/score/min  = 0.0
critic/rewards/mean = 0.0
critic/rewards/max  = 0.0
critic/rewards/min  = 0.0
critic/advantages/mean = 0.0
critic/advantages/max  = 0.0
critic/advantages/min  = 0.0
actor/pg_loss = 0.0
```

最新 run 已落盘的 step 1-3 也是：

```text
critic/score/mean/max/min = 0.0
critic/rewards/mean/max/min = 0.0
critic/advantages/mean/max/min = 0.0
```

因此这里的“没有可学习方向”不是泛化判断，而是 GRPO 组内相对排序信号不存在：同组 rollout 全部 0，归一化后 advantage 仍为 0，policy gradient loss 没有来自任务成功/失败差异的有效梯度。

### 4.2 Agent 有时产生 diff，但没有接近 resolved

最新 run 中 16/37 completed 有非空 diff，旧 run 中 27/71 completed 有非空 diff，但所有 diff 都没有通过目标判定。

测试通过数主要停留在 `531/673`。这说明 Agent 不是完全没有动作，而是动作质量不足或没有命中任务要求。当前 reward 没有区分：

- 无 diff
- 有 diff 但无改善
- 编译/测试数有改善
- 部分相关测试通过
- 完全 resolved

这些全都被压成 reward 0，训练无法知道哪些行为更接近目标。

### 4.3 上下文和响应长度消耗极高

最新 run step 指标：

```text
step 1:
  critic/score/mean = 0.0
  response_length/clip_ratio = 0.375
  timing_s/gen = 3609.3
  timing_s/step = 3745.3

step 2:
  critic/score/mean = 0.0
  response_length/clip_ratio = 0.5625
  timing_s/gen = 2139.0
  timing_s/step = 2246.4
```

大量响应触顶或接近触顶，生成耗时占主导。也就是说，训练正在花大量 GPU 时间生成长响应，但 reward 没有任何正反馈。

### 4.4 Agent / Worker slot 超时开始出现

最新 run 有：

```text
timeout acquiring agent+worker slot: 4
timeout waiting for agent completion: 1
```

这说明调度吞吐与执行耗时已经开始互相影响。即使 Server 和 Worker 有并行基础，Agent/Gateway/Worker slot 的实际可用性仍会导致 episode 失败或排队超时。

## 5. 为什么当前完全不可用于正常训练

当前状态下继续 GRPO 的主要问题是：

1. 没有正 reward，组内 advantage 为 0，训练没有可学习方向。
2. 未 solved 的 episode 中也没有 partial reward，无法区分“更接近解决”的行为。
3. 上下文预算固定导致边界样本被 vLLM 直接拒绝，造成无效 episode。
4. 响应长度 clip ratio 高，生成时间极长，单位训练信号成本过高。
5. Agent / Worker slot 超时说明执行链路还没有稳定支撑当前并行压力。
6. 样本当前集中在 `oauthlib` 同一 repo/commit 变体，若数据难度或任务构造不适合当前 Agent，会持续采不到正样本。

因此，当前不是“再跑久一点就会自然出现 reward 1”的状态，而是奖励设计、上下文管理、执行稳定性和数据难度都需要先修正。

## 6. 建议修复顺序

### 6.1 先修上下文预算

必须把固定 `4096` 输出预算改为动态预算：

```text
available = model_max_context - prompt_tokens - safety_margin
max_tokens = min(configured_max_tokens, max(1, available))
```

同时需要：

- 记录 prompt token、requested max_tokens、最终 max_tokens。
- 当 available 太小，提前终止或压缩上下文，不要送到 vLLM 后才 400。
- 压缩 OpenHands / browser / tool transcript，限制历史轮次。

### 6.2 加 partial reward

至少应加入以下 shaping：

| 信号 | 用途 |
|------|------|
| resolved | 最终二值成功 |
| git_diff_nonempty | 区分完全无动作 |
| patch applies | 区分无效 patch |
| tests_passed_delta | 区分是否有改善 |
| selected target tests | 给更稠密的任务相关反馈 |
| lint/compile pass | 避免破坏性修改 |
| context_overflow / timeout penalty | 避免长上下文和挂死行为 |

最终 reward 可以仍以 resolved 为主，但训练阶段不能只依赖 resolved。

### 6.3 做 gold patch / grader 校验

在继续训练前，需要对同一 SWE-smith instance 做离线校验：

1. 使用 gold patch 或人工正确 patch。
2. 通过当前 Worker/EnvPackage 路径执行。
3. 确认 `resolved=true` 且 reward=1。
4. 确认测试数量、catalog、instance_id、repo、base commit 都匹配。

该项已通过官方 harness 对照确认：官方 gold 能 `resolved=true`，但 UEnv 当前 gold 不能 reward 1，瓶颈在 UEnv SWE-smith 环境 / grader 映射与官方 harness 未对齐。

### 6.4 降低样本难度并做 smoke curriculum

先构造小规模可解数据：

- 更短文件、更少测试、更明确 issue。
- 每个 repo 先做 pass@N 评估，确认当前 Agent 有非零解决率。
- GRPO 训练只在有非零正样本概率的数据上开始。

### 6.5 对齐并行与 slot 参数

最新 run 已出现 slot 超时。需要把 Adapter run 级 `max_episode_concurrency`、Worker `warmup_target`、Worker busy 上限、Gateway session limit、Agent job 并发显式对齐。具体参数方案见同目录：

```text
Docs/worker/260807/Episode并行调度与预热池参数梳理.md
```

## 7. 当前服务器服务状态

检查时：

| 服务 | 状态 |
|------|------|
| Server admin `/status` | ready=true, accepting=true, worker_count=1, total_capacity=4 |
| trajectory service `/control/v1/trajectories/health` | db=ok |
| Worker 7143 `/health` | ok |
| Runtime Gateway `/runtime/v1/health` | ok |

Server `/status` 中可见：

- `platform_features`: 包含 `hub_dynamic_env`、`trajectory_v2_2`、`artifact_uri`、`reward_adapter_v1`
- `package_states`: 包含 `swe-bench-pro`、`swe-bench-smith`、`dyn-openenv-prod`
- `pool_slots`: 可见动态环境、qa、code 等 ready slots

## 8. 结论

当前 SWE-smith GRPO 最大瓶颈是“reward 环境本身未与官方 harness 对齐，导致没有任何可信正向训练信号”。上下文超限需要立即修，但它只解释一部分 failed episode，不能解释 completed episode 全部 reward 0，更不能解释 UEnv gold patch 也 reward 0。

在完成官方 harness reward adapter / 镜像对齐、动态 token 预算、partial reward、gold/empty 回归、样本难度筛选和并行 slot 对齐前，不建议继续用当前配置做正常 GRPO 训练。
