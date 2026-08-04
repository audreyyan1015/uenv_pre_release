# 260804 · AgentENV 对比分析与 UEnv Hub 的相应改造

| 项 | 内容 |
|----|------|
| 分支 | `feature/worker-pool-260728_HubEpisodeStackRubric` |
| 范围 | 仅 Hub（`uenv-hub/`），不涉及 Worker / Server / Bridge |
| 分析对象 | [kvcache-ai/AgentENV](https://github.com/kvcache-ai/AgentENV)（下称 AENV），基于其 `main` 分支 README、`docs/src/` 文档站与仓库结构 |
| 产出 | 一份对比分析 + Hub 两处改造（聚合观测端点、内建运维控制台） |

> **取证说明**：本文对 AENV 的描述均来自其公开仓库文档（`docs/src/internals/architecture.md`、
> `docs/src/concepts/{overview,snapshots,templates}.md`、`README.md`）与目录结构。
> 其中「启动/恢复 < 50 ms、暂停 < 100 ms」等为 AENV **自述性能指标**，本文未在本地复现，
> 引用时保留其"声称"属性，不作为既成事实参与结论推导。

---

## 1. 摘要

AENV 是 kvcache-ai 团队开源的**智能体沙箱运行时平台**，自述为 Kimi K3 的 agentic RL 训练提供环境执行能力。
它的技术重心在一件事上：**让海量、异构、可快照的隔离环境跑得快且便宜**——用 Firecracker microVM 做隔离，
用自研 overlaybd（LSMT 分层镜像）+ ublk（用户态块设备）做镜像与内存快照的按需加载。

UEnv 与 AENV **不在同一层**。AENV 解决"一个沙箱怎么起得快、停得省、存得下"；
UEnv 解决"一次 Episode 由哪些经过校验的组件构成、由谁调度、结果怎么判分"。
落到我只关注的 Hub 上，两者**唯一真正可比的部分**是各自的"环境分发目录"：
AENV 的 template/snapshot 目录 vs UEnv Hub 的 EnvPackage / Episode Stack 注册表。

对比后确认可借鉴、且与 Hub 定位不冲突的有两点，本次均已落地：

1. **节点快照式聚合观测**（借鉴 AENV `src/observability/`）：Hub 新增 `GET /api/v1/system/overview`，
   一次请求返回身份、注册表清点、磁盘足迹、主机资源与启动姿态。
2. **服务自带控制台**（借鉴 AENV 把可运维性打进单一交付物的思路）：Hub 内建 `/console`，
   随二进制分发，零构建步骤。

其余若干设计（P2P 分发、按需块加载、E2B 兼容 API、快照回写 OCI）经评估**明确不采纳**，
理由见 §4.3——它们要么属于运行时层而非注册层，要么与 UEnv 已有的零出网消费模型冲突。

---

## 2. AgentENV 解剖

### 2.1 定位

一句话：**分布式的、快照优先的 agent 沙箱运行时**。对标物是 E2B（AENV 直接提供 E2B 兼容 HTTP API，
把 `E2B_API_URL` 指过去即可复用 E2B 官方 SDK），而不是 Docker Hub 这类注册中心。

### 2.2 三层结构

```
Client ──HTTP──> Gateway(:8080, Go) ──gRPC──> Scheduler(:9090, Go)
                     │                            │ 选点 / 查绑定
                     ▼                            ▼
              Node A(:8000, Rust)          Node B(:8000, Rust)
                     │
        API(Axum) → Orchestrator(生命周期状态机) → Firecracker VM
                                                      │
                                        ublk(/dev/ublkbN) → overlaybd 分层镜像
```

**存储层（核心，也是 AENV 真正的技术护城河）**

| 组件 | 作用 |
|------|------|
| `overlaybd` | LSMT 分层镜像格式。层内 16 字节位压缩段映射，读路径自上而下查层，写路径只追加到可写上层；zstd(level 3) 压缩带随机访问跳表与 CRC32C 校验；后端可插拔（本地 io_uring / OCI registry / tar） |
| `ublk` | 基于 Linux ublk 驱动的用户态块设备服务，把 overlaybd 镜像暴露成 `/dev/ublkbN`，经 io_uring 异步处理 |
| `ublk-daemon` | 独立进程持有所有 ublk 设备与 io_uring 控制权，与 node 通过 Unix socket 通信 |
| 内存快照恢复 | 不用 userfaultfd，而是把内存层也做成只读 ublk 设备交给 Firecracker 作 `BackendType::File` 内存后端，mmap + 首写 COW。**同一快照派生的多个沙箱共享同一内存 ublk 设备（引用计数）**，从而共享 host page cache |

**按需加载**：本地磁盘只是有界缓存，热数据保留、冷数据淘汰，因此镜像总量可以超过单机磁盘容量，
快照落在 OSS 或共享 POSIX FS 上。这是"跑海量异构环境"承诺的实际支撑。

**控制面**：Go 写的 gateway（按 sandbox ID 反向代理）+ scheduler（round_robin / random 选点）。
绑定关系全部在内存，重启即丢；靠 runtime 心跳上报的**全量 sandbox roster 作为真值**重建与对账。

### 2.3 值得记录的设计取舍

- **快照是第一性原语，模板只是它的用户态包装。** 模板 ID 解析到一个已提交快照，
  沙箱从快照 resume。`aenv pull ubuntu:24.04` 直接把 OCI 镜像导成模板，
  `Env`/`WorkingDir`/`User` 从 OCI config 继承。这个"目录项 → 内容寻址实体"的两层结构，
  与 UEnv Hub 的 `package_id@version → bundle_digest → 产物摘要` 是同构的。
- **可观测性走"请求时投影"而非常驻采集。** orchestrator 用 `tokio::sync::watch` 增量维护计数器，
  host 指标（CPU/内存/磁盘）在**每次请求时**现采（CPU 取两次 `/proc/stat` 采样，首次请求用 100 ms 窗口
  以免返回一个假的 0），最后合并成一个 `NodeSnapshot` 同时供 admin API 与心跳上报使用。
- **P2P 只做加速，不改变真值。** iroh-blobs 传输层为快照产物提供节点间分发，
  但快照仓库始终是 source of truth，P2P 发布失败不回滚已成功的快照提交。

### 2.4 能力边界

必须记录的一条：**AENV 目前完全没有鉴权**，README 明确警告不要把 API 暴露到公网。
此外强依赖 Linux kernel 6.8+、`/dev/kvm`（或 PVM），scheduler 绑定不持久化。
换言之它是一个假设跑在可信内网的高性能执行底座，运维面（鉴权、审计、发布闸门）基本留白。

---

## 3. 与 UEnv 的对比

### 3.1 先说层次：相邻而非竞争

```
        AENV 的问题域                    UEnv 的问题域
  ┌──────────────────────────┐   ┌────────────────────────────────┐
  │ 一个沙箱怎么起得快、      │   │ 一次 Episode 由哪些经过校验的   │
  │ 停得省、存得下、          │   │ 组件构成，谁调度，怎么判分，    │
  │ 跨机怎么路由              │   │ 结果怎么回训练框架              │
  └──────────────────────────┘   └────────────────────────────────┘
            执行底座                        任务/契约/编排
```

UEnv Worker 若要换执行后端，AENV 是一个**候选后端**（当前是 process / podman / docker）；
但 AENV 里没有任何东西对应 UEnv Hub 的职责。

### 3.2 逐维度对照

| 维度 | AgentENV | UEnv（Hub 视角） |
|------|----------|------------------|
| 核心抽象 | Sandbox / Snapshot / Template | Environment / EnvPackage / **Episode Stack** |
| 隔离粒度 | Firecracker microVM（内核级） | 进程 / 容器（由 Worker 后端决定） |
| 镜像分发 | overlaybd 按需块加载，本地盘作有界缓存 | 摘要引用 + 离线 tar `docker load`（**零出网**消费） |
| 内容寻址 | 层 digest 去重，managed-layers 目录 | 产物 sha256 去重 + `bundle_digest` 组合摘要 |
| 版本与解析 | 模板别名 → 快照 UUID，无语义化版本 | semver + 约束解析 + `latest` 指针 + yank |
| 组合建模 | 无（一个模板就是一个镜像） | **Episode Stack**：任务环境 × Agent 脚手架 × 运行时网关，解析期钉版本并产出 `stack_digest` |
| 发布闸门 | 无 | C01–C13 一致性校验、Rubric 对齐闸门、依赖图校验 |
| 鉴权与审计 | **无**（README 明示） | Token + RBAC（reader/publisher/admin）+ 命名空间 + 审计日志 |
| 判分契约 | 不涉及 | `RubricSpec` + `reference_scorer`，由 Hub 分发 |
| 多节点 | Go gateway + scheduler，内存绑定 | Hub 是单一注册中心；调度在 uenv-server |
| 可观测性 | `NodeSnapshot` 聚合投影 + 心跳上报 | Prometheus `/metrics` + `/healthz`（**改造前无聚合视图**） |
| 运维界面 | 无内建 UI（CLI `aenv`） | **改造前无**；本次新增 `/console` |

### 3.3 结论

Hub 在**契约治理**维度（版本语义、发布闸门、鉴权审计、组合建模、判分分发）远比 AENV 完整——
这本来就是两者定位差异的必然结果。Hub 的真实短板在**可观测与可运维的呈现层**：
一个操作者要回答"这台 Hub 现在托管了多少东西、产物占了多少盘、进程健不健康"，
改造前只能拼十几个 list 接口，而且盘用量与主机资源**在任何接口里都拿不到**。
AENV 的 `NodeSnapshot` 恰好是这个问题的成熟答案。

---

## 4. 借鉴取舍

### 4.1 采纳：聚合式运行态快照

AENV 把「身份 + 运行时计数 + 请求时主机指标 + 当前 roster」合成一个 `NodeSnapshot`，
一个接口喂满 admin API 与心跳两个消费方。Hub 照此新增 `GET /api/v1/system/overview`。

三处细节直接沿用了 AENV 的做法，因为它们是对的：

- **计数按请求现算，不维护常驻 gauge。** 注册中心是低写系统，一个会与它所描述的表漂移的计数器，
  比多花几毫秒更糟。
- **CPU 用两次 `/proc/stat` 采样求差。** 首次请求没有基线时才付 100 ms 窗口的代价，
  之后用"上一次 overview 请求"的采样作基线，请求路径不再阻塞。
- **拿不到就报缺失，不编造 0。** 非 Linux 主机上 `/proc` 派生字段一律 `null`——
  0% CPU 与"这台机器不提供该指标"是两种完全不同的状态。

Hub 在此之上加了 AENV 没有、但注册中心必须有的两块：**磁盘足迹**（产物库实测字节 vs 发布时登记字节、
数据库文件大小）与**启动姿态**（是否强制鉴权、限流参数、CORS 白名单、播种开关）。

### 4.2 采纳：可运维性打进单一交付物

AENV 的安装脚本一条命令装完 server + CLI + systemd。同样的思路应用到 Hub 的控制台上：
**HTML/CSS/JS 用 `include_str!` 编进二进制**，不引入 Node 工具链、不新增静态文件根目录。
两个后果都是想要的——部署机只要有 Hub 本身；控制台永远不可能与它所绘制的 API 版本错配。

### 4.3 评估后不采纳

| AENV 设计 | 不采纳的理由 |
|-----------|--------------|
| overlaybd 按需块加载 | 属于 Worker 执行层。UEnv 的真机约束是**零出网**：镜像以 tar 形式离线 `docker load`，按需回源与该约束直接冲突 |
| iroh-blobs P2P 产物分发 | Hub 产物是 KB 级的 catalog / manifest / eval_spec，不是 GB 级镜像层，P2P 的复杂度换不来收益。真正大的镜像 tar 已由 Hub 托管路径 + 摘要校验覆盖 |
| E2B 兼容 API | E2B 的抽象是 sandbox 生命周期，Hub 不管理任何运行时实例，兼容它没有语义对应物 |
| 快照回写 OCI registry | 需要 Hub 具备 registry 写权限并参与镜像构建。Hub 的既定原则是**只按摘要引用镜像字节、绝不持有**，这条不应破例 |
| Firecracker / ublk | 执行层技术，与注册中心无关 |

---

## 5. UEnv Hub 的改动

### 5.1 新增 `GET /api/v1/system/overview`（角色：reader）

一次请求返回五个区块：

```jsonc
{
  "service":   { "name": "uenv-hub", "version": "0.1.0", "git_sha": null },
  "started_at": 1754000000, "uptime_seconds": 3600, "server_time": 1754003600,
  "db_up": true,

  "registry": {          // 注册表清点，soft-delete 的实体已排除
    "envs": 6, "env_versions": 9, "yanked_env_versions": 0, "deprecated_envs": 1,
    "packages": 5, "package_versions": 5, "yanked_package_versions": 0,
    "package_artifacts": 18, "package_artifact_bytes": 1048576,
    "stacks": 3, "stack_versions": 3, "yanked_stack_versions": 0,
    "agent_bridges": 2, "templates": 5, "active_tokens": 1, "audit_entries": 12
  },

  "storage": {           // 磁盘实测
    "artifact_dir": "/root/uenv/uenv-hub/data/artifacts", "artifact_dir_exists": true,
    "artifact_files": 18, "artifact_bytes": 1048576,
    "database_url": "sqlite:///root/uenv/uenv-hub/data/hub.db", "database_bytes": 262144
  },

  "host": {              // /proc 派生，非 Linux 主机上相应字段缺席
    "os": "linux", "arch": "x86_64", "cpu_cores": 4,
    "cpu_usage_percent": 3.2, "load_average": [0.14, 0.09, 0.05],
    "memory_total_bytes": 16482000000, "memory_available_bytes": 12900000000,
    "process_resident_bytes": 41000000
  },

  "posture": {           // 启动姿态
    "require_token": true, "rate_limit_enabled": true,
    "requests_per_second": 50, "burst": 100,
    "cors_allow_origins": ["*"], "seed_examples": true,
    "catalog_seed_dir": "/root/uenv/config/swe"
  }
}
```

几个刻意的设计决定：

- **角色定为 reader 而非 admin。** 这里的内容要么已能从公开 list 接口推导，要么是粗粒度主机遥测。
  若为了画一个只读仪表盘就必须签发 admin token，反而会把 admin 令牌散出去。
- **yanked 版本计入总数并单列。** 撤回版本仍然可查、仍占盘，运维需要知道目录里有多大比例处于该状态。
- **`storage.artifact_bytes`（磁盘实测）与 `registry.package_artifact_bytes`（发布时登记）分开报。**
  内容寻址按摘要去重，同一份字节被多版本引用时只落盘一次，所以**登记 ≥ 实测**是正常的；
  反向的差值才提示发布中断或产物被外部清理。控制台会在两者不等时给出这段解释。

### 5.2 新增内建控制台 `/console`

`GET /` 307 跳到 `/console`，即打开 Hub 地址就是控制台。功能覆盖 Hub 的全部只读面：

| 视图 | 覆盖内容 |
|------|----------|
| 总览 | 主机 CPU/内存/负载/进程 RSS，产物库与数据库占用，注册表全量计数，服务身份与启动姿态 |
| 环境 | 分页 + namespace/author/tag 筛选；详情含版本表、完整 manifest、镜像摘要与资源诉求、生命周期与弃用提示、发布闸门备注、Rubric 契约、配置 Schema、接口契约、示例、依赖；内置**版本约束解析器**（走服务端同一条解析逻辑） |
| 环境包 | 列表与详情；产物清单（名称/类型/摘要/大小/同步方式/落地路径）可**逐个在线查看与下载**；`bundle_digest`、`worker_overlay`、`agent_defaults`、`contracts`、`sync-plan` |
| Episode Stack | 列表、版本、声明；**解析后的启动计划**——组件表（角色/请求约束/解析版本/摘要/来源）、`stack_digest`、运行时网关要求、包同步计划、解析备注 |
| Agent Bridge | 脚手架目录，`bundle_digest` 与 Agent 注册上报字段同名同值，可直接比对 |
| SWE 实例目录 | verified / lite / pro / smith 四变体切换，实例数、去重仓库数、实例表 |
| 脚手架模板 | 列表 + 归档 sha256 + 直接下载 tar.gz |
| 搜索 | q / tag / author / namespace |
| 审计日志 | 分页浏览（需 admin） |
| 健康与指标 | `/healthz`、`/version`，并把 `uenv_hub_http_requests_total` 按「方法 + 路径」聚合成请求分布表 |
| 连接与凭据 | Token 存本地 localStorage，保存即校验；给出等效 CLI 命令 |

实现约束：

- **零构建步骤。** 原生 ES 模块级 JS + 单文件 CSS，`include_str!` 编入二进制。
- **设计沿用现有前端。** 色板与圆角直接取自 `frontend/src/styles.css` 的 oklch 设计令牌，
  与训练观测台同属一套视觉体系。
- **同源，无端点配置。** 控制台由 Hub 自身提供，不存在跨源问题；HTML 外壳是公开的（不含任何数据），
  数据请求一律带 Token，由与其它 API 客户端完全相同的中间件鉴权。

### 5.3 改动清单

| 文件 | 改动 |
|------|------|
| `uenv-hub-types/src/lib.rs` | 新增 `HubOverview` / `RegistryStats` / `StorageStats` / `HostStats` / `HubPosture` |
| `uenv-hub-core/src/repository.rs` | 新增 `SqliteStore::registry_stats()` |
| `uenv-hub-server/src/sysinfo.rs` | 新增：`/proc` 主机指标采集（`CpuMeter` 跨请求持有上次采样）、目录用量遍历、SQLite 文件大小；含 4 个单元测试 |
| `uenv-hub-server/src/ui.rs` | 新增：控制台静态资源路由，含 2 个"资源引用/端点引用一致性"测试 |
| `uenv-hub-server/console/{index.html,app.css,app.js}` | 新增：控制台本体 |
| `uenv-hub-server/src/state.rs` | `AppState` 增加 `started_at`、`cpu_meter` |
| `uenv-hub-server/src/lib.rs` | 装配上述两个字段，注册 `sysinfo` / `ui` 模块 |
| `uenv-hub-server/src/routes.rs` | 新增 `system_overview` 处理器与 `/system/overview` 路由；合并控制台路由 |
| `uenv-hub-server/tests/e2e.rs` | 新增 3 个端到端测试 |
| `uenv-hub/docs/api.md` | 补录新端点与控制台 |
| `scripts/verify-hub-console-e2e.sh` | 新增：控制台与 overview 的端到端联调脚本（HTTP 断言 + 无头浏览器渲染回归） |

**未引入任何新依赖**：主机指标读 `/proc`，静态资源用 `include_str!`。

### 5.4 测试

单元测试（`uenv-hub-server`）：

- `sysinfo`：目录缺失报"未创建"而非"空"、嵌套文件计数与字节累加、SQLite URL 带查询参数时仍能定位文件、
  主机指标自洽性（百分比落在 0–100，可用内存不超过总内存，非 Linux 上必须为 `None`）。
- `ui`：外壳引用的静态资源必须都被路由服务（拼错即生产白页，此前没有任何环节会更早失败）；
  控制台调用的每个端点都必须在 JS 里出现，防止某个视图接到一条从未注册的路径。

端到端测试（`tests/e2e.rs`）：

- `system_overview_reports_live_registry_inventory`：播种后各类计数均 > 0；
  随后新建一个环境并发布一个版本，断言 `envs` 与 `env_versions` **各 +1**、`packages` **不变**——
  验证计数确实跟随注册表而非常量。
- `system_overview_counts_yanked_versions_separately`：撤回一个版本后，
  `env_versions` 仍 +2、`yanked_env_versions` +1。
- `console_is_served_by_the_hub_itself`：`/` 307 跳到 `/console`；外壳 `text/html` 且引用 `app.js`；
  两个静态资源各自返回正确 Content-Type 且非空。

### 5.5 无头渲染回归与它抓到的三个缺陷

上述测试证明的是「资源取得到、API 有数据」，证明不了**页面画得出来**。
所以 `scripts/verify-hub-console-e2e.sh` 末尾追加了一段无头浏览器回归：
起一个带播种的 Hub，用 Chrome `--headless --dump-dom` 逐条路由渲染，
断言面包屑出现且 DOM 里没有错误块；机器上没有 Chrome 时跳过而不失败。

这段回归立刻抓到三个**只在浏览器里才暴露**的缺陷，均已修复：

| 现象 | 根因 | 修法 |
|------|------|------|
| Stack 详情整页空白，只剩一行 `appendChild: parameter 1 is not of type 'Node'` | `agent_scaffold` 在 API 里是结构体（`package_id`/`version`/`agent_kind`），而详情页按字符串渲染，把对象直接塞给了 `appendChild` | 新增 `scaffoldRef()` 渲染成指向该环境包的可点引用；同时给 `el()`/`kv()` 加兜底——任何非 Node 值降级为文本，**一个字段的类型意外不该让整页白屏** |
| SWE 实例目录恒为空态，尽管 API 返回 200 且有数据 | `config/swe/*.json` 是**以 instance_id 为键的字典**，而前端只认数组和 `{instances:[…]}` 两种形态 | 兼容第三种形态（`Object.values`） |
| 环境包详情恒显示「（无描述）」 | `description` 挂在注册表条目（`/packages` 列表项）上，不在版本清单里 | 详情页回列表取一次，缺失时再回落到清单字段 |

前两个是会被用户第一眼看到的功能性故障，而**全部 20 个 Rust 测试与全部 HTTP 断言当时都是通过的**——
这正是补这段回归的理由。

---

## 6. 联调验证

完整记录见同目录 [`260804-Hub控制台真机联调记录.md`](./260804-Hub控制台真机联调记录.md)。结论：

在 Hub 真机 `8.130.95.176`（Ubuntu，4 vCPU / 16 GiB）上另开隔离实例（18091，不碰 8088 生产实例）跑通——
编译 48.5 s、**151 项测试 0 失败**、联调脚本全部断言通过、
经 SSH 隧道用真实浏览器渲染 **16 条路由零错误**。

真机唯一独有的证据是 `/proc` 派生指标：本地 macOS 上 `cpu_usage_percent` 等四项恒为 `null`
（这本身也被单元测试断言），真机上则读到 `cpu 2.5% / load [0.93,0.84,0.35] / 15.1 GiB 可用 / RSS 29 MB`，
`cpu_cores=4`、`memory_total≈16.07 GB` 与 README 记录的机器规格吻合。

生产 8088 仍跑旧二进制，切换到带控制台的版本是一次独立的运维动作，本次未执行。

---

## 7. 后续可做（未在本次范围内）

1. **控制台的写操作面**：yank、patch 元数据、发布 Episode Stack 目前只能走 CLI。
   写操作要配合二次确认与审计回显，值得单独一轮设计。
2. **overview 的时间序列**：当前是瞬时快照。若要看趋势，正确做法是让 Prometheus 抓 `/metrics`
   并把 registry 计数也注册成 gauge，而不是让 Hub 自己存历史。
3. **AENV 作为 Worker 执行后端**：若将来需要 microVM 级隔离，AENV 的 E2B 兼容 API 是一条现成接入路径。
   这属于 Worker 范围，与 Hub 无关。
