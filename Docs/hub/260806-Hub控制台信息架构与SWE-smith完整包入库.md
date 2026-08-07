Hub 控制台：http://8.130.95.176:8088/console

# Hub 控制台信息架构与 SWE-smith 完整包入库

> 日期：2026-08-06（联调收口 / 文档修订 2026-08-07）  
> 分支：`feature/worker-pool-260728_HubEpisodeStackRubric`  
> 关联：
> - [AgentENV 对比分析与 Hub 控制台](./260804-AgentENV对比分析与Hub控制台.md)
> - [SWE-smith 变更与联调报告](../worker/260801/SWE-smith变更与联调报告.md)
> - [SWE-smith 全量 catalog 补齐](../worker/260802/SWE-smith全量catalog补齐与Worker重启报告.md)

---

## 1. 结论

| 项 | 结果 |
|---|---|
| 控制台信息架构 | 由「表结构平铺」改为「运行态 → 可运行 → 构件 → 存储 → 运维」 |
| 概念层级 | 环境契约 → 基准数据集（变体）→ 制品；Stack 在最上层组合 |
| 环境包角色 | API 派生 `kind` / `env_type` / `dataset` / `instance_count`（不入库） |
| 错位环境治理 | 5 个误注册 env + 历史 `math` 标 deprecated，指向正式契约 |
| 基准页分组 | 固定 swe / qa / code；夹具单独区；消除「未声明」误桶 |
| SWE-smith | `swe-bench-smith@0.2.0`：10 仓、8226 实例、10 个 Hub 托管 `image_tar`（≈13.8 GiB） |
| 零外拉 | Worker 可从 Hub `docker load`，无需 Docker Hub / 镜像代理 |

一句话：导航反映「怎么跑起来」，注册模型反映「契约 / 数据 / 字节」三层，而不是「数据库有几张表」。

---

## 2. 术语与对象模型

Hub 里容易混的名字，按**是什么 / 不是什么 / 在哪看**对齐如下。

### 2.1 一张总表

| 名称 | 是什么 | 不是什么 | 控制台入口 | 典型例子 |
|---|---|---|---|---|
| **环境契约**（Task Environment / `env_type`） | 能力抽象：一次 `reset`/`step` 的语义、奖励怎么算、`config_schema` | 不是题目集，不是镜像包 | 构件 → 环境契约 | `swe` / `qa` / `code` |
| **基准数据集**（benchmark 包） | 某契约下的题目目录 + 评测规格 +（可选）镜像 tar；版本化分发单元 | 不是新的环境类型 | 构件 → 基准与数据集 | `swe-bench-smith@0.2.0` |
| **变体 / dataset** | 契约内的路由键，写入 Stack / Worker 配置 | 不是与契约平级的第四类环境 | 基准表「变体 / dataset」列 | swe 下 `verified`/`pro`/`smith` |
| **环境包**（Env Package） | 一切可版本化发布的分发单元（manifest + artifacts） | 不是单一业务概念；需再看 `kind` | 包详情 `#/packages/:id` | 基准 / 脚手架 / rubric / … |
| **Episode Stack** | 可运行配方：确定「契约 + 数据集 + 脚手架 + 网关要求」 | 自身不含字节 | 可运行 → Episode Stack | `swe-bench-smith-openhands@1.1.0` |
| **Agent 脚手架** | 声明了 `agent_kind` 的环境包（怎么答） | 不是 Bridge 实体本身 | 构件 → Agent 脚手架 | `uenv-agent-openhands` |
| **Agent Bridge** | 脚手架包按 `agent_kind` 的**投影视图** | 不是独立注册表行 | （投影，与脚手架同源） | `openhands` / `toolenv` |
| **Rubric** | 评分契约（按 dataset 路由 scorer） | 不是题目数据 | 包详情 / 制品侧可见 | `uenv-qa-rubric` |
| **制品 / 镜像** | 内容寻址字节：`catalog` / `eval_spec` / `image_tar` … | 不进 SQLite；按 digest 存 | 存储 → 制品与镜像 | smith 的 10 个 tar |
| **联调夹具**（fixture） | smoke / 预热用小包 | 不是正式训练基准 | 基准页底部「联调夹具」 | `math-smoke-fixtures` |
| **脚手架模板** | `uenv env init` 本地生成模板 | 不是线上可运行组合 | 运维 → 脚手架模板 | 低频运维入口 |
| **SWE 实例目录** | 某个 SWE 基准包内的题目明细 | 不是顶级导航概念 | 基准详情内嵌 | smith / verified 的实例浏览 |

