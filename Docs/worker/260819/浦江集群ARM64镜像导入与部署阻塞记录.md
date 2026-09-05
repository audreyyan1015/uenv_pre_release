# 浦江集群 ARM64 镜像导入与部署阻塞记录

> 记录日期：2026-08-19
>
> 用途：独立记录当前将本地 ARM64 UEnv Worker 镜像导入浦江集群并启动探针时，已经执行的验证、验证结果、阻塞原因和需要平台协助的事项。
>
> 安全：本文不保存 kubeconfig、Token、密码、client certificate、private key 或其他凭证内容。

## 1. 当前目标

当前正在推进的步骤是：

```text
本地 ARM64 Worker 镜像
  -> 通过 SSH/scp 上传到浦江共享存储
  -> 导入内部 Registry 或节点 containerd
  -> 以集群可拉取镜像启动 CPU-only Worker/探针 Pod
  -> 验证镜像架构、imageID/digest、Pod Ready 和基础生命周期
  -> 继续 Worker SA、RBAC、SWE session Pod 和 Smith 链路验证
```

当前短期目标仍然是 Kubernetes-native SWE backend 的单实例完整链路；本阶段只处理 Worker 镜像进入集群和最小部署门禁，不提前开始 16 并发或真实 Smith episode。

## 2. 本地镜像事实

本地 Docker 镜像：

```text
引用：registry2.d.pjlab.org.cn/uenv/uenv-worker:arm64-dev
OS：linux
架构：arm64
镜像 digest：sha256:5635a68a555c8ef2e60f21eddd97a2b02bd8d18ba3d5d440929ef77cba023035
Docker inspect 逻辑大小：5270888795 bytes
```

本地 Docker Desktop 信息：

```text
Server Version：29.3.1
OS：Docker Desktop
Architecture：aarch64
```

镜像已通过 `docker save` 导出为归档：

```text
本地归档：/var/folders/d6/sqz5sk8s7wxd8wl36rfs5hv80000gn/T/opencode/uenv-worker-arm64-dev.tar
归档大小：5270912512 bytes
归档 SHA-256：954e6baf8d3a78ad86320901d73ce2909ff54500476fff1a2ab6795fa55463d3
```

本地归档位于临时目录，不属于 Git 工作区，也没有提交到仓库。

## 3. SSH 连接验证

SSH 网关连接方式已经验证成功：

```text
网关：h.pjlab.org.cn:22
本地 SOCKS5：127.0.0.1:7890
私钥：~/.ssh/id_ed25519_pjlab
```

使用 `ProxyCommand=nc -x 127.0.0.1:7890 -X 5 %h %p`，并指定：

```text
IdentitiesOnly=yes
IdentityAgent=none
```

认证结果：

```text
Server accepts key
Authenticated using publickey
```

SSH 过程中反复出现以下客户端提示：

```text
client_global_hostkeys_prove_confirm: server gave bad signature for RSA key 0: incorrect signature
```

但 SSH 认证、远程命令执行和文件传输均可继续完成；当前没有证据表明该提示导致本次镜像上传失败。

## 4. 共享存储验证

目标共享存储：

```text
/mnt/shared-storage-user/evobox-share
```

文件系统和容量探查结果：

```text
文件系统：gpfs
挂载源：ipfs[/public-shared/fileset-projects/evobox-share]
总容量：约 14T
初次探查可用：约 3.1T
上传后可用：约 2.9T
上传后使用率：约 80%
```

已创建目录：

```text
/mnt/shared-storage-user/evobox-share/uenv
```

目录创建后的状态：

```text
mode：drwxr-xr-x
创建时 owner：root
创建时 group：root
```

共享根目录原始状态记录为：

```text
owner：fangtianshun
group：fangtianshun
mode：drwxr-xr-x
```

## 5. 镜像上传验证

已使用 SSH/SOCKS5 通道将归档上传至：

```text
/mnt/shared-storage-user/evobox-share/uenv/uenv-worker-arm64-dev.tar
```

首次使用 `scp` 上传时，命令在 120 秒超时前只传输了一部分文件；随后使用支持断点续传的 `rsync --partial` 完成上传。

最终远端文件状态：

```text
文件大小：5270912512 bytes
远端 SHA-256：954e6baf8d3a78ad86320901d73ce2909ff54500476fff1a2ab6795fa55463d3
```

远端 SHA-256 与本地归档一致，确认文件已完整上传。

截至本记录，已确认的是“归档文件进入共享存储”，尚未确认“归档文件已进入 Registry 或节点 containerd 镜像存储”。

## 6. Registry push 验证

本地已经重新尝试直接推送：

```bash
docker push registry2.d.pjlab.org.cn/uenv/uenv-worker:arm64-dev
```

