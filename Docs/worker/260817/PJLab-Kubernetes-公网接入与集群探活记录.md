# 浦江 Lab Kubernetes 公网接入与集群探活记录

> 集群：`vc-a3-241ceshi`
> 记录日期：2026-08-17
> 用途：记录本机通过浦江公网 kubeconfig 访问集群的方式、验证结果和后续部署信息。
> 安全：本文不保存 kubeconfig、Token、client certificate、private key 或其他凭证内容。

## 1. 本机接入方式

### 1.1 kubeconfig 文件

公网访问 kubeconfig：

```text
~/.kube/pjlab-public-config
```

文件权限要求：

```text
chmod 600 ~/.kube/pjlab-public-config
```

当前 kubeconfig context：

```text
vc-a3-241ceshi
```

### 1.2 API Server

公网访问 API Server：

```text
https://10.140.158.149:49256
```

内网 kubeconfig（当前探活失败，保留备用）：

```text
~/.kube/pjlab-internal-config
https://10.107.40.153:443
```

### 1.3 kubectl 使用方式

建议始终显式指定公网 kubeconfig，避免覆盖本机其他 Kubernetes 集群：

```bash
KUBECONFIG="$HOME/.kube/pjlab-public-config" kubectl cluster-info
KUBECONFIG="$HOME/.kube/pjlab-public-config" kubectl get namespaces
KUBECONFIG="$HOME/.kube/pjlab-public-config" kubectl get nodes
```

也可以在当前 shell 临时设置：

```bash
export KUBECONFIG="$HOME/.kube/pjlab-public-config"
```

本机 `kubectl`：

```text
darwin/arm64
client v1.34.1
kustomize v5.7.1
```

集群 Server：

```text
linux/arm64
server v1.31.3+k3s-7c1b3e8e
```

客户端与服务端存在超过 Kubernetes 官方建议的 minor 版本偏差。当前基础命令已验证可用；后续如遇 API 兼容问题，应安装与 Server 更接近的 kubectl 版本，优先使用 v1.31.x。

## 2. 访问前置条件

公网 API 端点仍是 `10.140.158.149` 私网地址，访问依赖当前浦江 Clash Verge Rev 代理/TUN 网络环境。

本机当前验证过的工作路径：

```text
Clash Verge Rev PJLab 配置
  -> 127.0.0.1:7890
  -> 公网 API Server 10.140.158.149:49256
```

kubectl 进程当前可能继承以下代理变量：

```text
HTTP_PROXY=http://127.0.0.1:7890
HTTPS_PROXY=http://127.0.0.1:7890
ALL_PROXY=socks5h://127.0.0.1:7890
```

不要将 kubeconfig 内容或这些代理凭证提交到 Git。

## 3. 连接与权限探活

### 3.1 公网 API

验证命令：

```bash
KUBECONFIG="$HOME/.kube/pjlab-public-config" kubectl cluster-info
```

结果：通过。

```text
Kubernetes control plane is running at https://10.140.158.149:49256
CoreDNS is running
```

### 3.2 Namespace

验证命令：

```bash
KUBECONFIG="$HOME/.kube/pjlab-public-config" kubectl get namespaces
```

当前可见 namespace：

```text
02a6c4f5-684a-41c1-86fd-f57ab5c0f23b
ce309ab9-50ec-4dca-a02e-3a7c98997c96
inference-system
karmada-cluster
kube-node-lease
kube-public
kube-system
llm-gateway
lws-system
mindx-dl
studio-ams-system
volcano-system
```

### 3.3 资源权限

当前凭证已验证具备：

```text
get nodes       yes
create pods     yes
create jobs     yes
```

完整授权范围还包括 Pod、Deployment、DaemonSet、StatefulSet、Job、CronJob、Service、PVC、Secret、ConfigMap、Pod exec/log/port-forward，以及 Volcano Job 相关资源。

当前没有发现 ResourceQuota 或 LimitRange：

```text
kubectl get resourcequota -A -> No resources found
kubectl get limitrange -A -> No resources found
```

StorageClass 的集群级 list 权限被拒绝，但现有 PVC 能看到其 StorageClass 名称。

## 4. 集群资源状态

### 4.1 节点

探活统计：

```text
节点总数：524
Ready：524
ARM64：522
AMD64：2
Kubernetes：v1.28.15（节点侧）
容器运行时：containerd 1.6.22
操作系统：openEuler 22.03 (LTS-SP4)
```

### 4.2 Ascend 资源

资源名：

```text
huawei.com/Ascend910
```

节点资源统计：

```text
节点 capacity：520 × 16 = 8320 张
节点 allocatable：518 × 16 + 2 × 15 = 8318 张
```

说明：这是当前集群可见总资源，不代表当前用户已经独占或空闲的资源。实际可申请量仍由平台 VC、队列、调度状态和其他任务占用决定。

节点相关标签包括：

```text
node.kubernetes.io/npu.chip.name
mind-cluster/npu-chip-memory
resource.compute.sensecore.cn/vc-uid
cluster.x-k8s.io/vcluster-name
cluster.x-k8s.io/vcluster-namespace
```

### 4.3 PVC 与存储

当前可见大量 `default` namespace PVC，主要使用：

```text
StorageClass：quark-vcproxy-sc
AccessMode：RWX
```

已看到的 PVC 容量包括：

```text
1Ti、20Ti、60Ti、510Ti
```

存在部分 Pending PVC，创建新 PVC 前应先确认平台存储策略、配额和实际挂载路径。

## 5. 当前工作负载与代码框架部署情况

### 5.1 全局工作负载概览

当前 Pod 统计：

```text
Pod 总数：4558
Running：797
Succeeded：2286
Failed：1475
```

主要长期运行系统组件：

```text
kube-system：CoreDNS、Grafana、KubeRay、Volcano、VC webhook 等
llm-gateway：auth-verifier、auto-scaler、l7-gateway、配置服务、traffic mirror
lws-system：lws-controller-manager
```

### 5.2 UEnv 相关工作负载

当前 `default` namespace 已发现与 UEnv/VeRL 相关的运行工作负载：

```text
uenv-public-verl-128-copy-master-0
uenv-public-verl-128-copy-worker-0 ... worker-6
uenv-workspace-shell
```

状态：

```text
uenv-public-verl-128-copy：1 个 master + 7 个 worker，全部 Running
uenv-workspace-shell：Running
```

该运行任务使用镜像：

```text
registry2.d.pjlab.org.cn/ccr-huawei-infer/vllm-ascend:v0.22.1rc1-a3
```

每个 master/worker 的资源请求和限制：

```text
CPU：256
内存：1920Gi
huawei.com/Ascend910：16
```

总计：

```text
8 个 Pod × 16 张 Ascend910 = 128 张卡
8 个 Pod × 256 CPU = 2048 CPU
8 个 Pod × 1920Gi = 15360Gi 内存
```

该任务由 Volcano 管理：

```text
Job：uenv-public-verl-128-copy
Namespace：default
Scheduler：volcano
minAvailable：8
running：8
```

每个 Pod 挂载一个 RWX PVC，当前任务 PVC：

```text
pvc-prxv9
容量：510Ti
StorageClass：quark-vcproxy-sc
```

### 5.3 当前 UEnv 部署判断

当前集群已经存在名为 `uenv-public-verl-*` 的 VeRL/推理训练相关任务和 `uenv-workspace-shell`，说明此前已有 UEnv 训练侧或相关工作环境部署痕迹。

但本次探活未发现以下独立服务工作负载：

```text
uenv-worker Deployment/Pod
uenv-server Deployment/Pod
uenv-hub Deployment/Pod
OpenHands Agent Deployment/Pod
独立 SWE-smith Worker Pool
```

