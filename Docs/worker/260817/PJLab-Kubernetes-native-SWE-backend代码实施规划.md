# 浦江集群 Kubernetes-native SWE backend 代码实施规划

> 上层指导文档：[PJLab-Kubernetes-公网接入与集群探活记录.md](./PJLab-Kubernetes-公网接入与集群探活记录.md)
>
> 本文只记录代码改造和代码验证计划；集群事实、CPU-only 约束、短期/长期目标、模块拓扑、资源上限和最终验收定义以上层文档为准。
>
> 当前决策：**下一步采用方案 A：Kubernetes-native SWE backend**。本文是已确认方案的实施拆解，不代表代码或集群部署已经完成。
>
> 2026-08-17 审查补齐：对照母文档探活事实与当前 `uenv-worker` 代码，已补入架构/Volcano/namespace 隔离、`smith_eval` 落点、既有 `crate::backend` 边界、sync/async 桥接、容量对齐与探针门禁等缺口（见 §14）。

## 1. 目标与边界

### 1.1 短期交付目标

在不破坏现有 Docker/Podman backend 的前提下，完成一个 Kubernetes backend，使集群内的 UEnv Worker 能够：

```text
创建 1 个真实 SWE-smith instance 的 CPU-only session Pod
完成 clean-to-buggy provision
通过 Runtime Gateway 提供 exec/read/write/submit
使用集群内 Agent 和模型完成非 gold 修复
调用官方 SWE-smith harness，得到 reward > 0
写出有效 trajectory/rollout/artifact
完整删除 session Pod、exec 和 workspace
```

短期不做：

```text
不申请 Ascend910 给 SWE session
不实现多 Worker 自动扩缩容
不把全量 59136 catalog 注入每个 Pod
不全量预开 Smith 容器
不依赖 Docker socket、DinD 或 containerd socket
不使用 gold shortcut 作为最终验收
```

### 1.2 长期交付目标

短期单实例有效链路完成后，逐步支持：

```text
多 Worker、多 CPU-only session Pod 并行
run-level max_episode_concurrency
Worker/Gateway/Agent lease 对齐
镜像预取、catalog 分片和按需准备
跨 repo 长稳压测
trajectory/artifact 批量上传
资源画像驱动的 16/32/64/128 session 扩展
```

长期容量受 CPU、内存、存储、PID、FD、Pod 调度、Agent 和模型服务限制，不由 NPU 数量直接决定。具体目标以[上层指导文档](./PJLab-Kubernetes-公网接入与集群探活记录.md)第 8 节为准。

### 1.3 保持不变的上层契约

以下调用方不应感知 Kubernetes 资源细节：

```text
uenv-server / adapter-core
OpenHands Agent
Runtime Gateway client
native DispatchEpisode
VeRL AgentLoop
```

以下语义必须保持兼容：

```text
env_type=swe
benchmark_variant=smith
session create/exec/read/write/submit/delete
ResetObservation
EpisodeOutcome
reward/resolved
TrajectoryRef / artifact URI
episode_id / run_id / instance_id 关联
```

Pod name、namespace、Service、PVC、Kubernetes UID 只能存在于 Worker backend 内部和运维观测中，不得泄漏为新的上层协议字段。

## 2. 当前实现审计

### 2.1 现有代码职责

当前 Worker SWE 代码的主要触点：

| 文件 | 当前职责 | 改造方向 |
|---|---|---|
| `uenv-worker/src/swe/session.rs` | provision、reset、exec、read、write、submit、terminate；直接持有 container 字符串；**同步 API** | 改为持有抽象 backend session handle；保留同步对外表面或经 runtime 桥接 async backend |
| `uenv-worker/src/swe/instance_pool.rs` | capacity、pending reservation、session map、create/destroy | 注入 backend，保留 pool 语义并增加 reconcile |
| `uenv-worker/src/swe/runtime_contract.rs` | workspace、patch、gold、reward contract | 不改 Smith 语义，作为 backend 生命周期输入 |
| `uenv-worker/src/runtime_gateway/mod.rs` | HTTP API 和 API key（**不在 `swe/` 下**） | 路由保持不变，调用抽象 session；补 K8s 错误映射 |
| `uenv-worker/src/swe/image_cache.rs` | Docker/Podman inspect、load tar、pull | 抽取 image provider；Kubernetes 使用 registry digest/pre-pull |
| `uenv-worker/src/swe/resettable.rs` | Podman resettable instance；依赖 `crate::backend::PodmanBackend` | 将命令执行委托给 session backend；与沙箱 `SandboxProvisioner` 解耦 |
| `uenv-worker/src/swe/harness.rs` | 一次性容器 harness；`ContainerRuntime` 枚举 | 保留 Docker 回归；native/Gateway 共用 backend 注入路径 |
| `uenv-worker/src/episode/executor.rs` | native `DispatchEpisode(env_type=swe)` 仍直接构造 `ContainerRuntime` | 与 Gateway 共用同一 `SweInstancePool` + backend，禁止再开一条 CLI-only 路径 |
| `uenv-worker/src/swe/smith_eval.rs` | **在 Worker 进程本机** `sh -c` + 临时目录执行 `UENV_SWE_SMITH_EVAL_CMD` | 改为经 session backend.exec 在 session/grader Pod 内执行；补 timeout |
| `uenv-worker/src/swe/command_policy.rs` | Docker `cap_drop` / `network=none` / 可选 hostPath seccomp | 映射为 Pod `securityContext`；禁止依赖宿主机 seccomp 文件路径 |
| `uenv-worker/src/swe/trajectory.rs` / `trajectory_upload.rs` | trajectory bundle、可选 HTTP 上传 | 保持 schema；endpoint 改为集群内 Service DNS |
| `uenv-worker/src/control_plane/client.rs` | 注册 `advertise_endpoint`、`gateway_public_url`、`max_concurrent`、`ResourceSpec` | ClusterIP DNS 作为 advertise；gateway URL 用内部 Service；GPU 字段对 SWE Worker 置 0 |
| `uenv-worker/src/config` | 已有 `SweSection`（variants/env_package_dir），**尚无** `swe.backend=kubernetes` | 扩展 kubernetes 子配置与容量对齐校验 |
| `uenv-worker/src/metrics.rs` | Worker/SWE 指标 | 增加 Kubernetes API、Pod 生命周期、orphan 指标 |
| `uenv-worker/src/backend/` | 既有 **沙箱** `Backend` / `SandboxProvisioner`（Process/Podman） | **不得**与 `SweSessionBackend` 混名或互相替换；见 §2.3 |

