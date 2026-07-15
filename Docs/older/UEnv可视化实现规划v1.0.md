# UEnv可视化实现规划v1\.0

# **UEnv 前端可视化：实现规划（已冻结）**



\> **版本**：2026\-06\-12  

\> **状态**：面向代码实现的冻结规划   

\> **用途**：聚合层、前端、各模块事件上报的实现依据



---



## **0\. 背景、整体架构与前端接入说明**



本节交代**为什么需要这个前端**、UEnv 在整体上的运行形态，以及前端**要接入哪些能力**。目的是给实现者建立共同语境，**不是**与各后端仓库/进程的一对一组件对照表；具体接口与数据结构见后文 §4–§5。



### **0\.1 项目背景**



UEnv 是一套**分布式训练环境框架**：训练框架（如 VeRL）通过接入层把「一次样本 / 一道题 / 一个 Episode」送进系统，由调度层分发给执行层，在 Worker 上拉起具体环境（math 插件等）完成多步交互，再汇总奖励与轨迹回传给训练侧。



当前联调与实机验证已经跑通「Adapter → Server → Worker → 环境插件」的主链路，但这条链路具有以下特点，导致**仅靠分散日志很难做有效观测**：



\- **多环节、多进程**：一次 Episode 会穿越接入、调度、执行、环境多个环节，日志分散在各端，难以拼成「同一条线」。

\- **强异步**：调度、执行、结果上报、流式进度并非严格同步；网络抖动后还可能出现补发、迟到事件。

\- **状态有层级**：一次训练 run 下有多条样本、多个 episode；Worker 上有多实例、多 step，需要同时看清「整体进展」和「局部细节」。

\- **控制与观测应分离**：训练能否继续执行，不应绑定「可视化页面是否在线」；观测链路断了，业务仍应能跑完。



因此需要单独建设**面向人的可视化前端**：把一次训练从发起到结束的过程，以结构化、可追踪的方式呈现出来，支撑日常联调、问题定位与事后复盘。本文档规划的就是这一前端及其所依赖的**可视化聚合层**，而不是替换现有 gRPC 控制面或训练框架本身。

### **0\.2 整体架构（观测视角）**



从**业务执行**角度看，UEnv 大致是一条「训练请求进入 → 调度下发 → 环境执行 → 结果回传」的纵向链路，中间穿插注册/元数据、心跳、流式进度等横向能力。此处只强调与可视化相关的**分层关系**：

```Plain Text
flowchart TB
    subgraph train["训练与接入"]
        TF["训练框架 / VeRL 等"]
        AD["训练 Adapter<br/>（样本 → Episode 请求）"]
    end

    subgraph ctrl["控制与调度"]
        SV["调度 Server<br/>（分发、租约、重试）"]
    end

    subgraph exec["执行与环境"]
        WK["Worker<br/>（Episode 执行、流式上报）"]
        ENV["环境插件 / 实例"]
    end

    subgraph meta["元数据（非热路径）"]
        HB["Hub<br/>（环境注册与 manifest）"]
    end

    subgraph obs["观测面（本规划新增）"]
        AGG["可视化聚合层"]
        FE["可视化前端"]
    end

    TF --> AD --> SV
    SV <-->|"控制面 + 数据面"| WK
    WK --> ENV
    WK -.->|"启动时可选"| HB

    AD & SV & WK -->|"结构化观测事件（异步）"| AGG
    AGG -->|"SSE + REST"| FE
    FE -->|"start / stop run"| AGG
```

需要建立的心智模型：



|平面|作用|与前端关系|
|---|---|---|

\| **业务执行面** \| 真正完成训练调度与环境交互 \| 前端**不**直接参与，也不应成为其同步依赖 \|

\| **观测面** \| 收集、归并、展示链路状态 \| 前端**只**与此平面交互（聚合层） \|



Hub 与环境注册属于**元数据/冷路径**，不参与单次 Episode 的实时热路径；前端第一版**不必**为 Hub 单独建主视图，只需在链路状态中保留 \`env\_type\` 等字段即可。Worker 上的 Prometheus \`/metrics\` 等现有端点，经汇总后作为 **metrics 视图** 的数据来源之一，而不是前端逐台直连 Worker。



### **0\.3 前端在体系中的位置**



前端是**观测面的消费端**，不是调度器，也不是日志采集 Agent。它承担三类工作：

1\. **呈现**：把「一条训练 run 当前进行到哪、卡在哪、各 episode/step 关系如何」渲染成工作流与树状结构。

2\. **轻量控制**：仅对**整轮训练**发出开始/终止（经聚合层转发或触发），不做 episode 级、step 级细粒度遥控。

3\. **本地交互**：快照抓拍、实时/快照视图切换、（后续）历史回放时间轴——其中快照**不**向服务端请求「当前帧」，而是复制本地已同步的最新链路状态。



数据流方向可以概括为：



```Plain Text
业务模块 --(异步事件)--> 聚合层 --(SSE 增量)--> 前端本地 ChainState --(渲染)--> 工作流 / 树
                ^                              |
                |                              +--(用户抓拍)--> 本地快照列表
                +--(REST: start/stop run)------+
