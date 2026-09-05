# UEnv+VeRL 与原生 VeRL 20 步对齐实验结果

## 1. 实验概况与主要结论

本实验对比两种 SWE-smith GRPO 训练链路：

| 方案 | 说明 |
| --- | --- |
| UEnv+VeRL | VeRL 负责 GRPO 训练，SWE/OpenHands episode 交给 UEnv Server/Worker 链路执行并回传结果 |
| 原生 VeRL | VeRL 内部使用自定义 `native_swe_agent` AgentLoop，直接构造 SWE/OpenHands 任务并调用 driver 执行 |

本次对比使用同一模型、同一 SWE-smith 样本前缀、同一 batch/rollout 设置、同一模型 endpoint 和同一 OpenHands 最大执行步数。原生 VeRL 实际运行到 61 step，本报告只截取前 20 step，与 UEnv+VeRL 的 20 step 对齐比较。

主要结论：

| 指标 | UEnv+VeRL | 原生 VeRL | 结论 |
| --- | ---: | ---: | --- |
| 20 step 总耗时 | 10546.4s | 9003.7s | UEnv+VeRL 慢约 17.1% |
| 平均 step time | 527.3s | 450.2s | UEnv+VeRL 慢约 17.1% |
| 中位 step time | 376.3s | 370.3s | 两者接近，UEnv+VeRL 慢约 1.6% |
| 平均 rollout/gen time | 443.4s | 373.9s | 差异主要来自 rollout 阶段 |
| 平均训练吞吐 | 14.61 tokens/s | 16.28 tokens/s | UEnv+VeRL 低约 10.3% |
| episodes/hour | 54.6 | 64.0 | UEnv+VeRL 低约 14.6% |
| episode 完成率 | 160/160 | 160/160 | 两者均无 episode 失败 |
| 平均 reward | 0.2875 | 0.3250 | 本次短跑不用于判断最终训练效果 |

当前数据支持一个阶段性判断：在相同资源、小规模 20 step 对齐实验中，UEnv+VeRL 的整体训练效率低于原生 VeRL baseline，差距主要集中在 rollout/gen 阶段，而不是 actor 更新、ref logprob 或权重同步阶段。

需要注意：中位 step time 很接近，平均值差距主要由少数长尾 step 拉开，尤其是 step 4 和 step 5。因此本实验更适合说明当前 UEnv 链路存在额外调度和长尾放大开销，而不宜表述为所有 step 都稳定慢一个固定比例。

## 2. 实验设置

### 2.1 日志

日志已归档在：

```text
uenv/uenv-bridge/logs/curated/训练对比
```

具体文件：

| 方案 | 训练日志 |
| --- | --- |
| UEnv+VeRL | `uenv/uenv-bridge/logs/curated/训练对比/uenv_verl/verl_swesmith_grpo_uenv_align20_20260824_133530.log` |
| 原生 VeRL | `uenv/uenv-bridge/logs/curated/训练对比/native_verl/verl_native_swesmith_grpo_100step_20260824_001426.log` |


### 2.2 原生 VeRL 自定义 AgentLoop 逻辑

原生 VeRL baseline 新增 `native_swe_agent`，其核心逻辑如下：

| 环节 | 实现方式 |
| --- | --- |
| AgentLoop 注册 | 在 VeRL 内注册 `native_swe_agent` |
| 样本转换 | 从 VeRL batch 中读取 prompt、extra_info、sampling 参数，构造单条 SWE/OpenHands AgentJob |
| 环境执行 | 不经过 UEnv Server/Adapter Core，直接调用已有 OpenHands SWE driver |
| 模型访问 | 使用与 UEnv+VeRL 相同的模型 endpoint：`http://127.0.0.1:18088/v1` |
| 结果回填 | 读取 driver 产物，将 response ids、logprobs、reward、trajectory 等字段转回 VeRL `AgentLoopOutput` |
| 失败处理 | 使用与 UEnv 侧一致的 failed episode policy，将可容忍失败转为 zero reward |

相关实现文件：

| 文件 | 作用 |
| --- | --- |
| `src/uenv/bridge/native_swe_agent_loop.py` | 原生 VeRL SWE AgentLoop 实现 |
| `configs/native-swe-agent-loop.yaml` | 原生 AgentLoop 配置 |
| `scripts/train/launchers/swe/native/swe_smith_native_verl_grpo_train.sh` | 原生 VeRL SWE-smith GRPO 训练入口 |

该 baseline 的设计目标不是复刻 UEnv Server/Worker，而是在 VeRL 训练侧直接编排 SWE/OpenHands episode，用于对比 UEnv 编排层引入后的效率、稳定性和可观测差异。