### 2.2 现有强耦合点

代码审计显示需要优先处理的强耦合包括：

1. `SweSession::provision` 当前调用 `ImageCacheFactory`，随后通过 `Command::new(runtime.cli())` 执行容器启动。
2. `SweSession::terminate` 当前直接执行 `runtime.cli() rm -f`。
3. `SweSession` 的 `container: String` 同时承担容器标识、exec 目标和日志字段。
4. `PodmanResettableInstance` 直接调用 `podman exec` / `podman rm`，并绑定 `crate::backend::PodmanBackend`。
5. `ImageCacheFactory` 假设镜像存在于 Worker 本地 Docker/Podman image store。
6. `smith_eval::try_external_smith_grade*` **在 Worker 宿主机进程**写临时文件并 `Command::output()`，**无强制 timeout**；Kubernetes 下若仍在 Worker 本机执行，会找不到 `/testbed` 且违反隔离。
7. Runtime Gateway 当前正确地持有 `Arc<SweInstancePool>`，应继续保持 Gateway 不直接依赖 Kubernetes client。
8. `episode/executor.rs` 与 harness 仍按 `ContainerRuntime` 分支，未注入 backend trait。
9. `env.backend`（process/podman）与未来的 `swe.backend`（cli_container/kubernetes）是两套配置轴，配置解析必须显式区分，避免 `backend=kubernetes` 被误读成插件沙箱 backend。

### 2.3 与既有 `crate::backend` 的边界（补齐）

仓库里已存在通用沙箱抽象：

```text
uenv-worker/src/backend/
  BackendKind / BackendHandle / SandboxSpec / SandboxProvisioner
  ProcessBackend / PodmanBackend
```

本次 Kubernetes 改造新增的是 **SWE session 运行时**抽象，建议命名与目录固定为：

```text
uenv-worker/src/swe/backend/     # SweSessionBackend（本文主体）
```

约束：

```text
不要把 KubernetesSessionBackend 塞进 crate::backend::AnyBackend
不要让 SandboxProvisioner::create 去创建 Kubernetes Pod
不要为了“统一 backend”同时大改 process/math/code 插件路径
SweSessionBackend 只服务 swe session / Gateway / native SWE episode
```

首期允许 `CliContainerBackend` 内部继续调用现有 Docker/Podman CLI 或 `PodmanBackend`；Kubernetes 实现只依赖 kube API。

### 2.4 当前代码对规划的反向约束（本轮复核补齐）

再次对照当前实现确认，以下必须作为代码实施约束，而不是仅作为部署备注：

```text
1. runtime.rs 当前用 gateway_capacity.max(max_concurrent) 创建 SweInstancePool；Kubernetes
   路径必须改为使用同一个经校验的 effective K，不能用 max() 绕过 admission。
2. gateway_public_url 当前由 gateway listen 地址推导，不能从 0.0.0.0:port 得到可达地址；
   必须新增显式 advertise/public URL，并使用 ClusterIP Service DNS。
3. detect_resource_spec() 当前允许 UENV_WORKER_GPU_COUNT 覆盖默认值；SWE Kubernetes
   模式必须强制 gpu_count=0，或发现非零值直接拒绝启动。
4. SweSection 当前尚无 backend/kubernetes 子配置；规划字段必须真正贯通
   WorkerConfig -> WorkerRuntime -> backend 工厂，不能只增加 YAML 示例。
5. episode executor 虽在 pool 存在时复用共享池，仍保留无 pool 的 run_instance fallback；
   生产 Kubernetes 模式必须禁止该 fallback，仅允许测试/CLI 兼容路径。
6. runtime_gateway、trajectory_upload 已是现有顶层模块和配置轴；Kubernetes 改造应复用
   现有路由、鉴权、spool 和 trajectory schema，不复制第二套协议。
```

验收必须覆盖：pool capacity 不超过 Worker capacity、注册 endpoint 与 Service DNS 一致、
SWE 注册 `gpu_count=0`，以及 native/Gateway 两条路径都经过同一个 backend 实例。

## 3. 目标代码结构

建议新增目录：

```text
uenv-worker/src/swe/backend/
├── mod.rs                  # backend trait、handle、错误和工厂
├── cli_container.rs        # 现有 Docker/Podman 兼容实现
├── kubernetes.rs           # Kubernetes API 实现
├── kube_exec.rs            # Pod exec stream、超时、输出和退出码
├── kube_session.rs         # Pod/Job metadata、状态和生命周期
├── reconcile.rs            # label 扫描、孤儿 Pod/Job 清理
└── image_provider.rs       # registry digest、预取状态和镜像校验
```

如果项目当前模块布局不适合新增目录，也可以将上述职责拆到 `swe/backend.rs` 和相邻模块，但必须保留相同的逻辑边界。

### 3.1 Backend trait

建议接口如下，实际 Rust 签名应结合现有同步/异步调用链调整：

```rust
#[async_trait]
pub trait SweSessionBackend: Send + Sync {
    async fn provision(&self, spec: ProvisionSpec) -> Result<BackendSession, BackendError>;

    async fn health_check(
        &self,
        session: &BackendSession,
    ) -> Result<(), BackendError>;

    async fn exec(
        &self,
        session: &BackendSession,
        command: &str,
        timeout: Duration,
    ) -> Result<ExecResult, BackendError>;

    async fn read_file(
        &self,
        session: &BackendSession,
        path: &str,
    ) -> Result<Vec<u8>, BackendError>;

    async fn write_file(
        &self,
        session: &BackendSession,
        path: &str,
        data: &[u8],
    ) -> Result<(), BackendError>;

    async fn terminate(
        &self,
        session: &BackendSession,
        reason: TerminationReason,
    ) -> Result<(), BackendError>;

    async fn reconcile(&self, worker_id: &str) -> Result<ReconcileReport, BackendError>;
}
```

Backend 需要区分：

```text
provision failed
pod pending timeout
pod image pull failed
pod readiness failed
exec timeout
exec canceled
termination failed
reconcile found orphan
```

不能将所有 Kubernetes 错误折叠为普通 `session failed`，否则 Server/Agent 无法区分可重试、资源不足和环境损坏。

### 3.2 BackendSession

`BackendSession` 不能只保存 Pod name，应保存可观测和清理所需的不可变坐标：

```rust
pub struct BackendSession {
    pub session_id: String,
    pub episode_id: String,
    pub lease_id: Option<String>,
    pub worker_id: String,
    pub namespace: String,
    pub pod_name: String,
    pub job_name: Option<String>,
    pub container_name: String,
    pub image_ref: String,
    pub image_digest: Option<String>,
    pub workspace_dir: String,
    pub created_at: Instant,
}
```