结果仍然失败：

```text
unknown: unexpected status from HEAD request to
https://registry2.d.pjlab.org.cn/v2/uenv/uenv-worker/blobs/<digest>:
401 Unauthorized
```

从集群 SSH 工作空间访问 Registry：

```bash
curl -k -I https://registry2.d.pjlab.org.cn/v2/
```

结果：

```text
HTTP/2 401
docker-distribution-api-version: registry/2.0
www-authenticate: Bearer realm="https://registry2.d.pjlab.org.cn/service/token",service="harbor-registry"
```

这说明 Registry 服务网络可达且正常返回 Harbor 鉴权挑战，但当前本地 Docker 客户端没有可用于该 repository 的有效认证，或者当前账号没有 `uenv/uenv-worker` 项目的 push 权限。

当前尚未尝试或尚未具备的内容：

```text
Registry 用户名/token
正确的 Harbor project/repository 权限
平台提供的登录命令
平台提供的 robot account
平台提供的离线镜像导入接口
```

## 7. SSH 工作空间运行时边界

SSH 登录进入的是平台工作空间容器，不是 Kubernetes 节点 shell。

已验证：

```text
PID 1：/kubebrain/worker-init
hostname：colsoda-sts8k-1518854-worker-0
uid/gid：root/root
```

但以下节点级工具或接口均不可用：

```text
ctr：不存在
nerdctl：不存在
docker：不存在
crictl：不存在
/var/run/containerd：不存在
/run/containerd：不存在
/var/lib/containerd：不存在
containerd socket：未发现
kubeconfig：未发现
```

工作空间的 mount/cgroup 信息能够显示其属于 Kubernetes 管理的 containerd Pod，并且可以看到共享 GPFS 挂载，但这不等于拥有宿主机 containerd 的管理权限。

因此以下做法当前不可行，不能继续假设：

```bash
ctr images import /mnt/shared-storage-user/evobox-share/uenv/uenv-worker-arm64-dev.tar
nerdctl load -i /mnt/shared-storage-user/evobox-share/uenv/uenv-worker-arm64-dev.tar
crictl images
```

原因是这些命令即使安装到工作空间，也无法凭空获得节点 containerd socket 和节点级权限。

## 8. Kubernetes API 权限验证

本机使用：

```text
~/.kube/pjlab-public-config
```

访问的 API Server：

```text
https://10.140.158.149:49256
```

已验证 API 基础访问可用，但当前 kubeconfig 用户权限不足以进行部署探针：

```bash
kubectl auth can-i create pods
```

结果：

```text
no
```

以下操作均被拒绝或无法进行：

```text
列出全局 namespaces
列出全局 StorageClass
列出全局 PVC
列出 default namespace Pod
列出已知业务 namespace Pod
创建 Pod
创建 Job
```

API 返回的典型错误：

```text
namespaces is forbidden
pods is forbidden
storageclasses.storage.k8s.io is forbidden
persistentvolumeclaims is forbidden
```

因此当前还不能用本机 kubeconfig 代替最终 Worker ServiceAccount 完成 §11.1 探针门禁。

## 9. brainctl 探查

SSH 工作空间中发现平台工具：

```text
/kubebrain/brainctl
版本：v2.11.22-38-20260817030035-1c92099e14cd
```

相关能力包括：

```text
brainctl create
brainctl apply
brainctl get
brainctl delete
brainctl describe
brainctl logs
brainctl exec
brainctl cp
brainctl launch
```

`brainctl launch --help` 显示其支持：

```text
--image
--image-pull-policy
--mount
--volume
--local-storage
--cpu
--memory
```

其中 `--image` 的语义是使用一个可被平台拉取的镜像启动 Worker，并没有发现可以直接将共享路径下 Docker tar 归档作为镜像导入的参数。

使用当前 SSH 工作空间凭证执行平台资源查询时，权限仍不足：

```text
pods is forbidden in namespace ailab-sys
namespaces is forbidden at cluster scope
```

当前结论：

```text
brainctl 是后续可能的集群操作入口
brainctl launch --image 可能用于启动 Worker
但当前凭证没有足够的资源权限
尚未证明 brainctl 支持 tar 归档导入
```

## 10. 已排除的路径

### 10.1 在 SSH 工作空间直接导入 containerd

不可行。工作空间没有 containerd socket 和节点级运行时权限。

### 10.2 仅上传 tar 后直接在 Pod 中使用

不可行。Kubernetes `PodSpec.containers[].image` 需要镜像引用，不能直接填写共享盘上的 tar 文件路径。共享存储挂载只能让容器读取文件，不能自动把 tar 转换为 kubelet 可用镜像。