因此当前结论是：

```text
训练/VeRL 侧：已有 128 卡任务正在运行
UEnv Worker：尚未发现独立 Kubernetes 部署
UEnv Server/Hub：尚未发现独立 Kubernetes 部署
SWE-smith 环境实例池：尚未发现独立 Kubernetes 部署
```

`uenv-workspace-shell` 是一个可用的工作空间 Pod，但不能据此认定 UEnv Worker 或 SWE-smith Runtime 已完成部署。

## 6. 当前发现的风险与后续记录项

1. 当前 API Server 公网端点可用，内网 API `10.107.40.153:443` 仍未通过本机探活。
2. 当前 kubectl 客户端 v1.34.1 与集群 Server v1.31.3 存在版本偏差，建议后续准备 v1.31.x 客户端。
3. 集群总可见 Ascend910 资源为 8320 张，不能将其视为当前用户可立即使用的空闲资源。
4. 当前 128 卡任务已占用 8 个 Pod，每个 Pod 申请 16 张 Ascend910。
5. `default` namespace 存在大量历史 Succeeded/Failed Pod，后续部署前应区分现有任务，禁止误删或清理他人工作负载。
6. 尚未确认当前 VC/队列对新任务的实际剩余配额，后续需要结合平台任务列表和 Volcano queue 状态继续确认。
7. 尚未确认 UEnv Worker 在该集群内采用 Docker-in-Pod、Kubernetes-native Pod/Job 或其他 Runtime 方案。
8. 尚未执行任何创建、修改或删除集群资源的操作；本次仅执行读取、权限检查和 API 探活。

## 7. 后续增量记录模板

后续每次探活或部署在本文档追加：

```text
### YYYY-MM-DD HH:MM 探活/变更

- kubeconfig：公网/内网
- context：
- namespace：
- 变更：
- 资源：
- 镜像：
- Pod/Job：
- 结果：
- 日志/事件：
- 回滚方式：
- 下一步：
```

## 8. SWE-smith 集群部署可行性与容量规划

### 8.0 已确认的目标分层与实施决策

本项目已经确认下一步采用：

```text
方案 A：Kubernetes-native SWE backend
```

后续实现、部署和验收均以方案 A 为准。下一步不以 Docker socket、Docker-in-Docker 或 containerd socket 作为浦江集群的前置条件，也不把“申请 privileged 容器”作为默认补救路径。现有 Docker/Podman backend 只保留用于 7143 兼容回归，不作为浦江集群的部署方案。

SWE-smith session 的资源语义已经冻结为：

```text
每个 SWE-smith session 永远使用 CPU-only Kubernetes Pod/Job
不申请 huawei.com/Ascend910
不挂载 /dev/davinci*
不加载 CANN/torch-npu
不占用 NPU 设备
```

集群上的 NPU 只用于独立的模型推理、训练或其他明确声明 NPU 资源的服务，不得因为 SWE session 数量扩展而隐式申请 NPU。

#### 短期目标：单实例完整有效链路

短期目标不是并发压测，也不是 gold 演示，而是完成一条真实的非 gold SWE-smith episode：

```text
真实 Smith instance
  -> CPU-only session Pod
  -> 正确的 clean-to-buggy provision
  -> 集群内 Agent + 模型完成修复
  -> 官方 SWE-smith harness
  -> reward > 0
  -> 有效 trajectory/rollout
  -> 完整 cleanup
```

短期通过条件：

```text
session Pod 创建并 Ready
环境支持完整 Smith instance，而非只支持 gold shortcut
Agent 使用集群内模型服务完成非 gold 修复
model patch 非空且不是 gold patch
official harness 返回 reward > 0
trajectory.steps 非空且 schema 合法
rollout、patch、artifact 可回取
session Pod、exec、lease、workspace 无泄漏
```

#### 长期目标：多实例并行训练

长期目标是在不影响训练/推理服务的前提下，将 CPU-only SWE session 扩展为多 Worker、多 Pod 并发执行：

```text
多 Worker
  -> 多个 CPU-only SWE session Pod
  -> run-level admission / Worker lease / Agent lease
  -> 并行 Smith episode
  -> 批量有效 rollout
```

长期能力包括：

```text
按节点 CPU/内存/存储/PID/FD 实际上限扩容
按 Worker K 和 run-level scheduling policy 限流
镜像预取和 catalog 分片
多环境实例隔离和回收
Agent/model gateway 并发对齐
trajectory/artifact 高并发上传
Pod/exec/lease reconcile
按资源画像逐级扩展到 16/32/64，必要时再评估 128 个 session
```

长期目标中的“128”指最多 128 个 CPU-only SWE session 的容量候选，不指 128 张 NPU，也不指当前已经具备 128 个可运行实例。

### 8.1 当前目标

目标是在 `vc-a3-241ceshi` 上使用 UEnv 框架完成：

1. 准备 SWE-smith 环境镜像和 catalog；
2. 部署可注册到 UEnv Server 的 Worker；
3. 拉起若干 SWE-smith session/environment instance；
4. 接收外部训练侧 episode，执行 OpenHands/Agent 轨迹生成；
5. 回传 reward、resolved、trajectory 和 artifact，供训练侧消费。

目标链路仍然是：

```text
VeRL / Adapter
    -> UEnv Server / Adapter Core
    -> UEnv Worker
    -> Runtime Gateway
    -> OpenHands Agent
    -> SWE-smith environment session
    -> reward / trajectory / artifact
```

### 8.2 当前 128 卡资源的实际含义

探活时已经发现一个正在运行的 Volcano Job：

```text
uenv-public-verl-128-copy
```

该 Job 当前为：

```text
1 master + 7 worker = 8 个 Pod
每个 Pod 请求 16 张 huawei.com/Ascend910
总计 128 张卡
每个 Pod 请求 256 CPU、1920Gi 内存
```

因此不能把“当前申请的 128 张卡”再次按 128 张空闲卡计算。至少在该 Job 保持 Running 期间，128 张 Ascend910 已被该任务占用。

SWE-smith session 已明确只使用 CPU-only Pod/Job。它的主要资源是 CPU、内存、可写存储、进程数、文件描述符、容器运行时和网络，不申请 Ascend NPU。CPU-only Pod 是否可以与当前 NPU Job 共存，仍需确认 VC/队列的调度规则和节点剩余 CPU/内存。

模型推理服务可以单独申请 NPU，但必须与 SWE session 分离；模型服务的 NPU 资源不能计入 SWE session 容量，也不能把 NPU 绑定到每个 episode。

当前实测还没有拿到每节点 CPU、内存、cgroup、磁盘剩余量及当前用户级配额，因此不能仅根据 128 张卡给出最终实例数。

### 8.3 初始实例数量建议

既有 7143 估算给出的单个 SWE session admission 预算是：

```text
CPU：2 vCPU
内存：4GiB 硬预算，3GiB 稳态目标
可写层：15-20GiB
临时目录：2GiB
PID：256-512
FD：512-1024
```

但 7143 的数据只包含低负载容器 RSS 约 40-175MiB 的观测，不能直接作为 pytest、编译、git 和长尾仓库测试峰值。并且 Kubernetes-native session 与 Docker session 的启动和存储开销不同。

建议采用以下阶梯，而不是一次性拉满：

| 阶段 | Worker/Pod 规模 | 每 Worker 初始并发 K | 全局 SWE session | 目的 |
|---|---:|---:|---:|---|
| Smoke | 1 个 Worker | 2 | 2 | 验证镜像、Runtime、reset/provision/exec/submit/cleanup |
| 小规模压测 | 2-4 个 Worker | 4 | 8-16 | 观察单节点 CPU/RAM/存储和并行隔离 |
| 第一版生产候选 | 按实测节点数 | 8 | 32-64 | 保守利用 CPU-only 资源，避免影响 128 卡训练 |
| 扩展目标 | 按资源画像 | 8-12 | 64-128 | 只有长稳和清理验证通过后启用 |

