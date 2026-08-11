# SWE-smith GRPO 训练问题记录

> 记录日期：2026-08-08
> 说明：本文用于汇总 SWE-smith GRPO 训练过程中遇到的问题。一级章节按错误类型组织，具体训练 run 作为对应类型下的案例记录。

## 1. gRPC 消息大小限制

### 1.1 `verl_swesmith_grpo_train_20260808_080059`

本次训练在第二个 rollout batch 返回结果时中断，直接原因是 gRPC 单次响应体超过默认 4 MiB 接收上限。

#### 错误证据

主日志报错：

```text
grpc._channel._InactiveRpcError
status = StatusCode.RESOURCE_EXHAUSTED
details = "CLIENT: Received message larger than max (4646840 vs. 4194304)"
debug_error_string = "... grpc_message:\"CLIENT: Received message larger than max (4646840 vs. 4194304)\" ..."
```

其中 `4194304` 字节约等于 4 MiB，`4646840` 字节约等于 4.43 MiB，说明 Server 返回给 Adapter 的 `ExecuteBatch` 响应体超过了客户端默认接收上限。

#### 初步判断

该问题发生在 `Adapter -> Server ExecuteBatch -> Adapter` 的结果返回阶段，不是模型 gateway 请求失败。第二个 batch 可能聚合了 16 条 SWE/OpenHands episode 的结果，结果中包含轨迹、工具调用信息、token trace 或错误详情后，序列化后的 gRPC 响应体超过 4 MiB。

因此训练在 `AgentLoopWorker.generate_sequences()` 阶段被 RayTaskError 打断，第二个 batch 的结果没有写入本地 `agent-loop-results.jsonl`，也没有进入后续 VeRL advantage 计算和 actor update。

### 1.2 `verl_swesmith_grpo_train_20260808_152436`

本次训练显式设置了 `UENV_AGENT_LOOP_BATCH_SIZE=4`，Adapter 不再把 VeRL 的 16 条 rollout episode 一次性提交给 Server，而是按 4 条 episode 一组拆分提交。但训练仍在第一轮 rollout 的第二个 chunk 返回阶段触发 gRPC 4 MiB 超限。

#### 错误证据

主日志显示 VeRL 侧一个 rollout batch 仍包含 16 条 episode：

```text
uenv_agent_loop_batch_start batch_id=verl-agent-loop-step-1-f0c1ca49 sample_count=16 validate=False
```

本地结果文件显示只完成了前两个 4 条 chunk 的部分流程：

| 文件 | 记录数 | 含义 |
|---|---:|---|
| `agent-loop-requests.jsonl` | 8 | 前两个 chunk 共 8 条 episode 已提交 |
| `agent-loop-results.jsonl` | 4 | 第一个 chunk 的 4 条 episode 已成功返回并落盘 |

第二个 4 条 chunk 返回时触发报错：

```text
grpc._channel._InactiveRpcError
status = StatusCode.RESOURCE_EXHAUSTED
details = "CLIENT: Received message larger than max (4594969 vs. 4194304)"
```

其中 `4594969` 字节约等于 4.38 MiB，仍超过默认 4 MiB 接收上限。

第一个成功返回的 4 条结果中，已经出现单条结果体明显膨胀：

| sample_index | status | `response_ids_len` | `rollout_log_probs_len` | `verl_response_ids_len` |
|---:|---|---:|---:|---:|
| 0 | completed | 6310 | 6310 | 6310 |
| 1 | failed | 0 | 0 | 1 |
| 2 | completed | 214686 | 214686 | 8192 |
| 3 | completed | 10228 | 10228 | 8192 |

#### 初步判断

该问题不是 `UENV_AGENT_LOOP_BATCH_SIZE=4` 未生效，而是 4 条 episode 聚合后的返回体仍然可能超过 gRPC 默认上限。SWE/OpenHands episode 会包含多轮模型调用的 token trace；当 Worker 返回完整 `response_ids` 和 `rollout_log_probs` 时，单条 completed episode 就可能达到数十万 token 级别。

因此，`UENV_AGENT_LOOP_BATCH_SIZE=4` 只能减少每次聚合返回的 episode 条数，不能解决单条 episode trace 过大的问题。只要一个 chunk 内存在多个大 trace，仍会触发 4 MiB 限制。

## 2. Reward 信号失效

### 2.1 Worker 官方 Harness 对照发现 UEnv reward 不可信

Worker 侧对同一 SWE-smith instance 做了官方 harness 与 UEnv Worker 内部 grader 的对照。结论是：官方 harness 可作为最终 reward 的权威标准，但当前 UEnv Worker 内部 `swesmith` grader 与当前镜像 / catalog 组合不等价，不能作为 SWE-smith 最终训练 reward 的权威来源。

对照样本：

```text
instance_id = oauthlib__oauthlib.1fd52536.combine_file__0fceycuu
FAIL_TO_PASS = 13
PASS_TO_PASS = 660
```

