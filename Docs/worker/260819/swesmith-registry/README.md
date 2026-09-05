# SWE-smith Registry Preparation

这组文件来自浦江外部 A100 Worker `7143` 的实际 Docker 镜像列表，不是本地 5 条 smoke catalog 推导结果。

重要边界：`7143` 只是源镜像所在主机，不是浦江集群内网，也不代表能够访问
`registry.h.pjlab.org.cn`。不能在 7143 上直接执行面向浦江 Registry 的 `docker login`
或 `docker push`。

## Source

在 7143 上执行：

```bash
docker images --format '{{.Repository}}:{{.Tag}}\t{{.ID}}\t{{.Size}}'
```

筛选前缀：

```text
swebench/swesmith.x86_64.
```

结果为 `222` 个 unique 镜像，全部标记为 `linux/amd64`，与浦江集群 x86 节点架构匹配。

## Files

```text
smith-images.registry-manifest.json
  完整 source image -> target Registry image 映射、7143 image ID、大小、架构和状态。

smith-images.registry-map.tsv
  便于管理员审阅、导入表格或批量处理。

admin-push-swesmith-images.sh
  在拥有源镜像和 Registry push 权限的机器上执行 tag + push。

verify-swesmith-registry.sh
  push 后检查目标镜像是否存在并包含 linux/amd64 manifest。
```

## Target mapping

当前目标 Registry project：

```text
registry.h.pjlab.org.cn/ailab-sys
```

示例：

```text
source:
swebench/swesmith.x86_64.oauthlib_1776_oauthlib.1fd52536:latest

target:
registry.h.pjlab.org.cn/ailab-sys/swesmith.x86_64.oauthlib_1776_oauthlib.1fd52536:latest
```

目标 repository 名称保留完整 `swebench/swesmith...` 镜像名，避免不同 `image_cache_key` 因缩写或重命名发生冲突。

## Namespace Warning

不要把公开 Docker Hub 上的 `jyangballin/swesmith...` 直接当成 UEnv 的最终官方镜像引用。
仓库记录了一个实际的 reward 对齐问题：7143 曾使用 `jyangballin/swesmith...`，而官方
harness 使用 `swebench/swesmith...`，同一实例的 gold 结果不一致。当前代码、catalog、
`images.manifest.json` 和 7143 活动镜像均已统一到：

```text
swebench/swesmith.x86_64.*
```

因此如果管理员从 Docker Hub 公开源下载 `jyangballin/swesmith...`，必须先核对其内容
digest 与 7143 对应的 `swebench/swesmith...` 镜像一致，并在内部 Registry 中使用目标
`swebench/...` 映射。不能仅凭镜像名称相似就替换 namespace。

## Push procedure

管理员需要先把 7143 上的源镜像或镜像归档传入能够访问浦江内网 Registry 的内部环境，
然后在该内部环境登录 Registry 并执行：

```bash
docker login registry.h.pjlab.org.cn
bash admin-push-swesmith-images.sh smith-images.registry-manifest.json
```

上述脚本不能直接在 7143 执行，除非平台明确提供到浦江 Registry 的网络路径和 push 权限。
也不要在当前开发机直接执行，因为当前开发机没有这 222 个源镜像，也没有 Registry push 权限。

## Correct Transfer Flow

```text
7143 Docker runtime
  -> 管理员在 7143 导出镜像/生成归档
  -> 通过平台允许的内网传输或共享存储交给浦江内部管理员
  -> 浦江内网管理员导入到其 Docker/containerd runtime
  -> 浦江内网管理员登录 registry.h.pjlab.org.cn
  -> tag + push 222 个 linux/amd64 镜像
  -> 返回 Registry digest manifest
```

具体的 7143 到浦江内网传输方式需要管理员确认，不能由本清单假设为 SSH 直连、Registry pull
或共享路径自动导入。

## Verification output

管理员完成 push 后，应保存：

```text
每个 target image 的 Registry digest
每个 target image 的 linux/amd64 架构
push 成功/失败状态
失败原因和重试结果
```

当前生成清单的内容 digest：

```text
sha256:34d87280d7830336e85613823c2dcc72d6cbb6b06478acc908b993f7add5916c
```

该 digest 只标识本次清单内容，不是 Registry 镜像 digest。
