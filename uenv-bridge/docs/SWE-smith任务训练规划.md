# SWE-smith 任务训练规划

> 日期：2026-08-04
> 目标任务：SWE 类仓库级程序修复
> 基座模型：`Qwen/Qwen3.6-35B-A3B`
> 训练框架：VeRL + UEnv + OpenHands

## 1. 当前目标

当前阶段优先训练 SWE 类任务，使用 SWE-smith 作为训练数据来源。目标不是先做指令微调，而是直接验证 UEnv 在线环境、OpenHands 工具调用、Worker 判分和 VeRL GRPO 更新能否形成稳定闭环。

训练完成后，使用 SWE-bench-Pro 作为主要训练外评测集，对比基准模型在该任务上的 resolved 指标变化。

## 2. 数据安排

训练数据采用 `SWE-bench/SWE-smith` 中 `problem_statement` 非空的样本。当前 Worker 侧已补齐 SWE-smith 全量 EnvPackage catalog，理论可覆盖约 4.1 万条有效训练样本。

## 3. 训练方式

本阶段采用在线 GRPO 训练：

| 项 | 当前决策 |
|---|---|
| 是否做 SFT | 暂不做 |
| 是否使用 LoRA | 不使用 |
| 参数更新方式 | 全参数 GRPO |
| rollout 环境 | UEnv SWE-smith EnvPackage |
| Agent | OpenHands |
| 并行模式 | 先使用同步 `sync` |
| reward | Worker/OpenHands 执行测试后的 `resolved` / `reward` |
| 失败 episode | Adapter 侧按 `zero_reward` 容错，避免单条环境失败中断整个 step |

## 4. 参数策略

当前参数选择以“先稳定闭环，再扩大训练规模”为原则。

| 维度 | 当前判断 |
|---|---|
| 训练轮次 | 第一阶段先跑 1 个 epoch，重点验证全量数据下训练是否稳定。 |
| batch 规模 | 采用小 batch 起步，降低 SWE 长 rollout、长上下文和全参数更新带来的显存风险。 |
| rollout 采样 | 每个训练样本保留多条 rollout，用 GRPO 组内对比提供相对优势信号。 |
| GPU 使用 | 8 张 A100 以单个 8 卡 TP 推理实例起步，优先保证 rollout 和权重同步稳定。 |
| 输出长度 | SWE 任务需要较长工具调用和代码修改轨迹，response length 采用偏长设置，减少截断。 |
| OpenHands 步数 | 单个 episode 保留较多工具调用步数，使模型有机会查看仓库、定位文件、编辑代码并运行测试。 |
| vLLM 显存 | 保留 sleep/free-cache 与适中的 GPU memory utilization，平衡 KV cache、actor update 和 update weights 阶段显存。 |
| logprobs | 当前保留 rollout logprobs，用于 worker 回传 response trace 并恢复 token id。 |
| checkpoint | 按固定间隔保存，用于训练外评测、异常回退和阶段性对比。 |

SWE/OpenHands 训练需要 worker 回传完整 response trace；当前 worker 通过 OpenAI logprobs 恢复 token id，因此即使同步 GRPO 当前也保留 rollout logprobs。

## 5. 训练轮次估算

正式训练先跑 1 个 epoch。按当前约 4.1 万条非空 SWE-smith 样本和小 batch、多 rollout 的设置估算，一轮全量训练会达到两万级 VeRL step、十万级 episode rollout。

这是一轮比较重的全量训练。正式长跑前先用固定小子集验证 catalog、reward、trajectory、显存和吞吐稳定；稳定后再切换到全量非空样本。

## 6. 执行节奏

正式训练前先做小规模稳定性验证，重点确认 Worker/OpenHands driver 使用的是 SWE-smith 全量 catalog，且 reward、trajectory、response trace、gateway 日志和 VeRL step 都能正常闭环。

稳定性验证通过后，再进入全量训练。全量训练以 1 个 epoch 作为第一阶段目标，训练过程中持续观察 reward、显存峰值、rollout 耗时、失败 episode 类型和输出截断比例。

阶段性 checkpoint 使用 SWE-bench-Pro 全量评测回归，判断 SWE 修复能力是否相对基准模型提升。

## 7. 观察重点

正式训练主要观察：

| 指标 | 目的 |
|---|---|
| `critic/rewards/mean` | reward 是否有有效信号 |
| `response_length/clip_ratio` | 输出是否频繁被截断 |
| `timing_s/gen` | UEnv/OpenHands rollout 是否成为瓶颈 |
| `timing_s/update_actor` | actor 更新耗时与显存压力 |
| `timing_s/update_weights` | 权重同步是否稳定 |
| `git_diff_bytes` | OpenHands 是否实际产生代码修改 |
| `resolved` | SWE 任务最终成功信号 |
| failed / timeout 数量 | 区分模型失败与环境基础设施失败 |

阶段性 checkpoint 使用 SWE-bench-Pro 全量评测回归，核心指标为 `resolved` 数量和 resolved rate。
