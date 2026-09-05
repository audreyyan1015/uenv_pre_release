# Episode 执行与调度时序动画规划确认稿

> 日期：2026-08-24  
> 目标：在现有 PPT 范式下补充 Server + Worker 的 episode 调度与执行说明，并为后续生成 `.mp4` 动画提供确认基线。  
> 参考文件：`Docs/worker/260824/UEnv调度与执行概念补充_0824.pptx`  
> 输出约束：白色背景，16:9 画幅；后续视频建议使用 1280 x 720，对齐当前 PPT 渲染尺寸。

## 1. 现有 PPT 范式

当前补充稿采用较清晰的技术说明页风格，可继续沿用：

- 顶部深蓝横条，右上角保留 `Shanghai Artificial Intelligence Laboratory`。
- 左上红色章节标题：`工程成果 · UEnv 系统与多任务集成`。
- 主标题使用深蓝大字，副标题用灰蓝色短句解释本页作用。
- 主体图示以浅蓝描边容器、红色强调线和红色重点框为主。
- 说明文字尽量短，概念卡片控制在 2 到 3 行以内。
- 页面底部可放一句红色关键结论，用于收束调度语义。

## 2. 两页内容定位

### 第 1 页：通信与概念辨析

建议标题：

```text
Server + Worker 的通信边界与调度语义
```

本页作用：解释 Server、执行节点、环境运行时、AgentJob/工具/模型之间的边界，避免把“调度”“运行时占用”“Agent 执行命令”混为一谈。

建议主图：

```text
训练/评测侧
  -> UEnv Server
      全局观察、资源聚合、调度决策、lease 派发
  -> 执行节点 / Worker
      本地运行时池管理、环境运行时占用、执行结果上报
  -> Agent / 工具 / 模型能力
      发送命令、调用工具、访问模型或 scorer
  -> UEnv Server
      回传结果、trajectory、reward、episode 状态
```

右侧概念卡片建议保留四个问题：

| 问题 | 建议口径 |
|---|---|
| Server 观察什么？ | 环境资源池、执行节点容量、Worker 本地预热池摘要、心跳和运行状态。 |
| Worker 占用什么？ | 从本地运行时池获取隔离环境运行时；episode 执行期间保持绑定。 |
| AgentJob 发送什么？ | 身份、环境绑定、harness、工具、模型端点与回传约定。 |
| 完成后释放什么？ | 释放 episode lease、运行时绑定、Agent/工具/模型占用，并回传结果。 |

底部关键句：

```text
调度要点：Server 派发的是 Episode 与资源 lease；Worker 占用的是环境运行时；AgentJob 负责在绑定环境中执行行为。
```

### 第 2 页：调度与执行时序动画

建议标题：

```text
Episode 从调度到回收的执行时序
```

本页作用：用动画展示从 Server 准备调度开始，到 Worker 执行、特殊能力绑定、资源释放与生命周期管理的完整路径。主体应是时序示意动画，而不是静态流程表。

## 3. 对用户流程的正确性判断

你的主流程判断是正确的，核心链路可以成立：

1. Server 基于 Worker 上报的资源剩余、预热池状态和环境能力做调度决策。
2. 如果目标执行节点已有可用的所需环境运行时，Server 可直接派发 episode，Worker 占用该运行时执行，完成后释放、按需重置并回到预热池。
3. 如果没有可用预热运行时，但资源预估允许，Server 可选择可承载的执行节点，Worker 通过 Hub 制品拉起新的环境实例，执行完成后进入本地预热池等待复用。
4. 对需要 Agent、工具、模型等特殊能力的 episode，应在调度时同步考虑对应资源，并在执行时形成绑定关系。
5. 各类池化资源需要生命周期管理、心跳和清理机制，以回收未释放资源和僵尸进程。

为保证“遍历每一种情况分支”，建议补充四类边界，否则第二页动画会缺少完整性：

