# 使用指南

本组文档面向已经完成部署、开始使用 UEnv 的用户。公开运行链路只有三个角色：

| 角色 | 在哪里 | 做什么 |
|---|---|---|
| 评测程序 / 强化学习框架（含接入代码） | 客户端或 GPU 主机 | 提供样本并把任务数据转换为标准数据包，按 ID 还原结果；消费状态、reward 或执行参数更新 |
| UEnv Server | 中心服务主机 | 接收 Episode、管理 UEnv Worker、调度执行并汇总结果 |
| UEnv Worker | 环境执行主机 | 调用模型、运行环境并返回 reward 与 trajectory |

## 主线流程

按下面顺序完成一次完整的使用闭环：先验证模型能力，再用环境反馈训练模型，最后追溯每一次执行的数据。

1. [通用评测流程](./03-evaluation.md)：提交一批样本，逐条获得终态结果。每条结果里，`status` 只表示这次执行在基础设施上有没有跑完（`completed` / `failed` / `timeout`），`reward` 才是任务本身的得分。看结果时先按 `status` 排除系统故障，再用 `reward` 评价任务对错——模型答错是低分，不是系统故障。
2. [强化学习训练指南](./07-post-training.md)：用环境返回的 reward 驱动模型更新，trajectory 记录每一步的模型输入输出。
3. [轨迹采集指南](./12-trajectory.md)：从结果中读取轨迹；需要跨主机长期保存时，启用集中轨迹存储。

具体任务的输入、变量取值、命令和验收集中在[案例库](./02-cases.md)。接入新强化学习框架时才需要阅读[接入指南](../4-接入强化学习框架/01-integration.md)。

## 开始前的可执行检查

以下命令检查 UEnv Server 服务状态、UEnv Worker 注册与网络连通性，全部只读、不提交任何任务。先在 UEnv Server 主机执行：

```bash
uenv version
sudo systemctl is-active uenv-adapter-core.service
curl -fsS http://127.0.0.1:50052/health
uenv status
uenv workers
```

通过标准：systemd 输出 `active`，健康接口返回成功，目标 UEnv Worker 在 `uenv status` / `uenv workers` 中为 `ready`，并声明本次任务需要的环境能力。

再在将要运行评测或训练命令的主机检查到 UEnv Server 的网络。把地址改成实际内网地址；单机部署使用 `127.0.0.1`：

```bash
export UENV_SERVER_HOST='10.0.0.10'

python3 -c 'import os,socket; socket.create_connection((os.environ["UENV_SERVER_HOST"],50051),5).close(); print("UEnv Server 50051 reachable")'
```

需要模型回调时，还必须从可能接单的 UEnv Worker 主机检查模型 API。SWE 任务还需要 Runtime Gateway、catalog 和实例镜像；这些条件与任务类型相关，对应命令在评测及案例页中分别给出。

检查失败时按以下顺序处理：

1. 服务不是 `active`：查看 `sudo journalctl -u uenv-adapter-core.service -n 200 --no-pager`。
2. UEnv Worker 不是 `ready`：查看 `sudo journalctl -u uenv-worker.service -n 200 --no-pager`，再核对[UEnv Worker 接入与注册](../2-部署UEnv/04-worker-registration.md)。
3. 端口不可达：按[多机部署](../2-部署UEnv/02-multi-node.md)检查两个网络方向。
4. 单机安装本身未通过：回到[单机部署](../2-部署UEnv/01-single-node.md)的必要验收。

## 首次出现的术语

| 术语 | 含义 |
|---|---|
| Episode | 一次独立环境执行；包含输入、动作、终止状态和 reward |
| rollout | 模型在一个或多个 Episode 中产生 response/action 的过程 |
| reward | 环境给出的任务得分；基础设施成败由 status 字段表达 |
| trajectory | Episode 的 observation、action、reward 与关联信息 |
| token / mask | 训练框架用于计算 loss 的响应 token 及有效位置标记 |
| catalog / variant | SWE 实例清单及其评测变体 |
| Runtime Gateway | UEnv Worker 上管理 SWE 实例容器和文件/命令操作的接口 |

源码、服务单元或协议中偶尔出现的历史兼容名称，以旁边命令要求的字面值为准；它们不增加新的部署角色。

协议字段的完整定义以 `proto/` 和 `Docs/trajectory/frozen-spec-v2.2.md` 为准；日常使用按本指南操作即可。