在当前信息下，长期并发容量的候选阶梯是：

```text
先部署 1 个 Worker，K=2
通过后扩到 4 个 Worker × K=4 = 16 个并发 session
再根据资源采样扩到 32-64 个并发 session
```

短期只执行 1 个 Worker、1 个 CPU-only session 的完整非 gold 验收。短期验收通过后，才进入长期并发阶段：先按 **2-4 个 Worker、16-32 个 CPU-only session** 起步，再根据资源画像扩展到 64，最后才评估 64-128。不得当前直接配置 128 个 session。

### 8.4 关于通过 7143 日志估算资源

既有资料允许通过 `secrets/README.md` 的 SSH 拓扑连接 7143，读取 Worker/SWE 日志和容器统计。重点采集：

```bash
nproc
free -h
cat /sys/fs/cgroup/cpu.max
cat /sys/fs/cgroup/memory.max
cat /sys/fs/cgroup/pids.max
ulimit -n
ulimit -u
docker ps --format '{{.Names}}'
```

每 30 秒记录一次以下字段，至少覆盖 200-500 个跨仓库样本或一个完整压测窗口：

```text
episode_id / instance_id
session duration
CPU usage p50/p95/p99
RSS/working set p50/p95/p99
可用内存
容器数和超龄容器数
writable layer / workspace 磁盘增量
PID/FD 使用量
镜像命中/冷启动耗时
exec timeout 数
episode error/timeout rate
cleanup success rate
```

该数据只能作为 **Docker/7143 基线**。迁移到 Kubernetes 后仍需在目标 Pod/Job 运行同样的采样，不能直接把 A100 主机的 CPU/RAM/磁盘结论套到 A3 节点。

### 8.5 容量计算模型

目标集群上，单节点可容纳的 SWE session 数应按以下最小值计算：

```text
N_host = min(
  floor((CPU_available - reserve) / 2),
  floor((RAM_available - reserve) / 4GiB),
  floor((writable_storage_available - reserve) / 20GiB),
  PID_limit / 512,
  FD_limit / 1024,
  container_runtime_limit,
  worker_gateway_limit,
  agent_limit
)
```

建议保留：

```text
CPU：至少 25%，与训练/LLM 同节点时 35%
内存：至少 20-25%
存储：至少 20%，并设置每 session 上限和清理策略
```

全局并发不能只看节点数量，还要取以下上限的最小值：

```text
global_concurrency = min(
  sum(worker.max_concurrent),
  sum(gateway.capacity),
  sum(agent.max_concurrent_jobs),
  CPU admission,
  RAM admission,
  storage admission,
  run-level max_episode_concurrency
)
```

## 9. Worker 在 Kubernetes 上的部署方案

### 9.1 现有 Worker 不能直接原样搬入普通 Pod

当前 UEnv Worker 的 SWE 路径依赖 `SweInstancePool`、Runtime Gateway 和 Docker session。已有文档明确记录：Worker 如果在容器内继续启动 SWE Docker，需要 Docker socket、Docker-in-Docker 或受控容器 API。

浦江集群节点运行时是 containerd，当前没有证据表明普通用户 Pod 可以访问宿主 Docker socket。因此不能直接假定下面的方式可用：

```text
普通 Worker Pod
  -> docker run swe-smith-image
```

必须先确认平台是否允许：

```text
Docker socket
Docker-in-Docker / privileged
Podman service
Kubernetes API 创建 Job/Pod
受控 container runtime API
```

### 9.2 推荐的 Kubernetes-native 方案

如果平台不允许 Docker socket，推荐将 SWE session 抽象为 Kubernetes-native backend：

```text
UEnv Worker Deployment
    -> Kubernetes API / session launcher
    -> 每个 episode 一个受控 SWE Pod 或 Job
    -> Runtime Gateway / exec proxy
    -> episode 完成后删除或回收 Pod
```

每个 SWE session 应具备：

```text
唯一 episode_id / session_id
唯一 workspace
唯一 lease_id
指定 image digest
CPU/memory/ephemeral-storage/pids 限制
执行超时和 kill 机制
成功/失败/取消统一 cleanup
Worker 重启后的 reconcile
```

建议将镜像缓存和 session 生命周期分开：

```text
镜像层：节点缓存或平台镜像仓库
catalog/eval_spec：RWX PVC 或对象存储
每 episode workspace：独立 emptyDir/PVC 子目录或独立 PVC
trajectory/artifact：对象存储或专用 RWX 路径
```

不建议为全量 Smith catalog 预开匿名容器。预热应优先做镜像层、最常用 repo 子集和有限 ready slot。

### 9.3 Worker Deployment 的组成

首版可以拆成以下 Kubernetes 对象：

```text
Namespace：建议使用平台分配的业务 namespace，不要默认复用 default
Secret：Hub token、Server token、Gateway API key、LLM key
ConfigMap：Worker 非敏感配置、env package 坐标、并发策略
PVC：catalog、镜像索引、trajectory/artifact 或 workspace 元数据
Deployment：uenv-worker
Service：Worker control/health/gateway 集群内入口
Service/Ingress/平台端口：供 Server 或 Agent 回连的入口（如需要）
RBAC：仅允许 Worker 创建/查询/删除带固定 label 的 session Pod/Job
NetworkPolicy：只允许 Server、Agent、Hub、artifact 服务等必要方向
```

Worker 不应使用集群管理员权限。若采用 Kubernetes-native session launcher，建议建立专用 ServiceAccount，只允许：

```text
get/list/watch/create/delete/patch session Pod/Job
get/list/watch 相关 Service/PVC
读自身配置 Secret
```

并给所有 session 加固定标签，例如：

```text
app.kubernetes.io/part-of: uenv
uenv.io/component: swe-session
uenv.io/worker-id: <worker-id>
uenv.io/episode-id: <episode-id>
uenv.io/lease-id: <lease-id>
```

### 9.4 如果平台允许 Docker socket

只有平台明确提供受控 Docker socket 或 DinD 后，才考虑保留现有 Docker backend。此时 Worker Pod 仍需补齐：

```text
容器运行时访问权限
镜像仓库访问凭据
镜像本地缓存/预拉取
workspace 与 writable layer 配额
exec timeout/kill
容器 destroy/reconcile
```

即便 Docker backend 可用，也不建议将 128 个 session 与 128 张 NPU 绑定。SWE 容器默认不申请 `huawei.com/Ascend910`，LLM 推理单独部署和调度。

## 10. UEnv 模块间通信与外部服务器通信

### 10.1 全部部署在当前集群内

最容易控制的拓扑是：

```text
uenv-server / adapter-core  -> ClusterIP Service
uenv-hub                    -> ClusterIP Service + PVC
uenv-worker                 -> Deployment + Service
runtime-gateway             -> Worker sidecar 或 ClusterIP
openhands-agent             -> Deployment + Service
trajectory/artifact         -> PVC/Object Storage Service
VeRL training               -> ClusterIP/Service 或同 namespace 访问
```

服务间使用 Kubernetes DNS：

```text
<service>.<namespace>.svc.cluster.local
```

优点是无需公网回连，Service 地址稳定，适合高并发 episode 和内部 Gateway 调用。

### 10.2 与现有外部模块通信并非做不到

并不是不能通信，但要区分出站和入站：

#### 集群 Pod -> 外部 Server/Hub/Agent

理论上可行，只要集群 egress、DNS 和安全策略允许：