| 需要补充的分支 | 原因 | 动画表达 |
|---|---|---|
| 没有可用预热运行时，且资源预估也不可接受 | 这时不能继续派发，只能排队、等待资源释放或按策略失败。 | Server 进入 `Queue / Backpressure` 状态。 |
| Agent/工具/模型能力不足或不可用 | 特殊能力也是调度约束，不只是执行时附加配置。 | 能力池显示 `available / busy / unavailable` 分支。 |
| 执行失败、超时、取消 | 这些路径也必须释放 lease 和占用，否则会造成资源泄漏。 | 从执行态分出 `fail / timeout / cancel`，统一进入释放与清理。 |
| reset 失败或健康检查失败 | 运行时不一定总能回到预热池，失败时应销毁并补齐预热目标。 | reset 后分为 `ready` 与 `destroy + refill`。 |

## 4. 第二页建议时序模型

建议将动画主体组织为 5 条泳道：

```text
Server 调度器
Worker / 执行节点
环境运行时池
Hub 环境制品
Agent / 工具 / 模型能力池
```

完整时序建议如下。

### 4.1 调度准备

```text
Worker 心跳 / pool snapshot / load report
        -> Server 聚合为全局环境资源池视图
        -> Server 收到 episode / batch
        -> Server 根据 env_type、package、资源余量、预热状态、特殊能力需求做 reserve
```

这一段动画应先显示 Server 汇总信息，再高亮调度决策点。

### 4.2 分支 A：命中已预热运行时

触发条件：

```text
目标执行节点存在 ready 状态的所需隔离环境运行时
```

时序：

```text
Server 派发 episode + lease
        -> Worker acquire ready runtime
        -> runtime 状态 ready -> busy
        -> episode 在绑定运行时内执行行为
        -> 执行完成 / 失败 / 超时 / 取消
        -> Worker 释放 episode 绑定
        -> runtime reset / cleanup
        -> reset 成功：runtime 回到 ready 预热池
        -> reset 失败：runtime 销毁，Worker 按目标补齐预热
        -> Worker 回传 result / trajectory / reward / status
        -> Server 更新 episode 状态和资源视图
```

动画重点：这是低延迟路径，颜色可用蓝色表示稳定复用，红色只标注 busy/占用。

### 4.3 分支 B：预热池缺失或同类运行时已占满，但资源可接受

触发条件：

```text
目标环境运行时不存在 ready slot，或同类型 ready slot 已全部 busy；
同时某个执行节点的 CPU / GPU / 内存 / 并发槽位等资源预估仍可承载。
```

时序：

```text
Server 选择可承载执行节点
        -> 派发 episode + env package / runtime profile
        -> Worker 从 Hub 获取环境制品或使用本地缓存
        -> Worker 拉起新的隔离环境运行时
        -> runtime 状态 warming -> busy
        -> episode 在新运行时内执行
        -> 执行结束后 reset / cleanup
        -> runtime 进入 ready 预热池，成为后续 episode 的可复用资源
        -> Worker 回传结果并上报 pool snapshot
        -> Server 更新全局环境资源池
```

动画重点：这是按需扩容路径，应突出 `Hub artifact -> spawn runtime -> enter pool`。

### 4.4 分支 C：预热池缺失且资源不可接受

触发条件：

```text
没有 ready runtime，且所有执行节点均不满足资源预估、并发上限或健康状态要求。
```

时序：

```text
Server reserve 失败
        -> episode 保持 queued / pending
        -> 等待 Worker 心跳、资源释放或新节点注册
        -> 达到超时 / 取消策略时标记失败或取消
        -> 若后续资源恢复，重新进入调度决策
```

动画重点：这是背压路径，不应画成 Worker 已接收任务。可在 Server 侧显示灰色等待队列。

### 4.5 特殊能力绑定：Agent / 工具 / 模型

触发条件：

```text
episode 需要 agent、工具、模型能力、scorer 或外部 harness。
```

建议语义：

