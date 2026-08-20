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

SWE-smith 测试容器本身不需要 Ascend NPU。SWE session 的主要资源是 CPU、内存、可写存储、进程数、文件描述符、容器运行时和网络。因此有两种可能：

1. **训练和 SWE Worker 共用这批节点，但 Worker 不申请 NPU**。这种方式可以使用节点剩余 CPU/内存，但必须确认 VC/队列允许 CPU-only Pod 与当前 NPU Job 共存。
2. **释放或拆分部分 NPU 训练资源给独立推理/Agent 服务**。这种方式适合把 LLM 推理与 SWE session 分开，但需要重新安排 Volcano Job 和训练侧资源。

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

在当前信息下，推荐的第一版目标是：

```text
先部署 1 个 Worker，K=2
通过后扩到 4 个 Worker × K=4 = 16 个并发 session
再根据资源采样扩到 32-64 个并发 session
```

不建议当前直接配置 128 个 SWE session。128 是后续容量目标，不是首个 admission 值。若现有 128 卡 Job 继续占用节点，第一版更应按 **16-32 个并发 session** 起步；如果 Worker 使用独立 CPU 资源池且节点余量充分，再考虑 64-128。

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

只准备 1-3 个真实 Smith instance 和对应镜像，验证：

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
