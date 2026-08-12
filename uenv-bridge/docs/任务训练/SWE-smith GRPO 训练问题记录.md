# SWE-smith GRPO 训练问题记录

> 记录日期：2026-08-08
> 说明：本文用于汇总 SWE-smith GRPO 训练过程中遇到的主要问题。当前按“训练正确性”“训练效率”和“训练稳定性”三类组织，每个问题从问题说明、证据和解决方案三方面记录。

## 1. 训练正确性问题

训练正确性问题指训练链路虽然能够提交 episode、调用模型和返回结果，但环境状态、判分逻辑或 reward 信号不可靠，导致模型即使产生了有效行为也无法得到可信训练反馈。

### 1.1 官方 harness 与内部判分不一致

#### 1.1.1 问题说明

SWE-smith 训练依赖 Worker 在真实代码环境中执行测试，并把任务是否修复成功转换为 reward。如果同一个修复在官方 harness 下可以通过，但在 UEnv Worker 内部判分下被判失败，训练得到的 reward 就不可信。

这类问题会让模型的有效修复被错误地记为失败，进而影响 GRPO 的训练信号。

#### 1.1.2 证据

Worker 侧曾对同一 SWE-smith instance 做官方 harness 与 UEnv Worker 内部 grader 对照。对照样本：

```text
instance_id = oauthlib__oauthlib.1fd52536.combine_file__0fceycuu
FAIL_TO_PASS = 13
PASS_TO_PASS = 660
```

同一 gold patch 在两条链路下得到不同结果：

| 路径 | 镜像 | gold F2P | gold P2P | resolved |
|---|---|---:|---:|---|
| 官方 harness | `swebench/swesmith...` | 13/13 | 660/660 | true |
| UEnv Gateway | `jyangballin/swesmith...` | 13/13 | 518/660 | false |

这说明当前 UEnv 链路中即使输入 gold patch，也可能因为镜像、测试环境或判分 profile 不一致而被判为 `reward=0.0`。

#### 1.1.3 解决方案

Worker 侧需要以 SWE-smith 官方 harness 和官方 resolved 语义作为 reward 的基准，确保相同 instance、相同 patch 在官方链路和 UEnv 链路下得到一致结论。

具体方向是统一镜像命名空间、测试 profile、PASS_TO_PASS / FAIL_TO_PASS 口径和 resolved 判定逻辑。只有 Worker 内部 reward 与官方 harness 对齐后，SWE-smith GRPO 的 reward 才能作为有效训练信号。

### 1.2 初始环境处于 clean 状态

#### 1.2.1 问题说明

SWE-smith 任务要求模型在包含 bug 的代码环境中完成修复。如果 agent 进入环境时看到的代码已经是修复后的 clean 状态，它就无法复现 prompt 中描述的 bug，也难以产生有意义的修复 diff。

这类问题会导致模型看起来没有修改文件、没有解决任务，但根因并不是模型一定不会修，而是环境初始状态不符合任务语义。

#### 1.2.2 证据

Adapter 侧在训练 run 中观察到 agent 进入的 workspace 已经是 clean/fixed 状态。问题样本：

```text
run_id = verl_swesmith_grpo_train_20260808_185234
instance_id = oauthlib__oauthlib.1fd52536.combine_file__oni9ccvi
trajectory_id = trj-worker-7143-pro-1786188520094-00031
```

Prompt 描述的 bug 是接口返回：

```text
Content-Type: application/xml
Status: 500
```

但 trajectory 中 agent 在 `/testbed` 运行复现时看到：

```text
Content-Type: application/json
Access-Control-Allow-Origin: *
Status: 200
tests/oauth2/rfc6749/endpoints/test_metadata.py: 7 passed
```

对应 rollout 最终没有产生有效修复：

```text
git_diff_bytes = 0
git_diff_nonempty = 0
resolved = false
tests_passed = 531 / 673
```

#### 1.2.3 解决方案

Worker 侧需要保证 agent 进入环境时看到的是需要修复的 buggy baseline。

当前修复方向是通过 runtime contract 显式表达 SWE-smith 的环境语义：SWE-smith 的数据集 patch 表示 clean -> buggy，因此 Worker provision 阶段需要先把 `/testbed` 置为 buggy 状态；模型 rollout 产生的 diff 才表示从 buggy 到 fixed 的修复。gold patch 路径则需要按相反方向恢复到 fixed 状态。

Worker 侧反馈该问题已修复，并在 smoke 中观察到非零 reward：