所有 Kubernetes 对象的 name 生成必须经过长度限制和字符清理；原始 `instance_id` 不能直接作为 Pod name。完整 instance_id 放 label value 前也要做长度限制，超长时保存 hash，并在 annotation 或 Worker 内部 map 中保留完整值。

## 4. Kubernetes client 与依赖

### 4.1 Rust client

优先使用 Kubernetes Rust client，而不是在 Worker 容器内 shell 调 `kubectl`：

```text
kube client
kube runtime watcher
k8s-openapi 对应 Kubernetes API 版本
```

**API 版本对齐（母文档探活事实）**：

```text
集群 API Server：v1.31.3+k3s
节点侧可见版本：v1.28.15（勿与 Server 混用）
本机 kubectl：v1.34.1（仅运维探活；开发机可再装 v1.31.x）
```

`k8s-openapi` / `kube` feature 应按 **Server 1.31** 选型（例如 `v1_31`），不要按本机 kubectl 1.34 选型。遇 API 兼容问题时，以 Server 版本为准回退。

client 应支持 in-cluster 配置：

```text
/var/run/secrets/kubernetes.io/serviceaccount/token
KUBERNETES_SERVICE_HOST
KUBERNETES_SERVICE_PORT
namespace 文件
```

本地单测和开发环境支持：

```text
KUBECONFIG 或显式 kubeconfig
KUBE_API_SERVER
KUBE_CA_CERT
```

生产 Worker 不应把本机公网 kubeconfig（`~/.kube/pjlab-public-config`）打进镜像或挂载到 Pod。集群内使用 ServiceAccount 和 RBAC。本机探活用的公网 API/`HTTP_PROXY` 仅用于运维 kubectl，不得成为 Worker 运行时配置。

### 4.2 Cargo 变更

在 `uenv-worker/Cargo.toml` 中增加经过项目 MSRV/版本约束验证的 Kubernetes client 依赖，至少覆盖：

```text
kube client/api（对齐 1.31）
k8s-openapi Pod/Job/Service/Event/Volcano 相关（若启用 Job 路径）
websocket/SPDY exec 所需 transport
```

当前 Worker 已依赖 `tokio`、`axum 0.8`、`tonic 0.14`、`hyper-util`、`reqwest`。依赖版本必须与之兼容。添加依赖后执行：

```bash
cargo check -p uenv-worker
cargo test -p uenv-worker
```

不要在首个改造中同时升级 Tokio、Axum、Tonic 或现有 gRPC 依赖，避免把 backend 改造和全仓依赖升级混在一起。

### 4.3 同步 SweSession 与异步 kube client 桥接（补齐）

现状：`SweSession` / `SweInstancePool` / `smith_eval` / 大量 Gateway 调用路径是 **同步** 的；`kube` client 与 watch/exec 是 **异步** 的。

首期允许的桥接策略（二选一，实施时写死一种，勿混用）：

```text
策略 S1（推荐首期）：
  KubernetesSessionBackend 对外提供 sync 包装
  内部在已有 Tokio runtime 上使用边界清晰的 runtime handle
  禁止在持有标准库 Mutex 时 block_on，避免与 Gateway/async 控制面死锁

策略 S2：
  将 SweSession 关键路径逐步 async 化
  工作量大，不与首个 Kubernetes lifecycle PR 捆绑
```

验收：同一进程内 Runtime Gateway（axum）与 backend.provision/exec/terminate 不得互相饿死或双重 runtime panic。

## 5. Session Pod 规格

### 5.1 固定 CPU-only 资源

短期 smoke 的默认 Pod 规格：

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

母文档强调 PID/FD 也是容量瓶颈。首期在容器 runtime 允许时增加 pids 限制（配置项 `session_pids_limit`），并在节点探针中核实测值。无法设置时必须记入风险清单，靠 exec/harness timeout 兜底。

禁止向 session Pod 注入：

```yaml
huawei.com/Ascend910
privileged: true
hostNetwork: true
hostPID: true
hostPath: /var/run/docker.sock
hostPath: /run/containerd/containerd.sock
hostPath: 任意 seccomp 宿主机路径
```

长远资源值只能根据目标节点实测调整，不能直接套用 7143 的空闲 RSS，也不能按现有 `uenv-public-verl-128-copy`（每 Pod 256 CPU / 1920Gi / 16 Ascend）去推 SWE session。

### 5.2 Pod command 和工作目录

Pod 必须提供稳定的 shell 入口，推荐：

```text
command: ["bash", "-lc"]
args: ["sleep infinity"]
workingDir: /testbed
```

实际镜像若没有 `bash`，必须在镜像 smoke 阶段显式确认 shell、git、conda/testbed 环境和官方 harness 入口，不允许在 backend 中隐式假设 Pro 镜像布局。

Smith 固定使用：

```text
workspace=/testbed
```

不能使用 `/app`。

### 5.3 Workspace 与存储

短期优先使用 session Pod 的 `emptyDir` 作为工作区，并将大文件和最终 artifact 放到独立 PVC/对象存储：

```text
/testbed -> emptyDir 或 session 专属 PVC
catalog  -> 只读 PVC/ConfigMap；由 Worker 加载，不注入每个 session Pod
artifact -> trajectory/artifact ClusterIP Service
```

如果 Smith 镜像已经包含完整 repo 和构建产物，`emptyDir` 可避免每个 session 分配独立大 PVC。只有需要跨重启保留的中间产物才使用 PVC。

存储探活事实（母文档）：

```text
StorageClass 集群级 list 权限被拒绝
现有训练 PVC 可见 StorageClass：quark-vcproxy-sc
训练任务 PVC 容量极大（510Ti 级），SWE 不得复用该 PVC
```

因此代码与 manifest 必须：

```text
显式配置已知可用的 StorageClass 名称（若需要 PVC）
禁止 list 全集群 StorageClass 作为运行时依赖
禁止挂载或清理 default 中既有训练 PVC / 他人 PVC
Worker 与 session 的 PVC 使用独立 claim，带 uenv label
```

### 5.4 镜像 CPU 架构与调度（补齐）

探活事实：节点几乎全是 **ARM64**（522/524），仅 2 个 AMD64。

```text
Worker 镜像：必须构建 linux/arm64
Smith session 镜像：部署前必须确认 architecture（arm64 / amd64 / multi-arch）
```

