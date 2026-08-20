# UEnv 使用手册

UEnv 为模型评测和强化学习训练提供统一的环境执行能力。它的总体架构包含三个核心环节，外加一个可选组件：

```text
评测程序 / 强化学习框架 → UEnv Server → UEnv Worker
UEnv Hub（可选）→ 向 UEnv Worker 分发环境包
```

评测程序或强化学习框架接管任务数据后，直接将其转换为 UEnv Server 接入的标准数据包；之后由 UEnv Server 调度执行并汇总结果。

| 组件 | 做什么 |
|---|---|
| UEnv Server | 接收标准数据包，管理 UEnv Worker、选择执行节点并汇总结果 |
| UEnv Worker | 运行环境、调用模型并返回状态、奖励和轨迹 |
| UEnv Hub（可选） | 保存和分发有版本的环境包；任务调度始终由 UEnv Server 完成 |

一次样本的环境执行称为 **Episode**。Episode 完成后，上层会得到执行状态、奖励（reward）和可选的执行轨迹（trajectory）。

## 入门指引

从架构概念开始，先完成单机部署建立最小系统，再扩展到多机；随后按任务类型进入评测、强化学习训练与轨迹采集。

| 顺序 | 文档 | 内容 |
|---:|---|---|
| 1 | [架构与组件](./02-architecture.md) | 区分 UEnv Server、UEnv Worker 和可选 UEnv Hub |
| 2 | [一次 Episode 如何完成](./03-episode-lifecycle.md) | 理解样本从提交到返回结果的过程 |
| 3 | [单机部署](../2-部署UEnv/01-single-node.md) | 在一台主机上启动 UEnv Server 和一台 UEnv Worker |
| 4 | [多机部署](../2-部署UEnv/02-multi-node.md) | 把 UEnv Worker 分离到其他主机并验证双向网络 |
| 5 | [通用评测流程](../3-运行任务/03-evaluation.md) | 执行评测并读取终态结果 |
| 6 | [强化学习训练流程](../3-运行任务/07-post-training.md) | 使用环境奖励完成强化学习训练 |
| 7 | [轨迹采集](../3-运行任务/12-trajectory.md) | 保存、查询并关联一次执行的完整轨迹 |

评测和训练的具体数据、命令及预期产物集中在[案例库](../3-运行任务/02-cases.md)。完成入门指引后，按实际任务选择案例；部署章节检查服务、注册和网络，任务执行由案例章节给出。

## 按需查阅

| 需要完成的事情 | 文档 |
|---|---|
| 把强化学习框架接入 UEnv | [自定义强化学习框架接入](../4-接入强化学习框架/01-custom-framework.md) |
| 修改中心服务 | [配置 UEnv Server](../2-部署UEnv/03-server.md) |
| 新增或重新注册 UEnv Worker | [配置并注册 UEnv Worker](../2-部署UEnv/04-worker-registration.md) |
| 统一环境版本 | [部署和使用 UEnv Hub](../2-部署UEnv/05-hub.md) |
| 扩容、升级、下线或备份 | [运行维护](../5-运维UEnv/01-operations.md) |
| 理解名词 | [术语表](../6-查阅参考/01-glossary.md) |
| 配置防火墙 | [端口与连接方向](../6-查阅参考/03-ports.md) |
| 查配置项 | [UEnv Server 与 UEnv Worker 配置参考](../6-查阅参考/02-configuration.md) |
| 开发框架接入或 UEnv Worker | [协议与调用方向](../6-查阅参考/04-protocols.md) |
| 定位故障 | [故障排查](../5-运维UEnv/02-troubleshooting.md) |
