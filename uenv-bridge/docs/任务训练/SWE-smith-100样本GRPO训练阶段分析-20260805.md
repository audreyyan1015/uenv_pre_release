# SWE-smith 100 样本 GRPO 训练阶段分析

> 分析日期：2026-08-05
> Run ID：`verl_swesmith_grpo_train_20260805_102624`
> 模型：`Qwen/Qwen3.6-35B-A3B`
> 结论状态：**本次环境 rollout 已覆盖 100 个任务，但 VeRL 优化只确认完成 15/50 step，且所有 GRPO 组 reward 为 0，不能作为有效训练或 checkpoint 效果的结论。**

## 1. 产物与口径

| 产物 | 路径 | 用途 |
|---|---|---|
| VeRL 主日志 | `temp/logs/verl_layer4_agent_loop/verl_swesmith_grpo_train_20260805_102624.log` | 确认训练 step、loss、显存与时延 |
| AgentLoop 请求 | `temp/logs/layer4_distributed/verl_swesmith_grpo_train_20260805_102624/agent-loop-requests.jsonl` | 确认训练样本与 rollout 分组 |
| AgentLoop 结果 | `temp/logs/layer4_distributed/verl_swesmith_grpo_train_20260805_102624/agent-loop-results.jsonl` | 统计环境结果、reward、轨迹和测试字段 |
| 模型网关 | `temp/logs/layer4_distributed/verl_swesmith_grpo_train_20260805_102624/model-gateway.jsonl` | 统计模型请求与网关错误 |

统计时以 `uid` 识别 prompt，以同一 `uid` 的 4 条 rollout 识别一个 GRPO 组。主日志只把已完成 actor update 的 `step:N` 视为已完成训练 step；仅存在 AgentLoop 结果不等价于模型已经更新。

## 2. 实际生效配置

| 参数 | 本次值 | 说明 |
|---|---:|---|
| `TRAIN_BATCH_SIZE` | 2 | 每个 VeRL step 含 2 个 prompt |
| `ROLLOUT_N` | 4 | 每个 prompt 产生 4 条 rollout |
| 每个 rollout batch | 8 条 episode | 共 50 个 batch、400 条 episode |
| `PPO_MINI_BATCH_SIZE` | 2 | VeRL 内部按 rollout 数展开 |
| `PPO_MICRO_BATCH_SIZE_PER_GPU` | 1 | 保守显存配置 |
| `ROLLOUT_TP` | 8 | 单个 8 卡 vLLM 推理实例 |
| `DATA_MAX_RESPONSE_LENGTH` | 8192 | 回传到 VeRL 的 response 上限 |
| `max_model_len` | 131072 | vLLM 最大上下文 |
| `max_num_seqs` | 8 | 已较前一轮提升并发上限 |
| `max_num_batched_tokens` | 65536 | chunked prefill 调度上限 |
| `ROLLOUT_GPU_MEMORY_UTILIZATION` | 0.50 | vLLM KV cache 预留 |
| sleep/free-cache | 开启 | actor update 前释放 vLLM cache |
| actor learning rate | `1e-6` | 当前保守学习率 |
| KL coefficient | `0.001` | 低方差 KL loss |
| 失败策略 | `zero_reward` | 所有环境失败被转换为 0 reward |

## 3. 本次训练结果

### 3.1 训练闭环进度

| 指标 | 结果 |
|---|---:|
| 唯一 prompt 数 | 100 |
| AgentLoop 请求 / 结果 | 400 / 400 |
| rollout batch 数 | 50 |
| 每 batch episode 数 | 8 |
| 每个 GRPO 组 rollout 数 | 4 |
| VeRL 已记录 update step | 15 / 50（30%） |
| 已确认 completed episode | 390 / 400（97.5%） |
| failed episode | 10 / 400（2.5%） |

请求和结果中已经存在 `global_steps=1..50` 的完整记录，但 VeRL 主日志最后只到 `step:15`，随后开始 `batch_id=...step-16...` 后没有新的 `step:N` 指标或训练完成标记。主日志最后修改时间为 2026-08-05 14:09，而 result 文件最后修改时间为 22:38。

因此，step 16-50 的环境结果不能证明对应的 old-logprob、优势计算、actor update 和权重同步已完成。该异步/同步边界的时序错位是本次运行的首要工程问题。

### 3.2 Reward 与策略梯度