若 Smith 环境镜像仅为 `amd64`：

```text
不得静默调度到 ARM 节点后 ImagePullBackOff
可选路径（按优先级）：
  1. 使用已验证的 arm64/multi-arch Smith 镜像
  2. 平台明确提供的 amd64 节点 + nodeSelector/affinity（短期可接受，需记录配额）
  3. 平台允许的模拟层（一般不推荐，首期验收避免）
```

`ProvisionSpec` / image provider 必须记录并校验：

```text
image digest
os/arch（或明确的 multi-arch index digest）
与目标节点架构匹配结果
```

架构不匹配视为 **provision failed（不可重试到错误架构）**，不得当成普通 pending timeout。

### 5.5 SecurityContext 与 CommandPolicy 映射（补齐）

Docker 路径当前用 `CommandPolicy` 注入 `cap_drop`、`network=none|bridge`、可选 hostPath seccomp。Kubernetes 首期映射：

```text
RestrictedShell ->
  allowPrivilegeEscalation: false
  capabilities.drop: ["ALL"] 或与现网等价集合
  runAsNonRoot: 若镜像允许
  不挂载 Docker socket / 不 privileged

FullShell ->
  仍禁止 privileged / hostNetwork / hostPID / hostPath runtime socket
  仅放宽能力与网络策略到与现网 FullShell 等价的最小集
```

```text
不得把 sandbox_profiles/*.json 以 hostPath 挂进 session Pod
若需要 seccomp，使用集群支持的 seccompProfile.type（RuntimeDefault/Localhost）并预先验证 Smith 测试 syscall
首期若平台 seccomp 与 SWE 不兼容，可先 RuntimeDefault + CommandPolicy deny pattern，但必须在风险清单记录
```

## 6. Provision 和 Smith 语义

### 6.1 ProvisionSpec 来源

`ProvisionSpec` 必须从既有 `SweInstance` 和 `BenchmarkRuntimeContract` 生成，至少包含：

```text
instance_id
benchmark_variant=smith
image_ref
expected_image_digest
workspace_dir=/testbed
base_commit
environment_setup_commit
dataset patch
patch semantics
command policy
episode_id
lease_id
run_id
```

不允许由 Kubernetes backend 自行决定 patch 方向、workspace 或 grader。

### 6.2 Smith 初始状态

代码中 Smith contract 已定义为：

```text
patch_semantics=clean_to_buggy
provision_patch=apply_dataset_patch
commit_after_provision=true
reward authority=official_swesmith
```

backend 负责提供一个干净、可执行、可访问的 `/testbed`，然后交给现有 runtime contract 流程完成 Smith 初始化。不能在 Pod 创建阶段直接应用 gold patch。

### 6.3 镜像策略

Kubernetes backend 不在 episode 热路径调用 Docker inspect/load/pull。镜像状态拆成：

```text
registry image available
imagePullSecret valid
Pod image pull succeeded
image digest matches catalog/EnvPackage
```

短期允许使用 `imagePullPolicy: IfNotPresent`，但必须记录实际 imageID/digest。长期推荐：

```text
内部 registry + digest pin
按训练子集预拉取
DaemonSet/平台镜像缓存
失败时不允许静默拉公网镜像
```

`local_only` 在 Kubernetes 语义中应解释为：只允许内部 registry/预置镜像，不允许第三方公网 egress。

探活可见内部 registry 样例（训练任务镜像）：

```text
registry2.d.pjlab.org.cn/...
```

Smith 镜像同样应进入平台可拉取的内部 registry，并配置 `imagePullSecrets`。episode 热路径禁止依赖公网 Docker Hub。

### 6.4 Official harness 执行落点（补齐）

当前 `smith_eval.rs` 在 **Worker 进程本机**执行外部命令。Kubernetes backend 下该路径无效。

**首期默认决策（冻结）**：

```text
官方 harness 在同一 session Pod 内，经 backend.exec 调用
入口：/opt/uenv/bin/eval_swesmith_official（或镜像内等价 wrapper）
由 Worker 传入 instance JSON / model patch（经 write_file 或 stdin）
强制 timeout；超时 kill exec 并进入统一 cleanup
```

**备用路径（仅当 session 镜像无法容纳 harness 依赖时）**：

```text
短生命周期 grader Pod/Job，只读挂载 patch/artifact
同样走 SweSessionBackend，不走 Worker 本机 shell
```

禁止：

```text
在 Worker 容器内直接 sh -c UENV_SWE_SMITH_EVAL_CMD 并假定能看到 /testbed
无 timeout 的 Command::output()
把内部 pytest parser 结果标记为 official_swesmith
```

## 7. Kubernetes API 生命周期实现

### 7.1 Create

创建顺序：

```text
1. pool pending reservation
2. 生成 session/episode/lease labels
3. 构造 Pod（或 Volcano Job）manifest
4. create 对象，并设置 OwnerReference（若有 session 附属资源）
5. 记录 Pod UID/resourceVersion
6. watch 状态
7. Running 且容器 Ready 后返回 BackendSession
```

`create` 成功但 watch 超时必须执行 delete；不能只释放 Rust reservation 而留下 Pod。

### 7.1.1 Pod vs Volcano Job 闸门（补齐）

母文档与探活事实：现有 `uenv-public-verl-128-copy` 由 **Volcano** 调度；用户凭证具备 Volcano Job 权限；普通 CPU-only Pod 是否可稳定获得资源 **尚未用真实 Worker SA 验证**。

代码实施顺序：

```text
1. 默认实现原生 Pod 路径（session_kind=pod）
2. 在业务 namespace 用最终 Worker ServiceAccount 做 CPU-only 探针
3. 若 Pending 超过 schedule timeout 且事件显示需 Volcano/队列：
     启用 session_kind=volcano_job（或 platform_job）
     封装单副本 Job，backend 仍返回统一 BackendSession
4. 配置项预留：
     scheduler_name
     volcano_queue
     volcano_queue annotations
```

在闸门未通过前，不得把“Pod create API 成功”等同于“可调度可运行”。

### 7.2 Watch/Ready

状态机至少区分：

```text
Pending
Scheduled
Pulling
Running
Ready
Failed
Succeeded
Terminating
Deleted
```

超时维度分开配置：

```text
pod_schedule_timeout
image_pull_timeout
readiness_timeout
reset_timeout
episode_timeout
cleanup_timeout
```

### 7.3 Exec

使用 Kubernetes exec API，不在 Worker Pod 内执行 `kubectl exec` shell。必须：

