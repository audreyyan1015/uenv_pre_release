# Hub 控制台信息架构重构与 SWE-smith 完整包入库

> 日期：2026-08-06（联调收口 2026-08-07）  
> 分支：`feature/worker-pool-260728_HubEpisodeStackRubric`  
> 关联：
> - [AgentENV 对比分析与 Hub 控制台](./260804-AgentENV对比分析与Hub控制台.md)
> - [SWE-smith 变更与联调报告](../worker/260801/SWE-smith变更与联调报告.md)
> - [SWE-smith 全量 catalog 补齐](../worker/260802/SWE-smith全量catalog补齐与Worker重启报告.md)

---

## 1. 结论

| 项 | 结果 |
|---|---|
| 控制台信息架构 | 由「表结构平铺」改为「可运行 → 构件 → 存储 → 运维」 |
| 环境层级 | `swe` 是能力契约；verified / pro / smith 是其 dataset 变体，不再与契约平级 |
| 错位环境治理 | `swebenchpro` / `dscodebench` / `olymmath` / `scitab` / `pubmedqa` 标为 deprecated，指向对应契约 |
| 环境包角色 | API 新增派生字段 `kind`（benchmark / agent_scaffold / rubric / image_bundle / fixture） |
| SWE-smith 完整包 | `swe-bench-smith@0.2.0`：10 仓库、8226 实例、10 个 Hub 托管 image_tar（≈13.8 GiB） |
| 零外拉 | Worker 可从 Hub `docker load`，无需访问 Docker Hub / 镜像代理 |

一句话：导航反映的是「怎么跑起来」，而不是「数据库有几张表」；同时把一个仓库完备的 SWE-smith 基准真正放进了 Hub。

---

## 2. 为什么旧导航乱

旧侧栏把五类不同性质的对象平铺在「注册内容」下：

| 旧入口 | 实际是什么 | 问题 |
|---|---|---|
| 环境 | 能力契约 + 被误注册成环境的 benchmark | `swe` 与 `swebenchpro` 同级，层级塌了 |
| 环境包 | 任务数据 / Agent 脚手架 / 评分契约 / 纯镜像 / fixture | 一种分发单元被当成一类东西 |
| Episode Stack | 配方（引用清单） | 与原料平级，看不出依赖方向 |
| Agent Bridge | 脚手架包按 `agent_kind` 的投影 | 同一对象被数两遍 |
| SWE 实例目录 | `swe` 一个环境的题目明细 | 把字段提升成顶级概念 |
| 脚手架模板 | `uenv env init` 用 | 低频，不应与可运行组合同级 |

### 2.1 概念关系（正确层级）

```text
Episode Stack（可运行组合）
├── 环境契约（swe / qa / code / …）     ← 「一次 reset/step 是什么」
├── 基准数据集（swe-bench-smith@0.2.0） ← 「考哪些题 + 镜像 tar」
├── Agent 脚手架（uenv-agent-openhands）← 「怎么答」
└── Runtime Gateway 要求               ← 「命令往哪路由」

制品与镜像 = 上述包里的内容寻址字节（catalog / eval_spec / image_tar …）
```

- **环境包**是分发单元：目录、镜像清单、评测规格、Worker overlay 一起版本化；镜像字节按摘要引用，不进 SQLite。
- **Episode Stack**是配方：自己不含字节，只按版本钉死「这次用哪几个包怎么组合」。
- **Agent Bridge**不是独立实体：凡声明了 `agent_kind` 的环境包会出现在该投影里。

### 2.2 `swe` 是否包含 smith / pro

是。`swe@0.1.0` 的 `config_schema.dataset` 枚举为：

```json
["swe-bench-verified", "swe-bench-pro", "swe-bench-smith"]
```

生产库里另有一个 `swebenchpro` 环境类型，与 `swe` 的 `swe-bench-pro` 变体重叠——这是历史重复注册，本轮用 lifecycle=deprecated + superseded_by=swe 归并，旧引用仍 200 可解析。

---

## 3. 控制台改动

### 3.1 新侧栏

| 分组 | 入口 | 数据来源 |
|---|---|---|
| 运行态 | 总览 / 健康与指标 | `/api/v1/system/overview`、`/healthz`、`/metrics` |
| 可运行 | Episode Stack | `/api/v1/episode-stacks` |
| 构件 | 基准与数据集 / Agent 脚手架 / 环境契约 | packages（按 kind）+ agent-bridges + envs |
| 存储 | 制品与镜像 | 各包 latest manifest 的 artifacts 汇总 |
| 运维 | 搜索 / 审计 / 脚手架模板 / 连接与凭据 | 既有 API |

### 3.2 API 增量（向后兼容）

`PackageSummary` 增加派生字段（不入库，由 latest manifest 计算）：

- `kind`: `benchmark` \| `agent_scaffold` \| `rubric` \| `image_bundle` \| `fixture` \| `other`
- `env_type`: 基准供给的能力契约（如 `swe`）
- `instance_count`: overlay 声明的实例数（若有）

