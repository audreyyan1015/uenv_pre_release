# UEnv+VeRL 方案 D1：实施拆解与 Worker 材料清单

## 1. 目的

这份文档不是对比结果，而是把 D1 方案拆成可执行项，方便后续分工、提需求和交付 worker。

当前建议按四类来组织：

1. 编写和复用脚本
2. 修改 native VeRL 的故障处理逻辑
3. 给 worker 提供的材料
4. 补充验收与观测口径

其中第 4 类不是额外负担，而是保证前 3 类真的能跑通、能比较、能复盘。

## 2. 需要做的改动

### 2.1 脚本层

目标是把 D1 的运行方式固定下来，并尽量复用已有脚本。

| 项目 | 要做什么 | 备注 |
| --- | --- | --- |
| 训练启动脚本 | 增加 D1 入口 | 分别支持 UEnv+VeRL 和 native VeRL |
| 数据冻结脚本 | 固定训练集 / holdout / 顺序 | 先冻结再跑，不要边跑边变 |
| catalog 裁剪脚本 | 只保留本轮需要的镜像 | 避免全量 SWE-smith 部署 |
| 故障注入脚本 | 支持 kill slot、断 gateway、重启 | D1 的核心动作之一 |
| 日志收集脚本 | 统一收集 step、episode、gateway、worker 日志 | 方便算有效训练量 |
| 评测脚本 | 固定 holdout 上做前后对比 | 作为主结果口径 |

建议优先复用已有训练脚本，只改参数和入口，不要重写整套执行逻辑。

当前 D1 脚本入口放在：

```text
uenv-bridge/scripts/experiments/d1/run_swe_smith_wallclock_compare.sh
```

该入口只做实验参数收口和 backend 分流，底层仍复用现有的 SWE-smith UEnv 训练预设和 native VeRL 训练预设。

### 2.2 native VeRL 逻辑

native VeRL 侧要补的是“故障时不要把整轮训练拖死”的逻辑。

| 方向 | 说明 |
| --- | --- |
| 单 episode 失败处理 | 不要让单个 episode 失败直接中断整个 step |
| slot 故障处理 | 允许简单 retry、requeue 或重启后继续 |
| gateway 故障处理 | 记录失败样本，但保持训练进程可继续 |
| step 完成条件 | 保证 step 统计口径稳定，不因少量失败而整轮失败 |
| 恢复策略 | 优先简单、粗粒度、可复现的恢复策略 |

native 侧不需要做成和 UEnv 一样复杂，但要保证它在故障下仍然能往前跑，这样 D1 才能比较“有效训练量”而不是比较“谁更容易停掉”。

### 2.3 worker 侧材料

worker 侧最需要的是“本轮实验到底要跑什么、跑到什么程度、出现故障时怎么处理”。

| 材料 | 内容 |
| --- | --- |
| 冻结样本列表 | 训练集 100 条、holdout 100 条的固定列表 |
| catalog 子集 | 本轮实验需要的 instance_id / 镜像 tag 列表 |
| 镜像清单 | 需要同步到各 worker 服务器的镜像 |
| 运行参数 | max_steps、temperature、max_response_length、并发数等 |
| gateway 配置 | 模型 endpoint、token、超时、请求大小上限 |
| harness 说明 | OpenHands / reward / 判分口径 |
| 故障注入安排 | 何时 kill、何时断网、何时重启 |
| 日志清单 | 需要保留哪些日志文件，保留多久 |

worker 侧不需要完整 SWE-smith，只需要本轮 D1 会用到的那部分镜像和实例即可。

## 3. 建议补充的第四类

我建议额外补一类，不然前 3 类做完也不容易判断成败。

### 3.1 验收与观测口径

| 内容 | 作用 |
| --- | --- |
| 主指标定义 | 明确“有效 episode / 非零 reward / 有效 GRPO step”怎么算 |
| holdout 定义 | 明确 100 条 holdout 是哪一批，是否冻结 |
| 成功条件 | 明确 D1 算成功的最低标准 |
| 失败条件 | 明确哪些情况需要停跑重来 |
| 观测日志 | 统一看哪些日志：step、episode、gateway、worker、obs |

没有这一层，后面会出现“脚本跑了，但不知道是不是算完成”的问题。

## 4. D1 的交付顺序建议

| 顺序 | 任务 |
| --- | --- |
| 1 | 先冻结训练集 / holdout / catalog 子集 |
| 2 | 再确认脚本是否能复用，补 D1 入口 |
| 3 | 再改 native VeRL 的故障处理逻辑 |
| 4 | 再把 worker 需要的材料一次性交付 |
| 5 | 最后补验收口径和故障注入计划 |

## 5. 对 worker 的最小交付包

如果要先发一个最小可跑包，建议至少包含：

- 冻结后的样本列表
- catalog 子集
- 镜像 tag 清单
- 运行参数
- gateway / harness 配置
- 故障注入说明
- 日志收集要求

这套包足够 worker 侧先搭起环境，不必等全量 SWE-smith。
