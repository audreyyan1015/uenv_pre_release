# SWE-smith GRPO 初始环境与 Reward 核验说明

> 记录日期：2026-08-09
> 面向对象：Worker 侧
> 关联 run：`verl_swesmith_grpo_train_20260808_185234`
> 结论摘要：本轮 `0 resolved` 不应优先归因于模型参数或 `max_steps`。当前证据显示，至少部分 SWE-smith episode 的 workspace 初始状态无法复现 prompt 描述的 bug，且 reward 侧仍需要确认是否真正启用了官方 SWE-smith harness。

## 1. 相关日志

```text
/data/ronghao/uenv/uenv-bridge/temp/logs/layer4_distributed/verl_swesmith_grpo_train_20260808_185234
/data/ronghao/uenv/uenv-bridge/temp/logs/verl_layer4_agent_loop/verl_swesmith_grpo_train_20260808_185234.log
```

本轮检查时训练仍在运行，第 4 个 rollout batch 只返回了部分结果，因此以下统计是截至检查时的阶段性结果。

## 2. 阶段性结果

`agent-loop-results.jsonl` 中已有 52 条结果：

| 指标 | 数值 |
|---|---:|
| result 总数 | 52 |
| completed | 47 |
| failed | 5 |
| `reward=1.0` | 0 |
| `resolved=true` | 0 |
| 非空 git diff | 14 |
| 空 git diff | 33 |
| completed 中 response 被截到 8192 token | 46 / 47 |

5 条 failed 的直接原因是 vLLM 上下文窗口超限：

```text
This model's maximum context length is 262144 tokens.
However, you requested 4096 output tokens and your prompt contains at least 258049 input tokens,
for a total of at least 262145 tokens.
```

该错误只解释少数 failed episode，不能解释 47 条 completed 全部 `resolved=false`。

## 3. 关键证据：初始环境无法复现任务 bug

### 3.1 样本信息

本轮中 source_index=112 的样本：

```text
instance_id = oauthlib__oauthlib.1fd52536.combine_file__oni9ccvi
trajectory_id = trj-worker-7143-pro-1786188520094-00031
```

prompt 描述的问题是 OAuth2 Metadata Endpoint 返回错误的 content type 和 status：

```text
Expected: application/json / 200
Actual:   application/xml / 500
```

### 3.2 trajectory 中的实际 reproduction

OpenHands 在 workspace 中执行 reproduction 后，实际输出是：

```text
Content-Type: application/json
Access-Control-Allow-Origin: *
Status: 200
Body: {"issuer": "https://foo.bar", ...}
```

随后局部测试也显示：

```text
tests/oauth2/rfc6749/endpoints/test_metadata.py
7 passed
```

也就是说，模型进入环境后看到的代码已经表现为修复后的状态，而不是 prompt 中描述的 buggy 状态。该 rollout 最终没有产生 git diff：

```text
git_diff_bytes = 0
git_diff_nonempty = 0
resolved = false
tests_passed = 531 / 673
```

### 3.3 原始数据中的 patch 方向

本地原始 SWE-smith 数据：

```text
/data/ronghao/uenv/uenv-bridge/data/benchmarks/swesmith/raw/data/train-00000-of-00011.parquet
row index = 112
```

该样本的 `patch` 片段是：

```diff
-            'Content-Type': 'application/json',
-            'Access-Control-Allow-Origin': '*',
+            'Content-Type': 'application/xml',
+            'Access-Control-Allow-Origin': 'localhost',

-        return headers, json.dumps(self.claims), 200
+        return headers, json.dumps(self.claims), 500
```

这说明数据集中的 `patch` 是造 bug 方向。对于 agent 交互式训练，如果 prompt 要求模型修复该 bug，workspace 初始状态需要能复现 bug。当前 trajectory 显示至少该样本没有进入预期的 buggy 状态。

## 4. 关键证据：Reward 仍呈现历史 P2P 未对齐模式

有非空 diff 的样本也没有 resolved。例如：

```text
trajectory_id = trj-worker-7143-pro-1786187128000-00030
instance_id = oauthlib__oauthlib.1fd52536.combine_file__oni9ccvi
git_diff_bytes = 2414
tests_passed = 530 / 673
resolved = false
```

该 trajectory 中模型确实读写了源码文件，修改了：

```text
/testbed/oauthlib/oauth2/rfc6749/endpoints/metadata.py
```

并且自己运行 reproduction 得到：

```text
Content-Type: application/json
Status: 200
Access-Control-Allow-Origin: *
```