```text
stdin/stdout/stderr 独立处理
退出码可获得或通过 wrapper 明确返回
timeout 后关闭 stream
上游取消后 kill-on-drop
command policy 在 exec 前执行
输出长度限制与现有 ExecResult 对齐
```

建议在 Pod 内放一个受控 command wrapper，确保命令 PID 可定位和终止：

```text
/opt/uenv/bin/exec-wrapper <timeout> <command>
```

不能依赖远端 shell 自己终止所有子进程。pytest/retry 命令必须额外使用 harness timeout。

### 7.4 Read/Write

优先通过 exec + base64 或 Kubernetes stream 实现小文件 read/write；大文件通过 artifact/workspace 服务，不应把大 patch、日志和测试输出全部塞进 HTTP JSON。

写入必须：

```text
限制路径在 /testbed 内
拒绝路径穿越
保留 episode/session 关联
限制单文件大小
记录 checksum
```

### 7.5 Terminate/Cleanup

正常完成、失败、取消、超时、Worker shutdown 均进入同一 cleanup：

```text
停止或取消 active exec
删除 Pod/Job
删除 session Service（如创建）
删除临时 PVC（如创建且 owner 为 session）
释放 pool slot
记录 cleanup outcome
```

删除使用 propagation policy，避免 Job 删除后 Pod 残留。cleanup 必须幂等，重复执行不能误报失败。

### 7.6 Reconcile

Worker 启动和定期任务执行：

```text
list namespace 中 app.kubernetes.io/part-of=uenv 的 session
按 worker-id 过滤
匹配本地 session/lease map
无本地 owner 的 Pod 标记 orphan
超过 orphan TTL 的 Pod 删除
本地有 lease 但 Pod 不存在的 session 标记 failed/release
```

不能删除其他 Worker 或其他 run 的 Pod。reconcile 需要支持 dry-run 和 metrics。

### 7.7 RBAC 与 ServiceAccount 资源边界（本轮复核补齐）

RBAC 必须按实际启用的 backend 分支逐项验证，不能只验证 Pod create。首期使用 `emptyDir`
时不授予 PVC 写权限；启用 session 专属 PVC、Service 或 Volcano Job 时，才增加对应的
namespace-scoped 权限：

```text
pods: get/list/watch/create/delete/patch
pods/exec: create
pods/log: get
events: get/list/watch
jobs: get/list/watch/create/delete/patch（仅 Job/Volcano 路径）
services: get/list/watch/create/delete（仅实际创建 session Service）
persistentvolumeclaims: get/list/watch/create/delete（仅实际创建 session PVC）
```

Worker Deployment 的 ServiceAccount 与 session Pod 的 ServiceAccount 分开。session Pod 默认
`automountServiceAccountToken: false`，不得把 Worker 的 Kubernetes API 凭证带入不可信的
测试仓库；若 harness 确实需要 token，必须单独说明用途并审查。所有 list/watch/create/delete
必须限制在显式业务 namespace，selector 同时包含 `part-of=uenv`、`component=swe-session`
和当前 `worker-id`。门禁必须用最终 Worker ServiceAccount 实测 create/get/watch/exec/log/delete
及实际启用的 PVC/Service/Volcano 分支，管理员 kubeconfig 成功不能替代。

## 8. Pool、Gateway 和 Control Plane 对接

### 8.1 SweInstancePool

保持现有：

```text
capacity
pending reservation
sessions map
session_count
create_session
destroy
run_episode
```

改为：

```text
pool 持有 Arc<dyn SweSessionBackend>
create_session 调 backend.provision
destroy 调 backend.terminate
启动时 backend.reconcile
session_count 与 Kubernetes session 状态双向对账
```

session map 不能只在 Pod 创建后才写入；应在 create request 生成后记录 pending handle，避免 Worker 崩溃时无法知道未完成创建的 Pod。

### 8.2 Runtime Gateway

`uenv-worker/src/runtime_gateway/mod.rs` 的 HTTP 路由保持不变：

```text
POST /runtime/v1/sessions
POST /runtime/v1/sessions/{id}/exec
POST /runtime/v1/sessions/{id}/read
POST /runtime/v1/sessions/{id}/write
POST /runtime/v1/sessions/{id}/submit
DELETE /runtime/v1/sessions/{id}
GET /runtime/v1/health
```

需要补充：

```text
Kubernetes backend 错误到 HTTP status 的稳定映射
Pod pending/ready 状态可观测字段
submit 超时后的后台 cleanup
客户端断开后的 session release
X-API-Key 继续保留
```

Gateway 不应公开 Kubernetes API，也不应把 Kubernetes kubeconfig、ServiceAccount token 返回给 Agent。

### 8.3 Control Plane 与容量对齐

首期如果 Server/Hub/Agent 全在集群内，Worker 注册的 endpoint 使用 ClusterIP Service DNS。需要在 Worker 注册信息中区分：

```text
worker gRPC control endpoint   <- worker.advertise_endpoint
runtime gateway endpoint       <- gateway_public_url / 内部 Service
health/metrics endpoint
```

现有 `control_plane/client.rs` 已携带 `advertise_endpoint`、`gateway_public_url`、`max_concurrent`、`ResourceSpec`。集群部署时必须：

```text
advertise_endpoint = uenv-worker-control.<ns>.svc.cluster.local:28888
（或当前实际 control 端口，与 Service 一致）
gateway_public_url = http://uenv-worker-gateway.<ns>.svc.cluster.local:28097
ResourceSpec.gpu_count = 0；不得继承训练 Pod 的 Ascend 资源画像
```

不能把 Pod IP 作为长期 endpoint，也不能把公网 kubeconfig API 地址混入 Worker 配置。

**容量对齐（母文档踩坑 #11）**：下列值必须由同一 K 推导或启动时校验一致，否则拒绝启动：

```text
worker.max_concurrent
runtime_gateway.capacity
SweInstancePool capacity
Agent 侧并发上限（部署配置）
run-level max_episode_concurrency（长期）
```

短期单实例：`K=1`（验收）→ 通过后 `K=2`。Gateway capacity 与 pool 不得静默大于 Worker max_concurrent。

实现上不能继续沿用当前 `gateway_capacity.max(max_concurrent)` 的 pool 初始化逻辑。应在配置
加载阶段计算并保存明确的 effective `K`，首期要求：

```text
1 <= runtime_gateway.capacity <= worker.max_concurrent
pool.capacity == runtime_gateway.capacity == effective K
```

