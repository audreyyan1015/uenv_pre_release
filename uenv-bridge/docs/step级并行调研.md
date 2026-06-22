# Step 级并行调研

## 1. 背景

当前 UEnv Adapter 的主线是 VeRL pre-rollout 接管：VeRL 在 AgentLoop 阶段把 batch 发给 Adapter，Adapter 经 Rust adapter core 交给 Server/Worker，Server/Worker 完成 rollout、reward，并把 EpisodeResult 返回给 VeRL，随后 VeRL 计算 advantage、loss 并更新 actor。

当前完整训练日志显示，每个 step 的主要耗时集中在 `timing_s/gen`，也就是外部 rollout/generation 阶段。典型同步流程如下：

```text
step t rollout -> reward -> advantage -> actor update -> update rollout weights -> step t+1 rollout
```

如果 rollout 远慢于 actor update，同步 step 会导致训练吞吐受限。因此下一阶段可以考虑 step 级并行，让 rollout、Server/Worker 计算和 VeRL actor update 尽量重叠。

复杂流程图见：

[step-level-parallelism.drawio](asset/step-level-parallelism.drawio)

## 2. 三种训练形态对比

| 形态 | 核心思想 | 数据新鲜度 | 吞吐 | 实现复杂度 | 适合当前阶段 |
|---|---|---:|---:|---:|---:|
| 同步 step | 当前 step 全部完成后再进入下一 step | 最新 | 低 | 低 | 已实现 |
| One-step off-policy | rollout 和 update 做一拍流水线，训练数据最多旧一轮 | 轻微 stale | 中高 | 中 | 推荐优先尝试 |
| Full async | rollout producer、result queue、trainer consumer 全解耦 | 可能 stale 多轮 | 高 | 高 | 后续阶段 |

## 3. 当前同步 step

同步模式的语义最清楚：

```text
1. VeRL 用当前 policy v_t 发起 rollout。
2. Adapter 把 batch 转成 EpisodeRequest。
3. Server/Worker 返回 EpisodeResult。
4. VeRL 基于这个 batch 计算 reward、advantage、loss。
5. actor 从 v_t 更新到 v_t+1。
6. rollout/vLLM 同步到 v_t+1。
7. 下一 step 开始。
```

优点：

| 优点 | 说明 |
|---|---|
| 训练语义清楚 | rollout policy、old log prob、actor update 对应同一个 step |
| debug 简单 | request/result 和 step 一一对应 |
| 对 Adapter 要求低 | Adapter 只需要同步返回当前 batch 的结果 |

缺点：

| 缺点 | 说明 |
|---|---|
| 吞吐受 rollout 限制 | Server/Worker 慢时 VeRL 必须等待 |
| actor update 与 rollout 不能重叠 | `timing_s/gen` 大时整体 step time 高 |
| 慢样本阻塞整个 step | 一个 batch 的最后几个 result 会拖住整个训练 |

## 4. One-step off-policy