```text
UEnv Worker -> 8.130.75.157:8088
UEnv Worker -> 8.130.95.176:8088
Worker/Agent -> 外部 LLM endpoint
```

需要在集群内探测 TCP/HTTP，并确认平台是否要求 HTTP 代理或白名单。当前机器能访问 Kubernetes API，不代表 Pod 出站一定能访问这些地址。

#### 外部 Server -> 集群 Worker

这是更困难的方向。现有协议要求 Server 回连 Worker gRPC endpoint：

```text
Server -> Worker :28888 DispatchEpisode
```

集群内 Worker 的 ClusterIP 通常不能被公网 Server 直接访问。需要由平台提供以下任一方式：

1. LoadBalancer/公网 Service；
2. NodePort + 安全组白名单；
3. Ingress/TCP Gateway；
4. 专用反向隧道或 VPN；
5. 将 Server/Adapter 迁入同一 Kubernetes 集群；
6. 改为 Worker 主动 poll/stream 任务，避免 Server 入站回连。

当前最推荐的两种收敛方式：

```text
方案 A：Server、Worker、Agent 全部迁入集群，使用 ClusterIP
方案 B：保留外部 Server，但为 Worker 提供受控公网回连入口；Agent 采用主动 Poll
```

如果不提供 Server -> Worker 的回连入口，直接照搬现有 `ControlPlaneService + DispatchEpisode` 链路会卡在 dispatch 阶段。仅有 Worker -> Server 的出站连通并不足够。

### 10.3 与现有 7143/208.77/7142 模块的兼容方式

现有外部拓扑是：

```text
7142：VeRL / Adapter
8.130.75.157：Adapter Core + Server :8088
8.130.95.176：Hub :8088
7143：Worker :28888 / Gateway :28097
8.130.208.77：OpenHands Agent Pool
```

迁移到浦江后可以采用混合拓扑，但需要逐条验证：

```text
集群 Adapter/VeRL -> 外部 Server :8088：通常是出站连接
集群 Worker -> 外部 Hub :8088：通常是出站连接
外部 Server -> 集群 Worker :28888：需要公网 Service/隧道
外部 Agent -> 集群 Gateway：需要公网 Service/隧道
集群 Worker -> 外部 Agent/LLM：需要出站和协议匹配
```

因此“外部模块完全不能通信”不是结论；准确结论是：

```text
出站 HTTP/gRPC 通信有希望，需做 Pod 内实测；
外部主动回连集群 Worker/Gateway 必须补入口或改为主动轮询；
若将 Server/Agent/Worker 一起迁入集群，则最简单稳定。
```

## 11. 推荐实施阶段

### 阶段 A：目标节点和运行时探针

创建 1 个低资源 CPU-only 探针 Pod，确认：

```text
nproc/free/df/cgroup/pids/ulimit
Pod 到外部 Server/Hub 的 TCP/HTTP
Pod 到 Kubernetes API 的访问
PVC RWX 挂载和写入
镜像仓库拉取
是否允许创建 session Pod/Job
是否允许 privileged/Docker socket（如平台方案需要）
```

### 阶段 B：镜像和 catalog smoke

首期验证仍使用 1-3 个真实 Smith instance 和对应镜像，验证：

```text
镜像拉取或预加载
catalog 加载
image digest 校验
SWE session 创建
workspace provision
exec/read/write/submit
官方 swesmith grader
trajectory/artifact 输出
cleanup
```

### 阶段 C：集群内 Worker 单机双并发

```text
1 Worker
K=2
2 个不同 instance
```

验收：无串线、无超时残留、无孤儿 Pod/容器、两条 trajectory 均可回取。

### 阶段 D：外部 Server/Agent 混合联调

先验证：

```text
Worker -> 8.130.75.157:8088
Worker -> 8.130.95.176:8088/healthz
外部 Server -> 集群 Worker/gateway 的回连入口
外部 Agent -> 集群 Runtime Gateway
```

若最后两个方向无法打通，应迁移 Server/Agent 到集群或实现 Worker/Agent 主动 poll 方案。

### 阶段 E：小规模压测

```text
2-4 Worker
每 Worker K=4
全局 8-16 session
```

连续运行跨 repo 样本，验证 CPU、内存、PVC、镜像缓存、Pod 创建时延和 cleanup。

### 阶段 F：逐级扩容

建议扩容阶梯：

```text
2 -> 8 -> 16 -> 32 -> 64 -> 128
```

每次扩容前必须检查：

```text
CPU pressure P95 < 70%
PVC/writable storage < 70%
无 OOM/PID/FD exhaustion
无 Pod/container orphan
episode timeout/error rate 稳定
Agent/LLM QPS 未成为瓶颈
```

在 Worker lease、exec timeout、destroy/reconcile 和外部通信未验证前，不得因为集群有 128 张卡就直接申请 128 个 SWE session。

## 12. 当前结论

1. 当前集群上已经有 128 卡 VeRL 工作负载运行，但这 128 张 NPU 不是可直接分配给 SWE session 的空闲资源。
2. SWE-smith session 默认不申请 NPU，理论上可以使用 CPU-only Pod 与训练任务共存，但必须实测节点 CPU/内存、队列和 VC 规则。
3. 当前推荐先做 1 Worker/K=2，再做 2-4 Worker/K=4，第一版全局目标 16-32；根据实测再扩到 64，最终是否到 128 由资源画像决定。
4. 当前 UEnv Worker 的 Docker SWE backend 不能假定在普通 containerd Pod 中直接可用；优先设计 Kubernetes-native session launcher，或向平台申请受控 Docker runtime。
5. 外部模块通信不是绝对不可行：Worker 出站访问外部 Server/Hub 通常可行，但外部 Server/Agent 回连集群内 Worker/Gateway 必须提供公网 Service、隧道或改成主动轮询。
6. 最稳定的最终形态是将 Server、Hub、Worker、Agent、Runtime Gateway 和训练侧服务尽量放入同一 Kubernetes 网络，使用 ClusterIP；外部模块仅保留必要的出站集成。

## 13. 方案 A：Kubernetes-native SWE backend 改造规划

> 本节是下一阶段的代码与部署设计，当前尚未执行集群部署，也未创建新的 Pod、Job、Service 或 PVC。

### 13.1 改造目标

将当前依赖 Docker/Podman CLI 的 SWE session backend 改造成 Kubernetes API backend，使 Worker 对上层继续暴露现有 UEnv SWE 能力，而对下层将一个 SWE session 映射为一个受控 Kubernetes Pod/Job。

目标是不改变上层调用语义：

```text
Server / Agent / Runtime Gateway
  -> create session
  -> reset/provision
  -> exec/read/write
  -> submit/evaluate
  -> trajectory/artifact
  -> destroy/release
```

仅替换底层运行时：

```text
当前：SweSession -> docker/podman CLI -> container
目标：SweSession -> KubeSessionBackend -> Kubernetes Pod/Job
```

首期只支持 Kubernetes-native backend，不同时引入 Docker socket、DinD 或 containerd socket。这样可以避免把宿主机容器运行时权限暴露给 Worker Pod。

### 13.2 不改变的上层契约

以下 UEnv 接口和生命周期应保持兼容：