```



前端**不应**为了看链路状态而去订阅各模块原始 gRPC 流、也不应把各端文本日志当主数据源解析；统一由聚合层做去重、排序、生命周期判定后再推送（见 §6）。



### **0\.4 前端要接入的功能（按能力说明）**



以下按**用户可感知的能力**归纳，实现时对应 §4 通道与 §5 数据结构；**不**按 Adapter/Server/Worker 拆成三个前端子系统。



#### **A\. 训练 run 生命周期（REST → 聚合层）**



|能力|说明|第一版|
|---|---|---|
|开始一轮训练|用户触发 run，获得 `training_run_id` 与 SSE 订阅入口|必须|
|终止一轮训练|用户结束当前 run；聚合层标记 run 关闭边界|必须|
|Run 状态展示|顶栏展示 `RUNNING` / `STOPPING` / `CLOSED` 等|必须|



不包含：暂停/恢复 SSE、episode 级 cancel、Worker 级指令。



#### **B\. 实时链路状态（SSE ← 聚合层）**



|能力|说明|第一版|
|---|---|---|
|订阅 run 事件流|连接 `GET .../stream`，接收 `full_state` 与 `state_delta`|必须|
|维护本地 `ChainState`|按 `event_seq` / patch 规则合并增量，作为唯一实时渲染源|必须|
|断线重连|携带 `Last-Event-ID` 或游标；必要时拉全量 `full_state`|必须|
|工作流主视图|展示 submit → dispatch → execute → report 等阶段与卡点|必须|
|树状详情视图|展示 run / worker / instance / episode / step 层级|必须|
|节点下钻与联动|工作流节点与树节点选中态互通、展示摘要详情|建议|



SSE 推送的是**聚合层归并后的状态增量**，不是原始事件逐条刷屏；高频 step 由聚合层批处理后再推（见 §13\.3）。



#### **C\. 快照（纯前端）**



|能力|说明|第一版|
|---|---|---|
|手动抓拍|深拷贝当前 `ChainState` \+ `EventCursor` \+ `captured_at`|必须|
|快照列表与切换|查看已抓拍时刻的静态链路；可随时切回实时|必须|
|快照持久化|存浏览器 localStorage / IndexedDB|可选|



抓拍不中断 SSE；流式更新**不会**自动生成快照。



#### **D\. 多维索引与上下文（查询 ← 聚合层）**



|能力|说明|第一版|
|---|---|---|
|以 `training_run_id` 为主入口|打开页面即围绕当前 run 观测|必须|
|沿 `correlation_id` / `episode_id` 等关联|从任意已知 id 跳转同链路其他层级（聚合层维护反向索引）|必须|
|搜索 / 过滤|在 run 内按 id、状态筛选 episode 或 worker|建议|



索引字段全集见 §5 与讨论稿 §7\.4；前端消费的是聚合层已关联好的视图，不负责跨模块拼 id。



#### **E\. 日志与 Metrics（独立视图，共用\`correlation\_id\`）**



|能力|说明|第一版|
|---|---|---|

\| 日志面板 \| 按当前 run / \`correlation\_id\` 过滤诊断日志；与工作流**分页/分 Tab** \| P1 \|

\| Metrics 面板 \| 展示吞吐、活跃 episode、池化命中等；与工作流**分页/分 Tab** \| P1 \|

\| 上下文保持 \| 切换 Tab 时不丢失当前 `correlation_id` / run 上下文 \| P1 \|



日志、metrics、事件流**语义分离**：事件流驱动主可视化；日志回答「为什么」；metrics 回答「当时负载如何」。存储层绑定同一 \`correlation\_id\`，UI 不混排。



#### **F\. 历史回放（聚合层持久化 \+ 前端重放）**



|能力|说明|第一版|
|---|---|---|
|事件日志只读查询|从 SQLite 按时间/read 事件链|数据层 P0，UI P1|
|时间轴回放|按 `source_ts` 重建某一时刻界面|P1|
|手动快照作书签|从快照游标继续向前播放|P1|



回放读聚合层持久化事件，**不**依赖前端本地是否曾抓拍。



#### **G\. 体验与健壮性（前端本地）**



|能力|说明|第一版|
|---|---|---|
|数据滞后提示|展示 `updated_at` / 最后 ingest 时间|建议|
|连接状态|SSE 连接中 / 重连中 / 已断开|必须|
|空态与异常态|无 run、run 已关闭、无 episode 等|必须|



### **0\.5 前端明确不接入的范围**



避免 scope 蔓延，第一版**不做**：

- 直连 Adapter / Server / Worker 的 gRPC 控制接口（注册、心跳、Dispatch 等）

- 解析各模块原始 stdout / 非结构化日志作为主 UI 数据源

- 经通道的服务端抓拍、暂停/恢复流、step 级干预

- Hub 环境注册管理界面

- 替代 Prometheus/Grafana 的通用监控大盘（仅做 run 级上下文 metrics 视图）

### **0\.6 与后文章节的关系**



|本节（背景）|后文（实现）|
|---|---|
|0\.4 能力清单|§3 UI 布局、§4 SSE/REST、§5 数据结构|
|0\.2 观测面分层|§2 架构图、§7 聚合层行为|
|0\.3 本地 ChainState|§5\.5 `ChainState`、`StateDelta`|
|0\.4 E/F 日志与回放|§8 存储、§10 分期|



---



## **1\. 范围与目标**



\> 项目背景、整体架构与前端接入能力见 **§0**。



本规划在讨论稿 §7 已冻结架构决策之上，进一步冻结 **UI 形态、通道技术、控制边界、数据结构、异步事件治理、存储选型**，供各模块按同一契约开发。



**第一版目标**：



\- 默认**流式**观察训练链路；用户可随时**本地抓拍**当前整体链路状态为快照，并切回实时流

\- 主界面 **工作流视图 \+ 树状详情视图**（工作流为主）

\- 独立**聚合层**接收各模块异步事件，归并后通过 **SSE** 增量推送至前端

\- 前端控制信号仅 **开始 / 终止** 某轮训练，不做细粒度控制

\- 事件持久化至 **SQLite \+ WAL**；实时态内存缓存；历史回放读 SQLite；GreptimeDB 预留为长周期 metrics 备选项



**不在第一版范围**：



- 经通道的「暂停流 / 恢复流」控制（快照与视图切换纯前端本地行为）

- 经反向请求的服务端抓拍快照

- WebSocket 双工通道（已冻结为 SSE）

---



## **2\. 总体架构**



```Plain Text
flowchart LR
    subgraph modules["业务模块"]
        AD["Adapter"]
        SV["Server"]
        WK["Worker"]
    end

    subgraph agg["可视化聚合层"]
        IN["事件接入"]
        MERGE["排序 / 归并 / 生命周期判定"]
        MEM["内存：当前态缓存"]
        DB["SQLite + WAL：事件日志 / 快照元数据 / 迟到队列"]
        SSE["SSE 推送"]
    end

    subgraph fe["前端"]
        LOCAL["本地 ChainState"]
        SNAP["本地快照列表"]
        WF["工作流主视图"]
        TREE["树状详情视图"]
    end

    AD & SV & WK -->|"HTTP/gRPC 异步上报"| IN
    IN --> MERGE --> MEM
    MERGE --> DB
    MEM --> SSE --> LOCAL
    LOCAL --> WF & TREE
    LOCAL -->|"用户抓拍"| SNAP
    fe -->|"POST start/stop"| agg
