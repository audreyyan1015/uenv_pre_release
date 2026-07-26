# SWE-bench-Pro UEnv 路径异常样例核验说明

## 1. 背景

当前 SWE-bench-Pro UEnv 全量评测建议先保留继续运行，不因已观察到的 `resolved=false` 直接中断。当前更需要核验的是：部分样例里 OpenHands/模型可能创建或修改了与目标项目目录不匹配的文件路径，导致 Worker 最终返回 `resolved=false`。

当前结果目录：

```text
/data/ronghao/uenv/uenv-bridge/temp/benchmarks/swebenchpro/qwen3_6_35b_a3b_uenv_full_thinking_max32768_budget16384_restart_20260719_105613/
```

以下分析基于截至 `2026-07-19 17:05 CST` 已写入的 UEnv 结果，以及已有的直接生成 patch 与数据集 gold patch 对照。Adapter 侧当前没有收到完整 OpenHands 轨迹和最终 patch，因此本文列出的路径异常用于提示 Worker 侧按 `request_id` 回查实际轨迹、最终 patch、测试输出和工作目录。

## 2. 已观察到的路径异常证据

### 2.1 qutebrowser 历史验收记录

历史 SWE OpenHands 验收中，qutebrowser 已经出现过类似问题：

```text
qutebrowser gold: reward=1.0, tests=56/56
qutebrowser gold + 轨迹: reward=1.0, tests=56/56
qutebrowser llm 单轮: reward=0.0, tests=52/56；幻觉 patch 路径
```

来源：

```text
/data/ronghao/uenv/Docs/older/260627-swe-openhands-acceptance-report.md
```

### 2.2 代表性路径不匹配样例

下表中的 `生成路径` 来自已有直接生成结果：

```text
/data/ronghao/uenv/uenv-bridge/temp/benchmarks/swebenchpro/qwen3_6_35b_a3b_full/patches.json
```

`Gold 路径` 来自 SWE-bench-Pro 数据集：

```text
/data/ronghao/uenv/uenv-bridge/data/benchmarks/swebenchpro/test.jsonl
```

`request_id` 来自当前 UEnv 全量结果，Worker 侧可据此回查同一类样例在 OpenHands 中实际创建/修改了哪些文件。

| repo | instance_id / request_id | Gold 路径 | 生成路径 | 异常类型 |
|---|---|---|---|---|
| `qutebrowser/qutebrowser` | `instance_qutebrowser__qutebrowser-f91ace96223cac8161c16dd061907e138fe85111-v059c6fdc75567943479b23ebca7c07b5e9a7f34c`<br>`swebenchpro-instance_qutebrowser__qutebrowser-f91ace96223cac8161c16dd061907e138fe85111-v059c6fdc75567943479b23ebca7c07b5e9a7f34c-7d16ba1b` | `qutebrowser/browser/qtnetworkdownloads.py`<br>`qutebrowser/utils/log.py`<br>`qutebrowser/utils/qtlog.py` | `tests/unit/utils/test_qtlog.py`，且是从 `/dev/null` 新建 | 只新增测试文件，没有修改目标源码文件。 |
| `navidrome/navidrome` | `instance_navidrome__navidrome-7073d18b54da7e53274d11c9e2baef1242e8769e`<br>`swebenchpro-instance_navidrome__navidrome-7073d18b54da7e53274d11c9e2baef1242e8769e-37992a82` | `core/agents/interfaces.go`<br>`core/agents/lastfm/agent.go`<br>`core/agents/lastfm/auth_router.go`<br>`core/agents/lastfm/client.go` 等 | `internal/domain/lastfm/lastfm.go` | 生成路径与 gold 所在 `core/agents/...` 子树完全不重合，疑似项目结构或版本认知错误。 |
| `future-architect/vuls` | `instance_future-architect__vuls-407407d306e9431d6aa0ab566baa6e44e5ba2904`<br>`swebenchpro-instance_future-architect__vuls-407407d306e9431d6aa0ab566baa6e44e5ba2904-9ab61f53` | `contrib/trivy/pkg/converter.go` | `report/scan.go` | 目标应在 Trivy converter 子模块，生成结果落在另一个报告子系统。 |
| `protonmail/webclients` | `instance_protonmail__webclients-2c3559cad02d1090985dba7e8eb5a129144d9811`<br>`swebenchpro-instance_protonmail__webclients-2c3559cad02d1090985dba7e8eb5a129144d9811-f165a6c9` | `packages/components/containers/payments/planCustomizer/ProtonPlanCustomizer.tsx`<br>`packages/components/hooks/assistant/assistantUpsellConfig.ts`<br>`packages/components/hooks/assistant/useAssistantUpsellConfig.tsx`<br>`packages/components/payments/core/index.ts` 等 | `src/payment/components/assistant/AssistantUpsell.tsx`<br>`src/payment/components/subscription/core/index.ts` 等 | monorepo 前缀错位：生成结果使用 `src/...`，gold 使用 `packages/components/...`。 |
| `flipt-io/flipt` | `instance_flipt-io__flipt-c12967bc73fdf02054cf3ef8498c05e25f0a18c0`<br>`swebenchpro-instance_flipt-io__flipt-c12967bc73fdf02054cf3ef8498c05e25f0a18c0-2de64eb7` | `internal/cmd/grpc.go`<br>`internal/server/auth/middleware.go`<br>`internal/server/middleware/grpc/middleware.go` | `rpc/flipt/auth/middleware.go` | 包路径错位：目标应在 `internal/server/...`，生成结果落在 `rpc/flipt/...`。 |