`RegistryStats` 增加：

- `packages_by_kind`
- `active_envs`（排除 deprecated）

### 3.3 数据治理（seed 启动时执行）

对已存在的错位环境打标（不新建、不删除）：

| 旧 env_type | superseded_by |
|---|---|
| swebenchpro | swe |
| dscodebench | code |
| olymmath / scitab / pubmedqa | qa |

语义与既有 `math` → `qa` 改名相同：旧名带 `Deprecation: true` 头仍可解析，Worker 预热不破。

---

## 4. SWE-smith@0.2.0：仓库完备基准

### 4.1 范围选择依据

| 约束 | 数值 |
|---|---:|
| Hub 磁盘 | 99 G，artifacts 已占约 15 G |
| 全量 SWE-smith | 59136 实例 / 222 镜像 ≈ 290 G 级 |
| 本包范围 | **10 个 Python 仓库的全部实例**（无抽样） |

SWE-smith 镜像按**仓库**构建（`jyangballin/swesmith.x86_64.*`），bug 以补丁在运行时注入。因此「仓库完备」是可验证的最小完整单元：包内列出的每一条实例，都能从本包镜像 tar 跑起来。

### 4.2 包内容

| 项 | 值 |
|---|---:|
| package_id / version | `swe-bench-smith` / `0.2.0` |
| 仓库数 | 10 |
| 实例数 | 8226（有效题面 7668，空题面 558） |
| 镜像 tar | 10（合计 ≈ 13.78 GiB） |
| catalog.json | ≈ 568 MiB（compact JSON） |
| image_pull_policy | `local_only` + `load_images_from_package` |

覆盖仓库：jinja、starlette、pydantic、sqlglot、trio、conan、deepdiff、oauthlib、sunpy、gpxpy。

### 4.3 构建与发布流水线

1. 构建机 `121.89.82.128`：从 hf-mirror 拉 parquet → `scripts/build_swesmith_envpackage.py` 生成 catalog / images.manifest / eval_spec / overlay；`docker pull` + `docker save` 产出 tar。  
2. rsync（`setsid` 持久任务）到 Hub 暂存：`/root/hub-staging/swe-bench-smith-0.2.0/`（2026-08-06 04:33 传完）。  
3. SHA256 核验 10/10 通过后，`scripts/publish_swesmith_hub_package.py` 以 `file_artifacts` 流式入库。  
4. Episode Stack `swe-bench-smith-openhands@1.1.0` 钉住 `swe-bench-smith@0.2.0`。

### 4.4 与「环境创建」示例的关系

用户给出的 Verified 单实例流程（`psf__requests-1142` → `import-docker` → manifest）是**契约侧**标准化路径。本轮交付的是**数据集侧**完整包：同一 `swe` 契约下，把 smith 变体的任务数据 + 镜像字节一并版本化进 Hub，消费侧 `uenv env sync` + `docker load` 即可零外拉。

---

## 5. 真机联调检查清单

| 检查 | 期望 |
|---|---|
| `GET /console` | 浅色主题；侧栏为「可运行 / 构件 / 存储 / 运维」 |
| `GET /api/v1/packages?per_page=200` | 每项带 `kind`；smith 为 `benchmark`，`instance_count=8226` |
| `GET /api/v1/packages/swe-bench-smith/versions/0.2.0` | ≥14 个 artifacts，含 10 个 `image_tar` |
| `GET .../sync-plan` | bundle_digest 稳定；image_tar 可下载且摘要匹配 |
| `GET /api/v1/envs` | 在用契约与「已归并历史名」分区；`swebenchpro` 等为 deprecated |
| `GET /api/v1/episode-stacks/swe-bench-smith-openhands/versions/latest/resolve` | env_package 解析到 `0.2.0` |
| `scripts/verify-hub-console-e2e.sh` | 静态资源 + overview + 渲染回归 + 对比度审计通过 |

---

## 6. 脚本与产物路径

| 路径 | 用途 |
|---|---|
| `scripts/build_swesmith_envpackage.py` | 从 parquet + tar 构建包目录 |
| `scripts/publish_swesmith_hub_package.py` | Hub 本机流式发布 |
| Hub 暂存 | `/root/hub-staging/swe-bench-smith-0.2.0/` |
| Hub artifacts | `/root/uenv/uenv-hub/data/artifacts/swe-bench-smith/0.2.0/` |

---

## 7. 回滚

```bash
# 撤回 0.2.0 后 latest 回到 0.1.0 smoke
curl -X POST -H "Authorization: Bearer $UENV_HUB_TOKEN" \
  $UENV_HUB_ENDPOINT/api/v1/packages/swe-bench-smith/versions/0.2.0/yank \
  -H 'Content-Type: application/json' \
  -d '{"reason":"rollback"}'
```

控制台二进制回滚：恢复 `/root/uenv-public-deploy` 中备份的 `uenv-hub-server` 与 `hub.public.toml` 指向即可。