#### 错误证据

同一 gold patch 在官方 harness 与 UEnv Gateway 下得到不同结果：

| 路径 | 镜像 | gold F2P | gold P2P | resolved |
|---|---|---:|---:|---|
| 官方 harness | `swebench/swesmith...` | 13/13 | 660/660 | true |
| UEnv Gateway | `jyangballin/swesmith...` | 13/13 | 518/660 | false |

官方 profile 解析出的镜像与 UEnv catalog 中记录的镜像不同：

```text
official image_name = swebench/swesmith.x86_64.oauthlib_1776_oauthlib.1fd52536
uenv image_cache_key = jyangballin/swesmith.x86_64.oauthlib_1776_oauthlib.1fd52536:latest
```

这说明当前 UEnv 链路中即使输入 gold patch，也可能因为镜像、测试环境或判分 profile 不一致而被判为 `reward=0.0`。

#### 初步判断

该问题发生在 Worker SWE-smith 环境执行与 reward 计算口径层面，不是 Adapter 生成训练 batch 或 gateway 转发模型请求的问题。

当前 UEnv 的 all-pass 规则方向上接近官方 resolved 语义，即要求 FAIL_TO_PASS 全通过且 PASS_TO_PASS 不退化。但实际运行环境未与官方 harness 对齐，导致 gold patch 的 PASS_TO_PASS 有 142 项失败，进而被判为未解决。

因此，之前 SWE-smith GRPO 训练中出现的 reward 全 0 不能只解释为模型没有修复能力；其中至少包含 reward 环境本身不可信的问题。在官方 harness 对齐前，当前 SWE-smith GRPO reward 不应作为有效训练信号。

### 2.2 Worker 未注入 SWE-smith bug baseline 导致 agent 进入 clean 状态

Adapter 侧记录了另一类会导致 reward 全 0 / `resolved=false` 的问题：Worker 创建 SWE-smith session 后，没有把数据集中的 bug-inducing patch 注入到 `/testbed`，导致 OpenHands agent 进入的是已经修好的 clean 状态。

#### 错误证据

问题 run：

```text
run_id = verl_swesmith_grpo_train_20260808_185234
instance_id = oauthlib__oauthlib.1fd52536.combine_file__oni9ccvi
trajectory_id = trj-worker-7143-pro-1786188520094-00031
```

该样本 prompt 描述的 bug 是 Metadata Endpoint 返回：

```text
Content-Type: application/xml
Status: 500
```

但 trajectory 中 agent 在 `/testbed` 运行 reproduction 时实际看到：

```text
Content-Type: application/json
Access-Control-Allow-Origin: *
Status: 200
tests/oauth2/rfc6749/endpoints/test_metadata.py: 7 passed
```

这说明 agent 看到的 workspace 已经是 clean/fixed 状态，prompt 中的 bug 无法复现。对应 rollout 最终没有产生有效修复：

```text
git_diff_bytes = 0
git_diff_nonempty = 0
resolved = false
tests_passed = 531 / 673
```

本地原始 SWE-smith parquet 中同一实例的 `patch` 方向为 clean -> buggy：

```diff
-            'Content-Type': 'application/json',
-            'Access-Control-Allow-Origin': '*',
+            'Content-Type': 'application/xml',
+            'Access-Control-Allow-Origin': 'localhost',

-        return headers, json.dumps(self.claims), 200
+        return headers, json.dumps(self.claims), 500
```

因此，交互式 agent 训练时，Worker 需要先把 workspace 准备为 buggy baseline，再让模型产生 buggy -> fixed 的修复 diff。

#### Worker 修复

Worker 侧反馈该问题已经修复，修复记录保存在：

```text
Docs/adapter/20260809-SWE-smith-GRPO初始环境与Reward核验说明.md
Docs/debug_log/20260809-SWE-smith问题修复统一汇总.md
```

当前修复方向：

- `SweSession::provision()` 后对 SWE-smith 正向应用数据集 `patch`，把 `/testbed` 置为 buggy 状态。
- 随后执行 `git add -A && git commit -m 'uenv smith bug baseline'`，把 bug 状态提交为 baseline。
- 后续 agent 产生的 `git diff` 表示从 buggy baseline 到 fixed 状态的 model patch。
- native gold 路径对 SWE-smith 使用 `apply_patch_reverse()`，用于从 buggy baseline 还原到 fixed 状态。

Worker 侧 smoke 已验证修复后出现真实非零 reward：

```text
run_id = codex_swesmith_grpo_retry_20260809_1414
reward = 1.0
resolved = true
git_diff_bytes = 7928
used_pad_fallback = false
rollout_log_probs_len = 6305
```

#### 后续核验点

如果后续启用 SWE session recycle，需要确认 `reset_to_base()` 后也会重新注入 SWE-smith bug baseline。否则复用 session 可能再次回到 clean 状态，导致同类 reward 信号失效问题复现。