这些例子并不都能证明 Worker 当前这轮 UEnv 运行一定创建了同样路径，因为 Adapter 侧没有收到最终 patch。但它们说明同一批 SWE-bench-Pro 样例中存在多种路径错位风险，建议 Worker 侧按上表 request_id 核验当前 UEnv 轨迹。

## 3. 当前全量中需要 Worker 核验的 qutebrowser request

除上表中第一个 qutebrowser 代表样例外，当前全量里还有多条 qutebrowser request 已完成但 `resolved=false`。请 Worker 侧一并查询 OpenHands 轨迹、patch 和测试输出。

| request_id | instance_id | status | resolved |
|---|---|---|---|
| `swebenchpro-instance_qutebrowser__qutebrowser-f91ace96223cac8161c16dd061907e138fe85111-v059c6fdc75567943479b23ebca7c07b5e9a7f34c-7d16ba1b` | `instance_qutebrowser__qutebrowser-f91ace96223cac8161c16dd061907e138fe85111-v059c6fdc75567943479b23ebca7c07b5e9a7f34c` | `completed` | `false` |
| `swebenchpro-instance_qutebrowser__qutebrowser-c580ebf0801e5a3ecabc54f327498bb753c6d5f2-v2ef375ac784985212b1805e1d0431dc8f1b3c171-47300671` | `instance_qutebrowser__qutebrowser-c580ebf0801e5a3ecabc54f327498bb753c6d5f2-v2ef375ac784985212b1805e1d0431dc8f1b3c171` | `completed` | `false` |
| `swebenchpro-instance_qutebrowser__qutebrowser-f631cd4422744160d9dcf7a0455da532ce973315-v35616345bb8052ea303186706cec663146f0f184-f5d05ddb` | `instance_qutebrowser__qutebrowser-f631cd4422744160d9dcf7a0455da532ce973315-v35616345bb8052ea303186706cec663146f0f184` | `completed` | `false` |
| `swebenchpro-instance_qutebrowser__qutebrowser-96b997802e942937e81d2b8a32d08f00d3f4bc4e-v5fc38aaf22415ab0b70567368332beee7955b367-e5ace551` | `instance_qutebrowser__qutebrowser-96b997802e942937e81d2b8a32d08f00d3f4bc4e-v5fc38aaf22415ab0b70567368332beee7955b367` | `completed` | `false` |
| `swebenchpro-instance_qutebrowser__qutebrowser-fd6790fe8c02b144ab2464f1fc8ab3d02ce3c476-v2ef375ac784985212b1805e1d0431dc8f1b3c171-9faf9b7a` | `instance_qutebrowser__qutebrowser-fd6790fe8c02b144ab2464f1fc8ab3d02ce3c476-v2ef375ac784985212b1805e1d0431dc8f1b3c171` | `completed` | `false` |
| `swebenchpro-instance_qutebrowser__qutebrowser-0fc6d1109d041c69a68a896db87cf1b8c194cef7-v2ef375ac784985212b1805e1d0431dc8f1b3c171-20f38aad` | `instance_qutebrowser__qutebrowser-0fc6d1109d041c69a68a896db87cf1b8c194cef7-v2ef375ac784985212b1805e1d0431dc8f1b3c171` | `completed` | `false` |
| `swebenchpro-instance_qutebrowser__qutebrowser-70248f256f93ed9b1984494d0a1a919ddd774892-v2ef375ac784985212b1805e1d0431dc8f1b3c171-752d5fc2` | `instance_qutebrowser__qutebrowser-70248f256f93ed9b1984494d0a1a919ddd774892-v2ef375ac784985212b1805e1d0431dc8f1b3c171` | `completed` | `false` |