配置不一致时拒绝启动，而不是取较大值或静默裁剪。Gateway 未启用时，native pool 仍不得超过
Worker `max_concurrent`。`gateway_public_url` 不能由 `listen`（尤其 `0.0.0.0`）推导，应增加
显式 `runtime_gateway.advertise_url`，使用 gateway Service DNS；control-plane endpoint 继续
使用 Worker control Service DNS 和实际端口。

### 8.4 业务 namespace 与现有工作负载隔离（补齐）

```text
禁止默认复用 default namespace
禁止操作/删除：
  uenv-public-verl-128-copy-*
  uenv-workspace-shell
  既有训练 PVC（如 pvc-prxv9）
建议使用平台分配的业务 namespace（可能是 UUID 形态）
配置中的 namespace 必须显式给出，禁止代码默认 "default"
```

若平台无法创建名为 `uenv-swe` 的 namespace，配置改为实际分配的 namespace；manifest 模板用 kustomize/overlay 注入，不写死。

镜像拉取由节点 kubelet 执行，不代表 Worker Pod 自身具备 registry egress。必须单独验证
imagePullSecret、registry DNS/TLS、内部 registry arch/digest，以及 session Pod 的 Ready 结果。

## 9. Reward、Trajectory 和 Rollout

### 9.1 Official Smith grader

`UENV_SWE_SMITH_EVAL_CMD` 的实际实现应固定为镜像内版本化 wrapper（默认在 **同一 session Pod** 内经 `backend.exec` 调用，见 §6.4）：

```text
/opt/uenv/bin/eval_swesmith_official
```

输入至少绑定：

```text
instance JSON
model patch
episode_id
session_id
```

输出为单个 JSON，至少包含：

```json
{
  "resolved": false,
  "reward": 0.25,
  "per_test": [["test_name", true]]
}
```

wrapper 必须记录官方 harness 版本和命令摘要，禁止静默退回内部 parser 并声称是官方结果。

### 9.2 非 gold 约束

最终短期验收必须检查：

```text
model patch 非空
model patch 与 gold patch 不同
model patch 只代表 Agent 修复，不包含 dataset patch
reward > 0
trajectory.steps 非空
```

reward=0 时保存完整诊断，不修改 reward 使其通过。

### 9.3 Trajectory 元数据

每条 rollout 必须带：

```text
run_id
session_id
instance_id
benchmark_variant=smith
worker_id
lease_id
env/image digest
reward
resolved
trajectory schema
artifact URI/checksum
```

Kubernetes Pod name 可作为 debug 字段，但不能替代 session_id 或 episode_id。

## 10. 配置和部署文件

### 10.1 Worker 配置新增字段

在现有 `SweSection`（已有 `variants` / `env_package_dir` / `seccomp_profile_dir` 等）上扩展，建议增加等价配置：

```yaml
swe:
  backend: kubernetes          # 与 env.backend（process/podman）分开
  kubernetes:
    namespace: <platform-assigned-ns>   # 禁止默认 default
    service_account: uenv-worker
    session_kind: pod          # pod | volcano_job
    scheduler_name: ""         # 需要时填 volcano
    volcano_queue: ""
    session_image_pull_policy: IfNotPresent
    image_pull_secrets: []
    session_cpu: "2"
    session_memory: 4Gi
    session_ephemeral_storage: 20Gi
    session_pids_limit: 4096
    node_selector: {}          # 架构/队列需要时使用
    pod_schedule_timeout_secs: 180
    image_pull_timeout_secs: 300
    pod_ready_timeout_secs: 300
    exec_timeout_secs: 180
    cleanup_timeout_secs: 60
    orphan_ttl_secs: 900
    storage_class: ""          # 需要 PVC 时显式配置，如 quark-vcproxy-sc
  variants: ["smith"]
  env_package_dir: /var/lib/uenv/envs/swe-bench-smith/0.1.0

worker:
  advertise_endpoint: "uenv-worker-control.<ns>.svc.cluster.local:28888"
  max_concurrent: 2

runtime_gateway:
  enabled: true
  advertise_url: "http://uenv-worker-gateway.<ns>.svc.cluster.local:28097"
  capacity: 2                  # 必须 <= max_concurrent

trajectory_upload:
  endpoint: "http://trajectory-store.<ns>.svc.cluster.local:<actual-service-port>"
```

字段名以当前 config 模块风格为准，不能直接假定以上 YAML 已被现有配置反序列化。启动时应对齐校验 `max_concurrent` 与 `runtime_gateway.capacity`。

`trajectory_upload.endpoint` 的端口必须来自实际部署的 Server trajectory HTTP Service，不能
把 `8077` 当作仓库固定端口。部署清单、Server 监听配置和 Worker endpoint 必须一致，并验证
`POST /control/v1/trajectories`、鉴权、gzip 和幂等 ack。现有 uploader 失败会进入本地 spool，
因此 spool 的持久化卷、大小上限和重启恢复也必须纳入部署设计。

### 10.2 Kubernetes manifests

新增部署目录建议：

```text
deploy/kubernetes/uenv-swe/
├── namespace.yaml                 # 或仅文档说明使用平台已有 ns
├── service-account.yaml
├── role.yaml
├── role-binding.yaml
├── network-policy.yaml           # 仅允许 Server/Agent/Hub/model/artifact
├── worker-config.yaml
├── worker-secret.example.yaml
├── worker-deployment.yaml
├── worker-service.yaml           # control + gateway + health 端口
├── session-pod-template.yaml
├── hub-deployment.yaml
├── server-deployment.yaml
├── agent-deployment.yaml
├── model-gateway-deployment.yaml  # 若不用平台 llm-gateway
└── kustomization.yaml
```

实际 Secret 不提交；`worker-secret.example.yaml` 只能包含 key 名和注释。

**现有集群组件**：已存在 `llm-gateway` namespace。部署前先探针其 OpenAI-compatible `/v1/chat/completions`、鉴权与模型名；不兼容则部署独立 `uenv-model-gateway`，**不得修改平台系统组件**。

**隔离硬约束**：

```text
不在 default 部署 SWE 业务（除非平台强制，且仍禁止碰 verl-128 任务）
reconcile/cleanup 选择器必须含 worker-id + part-of=uenv
任何批量 kubectl delete 脚本禁止无 label 的命名空间级删除
```

### 10.3 镜像构建

需要构建 ARM64 Worker 镜像：

```text
uenv-worker linux/arm64 binary
Kubernetes client dependencies
ca-certificates
日志和 metrics 配置
不包含 kubeconfig / 公网 API 地址
不包含 Hub token/LLM key/API key
```