### 2.2 依赖方向（正确读法）

```text
                    ┌─────────────────────────────┐
                    │     Episode Stack（配方）      │
                    │  自己不含字节，只钉版本引用     │
                    └──────────────┬──────────────┘
           ┌───────────────────────┼───────────────────────┐
           ▼                       ▼                       ▼
   环境契约（能力）          基准数据集（考题）         Agent 脚手架（答题）
   swe / qa / code         swe-bench-* / …           uenv-agent-*
           │                       │
           │                       ▼
           │              制品（内容寻址字节）
           │           catalog / eval_spec / image_tar
           ▼
   config_schema.dataset 枚举
   （声明本契约接受哪些路由键）
```

读法：

1. **先定契约**（交互形态），再选 **dataset 变体**（考哪些题），再选 **脚手架**（谁来答），最后用 Stack 确定组合。  
2. **环境包**是分发单位；**Stack** 是组合单位；**制品**是字节单位。三者不可互换。  
3. 历史里把 `olymmath`、`dscodebench` 等注册成独立 `env_type`，等于把「数据集」抬成了「契约」——层级塌陷的根因。

### 2.3 三个正式能力契约

| 契约 | 交互形态 | `config_schema` 里的 dataset 枚举（要点） | 线上基准包（2026-08-07） |
|---|---|---|---|
| **swe** | 容器内多轮修 bug；镜像按仓，补丁运行时注入 | `swe-bench-verified` / `swe-bench-pro` / `swe-bench-smith` | `swe-bench-verified` / `pro` / `smith` |
| **qa** | 单轮问答 + 判分（历史名 `math` 已归并） | gsm8k / pubmedqa / scitab / olymmath[-easy\|-hard] 等 | `olymmath` / `pubmedqa` / `scitab` |
| **code** | 生成代码并执行测试拿奖励 | `dscodebench` | `dscodebench` |

补充：

- `verified` / `pro` / `smith` **都是 swe 的变体**，不是三种环境。  
- 控制台「变体 / dataset」列展示的是**短键**（如 `smith`）；Stack / Worker 配置里常见完整路由名（如 `swe-bench-smith`）。二者同指一个变体，勿当成两套体系。  
- 另有 `dyn-openenv-prod`、`agent` 等环境仍可存在，但不进入「基准与数据集」页的三契约固定分区（该页只组织训练向基准）。

### 2.4 环境包 `kind`（角色分类）

`PackageKind::classify(manifest)` **运行时派生**，不写入 SQLite。判定顺序（越靠前越优先）：

| 顺序 | 条件 | kind | 含义 |
|---:|---|---|---|
| 1 | `agent_defaults.agent_kind` 非空 | `agent_scaffold` | Agent 脚手架 |
| 2 | 包名含 `fixture`/`smoke`，或 overlay 标 `fixture_package` | `fixture` | 联调夹具（即使带小 catalog） |
| 3 | artifacts 含 `catalog` | `benchmark` | 正式基准数据集 |
| 4 | 制品全是镜像 tar | `image_bundle` | 纯镜像包 |
| 5 | 包名含 `rubric` | `rubric` | 评分契约包 |
| 6 | 其余 | `other` | 未识别角色 |

线上清单（节选，与真机一致）：

