# SWE-smith 镜像命名空间不一致 Worker 处理说明

> 日期：2026-08-11
> 关联训练：`verl_swesmith_grpo_train_20260810_233010`
> 关联 Worker：7143 / `worker-7143-pro`
> 关联报告：`Docs/worker/260802/SWE-smith全量catalog补齐与Worker重启报告.md`

## 1. 需要 Worker 侧处理什么

请 Worker 侧核验并修复 7143 机器上 SWE-smith Docker 镜像的 tag 命名空间。

当前训练请求和 Worker 新代码使用官方 SWE-smith 镜像名：

```text
swebench/swesmith.x86_64.<owner>_1776_<repo>.<commit8>:latest
```

但 7143 本地大部分已缓存镜像仍是旧命名空间：

```text
jyangballin/swesmith.x86_64.<owner>_1776_<repo>.<commit8>:latest
```

这会导致 catalog 已经能命中 instance，但容器启动阶段仍然报本地镜像不存在。

## 2. 我们遇到的问题

全量 SWE-smith GRPO 训练中，大量 episode 在 Worker 早期失败，典型错误如下：

```text
image `swebench/swesmith.x86_64.hypermodeinc_1776_ristretto.da570116:latest`
not present locally and image_pull_policy=local_only
```

这不是此前的 `not in catalog (size=736)` 问题。此前 Worker 报告已经说明 catalog 从 smoke 子集补齐到 SWE-smith 全量 catalog。本轮问题发生在更后一步：Worker 已经找到 instance，但按 `image_cache_key` 检查 Docker 镜像时找不到官方前缀的 tag。

## 3. 证据

7143 上检查 `swebench/swesmith` 前缀时只有 1 个镜像：

```bash
docker images --format '{{.Repository}}:{{.Tag}}' \
  | grep '^swebench/swesmith' \
  | sort -u \
  | wc -l
```

结果：

```text
1
```

同时检查官方前缀和旧前缀时共有 223 个：

```bash
docker images --format '{{.Repository}}:{{.Tag}}' \
  | grep -E '(^swebench/swesmith|^jyangballin/swesmith)' \
  | sort -u \
  | wc -l
```

结果：

```text
223
```

针对失败样例 `hypermodeinc_1776_ristretto.da570116`，官方前缀不存在，但旧前缀存在：

```bash
docker image inspect swebench/swesmith.x86_64.hypermodeinc_1776_ristretto.da570116:latest >/dev/null && echo yes || echo no
docker image inspect jyangballin/swesmith.x86_64.hypermodeinc_1776_ristretto.da570116:latest >/dev/null && echo yes || echo no
```

实际结果：

```text
no
yes
```

这说明镜像层本身大概率已经在本机，只是缺少 Worker 当前使用的官方 tag。