### 10.3 使用当前本机 kubeconfig 创建探针 Pod

不可行。当前用户 `create pods` 权限为 `no`，且没有可确认的业务 namespace。

### 10.4 使用当前 SSH 工作空间凭证通过 brainctl 创建 Pod

暂不可行。当前 brainctl 上下文为 `ailab-sys`，Pod 和 namespace 查询已经被 API 拒绝。

## 11. 当前阻塞问题

当前阻塞分为三个相互独立的权限/流程问题：

### 11.1 镜像进入集群的流程未明确

已完成共享盘上传，但没有已验证的：

```text
共享盘 tar -> 内部 Registry
共享盘 tar -> 节点 containerd
共享盘 tar -> 平台镜像缓存
```

需要平台明确支持哪种方式，以及对应命令、任务入口或操作权限。

### 11.2 Registry push 权限未开通

`registry2.d.pjlab.org.cn` 网络可达，但直接 push 返回 `401 Unauthorized`。

需要确认：

```text
Registry 登录地址
用户名或 robot account
Token 获取方式
允许 push 的 project/repository
是否需要使用不同的 repository 名称
是否要求先创建 project
```

### 11.3 Kubernetes 部署权限和业务 namespace 未确认

需要提供或确认：

```text
平台分配的业务 namespace
可创建 Pod/Job 的用户或 ServiceAccount
Worker ServiceAccount
imagePullSecret
Pod get/list/watch/exec/log/delete 权限
是否需要 Volcano Job/queue
```

当前不能使用 `default` namespace 提交 SWE 业务，也不能触碰：

```text
uenv-public-verl-128-copy-*
uenv-workspace-shell
既有训练 PVC
```

## 12. 需要平台协助确认的事项

建议平台按以下问题直接回复：

```text
1. 是否可以为当前用户开通 registry2.d.pjlab.org.cn 的 push 权限？
2. 如果可以，正确的登录命令、project、repository 和 token 获取方式是什么？
3. 如果不允许直接 push，是否有“从共享存储导入镜像”的平台接口或任务？
4. /mnt/shared-storage-user/evobox-share/uenv/uenv-worker-arm64-dev.tar 应通过什么命令导入？
5. 当前用户对应的 SWE 业务 namespace 是什么？
6. 应使用 kubectl、brainctl 还是平台控制台创建 Pod？
7. 是否可以创建 CPU-only 探针 Pod？
8. Worker ServiceAccount 和 imagePullSecret 应使用哪些名称？
9. session Pod 是否必须走 Volcano Job/queue？
10. 导入完成后，Worker 应使用哪个 image ref 和 digest？
```

## 13. 推荐下一步

按优先级建议：

```text
1. 优先获取 Registry push 权限；这是最符合 Kubernetes imagePull 工作流的路径。
2. 若不能 push，申请平台执行共享存储 tar 导入任务，并返回 image ref/digest。
3. 同时确认业务 namespace 和最小 Worker RBAC。
4. 以最终 Worker ServiceAccount 创建低资源 CPU-only 探针 Pod。
5. 在探针 Pod 中验证镜像拉取、ARM64 架构、Ready、log、exec、delete。
6. 再部署单 Worker/Gateway，验证 Kubernetes session Pod 生命周期。
7. 之后才进入真实 Smith 镜像、catalog、official harness 和 non-gold episode 验证。
```

## 14. 当前一句话结论

```text
ARM64 Worker 镜像已在本地构建并完整上传到浦江共享存储，但当前既没有 Registry push 权限，也没有共享 tar 到集群镜像存储的导入流程和可创建 Pod 的业务 namespace/RBAC，因此暂时无法将镜像真正用于 Kubernetes Worker 部署。
```

## 15. 新增架构结论（2026-08-19）

平台最新反馈确认：

```text
集群可用节点架构：x86/AMD64
ARM64 节点：当前不可用
平台镜像示例：registry.h.pjlab.org.cn/ailab-sys/persisting:sys_cpu_task
Registry push：只有实验室内部人员具备权限
后续 push：需要由管理员代为执行
```

因此之前构建的 ARM64 Worker 镜像不应继续作为浦江 Worker 部署镜像。下一步应在 x86 环境重新构建：

```text
目标平台：linux/amd64
目标镜像建议：registry.h.pjlab.org.cn/ailab-sys/uenv-worker:amd64-dev
```

本地 Mac 上的 Docker buildx 已确认可以解析并拉取 `dockerproxy.net/library/rust:1.96-bookworm` 的 `linux/amd64` 基础镜像，并已完成本地 amd64 Worker 构建。

管理员需要执行或协助执行：