- Agent 能力：episode 与 Agent 池中的进程形成一对一执行关系；Agent 持有环境绑定信息，并向对应执行节点中的隔离环境运行时发送动作命令。
- 工具能力：工具插件或浏览器 / Shell / 外部工具资源需要按工具类型占用能力槽位。
- 模型能力：模型 endpoint 或 scorer 需要按并发、配额或路由策略占用能力。

时序：

```text
Server 判断特殊能力需求
        -> 检查能力池 available slot
        -> 能力可用：随 episode lease 下发绑定约束
        -> Worker 建立 runtime binding
        -> Agent / 工具 / 模型按约束执行调用
        -> 完成后释放能力槽位
```

异常分支：

```text
能力不可用
        -> episode 排队等待
        -> 或按策略失败 / 降级 / 重试
```

动画重点：特殊能力不应画成 Worker 内部环境运行时的一部分，而应作为与运行时绑定的外部执行依赖。

## 5. 生命周期与心跳机制

生命周期管理建议作为第二页动画的收束层，不单独抢占主体空间。

需要表达的对象：

| 对象 | 状态或机制 |
|---|---|
| 执行节点 | 心跳、健康状态、drain/degraded、容量上报。 |
| 环境运行时 | ready、warming、busy、resetting、failed、destroyed。 |
| Agent / 工具 / 模型能力 | available、busy、stale、unavailable。 |
| Episode lease | active、completed、failed、timeout、cancelled、released。 |

统一回收规则：

```text
任何完成、失败、取消、超时路径都必须释放 lease、运行时绑定和特殊能力占用。
心跳超时或资源长期未释放时，由生命周期管理器清理僵尸进程并修正资源视图。
```

## 6. 第二页动画建议分镜

建议后续 `.mp4` 使用 12 到 16 秒，白底 1280 x 720，保留 PPT 顶部标题区。主体区域采用横向泳道时序。

| 时间段 | 画面动作 | 说明重点 |
|---|---|---|
| 0.0s - 1.5s | Worker 心跳、pool snapshot、资源余量汇入 Server。 | Server 不是盲派发，而是基于资源视图调度。 |
| 1.5s - 3.0s | Server 收到 episode，并打开调度决策节点。 | 输入包括 env_type、package、资源需求、特殊能力需求。 |
| 3.0s - 5.5s | 分支 A 高亮：命中 ready runtime，直接派发并置为 busy。 | 预热命中路径。 |
| 5.5s - 8.0s | 分支 A 完成后 reset，成功返回 ready；同时展示 reset 失败则销毁补齐。 | 释放和复用不是简单结束。 |
| 8.0s - 10.5s | 分支 B 高亮：无 ready slot 但资源可承载，从 Hub 拉起新 runtime。 | 按需拉起并在结束后沉淀为预热资源。 |
| 10.5s - 12.0s | 分支 C 高亮：资源不可承载，episode 留在 Server 队列。 | 背压和等待路径。 |
| 12.0s - 14.5s | 特殊能力绑定覆盖到 A/B 路径：Agent 与 runtime 建立一对一执行关系，工具/模型能力被占用。 | 特殊能力是额外调度约束。 |
| 14.5s - 16.0s | 所有路径汇入生命周期管理：心跳、清理、释放、回收。 | 资源可循环利用。 |

## 7. 建议在 PPT 上保留的关键结论

第二页底部可以保留一句红色总结：

```text
调度闭环：Server 负责全局选择与背压，Worker 负责本地运行时占用与回收，Agent/工具/模型能力按需绑定并随 Episode 生命周期释放。
```

## 8. 待确认点

生成动画前建议你确认以下口径：

1. 分支 C 是否按“排队等待”为默认表现，还是要展示为“立即失败 / 调度拒绝”。
2. Agent 池是否只展示 Agent 一类，还是同时展示工具池、模型池、scorer 池。
3. Hub 拉起路径是否需要区分“远端拉取制品”和“命中 Worker 本地缓存”。
4. reset 失败是否需要在动画里显式出现，还是只放在生命周期管理收束层。
5. 后续视频时长是否接受 12 到 16 秒；如果要嵌入 PPT 自动播放，建议控制在 15 秒左右。