```text
run_id = codex_swesmith_grpo_retry_20260809_1414
reward = 1.0
resolved = true
git_diff_bytes = 7928
used_pad_fallback = false
rollout_log_probs_len = 6305
```

后续需要继续确认 session recycle、reset 或多 episode 复用环境时，也会重新回到正确的 buggy baseline。

## 2. 训练效率问题

训练效率问题指 reward 语义本身不一定错误，但训练吞吐、并发、传输体积或系统资源利用存在瓶颈，导致训练慢、容易中断或无法稳定扩大到正式数据规模。

### 2.1 gRPC 消息大小限制

#### 2.1.1 问题说明

SWE-smith 使用 OpenHands agent 解决代码任务。一次 episode 可能包含多轮模型调用、工具调用、文件编辑、测试输出和 token trace，因此返回结果远大于普通 QA 任务。

当 Worker 把多个 episode 的完整结果聚合返回给 Adapter 时，gRPC 默认 4 MiB 消息上限可能不够，训练会在结果返回阶段中断。单纯减少每个 batch 的 episode 数只能缓解聚合体积，不能彻底解决单条长 trajectory 过大的问题。

#### 2.1.2 证据

`verl_swesmith_grpo_train_20260808_080059` 在第二个 rollout batch 返回结果时中断：

```text
grpc._channel._InactiveRpcError
status = StatusCode.RESOURCE_EXHAUSTED
details = "CLIENT: Received message larger than max (4646840 vs. 4194304)"
```

其中 `4194304` 字节约等于 4 MiB，`4646840` 字节约等于 4.43 MiB。

`verl_swesmith_grpo_train_20260808_152436` 显式设置 `UENV_AGENT_LOOP_BATCH_SIZE=4` 后仍然触发同类错误：

```text
grpc._channel._InactiveRpcError
status = StatusCode.RESOURCE_EXHAUSTED
details = "CLIENT: Received message larger than max (4594969 vs. 4194304)"
```

本地结果文件显示，单条 completed episode 的 token trace 已经可能非常大：

| sample_index | status | `response_ids_len` | `rollout_log_probs_len` | `verl_response_ids_len` |
|---:|---|---:|---:|---:|
| 0 | completed | 6310 | 6310 | 6310 |
| 2 | completed | 214686 | 214686 | 8192 |
| 3 | completed | 10228 | 10228 | 8192 |

#### 2.1.3 解决方案

需要把 Adapter、AdapterCore、Server 和 Worker 之间的 gRPC 消息上限统一提高，避免某一段仍停留在默认 4 MiB。当前链路已按 16 MiB 口径补齐：

| 链路 | 当前上限 |
|---|---:|
| Python Adapter -> Rust AdapterCore client | 16 MiB |
| Rust AdapterCore service | 16 MiB |
| Rust Server -> Worker client | 16 MiB |
| Worker runtime gRPC service | 16 MiB |

后续如果仍出现单条 trajectory 过大，需要继续评估两类方案：提高上限，或对训练不需要的工具日志、长 stdout 和冗余 trace 做裁剪。

### 2.2 Worker 侧并行能力

#### 2.2.1 问题说明

SWE-smith 训练的耗时主要来自 Worker 侧真实环境执行：每条 episode 都需要启动或复用代码环境，让 OpenHands agent 多轮调用模型、编辑文件并运行测试。提高吞吐的直观方案是启用多个 Worker，一个 Worker 对应一台服务器，多台服务器共同处理 episode。

当前资源条件下，Worker 侧只有一台服务器，因此另一种方案是在一个 Worker 内部启用多个处理进程或会话，让多个 episode 同时在各自环境中运行。

一开始我们讨论过由 Adapter 侧显式传递期望并行数，告诉 Worker 本次训练希望跑多少条 episode 并发。但后续讨论认为，这类参数更适合作为观测和调度提示，而不应成为 Worker 侧并行能力的硬约束。Adapter 侧通常希望并行度越高越好，真正可达到的并行度主要取决于 Worker 机器资源、OpenHands agent 池、Runtime Gateway session 数、CPU/内存和环境容器数量。

因此，更合理的方案是 Worker 根据自身资源动态安排最大并行度。当前已实现的阶段性方案是把 Worker 侧并行数设为固定值 4，即一个 Worker 同时处理 4 条 episode。

#### 2.2.2 证据

训练侧已经把调度意图透传到请求元数据中，例如：

