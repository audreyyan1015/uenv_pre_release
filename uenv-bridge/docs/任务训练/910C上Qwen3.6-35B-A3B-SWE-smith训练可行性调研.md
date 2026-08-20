# 910C 上 Qwen3.6-35B-A3B SWE-smith 训练可行性调研

> 日期：2026-08-12
> 目标：评估在 128 张华为 910C 上进行 `Qwen/Qwen3.6-35B-A3B` 的 VeRL 训练和 rollout 推理是否可行，并估算两天内完成 SWE-smith 全量训练需要的 SWE Worker 槽位。

## 1. 总体结论

在 910C 上部署 `Qwen3.6-35B-A3B` 做 rollout 推理是有可行基础的。vLLM-Ascend 已经提供该模型的部署说明，并且其功能矩阵中标注了 logprobs、张量并行、数据并行、专家并行、sleep mode 等能力。对 UEnv 这类训练链路来说，这些能力都很关键。

但“推理可部署”不等于“训练可稳定跑通”。当前 UEnv 的稳定训练经验来自 A100/CUDA 栈。迁移到 910C 需要重新验证 Ascend 容器、CANN、torch-npu、vLLM-Ascend、VeRL Ascend 训练后端、MoE 模型权重同步和长上下文 rollout。现阶段应判断为：910C 具备落地基础，但需要专项适配和分阶段验收，不能直接认为可稳定全量训练。

两天完成 SWE-smith 全量训练的主要瓶颈也不只在 910C 算力。SWE 任务需要真实代码环境、agent 多轮调用模型、编辑文件、运行测试和返回 reward，因此 CPU 侧 SWE Worker 槽位会成为关键资源。按当前训练目标估算，建议准备 `400-800` 个 Worker 槽位；如果长尾样本较多，需要按 `800-1100` 个槽位规划。

## 2. 当前训练任务规模

SWE-smith 原始数据规模在 5 万条量级。经过当前训练数据过滤后，实际用于训练的有效样本约为：

```text
有效训练样本数 = 41103
每个样本采样 rollout 数 = 4
总 episode 数 = 41103 * 4 = 164412
```

如果要求 48 小时内完成这些 episode，系统需要达到的平均 episode 吞吐为：

```text
48 小时 = 172800 秒
目标吞吐 = 164412 / 172800 = 0.951 episode/s
```

也就是说，整个系统平均每秒要完成接近 1 条 SWE episode。这个吞吐包含模型推理、环境执行、测试判分、结果回传和训练侧等待，不只是 GPU/NPU 生成速度。

## 3. 910C 推理与训练可行性

### 3.1 Rollout 推理

910C 上 rollout 推理的可行性相对较高，原因是 vLLM-Ascend 已经覆盖了 Qwen3.6-35B-A3B 这类 MoE 模型的部署路径。对 SWE 训练来说，需要重点验证以下能力：

| 能力 | 为什么重要 |
|---|---|
| OpenAI 兼容接口 | OpenHands / Worker 侧通过 OpenAI 风格接口调用模型 |
| logprobs 返回 | 训练侧需要恢复模型生成 token trace |
| 长上下文 | SWE prompt、仓库信息和多轮工具调用会产生长上下文 |
| 多卡并行 | 35B-A3B MoE 模型需要多卡并行支撑吞吐 |
| sleep / wake 能力 | VeRL 训练中推理引擎和训练引擎需要分时复用显存 |

因此，第一步应先验证单独的 910C vLLM-Ascend 推理服务：能否稳定完成长上下文 chat completion，能否返回 logprobs，能否支撑 OpenHands 的多轮请求。

### 3.2 VeRL 训练

训练侧风险明显高于推理侧。当前 A100 路径使用的是 CUDA 生态，910C 需要切换到 Ascend 生态。关键差异包括：

| 差异点 | 影响 |
|---|---|
| CUDA 栈变为 CANN / torch-npu 栈 | 训练镜像、算子、通信和调试方式都会变化 |
| 当前 FSDP 经验不能直接复用 | 需要确认 VeRL Ascend 推荐的训练后端和 Qwen MoE 支持情况 |
| vLLM 变为 vLLM-Ascend | 权重同步、logprobs、sleep mode 和长上下文都需要重新测 |
| 多机 128 卡训练 | 需要验证 Ray、HCCL、checkpoint 和网络稳定性 |

因此，910C 训练应按“小模型或小样本 smoke -> Qwen3.6 单步 -> 小规模 SWE-smith -> 多机全量”的顺序推进。

## 4. Worker 槽位估算

### 4.1 估算方法

Worker 槽位可以理解为“同一时间能并行执行多少条 SWE episode”。如果一条 episode 平均耗时越长，需要的槽位越多。

估算公式为：

```text
所需 Worker 槽位 = 目标 episode 吞吐 * 平均单 episode 耗时
```

当前两天目标的 episode 吞吐是 `0.951 episode/s`。因此，只要给定平均单 episode 耗时，就可以换算出槽位需求。

### 4.2 当前日志给出的耗时范围

从已有 A100 训练日志看，SWE/OpenHands rollout 是主要耗时部分。按当前一次提交 8 条 episode、Worker 侧约 4 槽位并发的口径折算，单条 episode 的服务时间大致如下：

当前训练日志的统计结果可以概括为：