| UEnv 层 | 现有能力 | Kubernetes backend 对接方式 |
|---|---|---|
| `SweInstancePool` | capacity、pending reservation、session map、create/destroy | session map 保存 `session_id -> KubeSessionHandle`，继续做并发准入 |
| `SweSession` | provision、reset、exec、read、write、submit、terminate | 将容器句柄替换为 Pod/Job handle + Pod 名称 + namespace |
| Runtime Gateway | `/runtime/v1/sessions` | 路由、鉴权、响应 JSON 保持不变 |
| Runtime Gateway | `exec/read/write/submit/delete` | 继续调用 `SweSession`，不让 Gateway 直接调用 Kubernetes API |
| native Episode | `DispatchEpisode(env_type=swe)` | 与 Gateway 共用同一个 pool/backend |
| catalog | `instance_id`、variant、image、patch、test 命令 | 继续由 `InstanceStore` 提供，session 只引用 instance_id |
| runtime contract | workspace、patch semantics、reward adapter | 继续作为 provision/evaluate 的权威语义 |
| trajectory | step trace、seal、upload、trajectory ref | session backend 只提供命令和文件结果，不改变轨迹 schema |
| metrics | pool size、session、timeout、cleanup | 增加 Pod/Job 状态和 Kubernetes API 延迟指标 |

关键原则：不能为了适配 Kubernetes 而新增另一套 `swe-k8s` 上层协议，也不能让 OpenHands、Adapter 或 Server 感知 Pod 名称、namespace、PVC 等基础设施细节。

### 13.3 新增 backend 抽象

现有 `ContainerRuntime`/CLI 路径需要从 `SweSession` 中抽离。建议新增内部 trait，名称可按代码风格调整：

```rust
trait SweSessionBackend: Send + Sync {
    fn provision(&self, spec: &ProvisionSpec) -> Result<BackendSession, BackendError>;
    fn exec(&self, session: &BackendSession, command: &str, timeout: Duration)
        -> Result<ExecResult, BackendError>;
    fn read_file(&self, session: &BackendSession, path: &str)
        -> Result<Vec<u8>, BackendError>;
    fn write_file(&self, session: &BackendSession, path: &str, data: &[u8])
        -> Result<(), BackendError>;
    fn terminate(&self, session: &BackendSession, reason: TerminationReason)
        -> Result<(), BackendError>;
    fn reconcile(&self, worker_id: &str) -> Result<ReconcileReport, BackendError>;
}
```

建议实现：

```text
CliContainerBackend       # 兼容现有 7143 Docker/Podman 路径
KubernetesSessionBackend  # 浦江集群首期目标
```

`SweSession` 只依赖 `SweSessionBackend`，不再直接调用 `Command::new(runtime.cli())`。现有 Docker 路径保留为回归实现，便于继续验证 7143，不允许为了 Kubernetes 改坏现网路径。

### 13.4 Kubernetes session 生命周期

一个非 gold SWE-smith episode 的完整 session 生命周期：

```text
1. Server/Agent 请求 instance_id + benchmark_variant=smith
2. Worker pool reserve 一个 slot
3. Worker 校验 catalog row 和 runtime contract
4. Worker 生成 session_id、episode_id、lease_id
5. Worker 创建 swe-session Pod 或 Job
6. Pod 使用已准备的 Smith 镜像 digest 启动
7. Worker watch Pod Ready
8. Worker 执行 reset/provision contract
9. Agent 通过 Runtime Gateway 执行 read/write/exec
10. Worker 收集 model patch / git diff
11. Worker 调用官方 SWE-smith harness 评分
12. Worker seal trajectory 并上传 artifact
13. Gateway 返回 reward/resolved/trajectory_ref
14. Worker destroy Pod/Job 和临时 workspace
15. pool release slot
16. reconcile 确认没有孤儿 Pod、Job、PVC 或 exec
```

任何中间步骤失败都必须进入统一补偿流程：

```text
取消 Agent job
停止 exec
删除 Pod/Job
清理 session workspace
释放 pool reservation
写入失败原因和 cleanup 结果
```

### 13.5 Pod 模型选择

首期推荐“一 episode 一个 Pod”，而不是长期运行一个 Pod 再在其中复用多个工作区：

```text
swe-session-<short-episode-id>
```

原因：

- workspace、环境状态和 patch 隔离更清楚；
- episode 结束后删除边界明确；
- 不会因容器复用导致 instance 串线；
- Pod 资源限制可以直接表达 CPU、memory、ephemeral-storage 和 PID；
- Worker 重启后可以按 label 扫描并 reconcile。

建议首期使用 Kubernetes `Pod` 而不是 `Job` 作为 session 执行载体，Worker 自己控制生命周期；批量预处理或镜像准备才使用 `Job`。如果平台强制要求 Volcano Job 才能获得资源，则封装为单副本 Job，但 backend 对上层仍暴露统一 session handle。

session Pod 必须带以下标签：

```yaml
app.kubernetes.io/part-of: uenv
uenv.io/component: swe-session
uenv.io/benchmark-variant: smith
uenv.io/worker-id: <worker-id>
uenv.io/episode-id: <episode-id>
uenv.io/session-id: <session-id>
uenv.io/lease-id: <lease-id>
```

### 13.6 Session Pod 资源和安全边界

首个 Kubernetes smoke 不申请 NPU：

```yaml
resources:
  requests:
    cpu: "2"
    memory: 4Gi
    ephemeral-storage: 20Gi
  limits:
    cpu: "2"
    memory: 4Gi
    ephemeral-storage: 20Gi
```

实际数值在目标节点采样后调整。Pod 不应默认包含：

```text
huawei.com/Ascend910
/dev/davinci*
CANN runtime
privileged: true
hostNetwork: true
hostPID: true
hostPath: /var/run/docker.sock
hostPath: /run/containerd/containerd.sock
```

SWE-smith 测试环境需要 shell、git、pytest 和官方 harness 依赖，但不需要 NPU。若后续 LLM 推理在 A3 集群内提供 NPU 服务，应单独部署 inference Service，不把 NPU 注入 SWE session。

### 13.7 Worker RBAC

Worker 使用专用 ServiceAccount，不使用集群管理员凭证。首期最小 RBAC 应允许 Worker 在专用业务 namespace 内：

```text
pods: get/list/watch/create/delete/patch
pods/exec: create/get
pods/log: get
jobs: get/list/watch/create/delete/patch（若采用 Job）
services: get/list/watch/create/delete（仅 session service，如需要）
events: get/list/watch
```

不应授予：

```text
nodes 写权限
secrets 全集群读权限
clusterrole/clusterrolebinding 管理权限
任意 namespace 的 Pod 管理权限
Docker/containerd socket 权限
```

Worker 的 session label selector 必须固定，cleanup 只能删除自身 `worker_id` 和 `part-of=uenv` 的资源。

### 13.8 Kubernetes exec 适配要求

Docker backend 的 `docker exec` 需要替换为 Kubernetes remote command exec。实现必须满足：

1. 使用 SPDY/WebSocket exec，而不是在 Worker 容器内 shell 调用 kubectl；
2. stdout、stderr、exit code 分离并映射到现有 `ExecResult`；
3. 每次 exec 绑定 `session_id` 和 `episode_id`；
4. `CommandPolicy` 继续执行 deny pattern、command mode 和输出截断；
5. 强制执行 `timeout_sec`；
6. timeout 时关闭 exec stream，并删除/终止 Pod 内进程；
7. 上游 HTTP 取消或客户端断开时 kill-on-drop；
8. 对 retry/pytest 等长命令增加 harness 级 timeout；
9. 记录 `swe_exec_started`、`swe_exec_finished`、`swe_exec_timeout`、`swe_exec_killed`。

不能只实现“能 exec”而不实现超时和进程回收。7143 已经出现过无限 retry、同步 `.output()` 不强制 timeout、episode 完成后残留容器等问题；Kubernetes 迁移不能复现这些问题，只是把残留物从 container 换成 Pod。

### 13.9 镜像准备与完整 Smith 支持

本任务要求完整环境和 benchmark 支持，不能只准备 gold patch 或单个 gold fixture。

镜像准备必须包含：