```

![Image](https://internal-api-drive-stream.feishu.cn/space/api/box/stream/download/authcode/?code=YjczODY0YzBjYWViYmEyNjgxY2FhZmM2Yzg5YmMxODRfOTU5YjkwZGZiZDhhODkxNzFhNjRmMDg5OWY3YTk2YzZfSUQ6NzY1MDIzODkyODM0NzIyMDk2OF8xNzgxNTQ1ODI3OjE3ODE2MzIyMjdfVjM)



|层级|职责|
|---|---|
|业务模块|关键状态变化产出结构化事件；本地 WAL；异步上报聚合层|
|聚合层|事实源：去重、乱序归并、生命周期边界、推 SSE、写 SQLite|
|前端|维护本地状态副本；抓拍快照；视图切换；仅 start/stop 控制|



## **3\. UI 与交互（已冻结）**



### **3\.1 查看模式**



|模式|默认|数据来源|行为|
|---|---|---|---|

\| **实时流式** \| 是 \| SSE 增量更新本地 \`ChainState\` \| 工作流 \+ 树随事件推进 \|

\| **快照查看** \| 否 \| 用户抓拍时深拷贝的本地 \`ChainState\` \+ 游标 \| 静态展示某一时刻链路；可串联该时刻前事件（只读） \|

\| **历史回放** \| 否（UI 可分期） \| SQLite 事件日志 \+ 重放器 \| 按时间轴重建；数据模型第一版就绪 \|



### **3\.2 快照抓拍（纯前端）**



\- **不**向聚合层发送「抓拍」请求

\- 用户点击「抓拍」时，前端对当前内存中 **最新一次整体链路状态**（\`ChainState\`）做深拷贝，并记录游标（最后收到的 \`event\_id\` / \`\(source\_id, seq\)\`）与 \`captured\_at\`

\- 抓拍后**立即可查看**该快照；SSE 订阅**不中断**，本地 \`ChainState\` 继续更新

- 查看快照时 UI 渲染快照副本；切回「实时」即恢复渲染正在增长的 `ChainState`

\- 流式每次 SSE 更新**不会**自动生成快照



### **3\.3 布局**



|区域|组件|说明|
|---|---|---|

\| 主区 \| **工作流视图** \| Episode 自提交→调度→执行→完成的路径、当前卡点、节点状态 \|

\| 侧栏/抽屉 \| **树状详情视图** \| \`training\_run\` → worker → env\_instance → episode → step 层级 \|

\| 顶栏 \| Run 控制 \| 仅「开始训练」「终止训练」 \|

\| 顶栏 \| 视图切换 \| 「实时」「快照列表」；抓拍按钮 \|

\| 独立 Tab/页 \| 日志 / Metrics \| 共用当前 `correlation_id` 或 `training_run_id` 上下文（§7\.8） \|



### **3\.4 第一版 UI 方案**



采用讨论稿 **方案 B（默认流式 \+ 快照按钮）**；侧边多快照面板（方案 C）可后续演进。

![Image](https://internal-api-drive-stream.feishu.cn/space/api/box/stream/download/authcode/?code=ZTA5ZjI0ZjMzYzQ0NmYwNmQ5MDNjNDI3OGU5ZjRjMjBfY2EzODY5MDFhM2NhZTcyZTE5ZGQ1MzNlYWQzNjA1ZTFfSUQ6NzY1MDIzOTA1MjIyNTk4OTg3MF8xNzgxNTQ1ODI3OjE3ODE2MzIyMjdfVjM)

![Image](https://internal-api-drive-stream.feishu.cn/space/api/box/stream/download/authcode/?code=NjU3YmYyZmIwZTk2NGZhNTE1MjM4NjlkMzhmNTIwM2JfNWQ1NTEwNTMxODZmYWUzMTI3NWJmOWY2NzUyZTUyMDlfSUQ6NzY1MDIzOTI4MzAzMDE5OTI3Nl8xNzgxNTQ1ODI3OjE3ODE2MzIyMjdfVjM)

![Image](https://internal-api-drive-stream.feishu.cn/space/api/box/stream/download/authcode/?code=NjU5OTAwNzgzYmE2MjM3MzdkNDQxMzE5OTUwYmQyMjhfNDRlNmI4ODgzMWE0MWVkNTAyZDJiZjkwNWExNDMyNDhfSUQ6NzY1MDIzOTM2NTU0MDU0NzUxNl8xNzgxNTQ1ODI3OjE3ODE2MzIyMjdfVjM)

![Image](https://internal-api-drive-stream.feishu.cn/space/api/box/stream/download/authcode/?code=ZDJmODQ2ZjgzZGMwYWIxZDMyYzdlYzBmMTc2ZTQ0NGNfMmY3NGNhNjYxZjI2YWFlZDY2NTQwNzM1ODI5NGRhODFfSUQ6NzY1MDIzOTQ2ODgzNzgxNzMwN18xNzgxNTQ1ODI3OjE3ODE2MzIyMjdfVjM)

![Image](https://internal-api-drive-stream.feishu.cn/space/api/box/stream/download/authcode/?code=ODBlNTZiMjAzZjlhNDhjNWZiNmM0NjU2ODljMWQ1NjFfMTBiODcxZTk4MTVmMTEwYzYyNmU0MjEzY2U3MDY2OWRfSUQ6NzY1MDIzOTUyMzczNzA2MjM0MF8xNzgxNTQ1ODI3OjE3ODE2MzIyMjdfVjM)

![Image](https://internal-api-drive-stream.feishu.cn/space/api/box/stream/download/authcode/?code=MmY5OTEzYTk2ODk5MzllNjNjM2JiZjYwNzk3ODAwMGJfZWVkZmJkYWM0ZWJiOGIzM2FmZDNiN2QzMWEwYTVkNmVfSUQ6NzY1MDIzOTU5ODQyOTI0NDU5N18xNzgxNTQ1ODI3OjE3ODE2MzIyMjdfVjM)

![Image](https://internal-api-drive-stream.feishu.cn/space/api/box/stream/download/authcode/?code=M2JmNzgwZGQ2OTAzNmQwOGE2ZmExZDY2ODFkYmFmZDBfY2QyMTJiZjM5MGE3N2JmMDk4MDYzYjFmNmU4ZWJlZjZfSUQ6NzY1MDIzOTY0NDI1NjIyNjI1NV8xNzgxNTQ1ODI3OjE3ODE2MzIyMjdfVjM)



---



## **4\. 通道与控制信号（已冻结：SSE）**



### **4\.1 技术选型**



|方向|协议|说明|
|---|---|---|

\| 聚合层 → 前端 \| **SSE** \| 单向推送；增量更新本地状态 \|

\| 前端 → 聚合层 \| **HTTP REST** \| 仅 run 级 start / stop；查询类 GET \|

\| 模块 → 聚合层 \| **HTTP REST 或 gRPC** \| 异步事件上报（与现有 gRPC 控制面并存，观测面独立） \|



不使用 WebSocket；不做经通道的暂停/恢复/抓拍控制。



### **4\.2 SSE 事件类型（聚合层 → 前端）**



|`type`|时机|载荷|
|---|---|---|
|`full_state`|连接建立或 `Last-Event-ID` 重连后|完整 `ChainState`|
|`state_delta`|归并后状态有变化|`StateDelta`（见 §5\.4）|
|`run_status`|run 开始/结束/关闭|`RunStatusPayload`|
|`ping`|保活|`{}`|



前端收到 \`state\_delta\` 后按 §6\.3 规则合并进本地 \`ChainState\`；**不在前端逐条裸渲染原始事件**。



### **4\.3 前端 → 聚合层控制 API（仅 run 级）**



|方法|路径|说明|
|---|---|---|
|`POST`|`/api/v1/runs`|开始一轮训练；body 含 `training_run_id`、adapter 配置引用等|
|`POST`|`/api/v1/runs/{training_run_id}/stop`|终止该轮训练|
|`GET`|`/api/v1/runs/{training_run_id}/stream`|SSE 订阅|
|`GET`|`/api/v1/runs/{training_run_id}/state`|拉取当前完整状态（非 SSE 场景）|



---



## **5\. 数据结构规划**



![Image](https://internal-api-drive-stream.feishu.cn/space/api/box/stream/download/authcode/?code=OTkwNzEzOWU0NDcyODIxMDgzOGZjZWRjNzQ1MDlhZTdfMTM3NmIzNWRjZjFkODNiNmY4ZDg5NWRkMTNkMzA3MTZfSUQ6NzY1MDI0MDY0NjcxOTk5ODkxNl8xNzgxNTQ1ODI3OjE3ODE2MzIyMjdfVjM)

上图展示了如何使用关联到的字段串联起完整链路结构



以下为本规划涉及的**全部新增或扩展**逻辑结构；实现语言可为 Rust / TypeScript，字段名保持一致。



### **5\.1\`ObservabilityEvent\`（模块 → 聚合层，入站事件）**



模块在关键状态变化时上报；心跳单独 `event_type = HEARTBEAT`。



|字段|类型|必填|说明|
|---|---|---|---|
|`event_id`|string|是|全局唯一，幂等键|
|`schema_version`|string|是|如 `"1"`|
|`correlation_id`|string|是|全链路 trace|
|`training_run_id`|string|否|所属训练 run|
|`adapter_run_id`|string|否|Adapter run|
|`batch_id`|string|否|批次|
|`episode_id`|string|否|Episode|
|`attempt_id`|uint32|否|重试序号，从 1|
|`worker_id`|string|否|Worker|
|`env_instance_id`|string|否|环境实例|
|`step_index`|int32|否|Step 序号|
|`dispatch_lease_id`|string|否|调度租约|
|`scheduler_epoch`|uint64|否|调度 epoch|
|`env_type`|string|否|如 `math`|
|`source_id`|string|是|如 `worker:5e96910f`|
|`module`|string|是|`adapter` / `server` / `worker` / `env`|
|`entity_type`|string|是|`training_run` / `episode` / `worker` / `env_instance` / `step`|
|`entity_id`|string|是|与 `entity_type` 对应|
|`event_type`|string|是|见 §5\.1\.1|

\| \`seq\` \| uint64 \| 是 \| **业务序号**：同一 \`source\_id\` 内单调递增 \|

\| `source_ts` \| int64 \| 是 \| 事件发生时间（Unix 毫秒） \|

\| `payload` \| object \| 否 \| JSON 扩展 \|



聚合层入库时追加：



|字段|类型|说明|
|---|---|---|
|`ingest_ts`|int64|聚合层收到时间（Unix 毫秒）|
|`disposition`|string|处理结果：`accepted` / `late_arrival` / `rejected_closed` / `duplicate`|



#### **5\.1\.1\`event\_type\` 枚举（第一版）**



|值|产出时机|
|---|---|
|`RUN_STARTED`|Adapter 开始一轮训练|
|`RUN_STOPPED`|用户或框架终止 run|
|`EPISODE_SUBMITTED`|Episode 提交至 Server|
|`EPISODE_DISPATCHED`|Server 分发至 Worker|
|`STEP_STARTED`|Step 开始|
|`STEP_COMPLETE`|Step 完成|
|`EPISODE_COMPLETED`|Episode 成功结束|
|`EPISODE_FAILED`|Episode 失败|
|`ATTEMPT_STARTED`|新 attempt 开始|
|`ATTEMPT_CLOSED`|attempt 生命周期关闭|
|`EPISODE_CLOSED`|episode 生命周期关闭|
|`RUN_CLOSED`|run 生命周期关闭|
|`WORKER_REGISTERED`|Worker 注册|
|`WORKER_HEARTBEAT`|Worker 心跳|
|`HEARTBEAT`|通用存活心跳|



### **5\.2\`RunLifecycle\`（聚合层内部，生命周期边界）**



用于判定僵尸事件；写入内存与 SQLite。



|字段|类型|说明|
|---|---|---|
|`training_run_id`|string|PK|
|`run_state`|enum|`PENDING` / `RUNNING` / `STOPPING` / `CLOSED`|
|`run_closed_at`|int64|关闭时间；`RUN_CLOSED` 后设置|
|`started_at`|int64||
|`stopped_at`|int64|用户 stop 或自然结束|



|字段|类型|说明|
|---|---|---|
|`episode_id`|string|PK|
|`correlation_id`|string|索引|
|`episode_state`|enum|`ACTIVE` / `CLOSED`|
|`episode_closed_at`|int64|`EPISODE_CLOSED` 后设置|
|`current_attempt_id`|uint32|当前有效 attempt|



|字段|类型|说明|
|---|---|---|
|`episode_id` \+ `attempt_id`|composite PK||
|`attempt_state`|enum|`ACTIVE` / `CLOSED` / `SUPERSEDED`|
|`attempt_closed_at`|int64||
|`confirmed_event_seq`|uint64|该 attempt 上已确认的最大实体版本（见 §5\.3）|



**关闭语义**：一旦 \`run\` / \`episode\` / \`attempt\` 标记 \`CLOSED\`（或 attempt 为 \`SUPERSEDED\`），后续属于该边界的旧进展事件**不得回写当前 UI 主状态**。



### **5\.3\`EntityVersion\`（聚合层内部，乱序归并用）**



对每个可被更新的聚合对象（如某 `episode_id`、某 `step`）维护：



|字段|类型|说明|
|---|---|---|
|`entity_key`|string|如 `episode:{id}`、`step:{episode_id}:{index}`|
|`source_id`|string|最后更新来源|
|`confirmed_seq`|uint64|已接受的该 `source_id` 最大 `seq`|

\| \`event\_seq\` \| uint64 \| **实体版本号**：每次有效更新 \+1，推给前端的单调版本 \|

\| `last_source_ts` \| int64 \| 最后有效事件的 `source_ts` \|

\| `last_ingest_ts` \| int64 \| 最后入库的 `ingest_ts` \|



命名约定：



- `seq`：来源侧业务序号（入站事件字段）

- `event_seq`：聚合后实体版本（出站 / 前端合并用）

### **5\.4\`StateDelta\`（聚合层 → 前端，SSE 增量）**



|字段|类型|说明|
|---|---|---|
|`training_run_id`|string||
|`event_seq`|uint64|本次 run 或子树的全局递增版本（可选，至少 entity 级）|
|`entity_key`|string|更新对象|
|`patch`|object|JSON Merge Patch 或预定义字段子集|
|`source_ts`|int64|业务发生时间|
|`ingest_ts`|int64|推送时间|
|`cursor`|object|`{ "event_id", "source_id", "seq" }` 供前端快照游标|



### **5\.5\`ChainState\`（前端本地 \+ 聚合层内存，整体链路状态）**



SSE `full_state` 与快照抓拍的对象。



|字段|类型|说明|
|---|---|---|
|`training_run_id`|string||
|`run_state`|enum|同 `RunLifecycle`|
|`updated_at`|int64|最后更新时间（`ingest_ts`）|
|`global_event_seq`|uint64|run 级版本|
|`workflow`|`WorkflowGraph`|工作流渲染态|
|`tree`|`TreeGraph`|树状渲染态|
|`episodes`|map|`episode_id` → `EpisodeView`|
|`workers`|map|`worker_id` → `WorkerView`|
|`cursor`|`EventCursor`|当前同步游标|



#### **5\.5\.1\`WorkflowGraph\`**



|字段|类型|说明|
|---|---|---|
|`nodes`|`WorkflowNode[]`|阶段节点|
|`edges`|`WorkflowEdge[]`|有向边|
|`active_node_id`|string|当前活跃节点|



#### **5\.5\.2\`WorkflowNode\`**



|字段|类型|说明|
|---|---|---|
|`node_id`|string||
|`stage`|enum|`SUBMIT` / `DISPATCH` / `EXECUTE` / `REPORT` / `DONE` / `FAILED`|
|`status`|enum|`PENDING` / `ACTIVE` / `DONE` / `FAILED` / `SKIPPED`|
|`correlation_id`|string||
|`episode_id`|string||
|`label`|string|展示文案|
|`source_ts`|int64|进入该状态的时间|
|`payload_summary`|object|摘要|



#### **5\.5\.3\`TreeGraph\`**



|字段|类型|说明|
|---|---|---|
|`root_id`|string|通常为 `training_run_id`|
|`nodes`|`TreeNode[]`||



#### **5\.5\.4\`TreeNode\`**



|字段|类型|说明|
|---|---|---|
|`node_id`|string||
|`parent_id`|string||
|`kind`|enum|`run` / `worker` / `env_instance` / `episode` / `step`|
|`ref_id`|string|对应实体 ID|
|`status`|enum|`ACTIVE` / `DONE` / `FAILED` / `CLOSED`|
|`children_count`|int32||
|`meta`|object||



#### **5\.5\.5\`EpisodeView\` / \`WorkerView\`**



|结构|主要字段|
|---|---|
|`EpisodeView`|`episode_id`, `correlation_id`, `attempt_id`, `worker_id`, `step_index`, `status`, `event_seq`, `last_source_ts`|
|`WorkerView`|`worker_id`, `active_episodes`, `env_instances`, `last_heartbeat_ts`|



#### **5\.5\.6\`EventCursor\`**



|字段|类型|说明|
|---|---|---|
|`last_event_id`|string||
|`last_source_id`|string||
|`last_seq`|uint64||
|`last_ingest_ts`|int64||



### **5\.6\`ClientSnapshot\`（前端本地，用户抓拍）**



|字段|类型|说明|
|---|---|---|
|`snapshot_id`|string|客户端生成 UUID|
|`training_run_id`|string||
|`captured_at`|int64|抓拍时刻|
|`state`|`ChainState`|深拷贝|
|`cursor`|`EventCursor`|游标|
|`label`|string|用户可选备注|



可选：序列化存 `localStorage` / IndexedDB；第一版不要求上传服务端。



### **5\.7\`LateEventRecord\`（聚合层 → SQLite，迟到/僵尸事件）**



|字段|类型|说明|
|---|---|---|
|`id`|int64|自增 PK|
|`original_event`|JSON|完整 `ObservabilityEvent`|
|`ingest_ts`|int64||
|`reason`|enum|`run_closed` / `episode_closed` / `attempt_closed` / `attempt_superseded` / `seq_stale`|
|`training_run_id`|string|索引|
|`correlation_id`|string|索引|



用途：历史审计、回放日志、统计修正；**不回写**实时 \`ChainState\`。



### **5\.8\`ReverseIndexEntry\`（聚合层内存 \+ SQLite 辅助表）**



|字段|类型|说明|
|---|---|---|
|`index_key`|string|如 `episode_id:xxx`、`worker_id:yyy`|
|`index_value`|string|目标 `correlation_id` 或 `training_run_id`|
|`updated_at`|int64||



保证从 §7\.4 任意索引字段可关联整条链路。



### **5\.9\`LogRecord\`（日志面，存储独立）**



|字段|类型|说明|
|---|---|---|
|`log_id`|string||
|`correlation_id`|string|必填，关联键|
|`training_run_id`|string||
|`episode_id`|string||
|`worker_id`|string||
|`level`|string||
|`message`|string||
|`timestamp`|int64||
|`fields`|JSON|结构化字段|



### **5\.10\`MetricSample\`（metrics 面，存储独立）**



|字段|类型|说明|
|---|---|---|
|`sample_id`|string||
|`correlation_id`|string|可选|
|`training_run_id`|string||
|`metric_name`|string||
|`value`|float64||
|`labels`|JSON||
|`timestamp`|int64||



第一版可写 SQLite；长周期分析迁移 GreptimeDB（§8）。



### **5\.11\`RunControlRequest\` / \`RunControlResponse\`**



**Start**



|字段|类型|说明|
|---|---|---|
|`training_run_id`|string|客户端或 Adapter 生成|
|`adapter_config_ref`|string|配置引用|
|`metadata`|object|可选|



**Stop**



|字段|类型|说明|
|---|---|---|
|`reason`|string|可选|



**Response**



|字段|类型|说明|
|---|---|---|
|`training_run_id`|string||
|`run_state`|enum||
|`stream_url`|string|SSE 路径|



---



## **6\. 异步事件治理（已冻结）**



### **6\.1 僵尸事件**



**定义**：当前 run/episode/attempt 已结束或关闭，但模块因延迟或补发仍上报属于旧生命周期的进展事件。在异步系统中是**常态**，不是异常。



**必备字段**（见 §5\.1、§5\.2）：\`correlation\_id\`、\`episode\_id\`、\`attempt\_id\`、\`run\_state\`、边界关闭标记、\`seq\`、\`source\_ts\`、\`ingest\_ts\`。



### **6\.2 三层处理策略**



```Plain Text
flowchart TD
    E["事件到达"] --> A["1. 判归属"]
    A --> B{"属于当前活跃\nrun/attempt?"}
    B -->|否| L["late_arrival → LateEventRecord"]
    B -->|是| C["2. 判时效"]
    C --> D{"seq / 边界 OK?"}
    D -->|否| L
    D -->|是| F["3. 判用途"]
    F --> G["归并 ChainState + StateDelta"]
    F --> H["追加 events 表"]
    L --> I["审计 / 回放 / 统计"]