| kind | package_id@version | env_type | dataset |
|---|---|---|---|
| benchmark | `swe-bench-smith@0.2.0` | swe | smith（8226 实例） |
| benchmark | `swe-bench-verified@1.0.0` | swe | verified |
| benchmark | `swe-bench-pro@0.3.4` | swe | pro |
| benchmark | `olymmath` / `pubmedqa` / `scitab` | qa | 同包名 |
| benchmark | `dscodebench@0.2.0` | code | dscodebench |
| fixture | `math-smoke-fixtures@0.1.0` | qa | math-smoke-fixtures |
| agent_scaffold | `uenv-agent-openhands` / `toolenv` | — | — |
| rubric | `uenv-qa-rubric` / `qa-rubric-align` | — | — |
| image_bundle | `echo-container-image(s)` | — | —（历史 echo，可后续 yank） |

### 2.5 `env_type` / `dataset` 如何从 manifest 推出

供基准页分组与 API 摘要使用（`PackageSummary`）：

**`env_type` 解析顺序**

1. overlay 显式 `env_type`（含 `swe|qa|code|math.env_type`）  
2. 契约子树：存在 `swe.benchmark_variant` → `swe`；存在 `code` → `code`；存在 `qa` 或 `math` → `qa`  
3. **包名约定**（消化脏 overlay）：`swe-bench-*` / `swebenchpro` → swe；`olymmath|pubmedqa|scitab|gsm8k` → qa；`dscodebench` → code  

说明：仅有 `{"swe":{"image_pull_policy":"local_only"}}` **不会**因此判成 swe（避免 QA 包被误粘贴的 pull policy 带偏）。此时走步骤 3，按包名归入 qa。

**`dataset` 解析顺序**

1. `swe.benchmark_variant` / `code.dataset` / `qa.dataset` / `math.dataset` / 顶层 `dataset`  
2. 包名 `swe-bench-<variant>` → `<variant>`  
3. 能推断出 env 的单数据集包 → 用 `package_id` 本身  

这就是原先控制台出现「未声明」的根因与修复：**脏 overlay + 旧前端把未知 env 丢进杂项桶**；现在固定展示三契约，并用包名兜底归类。

---

## 3. 控制台信息架构

### 3.1 侧栏（按「怎么跑起来」分组）

| 分组 | 入口 | 数据来源 | 人话 |
|---|---|---|---|
| 运行态 | 总览 / 健康与指标 | `/api/v1/system/overview`、`/healthz`、`/metrics` | 集群现在怎样 |
| 可运行 | Episode Stack | `/api/v1/episode-stacks` | 已经钉好、可以直接解析的组合 |
| 构件 | 基准与数据集 / Agent 脚手架 / 环境契约 | packages（按 kind）+ agent-bridges + envs | Stack 引用的三类原料 |
| 存储 | 制品与镜像 | 各包 latest manifest 的 artifacts 汇总 | 原料底下的字节 |
| 运维 | 搜索 / 审计 / 脚手架模板 / 连接与凭据 | 既有 API | 低频操作与凭据 |

### 3.2 「基准与数据集」页怎么读

页面固定三块 + 夹具，不再用「未声明」当第四契约：

1. **怎么读这一页**：契约 → 数据集 → 变体；并说明 Stack 再往上选一层。  
2. **SWE / QA / Code** 三张契约卡：每张有中文 blurb、包表（基准包 / 变体 / 版本 / 实例数）、跳转契约详情。  
3. **尚无法归入**（仅当仍有无法映射的 catalog 包时出现）：提示补 `worker_overlay.env_type` 或命名规范。  
4. **联调夹具**：`fixture` kind，不计入正式基准目录。

### 3.3 API 增量（向后兼容）

`PackageSummary` 派生字段：

| 字段 | 含义 |
|---|---|
| `kind` | 见 §2.4 |
| `env_type` | 基准/夹具供给的能力契约 |
| `dataset` | 契约内变体路由键（短键或包名） |
| `instance_count` | overlay 声明的实例数（若有） |

`RegistryStats` 增加：`packages_by_kind`、`active_envs`（排除 deprecated）。

---

## 4. SWE-smith@0.2.0：仓库完备基准

### 4.1 范围选择依据