但最终 reward 仍为 0。进一步查看 `per_test`，失败项大量集中在：

```text
tests/openid/connect/...
```

类似模式在其他 instance 上也出现，常见结果为：

```text
531 / 673
532 / 673
533 / 673
```

这与此前 Worker 文档中记录的 SWE-smith reward / harness 未对齐问题一致：即使部分 FAIL_TO_PASS 修复或局部测试通过，PASS_TO_PASS 中仍有大批固定失败项，导致 `resolved=false`。

## 5. 当前代码与文档中的口径冲突

### 5.1 OpenHands driver 仍假设 Worker 已注入 bug

`/data/ronghao/uenv/integrations/openhands/run_swebenchpro_official.py:688` 附近写明：

```text
SWE-smith：数据集 patch 为造 bug 补丁；Worker provision 已注入，
gold 用 git apply -R 还原。
```

这与本轮 trajectory 中看到的初始环境表现不一致。

### 5.2 Worker session 中仍保留 reverse patch 语义

`/data/ronghao/uenv/uenv-worker/src/swe/session.rs:347` 附近写明：

```text
SWE-smith：数据集 `patch` 为造 bug 补丁，gold 验收需 `git apply -R` 还原。
```

同时 `apply_patch_reverse()` 仍存在。

### 5.3 不同路径的 gold patch 方向不一致

`/data/ronghao/uenv/uenv-worker/src/swe/harness.rs:105` 附近：

```text
session.apply_patch(&instance.patch, "gold")
```

这里对 gold patch 是正向应用。

但 `/data/ronghao/uenv/uenv-worker/src/swe/instance_pool.rs:211` 附近：

```text
if variant == BenchmarkVariant::Smith {
    self.get(&session_id)?.apply_patch_reverse(p, "gold")?;
}
```

这里对 Smith gold patch 是反向应用。

这说明 Worker 内部不同入口对 SWE-smith patch 方向的处理仍需统一。

## 6. 需要 Worker 侧核验的问题

### 6.1 session 初始状态

请确认当前生产 Worker 创建 SWE-smith session 后，`/testbed` 是否应该处于 buggy 状态。

建议用上述样本做单实例核验：

```text
instance_id = oauthlib__oauthlib.1fd52536.combine_file__oni9ccvi
```

在 agent 开始前执行 reproduction，应看到 prompt 描述的失败。如果 agent 开始前已经是 `application/json / 200`，则训练任务语义与 prompt 不一致。

### 6.2 patch 注入方向

请确认 SWE-smith 的数据集 `patch` 在 Worker 中的语义：

```text
数据集 patch: clean -> buggy
agent 期望 patch: buggy -> fixed
```

如果训练路径采用 agent 交互式修复，provision 阶段需要把 workspace 准备成 buggy 状态，最终 model patch 才能表示修复动作。

### 6.3 官方 harness adapter 是否真实启用

Worker 文档中提到可通过：

```text
UENV_SWE_SMITH_EVAL_CMD
```

启用官方 SWE-smith harness 作为最终 reward。请确认当前生产 Worker 是否设置了该环境变量，并在结果 metadata 或日志中区分：

```text
official_resolved
uenv_internal_resolved
reward_source
```

如果未启用，则当前 reward 可能仍来自内部 parser fallback，不能作为最终训练信号。

### 6.4 gold / empty 对照

建议在同一个生产 Worker、同一个 EnvPackage、同一个 catalog 上固定做两条回归：

| 对照 | 预期 |
|---|---|
| gold 修复 | `resolved=true`, `reward=1.0` |
| empty patch | `resolved=false`, `reward=0.0` |

如果 gold 仍不能过，则训练 reward 不可信，应先修复 Worker 环境与官方 harness 对齐问题。

## 7. 对本轮训练的判断

本轮 `0 resolved` 包含三类问题：

1. 少数 episode 确实因为上下文窗口超限失败。
2. 多数 completed episode 的 reward 全为 0，VeRL 日志中 `critic/rewards`、`critic/advantages`、`actor/pg_loss` 均为 0，几乎没有任务奖励驱动的更新。
3. 至少部分 episode 的初始 workspace 已经无法复现 prompt 中的 bug，模型没有处在正确的修复任务环境里。

因此，当前不建议把这轮结果解释为“模型能力不足”或“max_steps 不够”。优先级更高的问题是 Worker 侧 SWE-smith 初始环境、patch 方向、官方 harness reward 是否完全对齐。