```



![Image](https://internal-api-drive-stream.feishu.cn/space/api/box/stream/download/authcode/?code=OWRiZTk3M2Y5ZWVlNWJmZDk3NGZmM2E4NDRjYzIzZjhfNWY4OTgxYzA2ZjI5MDk3ZGI5MzIxZjFiYTkwOGJiNjNfSUQ6NzY1MDI0MDI0MTQ2MjE2ODUyNV8xNzgxNTQ1ODI3OjE3ODE2MzIyMjdfVjM)



|步骤|规则|
|---|---|

\| **1\. 判归属** \| 区分：当前活跃 run、已结束 run、已被替换的新 attempt、已过期旧 attempt \|

\| **2\. 判时效** \| 归属已关闭边界 → 禁止改写当前态；写入迟到队列或标记 \`late\_arrival\` \|

\| **3\. 判用途** \| 迟到事件进入审计、回放日志、统计修正；**不回写**实时 UI \|



### **6\.3 乱序事件**



**场景**：B 后发生但先于 A 到达聚合层。



|规则|说明|
|---|---|
|同一 `source_id`|只接受 `seq > confirmed_seq` 的更新；更小 `seq` 丢弃或归档|
|同一 `seq`|按 `event_id` 幂等去重|
|跨来源|归并时参考 `source_ts`；同一实体内更高 `event_seq` 覆盖|
|推前端前|聚合层已完成排序与状态归并；前端只合并 `StateDelta`，不裸应用乱序原始事件|



**前端合并守则**：



- 只接受同一实体上更高 `event_seq`（或更高 `confirmed_seq`）的 patch

- 同一阶段内可按 `source_ts` 辅助排序展示时间线

- 迟到信息若已进入 `LateEventRecord`，实时视图不体现

### **6\.4 Episode 级归并示例**



对 `episode_id = E1`：



|条件|动作|
|---|---|
|`seq < confirmed_seq`|丢弃或 → `LateEventRecord`（`seq_stale`）|
|`seq = confirmed_seq` 且同 `event_id`|duplicate，忽略|
|`seq > confirmed_seq` 且 attempt 仍 ACTIVE|更新状态，`event_seq++`，推 `state_delta`|
|attempt 已 CLOSED / SUPERSEDED|→ `LateEventRecord`，不推前端|



---



## **7\. 聚合层行为摘要**



|职责|说明|
|---|---|
|接入|接收模块异步 POST；写模块侧 WAL 由模块负责|
|入库|所有事件 append 至 SQLite `events`；附 `ingest_ts`、`disposition`|
|归并|更新内存 `ChainState`、`EntityVersion`、`RunLifecycle`|
|推送|仅归并后的 `StateDelta` / `full_state` 经 SSE 发出|
|关闭|收到 `RUN_STOPPED` / `RUN_CLOSED` 等后标记边界|
|查询|按 `correlation_id` 等索引查事件链；供回放 API（可分期）|



模块上报失败**不阻断**训练执行（讨论稿 §7\.2）。



---



## **8\. 存储方案（已冻结）**



|层次|技术|用途|
|---|---|---|
|模块本地|WAL（各模块自有）|断链缓冲、补发|

\| 聚合层实时态 \| **内存** \| 当前 \`ChainState\`、\`EntityVersion\`、反向索引热数据 \|

\| 聚合层持久化 \| **SQLite \+ WAL 模式** \| 事件 append\-only 日志、\`LateEventRecord\`、生命周期快照、反向索引、日志/metrics 表 \|

\| 长周期 metrics 分析 \| **GreptimeDB（备选）** \| 后续若做长时间、细粒度指标分析再引入；第一版不依赖 \|



### **8\.1 SQLite 表规划**



|表名|主要列|说明|
|---|---|---|
|`events`|`event_id`, `training_run_id`, `correlation_id`, `episode_id`, `attempt_id`, `source_id`, `seq`, `source_ts`, `ingest_ts`, `event_type`, `disposition`, `body_json`|Append\-only|
|`run_lifecycle`|`training_run_id`, `run_state`, `run_closed_at`, …||
|`episode_lifecycle`|`episode_id`, `correlation_id`, `episode_state`, …||
|`attempt_lifecycle`|`episode_id`, `attempt_id`, `attempt_state`, `confirmed_event_seq`, …||
|`late_events`|§5\.7||
|`reverse_index`|`index_key`, `index_value`, `updated_at`||
|`logs`|§5\.9||
|`metrics`|§5\.10|可选同步 GreptimeDB|



**历史回放**：从 \`events\`（及 \`late\_events\`）按 \`source\_ts\` / \`seq\` 重放；内存态丢失时可从事件日志重建。



---



## **9\. 模块上报契约（各层需实现）**



|模块|上报时机|最低必填字段|
|---|---|---|
|Adapter|run 开始/停止、batch 提交|`training_run_id`, `correlation_id`, `source_id`, `seq`, `source_ts`, `event_type`|
|Server|调度分发、attempt 切换、episode 关闭|\+ `episode_id`, `attempt_id`|
|Worker|step 完成、结果、心跳|\+ `worker_id`, `env_instance_id`, `step_index`|



各模块本地 WAL \+ 异步重试；心跳 `HEARTBEAT` 不驱动 UI 主更新，仅存活与补发触发。



---



## **10\. 实现分期**



|阶段|交付|
|---|---|

\| **P0** \| 聚合层：事件接入、SQLite、内存归并、生命周期 \+ 僵尸/乱序策略、SSE \`full\_state\` \+ \`state\_delta\` \|

\| **P0** \| 前端：工作流 \+ 树、SSE 订阅、本地 \`ChainState\`、start/stop、本地快照抓拍与切换 \|

\| **P1** \| 日志 / metrics 独立 Tab，共用 \`correlation\_id\` \|

\| **P1** \| 历史回放 API \+ 回放 UI \|

\| **P2** \| 侧边多快照面板；GreptimeDB metrics 管道（若需要） \|



---



## **11\. 冻结决策速查**



|\#|议题|结论|
|---|---|---|
|1|查看模式|默认流式；可随时本地抓拍快照（状态 \+ 游标）|
|2|UI|实时 ↔ 快照切换；抓拍不中断 SSE|
|3|布局|工作流主视图 \+ 树状详情视图|
|4|数据结构|见 §5 全部表格|
|5|通道|SSE 增量更新；控制仅 start/stop；快照纯前端|
|6|异步治理|僵尸事件三层策略；乱序 seq \+ 双时间戳|
|7|存储|SQLite \+ WAL \+ 内存缓存；GreptimeDB 备选|
|8|架构|独立聚合层、事件驱动为主心跳为辅（讨论稿 §7）|



---



## **12\. 参考**



- \[2026\-06\-12\-frontend\-visualization\-design\.md\]\(\./discussions/2026\-06\-12\-frontend\-visualization\-design\.md\)

- \[全链路联调\-各层接口与参数字段\.md\]\(\./全链路联调\-各层接口与参数字段\.md\)

- \[PROTOCOL\.md\]\(\.\./PROTOCOL\.md\)

## **13\. 補充约束**



### **13\.1 鉴权与控制边界**



前端控制面只允许两类操作：**开始** 与 **终止**。所有控制 API 必须鉴权，且不得暴露匿名调用路径。



|项|约束|
|---|---|
|鉴权|复用项目现有管理端鉴权体系；若后端尚未统一，则先用 bearer token / API token|
|授权|`start/stop` 视为管理操作，至少要求 operator/admin 级别权限|
|传输|控制 API 走普通 HTTP；SSE 只负责单向订阅|
|跨域|若前端与聚合层同源，优先 same\-origin；若跨域，则必须显式配置 CORS|
|CSRF|若控制面使用 cookie/session 鉴权，则必须增加 CSRF 防护；若使用 bearer token，则不强制 CSRF|



**约束原则**：前端页面可以展示任意状态，但控制权限不能从展示权限自动推导。



### **13\.2 SSE 重连与续传语义**



SSE 不是“只连一次就永不失败”的通道，必须定义重连后的恢复规则。



|项|规则|
|---|---|
|事件 ID|每个 SSE 事件携带 `event_id` 或等价 cursor|
|重连依据|前端优先使用 `Last-Event-ID`，若浏览器不支持，则用本地 `EventCursor`|
|重放窗口|聚合层保存最近一段时间的 `StateDelta` / `full_state` 以支持重连补齐|
|cursor 失效|若客户端 cursor 已过期或已跨越保留窗口，聚合层直接返回一次 `full_state`|
|状态恢复|前端收到 `full_state` 后，以该状态覆盖本地 `ChainState`，再继续消费后续 delta|
|去重|重连期间可能重复收到最近事件，前端与聚合层都必须按 `event_id` / `seq` 幂等去重|



**建议实现**：



- SSE 连接建立时先下发一次 `full_state`

- 后续只发 `state_delta`

- 断线重连后，若 cursor 仍有效，则补发缺口；若无效，则重新给全量状态

### **13\.3 高频推送的背压与批处理**



如果 step 级别事件较密集，聚合层不能逐条原样推送前端，否则会把 UI、网络和渲染都打爆。



|场景|处理方式|
|---|---|
|同一实体短时间多次更新|聚合层合并为一次 `StateDelta` 再推送|
|高频 step / heartbeat|前端只保留最新一帧，不逐条渲染历史中间态|
|短周期内重复状态变化|使用 flush window 批量发送，例如 100ms \- 250ms|
|细粒度日志|进入事件日志/SQLite，不进入实时 UI 主渲染链|



**原则**：



- 实时 UI 只消费“可见变化”

- 完整原始事件只用于历史回放、审计、排错

- `HEARTBEAT` 不驱动 UI 主状态，只用于存活和负载展示

### **13\.4 僵尸事件与迟到事件保留策略**



异步回传下，迟到事件一定会出现。处理原则是：**允许到达，不允许改写已关闭主状态**。



![Image](https://internal-api-drive-stream.feishu.cn/space/api/box/stream/download/authcode/?code=MjlmZDkwODNiNTI4YzE2YzlhODVlMWNmNzg1ZjFlNjFfODI1NDFmYmE3NTM4ZjMyMzEyMzAwNDdjMjg4MzA0ODVfSUQ6NzY1MDI0MDQ1NTg0MTQ2NzM0Nl8xNzgxNTQ1ODI3OjE3ODE2MzIyMjdfVjM)

|状态|处理|
|---|---|
|`run/episode/attempt` 仍 ACTIVE|正常归并并推送|
|已 CLOSED / SUPERSEDED|进入 `LateEventRecord`，不回写当前 UI|
|`seq` 落后于确认版本|视为 stale，进入审计 / 回放，不影响当前态|
|重复事件|幂等去重|



**保留策略**：



- `LateEventRecord` 允许留存一段可配置时间

- 过期后可归档或压缩

- 归档前仍可用于历史回放和统计修正

### **13\.5 聚合层恢复与一致性校验**



聚合层重启后必须能从 SQLite 事件表恢复当前态，不能依赖纯内存状态。



|项|规则|
|---|---|
|启动恢复|从 `events` / `late_events` 重建 `RunLifecycle`、`EntityVersion`、`ChainState`|
|一致性校验|启动后校验 `confirmed_seq`、`global_event_seq`、`run_state` 是否自洽|
|健康探针|聚合层必须提供健康检查，至少包括 DB 可用性和恢复完成状态|
|快照刷新|启动后先恢复快照，再恢复增量事件，避免 UI 闪烁|



**建议**：把“启动恢复”视为一等能力，而不是异常处理分支。



### **13\.6 事件保留与清理**



SQLite 作为事件底座时，必须同时定义保留和清理策略，否则会无限膨胀。



|数据|建议策略|
|---|---|
|`events`|长期保留或按时间分区归档|
|`late_events`|保留较短窗口，便于审计和回放|
|`logs`|可按日志策略单独轮转|
|`metrics`|若未来迁移 GreptimeDB，可只保留一段本地缓存|



**关键点**：清理只能作用于“历史可再生数据”，不能破坏当前 \`ChainState\` 恢复所需的最小事件集。