```text
Smith catalog 子集或训练集索引
每个 instance 的 image_cache_key/image digest
对应 repo 的完整 SWE-smith 环境镜像
/testbed workspace 语义
environment_setup_commit
problem_statement
patch（bug 注入补丁）
test_patch/FAIL_TO_PASS/PASS_TO_PASS
official swesmith harness 依赖
eval_spec 和 worker overlay
trajectory/artifact 目录
```

镜像策略：

1. 首期验证选择 1-3 个已验证的真实 Python repo instance 做 smoke；全量 222 个镜像同步准备，不等同于立即启动全部实例；
2. 不把 gold patch 作为环境镜像内容；
3. provision 时按 Smith runtime contract 将 dataset patch 应用到 clean image state，构造 buggy state；
4. Agent 在 buggy state 上进行非 gold 修复；
5. 评分时使用官方 `swesmith.harness.eval` 语义；
6. 只有镜像、catalog、digest、harness 和 runtime contract 一致时才扩大子集；
7. 全量 59136 catalog 和 222 镜像可以提前完成归档、Registry push 和 digest manifest；但不能在第一步全部预开 Pod，运行实例仍按并发配额和实际请求创建。

### 13.10 Smith runtime contract 不能错误复用 Pro

必须保持代码中已经冻结的 Smith 语义：

```text
benchmark_variant=smith
workspace=/testbed
patch_semantics=clean_to_buggy
provision_patch=apply_dataset_patch
commit_after_provision=true
reward authority=official_swesmith
fallback=internal_pytest_parser_diagnostic
```

禁止出现以下回归：

```text
把 Smith 当成 benchmark_variant=pro
把 workspace 写死为 /app
把 Smith dataset patch 当成 model patch
在 provision 时直接应用 gold 修复补丁
只调用内部 pytest parser 并声称等同官方 harness
只跑 gold episode 并作为训练 rollout 验收
```

### 13.11 官方 harness 接入

`UENV_SWE_SMITH_EVAL_CMD` 应指向运行在 session Pod 内、或由受控 grader Pod 执行的官方 harness wrapper，例如：

```text
python -m swesmith.harness.eval
```

实际命令必须由当前 Smith 镜像和官方版本确定，不能只按示例字符串硬编码。推荐把命令封装为镜像内固定入口：

```text
/opt/uenv/bin/eval_swesmith_official
```

wrapper 输入：

```text
UENV_SWE_INSTANCE_ID
UENV_SWE_INSTANCE_JSON
UENV_SWE_MODEL_PATCH
```

wrapper 输出单个 JSON：

```json
{
  "resolved": false,
  "reward": 0.25,
  "per_test": [["test_name", true]]
}
```

非 gold 验收必须满足：

```text
model_patch != gold patch
reward > 0
trajectory.steps 非空
resolved 字段来自官方 harness 或明确的官方兼容 wrapper
```

如果当前官方 harness 只返回 resolved 布尔值，则必须明确 reward 映射和 per-test 结果来源，不能把“执行成功”误当成 reward 非零。

### 13.12 模型和 Agent 部署

目标 episode 不能使用 gold shortcut。当前外部拓扑中的模型和 Agent 参考配置是：

```text
OpenHands Agent：8.130.208.77
外部 Agent/Runner：:8777 / :8888
Worker 7143 的 ModelClient：config/uenv-worker-llm.env
7142 可选模型网关：219.147.100.43:18888/v1
```

本次目标全部部署在集群内，因此应将这些角色迁移为集群内服务：

```text
uenv-agent-openhands Deployment
model gateway Deployment/Service
uenv-server/adapter-core Deployment
uenv-worker Deployment
uenv-hub Deployment
```

模型选择应先与原 7142/7143 链路保持兼容：

1. 优先使用与原 `config/uenv-worker-llm.env` 相同的 OpenAI-compatible API 语义；
2. 如果集群已经有 `llm-gateway`，先确认它的服务协议、模型名、鉴权和 OpenAI-compatible `/v1/chat/completions` 兼容性；
3. 如果现有 llm-gateway 只服务平台内部推理，部署独立 UEnv model gateway Service，不直接修改平台系统组件；
4. 首个模型 episode 使用低并发、固定 model endpoint 和完整日志关联；
5. Agent 并发初期固定为 1，Worker K=2 只用于验证环境并行能力，避免模型服务成为未知瓶颈。

模型 Secret、Hub token、UEnv API key 和 Agent key 只放 Kubernetes Secret，不放 ConfigMap、镜像、Git 或日志。

### 13.13 集群内通信拓扑

首期全部模块部署在一个业务 namespace，使用 ClusterIP：

```text
uenv-adapter-core:8088
uenv-hub:8088
uenv-worker-control:28888
uenv-worker-gateway:28097
uenv-agent:8777/8888 或 AgentControl 内部端口
uenv-model-gateway:18888
trajectory-store:8077
```

推荐 UEnv Server/Adapter Core 和 Worker 都在集群内部，避免公网 Server 回连集群 Worker 的问题：

```text
Adapter/VeRL -> adapter-core ClusterIP
adapter-core -> worker-control ClusterIP
agent -> agent-control ClusterIP
agent -> worker-gateway ClusterIP
worker -> hub ClusterIP
worker -> model-gateway ClusterIP
worker -> trajectory-store ClusterIP
```

Runtime Gateway 不应暴露公网。若后续确需外部 Agent 访问，再单独增加受控 TCP/HTTP Ingress，并且每个 Worker 需要唯一可达地址或反向隧道。

### 13.14 代码改造顺序

按以下顺序改造，避免先写 Kubernetes YAML 再发现接口缺口：

```text
1. 抽取 SweSessionBackend trait 和 BackendSession handle
2. 将 SweSession/instance_pool 从 CLI 细节解耦
3. 保留并回归 CliContainerBackend
4. 实现 KubernetesSessionBackend：create/watch/exec/read/write/delete/reconcile
5. 将 Smith runtime contract 和 official harness 接到新 backend
6. 增加 Kubernetes RBAC/ServiceAccount 配置
7. 增加 session Pod 模板和 label/annotation schema
8. 增加 cleanup、timeout、kill-on-drop、reconcile 指标
9. 增加 Worker health/metrics 中的 k8s session 状态
10. 增加本地 fake Kubernetes client 或 mock backend 单测
11. 构建 arm64 Worker 镜像
12. 在集群创建独立业务 namespace 和最小 RBAC
13. 部署单 Worker，不申请 NPU
14. 运行单个 Smith non-gold episode
15. 通过后才扩展 K=2、4 和多 Worker
```

### 13.15 关键代码触点

| 模块 | 需要调整 |
|---|---|
| `uenv-worker/src/swe/session.rs` | 从 CLI container handle 改成 backend handle；保留 Smith reset/provision/evaluate 语义 |
| `uenv-worker/src/swe/instance_pool.rs` | backend 注入、capacity、pending、release/reconcile |
| `uenv-worker/src/swe/image_cache.rs` | 从 Docker inspect/load/pull 抽出 image provider；K8s 使用 image digest/registry pre-pull，不在 episode 内公网 pull |
| `uenv-worker/src/runtime_gateway/mod.rs` | 路由保持兼容，错误和 timeout 映射补齐 |
| `uenv-worker/src/swe/smith_eval.rs` | 官方 harness wrapper、JSON 输出、timeout 和 reward 语义 |
| `uenv-worker/src/swe/runtime_contract.rs` | Smith contract 作为唯一 patch/workspace/reward 权威 |
| `uenv-worker/src/swe/trajectory.rs` | episode/session/lease 关联保持完整 |
| `uenv-worker/src/metrics.rs` | Pod pending/ready/exec/cleanup/orphan 指标 |
| `uenv-worker/src/config` | backend kind、namespace、ServiceAccount、session image、resource profile |
| `uenv-worker/src/control_plane` | Worker capability、gateway endpoint、pool snapshot、capacity |
| `uenv-server`/scheduler | 初期集群内 Service endpoint；后续按 pool/lease 调度 |
| Kubernetes manifests | Namespace、Secret、ConfigMap、RBAC、Deployment、Service、session template |