| 指标 | 结果 |
|---|---:|
| reward=1 episode | 0 / 400 |
| reward=0 episode | 400 / 400 |
| `resolved=True` | 0 / 390 completed |
| 有 reward 方差的 GRPO 组 | 0 / 100 |
| `critic/rewards/mean` | 已记录的 15 step 均为 0 |
| `critic/advantages/mean` | 已记录的 15 step 均为 0 |
| `actor/pg_loss` | 已记录的 15 step 均为 0 |
| `actor/ppo_kl` / `actor/pg_clipfrac` | 已记录的 15 step 均为 0 |

当前运行没有来自 GRPO 的 policy gradient。日志中仅有很小的 KL loss（约 `0.0045-0.0059`）和约 `0.0017-0.0036` 的梯度范数，不能视为任务 reward 驱动的能力更新。因此本次不应保存或评估为训练后 checkpoint。

390 条完成 episode 中，197 条产生了非空 git diff，说明 OpenHands 能执行编辑动作；但最终 `resolved` 均为 false。`tests_passed` 中位数为 531、90 分位为 532、最大值为 536，表明存在部分测试进展信息，但原始 `tests_passed` 不可直接作为跨任务 reward，必须相对任务初始状态或目标 F2P/P2P 测试计算。

### 3.3 上下文与 response trace

| 指标 | 原始完整 trace | 回传 VeRL 的 trace |
|---|---:|---:|
| token 中位数 | 6308 | 6308 |
| token P90 | 9598 | 8192 |
| token P95 | 11477 | 8192 |
| 最大 token 数 | 76911 | 8192 |
| 超过 8192 条数 | 82 / 400（20.5%） | 被截断为 8192 |
| 空 trace | 10 | 0，使用 pad fallback |

20.5% 的完整 agent trace 在进入 VeRL 前被截断。现有桥接路径保留前 8192 token，长轨迹中后续的工具观察、模型决策和最终 patch 可能不参与 loss；这会损害多轮 agent 的 credit assignment。

此外，10 条失败均为同一种 vLLM `ContextWindowExceeded`：输入至少 126977 token，再请求 4096 输出 token，超过模型 131072 token 上限。将 `max_model_len` 设为 131072 没有解决上下文增长，只是把失败延后到模型硬上限。

### 3.4 性能与资源

下表仅统计主日志中确认完成的 step 2-15，排除第一步冷启动。

| 指标 | 均值 | 范围 |
|---|---:|---:|
| `timing_s/gen` | 720.08 s | 583.51-1109.39 s |
| `timing_s/old_log_prob` | 13.22 s | 11.07-15.69 s |
| `timing_s/ref` | 4.86 s | 4.24-5.43 s |
| `timing_s/update_actor` | 37.10 s | 33.13-42.04 s |
| `timing_s/update_weights` | 16.78 s | 15.88-17.91 s |
| `timing_s/step` | 792.05 s | 656.38-1179.01 s |
| `perf/throughput` | 8.55 token/s | 4.74-9.90 token/s |

rollout 占稳定 step 时间约 91%，因此当前主要瓶颈是 OpenHands/环境长回合，而不是 actor、ref 或权重同步。没有发现 CUDA OOM 或 vLLM EngineCore fatal error；step 15 时 actor GPU 最大 allocated/reserved 分别约为 47.21 GB / 68.99 GB，8 张 A100 80 GB 显存仍有余量。

虽然 `max_num_seqs` 已提高到 8，但 vLLM 运行日志大多显示 `Running: 1, Waiting: 0`，KV cache 使用率约 0-4%。这说明环境/agent 侧尚未形成足够的并发模型请求，单纯继续增大 vLLM 并发上限或 GPU KV 预留不会明显改善端到端吞吐。

网关共记录 11987 次模型请求，其中 11977 次 HTTP 200、10 次 HTTP 400；P50/P95 延迟为 0.90 s / 4.99 s。模型调用本身并非 12-18 分钟单 step 的主要耗时来源。

## 4. 问题归因

| 优先级 | 问题 | 证据 | 对训练的影响 |
|---|---|---|---|
| P0 | GRPO 没有有效 reward | 100 个组全 0，`pg_loss=0`、advantage=0 | 模型没有学习，继续训练只消耗资源 |
| P0 | 上下文预算未闭合 | 10 条 `ContextWindowExceeded`，输入 126977 + 输出 4096 | 环境错误被误记为失败 rollout，且使 run 无法稳定收敛 |
| P0 | rollout 与 VeRL update 时序错位 | 结果覆盖 step 1-50，主日志只完成 step 1-15 | 无法证明后 70% rollout 已被用于正确的 on-policy update |
| P0 | 长轨迹被静默截断 | 82 条完整 trace 超过 8192 token | 最终关键决策可能不在训练 token 中，old/new logprob 条件上下文不完整 |
| P1 | `zero_reward` 污染组内比较 | 10 条基础设施错误被赋为 0 | 环境质量与模型能力混淆；本次虽全 0，后续会制造伪优势 |
| P1 | 终局 reward 过稀疏 | 完成率高但 resolved 为 0 | 无法从编辑和局部测试进展获得学习信号 |
| P1 | 端到端并发不足 | vLLM 大多只有 1 个运行请求，KV 使用低 | 8 卡 TP 推理等待 OpenHands/环境，不是计算饱和 |