```text
expected_worker_parallelism = 8
max_episode_concurrency = 8
target_worker_slots = 8
max_parallel_per_worker = 4
agent_job_max_concurrency = 4
runtime_gateway_session_limit = 4
```

Worker 侧文档与最近联调结论显示，单 Worker 内部实际并行能力受 OpenHands agent 池和 Runtime Gateway session 管理限制。Worker 侧当前方案是把 OpenHands agent 并发和 Runtime Gateway session 上限对齐到 4，并通过 Worker 自身状态暴露 active session、runtime load 等观测指标。

从训练日志看，SWE-smith 单 step 仍然主要耗时在 rollout/generation 阶段，而不是 actor update 阶段。代表性训练指标中：

```text
timing_s/gen ~= 931.95
timing_s/update_actor ~= 39.06
timing_s/update_weights ~= 15.82
timing_s/step ~= 1004.99
```

这说明提高 Worker 侧 episode 并行和环境执行吞吐，是后续优化训练速度的关键方向之一。

#### 2.2.3 解决方案

当前阶段采用 Worker 侧固定并行数 4 的方案，先保证单机多 episode 并发可用，并通过训练日志和前端观测确认实际并行是否生效。

Adapter 侧保留 `UENV_EXPECTED_WORKER_PARALLELISM`、`max_episode_concurrency`、`target_worker_slots`、`max_parallel_per_worker` 等字段，用于表达训练侧期望、记录实验配置和辅助 Server/Worker 调度，但实际并行上限由 Worker 根据资源情况决定。

后续更完整的方案应由 Worker 侧根据机器资源动态调整并发，例如根据 CPU、内存、可用容器、OpenHands agent 池、Runtime Gateway session 和当前负载自动决定可接收 episode 数。多机资源可用后，再扩展为多 Worker 横向并行。

## 3. 训练稳定性问题

训练稳定性问题指单条 episode 或单个基础设施环节出现异常后，训练框架能否把该样本转为可控失败，并继续完成当前 GRPO step。SWE-smith 这类 agentic 任务链路长，涉及模型推理、工具调用、容器执行、测试判分和 token trace 回填，因此需要对局部失败有更强的容错能力。

### 3.1 长测试列表导致 Worker 命令过长

#### 3.1.1 问题说明

部分 SWE-smith 样本包含大量测试项。如果 Worker 把所有测试项直接拼到单条 `docker exec` 命令中，命令参数可能超过操作系统限制，导致 episode 在测试启动阶段失败。

#### 3.1.2 证据

关联 run：

```text
verl_swesmith_grpo_train_20260811_120115
```

第一轮 rollout batch 共 8 条 episode：

```text
agent-loop-requests.jsonl = 8
agent-loop-results.jsonl = 8
completed = 4
failed = 4
```

失败的 4 条都集中在同一个样本：

```text
instance_id = pyparsing__pyparsing.533adf47.combine_file__dsi7jva0
fail_to_pass_count = 476
pass_to_pass_count = 1315
```

Worker 返回的直接错误是：

```text
uenv_runtime.client.GatewayError:
gateway HTTP 500: docker exec spawn failed: Argument list too long (os error 7)
```

失败结果进入 Adapter 后被转为 zero-reward fallback：

```text
status = failed
used_pad_fallback = true
response_ids = []
verl_response_ids = [248044]
verl_response_mask = [0]
rollout_log_probs_len = 0
```

随后 VeRL 在合并 `response_logprobs` 时中断：

```text
TypeError: expected Tensor as element 4 in argument 0, but got NoneType
```

对应位置：

```text
/workspace/verl/verl/experimental/agent_loop/agent_loop.py
optional_outputs["rollout_log_probs"] = torch.cat([input.response_logprobs for input in inputs], dim=0)
```

#### 3.1.3 解决方案

Worker 侧需要改造长测试列表的执行方式，避免把大量 pytest node id 直接拼入单条 `docker exec` 参数。可采用测试列表文件、分批执行或官方 harness 支持的列表输入方式。

Adapter 侧需要补齐失败 episode 的 schema 容错。当 `failed_episode_policy=zero_reward` 且训练配置要求 `calculate_log_probs=True` 时，失败样本也应返回与占位 token 对齐的 dummy `response_logprobs`，确保 VeRL 后处理拿到的每条样本字段类型一致。

这类问题处理完成后，单条 episode 的 Worker 执行失败会稳定转为 `reward=0.0` 的训练样本，当前 GRPO step 可以继续完成。