Smith session image 与 Worker image 分离。Worker image 不应把所有 Smith repo 镜像打进去。

## 11. 代码实施阶段

阶段与母文档验证方案对齐：`P*` 是代码交付；集群探针门禁见 §11.1，未通过不得进入 P4 真实业务部署。

### 11.1 集群探针门禁（代码前/并行，不计入“已部署”）

对应母文档阶段 A / §14.2。用最终业务 namespace + **Worker ServiceAccount**（不是管理员 kubeconfig）验证：

```text
CPU-only 探针 Pod 可调度、Ready
in-cluster API：create/get/delete/exec session 级资源
DNS 到计划中的 ClusterIP 名称
到 Hub/model/trajectory 的 HTTP（若已部署）或占位 Service
PVC（若需要）可绑定已知 StorageClass
镜像可从内部 registry 拉取；记录 digest 与 arch
确认不需要或需要 Volcano Job / queue
确认不触碰 uenv-public-verl-128-copy
确认最终 Worker ServiceAccount 对应的 RBAC 边界满足 §7.7
确认 control/gateway/trajectory endpoint 与 Service 和端口一致
确认 Worker 注册 `ResourceSpec.gpu_count=0`，且没有非零 `UENV_WORKER_GPU_COUNT`
```

未通过本门禁时，P1–P3 仍可在 fake/mock 上开发，但 **禁止** 向集群提交业务 Deployment。

### P0：抽象与兼容回归

交付：

```text
SweSessionBackend trait（与 crate::backend 沙箱抽象分离）
BackendSession/Error
CliContainerBackend 保持原路径
SweSession/InstancePool/episode executor 改为依赖 trait
现有 Docker/Podman 测试通过
配置轴：swe.backend vs env.backend 不混淆
Kubernetes 模式禁止 native `run_instance` fallback；兼容 fallback 仅限测试/CLI
effective K 明确定义；Gateway 启用时 pool == gateway.capacity <= worker.max_concurrent，Gateway 关闭时 pool <= worker.max_concurrent
```

验证：

```bash
cargo fmt --all -- --check
cargo check -p uenv-worker
cargo test -p uenv-worker swe
```

### P1：Kubernetes client 和单 Pod 生命周期

交付：

```text
in-cluster client（API 对齐 1.31）
sync/async 桥接策略落地
Pod create/watch/delete
CPU-only manifest + SecurityContext 初版
label/annotation + 架构校验钩子
readiness/schedule/imagePull timeout
幂等 cleanup
Volcano Job 配置面预留（实现可后置）
```

验证：fake client 或 mock API 覆盖 create、watch timeout、delete error、reconcile、架构不匹配。

### P2：Kubernetes exec/read/write

交付：

```text
remote exec
stdout/stderr/exit code
timeout/cancel/kill
path policy
output truncation
```

验证：`sleep`、无限循环、pytest 超时、客户端断开、并发 exec。

### P3：Smith provision/evaluate

交付：

```text
/testbed
clean-to-buggy
dataset patch
session Pod 内官方 harness wrapper（默认落点）
reward/resolved
trajectory/artifact（集群内 endpoint）
```

验证：使用 fake/mock backend 完成 1 个真实 catalog row 的环境 contract smoke，再完成 Pod
内 harness 的非 Agent grading smoke；不得把真实 Agent episode 放在 P4 集群模块部署之前。

### P4：集群内模块部署

交付：

```text
通过 §11.1 门禁后的：
Server/Adapter Core
Hub
Worker
Agent
model gateway（复用或独立）
trajectory/artifact service
ClusterIP DNS
Secret/RBAC/NetworkPolicy
advertise_endpoint / gateway URL 正确
```

验证：全部模块不依赖外部 Server/Agent 回连，Worker 注册、Gateway 健康、Hub catalog、model
chat API 均可用；随后在本阶段完成一个真实 Smith non-gold Agent episode，作为业务联调验收。
若必须混合外部模块，先完成母文档 §10.2 入站入口，否则本阶段失败。

### P5：并发能力

交付：

```text
K=2
双 session
lease/release
orphan reconcile
pool metrics
容量字段一致性
```

短期目标完成后再进入：

```text
K=4 -> 16/32 -> 64 -> 128 CPU-only sessions
```

## 12. 测试矩阵

### 12.1 单元测试

```text
Pod name/label 生成
ProvisionSpec Smith contract
CPU-only resource manifest
SecurityContext / CommandPolicy 映射
image digest + os/arch 校验
Pod 状态映射
exec timeout/kill
path traversal 拒绝
cleanup 幂等
reconcile orphan（不删其他 worker-id）
lease release
official harness JSON 解析（backend.exec 路径）
非 gold/gold patch 区分
swe.backend vs env.backend 配置解析
max_concurrent / gateway.capacity 对齐校验
sync/async 桥接在 mock 下不 panic
listen 与 advertise URL 分离，禁止注册 `0.0.0.0` 或 Pod IP
SWE Kubernetes 注册资源强制 `gpu_count=0`
Kubernetes 模式不走 `run_instance` fallback
Worker/session ServiceAccount 与 namespace-scoped RBAC 校验
```

### 12.2 集成测试

```text
create Pod -> Ready -> reset
exec/read/write
官方 harness wrapper（Pod 内）
trajectory seal/upload
delete Pod
Worker 重启 reconcile
Pod image pull failure
镜像架构不匹配
PVC mount failure
ServiceAccount RBAC denial
session Service/PVC RBAC（启用对应资源分支时）
API server timeout/retry
Volcano/queue Pending（若启用）
```

### 12.3 最终验收测试

使用一个真实 Smith instance：

```text
env_type=swe
benchmark_variant=smith
execution_mode=agent
CPU-only session Pod
model patch 非 gold
official reward > 0
trajectory.steps 非空
rollout 可回取
cleanup 完成
```

失败时保留：

```text
Pod describe/events
Worker structured logs
Agent logs
model request metadata（不含密钥）
official harness stdout/stderr
trajectory/artifact references
cleanup report
```

## 13. 交付检查清单

### 代码

