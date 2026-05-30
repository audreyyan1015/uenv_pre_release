# uenv\-design\-prd\-v7\.2

# UEnv — 分布式训练环境框架 总体解决方案

*版本: v7\.2 \| 日期: 2026\-05\-27 \| 状态: 总体解决方案*

\&gt;

*本文档是 UEnv 分布式训练环境框架的总体解决方案，在 v7\.1 设计评审基础上，围绕六大核心维度组织全量内容：场景需求、概要设计、环境构造（数据合成 \+ 封装）、交互机制（流式交互 \+ 步调感知）、可观测性（负载画像 \+ 基础数据采集）、任务调度。同时覆盖容错可靠性、使用场景和测试策略。文档面向架构师、技术负责人和核心开发团队，提供从需求到实现的完整视图。*

## 目录

一、场景需求

1\.1 项目背景与产品定位

1\.2 目标用户画像

1\.3 用户故事

1\.4 功能需求矩阵

1\.5 非功能需求

1\.6 约束与假设

二、概要设计

2\.1 总体架构

2\.2 设计原则：四层解耦与控制面分离

2\.3 系统分层

2\.4 核心数据流

2\.5 组件职责矩阵

三、环境构造

3\.1 环境数据合成

3\.1\.1 环境执行与数据生成模型

3\.1\.2 轨迹数据结构化

3\.1\.3 奖励信号的合成与组合

3\.1\.4 多源信号融合策略

3\.1\.5 数据完整性保障

3\.1\.6 数据产出质量评估

3\.2 环境封装

3\.2\.1 环境抽象接口

3\.2\.2 MCP 工具扩展机制

3\.2\.3 环境容器化与安全隔离

3\.2\.4 四级注册与分发链路

3\.2\.5 内置环境矩阵与生态覆盖

四、交互机制

4\.1 通信架构选型

4\.2 流式协议设计概览

4\.3 Training Adapter 层

4\.4 步调感知与自适应流控

4\.5 多模式通信策略

4\.6 Episode 完整交互时序

4\.7 协议版本与兼容策略

五、可观测性

5\.1 负载画像模型

5\.2 心跳驱动的实时感知

5\.3 指标采集规范

5\.4 可观测性体系

5\.5 调度决策的数据基础

六、任务调度

6\.1 调度器框架

6\.2 调度流程

6\.3 调度策略

6\.4 环境预热池

6\.5 扩缩容策略

6\.6 Server 水平扩展与分片

6\.7 分阶段状态管理

6\.8 Worker 运行时管理

七、容错与可靠性

7\.1 四层容错金字塔

7\.2 重试策略

7\.3 数据完整性

7\.4 Server 高可用

7\.5 网络分区与脑裂防护

7\.6 Worker 断连与请求持久化

7\.7 GC 暂停与心跳隔离

八、使用场景

8\.1 数学训练环境（ROLL \+ MathEnv）

8\.2 代码训练环境（VeRL \+ CodeEnv）

8\.3 Agent 训练环境

8\.4 自定义环境开发与发布

8\.5 大规模并行训练

九、测试策略与质量保证

9\.1 测试分层策略

9\.2 单元测试与覆盖率目标

9\.3 集成测试方案

9\.4 端到端测试

9\.5 混沌测试计划

9\.6 CI/CD 流水线

附录 A: 术语定义

附录 B: Reward 兼容性声明

附录 C: 架构可行性评审摘要

## 一、场景需求

*本章定义 UEnv 解决的核心问题域、目标用户、典型场景和量化需求指标，是全文的\&\#34;为什么做\&\#34;部分。核心判断：UEnv 定位为 Agentic RL 环境框架，而非通用 RLVR 框架。RLVR 场景（prompt→answer→verify）通常通过训练框架内置 reward model 直接处理，UEnv 的核心价值在 Agent 训练的多步交互场景。本章从行业痛点出发，明确产品定位和适用边界，通过五类用户故事和四层优先级需求矩阵，奠定后续所有设计决策的出发点。*

### 1\.1 项目背景与产品定位

**背景痛点：** 在 LLM 后训练（Post\-Training）生态中，RLVR、在线强化学习、Agent 训练等场景需要与外部环境交互来收集训练信号。当前存在四大痛点：

|**痛点**|**表现**|**根因**|
|---|---|---|
|\*\*重复造轮子\*\*|每套训练框架（ROLL、VeRL、TRL）自建环境接口|缺乏统一环境协议|
|\*\*环境不可迁移\*\*|切换训练框架意味着重写全部环境代码|环境与框架强耦合|
|\*\*缺乏统一管理\*\*|大规模分布式训练缺少环境注册、调度和生命周期管理|无统一调度层|
|\*\*部署碎片化\*\*|环境从开发到生产的路径不清晰|容器化和隔离方案不统一|



**产品定位：** UEnv 是一个**训练框架无关的分布式环境执行框架**，为 LLM 后训练提供统一的 Environment 接口。通过 gRPC 双向流实现 Episode 级粒度的高效通信，让同一套环境实现可在三大主流训练框架间无缝切换。

**核心价值主张：**

|**价值**|**说明**|
|---|---|
|训练框架无关|通过 Adapter 适配 3 大训练框架（ROLL/VeRL/TRL）|
|环境可定制|MCP 工具 \+ 可组合 Reward（类 nn\.Module）\+ 可注入执行逻辑|
|大规模分布式|UEnv Server 调度（注册表 \+ 负载均衡）\+ Worker 本地实例池 \+ 自动扩缩容|
|开发到生产|Process → Podman 渐进部署路径|
|环境即服务|注册到 UEnvHub，拉取即用|



