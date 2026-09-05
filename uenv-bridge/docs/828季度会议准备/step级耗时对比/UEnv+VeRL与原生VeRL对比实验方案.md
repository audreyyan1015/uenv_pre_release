# UEnv+VeRL 与原生 VeRL 对比实验方案

## 1. 实验目标

本实验用于说明 UEnv+VeRL 相比原生 VeRL 在复杂环境任务中的系统价值。对比重点不是重新验证 VeRL 的训练算法能力，而是评估在 SWE、Web、工具调用等需要外部环境交互的任务中，UEnv 是否能够提供更稳定、更易扩展、更可观测的训练执行能力。

本方案聚焦两类实验：SWE 多轮环境交互任务的训练闭环对比，以及环境执行侧的横向扩展能力验证。PubMedQA、SciTab、DS-CodeBench 等轻量任务已有 UEnv eval 基线，不在本对比实验范围内。

## 2. 对比对象

| 方案 | 定义 | 主要作用 |
| --- | --- | --- |
| 原生 VeRL | 直接使用 VeRL 训练框架运行 GRPO/PPO 等算法 | 提供强化学习训练、rollout、参数更新与 checkpoint |
| UEnv+VeRL | VeRL 负责训练算法，UEnv 负责环境任务执行、结果回传和运行观测 | 在复杂任务中统一环境交互、任务调度、结果收集和可观测 |

## 3. 核心对比维度

| 维度 | 对比内容 |
| --- | --- |
| 训练效果 | reward、holdout resolve rate、训练前后 benchmark 指标变化 |
| 训练效率 | step 耗时、rollout 耗时、update 耗时、episode 吞吐 |
| 系统稳定性 | episode 失败率、超时率、异常恢复能力、checkpoint 恢复成功率 |
| 任务扩展性 | 新任务接入成本、多任务复用能力、环境执行逻辑复用程度 |
| 可观测性 | 是否能看到 run、episode、worker、timeline 等多层运行状态 |

## 4. 实验设计

### 4.0 Phase 0：Reward 对齐（训练前门禁）

在进入实验 B 的训练对比前，先用固定模型输出（或同一 checkpoint greedy decode）做 eval-only 对齐，排除「判分不一致」导致的虚假差异。

| 任务 | 样本量 | 通过标准 |
| --- | --- | --- |
| SWE-smith smoke | 20 条 | resolved 口径一致（同 harness） |

未通过 Phase 0 时，先修复 Bridge/Worker 判分或 baseline AgentLoop，不进入训练对比。

### 4.1 实验 B：SWE 任务编排层对比

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

#### 4.1.1 对比公平性说明

原生 VeRL **不内置** SWE 执行环境。实验 B 对比的不是「VeRL 官方开箱能力」，而是**在相同 OpenHands + SWE-smith harness 前提下，环境编排与调度层的差异**。

| 层级 | 原生 VeRL 侧实现 | 对比含义 | 是否采用 |
| --- | --- | --- | --- |
| L0 | 无 SWE 能力（仅 rule reward 任务） | 仅说明 UEnv 独有能力边界 | 能力对比表引用，不作效果胜负 |
| L1 | 自研 `SweAgentLoop`（同 OpenHands、同 harness） | **主 baseline**：编排层公平对比 | ✅ 推荐 |
| L2 | 社区 recipe（verl-recipe #91 / RemoteAgentLoop） | 工程成熟度差异参考 | 工期紧可备选 |

原生 VeRL 侧的 SWE 接入可以有两种实现路径：一是我们自行实现 SWE/OpenHands 专用 AgentLoop（L1）；二是参考或复用网上已有的 SWE-agent / RemoteAgentLoop 相关方案（L2）。需要注意的是，这些现有方案更多是 SWE-agent 或远程 agent 框架方向，是否能直接替代 OpenHands 仍需要实际适配和验证。

在部署位置上，原生 VeRL baseline 中的 `SweAgentLoop` 应运行在 VeRL 训练机器上，作为 trainer / Ray / rollout 进程的一部分，直接消费 VeRL 的 batch、response、reward 和 trajectory 等训练侧数据结构。SWE/OpenHands 的实际环境执行可以放在与 UEnv Worker 相同的 CPU 机器上运行，包括 Docker 容器、代码仓库、测试 harness 和 OpenHands runtime。

也就是说，公平对比时应保持如下边界：

| 组成部分 | 原生 VeRL baseline | UEnv+VeRL |
| --- | --- | --- |
| 训练算法与参数更新 | VeRL 训练机器 | VeRL 训练机器 |
| SWE 调度入口 | 训练侧自定义 `SweAgentLoop` | UEnv episode 提交与调度 |
| SWE/OpenHands runtime | 可复用同一批 CPU worker 机器 | UEnv Worker 机器 |
| 对比差异 | VeRL 自行编排环境执行 | UEnv 负责统一编排、调度、观测和容错 |

