# Worker 镜像管理员推送请求

当前浦江集群节点为 `linux/amd64`，请不要推送此前的 ARM64 Worker 镜像用于部署。

本地 `linux/amd64` 构建已完成，管理员现在只需要在浦江内网环境执行导入和 push，不需要重新构建。7143 只是源镜像所在主机，不能假设它能访问浦江 Registry。

## 请求内容

```text
目标镜像：registry.h.pjlab.org.cn/ailab-sys/uenv-worker:amd64-dev
目标平台：linux/amd64
构建上下文：UEnv 仓库根目录
Dockerfile：deploy/kubernetes/uenv-swe/Dockerfile
```

本地镜像信息：

```text
本地 tag：uenv-worker:amd64-dev
本地 image ID / manifest list digest：sha256:60d46fb1328b7d484d67f9dd4153c692e9ac62759a7c1495af92a0986fd72d96
架构：linux/amd64
Docker 逻辑大小：5339488549 bytes
归档：/var/folders/d6/sqz5sk8s7wxd8wl36rfs5hv80000gn/T/opencode/uenv-worker-amd64-dev.tar
归档 SHA-256：015d55ab9aec6f34f57a82e4b349673b83f92bd3487eedb177fb59788198fe13
```

如果管理员直接拿归档导入，请使用上述 tar，并在导入后重新返回 Registry manifest digest。

构建命令：

```bash
docker buildx build \
  --platform linux/amd64 \
  --tag registry.h.pjlab.org.cn/ailab-sys/uenv-worker:amd64-dev \
  --push \
  -f deploy/kubernetes/uenv-swe/Dockerfile .
```

如果构建环境不能直接访问 Dockerfile 中的基础镜像：

```text
请替换为平台可访问且已验证的 linux/amd64 Rust 1.96 Bookworm 基础镜像，
但需记录替换后的基础镜像引用和最终 Worker manifest digest。
```

## 推送后请返回

```text
1. 最终镜像完整引用；
2. manifest digest；
3. amd64 架构确认；
4. Registry push 是否成功；
5. 业务 namespace 是否可以 pull；
6. 如需要 imagePullSecret，请提供 Secret 名称和使用方式。
```

## SWE-smith 全量镜像准备

当前决定提前准备 SWE-smith 全量 `222` 个 unique 镜像。该动作只表示镜像预置，不表示同时创建 `59136` 个 session 或预开容器。

管理员需要基于最终 Smith catalog 和镜像清单批量处理：

```text
1. 确认所有镜像可在浦江集群使用，架构为 linux/amd64；
2. 将镜像推送到实验室内部 Registry 的统一 project；
3. 为每个源镜像保留 source image、目标 image ref 和 Registry digest；
4. 返回 222 个镜像的 push 结果和 digest manifest；
5. 确认目标业务 namespace 的 kubelet/Worker ServiceAccount 可以 pull；
6. 不在此阶段启动全量 Pod，仅完成镜像可拉取准备。
```

全量准备前必须先确认：

```text
Smith catalog 最终版本和 59136 条记录；
222 个 unique image refs 的完整清单；
源镜像到目标 Registry image ref 的确定性映射；
每个镜像的 linux/amd64 架构和 digest；
总 Registry 存储量约 290GB 的配额和保留策略。
```

不能只批量修改 tag 而不建立映射，否则会复现 `instance_id` 能命中 catalog、但 `image_cache_key` 无法 provision 的历史问题。

## 注意

```text
不要使用 registry2.d.pjlab.org.cn/uenv/uenv-worker:arm64-dev 作为集群 Worker 镜像。
不要把本地 ARM64 镜像 tag 直接改名为 amd64。
必须重新构建 linux/amd64 镜像。
```