### 2.3 关键参数

| 参数 | UEnv+VeRL | 原生 VeRL |
| --- | ---: | ---: |
| 模型 | Qwen3.6-35B-A3B | Qwen3.6-35B-A3B |
| 算法 | GRPO | GRPO |
| 对比 step 数 | 20 | 前 20 |
| `train_batch_size` | 2 | 2 |
| `rollout.n` | 4 | 4 |
| 每 step episode 数 | 8 | 8 |
| temperature | 1.0 | 1.0 |
| `max_response_length` | 8192 | 8192 |
| OpenHands 最大步数 | 50 | 50 |
| rollout TP/DP | TP=8, DP=1 | TP=8, DP=1 |
| `max_model_len` | 262144 | 262144 |
| `max_num_batched_tokens` | 65536 | 65536 |
| `enable_chunked_prefill` | true | true |
| `enforce_eager` | false | false |
| `test_freq` | -1 | -1 |
| `save_freq` | 50 | 50 |

## 3. 实验结果

### 3.1 逐 step 耗时

| step | UEnv step(s) | 原生 step(s) | 差值(s) | UEnv rollout(s) | 原生 rollout(s) | UEnv吞吐 | 原生吞吐 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 518.7 | 555.5 | -36.8 | 401.4 | 440.2 | 11.74 | 11.83 |
| 2 | 432.3 | 411.6 | +20.7 | 348.4 | 336.3 | 17.80 | 16.67 |
| 3 | 407.9 | 437.9 | -29.9 | 331.5 | 359.6 | 16.82 | 16.66 |
| 4 | 1863.1 | 1556.3 | +306.8 | 1784.4 | 1475.0 | 3.87 | 5.01 |
| 5 | 1577.6 | 972.5 | +605.1 | 1494.1 | 891.0 | 5.41 | 8.34 |
| 6 | 369.5 | 362.9 | +6.6 | 291.2 | 288.1 | 16.56 | 17.93 |
| 7 | 351.1 | 209.7 | +141.4 | 276.2 | 144.5 | 14.71 | 23.15 |
| 8 | 541.7 | 400.3 | +141.5 | 463.8 | 325.8 | 12.23 | 16.06 |
| 9 | 383.0 | 327.9 | +55.1 | 308.1 | 256.8 | 16.85 | 17.92 |
| 10 | 261.5 | 295.2 | -33.7 | 192.4 | 224.1 | 17.62 | 18.37 |
| 11 | 616.0 | 562.1 | +53.9 | 533.9 | 481.9 | 13.47 | 13.12 |
| 12 | 361.1 | 404.4 | -43.2 | 284.1 | 329.4 | 14.91 | 13.34 |
| 13 | 598.1 | 482.9 | +115.2 | 514.2 | 398.0 | 14.33 | 17.75 |
| 14 | 293.7 | 272.0 | +21.6 | 227.3 | 205.2 | 11.72 | 13.46 |
| 15 | 298.7 | 269.7 | +29.0 | 231.9 | 197.3 | 14.76 | 15.54 |
| 16 | 355.4 | 330.9 | +24.5 | 278.8 | 253.0 | 20.93 | 19.68 |
| 17 | 384.2 | 377.6 | +6.6 | 306.9 | 302.3 | 16.96 | 16.78 |
| 18 | 283.5 | 277.5 | +6.0 | 209.3 | 206.4 | 20.85 | 24.06 |
| 19 | 333.0 | 318.8 | +14.2 | 254.9 | 242.5 | 19.06 | 19.80 |
| 20 | 316.0 | 178.0 | +138.0 | 135.7 | 119.6 | 11.58 | 20.11 |

### 3.2 汇总指标

| 指标 | UEnv+VeRL | 原生 VeRL | 差异 |
| --- | ---: | ---: | ---: |
| 平均 step time | 527.3s | 450.2s | +77.1s |
| 中位 step time | 376.3s | 370.3s | +6.0s |
| 最慢 step | 1863.1s | 1556.3s | +306.8s |
| 平均 rollout/gen time | 443.4s | 373.9s | +69.6s |
| 中位 rollout/gen time | 299.1s | 295.2s | +3.9s |
| 平均 old logprob time | 13.6s | 13.2s | +0.3s |
| 平均 ref time | 6.6s | 6.5s | +0.1s |
| 平均 update_actor time | 41.3s | 40.3s | +1.0s |
| 平均 update_weights time | 16.3s | 16.3s | 约持平 |
| 平均吞吐 | 14.61 tokens/s | 16.28 tokens/s | -1.67 tokens/s |
| episodes/hour | 54.6 | 64.0 | -9.4 |
| response length 均值 | 5898.5 | 5848.2 | +50.3 |
| response clip ratio 均值 | 0.394 | 0.350 | +0.044 |
| actor 显存峰值 | 47.20 GB | 47.41 GB | 约持平 |
| episode 完成率 | 100% | 100% | 持平 |
| episode failed ratio | 0% | 0% | 持平 |
| reward mean | 0.2875 | 0.3250 | 原生略高 |
| 非零 reward 占比 | 28.75% | 32.50% | 原生略高 |