不建议把 `SweAgentLoop` 本身部署到 UEnv Worker 机器上。否则它会变成一个外部调度服务，实际是在复刻 UEnv 的 server/worker 模式，会削弱“原生 VeRL baseline”的含义，也会让对比边界不清晰。

当前已补充一套原生 VeRL baseline 的自定义 `SweAgentLoop`：

| 文件 | 作用 |
| --- | --- |
| `uenv-bridge/src/uenv/bridge/native_swe_agent_loop.py` | 注册 `native_swe_agent`，在 VeRL 训练进程内构造单样本 AgentJob，直接调用 OpenHands SWE driver，并把 driver 产物转回 `AgentLoopOutput` |
| `uenv-bridge/configs/native-swe-agent-loop.yaml` | VeRL AgentLoop 配置入口，可通过环境变量指定 runtime gateway、LLM config、catalog、driver 输出目录等 |
| `uenv-bridge/scripts/train/launchers/swe/native/swe_smith_native_verl_grpo_train.sh` | SWE-smith 原生 VeRL baseline 入口；复用当前 SWE-smith 训练超参，只把编排层切到 `native_swe_agent` |

这套 baseline 不调用 UEnv Adapter Core / Server。它仍可复用同一批 SWE/OpenHands runtime 机器和 runtime gateway，以保证环境、镜像、判分 harness 与 UEnv+VeRL 侧一致；对比差异集中在“训练侧自行编排 SWE episode”与“交给 UEnv 统一编排”。

| 来源 | 可参考内容 | 说明 |
| --- | --- | --- |
| VeRL AgentLoop 文档 | https://verl.readthedocs.io/en/latest/advance/agent_loop.html | 说明 VeRL 如何注册和运行自定义 AgentLoop |
| verl-recipe | https://github.com/verl-project/verl-recipe | VeRL 社区 recipe 仓库，可查看是否已有 SWE-agent 相关实现 |
| SWE-Agent recipe PR | https://github.com/verl-project/verl-recipe/pull/91 | 社区 SWE-agent recipe 方向，可作为原生 VeRL baseline 的参考 |
| RemoteAgentLoop issue | https://github.com/verl-project/verl/issues/5737 | VeRL 社区讨论将外部 agent 作为远程 AgentLoop 接入 |
| 阿里 ACK 示例 | https://help.aliyun.com/zh/ack/training-agentic-reinforcement-learning-on-ack-using-the-verl-framework | 展示 VeRL、RemoteAgentLoop 和 SWE-agent/Harbor 结合的工程方案 |

#### 4.1.2 必须锁定的控制变量

- 同一 `instance_id` 子集（建议先 100 条，再扩至 500 条）
- 同一 OpenHands / SWE-smith harness 版本
- 同一 `max_steps`、temperature、max_tokens
- 同一 resolved 判分口径（官方 pytest harness）
- 同一模型 checkpoint 起点

建议先使用 100 到 500 条 SWE-smith 子集完成可控对比，再扩展到更大规模训练。该实验重点观察：

| 指标 | 含义 | 优先级 |
| --- | --- | --- |
| holdout resolve rate | 固定 holdout 子集上的 resolved 比例（主效果指标） | P0 |
| episode 成功完成率 | 环境执行链路是否稳定（SYSTEM 失败率） | P0 |
| rollout 阶段耗时 | 外部环境交互是否成为瓶颈 | P0 |
| 失败样本处理 | 单条失败是否会导致整轮训练中断 | P0 |
| 训练中 resolved 数 | 训练过程中产生有效修复的累计数量 | P1 |
| 非零 reward 占比 | GRPO 是否获得有效学习信号（仅用于 B，不跨任务横向比） | P1 |
| 单 step 平均耗时 | 训练循环整体效率 | P1 |

评测节奏：训练前、训练中每 N steps、训练后各在**同一 holdout**（建议 smith 100 条）上 eval 一次。

### 4.2 实验 C：环境执行扩展性对比

目标是评估 UEnv 在 SWE 任务下的环境侧横向扩展能力，以及任务扩展性的工程叙事支撑。

#### 4.2.1 吞吐扩展（主实验）

| 设置 | 说明 |
| --- | --- |
| 固定训练资源 | 8 卡训练侧保持不变 |
| 变化环境资源 | Worker 槽位从 1、2、4、8 逐步增加 |
| 任务 | SWE-smith 固定子集 |
| 重复次数 | 每个槽位配置至少 2 次 |
| 观察指标 | episodes/hour、step time、worker 利用率、失败率、扩展效率、rollout 等待占比 |

衍生指标：

| 指标 | 定义 |
| --- | --- |
| 扩展效率 | `throughput(N) / throughput(1) / N`，理想接近 1 |
| rollout 等待占比 | `rollout_time / step_time`，Worker 增加后应下降 |

该实验用于回答：当训练侧等待环境执行时，增加环境执行并行度是否能提升整体吞吐。

