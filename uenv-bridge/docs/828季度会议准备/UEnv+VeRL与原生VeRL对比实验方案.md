# UEnv+VeRL 与原生 VeRL 对比实验方案

## 1. 实验目标

本实验用于说明 UEnv+VeRL 相比原生 VeRL 在复杂环境任务中的系统价值。对比重点不是重新验证 VeRL 的训练算法能力，而是评估在 SWE、Web、工具调用等需要外部环境交互的任务中，UEnv 是否能够提供更稳定、更易扩展、更可观测的训练执行能力。

实验设计需要区分两类问题：第一类是简单任务上的训练正确性和接入开销，即 UEnv+VeRL 是否能在相同训练预算下保持接近原生 VeRL 的训练效果；第二类是复杂环境任务上的系统收益，即 UEnv 是否能在多环境执行、失败容错、并行扩展和可观测方面体现优势。

## 2. 对比对象

| 方案 | 定义 | 主要作用 |
| --- | --- | --- |
| 原生 VeRL | 直接使用 VeRL 训练框架运行 GRPO/PPO 等算法 | 提供强化学习训练、rollout、参数更新与 checkpoint |
| UEnv+VeRL | VeRL 负责训练算法，UEnv 负责环境任务执行、结果回传和运行观测 | 在复杂任务中统一环境交互、任务调度、结果收集和可观测 |

## 3. 核心对比维度

| 维度 | 对比内容 |
| --- | --- |
| 训练效果 | reward、resolved、训练前后 benchmark 指标变化 |
| 训练效率 | step 耗时、rollout 耗时、update 耗时、episode 吞吐 |
| 系统稳定性 | episode 失败率、超时率、异常恢复能力、checkpoint 恢复成功率 |
| 任务扩展性 | 新任务接入成本、多任务复用能力、环境执行逻辑复用程度 |
| 可观测性 | 是否能看到 run、episode、worker、timeline 等多层运行状态 |

## 4. 实验设计

### 4.1 实验 A：轻量任务正确性与开销对齐

目标是在无复杂外部环境的任务上对比原生 VeRL 与 UEnv+VeRL，评估 UEnv 接入层带来的额外开销，并明确 UEnv 的能力边界。该实验不用于突出 UEnv 的主要优势，而是证明接入 UEnv 后训练闭环没有破坏 VeRL 原有训练能力。

| 项目 | 原生 VeRL | UEnv+VeRL |
| --- | --- | --- |
| 任务 | PubMedQA、SciTab、DS-CodeBench | 同一数据集 |
| 模型 | Qwen3.6-35B-A3B | Qwen3.6-35B-A3B |
| 算法 | GRPO | GRPO |
| batch / rollout | 保持一致 | 保持一致 |
| 数据划分 | 固定 90% 训练、10% 验证 | 使用完全相同的划分 |
| 环境交互 | 无复杂外部环境，仅模型生成与答案判分 | 通过 UEnv episode 流程完成同等生成与判分 |
| 关注指标 | step 耗时、tokens/s、reward 曲线、验证集指标 | step 耗时、tokens/s、reward 曲线、验证集指标、episode 调度开销 |

数据划分采用固定随机种子生成，确保两种方案使用完全相同的 train/eval 文件。若数据集本身提供官方验证集或测试集，则官方划分作为最终效果评估依据，90/10 划分作为内部对齐实验口径。

该实验用于说明：在 PubMedQA、SciTab、DS-CodeBench 这类轻量任务中，UEnv+VeRL 应保持与原生 VeRL 接近的训练效果，同时量化统一 episode 流程带来的系统开销。若两者效果接近但 UEnv 存在少量额外耗时，可以作为后续解释复杂任务收益的基础参照。

### 4.2 实验 B：SWE 任务公平对比

目标是比较原生 VeRL 接复杂环境任务与 UEnv+VeRL 接复杂环境任务时的训练闭环差异。

| 项目 | 原生 VeRL | UEnv+VeRL |
| --- | --- | --- |
| 任务 | SWE-smith 子集 | SWE-smith 子集 |
| 模型 | Qwen3.6-35B-A3B | Qwen3.6-35B-A3B |
| 算法 | GRPO | GRPO |
| batch / rollout | 保持一致 | 保持一致 |
| 最大输出长度 | 保持一致 | 保持一致 |
| 环境执行 | 自定义 AgentLoop 接入 SWE/OpenHands | UEnv 统一 episode 执行 |
| reward | SWE resolved / test pass 结果 | SWE resolved / test pass 结果 |

原生 VeRL 侧的 SWE 接入可以有两种实现路径：一是我们自行实现 SWE/OpenHands 专用 AgentLoop；二是参考或复用网上已有的 SWE-agent / RemoteAgentLoop 相关方案。需要注意的是，这些现有方案更多是 SWE-agent 或远程 agent 框架方向，是否能直接替代 OpenHands 仍需要实际适配和验证。

| 来源 | 可参考内容 | 说明 |
| --- | --- | --- |
| VeRL AgentLoop 文档 | https://verl.readthedocs.io/en/latest/advance/agent_loop.html | 说明 VeRL 如何注册和运行自定义 AgentLoop |
| verl-recipe | https://github.com/verl-project/verl-recipe | VeRL 社区 recipe 仓库，可查看是否已有 SWE-agent 相关实现 |
| SWE-Agent recipe PR | https://github.com/verl-project/verl-recipe/pull/91 | 社区 SWE-agent recipe 方向，可作为原生 VeRL baseline 的参考 |
| RemoteAgentLoop issue | https://github.com/verl-project/verl/issues/5737 | VeRL 社区讨论将外部 agent 作为远程 AgentLoop 接入 |
| 阿里 ACK 示例 | https://help.aliyun.com/zh/ack/training-agentic-reinforcement-learning-on-ack-using-the-verl-framework | 展示 VeRL、RemoteAgentLoop 和 SWE-agent/Harbor 结合的工程方案 |