## 14. Kubernetes-native 验证方案

### 14.1 阶段 0：代码和本地契约验证

不连接集群完成：

```text
Rust 单元测试
backend trait mock 测试
session lifecycle 状态机测试
timeout/kill-on-drop 测试
lease acquire/release/reconcile 测试
Smith runtime contract fixture 测试
official harness wrapper JSON 测试
trajectory schema 校验
```

必须覆盖：

```text
创建失败 -> reservation release
Pod Pending 超时 -> delete + release
Pod Ready 但 reset 失败 -> delete + release
exec timeout -> kill + delete/reuse policy
Agent 取消 -> session cleanup
Worker 重启 -> orphan Pod reconcile
Server 断开 -> lease/session release
```

### 14.2 阶段 1：集群 CPU-only 探针

只创建一个低资源 Pod，验证：

```text
Pod 启动和镜像拉取
ServiceAccount token
in-cluster API 访问
Worker ServiceAccount 能否创建/查询/删除 session Pod
PVC RWX 挂载
Pod 到集群内 Service 的 DNS
Pod 到 model/Hub/trajectory Service 的 HTTP
Pod 的 CPU/memory/ephemeral-storage/pids 限制
```

不创建 SWE session，不申请 NPU，不触碰现有 `uenv-public-verl-128-copy`。

### 14.3 阶段 2：单镜像、单 Pod、非 Agent smoke

选择一个真实 Smith instance，必须包含：

```text
真实 catalog row
真实完整 environment image
真实 problem_statement
真实 dataset patch
真实 FAIL_TO_PASS
官方 harness 依赖
```

验证：

```text
Pod Ready
/testbed 存在
repo 和 base commit 正确
provision 后工作区是 buggy state
不注入 gold 修复
exec/read/write 可用
official harness 可运行
cleanup 成功
```

这一阶段可以使用确定的手工 model patch 做非 gold grader smoke，但不能把它作为最终 Agent rollout 验收。

### 14.4 阶段 3：单 Agent 非 gold episode

最终单 episode 必须经过真实 Agent/模型：

```text
Server/Adapter 提交 env_type=swe
benchmark_variant=smith
execution_mode=agent
instance_id=<真实 Smith instance>
```

Agent 执行：

```text
读取 issue
检查仓库
修改源码/测试允许的目标文件
运行受 timeout 保护的测试
生成 model patch
submit
```

硬性通过条件：

```text
不是 gold shortcut
model patch 非空且不是 gold patch
status=completed
reward > 0
resolved/per-test 结果可追溯
trajectory.steps 非空
trajectory_id 可回取
artifact/patch 可回取
session Pod 已清理
无孤儿 lease/exec/Pod
```

若 Agent 生成了 reward=0，不能只修改 reward 映射来“通过”。应保留原始 trajectory，诊断：

```text
模型 endpoint/模型名
prompt 与 Smith issue
工作目录
patch 方向
测试命令
官方 harness 输入
测试超时
```

### 14.5 阶段 4：双并发和回收

在单 episode 通过后：

```text
Worker K=2
两个不同 Smith instance
两个 Agent job 或一批中两个 episode
```

验收：

```text
2 个 session Pod 同时 Running
workspace 不串线
catalog row 不串线
trajectory 不串线
一个 episode 失败不影响另一个
两个 Pod 最终均清理
pool busy 回到 0
```

### 14.6 阶段 5：扩容验收

扩容顺序：

```text
K=2 -> K=4 -> 2-4 Worker -> 8/16/32 session
```

每级至少观察一个完整长稳窗口，记录：

```text
Pod pending/starting/ready 时延
session provision p50/p95
exec p50/p95/p99
reward/resolved 比例
CPU/memory/storage/pids/fd
API server QPS 和错误
镜像拉取/cache hit
orphan Pod/Job 数
cleanup 延迟
Agent/model QPS 和错误
```

没有通过 cleanup 和 timeout 回归时，禁止升 K。

## 15. 历史 SWE-smith 踩坑清单（本方案硬性继承）

以下问题来自 `Docs/worker` 既有记录，作为本次 Kubernetes backend 的阻断项，而不是上线后再补的优化项：

1. **catalog 不完整**：历史上只加载 5 条 Smith smoke，训练请求报 `instance_id not in catalog`。本次必须先确定训练子集 catalog 与镜像集合的一致性，不能只部署一个 gold fixture。
2. **全量 catalog 规模**：已记录 Smith catalog 约 59136 条、222 个 unique images、catalog 约 4.8GiB。可以提前准备全量镜像和 digest manifest，但不能让每个 session Pod 重复加载全量 catalog，也不能启动时全量预开容器。
3. **镜像命名空间不一致**：历史上 Worker 使用官方 `image_cache_key` 查找镜像，但本地 tag 命名空间不一致，导致 instance 找到却无法 provision。必须以 digest/规范化 image ref 建立 manifest，部署前逐条校验。
4. **Pro/Smith 语义混用**：Smith 必须保持 `benchmark_variant=smith`，不能归一成 Pro；grader、catalog、workspace、patch 方向必须分桶。
5. **工作目录错误**：Pro 使用 `/app`，Smith 使用 `/testbed`。Smith driver、Runtime Gateway、harness、prompt 均不能硬编码 Pro 工作目录。
6. **patch 方向错误**：Smith image state 是 clean，provision 阶段应用 dataset patch 构造 buggy state；gold 评测使用 reverse dataset patch；model patch 不能混入 dataset/gold patch。
7. **官方 harness 缺失**：Rust 内部 pytest parser 只能作为诊断 fallback；最终 Smith reward 必须经过官方 `swesmith.harness.eval` 语义或固定 wrapper。
8. **只跑 gold**：Gold 只能证明环境和 grader 基础链路可用，不能作为训练 rollout 验收。本次最终验收必须是非 gold、reward 非 0、轨迹有效。
9. **exec 无超时**：7143 曾出现 retry pytest 无限运行。Kubernetes exec 必须强制 timeout、kill-on-drop 和进程回收。
10. **容器/Pod 泄漏**：历史上 submit 后容器未可靠 destroy。Kubernetes 版本必须有 delete、reconcile、orphan 指标和 Worker 重启清理。
11. **并发配置不一致**：`worker.max_concurrent`、Gateway capacity、Agent capacity、pool capacity 必须统一由 K 和 run scheduling policy 约束。
12. **外部回连缺失**：如果 Server/Agent 保留在集群外，必须先解决 Worker/Gateway 入站地址；本次目标优先全部集群内通信，不依赖公网回连。
13. **轨迹隔离不足**：每条 trajectory/artifact 必须绑定 episode_id、session_id、instance_id、benchmark_variant、run_id 和 lease_id。
14. **镜像拉取风暴**：训练子集镜像必须预拉取或分批准备，episode 热路径默认 `local_only`，禁止 128 个 session 同时公网 pull。

## 16. 设计审查结果与补充边界

本轮对全文规划进行了概念审查，确认下一步实施方案和目标分层如下：

```text
下一步 backend：方案 A，Kubernetes-native SWE backend
短期：1 个真实 Smith instance + 1 个 CPU-only session + 1 条非 gold reward>0 rollout
长期：多 Worker + 多 CPU-only session + 按集群资源上限扩展并行训练
```

以下概念已经统一：