One-step off-policy 的目标是让 rollout 和 actor update 重叠，但限制数据最多只旧一轮。(https://github.com/verl-project/verl/blob/main/docs/advance/one_step_off.md)

### 4.1 基本流水线

```text
time 0:
  rollout batch t with policy v_t

time 1:
  trainer updates actor with batch t: v_t -> v_t+1
  rollout side starts batch t+1, using policy v_t or v_t+1

time 2:
  trainer updates actor with batch t+1: v_t+1 -> v_t+2
  rollout side starts batch t+2
```

这里的关键是允许 batch t+1 的 rollout policy 相比 trainer 当前 policy 落后一小步。

### 4.2 需要新增的元数据

Adapter 发出的 EpisodeRequest 需要显式记录：

| 字段 | 说明 | 示例 |
|---|---|---|
| `run_id` | 一次训练运行 ID | `layer4_20260621_101500` |
| `global_step` | VeRL 逻辑 step | `42` |
| `rollout_step` | rollout 请求所属 step | `43` |
| `policy_version` | 生成该 request 时的 actor/rollout 权重版本 | `v42` |
| `max_staleness` | 允许结果落后的最大 policy 版本数 | `1` |
| `batch_id` | Adapter batch ID | `verl-agent-loop-step-42-a1b2c3d4` |
| `request_id` | 单条 episode ID | UUID |

返回的 EpisodeResult 也需要带回或可恢复这些字段，至少要能通过 `request_id` 查回原始 metadata。

### 4.3 Adapter 需要承担的职责

| 职责 | 说明 |
|---|---|
| 记录 policy version | 每批请求发出时记录使用的 policy 版本 |
| 检查 staleness | result 回来时确认没有超过 `max_staleness` |
| 维护 request/result 归属 | 按 `request_id` 回填，不依赖返回顺序 |
| 记录日志 | request/result/gateway 日志都要包含 policy/version 信息 |
| 保留同步 fallback | 出问题时可退回当前同步模式 |

### 4.4 优点和风险

| 类型 | 内容 |
|---|---|
| 优点 | 能把外部 rollout 和 actor update 重叠，改动小于 Full async |
| 优点 | 最大 stale 程度受控，训练稳定性相对可控 |
| 风险 | old log prob、policy version、weight sync 的语义必须严格对齐 |
| 风险 | 如果 Server/Worker 延迟过大，result 仍可能过期 |

### 4.5 VeRL 如何保证 one-step 计算准确性

VeRL 的 one-step off-policy 不是让任意旧样本进入训练，而是把异步范围限制在“一拍流水线”里。实现上，`OneStepOffRayTrainer.fit()` 先启动下一批数据的异步 rollout task，然后 `fit_step()` 等上一批 rollout 结果进入训练；在当前 batch 做 reward、log prob、advantage、actor update 的同时，下一批 rollout 已经在后台生成。

关键准确性约束如下：

| 机制 | VeRL 中的含义 | 准确性作用 |
|---|---|---|
| batch 独立 future | `batch_data_future` 只对应一个 batch 的 rollout 结果 | 避免不同 batch 的 response 混在一起 |
| `uid` | 每条样本进入 rollout 前生成唯一 `uid` | advantage 按样本归属聚合，即使 balance batch 改变顺序也不影响样本级归属 |
| `global_steps` | rollout 请求的 `meta_info` 带当前 `global_steps` | 方便 trace 当前样本由哪个训练 step 发起 |
| rollout 侧 log prob | 配置要求 `actor_rollout_ref.rollout.calculate_log_probs=True` | old log prob 来自生成 response 的 rollout policy，PPO/GRPO ratio 才有意义 |
| rollout correction | 配置中 `algorithm.rollout_correction.bypass_mode=True` | 默认直接使用 rollout log prob；需要更强校正时可接入 rollout correction |
| 权重同步位置 | 当前 batch 被取回后调用 `_fit_update_weights()`，再启动下一批 rollout | 控制 rollout policy 与 trainer policy 的距离，避免无限 stale |

因此 one-step 的准确性不是“等价于严格同步 on-policy”，而是通过“一批 future 对一批训练数据”“rollout log prob 随 response 返回”“每轮启动下一批前同步 rollout 权重”把 off-policy 程度限制在可解释、可监控的范围内。

## 5. Full async

Full async 是更彻底的生产者-消费者模式。(https://github.com/verl-project/verl/blob/main/docs/advance/fully_async.md)

```text
Rollout Producers -> Server/Worker -> Result Queue -> Trainer Consumer
```

rollout worker 持续生成 episode，Server/Worker 持续计算，trainer 持续从 result queue 里取可用 batch 更新 actor。它不再严格等待某个 step 的完整 batch。

### 5.1 必备组件

| 组件 | 作用 |
|---|---|
| Request Queue | 保存待处理 EpisodeRequest |
| Result Queue | 保存已完成 EpisodeResult |
| Policy Version Manager | 记录 rollout policy 和 trainer policy 的版本关系 |
| Backpressure | 队列过长时限制继续发 request |
| Expiration | 丢弃过旧 result |
| Retry/Failure Handling | 处理 Server/Worker 超时、失败、重复返回 |

### 5.2 Adapter/Server/Worker 侧影响

| 模块 | 影响 |
|---|---|
| Python Adapter | 不再只是同步等待当前 batch，需要异步提交和异步收结果 |
| Rust adapter core | 需要支持更明确的 request/result 生命周期和可能的队列语义 |
| Server/Worker | 需要处理更多并发请求，并保留 request metadata |
| Model Gateway | 需要 task/run/policy 感知的日志和限流 |
| VeRL Trainer | 需要从 queue 中取可训练 batch，而不是严格当前 step batch |

### 5.3 主要风险

| 风险 | 表现 | 处理 |
|---|---|---|
| 数据过旧 | trainer 已到 v20，但 result 来自 v12 | 设置 `max_staleness`，过期丢弃 |
| 指标难解释 | step 和 result 不再一一对应 | 指标按 `policy_version`、`rollout_step` 聚合 |
| 失败恢复复杂 | 队列中存在半完成 request | request/result 状态机持久化 |
| 资源打满 | Worker 或 vLLM 被请求淹没 | backpressure、per-run inflight 限制 |
| debug 困难 | 乱序返回导致问题定位困难 | 全链路记录 `run_id/request_id/policy_version` |

### 5.4 VeRL 如何保证 fully async 计算准确性

VeRL fully async 的准确性依赖“样本携带生成时的 log prob + 队列按样本聚合 + 参数版本与 staleness 控制”。它允许 rollout producer 和 trainer consumer 解耦，但不会把没有边界的旧样本无限制送入训练。

关键机制如下：

| 机制 | VeRL 中的含义 | 准确性作用 |
|---|---|---|
| `RolloutSample` | Rollouter 将单条 rollout 结果封装成样本放入 `MessageQueue` | trainer 从队列取样后再组装 batch，避免依赖返回顺序 |
| `required_samples` | trainer 每次取 `ppo_mini_batch_size * require_batches` 个样本 | 保证每次 actor update 的 batch 尺寸满足算法配置 |
| `use_rollout_log_probs=True` | fully async 配置要求 actor 使用 rollout log probs | old log prob 与生成 token 的 policy 对齐，避免用更新后的 actor 重新解释旧 response |
| `calculate_log_probs=True` | rollout/vLLM 侧生成时计算 log probs | response、token、old log prob 在同一个 rollout 结果中闭合 |
| `current_param_version` | trainer 每次参数同步后递增版本 | 记录 trainer 当前权重版本，用于参数同步和日志追踪 |
| `trigger_parameter_sync_step` | trainer 本地更新若干次后再同步 rollout 权重 | 控制 rollout policy 与 trainer policy 的同步频率 |
| `staleness_threshold` | rollouter 根据阈值计算 `max_required_samples` | 限制参数更新之间允许积压的 stale 样本数量 |
| `reset_staleness()` | 每次参数同步后重置 rollouter 的 stale 计数 | 新旧参数切换后重新统计 in-flight 和队列中的旧样本 |
| partial rollout metadata | partial rollout 时记录 `min_global_steps`、`max_global_steps` | 多段生成跨版本时仍能看到 token 生成涉及的版本范围 |

VeRL 文档里也强调：PPO/GRPO 的 old log prob 必须对应 rollout 参数和 token，否则 ratio、KL、loss 的语义会错。fully async 默认通过 rollout log prob 解决这个问题；当需要更强 off-policy 校正时，可以把 `algorithm.rollout_correction.bypass_mode` 设为 `False` 并启用 rollout importance sampling 等 correction。

需要注意的是，fully async 仍然不是严格同步 on-policy。它保证的是：每个训练样本有明确来源、old log prob 与生成策略匹配、stale 程度受配置限制、过多积压会被 backpressure 暂停。训练效果是否稳定，还需要通过 `staleness_threshold`、`require_batches`、`trigger_parameter_sync_step`、reward 曲线和验证指标共同判断。

## 6. 和当前 UEnv Adapter 的关系

当前 Adapter 已经具备几个基础能力：

| 已有能力 | 对 step 并行的价值 |
|---|---|
| pre-rollout 接管 | 能在 rollout 前把 batch 发给 Server/Worker |
| batch patch | 能在 VeRL AgentLoopWorker 层拿到 batch |
| request/result JSONL | 能追踪请求和结果 |
| Model Gateway | 能把 Worker 请求转发到多个 vLLM endpoint |
| `request_id/batch_id/sample_index` | 能做基本结果归属 |

但 step 级并行还需要补齐：

| 缺口 | 说明 |
|---|---|
| `policy_version` | 当前 request/result 没有明确记录权重版本 |
| `rollout_step` | 当前只有 `global_steps`，语义还不够清晰 |
| result staleness 检查 | 当前不会判断结果是否过旧 |
| 异步结果队列 | 当前 AgentLoop 仍等待当前 batch 结果 |
| backpressure | 当前没有限制外部并发积压 |

## 7. 推荐路线

### 7.1 第一阶段：One-step off-policy smoke

目标：评估 step 级流水线是否能提升吞吐，同时不大幅改动训练语义。

建议做法：

```text
1. 在 EpisodeRequest metadata 中补充 policy_version、rollout_step、max_staleness。
2. Adapter result log 记录这些字段。
3. 先不允许超过 one-step stale。
4. 跑 10-20 step，对比同步模式的 step time、gen time、reward。
```

验收指标：

| 指标 | 预期 |
|---|---|
| request/result 数量 | 一致，无重复、无丢失 |
| staleness | 所有可训练 result 的 stale <= 1 |
| step time | 观察是否相比同步模式下降；若未下降，需要定位资源拆分、rollout 长度或同步开销原因 |
| reward | 不出现明显异常退化 |

### 7.2 第二阶段：Bounded staleness

目标：允许结果最多落后 2-3 个版本，观察吞吐和训练稳定性。

新增能力：

```text
max_staleness = 2 or 3
result expiration
per-run inflight limit
```

### 7.3 第三阶段：Full async

目标：将 rollout、Server/Worker、trainer 完全解耦。

不建议当前直接进入 Full async。它需要改动训练调度、队列管理、过期策略和失败恢复，工作量明显高于 one-step off-policy。

## 8. 当前结论

对当前 UEnv Adapter 项目，推荐顺序是：

```text
同步 baseline -> one-step off-policy -> bounded staleness -> full async
```

理由：

| 理由 | 说明 |
|---|---|
| 当前瓶颈明确 | `timing_s/gen` 占 step time 大头，适合先做 rollout/update 重叠 |
| One-step 风险更低 | 只允许落后一轮，训练语义相对可控 |
| Adapter 改动可渐进 | 先补 metadata 和日志，再做异步调度 |
| Full async 成本高 | 需要队列、过期、限流、失败恢复和指标重构 |

因此下一步更合理的是先实现 one-step off-policy 的最小验证，而不是直接切到 full async。

## 9. 纯 VeRL 三种调度实测

本节只比较 VeRL 自身组件，不经过 UEnv Server/Worker，也不启用 UEnvAgentLoop。目的不是验证 Adapter 联调链路，而是单独观察 VeRL 同步训练、one-step off-policy 和 fully async policy 在当前机器上的耗时差异。

### 9.1 测试对象

| 项目 | 同步 VeRL baseline | VeRL one-step off-policy | VeRL fully async policy |
|---|---|---|---|
| 入口 | `python3 -m verl.trainer.main_ppo` | `python3 -m verl.experimental.one_step_off_policy.main_ppo` | `python3 -m verl.experimental.fully_async_policy.fully_async_main` |
| 脚本 | `scripts/onestep_offpolicy/run_verl_grpo_sync_native.sh` | `scripts/onestep_offpolicy/run_verl_grpo_onestep_offpolicy.sh` | `scripts/fully_async_policy/run_verl_grpo_fully_async.sh` |
| 是否经过 Server/Worker | 否 | 否 | 否 |
| 是否启用 UEnvAgentLoop | 否 | 否 | 否 |
| 算法 | GRPO | GRPO | GRPO |
| 数据 | GSM8K parquet | GSM8K parquet | GSM8K parquet |
| 模型 | Qwen2.5-0.5B-Instruct | Qwen2.5-0.5B-Instruct | Qwen2.5-0.5B-Instruct |
| 对齐方式 | `10 step * train_batch_size=4` | `10 step * train_batch_size=4` | `rollout.total_rollout_steps=40`，trainer 处理 10 个 mini-batch |
| 总 GPU 数 | 4 | 4 | 4 |

### 9.2 运行命令

同步 VeRL baseline：

```bash
cd /data/ronghao/uenv/uenv-bridge
TRAINING_STEPS=10 \
TRAIN_BATCH_SIZE=4 \
PPO_MINI_BATCH_SIZE=4 \
PPO_MICRO_BATCH_SIZE_PER_GPU=1 \
ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=1 \
REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=1 \
TEST_FREQ=-1 \
PODMAN_GPU_ARGS="nvidia.com/gpu=0,1,2,3" \
CUDA_VISIBLE_DEVICES_IN_CONTAINER=0,1,2,3 \
NGPUS_PER_NODE=4 \
ROLLOUT_TP=2 \
AGENT_NUM_WORKERS=1 \
RUN_ID=sync_native_10step_20260621_211205 \
./scripts/onestep_offpolicy/run_verl_grpo_sync_native.sh
```

VeRL one-step off-policy：

```bash
cd /data/ronghao/uenv/uenv-bridge
TRAINING_STEPS=10 \
TRAIN_BATCH_SIZE=4 \
PPO_MINI_BATCH_SIZE=4 \
PPO_MICRO_BATCH_SIZE_PER_GPU=1 \
ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=1 \
REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=1 \
TEST_FREQ=-1 \
PODMAN_GPU_ARGS="nvidia.com/gpu=0,1,2,3" \
CUDA_VISIBLE_DEVICES_IN_CONTAINER=0,1,2,3 \
NGPUS_PER_NODE=4 \
TRAINING_GPUS_PER_NODE=2 \
ROLLOUT_GPUS_PER_NODE=2 \
ROLLOUT_TP=2 \
AGENT_NUM_WORKERS=1 \
RUN_ID=onestep_layer4_aligned_10step_20260621_204955 \
./scripts/onestep_offpolicy/run_verl_grpo_onestep_offpolicy.sh
```

VeRL fully async policy：

```bash
cd /data/ronghao/uenv/uenv-bridge
TRAINING_STEPS=10 \
TRAIN_BATCH_SIZE=4 \
PPO_MINI_BATCH_SIZE=4 \
PPO_MICRO_BATCH_SIZE_PER_GPU=1 \
ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=1 \
REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=1 \
TEST_FREQ=-1 \
PODMAN_GPU_ARGS="nvidia.com/gpu=0,1,2,3" \
CUDA_VISIBLE_DEVICES_IN_CONTAINER=0,1,2,3 \
NGPUS_PER_NODE=4 \
TRAINING_GPUS_PER_NODE=2 \
ROLLOUT_GPUS_PER_NODE=2 \
ROLLOUT_TP=2 \
RAY_NUM_CPUS=64 \
FULLY_ASYNC_TRIGGER_PARAMETER_SYNC_STEP=1 \
FULLY_ASYNC_REQUIRE_BATCHES=1 \
FULLY_ASYNC_STALENESS_THRESHOLD=0.1 \
FULLY_ASYNC_PARTIAL_ROLLOUT=False \
RUN_ID=fully_async_layer4_aligned_10step_20260621_222641 \
./scripts/fully_async_policy/run_verl_grpo_fully_async.sh
```

fully async 第一次尝试时 `RAY_NUM_CPUS=26` 不够，Ray 卡在创建 `FullyAsyncRollouter`，日志报 `{'CPU': 10.0}` 无法调度。原因是 fully async 额外启动 `FullyAsyncTrainer`、`FullyAsyncRollouter`、`MessageQueue` 等 actor，其中 trainer 和 rollouter 代码里各自声明了 `num_cpus=10`。脚本已把默认 `RAY_NUM_CPUS` 调整为 `64`，本次实测使用该配置跑通。

### 9.3 日志路径

| 项目 | 日志 |
|---|---|
| 同步 VeRL baseline | `temp/logs/verl_sync_native/sync_native_10step_20260621_211205.log` |
| VeRL one-step off-policy | `temp/logs/verl_onestep_offpolicy/onestep_layer4_aligned_10step_20260621_204955.log` |
| VeRL fully async policy | `temp/logs/verl_fully_async/fully_async_layer4_aligned_10step_20260621_222641.log` |

资源布局：

| 项目 | 训练侧 | rollout/vLLM 侧 | 备注 |
|---|---|---|---|
| 同步 VeRL baseline | colocate/hybrid | 两个 TP=2 vLLM endpoint，分别使用 `0,1` 和 `2,3` | rollout 阶段可用 4 张 GPU |
| One-step off-policy | GPU `0,1` | 一个 TP=2 vLLM endpoint，使用 `2,3` | rollout 与 update 可以重叠，但 rollout 只有 2 张 GPU |
| Fully async policy | GPU `0,1` | 一个 TP=2 vLLM endpoint，使用 `2,3` | 通过 `MessageQueue` 流式传样本 |

fully async 日志中可以看到：

```text
Total rollout steps: 40
required_samples : 4 max_required_samples: 4 max_queue_size: 4 total_train_steps: 10
LLMServerManager: ['10.10.20.142:42053']
CUDA_VISIBLE_DEVICES: 2,3
```

### 9.4 指标对比

`step1` 通常包含 Ray、FSDP、vLLM 初始化和首次权重同步，冷启动开销明显高于后续 step。因此下面同时给出全量均值和去掉首条 step metric 后的均值。fully async 的详细 metric 从 `step:2` 开始打印，所以该列的样本数是 9，而不是 10。

| 指标 | 同步全量均值 | 同步去首条 | One-step 全量均值 | One-step 去首条 | Fully async 可解析均值 | Fully async 去首条 |
|---|---:|---:|---:|---:|---:|---:|
| `timing_s/step` | 17.690s | 16.215s | 23.677s | 21.617s | 28.357s | 27.175s |
| `timing_s/gen` | 13.906s | 12.712s | 19.262s | 17.493s | 23.973s | 23.109s |
| `timing_s/update_actor` | 1.257s | 1.212s | 2.239s | 2.209s | 2.196s | 2.154s |
| `timing_s/ref` | 0.538s | 0.456s | 1.441s | 1.192s | 1.467s | 1.191s |
| 权重同步 | `update_weights` 1.415s | 1.380s | `update_weights` 0.727s | 0.715s | `param_sync` 0.708s | 0.708s |
| `perf/throughput` | 115.42 | 121.35 | 178.35 | 186.58 | 74.39 | 76.30 |
| `critic/rewards/mean` | 0.0050 | 0.0056 | 0.0050 | 0.0056 | 0.0056 | 0.0063 |
| `response_length/mean` | 281.74 | 282.19 | 291.61 | 286.92 | 305.96 | 301.58 |

端到端耗时：

| 项目 | 训练进度条耗时 | 额外 `total time` |
|---|---:|---:|
| 同步 VeRL baseline | 2m56s | 日志未打印 |
| One-step off-policy | 3m56s | 333.13s |
| Fully async policy | 4m39s | 392.05s |

fully async 关键异步指标：

| 指标 | 均值 | 去首条均值 | 说明 |
|---|---:|---:|---|
| `fully_async/trainer/idle_ratio` | 0.843 | 0.846 | trainer 大部分时间在等待 rollout 样本 |
| `fully_async/rollouter/idle_ratio` | 0.146 | 0.161 | rollouter 闲置较少，说明瓶颈更偏 rollout 产样速度 |
| `fully_async/total_wait_time` | 23.958s | 23.097s | trainer 每次取 4 个样本的等待时间 |

每 step 的 `timing_s/step`：

| step | 同步 VeRL | One-step | Fully async |
|---:|---:|---:|---:|
| 1 | 30.968 | 42.221 | - |
| 2 | 13.860 | 14.081 | 37.814 |
| 3 | 16.063 | 20.914 | 22.420 |
| 4 | 16.795 | 26.600 | 29.195 |
| 5 | 16.691 | 22.468 | 31.284 |
| 6 | 22.332 | 30.815 | 32.644 |
| 7 | 16.191 | 22.078 | 34.656 |
| 8 | 15.299 | 14.607 | 24.136 |
| 9 | 13.049 | 23.945 | 20.683 |
| 10 | 15.654 | 19.045 | 22.385 |

### 9.5 结论

在本次 `4 GPU total / TRAIN_BATCH_SIZE=4 / ROLLOUT_N=5 / ROLLOUT_TP=2 / 10 step` 配置下，纯 VeRL 同步 baseline 的 wall-clock step time 仍然最短：

```text
同步 VeRL 去首条平均 step time: 16.215s
One-step 去首条平均 step time: 21.617s
Fully async 去首条平均 step time: 27.175s
```

fully async 能正常跑通，但没有带来加速。核心原因不是 fully async 机制不可用，而是当前小规模配置不适合发挥它的优势：

| 原因 | 说明 |
|---|---|
| GPU 被拆分 | 同步 baseline 的 rollout 可用 4 张 GPU；fully async 只有 2 张 GPU 做 rollout，另外 2 张给 trainer |
| batch 太小 | `PPO_MINI_BATCH_SIZE=4`，`required_samples=4`，`max_queue_size=4`，队列没有足够积压来隐藏 rollout 长尾 |
| 参数同步频繁 | `trigger_parameter_sync_step=1`，每处理 4 个样本就同步一次参数 |
| trainer idle 高 | `trainer/idle_ratio≈0.84`，说明 trainer 大量时间在等 rollout 样本 |
| 总样本数少 | `rollout.total_rollout_steps=40`，启动和调度成本占比高 |

因此当前结论是：在 4 GPU、小 batch、短 10 step 的约束下，不能假设 one-step 或 fully async 会比同步更快。它们更适合训练和 rollout 拥有独立 GPU 池、样本数更大、rollout 长尾更明显的场景。对 UEnv Adapter 当前阶段，更现实的优先级仍是优化同步 pre-rollout 链路、Server/Worker batch 调度、模型 endpoint 吞吐和日志可观测性；当资源允许扩展到 `training 4 GPU + rollout 4 GPU` 或更大规模时，再重新评估 one-step、bounded staleness 和 fully async。

## 10. 8GPU 资源切分实验

本节记录 2026-06-22 的 8GPU 资源切分实验。实验目标是验证在 rollout 是瓶颈的前提下，是否应该把更多 GPU 分给 rollout 侧，并对比三类 VeRL 执行模式：完全同步、one-step off-policy、fully async。

本实验是纯 VeRL 对比实验，不经过 UEnv Server/Worker，也不经过 Adapter model gateway。它的作用是为后续中转站应该暴露多少个 vLLM endpoint、以及 trainer/rollout 资源如何切分提供依据。

### 10.1 统一配置

| 配置项 | 值 |
|---|---|
| GPU | 8 张，`0,1,2,3,4,5,6,7` |
| `TRAINING_STEPS` | 5 |
| `TRAIN_BATCH_SIZE` | 16 |
| `PPO_MINI_BATCH_SIZE` | 16 |
| `PPO_MICRO_BATCH_SIZE_PER_GPU` | 1 |
| `ROLLOUT_N` | 5 |
| `ROLLOUT_TP` | 2 |
| `DATA_MAX_RESPONSE_LENGTH` | 1024 |
| `TEST_FREQ` | -1 |
| 数据集 | GSM8K |
| 模型 | Qwen2.5-0.5B-Instruct |

### 10.2 资源切分

| 方案 | trainer/update | rollout | vLLM endpoint 形态 |
|---|---:|---:|---|
| 完全同步 | 8 GPU 共享 | 8 GPU 共享 | 4 个 TP=2 rollout endpoint |
| one-step off-policy 6/2 | 2 GPU | 6 GPU | 3 个 TP=2 rollout endpoint |
| one-step off-policy 4/4 | 4 GPU | 4 GPU | 2 个 TP=2 rollout endpoint |
| fully async 6/2 | 2 GPU | 6 GPU | 3 个 TP=2 rollout endpoint |
| fully async 4/4 | 4 GPU | 4 GPU | 2 个 TP=2 rollout endpoint |

### 10.3 结果表

表中 `avg step`、`avg gen`、`avg update_actor` 等均为去掉首个 warm step 后的平均值。首个 step 包含 Ray、vLLM、参数同步和异步队列填充成本，单独用于判断冷启动开销，不适合作为 steady-state 指标。`perf/throughput` 是 VeRL 日志中的训练侧指标，不同模式的分母不同，只作为辅助参考。

| 方案 | 是否完成 | avg step(s) | avg gen(s) | avg async gen(s) | avg ref(s) | avg update_actor(s) | trainer idle | 备注 |
|---|---|---:|---:|---:|---:|---:|---:|---|
| 完全同步 | 完成 5/5 | 30.19 | 24.71 | - | 0.69 | 2.45 | - | 同步基线，8 GPU 同时参与训练与 rollout |
| one-step 6/2 | 完成 5/5 | 32.08 | 18.11 | 31.77 | 4.39 | 8.71 | - | rollout 变快，但 2 GPU trainer 让 ref/update 变慢 |
| one-step 4/4 | 完成 5/5 | 46.88 | 40.22 | 46.68 | 1.56 | 4.29 | - | trainer 更快，但 rollout endpoint 从 3 个降到 2 个，整体明显变慢 |
| fully async 6/2 | 完成 5/5 | 30.19 | 15.85 | - | 4.47 | 9.07 | 0.53 | step 时间接近同步基线，但 trainer 仍在等待 rollout |
| fully async 4/4 | 未完成 | - | - | - | - | - | - | 1/5 后停止，见 10.6 |

### 10.4 时间差距图

图 10-1 展示 one-step off-policy 6/2 配置下几个核心时间的关系。这里 `avg async gen=31.77s` 表示后台 rollout 的真实生成总时长，`avg gen=18.11s` 表示 trainer 当前 step 实际等待 rollout 结果的可见阻塞时间，两者差值约 `13.66s`，对应被上一轮训练计算覆盖掉的生成时间。

图文件：[`docs/asset/one-step-8gpu-timing-gap.drawio`](asset/one-step-8gpu-timing-gap.drawio)

### 10.5 日志位置

| 方案 | 日志 |
|---|---|
| 完全同步 | `temp/logs/verl_sync_native/sync_native_8gpu_b16_5step_20260622_135613.log` |
| one-step 6/2 | `temp/logs/verl_onestep_offpolicy/onestep_8gpu_t2r6_b16_5step_20260622_140249.log` |
| one-step 4/4 | `temp/logs/verl_onestep_offpolicy/onestep_8gpu_t4r4_b16_5step_20260622_141728.log` |
| fully async 6/2 | `temp/logs/verl_fully_async/fully_async_8gpu_t2r6_b16_5step_20260622_140928.log` |
| fully async 4/4 | `temp/logs/verl_fully_async/fully_async_8gpu_t4r4_b16_5step_20260622_142528.log` |

### 10.6 fully async 4/4 失败点

fully async 4/4 没有完成有效 5-step。日志中出现：

```text
AttributeError: 'list' object has no attribute 'dim'
Training stopped by queue termination signal
not enough samples collected after loop
```

错误位置在 VeRL experimental agent loop 的 postprocess 阶段：

```text
verl/experimental/agent_loop/agent_loop.py
if response_output["input_ids"].dim() == 1:
```

这说明某些 rollout task 返回的 `input_ids` 是 list，而不是预期的 tensor。该问题出现在 VeRL fully async experimental 链路内部，本次未修改 VeRL 源码处理它。由于 6/2 fully async 能完成，4/4 失败更可能与较少 rollout replica 下的并发/终止时序有关，需要后续单独验证。

### 10.7 结论

本轮实验不支持“4 rollout + 4 update 更合适”。在 one-step off-policy 中，4/4 的 trainer 计算确实更快，但 rollout endpoint 从 3 个降到 2 个后，生成阶段显著变慢，steady-state step 从 32.08s 退化到 46.88s。因此在当前 `TRAIN_BATCH_SIZE=16`、`ROLLOUT_TP=2`、rollout 明显偏慢的配置下，6 rollout + 2 update 比 4/4 更合理。

6/2 fully async 的 steady-state step 约 30.19s，已经接近完全同步基线，但 trainer idle ratio 仍约 0.53，说明 trainer 仍有一半左右时间在等 rollout sample。它没有明显超过同步基线，主要是因为 trainer 侧只有 2 张 GPU，`ref` 和 `update_actor` 变慢，同时 async queue 的 staleness 阈值会让 rollouter 暂停。

对中转站设计的直接启示是：当 VeRL 以 `ROLLOUT_TP=2` 启动多个 rollout endpoint 时，中转站应优先把 Worker 请求均匀打到多个 endpoint 上。当前 6/2 会提供 3 个 TP=2 endpoint，比 4/4 的 2 个 endpoint 更能缓解 rollout 瓶颈。后续如果要继续优化，需要系统扫 `5/3`、`6/2`、`7/1`，并联动调整 `TRAIN_BATCH_SIZE`、queue size、staleness threshold 和 `ROLLOUT_TP`。