#### 4.2.2 任务扩展性（汇报补充，不单独跑大规模实验）

828 季度会采用轻量方式体现「可扩展性」：在对比表中补充 UEnv 相对自研 AgentLoop 的**新环境接入成本**（人天）、Hub/Rubric 复用、评测与训练是否同源等指标；不要求在 C 中额外切换 `qa` → `code` → `swe` 做大规模混跑。

## 5. 指标口径

### 5.1 训练效果指标

| 指标 | 说明 | 适用实验 |
| --- | --- | --- |
| reward mean / std | 每个训练 step 的平均 reward 及方差 | B |
| holdout resolve rate | 固定 holdout 子集上的 resolved 比例 | B |
| 训练中 resolved 数 | 训练过程中累计 resolved episodes | B |
| 非零 reward 占比 | 一个 batch 或阶段内 reward > 0 的样本比例 | B |
| benchmark 指标变化 | 训练前后在同一 holdout 上的主指标变化 | B |

### 5.2 训练效率指标

| 指标 | 说明 | 适用实验 |
| --- | --- | --- |
| step time | 单个 GRPO step 总耗时 | B、C |
| rollout time | 采样和环境交互耗时 | B、C |
| update_actor time | actor 更新耗时 | B |
| update_weights time | 权重同步到推理侧耗时 | B |
| episodes/hour | 每小时完成的 episode 数量 | B、C |
| GPU 显存峰值 | 训练过程中 GPU 显存峰值 | B |
| time to first step | 从启动训练到第一个 step 开始的时间 | B |
| worker utilization | `busy_slots / total_slots` 的时间加权均值 | B、C |
| 扩展效率 | `throughput(N) / throughput(1) / N` | C |
| rollout 等待占比 | `rollout_time / step_time` | B、C |

### 5.3 系统稳定性指标

| 指标 | 说明 |
| --- | --- |
| episode failed ratio | episode 执行失败比例（区分 SYSTEM / BUSINESS） |
| timeout ratio | 环境执行或模型请求超时比例 |
| recover success | Worker kill 或 Server SIGKILL 后，从 checkpoint 恢复并成功继续训练 |
| observable coverage | checklist：run / episode / worker / timeline 四层是否可查 |

## 6. 预期展示结果

建议在汇报中展示两类结果：

| 图表 | 展示内容 |
| --- | --- |
| 对比表 | 原生 VeRL 与 UEnv+VeRL 的能力边界对比（含 L0/L1 baseline 说明） |
| 训练过程图 | step time、rollout time、update time 的阶段耗时 |
| 稳定性统计图 | episode 完成、失败、超时和恢复情况 |
| 扩展性曲线 | worker 槽位增加时 episodes/hour、扩展效率与 rollout 等待占比的变化 |
| 任务扩展性表 | 新环境接入成本、Hub/Rubric 复用、评测训练同源（定性 + 人天） |

## 7. 实验预期结果

| 实验 | 预期结果 | 解释 |
| --- | --- | --- |
| 实验 B：SWE 任务编排层对比 | 训练效果（holdout resolve rate）应接近；UEnv 优势在稳定性、容错、可观测与工程成本 | 同栈 L1 baseline 下 resolved 未必显著领先；UEnv 价值在 episode 成功率、失败隔离、问题定位和接入成本 |
| 实验 C：环境执行扩展性对比 | Worker 槽位增加时 episode 吞吐应提升，rollout 等待占比应下降 | SWE rollout 瓶颈在环境执行和多轮 agent 交互，UEnv 可通过扩展环境侧并行度降低训练侧等待 |

实验 B 和实验 C 是突出 UEnv 价值的重点，尤其是复杂环境任务下的训练可用性、吞吐扩展和问题定位能力。

## 8. 预期结论

UEnv+VeRL 的定位是：VeRL 继续负责强化学习训练算法，UEnv 负责复杂任务的环境执行底座。对于 SWE、Web、工具调用和多环境任务，UEnv 可以提供统一任务协议、分布式环境执行、失败容错、checkpoint 恢复和多层可观测能力。

因此，当前实验应聚焦 SWE 任务，重点展示环境执行稳定性、episode 吞吐、worker 扩展能力和工程化优势；不宜将实验 B 表述为「UEnv 在 resolved 上碾压原生 VeRL」，而应强调**同执行栈下的编排与系统能力对比**。

## 9. 执行检查清单

| 阶段 | 动作 | 产出 |
| --- | --- | --- |
| Phase 0 | SWE-smith smoke reward 对齐 | 对齐报告 |
| 实验 B | smith 100–500 × L1 baseline × 2 臂 | resolve 曲线 + 稳定性统计 |
| 实验 C | Worker 1/2/4/8 × 2 重复 | 扩展性曲线 |
| 汇报 | 能力边界表 + 训练过程图 + 扩展曲线 | 828 季度会材料 |