- [ ] backend trait 和 handle（与 `crate::backend` 沙箱抽象分离）
- [ ] Docker/Podman 兼容回归
- [ ] `episode/executor` 与 Gateway 共用 backend
- [ ] sync/async kube 桥接无死锁
- [ ] Kubernetes client（对齐 API 1.31）
- [ ] Pod lifecycle（含 Volcano 闸门配置面）
- [ ] Kubernetes exec
- [ ] read/write path safety
- [ ] timeout/cancel/kill
- [ ] cleanup/reconcile（仅自身 label）
- [ ] image digest/provider + **os/arch 校验**
- [ ] CommandPolicy → SecurityContext 映射
- [ ] Smith contract 接入
- [ ] official harness **在 session/grader Pod 内**执行 + timeout
- [ ] trajectory metadata + 集群内 upload endpoint
- [ ] control plane advertise/gateway URL / gpu_count=0
- [ ] max_concurrent ↔ gateway.capacity ↔ pool 对齐
- [ ] Kubernetes 模式禁止 native `run_instance` fallback
- [ ] Worker/session ServiceAccount 分离及最小 namespace-scoped RBAC
- [ ] trajectory Service 实际端口、鉴权、gzip、spool 重启恢复
- [ ] metrics/logging

### 部署

- [ ] §11.1 Worker SA 探针门禁通过
- [ ] linux/arm64 Worker image
- [ ] 平台业务 namespace（非 default）
- [ ] ServiceAccount/RBAC（最小权限）
- [ ] NetworkPolicy
- [ ] ConfigMap
- [ ] Secret 注入
- [ ] Worker Deployment/Service
- [ ] session Pod template（CPU-only，无 Ascend）
- [ ] Hub/Server/Agent/model gateway/artifact Service
- [ ] 现有 llm-gateway 兼容性结论或独立网关
- [ ] imagePullSecret/内部 registry
- [ ] StorageClass 显式配置（若用 PVC）
- [ ] 确认未触碰 verl-128 / workspace-shell / 既有 PVC

### 短期验收

- [ ] 真实 Smith catalog row
- [ ] 对应完整镜像 digest + **架构匹配**
- [ ] `/testbed` 正常
- [ ] dataset patch 方向正确
- [ ] 官方 harness 可执行（Pod 内）
- [ ] 集群内模型服务可用
- [ ] Agent 完成非 gold 修复
- [ ] reward > 0
- [ ] trajectory/rollout 有效
- [ ] Pod/exec/lease/workspace 无泄漏

### 长期扩展前置

- [ ] K=2 双并发通过
- [ ] K=4 长稳通过
- [ ] CPU/memory/storage/pids/fd 画像
- [ ] Agent/model gateway 容量对齐
- [ ] 镜像预取策略
- [ ] catalog 分片策略
- [ ] run-level admission
- [ ] 多 Worker 调度和 lease
- [ ] 16/32/64/128 扩容门槛

## 14. 本轮审查补齐的缺口摘要

相对母文档与当前代码，原规划稿遗漏或表述不足、现已写入上文的要点：

| 缺口 | 补齐位置 |
|---|---|
| `runtime_gateway` 路径写错；遗漏 `episode/executor`、`control_plane`、`command_policy`、`trajectory_upload` | §2.1 |
| 与既有 `crate::backend` 沙箱抽象命名冲突 | §2.3 |
| `smith_eval` 仍在 Worker 本机执行、无 timeout | §2.2、§6.4 |
| kube client 应对齐 Server **1.31**，而非本机 kubectl 1.34 | §4.1 |
| 同步 SweSession vs 异步 kube 桥接 | §4.3 |
| ARM64 节点 vs Smith 镜像架构 | §5.4 |
| PID/SecurityContext/禁止 hostPath seccomp | §5.1、§5.5 |
| StorageClass list 被拒、禁止复用训练 PVC | §5.3 |
| 内部 registry / imagePullSecrets | §6.3 |
| Volcano 调度闸门未写成实施步骤 | §7.1.1 |
| advertise_endpoint、容量字段对齐、gpu_count=0 | §8.3 |
| 禁止碰 default 中 verl-128 等工作负载 | §8.4 |
| NetworkPolicy、llm-gateway 复用策略、配置轴分离 | §10 |
| 集群探针门禁与 P4 部署顺序 | §11.1 |
| 当前 pool 用 `max()` 超过 Worker 容量、listen 被误当 advertise、SWE GPU 可被环境变量覆盖、native fallback 绕过新 backend | §2.4、§8.3、§11 |
| 实际 trajectory Service 端口、gzip/鉴权/幂等、spool 持久化恢复未形成部署验收 | §10.1、§12、§13 |
| Worker 与 session ServiceAccount/RBAC 及 token automount 边界不足 | §7.7、§11.1、§12 |

若后续探活推翻某项假设（例如必须 Volcano、必须 amd64 节点），先更新母文档事实，再修订本表对应条款。

## 15. 与上层指导文档的关系

本文不重复定义集群事实和最终业务目标。以下内容以母文档为准：

| 主题 | 权威位置 |
|---|---|
| 公网 kubeconfig、API Server、kubectl | 母文档第 1-3 节 |
| 集群节点、Ascend 资源、PVC、现有 128 卡任务 | 母文档第 4-5 节 |
| SWE-smith 容量、CPU-only、短期/长期目标 | 母文档第 8 节、第 16-17 节 |
| Kubernetes-native 方案和模块通信 | 母文档第 9-12 节 |
| 历史 SWE-smith 踩坑 | 母文档第 15 节 |
| 本文 | 具体 backend 代码、测试、部署 manifest 和实施顺序 |

若本文与母文档冲突，优先修订本文以对齐母文档；不得通过代码默认值绕过母文档冻结的 CPU-only、Smith contract 和非 gold 验收要求。

## 16. 当前实施进度（2026-08-19）

```text
P0/P1 代码主链路：已完成初版，SweSessionBackend、CLI backend、Kubernetes backend、配置轴和 runtime 工厂已接入
本地编译：cargo check -p uenv-worker 通过
Worker 单元测试：160 passed
ARM64 Worker 镜像：本地构建成功，digest 为 sha256:5635a68a555c8ef2e60f21eddd97a2b02bd8d18ba3d5d440929ef77cba023035
SSH 访问：已通过本地 SOCKS5 代理和 id_ed25519_pjlab 成功连接 h.pjlab.org.cn
镜像上传：已通过 scp 上传 ARM64 Worker 镜像归档到 `/mnt/shared-storage-user/evobox-share/uenv`
集群部署：未开始，不得视为 P4 已部署
Smith non-gold 验收：未开始
```

当前进度只证明代码可编译、本地测试通过和 ARM64 镜像可构建，不证明 Kubernetes session
Pod 已可调度，也不证明 Smith 镜像、官方 harness、Agent 和模型链路已可用。必须完成 §11.1
Worker ServiceAccount 探针门禁后，才能进入真实业务部署和短期验收。