1. `huawei.com/Ascend910` 属于模型/训练资源，不属于 SWE session 资源。
2. SWE session 的下层载体是 CPU-only Pod/Job，上层仍是 `SweSession` 和 Runtime Gateway。
3. 训练侧的 128 张 NPU 不等于 128 个 SWE session，也不代表 128 个 CPU-only session 已经可用。
4. Gold 只用于环境、harness 和 reset 的辅助 smoke，不用于最终 rollout 验收。
5. Smith 的 `/testbed`、`clean_to_buggy`、dataset patch、reverse gold patch 和官方 harness 是不可替换的运行契约。
6. Worker、Server、Agent、Model Gateway、Hub 和 artifact 服务首期都在集群内通过 ClusterIP 通信，不依赖外部回连。

以下事实仍必须在实现前通过探针或平台确认：

```text
业务 namespace 和配额
Worker ServiceAccount 是否能管理 session Pod
CPU-only Pod 的 CPU/memory/ephemeral-storage/pids 上限
RWX PVC 的实际读写性能
Smith 镜像是否可由集群内部 registry 拉取
镜像与 catalog image digest 是否一一对应
集群内 model gateway 的 OpenAI-compatible API、模型名和鉴权
官方 swesmith harness 的镜像版本、命令和依赖
Kubernetes exec client library/API 兼容性
session Pod 是否需要 Volcano Job 才能稳定调度
Pod cleanup、watch、ServiceAccount token 和 DNS 行为
```

在短期单实例验收通过前，不应：

```text
申请或绑定 NPU 给 SWE session
创建 128 个 session Pod
把全量 catalog 注入每个 Pod
全量预开 Smith 容器
把外部 7143 Docker Worker 配置原样复制到集群
把 gold patch 当作训练 rollout
用内部 pytest fallback 替代官方 harness
开放 Worker 的 cluster-admin 权限
挂载 Docker/containerd socket
```

当前文档同时包含探活事实、容量规划和实现设计。后续增量记录应按以下顺序追加，避免把“已验证事实”和“待实现计划”混淆：

```text
1. 集群事实和探活结果
2. 短期单实例实施记录
3. Kubernetes backend 代码变更记录
4. 集群内模块部署记录
5. 非 gold episode 验收记录
6. 长期并发压测和资源画像
```

本节确认：方案 A 是已批准的下一步实施方案。Docker-in-Pod 仅作为现有 7143 backend 的兼容路径或备用路径，不再作为浦江集群的并行主线。

## 17. 最终验收定义

本方案的最终验收不是“Pod Running”，而是以下完整条件同时成立：

```text
1. 集群内 uenv-server/adapter-core、uenv-hub、uenv-worker、Agent、model gateway 可通过 ClusterIP 通信
2. Worker 使用 Kubernetes API 创建和销毁 SWE session Pod
3. SWE session 明确为 CPU-only，不申请 `huawei.com/Ascend910`
4. Smith 完整 catalog row 和对应 image digest 可解析
5. Smith image 的 /testbed、依赖和官方 harness 正常
6. 一个真实非 gold Agent episode 完整执行
7. model patch 非 gold 且有效
8. 官方 Smith harness 返回 reward > 0
9. trajectory.steps 非空且 schema 合法
10. trajectory/artifact/patch 可通过 ref 回取
11. Worker/Server/Agent 状态为 completed
12. session Pod、exec、lease、workspace 没有泄漏
13. 资源指标和日志能按 episode_id 追踪
```

完成上述短期单 episode 验收后，才允许进入长期并发阶段，将 K 从 2 提高到 4，并进一步评估 16、32、64、128 个 CPU-only 并发 session。

## 18. 当前推进状态（2026-08-19）

### 18.1 已完成

```text
本机 SSH 已通过浦江 SOCKS5 代理连接 h.pjlab.org.cn 成功
本机 ed25519 私钥和公钥指纹已确认
Worker Kubernetes-native backend 初版代码已接入并通过本地 cargo check
Worker 单元测试 160 passed
linux/arm64 Worker 镜像已在本机构建成功
本地镜像 manifest：sha256:5635a68a555c8ef2e60f21eddd97a2b02bd8d18ba3d5d440929ef77cba023035
```

### 18.2 尚未完成

```text
SSH 镜像上传通道已验证；ARM64 Worker 镜像归档已通过 scp 上传到 `/mnt/shared-storage-user/evobox-share/uenv`
归档文件：`uenv-worker-arm64-dev.tar`，大小 `5270912512` bytes，SHA-256：`954e6baf8d3a78ad86320901d73ce2909ff54500476fff1a2ab6795fa55463d3`
目标共享存储当前容量约 14T、已使用约 12T、可用约 2.9T（80%）；目录当前归属 `fangtianshun:fangtianshun`，权限为 `drwxr-xr-x`
远端 SSH 工作环境未提供 `ctr`、`nerdctl`、`docker` 或 `crictl`；其 PID 1 为平台 `kubebrain/worker-init`，属于普通工作空间容器，不是 Kubernetes 节点运行时环境，不能直接导入节点 containerd
远端工作空间内可见 `/mnt/shared-storage-user/evobox-share`，但没有 kubeconfig、containerd socket 或节点级运行时权限
已发现平台提供 `/kubebrain/brainctl`，支持 Kubernetes 资源操作和 `launch --image`；当前 SSH 工作空间凭证只能访问 `ailab-sys` 上下文且无 Pod/Namespace 列表权限，尚未证明可用其导入本地 tar 镜像
尚未部署 Worker Deployment/Service/RBAC 到业务 namespace
尚未创建真实 session Pod 并验证 Pod/exec/read/write/delete lifecycle
尚未完成 Smith 镜像和真实 catalog row 部署
尚未运行 Pod 内 official SWE-smith harness
尚未运行真实 Agent non-gold episode
短期 reward > 0、trajectory 有效和完整 cleanup 尚未验收
```

### 18.3 当前资源与权限注意事项

当前 kubeconfig 访问仍应使用显式公网配置：

```bash
KUBECONFIG="$HOME/.kube/pjlab-public-config" kubectl <command>
```

本轮读取时集群返回当前用户无权列出全局 namespaces；这不等于 namespace 内资源权限已验证。
后续必须使用最终业务 namespace 和 Worker ServiceAccount 实测 Pod create/get/watch/exec/delete，
不能用管理员或本机 kubeconfig 的全局读取能力替代 Worker SA 门禁。

当前不得：

```text
向 default namespace 提交 SWE 业务
触碰 uenv-public-verl-128-copy、uenv-workspace-shell 或既有训练 PVC
把本地 Docker image tag 当作集群已可拉取镜像
在未完成集群侧镜像导入/加载验证前部署 Worker
```

### 18.4 下一步顺序

```text
1. 向平台确认 `/kubebrain/brainctl launch --image` 支持的镜像来源，以及共享路径 tar 到可拉取镜像的导入流程；
2. 优先申请或确认 `registry2.d.pjlab.org.cn` 的推送凭证/项目权限；本地直接 `docker push` 当前仍返回 `401 Unauthorized`，说明 Registry 认证尚未打通；
3. 若平台提供节点侧导入任务，使用已上传归档导入并核对 ARM64 image digest；禁止在普通 SSH 工作空间内伪造节点 containerd 导入；
4. 确认业务 namespace 和 imagePullSecret，或确认导入后的镜像引用方式；
5. 使用 Worker SA 完成 §11.1 探针门禁；
6. 部署单 Worker/Gateway 并验证 session Pod lifecycle；
7. 部署一个真实 Smith image/catalog row；
8. 验证 Pod 内 official harness；
9. 完成真实 non-gold Agent episode 后，才判定短期目标完成。
```