## 4. 请 Worker 侧查明的问题

请重点确认上面这些 request 中，OpenHands 实际工作目录、最终 patch 路径、测试执行目录是否和 SWE-bench-Pro 的 `repo/base_commit/dockerhub_tag` 一致。

需要核验的路径异常包括：

1. 是否创建了明显不属于目标 repo 的通用路径，例如：

```text
/app/app.py
/app/routes/...
/workspace/app.py
/workspace/routes/...
app.py
routes/...
```

2. 是否只新增或修改测试文件，但没有修改 gold patch 对应的源码文件。
3. 是否出现 monorepo 前缀错位，例如应该修改 `packages/components/...`，但实际修改 `src/...` 或 `frontend/src/...`。
4. 是否出现 repo 内部子系统错位，例如应该修改 `contrib/trivy/...`，但实际修改 `report/...`。
5. 是否出现包路径错位，例如应该修改 `internal/server/...`，但实际修改 `rpc/flipt/...`。

请进一步确认：

1. Worker 挂载的 workspace 是否是正确的目标仓库。
2. workspace 是否 checkout 到 request 中的 `base_commit`。
3. OpenHands 启动时的当前工作目录是仓库根目录，还是空目录 / 模板目录。
4. EnvPackage / catalog 中的 `instance_id` 是否映射到了错误镜像、错误 repo 或错误 base commit。
5. patch 收集逻辑是否把 agent 在错误目录下创建的文件也当成最终 patch。
6. prompt 或 tool context 中是否缺少文件树、当前工作目录、目标源码路径等信息，导致模型按记忆或通用项目结构生成路径。

## 5. 可能原因

路径不符可能来自以下几类原因：

1. **workspace 初始化错误**：Worker 没有进入目标 repo 根目录，OpenHands 在默认目录或空目录下创建文件。
2. **catalog / instance 映射错误**：`instance_id` 对应的镜像、repo、base commit 不一致，导致 agent 看到的项目不是请求里的项目。
3. **路径前缀不一致**：OpenHands 使用 `/app`，请求配置使用 `/workspace`，patch 收集或路径归一化时发生错位。
4. **monorepo 根目录理解错误**：模型或工具上下文没有明确当前项目的真实源码根目录，导致 `src/...`、`frontend/src/...`、`packages/...` 混用。
5. **模型幻觉路径**：模型没有正确理解目标仓库结构，生成了通用 Web 项目路径，如 `app.py`、`routes/`。
6. **测试文件与源码文件混淆**：模型只新增/修改测试文件，但没有修改实际源码文件，最终自然无法通过官方 resolved。
7. **patch 收集范围过宽**：如果 patch 收集没有限定到目标 repo 根目录，可能把错误目录里的文件创建也收集进最终 patch。

我的建议是：先保留当前全量继续跑，同时把第 2 节和第 3 节的 request_id 发给 Worker 侧，让他们查明 OpenHands 实际创建/修改路径是否与目标项目目录一致。