| 约束 | 数值 |
|---|---:|
| Hub 磁盘 | 约 99 G 级可用，artifacts 已占十余 G |
| 全量 SWE-smith | ≈59136 实例 / 222 镜像 ≈ 290 G 级 |
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
2. rsync（`setsid` 持久任务）到 Hub 暂存：`/root/hub-staging/swe-bench-smith-0.2.0/`。  
3. SHA256 核验 10/10 通过后，`scripts/publish_swesmith_hub_package.py` 以 `file_artifacts` 流式入库。  
4. Episode Stack `swe-bench-smith-openhands@1.1.0` 钉住 `swe-bench-smith@0.2.0`。

### 4.4 与「环境创建」示例的关系

Verified 单实例流程（如 `psf__requests-1142` → `import-docker` → manifest）是**契约侧**标准化路径。本轮交付的是**数据集侧**完整包：同一 `swe` 契约下，把 smith 变体的任务数据 + 镜像字节一并版本化进 Hub；消费侧 `uenv env sync` + `docker load` 即可零外拉。

---

## 5. 真机部署与验收

| 项 | 值 |
|---|---|
| Hub 主机 | `8.130.95.176:8088` |
| 二进制 | `/root/uenv-console-e2e/uenv-hub/target/release/uenv-hub-server` |
| 配置 | `/root/uenv-public-deploy/hub.public.toml` |
| 数据目录 | `/root/uenv/uenv-hub/data/` |
| Reader token | `/root/uenv/uenv-hub/data/.console_reader_token` |

| 检查 | 期望 |
|---|---|
| `GET /console` | 浅色主题；侧栏为「可运行 / 构件 / 存储 / 运维」 |
| `#/benchmarks` | 固定 swe/qa/code 三区；无「未声明」；夹具单独列出 |
| `GET /api/v1/packages?per_page=200` | 每项带 `kind`；smith 为 `benchmark`，`env_type=swe`，`dataset=smith`，`instance_count=8226` |
| `GET /api/v1/packages/swe-bench-smith/versions/0.2.0` | ≥14 artifacts，含 10 个 `image_tar` |
| `GET .../sync-plan` | bundle_digest 稳定；image_tar 可下载且摘要匹配 |
| `GET /api/v1/envs` | 在用契约与「已归并历史名」分区；错位名为 deprecated |
| Stack resolve | `swe-bench-smith-openhands` → env_package `0.2.0` |
| `scripts/verify-hub-console-e2e.sh` | 静态资源 + overview + 渲染回归 + 对比度审计通过 |

---

## 6. 脚本与产物路径

| 路径 | 用途 |
|---|---|
| `scripts/build_swesmith_envpackage.py` | 从 parquet + tar 构建包目录 |
| `scripts/publish_swesmith_hub_package.py` | Hub 本机流式发布 |
| `uenv-hub-types` → `PackageKind` | kind / env_type / dataset 派生逻辑 |
| `uenv-hub-server/console/{app.js,app.css}` | 控制台 IA 与基准页 |
| Hub 暂存 | `/root/hub-staging/swe-bench-smith-0.2.0/`（发布后可清理） |
| Hub artifacts | `/root/uenv/uenv-hub/data/artifacts/swe-bench-smith/0.2.0/` |

---

## 7. 回滚

```bash
# 撤回 0.2.0 后 latest 回到上一非 yank 版本（如 0.1.0 smoke）
curl -X POST -H "Authorization: Bearer $UENV_HUB_TOKEN" \
  "$UENV_HUB_ENDPOINT/api/v1/packages/swe-bench-smith/versions/0.2.0/yank" \
  -H 'Content-Type: application/json' \
  -d '{"reason":"rollback"}'
```

控制台二进制回滚：恢复 `/root/uenv-public-deploy` 中备份的 `uenv-hub-server`，并确认 `hub.public.toml` 指向即可。

---

## 8. 本轮代码改动摘要（相对上一 commit）

| 区域 | 改动 |
|---|---|
| `uenv-hub-types` | fixture 优先分类；`benchmark_env_type` / `benchmark_dataset`；包名兜底；单测对齐生产脏 overlay |
| `uenv-hub-core` | `PackageSummary.dataset` 填充 |
| `console` | 基准页固定三契约 + 夹具区 + 说明卡片；配套 CSS |
| 本文档 | 补齐术语表、层级、kind/env/dataset 规则与真机清单 |
