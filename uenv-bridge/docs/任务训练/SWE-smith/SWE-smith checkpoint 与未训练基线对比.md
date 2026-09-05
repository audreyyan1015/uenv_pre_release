# SWE-smith checkpoint 与未训练基线对比

> 评测对象：SWE-smith 训练后的 checkpoint 与未经过训练的基线模型
> 评测集：SWE-bench-Pro 全量 731 条
> 结论：checkpoint 没有形成稳定净提升，`resolved` 为 104，基线为 106，整体接近但略低。

## 1. 概述

这次对比使用同一套 SWE 评测链路，比较两类模型：

1. 训练后的 checkpoint
2. 未经过训练的基线模型

从全量结果看，checkpoint 的整体通过数没有超过基线，属于“局部样本有提升，但总体没有拉开差距”的状态。

主要结论：

- checkpoint：`104 / 731`，`14.23%`
- 基线模型：`106 / 731`，`14.50%`
- 训练后结果略低于基线，差距很小，但没有证明 checkpoint 已经带来稳定增益

## 2. 配置对比

| 项 | checkpoint | 基线模型 |
|---|---|---|
| 评测集 | 731 条 | 731 条 |
| `resolved_count` | 104 | 106 |
| `resolve_rate` | 14.23% | 14.50% |
| `completed_count` | 679 | 675 |
| `failed_count` | 52 | 56 |
| `model_name` | `Qwen/Qwen3.6-35B-A3B` | `Qwen/Qwen3.6-35B-A3B` |
| `batch_size` | 4 | 1 |
| `max_iterations` | 60 | 60 |
| `temperature` | 0.0 | 0.0 |
| `top_p` | 1.0 | 1.0 |
| `max_tokens` | 8192 | 8192 |
| `max_new_tokens` | 8192 | 8192 |
| `thinking_token_budget` | 4096 | 4096 |
| `max_retries` | 3 | 3 |
| `timeout_seconds` | 7200 | 7200 |


## 3. 结果

### 3.1 总体结果

| 指标 | checkpoint | 基线 | 差值 |
|---|---:|---:|---:|
| `resolved` | 104 | 106 | -2 |
| `resolve_rate` | 14.23% | 14.50% | -0.27 pp |
| `completed_count` | 679 | 675 | +4 |
| `failed_count` | 52 | 56 | -4 |

### 3.2 按仓库拆分

| Repo | checkpoint | 基线 | 变化 |
|---|---:|---:|---:|
| `NodeBB/NodeBB` | 14/44 | 12/44 | +2 |
| `ansible/ansible` | 25/96 | 20/96 | +5 |
| `element-hq/element-web` | 0/56 | 0/56 | 0 |
| `flipt-io/flipt` | 0/85 | 0/85 | 0 |
| `future-architect/vuls` | 0/62 | 0/62 | 0 |
| `gravitational/teleport` | 0/76 | 0/76 | 0 |
| `internetarchive/openlibrary` | 52/91 | 57/91 | -5 |
| `navidrome/navidrome` | 0/57 | 0/57 | 0 |
| `protonmail/webclients` | 0/65 | 0/65 | 0 |
| `qutebrowser/qutebrowser` | 13/79 | 17/79 | -4 |
| `tutao/tutanota` | 0/20 | 0/20 | 0 |

### 3.3 训练过程情况

这轮 SWE-smith GRPO 训练从 `2026-08-12 18:42:38` 开始，最后一批结果产生于 `2026-08-22 08:10:51`，对应 `global_step_1121`，共 `1121` 个训练 step、`2242` 个 rollout group 以及 `8968` 条 episode：

| 指标 | 数值 | 占比 |
|---|---:|---:|
| `resolved` / 成功解决 episode | 2527 | 28.18% |
| 报错 episode | 565 | 6.30% |
| 非空 response episode | 8403 | 93.70% |
| 有非零 reward 的 step | 625 / 1121 | 55.75% |

按 GRPO 分组看，`2242` 个 rollout group 中，`1512` 组全部失败，占 `67.44%`；只有 `206` 组同时包含成功和失败样本，占 `9.19%`。这意味着即使排除后续大规模连接错误，组内 reward 差异仍然不足，能够提供稳定策略优化信号的样本比例偏低。

训练耗时情况如下：

| 总训练 step 耗时 | 平均每 step | 中位数 | P90 | P95 | 最大值 |
|---:|---:|---:|---:|---:|---:|
| 207.38 h | 666.0 s | 407.6 s | 1710.1 s | 1952.0 s | 3678.1 s |

每个 step 的延迟主要由以下部分组成：

| 分项 | 累计耗时 | 占总 step 耗时 |
|---|---:|---:|
| `timing_s/gen` | 181.80 h | 87.7% |
| `timing_s/old_log_prob` | 3.87 h | 1.9% |
| `timing_s/ref` | 1.50 h | 0.7% |
| `timing_s/update_actor` | 11.24 h | 5.4% |
| `timing_s/update_weights` | 4.95 h | 2.4% |

可以看到，step 耗时主要来自 `timing_s/gen`，也就是 rollout / 环境交互阶段；反向更新、权重同步、old log prob 和 reference log prob 不是主要耗时来源。

失败 episode 的原因分析如下：

| 失败原因 | episode 数 | 占失败 episode | 涉及 step | 说明 |
|---|---:|---:|---:|---|
| 模型服务连接错误 | 412 | 72.92% | 53 | agent 调用模型服务时连接失败，这是模块之间的网络异常导致的。 |
| worker baseline commit 失败 | 128 | 22.65% | 31 | worker 在进入 agent 交互前创建基线提交时被仓库自带 hook 拦截，属于环境准备阶段失败。 |
| 上下文超长 | 24 | 4.25% | 23 | prompt 或交互历史超过模型上下文窗口，导致模型请求被拒绝。 |
| 等待 / 槽位超时 | 1 | 0.18% | 1 | episode 等待 agent 完成或等待可用执行槽位时超时。 |

整体看，失败 episode 中占比最高的是模型服务连接错误，其次是 worker baseline commit 失败。这两类问题更偏向系统链路和环境准备问题，不完全代表模型本身没有能力完成修复；但它们会转化为空 response 或 zero reward，从而降低有效训练样本比例。

## 4. 原因分析

这次 checkpoint 没有超过基线，主要可以从四点理解：

1. 提升不是均匀分布的。`NodeBB` 和 `ansible` 有提升，但 `openlibrary` 和 `qutebrowser` 回落，最后被抵消。
2. 训练收益还不稳定。当前 checkpoint 更像是局部样本上学到了一些模式，但还没有形成跨仓库的稳定泛化。
3. 截断口径下链路错误比例不高，但 GRPO 组内 reward 差异仍然不足，能够形成有效策略优化信号的 rollout group 占比偏低。
4. 训练耗时瓶颈主要在 rollout / 环境交互。`timing_s/gen` 占总 step 耗时的 `87.7%`，远高于模型更新、权重同步和 log prob 重算，因此后续优化应优先面向 episode 执行吞吐，而不是只优化训练侧反向计算。

## 5. 下一步建议

1. 对同一配置至少重复评测 2-3 次，记录均值和波动范围。当前 `104` vs `106` 的差距很小，单次结果不足以排除测评随机性。
2. 训练侧优先处理有效信号不足问题，重点降低全失败 rollout group 占比，并修复模型服务连接、baseline commit hook、上下文超长等会产生 zero reward 的系统问题。
3. 效率优化优先面向 rollout / 环境交互阶段，包括提升 worker 与 agent 并发、减少长尾 episode 等待，以及对不可恢复错误做更早失败和更清晰的重试策略。