| 结果规模 | 8 条 episode 的 rollout P50 | 8 条 episode 的 rollout P75 | 折算单 episode P50 | 折算单 episode P75 |
|---:|---:|---:|---:|---:|
| 400 条 episode | 966 秒 | 1225 秒 | 483 秒 | 612 秒 |

日志中有一个共同结论：训练 step 的主要时间花在 rollout，即 Worker/OpenHands 执行 episode 并反复调用模型；actor update 和权重同步通常是几十秒量级，不是当前两天全量目标的首要瓶颈。

### 4.3 槽位需求

代入两天全量目标后，Worker 槽位需求如下：

| 平均单 episode 耗时 | 理论最少槽位 | 加 30% 余量后 |
|---:|---:|---:|
| 150 秒 | 143 | 186 |
| 260 秒 | 247 | 322 |
| 300 秒 | 286 | 372 |
| 500 秒 | 476 | 619 |
| 600 秒 | 571 | 743 |
| 900 秒 | 856 | 1113 |

据此可得出三个规划区间：

| 规划口径 | 建议 Worker 槽位 | 适用场景 |
|---|---:|---|
| 最小验证 | 200-300 | 只用于确认链路和短样本吞吐 |
| 正式训练起步 | 400-800 | 覆盖 5-10 分钟平均 episode 耗时 |
| 长尾稳态 | 800-1100 | 覆盖慢仓库、长测试列表和多轮 agent 行为 |

如果当前一个 Worker 进程固定提供 4 个槽位，那么 `400-800` 个槽位大约对应 `100-200` 个 Worker 进程。实际部署时不一定需要同等数量物理服务器，但 CPU、内存、容器数、磁盘 I/O 和测试执行能力必须能支撑这些并发。

## 5. 模型服务压力

SWE episode 通常不是一次模型调用就结束。已有日志显示，一条 episode 平均会触发约 20-35 次 OpenAI chat 请求。

按 `164412` 条 episode 估算，全量训练会产生：

```text
模型请求总数 ~= 164412 * (20 到 35)
             ~= 330 万到 575 万次请求
```

两天内完成时，平均请求压力约为：

```text
19 到 33 次 chat 请求 / 秒
```

这些请求往往包含较长 prompt，并且训练需要 logprobs。因此，910C rollout 服务除了要能跑通模型，还需要验证并发请求、长上下文、logprobs 和排队延迟。如果单个推理服务吞吐不足，就需要多个 rollout 副本、量化推理或更强的请求调度。

## 6. 对两天全量训练目标的判断

128 张 910C 能提供更大的训练和推理算力，但两天内完成全量 SWE-smith 训练需要同时满足三类条件：

| 条件 | 判断 |
|---|---|
| 910C 训练栈稳定 | 需要专项验证，当前不能直接认为可用 |
| rollout 推理吞吐足够 | 具备可行基础，但必须压测长上下文和 logprobs |
| SWE Worker 槽位足够 | 至少数百槽位，推荐 400-800 起步 |

如果只具备 128 张 910C，但 Worker 侧仍只有少量槽位，训练会主要卡在环境执行和 agent rollout，而不是训练更新。如果 Worker 槽位足够，但 910C 训练栈或 vLLM-Ascend logprobs 不稳定，也无法进入全量训练。

因此，两天全量训练是一个系统吞吐目标，不是单纯的卡数目标。它需要 NPU 训练、NPU 推理、CPU Worker 环境池、模型请求网关和训练容错一起达标。

## 7. 建议推进路径

建议按以下顺序推进：

| 阶段 | 目标 | 通过标准 |
|---|---|---|
| 910C 推理 smoke | 验证模型服务 | 长上下文 chat、logprobs、多轮请求可用 |
| UEnv episode smoke | 验证 Worker 能消费 910C 模型服务 | 少量 SWE episode 能返回 reward 和 token trace |
| VeRL Ascend smoke | 验证训练栈 | 小任务能完成若干训练 step |
| Qwen3.6 小规模训练 | 验证目标模型 | 能完成 1-10 个 SWE-smith 训练 step |
| Worker 并发压测 | 验证 CPU 环境池 | 达到数百级槽位规划或明确瓶颈 |
| 多机扩大 | 验证 128 卡 | 训练、推理、权重同步、checkpoint 稳定 |
| 全量训练 | 执行两天目标 | 观察 episode P50/P90/P99、reward、失败率和吞吐 |

## 8. 参考资料

| 来源 | 用途 |
|---|---|
| https://docs.vllm.ai/projects/ascend/en/latest/tutorials/models/Qwen3.6-35B-A3B.html | vLLM-Ascend 的 Qwen3.6-35B-A3B 部署说明 |
| https://docs.vllm.ai/projects/ascend/en/latest/user_guide/support_matrix/supported_features.html | vLLM-Ascend 功能支持矩阵 |
| https://docs.vllm.ai/projects/ascend/en/latest/installation.html | vLLM-Ascend 安装与 Ascend 环境说明 |
| https://verl.readthedocs.io/en/latest/ascend_tutorial/index.html | VeRL Ascend 教程入口 |
| https://verl.readthedocs.io/en/latest/ascend_tutorial/model_support/examples/ascend_vllm_best_practices.html | VeRL Ascend vLLM 实践说明 |