```text
1. 在 x86 CPU 工作空间内获取代码和 vendor；
2. 使用可用的 amd64 Rust 构建基础镜像构建 Worker；
3. 将镜像标记为 registry.h.pjlab.org.cn/ailab-sys/uenv-worker:amd64-dev；
4. 由实验室内部账号 push；
5. 返回远端 manifest digest；
6. 确认业务 namespace 的 Pod 可以 pull 该镜像。
```

Kubernetes Worker 和后续 Smith session 镜像均必须单独核对 `linux/amd64`，不能仅凭本地 ARM64 镜像归档或镜像 tag 判断可部署。

本地构建结果：

```text
基础镜像 manifest：sha256:a339861ae23e9abb272cea45dfafde21760d2ce6577a70f8a926153677902663
amd64 基础镜像子 manifest：sha256:d99f7b31f49909348dc59b51f3c95d1efded1701ffb222f095aaab7de3c4abd8
本地 tag：uenv-worker:amd64-dev
架构：linux/amd64
本地 image ID / manifest list digest：sha256:60d46fb1328b7d484d67f9dd4153c692e9ac62759a7c1495af92a0986fd72d96
Docker 逻辑大小：5339488549 bytes
归档：/var/folders/d6/sqz5sk8s7wxd8wl36rfs5hv80000gn/T/opencode/uenv-worker-amd64-dev.tar
归档 SHA-256：015d55ab9aec6f34f57a82e4b349673b83f92bd3487eedb177fb59788198fe13
```

因此后续不需要管理员重新构建，只需要使用上述 amd64 镜像或归档完成 push/导入，并返回远端 manifest digest。

### 16.1 共享存储镜像替换结果

当前 SSH 工作空间对 `/mnt/shared-storage-user/evobox-share/uenv` 具备写权限，已完成旧归档替换：

```text
已上传：uenv-worker-amd64-dev.tar
远端大小：5339512320 bytes
远端 SHA-256：015d55ab9aec6f34f57a82e4b349673b83f92bd3487eedb177fb59788198fe13
已删除：uenv-worker-arm64-dev.tar
```

共享目录当前仅保留最新的 AMD64 Worker 镜像归档。删除旧归档后，挂载点显示可用空间约 2.4T；GPFS 的空间显示可能受共享盘其他任务并发写入影响，不能将变化全部归因于本次删除。

## 17. 7143 全量 Smith 镜像清单（2026-08-20）

根据 `secrets/README.md` 的连接方式，已连接现有 A100 Worker `7143`，直接读取 Docker 镜像列表，并筛选官方 SWE-smith 前缀：

```text
swebench/swesmith.x86_64.
```

实测结果：

```text
222 个 unique image
222 个均为 x86_64 命名空间
目标集群架构：linux/amd64
源主机：7143
```

这确认 7143 上确实存在完整的 222 个源镜像，但 7143 位于浦江集群外部，不能据此推断它能访问
`registry.h.pjlab.org.cn`。管理员需要先将镜像或归档传入具备浦江内网 Registry 访问权限的环境，
 再由该内部环境执行导入和 push；不能在 7143 上直接执行面向浦江 Registry 的 `docker login`/`docker push`。

此前关于公开 Docker Hub `jyangballin/swesmith...` 的表述需要修正：它可以作为可能的公开
下载源，但不能直接作为最终运行引用。历史验收已经确认 `jyangballin/swesmith...` 与官方
harness 使用的 `swebench/swesmith...` 存在 namespace/内容对齐风险；当前 7143 的 222 个
活动镜像、catalog 和 `images.manifest.json` 均使用 `swebench/swesmith...`。任何从公开源
重新下载的镜像，都必须先与 7143 对应镜像核对 digest，再转为内部 `swebench/...` 引用。

已生成交付物：

```text
Docs/worker/260819/swesmith-registry/smith-images.registry-manifest.json
Docs/worker/260819/swesmith-registry/smith-images.registry-map.tsv
Docs/worker/260819/swesmith-registry/admin-push-swesmith-images.sh
Docs/worker/260819/swesmith-registry/verify-swesmith-registry.sh
Docs/worker/260819/swesmith-registry/README.md
```

清单内容 digest：

```text
sha256:34d87280d7830336e85613823c2dcc72d6cbb6b06478acc908b993f7add5916c
```

目标 Registry 映射为：

```text
swebench/swesmith.x86_64.<name>:latest
  -> registry.h.pjlab.org.cn/ailab-sys/swesmith.x86_64.<name>:latest
```

当前已完成源镜像发现、去重、映射和脚本生成；Registry push 仍需实验室内部管理员在浦江内网环境中执行。
下一步必须先确认 7143 到该内网环境的合法镜像传输或导入方式。