### 3.3 对照实验方案中的指标

| 指标类型 | 本次结果 |
| --- | --- |
| reward mean | UEnv+VeRL 为 0.2875，原生 VeRL 为 0.3250 |
| holdout resolve rate | 本次未做训练前后 holdout eval，不报告该指标 |
| 训练中 resolved / 非零 reward | 以 reward > 0 近似观察，UEnv 为 46/160，原生为 52/160 |
| step time | UEnv 平均 527.3s，原生平均 450.2s |
| rollout time | UEnv 平均 443.4s，原生平均 373.9s |
| update_actor time | UEnv 平均 41.3s，原生平均 40.3s |
| update_weights time | 两者均约 16.3s |
| episodes/hour | UEnv 为 54.6，原生为 64.0 |
| GPU 显存峰值 | 两者约 47GB，差异不明显 |
| episode failed ratio | 两者均为 0% |
| timeout ratio | 日志中未观察到 timeout |
| observable coverage | UEnv 可进入 Obs/Server 观测链路，原生 baseline 主要依赖本地日志 |

## 4. 结果原因分析

### 4.1 慢的主要阶段是 rollout/gen

20 step 对比中，UEnv+VeRL 比原生 VeRL 平均多耗时 77.1s，其中 rollout/gen 平均多耗时 69.6s。old logprob、ref、update_actor 和 update_weights 基本持平。

这说明当前差距主要不是训练更新造成的，也不是 FSDP、ref logprob 或权重同步造成的，而是发生在 episode 采样和环境交互阶段。

### 4.2 response 长度不能解释主要差异

两边 response length 均值非常接近：

| 方案 | response length mean | clip ratio |
| --- | ---: | ---: |
| UEnv+VeRL | 5898.5 | 0.394 |
| 原生 VeRL | 5848.2 | 0.350 |

UEnv 的输出略长，但差异很小，不能解释平均 step time 约 17% 的差距。更合理的解释是 rollout 链路中的调度、等待和长尾 episode 放大了总耗时。

### 4.3 当前资源规模下，UEnv 链路没有单条 episode 的速度优势

在本次实验中，两边使用同一模型 endpoint、同一 OpenHands runtime 和同一 worker 资源。原生 VeRL baseline 在训练侧直接调用 SWE/OpenHands driver；UEnv+VeRL 需要经过 episode 提交、Server 调度、Worker 领取、结果回传、Obs 上报等控制面链路。

因此，在同资源、小规模场景下，UEnv 链路理论上不会比原生直连更短。它的价值主要体现在统一任务协议、可观测、失败隔离、多 worker 扩展和多任务复用，而不是单条 episode 的最低延迟。

### 4.4 平均差距主要由长尾 step 拉开

UEnv+VeRL 在 step 1、3、10、12 比原生 VeRL 更快，但 step 4、5、7、8、20 明显更慢。其中 step 4 和 step 5 合计带来约 912s 的额外差距，是平均值拉开的主要来源。

这说明后续优化重点应放在：

| 方向 | 目的 |
| --- | --- |
| worker 槽位利用率观测 | 判断是否存在空槽等待或长尾阻塞 |
| episode 级耗时拆解 | 区分模型生成、环境执行、测试判分和结果回传耗时 |
| UEnv Server 调度粒度检查 | 确认 slot 释放后是否能立即补发新 episode |
| OpenHands 调用链路对齐 | 确认两边 prompt、工具调用、日志采集和提交路径没有额外差异 |
| 多 worker 扩展实验 | 验证 UEnv 在资源池扩大后能否弥补单链路控制面开销 |

### 4.5 当前结论边界

本报告只覆盖 20 step 训练过程对比，不包含训练前后 holdout eval。因此当前结论应限定为系统效率观察：

1. 在当前 20 step 对齐实验中，UEnv+VeRL 平均 step time 慢于原生 VeRL。
2. 差异主要来自 rollout/gen 阶段。
3. 训练更新相关阶段基本持平。
4. episode 完成率和失败率两边一致，均未出现系统失败。
5. 是否影响最终训练效果，需要后续在同一 holdout 上做训练前后 eval。