**适用场景边界：** UEnv 的核心定位是 **Agentic RL 的环境框架**（多步交互，5\-80 步 Episode）。RLVR 场景（单步验证）通常通过训练框架内置 reward model 直接处理，不需要 UEnv 介入。代码库证据：ROLL 的 RLVR Pipeline 使用 \`AsyncDynamicSamplingScheduler\` 而非 \`TrajEnvManager\`。

### 1\.2 目标用户画像

|**角色**|**核心需求**|**UEnv 提供的价值**|
|---|---|---|
|\*\*训练框架开发者\*\*|集成多环境支持|通过 Training Adapter 接入，一次集成获得所有环境|
|\*\*环境开发者\*\*|一次开发，多框架可用|实现 Environment ABC \+ 注册，所有训练框架自动可用|
|\*\*RL 训练工程师\*\*|配置即用，无需关心部署|环境注册到 UEnvHub，拉取即用，调度自动|
|\*\*平台运维\*\*|统一管理和监控训练环境|UEnv Server 提供环境注册/调度/监控一体化管理|



### 1\.3 用户故事

**故事 2 — 训练工程师使用 VeRL 训练代码生成：** 训练工程师使用 VeRL 训练代码生成模型。VeRLAdapter 将 DataProto 批量样本转换为并行 EpisodeRequest，通过 gRPC 双向流提交到 UEnv Server，8 个 GPU Worker 并行执行代码沙箱环境，流式回收结果。

**故事 3 — 环境开发者创建新环境：** 环境开发者需要创建一个安全分析环境。实现 MCPEnvironment 子类，注册 scan\_code 和 query\_cve 两个 MCP 工具，实现 \_step\_impl\(\) 方法。通过 @register\_env\(\&\#34;security\&\#34;\) 注册后打包发布到 UEnvHub，所有训练框架立即可用。

**故事 5 — 平台运维部署大规模训练集群：** 平台运维在 Podman 集群上部署 UEnv。配置 PodmanBackend 管理环境容器，预热池根据历史预测提前创建环境实例。Prometheus \+ Grafana 监控集群状态，Podman rootless \+ seccomp 确保安全隔离。

### 1\.4 功能需求矩阵

UEnv 的功能需求按四个阶段递进交付，从最小可验证的端到端链路到完整的高级环境生态。

**架构验证阶段**

第一阶段的目标是验证架构可行性——用最简实现跑通 ROLL \+ Sokoban 端到端训练循环。仅交付七项核心能力，不涉及认证、预热池、WAL 持久化和多 Worker 调度。

|**编号**|**功能**|**描述**|**验收标准**|
|---|---|---|---|
|F\-01|Protobuf Schema|定义 EpisodeRequest / EpisodeResult / StreamReport 消息|proto 编译通过，可生成多语言代码|
|F\-02|UEnv Server 核心服务|环境注册表 \+ 调度器 \+ gRPC Server|能接收 EpisodeRequest，调度到 Worker，返回结果|
|F\-03|Worker 基础框架|gRPC Server \+ Episode 执行循环 \+ 心跳|能接收 DispatchEpisode，执行环境，返回结果|
|F\-04|Training Adapter 基类|convert\_request / convert\_response / execute\_episode|抽象接口可被子类实现|
|F\-05|GEMAdapter|将 ROLL 的 GEM make/step/reset/close 自动转换为 EpisodeRequest|ROLL 训练任务端到端跑通|
|F\-06|MathEnv|数学问题求解 \+ 可验证奖励|能执行数学 Episode 并返回正确 reward|
|F\-07|ProcessBackend|本地进程后端，零额外依赖|环境实例在本地进程中运行|



**核心扩展阶段**

在架构验证通过后，将 UEnv 从单框架推进到双框架可用状态，并补齐生产化的基础能力。扩展 Adapter 接入 VeRL，引入 MCP 工具系统和代码沙箱环境，以 Podman 容器后端替代 Process 实现生产级安全隔离。

|**编号**|**功能**|**描述**|
|---|---|---|
|F\-08|VeRLAdapter|DataProto 批量样本 → EpisodeRequest 转换|
|F\-09|—|—|
|F\-10|MCPEnvironment|中间层自动路由 list\_tools / call\_tool / 普通 step|
|F\-11|Reward 系统|类 nn\.Module 可组合奖励：Sequential / Gate / WeightedSum 等全部容器和信号源|
|F\-12|CodeEnv|多语言代码沙箱执行 \+ 单元测试|
|F\-13|Worker 预热池|根据历史请求模式提前创建实例，降低冷启动延迟|
|F\-14|StreamReport 流式上报|Episode 执行中实时上报进度和奖励信号|
|F\-15|PodmanBackend|Rootless 容器隔离，Seccomp \+ AppArmor 安全策略|



**生产完备阶段**

补齐容错能力和监控体系，向生产级系统演进。引入 Agent 交互型环境和 UEnvHub 轻量注册中心。

|**编号**|**功能**|**描述**|
|---|---|---|
|F\-16|—|—|
|F\-17|TRLAdapter|MCP tool call context → EpisodeRequest 转换|
|F\-18|AgentEnv|多轮对话 \+ 工具调用 \+ 多智能体协作|
|F\-19|Worker 级容错|Worker OOM 自动重启，Server 重新分配未完成 Episode|
|F\-20|Episode 级容错|失败后最多 3 次重试，支持 partial trajectory 降级返回|
|F\-21|UEnvHub Git\+YAML|轻量环境元数据仓库，版本化 YAML manifest 分发|
|F\-22|Prometheus 监控|Episode 成功率、延迟分布、吞吐量、Worker 负载等核心指标|



**生态扩展阶段**

面向长尾场景和生态建设。新增 Web 浏览器自动化和游戏仿真两类高级环境，将 UEnvHub 从 Git\+YAML 演进为 HTTP API 注册中心服务，支持环境检查点恢复和跨步骤延迟轨迹奖励等高级能力。

|**编号**|**功能**|**描述**|
|---|---|---|
|F\-23|WebEnv|基于 Playwright 的浏览器自动化 Agent 训练环境|
|F\-24|GameEnv|棋盘游戏与物理仿真环境封装|
|F\-25|UEnvHub HTTP API|完整环境注册中心（发布/搜索/版本管理）|
|F\-26|环境检查点/恢复|环境状态快照保存与回滚|
|F\-27|TrajectoryReward|跨步骤延迟轨迹奖励信号源|



### 1\.5 非功能需求

|**维度**|**指标**|**目标**|**验证方式**|
|---|---|---|---|
|\*\*延迟\*\*|框架开销（不含 Episode 执行）|尽可能低|端到端延迟基准测试|
|\*\*吞吐\*\*|并发 Episode|1000\+ eps/s（数学场景，4 Worker × 8 并发）|压力测试|
|\*\*可靠性\*\*|Episode 成功率|\&gt; 99%（含重试，3 次重试 \+ Worker 故障转移）|长时间运行统计|
|\*\*可用性\*\*|UEnv Server 可用性|99\.9%（主从切换可选）|故障注入测试|
|\*\*数据完整性\*\*|轨迹数据校验|SHA\-256 校验和匹配率 100%|传输前后校验|
|\*\*安全\*\*|容器隔离|Podman rootless \+ seccomp \+ AppArmor|安全审计|
|\*\*序列化\*\*|Protobuf vs JSON|显著优于 JSON|序列化基准测试|
|\*\*负载感知\*\*|Worker 负载画像更新|心跳间隔 ≤ 5s，负载数据新鲜度 ≤ 10s|心跳延迟监控|



### 1\.6 约束与假设

**技术约束：**

\- 不使用消息中间件（Redis/Kafka/RabbitMQ），直接 gRPC 通信

\- Episode 级粒度调度（非单步），降低通信开销

\- gRPC 双向流 \+ Protobuf 序列化，不使用 JSON

\- Reward 系统为类 nn\.Module 可组合设计，支持容器层、信号源层、信用分配层和归一化层

**前提假设：**

\- 训练框架已支持某种协议（GEM / gRPC / OpenAI / MCP），Adapter 可做转换

\- 模型推理端点（vLLM / SGLang）已部署并可访问

\- Worker 运行环境已安装必要依赖（Python 3\.10\+ / Podman）

**范围排除：**

\- 不包含训练引擎本身（PPO / GRPO / DAPO 等算法由训练框架负责）

\- 不包含推理引擎（vLLM / SGLang 等由模型服务层负责）

\- 不包含模型管理和权重分发

## 二、概要设计

*本章从全局视角描述 UEnv 的总体架构、设计原则和核心数据流，是理解后续各维度详细设计的基础。架构遵循\&\#34;四层解耦 \+ 控制数据面分离\&\#34;原则，将训练框架接入、调度服务、环境执行和环境注册四个关注点彻底分离。通过 ASCII 架构图、分层职责表和组件职责矩阵三种视图，从不同粒度展示系统的全貌。核心数据流图则描绘了一个 Episode 从提交到完成的完整路径，涉及 Adapter、Server、Worker 和 UEnvHub 五个子系统的协作。*

### 2\.1 总体架构

![Image](https://internal-api-drive-stream.feishu.cn/space/api/box/stream/download/authcode/?code=MmY3MDAwN2Q3MDZjNGMwZjhkZDEyNmM4M2Q5NGJiZGNfMjlmMDcyNzg3NDA0MTJhZjdmODc4ZmZiMzAyOTA2YWNfSUQ6NzY0NDcyMzAxMjYxNjQ0MDc2OF8xNzc5OTU5MDUzOjE3ODAwNDU0NTNfVjM)

**架构核心要点：**

\- **控制平面（Server）**：仅负责 Episode 的调度编排、Worker 注册和负载感知。不参与 Episode 执行期间的 step 级数据流。实例生命周期管理下沉到 Worker 本地。

\- **数据平面（Worker）**：Episode 执行期间直接与推理服务通信，不经过 Server。

\- **UEnvHub**：离线持久化目录，最终一致非运行时依赖。环境开发者 push 发布，Server 定期 sync，Worker 按需 pull，Adapter 读取 config\_schema。

### 2\.2 设计原则：四层解耦与控制面分离

UEnv 的架构遵循**四层解耦 \+ 控制数据面分离**原则：

**第一层解耦：训练框架 ↔ 环境（通过 Adapter \+ gRPC）**

训练框架不直接调用环境，而是通过 Training Adapter 将框架原生协议（GEM/DataProto/OpenAI/MCP）转换为统一的 EpisodeRequest，经 gRPC 发送到 UEnv Server。新增训练框架只需开发一个 Adapter，无需修改环境代码。

**第二层解耦：调度 ↔ 执行（通过 UEnv Server ↔ Worker）**

UEnv Server 负责调度决策（哪个 Worker 有空、资源是否匹配），Worker 负责实际执行 Episode。调度逻辑不关心环境内部实现，Worker 不关心任务从哪个训练框架来。调度和执行可以独立扩缩容。

**第三层解耦：环境定义 ↔ 环境分发（通过 UEnvHub）**

环境开发者将环境元数据发布到 UEnvHub（持久化目录），Worker 启动时拉取环境定义。环境发布与部署异步，版本管理与运行时实例管理分离。UEnvHub 是静态目录，不参与运行时调度。

**第四层原则：控制平面与数据平面分离**

这一分离确保 Server 不会成为 step 级数据瓶颈，同时保持对 Episode 生命周期的完整编排能力。

### 2\.3 系统分层

|**层级**|**名称**|**核心职责**|**关键组件**|
|---|---|---|---|
|Layer 4|训练框架接入层|协议转换、错误映射、重试|5 个 Training Adapter|
|Layer 3|调度服务层|请求路由、负载均衡、Worker 健康管理|UEnv Server（注册表 \+ 调度器）|
|Layer 2|环境执行层|Episode 循环、后端管理、实例池、心跳与进度上报|Worker Pool \+ 环境实例|
|Layer 1|环境注册层|元数据存储、版本管理、镜像分发、配置 Schema|UEnvHub|



**横切关注点：**

\- 可观测：Worker 心跳 \+ Episode metrics \+ Prometheus \+ OTLP 分布式追踪

\- 安全：Podman rootless \+ seccomp \+ AppArmor \+ 网络隔离

### 2\.4 核心数据流

![Image](https://internal-api-drive-stream.feishu.cn/space/api/box/stream/download/authcode/?code=MTJhYmFjZWYwNjA4NDA5NDJmNDM4ODAyYzQ2MmM5ZjBfMjRhMTJhZjBjMmZlN2NiMTIyNzNlZDM2Y2NkMDVkNDlfSUQ6NzY0NDcyMzAxMTc2MDc1MzU5Nl8xNzc5OTU5MDUzOjE3ODAwNDU0NTNfVjM)

一个 Episode 从提交到完成的完整数据流涉及 5 个子系统的协作：

**数据流步骤说明：**

7\. Worker ↔ UEnvHub：启动时拉取环境定义

### 2\.5 组件职责矩阵

|**组件**|**环境构造**|**交互机制**|**可观测性**|**调度**|**容错**|
|---|---|---|---|---|---|
|Training Adapter||✅ 协议转换\+步调|||✅|
|UEnv Server||✅ 流控|✅ 全局负载|✅|✅|
|Worker|✅ 数据合成\+封装|✅ 流式上报|✅ 本地负载||✅|
|UEnvHub|✅ 注册分发|||||
|Environment ABC|✅ 奖励\+接口|||||



## 三、环境构造

*本章描述 UEnv 环境的两大核心能力：环境数据合成（环境如何产出结构化训练数据）和环境封装（如何构建可被 UEnv 调度和分发的环境）。两部分共同构成从\&\#34;原始交互\&\#34;到\&\#34;可调度服务\&\#34;的完整链路——前者关注 Episode 执行过程中轨迹记录、奖励计算和多源信号融合的数据产出机制，后者关注环境接口抽象、MCP 工具扩展、容器化隔离和四级注册分发的工程实现方法。*

### 3\.1 环境数据合成

*环境数据合成是 UEnv 的核心价值输出环节。本节描述环境如何从原始交互中合成结构化训练数据——包括轨迹记录、奖励信号和多源信号融合——为训练框架提供高质量的训练信号。*

#### 3\.1\.1 环境执行与数据生成模型

![Image](https://internal-api-drive-stream.feishu.cn/space/api/box/stream/download/authcode/?code=YjkxMjlhMDk1MzcxNWJiMzgzNDI4YjMyYjZlYTRjMzVfYWMxZTdmMzVkMmY0N2Q4MmRiNWUxMTMwNzJhNzdhZTJfSUQ6NzY0NDcyMzAxMjkxNDE1NDQ2Nl8xNzc5OTU5MDUzOjE3ODAwNDU0NTNfVjM)

**Episode 执行引擎**是数据合成的运行时核心。一次完整的 Episode 执行生成一条完整的训练轨迹（Trajectory），分以下阶段产出数据：

**Episode 执行引擎的核心步骤：**

1\. **获取环境实例**：从预热池获取或新建环境实例

2\. **初始化 Reward 链**：根据 RewardConfig 构建可组合的 Reward 树

3\. **环境 Reset**：调用 env\.reset\(seed\) 初始化环境状态

5\. **延迟奖励计算**：跨步骤的 TrajectoryReward（如折扣累计）

6\. **汇总返回**：构建 EpisodeSummary 和 EpisodeResult

7\. **实例归还**：环境实例归还预热池复用

**四种执行模式：**

|**模式**|**说明**|**适用场景**|
|---|---|---|
|单轮|一步完成，action = 完整输出|数学、问答、代码生成|
|多轮|Agent 多次 step，每步调用模型|工具使用、多步推理|
|模型回调|Worker 内部循环调用模型|Agent 训练、ReAct 循环|
|可定制|用户自定义执行循环|复杂环境逻辑|



#### 3\.1\.2 轨迹数据结构化

**Trajectory** 是 Episode 执行产出的核心数据结构，包含完整的 step\-by\-step 记录。其结构包含：

\- **元信息**：episode\_id、env\_type、worker\_id、时间戳

\- **观测序列**：initial\_observation（reset 产出）、final\_observation（终止时）、steps（每步的 StepRecord 列表）

\- **每步记录**：step\_index、observation、action、action\_logprob、step\_reward、cumulative\_reward、reward\_components（各信号源分值）、terminated/truncated 标志、性能指标（step\_latency\_ms、req/resp\_tokens）、info 扩展

\- **汇总**：total\_steps、total\_tokens、total\_model\_calls、总延迟/模型延迟/环境延迟

\- **校验**：trajectory\_checksum（SHA\-256）、integrity\_verified 标志

轨迹数据通过 Protobuf 序列化，支持 gzip 自动压缩（\&gt;1MB 触发），超过 64MB 自动分片传输。

#### 3\.1\.3 奖励信号的合成与组合

![Image](https://internal-api-drive-stream.feishu.cn/space/api/box/stream/download/authcode/?code=N2U3N2Q4ZTViZmU1NjIyYjJjMjgzOWZkMDRiMDM5NTVfY2Q5YjgzNDdmYWRhZDNlNTJmMDBhZTAzZjg5MDM5N2FfSUQ6NzY0NDcyMzAxMDM4OTI2NTM1N18xNzc5OTU5MDUyOjE3ODAwNDU0NTJfVjM)

UEnv 采用类 \`nn\.Module\` 的可组合奖励系统，支持信号源层、容器层、信用分配层和归一化层四层架构：

**组合示例（代码生成场景）：**

为代码生成任务构建多级奖励链：第一步，Gate \+ RuleReward 确保代码必须编译通过；第二步，Gate \+ RuleReward 确保至少通过一半测试；第三步，WeightedSum 加权评估测试通过率、神经网络 RM 和 LLM 评判代码质量。任何一步失败（零值），Sequential 立即短路返回零，避免浪费后续昂贵的 LLM 评判计算。

#### 3\.1\.4 多源信号融合策略

当使用 \`WeightedSum\` 等组合容器时，多个信号源并行计算后融合：

**融合配置示例：** 混合奖励模式下，正确性（规则）\+ 风格（LLM 评判）\+ 效率（规则），信用分配采用指数折扣，归一化采用组内 z\-score。

#### 3\.1\.5 数据完整性保障

为确保训练数据的可靠性，UEnv 在数据传输的每个环节实施完整性校验：

**完整性保障链路：**

|**层级**|**保障机制**|**检测内容**|
|---|---|---|
|传输层|gRPC 内置校验（HTTP/2 帧级）|网络传输损坏|
|消息层|Protobuf 反序列化校验|消息格式损坏|
|业务层|SHA\-256 轨迹校验和|业务数据损坏、篡改|
|日志层|WAL CRC32 校验|磁盘写入损坏|



#### 3\.1\.6 数据产出质量评估

**Episode 结果状态分类：**

|**状态**|**含义**|**数据可用性**|**训练侧处理**|
|---|---|---|---|
|\`COMPLETED\`|正常完成|完整 trajectory|直接用于训练|
|\`PARTIAL\`|超时但部分完成|partial\_trajectory|训练框架决定是否使用|
|\`FAILED\`|执行失败|partial\_trajectory \+ error|标记为坏样本，跳过|
|\`TIMEOUT\`|整体超时|partial\_trajectory|标记为坏样本，跳过|



**数据质量核心指标：**

|**指标**|**计算方式**|**告警阈值**|
|---|---|---|
|Episode 完成率|completed / total|\&lt; 95% → P2 告警|
|轨迹完整性|校验通过 / total|\&lt; 99\.9% → P1 告警|
|奖励非零率|非零奖励 Episode / total|视环境而定|
|数据产出延迟|完成时间 − 提交时间|P99 \&gt; 300s → P2 告警|

### 3\.2 环境封装

*本节描述如何从零构建一个可被 UEnv 调度的环境，涵盖接口抽象、工具扩展、安全隔离和分发链路。核心设计思想是\&\#34;环境不关心谁在调用它\&\#34;——通过最小接口 \+ 自动注册 \+ 渐进隔离，让环境实现与训练框架完全解耦。*

#### 3\.2\.1 环境抽象接口

Environment 是所有 UEnv 环境的顶层抽象基类，参考 OpenEnv 设计，采用泛型三参数 \`Environment\&lt;ActT, ObsT, StateT\&gt;\` 约束类型。

**最小接口契约（Gymnasium 兼容）：**

|**方法**|**功能**|**返回值**|**约束**|
|---|---|---|---|
|\`reset\(seed?\)\`|重置到初始状态|\`ObsT\`|必须支持可复现 seed|
|\`step\(action\)\`|执行一步动作|\`\(ObsT, float, bool, bool, dict\)\`|Gymnasium 五元组兼容|
|\`close\(\)\`|清理资源|\`None\`|释放文件句柄、网络连接等|
|\`state\` \(property\)|获取当前状态|\`StateT\`|用于检查点/恢复|



**设计原则：**

|**原则**|**说明**|
|---|---|
|训练框架无关|环境不关心谁在调用它，只接受 action 返回 observation|
|Gymnasium 兼容|step 返回 \(obs, reward, terminated, truncated, info\)|
|泛型约束|ActT / ObsT / StateT 三参数约束类型安全|
|最小接口|四个方法是最小契约，子类可扩展但不缩减|



#### 3\.2\.2 MCP 工具扩展机制

![Image](https://internal-api-drive-stream.feishu.cn/space/api/box/stream/download/authcode/?code=YTNjNmEyNTc0OWExMTdhZGJmNzU1ZTk2MjgzMDYwMjBfNDAxMmQ4ZWI5MTA2NWQ3ZTNjYjI0ZGU4MzFlNDgwNGJfSUQ6NzY0NDcyMjk5OTc4NTk4MjkyNV8xNzc5OTU5MDUyOjE3ODAwNDU0NTJfVjM)

**MCPEnvironment** 是 Environment 的中间层子类，为 Agent 训练场景提供**自动工具路由**能力。子类只需实现 \`\_step\_impl\(\)\` 方法定义环境特定逻辑，工具调用通过 MCP 协议自动分发。

**自动路由机制：** 当 \`step\(action\)\` 收到 action 时，根据 action 类型自动判断路由：

|**action 类型**|**路由目标**|**处理逻辑**|
|---|---|---|
|\`\&\#34;list\_tools\&\#34;\`|内部处理|遍历已注册工具，返回名称、描述和参数 Schema 列表|
|\`\&\#34;call\_tool\&\#34;\`|注册的工具函数|查找工具 → 调用 execute\(arguments\) → 返回结果|
|其他|\`\_step\_impl\(\)\`|委托给子类实现的环境特定 step 逻辑|



**双 API 边界设计：**

|**API**|**调用方**|**用途**|
|---|---|---|
|Simulation API|Worker 内部训练循环|env\.reset/step/close，训练框架集成|
|Production API|Agent（推理侧）|MCP list\_tools/call\_tool，Agent 自主决策|



#### 3\.2\.3 环境容器化与安全隔离

Worker 将环境实例运行在后端管理器中，支持两种运行时模式：

**后端对比：**

|**后端**|**启动延迟**|**隔离级别**|**资源占用**|**适用场景**|**安全特性**|
|---|---|---|---|---|---|
|\*\*Process\*\*|极低|进程级|最低|开发调试、CI 测试|无隔离|
|\*\*Podman\*\*|\~2s|容器级\(rootless\)|低|生产默认|Rootless \+ Seccomp \+ AppArmor|



**Podman 安全策略：**

|**安全机制**|**说明**|
|---|---|
|Podman Rootless|无需 root 权限，用户命名空间隔离|
|Seccomp|自定义系统调用白名单，限制危险 syscall|
|AppArmor|容器级强制访问控制|
|资源限制|CPU / 内存 / GPU 约束，防止资源逃逸|
|网络隔离|容器独立网络命名空间，可选完全断网|



**渐进部署路径：**

#### 3\.2\.4 四级注册与分发链路

![Image](https://internal-api-drive-stream.feishu.cn/space/api/box/stream/download/authcode/?code=MGQ2MTVkZGNkZGEzMzAwNzFkY2NlMmRlZDk1NTM3YTNfODhlZTNiNTI4YjE0OTZjY2I2YmE3Mzk3NmZhMDdmZjBfSUQ6NzY0NDcyMjk5ODIxMzIzMzYxOF8xNzc5OTU5MDUzOjE3ODAwNDU0NTNfVjM)

环境从\&\#34;代码实现\&\#34;到\&\#34;可调度服务\&\#34;经历四级注册：

**注册职责划分：**

|**级别**|**执行者**|**职责**|**存储**|
|---|---|---|---|
|UEnvHub 发布|环境开发者|发布环境元数据（名称/版本/描述/资源需求/镜像）|UEnvHub（Git/HTTP）|
|开发者注册|环境开发者|\`@register\_env\` 声明类型和资源需求|Python 包内|
|Worker 本地注册|Worker 启动|加载环境包，实例化到本地注册表|Worker 内存|
|Server 全局注册|UEnv Server|Worker 上报 \+ UEnvHub 拉取元数据|Server 内存|



**UEnvHub 与 Server 注册表的分工：**

|**维度**|**UEnvHub（持久化目录）**|**Server 注册表（运行时）**|
|---|---|---|
|存储内容|环境类型、版本、镜像地址、资源需求、配置 Schema|在线 Worker 列表、Worker 持有环境|
|生命周期|永久（发布后一直存在）|临时（Worker 下线后清除）|
|类比|Docker Hub|Docker daemon 的 container list|
|一致性|最终一致|强一致（基于心跳实时更新）|



**UEnvHub 演进路径：**

\- 初始：Git 仓库 \+ YAML manifest（2\-3 个内置环境）

\- 扩展：HTTP API 服务 \+ 对象存储（多团队协作）

\- 完善：Web UI \+ 评分 \+ 自动测试（开源生态）

**环境 Manifest 核心字段：** env\_type（唯一标识）、version（semver）、description、image（容器镜像）、supported\_backends、resource\_requirements（CPU/内存/GPU）、config\_schema（JSON Schema）、tags。

**UEnvHub 在调度中的作用：**

1\. **资源匹配**：根据 resource\_requirements 确保 Worker 有足够资源

2\. **后端过滤**：根据 supported\_backends 过滤不支持的 Worker

3\. **配置验证**：根据 config\_schema 验证 EpisodeRequest 配置合法性

4\. **版本管理**：同一 env\_type 支持多版本

#### 3\.2\.5 内置环境矩阵与生态覆盖

采用**\&\#34;3 核心自研\&\#34;**策略。数学、代码、Agent 三类环境覆盖主流后训练场景，游戏、Web、工具等长尾场景通过扩展适配器覆盖。

|**环境**|**类型**|**核心功能**|**适用场景**|**阶段**|
|---|---|---|---|---|
|\*\*MathEnv\*\*|验证型|数学问题求解、Lean 定理证明、可验证奖励|数学 RLVR、形式化验证|架构验证|
|\*\*CodeEnv\*\*|执行型|多语言代码执行、沙箱安全、单元测试|代码生成、程序修复|架构验证|
|\*\*AgentEnv\*\*|交互型|多轮对话、工具调用、多智能体协作|Agent 训练、工具学习|核心扩展|
|\*\*GameEnv\*\*|仿真型|棋盘游戏、物理仿真|策略 RL、多智能体|生态扩展|
|\*\*WebEnv\*\*|自动化型|浏览器控制（Playwright）|Web Agent|生态扩展|
|\*\*ToolEnv\*\*|工具型|Shell 执行、API 调用、数据库|工具学习|生态扩展|



## 四、交互机制

*本章描述各组件的通信架构、流式协议和步调感知机制。核心设计思想是\&\#34;gRPC 双向流 \+ 自适应流控\&\#34;——通过流式上报让训练侧实时感知 Episode 进度，通过背压机制让系统在负载波动时自动调整节奏。本章从通信架构选型出发，对比 gRPC、Ray Actor 和 HTTP REST 三种方案的优劣，论证 gRPC 作为统一通信层的合理性。在此基础上，详细定义三组 gRPC Service 的职责边界、四种通信模式（同步/流式/批量/异步）的适用场景、以及覆盖 Worker/Server/Adapter 三层的步调感知与流控机制。*

### 4\.1 通信架构选型

**选型结论：gRPC 双向流 \+ 有状态 UEnv Server**

|**维度**|**gRPC 双向流**|**Ray Actor**|**HTTP REST**|
|---|---|---|---|
|延迟|⭐⭐⭐ 极低|⭐⭐ 较低|⭐ 一般|
|流式支持|✅ 原生双向流|⚠️ Ray Serve|❌ SSE/WebSocket|
|类型安全|✅ \.proto 强类型|⚠️ Pickle|❌ JSON|
|语言支持|✅ 全语言|❌ Python only|✅ 全语言|
|序列化性能|⭐⭐⭐ protobuf|⭐⭐ Pickle|⭐ JSON|



**核心理由：**

1\. 行业事实标准 — VeRL 等主流框架均使用直接 RPC，无消息中间件

2\. 流式支持 — 原生双向流支持长 Episode 的中间状态上报

3\. 性能 — protobuf 序列化性能显著优于 JSON

4\. 统一调度 — Server 内置注册表 \+ 调度器

**与消息中间件方案对比：** UEnv 使用 gRPC 双向流替代了常见的消息中间件方案。gRPC 长连接复用消除了每次请求建连的开销，Protobuf 序列化替代 JSON 提升了类型安全和性能，双向流原生支持了消息中间件不具备的中间状态上报能力。同时去除了消息中间件集群的额外运维成本。

### 4\.2 流式协议设计概览

UEnv 定义了三组 gRPC Service，各有明确的职责边界：

**UEnvService — 面向训练侧和 Worker：**

|**方法**|**模式**|**说明**|
|---|---|---|
|\`SubmitEpisode\`|同步 unary|提交单个 Episode，阻塞等待结果|
|\`SubmitEpisodeStream\`|双向流|批量提交 \+ 流式返回|
|\`SubmitBatch\`|同步 unary|批量提交，整批完成后统一返回（GRPO/PPO）|
|\`SubmitEpisodeAsync\`|异步 unary|立即返回 ACK，不阻塞训练循环|
|\`DispatchEpisode\`|服务端流|Server → Worker，返回 StreamReport|
|\`HealthCheck\`|unary|健康检查|



**WorkerDirectService — Worker 主动上报：**

|**方法**|**说明**|
|---|---|
|\`ReportResult\`|Worker → Server 返回 EpisodeResult|



**DispatcherService — Worker 注册与心跳：**

|**方法**|**说明**|
|---|---|
|\`RegisterWorker\`|Worker 启动时注册，上报 worker\_id、supported\_envs、capacity|
|\`WorkerHeartbeat\`|双向流心跳，Worker 上报负载，Server 下发控制指令|



**StreamReport 核心字段：** request\_id、worker\_id、report\_type（PROGRESS/STEP\_COMPLETE/REWARD\_SIGNAL/LOG/PACING）、current\_step、total\_steps、step\_latency\_ms、model\_latency\_ms、cumulative\_reward、reward\_components、estimated\_remaining\_seconds、worker\_active\_episodes、worker\_capacity。

**错误处理策略：**

|**错误类型**|**处理方式**|**是否重试**|
|---|---|---|
|\`env\_error\`|返回 partial\_trajectory \+ error|Adapter 决定|
|\`model\_error\`|返回 error，由 Adapter 重试推理|✅ max\_retries 次|
|\`timeout\`|返回 partial\_trajectory|❌|
|\`reward\_error\`|返回 partial\_trajectory（缺 reward）|❌|
|\`internal\`|Worker 重启，返回 error|Server 重新分配|
|\`config\_error\`|立即返回 error|❌|



### 4\.3 Training Adapter 层

![Image](https://internal-api-drive-stream.feishu.cn/space/api/box/stream/download/authcode/?code=ZjU4OWMxNjdkMTAxOTA1YWNkMzZjNDRjODA3ZmEzODBfNDI0ODNjYWQ1NjNlZDAyYjI4YjZhY2RmMzA1NGYxM2FfSUQ6NzY0NDcyMzAwMDA0NjA5NTMyMV8xNzc5OTU5MDUzOjE3ODAwNDU0NTNfVjM)

**五个框架 Adapter 对比：**

|**Adapter**|**框架**|**输入协议**|**集成难度**|
|---|---|---|---|
|GEMAdapter|ROLL|GEM make/step/reset/close → EpisodeRequest|低（GEM 协议清晰）|
|VeRLAdapter|VeRL|DataProto batch → EpisodeRequest|中（DataProto 结构复杂）|
|TRLAdapter|TRL|MCP tool call context → EpisodeRequest|低|



**部署模式：** 嵌入式（推荐，低延迟独立扩缩）、内嵌式（简单部署单进程）、Sidecar（网络优化运维复杂）。

### 4\.4 步调感知与自适应流控

![Image](https://internal-api-drive-stream.feishu.cn/space/api/box/stream/download/authcode/?code=MDhjMjI2NWFmY2ZkZDY0MjMzYTk5OTg0OTRkNTBiNGVfNzcwNWUxYzcyOWUyYzM3MWI1OGEwNTQ3Y2Y0YTYwZWZfSUQ6NzY0NDcyMjk5ODE1ODc0MDQ1Ml8xNzc5OTU5MDUyOjE3ODAwNDU0NTJfVjM)

步调感知（Pacing Awareness）是 v7\.2 的核心增强，确保 UEnv 在高并发场景下不会压垮训练侧或推理服务。

**步调感知模型：** \`步调感知 = f\(训练消费速率, 环境产出速率, 推理服务负载, 队列深度\)\`

UEnv 在三个层面实现步调感知：

**L1: Worker 端 — Episode 步调自适应：** 推理步调检查 pending\_requests，超过高水位时主动减速；环境步调检查本地负载，超过容量 80% 时通知 Server 降低分发速率；通过 StreamReport（type=PACING）上报 estimated\_remaining 进度。

**L2: Server 端 — 流控与背压：** 有界队列（maxsize=1000），队列满时发送 PAUSE，消费后低于低水位发送 RESUME；根据 pending\_episodes/worker\_count 比值决定 THROTTLE/PAUSE/RESUME。

**L3: Adapter 端 — 训练步调同步：** 信号量控制 max\_concurrent，基于 Episode 延迟反馈动态调整并发度。

**自适应流控参数：**

|**参数**|**默认值**|**说明**|
|---|---|---|
|\`queue\_high\_watermark\`|800 / 1000|触发 PAUSE|
|\`queue\_low\_watermark\`|200 / 1000|触发 RESUME|
|\`avg\_queue\_wait\_threshold\`|100|调度延迟阈值|
|\`model\_pending\_high\_watermark\`|100|推理服务待处理上限|



### 4\.5 多模式通信策略

UEnv 支持四种通信模式，适配不同训练场景：

|**模式**|**RPC 方法**|**适用场景**|**阶段**|
|---|---|---|---|
|\*\*同步\*\*|\`SubmitEpisode\`|调试、单 Episode|架构验证|
|\*\*流式\*\*|\`SubmitEpisodeStream\`|批量训练|架构验证|
|\*\*批量\*\*|\`SubmitBatch\`|GRPO/PPO 整批完成|核心扩展|
|\*\*异步\*\*|\`SubmitEpisodeAsync\` \+ \`WatchEpisodes\`|高并发 \&gt;500 eps/s|生产完备|



**同步 vs 异步效率：** 同步模式下每线程仅少量推理请求在途，GPU batch 较小。异步模式下所有协程推理请求汇聚，GPU continuous batching 最大化批量，显著提升推理 GPU 利用率。

### 4\.6 Episode 完整交互时序

![Image](https://internal-api-drive-stream.feishu.cn/space/api/box/stream/download/authcode/?code=OGZhZWM0MTBjMmQwMTgyMmM4NTgyODA0M2UyOTU4YmZfMWM2Y2EyOWY0MTE2ZGI5ZTZhYjViM2E2NzhiZWI0M2ZfSUQ6NzY0NDcyMjk5OTc4NjEzMDM3MF8xNzc5OTU5MDUzOjE3ODAwNDU0NTNfVjM)

### 4\.7 协议版本与兼容策略

**消息大小与传输：** max\_message\_size 64MB，\&gt;1MB 自动 gzip 压缩，\&gt;64MB 分片传输，支持截断策略（保留最近 N 步）。

## 五、可观测性

*本章描述 UEnv 如何实时感知系统负载状态、采集运行数据、建立从指标到追踪的完整可观测性体系。负载画像是 UEnv 从\&\#34;被动调度\&\#34;升级为\&\#34;主动感知\&\#34;的关键能力——通过心跳驱动的实时数据采集，Server 掌握每个 Worker 的六维负载画像（资源/Episode/延迟/实例池/可靠性/环境亲和），并以此为基础驱动调度决策。本章还涵盖 Prometheus 指标采集规范、OpenTelemetry 分布式追踪全链路 Span 树、以及 Grafana/AlertManager/Jaeger 三层可观测架构。*

### 5\.1 负载画像模型

负载画像是对 Worker、Server 和整体系统运行状态的持续量化描述，是 UEnv 从\&\#34;被动调度\&\#34;升级为\&\#34;主动感知\&\#34;的关键能力。Worker 端的负载画像覆盖六大维度：资源负载（CPU/内存/GPU/磁盘 IO）、Episode 负载（活跃数/可用槽位/队列利用率）、延迟画像（均值/P50/P95/P99 分位数的 Episode 耗时和模型推理延迟）、实例池画像（预热/活跃/空闲实例数和命中率）、可靠性画像（完成/失败数和失败率）、环境亲和画像（活跃和预热中的环境类型分布）。Server 端则聚合所有 Worker 的负载数据，形成全局视图，包含集群规模、排队深度、调度延迟和按环境类型的统计。

**Worker 负载画像六大维度：**

|**维度**|**核心指标**|
|---|---|
|\*\*资源负载\*\*|cpu\_utilization、memory\_utilization、gpu\_utilization、disk\_io|
|\*\*Episode 负载\*\*|active\_episodes、available\_slots、queue\_utilization|
|\*\*延迟画像\*\*|avg/p50/p95/p99 episode\_duration\_ms、avg\_model\_latency\_ms、avg\_env\_step\_latency\_ms|
|\*\*实例池画像\*\*|warm\_instances（按 env\_type）、active\_instances、pool\_hit\_rate|
|\*\*可靠性画像\*\*|total\_completed、total\_failed、failure\_rate、last\_error|
|\*\*环境亲和画像\*\*|active\_env\_types、warm\_env\_types、env\_type\_distribution|



**Server 全局负载画像：** total\_workers、active/partitioned/draining\_workers、pending\_episodes、in\_flight\_episodes、scheduler\_queue\_utilization、per\_env\_type\_stats、pacing\_state（NORMAL/SLOW\_DOWN/PAUSE）。

### 5\.2 心跳驱动的实时感知

Worker 心跳是负载画像的主要数据来源，采用自适应间隔。

**心跳内容：** worker\_id、active\_episodes、available\_slots、avg\_episode\_duration\_ms、avg\_model\_latency\_ms、warm\_instances（含按类型分布）、pacing\_state、current\_eps\_rate、heartbeat\_seq。

**自适应心跳间隔：** \`heartbeat\_interval = base\_interval × min\(√\(N / 10\), 3\)\`，base\_interval = 5s，N = 注册 Worker 数量。Worker 规模越大，心跳间隔越长，避免 Server 端心跳处理压力随 Worker 数量线性增长。

**心跳隔离：** Worker 心跳在独立线程发送，内容极简，Server 端超时 = 3 × 心跳间隔（容忍 2 次连续丢失）。

### 5\.3 指标采集规范

UEnv 采用三层指标采集架构：L3 业务指标（Episode 成功率/延迟/奖励分布）、L2 调度指标（队列深度/调度延迟/Worker 分布）、L1 系统指标（CPU/内存/GC/gRPC 连接数）。

**核心指标：** uenv\_episode\_duration\_seconds、uenv\_episode\_total、uenv\_worker\_active\_episodes、uenv\_worker\_load\_ratio、uenv\_scheduler\_queue\_size、uenv\_worker\_heartbeat\_last\_seen、uenv\_instance\_pool\_hit\_rate、uenv\_pacing\_state、uenv\_data\_integrity\_checks\_total。

### 5\.4 可观测性体系

![Image](https://internal-api-drive-stream.feishu.cn/space/api/box/stream/download/authcode/?code=ZDIwMjk4YjM3ZDA1NDdiNjFjM2ZkNGExMGM4M2M5NTZfMWYxZTI1MjQ5ZjNmNGQzMTU3NTRiNzIyZjM4Y2I3ZmNfSUQ6NzY0NDcyMzAwMDM3MzIxODI0NF8xNzc5OTU5MDUzOjE3ODAwNDU0NTNfVjM)

**三层架构：** 查询分析层（Grafana / AlertManager / Jaeger）、采集存储层（Prometheus / OTLP / Fluentd）、产出层（/metrics 端点、JSON 日志、OTLP Traces、健康检查）。

**结构化日志：** JSON 格式，含 timestamp/level/logger/message/trace\_id/context。ERROR/WARN/INFO 三级覆盖全部关键事件。

**告警规则：** P1：Worker 心跳丢失 \&gt;15s、Server 不可达 \&gt;30s；P2：Episode 失败率 \&gt;5%、调度队列积压 \&gt;100、Server 内存 \&gt;80%。

### 5\.5 调度决策的数据基础

负载画像数据直接驱动调度决策，调度器综合 Worker 的负载状态、环境亲和度和历史延迟，结合全局步调状态进行决策。**调度决策 = f\(负载画像, 亲和画像, 延迟画像, 步调状态\)**。

## 六、任务调度

*本章描述 Episode 从提交到完成的全链路调度机制。核心设计思想是\&\#34;Server 即控制平面\&\#34;——仅负责调度编排（哪个 Worker 执行、何时执行、如何分配），不参与 Episode 执行期间的 step 级数据流。调度器采用四层架构（请求入口→请求队列→决策引擎→执行分发），综合 Worker 负载、环境亲和度和历史延迟选择最优 Worker，并辅以步调感知修正全局分发速率。本章还涵盖环境预热池的降低冷启动策略、基于 KEDA/HPA 的自动扩缩容、以及按环境类型分片的 Server 水平扩展方案。*

### 6\.1 调度器框架

![Image](https://internal-api-drive-stream.feishu.cn/space/api/box/stream/download/authcode/?code=YjVhNDg3N2Q0M2U5NzlhOGNiY2Q1ZjNjMmVlN2JkNTZfMDk0ZjY5OTY4NzI2ZGE2NzFiZDg3NmIwNzJlMDI2Y2VfSUQ6NzY0NDcyMzAwMTg2MjE2MzQxMl8xNzc5OTU5MDUyOjE3ODAwNDU0NTJfVjM)

UEnv 调度器采用四层架构设计，将请求接收、排队、决策和分发四个关注点分离。请求入口层接收来自 gRPC Server、管理 API 和内部超时/重试触发器的 EpisodeRequest；请求队列按优先级排序暂存待调度请求；调度决策引擎是核心，整合环境注册表（资源需求）、Worker 注册表（负载与健康状态）、候选过滤（健康\+资源\+类型匹配）和步调修正（全局流控状态），最终产生分发决策；执行分发层通过 gRPC DispatchEpisode 将请求发送到最优 Worker，失败时自动降级到次优候选。

### 6\.2 调度流程

### 6\.3 调度策略

UEnv 支持多种调度策略以适应不同场景需求，策略可组合使用。默认采用「最少负载」策略确保 Episode 均匀分布：

|**策略**|**说明**|**适用场景**|
|---|---|---|
|最少负载|选 in\-flight 最少的 Worker|默认策略|
|类型亲和|优先已持同类型环境的 Worker|环境预热|
|资源感知|根据资源需求匹配节点|GPU 环境|
|优先级|高优先级抢占式|多租户|
|步调感知|动态调整全局分发速率|高并发|



### 6\.4 环境预热池

预热池是降低环境冷启动延迟的关键机制。调度器根据历史请求模式提前在 Worker 上创建环境实例并保持 Warm 状态，Episode 到达时可直接从池中获取，跳过环境创建和初始化的等待时间。预热池采用 LRU 策略保留最近活跃的环境类型，Episode 完成后实例归还池中复用而非销毁，从而减少因反复创建/销毁带来的性能抖动。核心参数：warmup\_pool\_size=5、max\_idle\_time=300s、max\_episode\_count=1000。

### 6\.5 扩缩容策略

UEnv 各组件的扩缩容采用不同的触发机制以适应各自的工作负载特征。Worker Pod 基于 KEDA 自定义指标自动扩缩（活跃 Episode 数与容量比值超过 80% 触发扩容），Server 基于 HPA 按 CPU 利用率和请求排队延迟自动扩容，GPU Worker 因资源稀缺采用手动扩容配合 Prometheus 告警，预热池则基于历史请求模式在训练 Job 启动前主动预热。缩容统一采用空闲超时策略，Worker 空闲超过 idle\_timeout 后自动销毁释放资源。

### 6\.6 Server 水平扩展与分片

### 6\.7 分阶段状态管理

UEnv Server 的状态管理随规模渐进演进，避免过早引入复杂性：

|**Worker 规模**|**状态方案**|**RTO**|
|---|---|---|
|\&lt;10|进程内 \+ WAL|\&lt;5s|
|10\-50|WAL \+ Active\-Passive|\&lt;30s|
|50\-200|状态外部化 etcd/Redis|\&lt;10s|



### 6\.8 Worker 运行时管理

![Image](https://internal-api-drive-stream.feishu.cn/space/api/box/stream/download/authcode/?code=ZjQ5MDEzODIwNGIxOTExNmEyNWZiZTAyMDc1MzQ0NWVfMTMzYTQ3YTEwYTcwOTFlOWIxZTYxOGY4MWEzNDE2MmZfSUQ6NzY0NDcyMjk5MDE1MTY1MDI0Ml8xNzc5OTU5MDUzOjE3ODAwNDU0NTNfVjM)

## 七、容错与可靠性

*本章描述各层如何处理故障。核心设计思想是\&\#34;错误应尽可能在离发生点最近的位置被处理\&\#34;——形成 L1 基础设施→L2 Step 级→L3 Episode 级→L4 训练级的四层金字塔，确保大部分错误在底层就被消化。本章覆盖重试策略（区分瞬态/持久故障）、SHA\-256 数据完整性校验链路、Server 主从切换与 WAL 崩溃恢复、网络分区下的 Worker 本地缓存和重连握手、以及基于 Epoch Fencing Token 的脑裂防护机制。*

### 7\.1 四层容错金字塔

![Image](https://internal-api-drive-stream.feishu.cn/space/api/box/stream/download/authcode/?code=NjM3MTI1YmZkYmVjMGQzNWUwZThmZGRjNDdiMTNmZTFfOGE4ZDFkN2I3NzQwM2I2MjEyNDQwN2FmODQ4YWZkMjlfSUQ6NzY0NDcyMjk4NjMyMjM0OTAyNV8xNzc5OTU5MDUyOjE3ODAwNDU0NTJfVjM)

### 7\.2 重试策略

UEnv 在各类故障场景下采用不同的重试策略。可恢复的瞬态故障（网络抖动、模型推理超时）自动重试，不可恢复的故障（配置错误、环境异常）立即返回错误：

|**场景**|**重试次数**|**退避**|
|---|---|---|
|Episode 失败|3 次（重新分配）|立即|
|模型推理失败|max\_retries 次|\+2s 退避|
|Server 不可用|自动|指数退避|
|超时/环境错误/配置错误|无重试|—|

### 7\.3 数据完整性

### 7\.4 Server 高可用

### 7\.5 网络分区与脑裂防护

### 7\.6 Worker 断连与请求持久化

![Image](https://internal-api-drive-stream.feishu.cn/space/api/box/stream/download/authcode/?code=OWRkZmZmYzFhMzI4MDE0YzViNDNkNTI5MWE1NmE3MWVfNGIzZDI0NDY4OTNiNWQwYWM2MDU3NjViZjMxMGIxMTlfSUQ6NzY0NDcyMjk4Nzk0NTQ3OTEyNF8xNzc5OTU5MDUzOjE3ODAwNDU0NTNfVjM)

### 7\.7 GC 暂停与心跳隔离

Python 等 VM 环境的 GC 暂停可能导致心跳延迟误判 Worker 故障。UEnv 通过线程隔离策略解决此问题：Worker 心跳在独立线程发送且不持有 GIL，与 Episode 执行线程完全隔离，心跳内容极简无序列化开销。Server 端超时阈值设为 3 × 心跳间隔，可容忍 2 次连续心跳丢失，SUSPECT 状态额外等待 1 个周期后才标记 PARTITIONED，为 GC 暂停等短暂延迟提供缓冲窗口。

## 八、使用场景

![Image](https://internal-api-drive-stream.feishu.cn/space/api/box/stream/download/authcode/?code=YTE0ZmVkZDE0ZTlkNWM4MjU2YzEzMjYwOTVlOTgwYjVfMDQzZTg5ZjI0OTE2MDgwOTVlODZkMmRjY2E4ZjQyMjFfSUQ6NzY0NDcyMjk4OTExOTg4NDI0OV8xNzc5OTU5MDUyOjE3ODAwNDU0NTJfVjM)

*本章通过 4 个端到端场景展示 UEnv 在实际训练工作流中的集成方式。场景覆盖三大主流训练框架（ROLL/VeRL/TRL）的典型任务（数学推理/代码生成/Agent 工具使用），以及自定义环境开发发布。每个场景从用户视角出发，描述从训练请求发起到结果返回的完整链路，帮助读者快速理解 UEnv 各组件的协作关系和实际使用体验。*

### 8\.1 数学训练环境（ROLL \+ MathEnv）

**场景：** 训练工程师使用 ROLL 框架训练数学推理模型，期望在不修改 ROLL 代码的前提下接入 UEnv 统一环境接口。

### 8\.2 代码训练环境（VeRL \+ CodeEnv）

### 8\.3 Agent 训练环境

**场景：** 训练工程师使用 TRAINING\_FRAMEWORK 框架做 Agent 工具使用训练，需要环境支持多轮对话和动态工具调用。Adapter 将框架原生协议转换为 EpisodeRequest，AgentEnv 执行多轮 Episode——每步由模型生成 action，MCP 工具自动路由，环境根据工具返回结果推进状态。支持 list\_tools/call\_tool 双 API，训练侧通过 StreamReport 实时接收每步进度和奖励信号。

### 8\.4 自定义环境开发与发布

**场景：** 环境开发者需要为特定领域（如安全分析）创建自定义环境，期望一次开发后所有训练框架可用。

### 8\.5 大规模并行训练

**场景：** 使用 TRL 框架进行大规模 Agent 训练，需要并发提交大量 Episode 并由 UEnv Server 自动调度和容错。

## 九、测试策略与质量保证

*本章定义 UEnv 的测试分层策略、覆盖率目标和 CI/CD 流水线。测试体系按照标准金字塔组织：单元测试确保各组件独立可验证（覆盖 Reward 系统/Adapter 转换/调度算法/gRPC Handler），集成测试验证 gRPC 全链路和五框架 Adapter 的协议转换正确性，端到端测试以真实训练框架的完整训练循环作为功能门禁。混沌测试（进程 Kill/网络分区/磁盘满/模型宕机）验证容错设计在极端条件下的有效性。*

### 9\.1 测试分层策略

![Image](https://internal-api-drive-stream.feishu.cn/space/api/box/stream/download/authcode/?code=ZjViODU0MTVlY2EyOTVkMGI5ZWQxMjhlODY2ZjQwOWVfMGU4MDg4ODVmNjA2MzAyZDM4Mjc0MmNmOTg2MzkxOGVfSUQ6NzY0NDcyMjk4NjcxMjM3MDExNl8xNzc5OTU5MDUzOjE3ODAwNDU0NTNfVjM)

UEnv 的测试体系按照标准金字塔模型组织，从底层到顶层共五层，各自对应不同的触发频率和质量门禁角色：

底层单元测试和集成测试作为 PR 门禁，每次提交必须通过；端到端测试作为阶段门禁，以真实训练框架的完整训练循环验证系统连通性；混沌测试在生产完备阶段引入，验证容错设计在极端条件下的有效性。

### 9\.2 单元测试与覆盖率目标

单元测试覆盖 UEnv 全部核心模块，确保每个组件独立可验证。覆盖率按模块分层设定目标：

|**模块**|**目标**|**说明**|
|---|---|---|
|Reward 系统|≥ 95%|类 nn\.Module 可组合奖励，已有测试基础|
|Adapter 协议转换|≥ 85%|convert\_request/convert\_response|
|调度算法|≥ 90%|多维过滤逻辑|
|gRPC Service Handler|≥ 80%|Mock Client 测试|
|整体|≥ 80%|全项目目标|



**工具栈：** pytest \+ pytest\-asyncio、grpcio\-testing、pytest\-cov、hypothesis（属性测试）。

### 9\.3 集成测试方案

集成测试验证各组件间的协作正确性，覆盖 gRPC 全链路和五个 Adapter 的协议转换。

**gRPC 全链路测试场景：** 完整 Episode 流程、Worker 注册\+心跳、多 Worker 调度（10 Worker \+ 100 Episode 均衡分配）、5 个 Adapter 转换 round\-trip。五个 Adapter 各含协议转换测试和端到端集成测试。

### 9\.4 端到端测试

每个阶段至少一个真实训练框架的完整训练循环作为端到端验证。端到端测试是最高级别的功能验证，确保从 Adapter 协议转换到 Server 调度再到 Worker 环境执行的完整链路在真实条件下工作正常。

|**阶段**|**框架**|**后端**|**验证目标**|
|---|---|---|---|
|架构验证|ROLL|Process|Episode 完整执行 \+ Reward 正确|
|核心扩展|VeRL|Podman|容器隔离 \+ 资源限制|
|生产完备|TRL|Podman|分布式调度、MCP 工具、大规模并行|



### 9\.5 混沌测试计划

混沌测试在生产完备阶段引入，通过主动注入故障验证系统容错设计的有效性。混沌测试覆盖六大故障场景：Worker OOM（kill \-9 进程，验证 Episode 重新调度和数据不丢失）、Server 崩溃（kill 主节点，验证主从切换和 Worker 自动重连）、网络延迟（tc netem 注入延迟，验证系统在延迟增加时仍能正常完成）、网络分区（iptables DROP 隔离 Worker 与 Server，验证本地 WAL 缓存和重连后数据回传）、磁盘满（dd 填满磁盘，验证 WAL 写入失败的告警和降级处理）、模型端点宕机（停止推理服务，验证 Episode 超时和 fallback 机制）。

### 9\.6 CI/CD 流水线

UEnv 的持续集成/持续部署流水线按触发频率分层配置，确保从代码提交到生产部署的每个环节都有对应的质量门禁。lint 和单元测试在每个 PR 提交时自动运行，集成测试作为合并阻塞项，Docker 镜像构建在 merge to main 时触发，端到端测试按每日运行以降低 CI 资源消耗，混沌测试验证容错机制持续有效。

|**阶段**|**触发**|**通过条件**|
|---|---|---|
|lint \+ unit|每个 PR|0 error, coverage ≥ 80%|
|integration|每个 PR|全部通过|
|build|merge to main|image 构建成功|
|e2e|每日|≥ 1 框架通过|
|chaos|按需|全部场景通过|



## 附录 A: 术语定义

|**术语**|**定义**|
|---|---|
|Episode|一次完整的环境交互周期（reset → N × step → close）|
|EpisodeRequest|训练侧发送的环境运行请求（protobuf 消息）|
|EpisodeResult|环境侧返回的执行结果（protobuf 消息）|
|Trajectory|Episode 执行产出的完整 step\-by\-step 记录|
|StreamReport|Episode 执行中的实时进度反馈|
|Worker|环境执行进程，持有环境实例，执行完整 Episode|
|UEnv Server|有状态环境管理服务（控制平面），调度 \+ 注册|
|UEnvHub|持久化环境注册中心（离线目录服务）|
|Training Adapter|训练框架适配器，协议转换层|
|环境实例|Worker 上的具体环境对象，由实例池管理|
|MCP|Model Context Protocol 风格的工具系统|
|负载画像|Worker/Server 运行状态的持续量化描述|
|步调感知|基于系统负载动态调整交互节奏的能力|
|环境数据合成|环境从交互中产出结构化训练数据的过程|
|预热池|提前创建的环境实例缓存，降低冷启动延迟|
|WAL|Write\-Ahead Log，崩溃恢复的预写日志|



## 附录 B: Reward 兼容性声明

UEnv v7\.2 的 Reward 系统采用类 \`nn\.Module\` 的可组合设计：

\- **Reward 基类**：forward、\\\_\\\_call\\\_\\\_、state\_dict、hooks

\- **容器层**：Sequential、Gate、WeightedSum、RewardList、RewardDict、Mux

\- **信号源层**：RuleReward、NeuralRewardModel、LLMJudge、PRIMEReward、TrajectoryReward

\- **信用分配层**：ExponentialDiscounting、UniformCredit、TokenLevelPRM

\- **归一化层**：NormalizeThenSum、SumThenNormalize、GroupNorm

全部 Reward 组件可直接在 v7\.2 Worker 中运行，无需任何修改。

## 附录 C: 架构可行性评审摘要

*评审基于对 ROLL、ROCK、OpenEnv 三大代码库的深度探索。*

**核心发现：**

1\. **UEnv 定位验证**：RLVR 场景通常使用内置 reward 直接处理，不需要 UEnv。UEnv 核心价值在 Agent 场景（5\-80 步），与 Episode 抽象完全匹配。代码证据：ROLL 的 Sokoban 20 步、WebShop 10 步、Code 80 步。

2\. **Server SPOF 严重度修正**：小规模部署（\&lt;50 Worker）为 HIGH（WAL \+ Active\-Passive 足够），大规模部署（50\-200 Worker）为 CRITICAL（需状态外部化）。

3\. **协议开销**：RLVR 接近单步时 UEnv 的 Episode 抽象有不必要开销，但 RLVR 场景本身不需要外部环境交互。

4\. **缺失能力补齐**：v7\.2 已补充 SubmitBatch、SubmitEpisodeAsync、流控与背压、协议版本策略、自适应心跳、网络分区处理、脑裂防护、请求持久化、GC 暂停隔离等。

*文档结束。 本文档是 UEnv v7\.2 的总体解决方案，覆盖场景需求、概要设计、环境构造（数据合成 \+ 封装）、交互机制（流式交互 \+ 步调感知）、可观测性（负载画像 \+ 基础数据采集）、任务调度、容错可靠性、性能分析、使用场景和测试策略十大维度。*