建议先使用 100 到 500 条 SWE-smith 子集完成可控对比，再扩展到更大规模训练。该实验重点观察：

| 指标 | 含义 |
| --- | --- |
| resolved 数量 | 训练过程中产生有效修复的数量 |
| 非零 reward 占比 | GRPO 是否获得有效学习信号 |
| episode 成功完成率 | 环境执行链路是否稳定 |
| 单 step 平均耗时 | 训练循环整体效率 |
| rollout 阶段耗时 | 外部环境交互是否成为瓶颈 |
| 失败样本处理 | 单条失败是否会导致整轮训练中断 |

### 4.3 实验 C：环境执行扩展性对比

目标是评估 UEnv 在多环境任务下的扩展能力。

| 设置 | 说明 |
| --- | --- |
| 固定训练资源 | 8 卡训练侧保持不变 |
| 变化环境资源 | Worker 槽位从 1、2、4、8 逐步增加 |
| 任务 | SWE-smith 固定子集 |
| 观察指标 | episodes/hour、step time、worker 利用率、失败率 |

该实验用于回答：当训练侧等待环境执行时，增加环境执行并行度是否能提升整体吞吐。

## 5. 指标口径

### 5.1 训练效果指标

| 指标 | 说明 |
| --- | --- |
| reward mean | 每个训练 step 的平均 reward |
| resolved 数 | SWE 样本被成功修复的数量 |
| 非零 reward 占比 | 一个 batch 或一个阶段内 reward 大于 0 的样本比例 |
| benchmark 指标变化 | 训练前后在 SWE benchmark 上的 resolved 变化 |

### 5.2 训练效率指标

| 指标 | 说明 |
| --- | --- |
| step time | 单个 GRPO step 总耗时 |
| rollout time | 采样和环境交互耗时 |
| update_actor time | actor 更新耗时 |
| update_weights time | 权重同步到推理侧耗时 |
| episodes/hour | 每小时完成的 episode 数量 |
| GPU 显存峰值 | 训练过程中 GPU 显存峰值 |
| time to first step | 从启动训练到第一个 step 开始的时间 |
| samples/s | 每秒处理训练样本数量 |
| worker utilization | 环境执行 worker 的忙闲比例 |

### 5.3 系统稳定性指标

| 指标 | 说明 |
| --- | --- |
| episode failed ratio | episode 执行失败比例 |
| timeout ratio | 环境执行或模型请求超时比例 |
| recover success | 训练中断后从 checkpoint 恢复是否成功 |
| observable coverage | run、episode、worker、timeline 是否能被前端观测到 |

## 6. 预期展示结果

建议在汇报中展示三类结果：

| 图表 | 展示内容 |
| --- | --- |
| 对比表 | 原生 VeRL 与 UEnv+VeRL 的能力边界对比 |
| 训练过程图 | step time、rollout time、update time 的阶段耗时 |
| 稳定性统计图 | episode 完成、失败、超时和恢复情况 |
| 扩展性曲线 | worker 槽位增加时 episodes/hour 与 step time 的变化 |

## 7. 实验预期结果

| 实验 | 预期结果 | 解释 |
| --- | --- | --- |
| 实验 A：轻量任务正确性与开销对齐 | UEnv+VeRL 的训练效果应与原生 VeRL 基本持平，效率可能略低 | PubMedQA、SciTab、DS-CodeBench 不依赖复杂外部环境，UEnv 的主要作用是验证接入正确性，因此不预期显著超过原生 VeRL |
| 实验 B：SWE 任务公平对比 | UEnv+VeRL 预期优于原生 VeRL baseline | SWE 任务需要环境启动、代码修改、测试执行、trajectory 收集和异常处理，UEnv 已经封装这些能力，原生 VeRL 需要额外实现或适配 AgentLoop |
| 实验 C：环境执行扩展性对比 | 随着 Worker 槽位增加，UEnv+VeRL 的 episode 吞吐应提升，单 step 中等待环境执行的时间应下降 | SWE rollout 的瓶颈主要来自环境执行和多轮 agent 交互，UEnv 可以通过扩展环境执行侧并行度降低训练侧等待 |

实验 A 的理想结果不是显著领先，而是证明 UEnv 接入不会破坏训练效果；实验 B 和实验 C 才是突出 UEnv 价值的重点，尤其是复杂环境任务下的训练可用性、吞吐扩展和问题定位能力。

## 8. 预期结论

UEnv+VeRL 的定位是：VeRL 继续负责强化学习训练算法，UEnv 负责复杂任务的环境执行底座。对于 PubMedQA、SciTab、DS-CodeBench 等轻量任务，原生 VeRL 已经能直接完成训练，UEnv 的主要作用是验证统一 episode 流程的正确性并量化接入开销；对于 SWE、Web、工具调用和多环境任务，UEnv 可以提供统一任务协议、分布式环境执行、失败容错、checkpoint 恢复和多层可观测能力。

因此，当前实验应采用“简单任务证明不破坏训练能力，SWE 任务证明复杂环境训练价值”的组织方式。简单任务主要看训练效果是否对齐，SWE 任务重点展示环境执行开销、episode 吞吐和 worker 扩展能力。
