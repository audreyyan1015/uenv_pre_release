# 术语表

首次使用先看“核心组件”和“任务与数据”。协议和兼容代码标识只在开发或排障时需要。

## 核心组件

| 术语 | 含义 |
|---|---|
| 任务样本（sample） | 评测数据或训练数据中的一条输入记录 |
| Episode | 一条任务样本的一次环境执行；多条样本组成批次任务 |
| 框架接入代码 | 评测程序或强化学习框架一侧的转换代码，把上层样本转换为 UEnv Server 的标准数据包，并把结果映射回上层 |
| UEnv Server | 中心服务，管理 UEnv Worker、选择执行节点、跟踪 Episode 并汇总结果 |
| UEnv Worker | 环境执行节点，运行环境、调用模型并回报结果 |
| UEnv Hub | 可选的环境版本与环境包分发服务；Episode 调度由 UEnv Server 与 UEnv Worker 完成 |

## 任务与数据

| 术语 | 含义 |
|---|---|
| `env_type` | 选择环境交互与判分实现的调度键，例如 `qa`、`code`、`swe` |
| `dataset` | 同一环境实现下的数据格式或判分变体，例如 `gsm8k` |
| reward（奖励） | Episode 的累计得分或步骤得分；不等同于训练框架的 loss |
| trajectory（轨迹） | Episode 中的动作、观察、得分和相关元数据 |
| EnvPackage（环境包） | 一个确定版本、带摘要校验的环境文件集合 |
| process plugin | UEnv Worker 以独立进程运行的一类环境实现，位于环境执行侧 |
| rollout | 强化学习训练中由当前策略产生的一次或一批交互样本 |

## 运行与可靠性

| 术语 | 含义 |
|---|---|
| endpoint | 一个组件供其他组件访问的网络地址，通常写成 `HOST:PORT` |
| listen / bind | 服务在本机绑定并监听的地址 |
| `advertise_endpoint` | UEnv Worker 告诉 UEnv Server“请通过这个地址回连我” |
| `ready` | UEnv Worker 心跳正常且可以接收新 Episode |
| `draining` | UEnv Worker 进入排水状态：已派发任务继续完成，新 Episode 由其他 UEnv Worker 承接 |
| `degraded` | UEnv Worker 心跳或执行状态异常，暂停参与新调度 |
| `server_epoch` | UEnv Server 某次运行实例的标识，用于区分重启前后的状态 |
| dispatch lease | UEnv Server 把某次 Episode 尝试交给特定 UEnv Worker 的临时所有权 |
| 幂等 | 相同的最终结果重复上报时，不会把同一 Episode 完成多次 |
| WAL | UEnv Worker 在结果得到 UEnv Server 确认前保存的本地待重放记录 |
| ACK | 接收方确认已经接受并持久化本次上报 |
| Control Plane | UEnv Server 内部的 UEnv Worker 注册、心跳、派发和管理能力，随 UEnv Server 同一进程提供 |

## 兼容代码标识

公开文档把中心组件称为 UEnv Server。以下名称为了兼容当前安装包、配置和协议继续保留，必须在真实命令或代码中原样使用：

| 看到的名称 | 实际含义 |
|---|---|
| `uenv-adapter-core` | UEnv Server 当前的可执行文件和 Cargo 包名 |
| `uenv-adapter-core.service` | UEnv Server 当前的 systemd 单元 |
| `uenv-server` crate | UEnv Server 的注册、调度、状态与结果管理核心库，与 UEnv Server 二进制同属一个服务 |
| `--profile control-plane` | 只安装 UEnv Server |
| `--server HOST:PORT` | 给 UEnv Worker 填写 UEnv Server 地址 |
| `server.endpoint` | UEnv Worker 配置中的 UEnv Server 地址 |
| `uenv logs server` | 查看 UEnv Server 日志 |
| `AdapterCoreService` | 框架接入代码调用 UEnv Server 的 gRPC 服务名 |
| `ControlPlaneService` | UEnv Worker 调用 UEnv Server 的注册、心跳和结果 RPC 集合 |