## 5. 后续改进措施

### 5.1 先恢复训练正确性

1. 在 OpenHands 侧实施上下文压缩和硬预算。每个模型调用必须保存实际使用的压缩后 prompt，并在调用前限制 `input_tokens + max_tokens`；建议在 96k token 左右开始压缩，为工具输出和 4096 token 生成预留余量。
2. 完整原始轨迹保存到冷存储；VeRL 不需要加载未压缩历史，但每个 LLM turn 必须返回实际 `effective_prompt_ids`、assistant `response_ids`、loss mask 和行为策略 logprobs。工具观察和摘要 token 应作为非 loss token 保留在序列中。
3. 调试阶段将失败策略改为 `raise`，首先消灭环境异常。正式容错需要实现“重试后丢弃整个 GRPO 组”，不能把安装、超时、上下文错误直接作为模型的 0 reward。
4. 为每个训练 batch 增加不可变的生命周期记录：`rollout_completed`、`old_logprob_completed`、`actor_update_committed`、`checkpoint_committed`。仅在 `actor_update_committed` 后将该 batch 计入训练进度。

### 5.2 恢复有效 reward 信号

1. 先构造可解性更高的 100-500 条课程子集，使同一 prompt 的 4 条 rollout 中同时出现成功和失败，验证 GRPO advantage 非零。
2. 保持 `resolved` 作为唯一主奖励和训练外指标；辅助奖励使用相对初始状态的 F2P 通过数、P2P 保持率和有效 patch 校验，避免使用跨任务不可比的原始 `tests_passed`。
3. 辅助奖励应有上限，且最终 resolved 仍占主要权重；每个 reward 分量与失败原因必须写入 result，便于按组过滤和回放。

### 5.3 在正确性通过后优化效率

1. 保持当前 `TP=8`、`micro_batch=1`、learning rate `1e-6` 和 KL coefficient `0.001`，在出现非零 advantage 前不要调大学习率或 rollout 数。
2. 先将 OpenHands/container 的真实并发槽位提升到至少 8，并验证 vLLM 存在多个同时运行请求；达到该条件后再比较 `max_num_seqs=8` 与 16，及 `gpu_memory_utilization=0.50` 与 0.55。
3. 在无上下文错误、trace 不截断后，再将 `TRAIN_BATCH_SIZE` 从 2 扩大到 4。扩大 batch 前必须保证 `TRAIN_BATCH_SIZE * ROLLOUT_N` 与 AgentLoop worker 分片、UEnv 容量一致。
4. response trace 若仍超过 8192，优先完成 turn-level 训练序列和动态 token batch，而不是盲目提高单条 `DATA_MAX_RESPONSE_LENGTH` 并冒显存风险。

## 6. 下一轮验收门槛

下一轮 20-step、100 样本验证必须同时满足以下条件，才允许扩大数据规模或评估 checkpoint：

| 验收项 | 通过条件 |
|---|---|
| 训练完成度 | `actor_update_committed` 数量等于计划 step 数；主日志出现 `Training Progress: 100%` |
| 上下文稳定性 | `ContextWindowExceeded=0`，无 vLLM engine fatal 或 CUDA OOM |
| 环境数据质量 | 环境失败有明确分类且不进入 GRPO reward；失败率低于 1% |
| 轨迹完整性 | 不存在静默 8192-token 截断，或已采用完整的 turn-level loss mask 表示 |
| 学习信号 | 至少存在 reward 有方差的 GRPO 组，`critic/advantages` 与 `actor/pg_loss` 非零且可解释 |
| 并发证据 | Agent、UEnv worker、gateway 和 vLLM 的实际并发数可观测，且端到端吞吐提升有对照数据 |

在上述门槛通过前，本次 run 仅可作为 UEnv/OpenHands 任务执行、轨迹收集与故障定位证据，不能用于判断 GRPO 训练效果。
