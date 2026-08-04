# SWE-smith 环境支持与 OpenHands Rollout 联调规划

> 日期：2026-08-01  
> 状态：Phase 1–3 已在实机验证；Hub EnvPackage / Episode Stack 注册及隔离联调已补齐，生产部署与全量 image tar 仍待运维  
> 范围：Worker 侧 SWE-smith 环境接入 → 多机功能服务全链路联调 → Rollout 训练可用性验证  
> 实机拓扑：[secrets/README.md](../../../secrets/README.md)  
> 前置参考：[SWE-bench Pro + OpenHands 集成方案](../../older/260627-swe-openhands-integration-plan.md)、[SWE-bench-Pro UEnv 联调依赖说明](../260713/SWE-bench-Pro-UEnv联调依赖说明.md)、[五类 Benchmark Worker 支持现状](../260709/五类Benchmark-Worker支持现状与跨层调整.md)

---

## 0. 目标与验收边界

### 0.1 背景

[SWE-smith](https://github.com/SWE-bench/SWE-smith) 是面向 SWE-agent 训练的大规模任务集（HF：`SWE-bench/SWE-smith`，约 52k instance；镜像按 repo 预构建于 `jyangballin/swesmith.x86_64.*`）。官方训练闭环为：子集筛选 → Agent 生成轨迹 → `resolved` 过滤 → SFT/RL 格式转换 → 在 SWE-bench Verified 等评测集上验收。

UEnv 侧已具备 `env_type=swe` + OpenHands Agent 池（208.77）+ Worker Runtime Gateway（7143）的 Pro 评测链路。本规划将 **SWE-smith 作为新的 `benchmark_variant`** 接入同一框架，使 Worker 能提供训练用交互环境，并与 OpenHands 组合产出可训练 Rollout。

### 0.2 目标

| # | 目标 | 验收口径 |
|---|------|----------|
| **A** | Worker 直接支持 SWE-smith 环境（catalog / 镜像 / session / 判分） | `benchmark_variant=smith` 可 provision ≥1 真实 instance；gold / Agent 路径均可 `submit` 出 `resolved` + trajectory |
| **B** | 结合现有各服务器功能服务完成完整链路联调 | Adapter → Server(`8.130.75.157:8088`) → Worker(7143) → Gateway → OpenHands(208.77) 跑通 ≥1 条 Smith episode |
| **C** | 验证产出 Rollout 可用于训练 | 导出轨迹满足约定 schema；至少完成「resolved 过滤 → SFT/训练侧可读 JSONL」样例转换；7142 侧可消费或离线校验通过 |
| **D** | 为后续 Hub 注册预留清晰交接面 | 冻结 EnvPackage / catalog / overlay 字段约定；Hub 适配由 Hub 模块负责人承接，本阶段不阻塞 Worker 联调 |

### 0.3 非目标（本规划不做）

- Hub 已正式注册 `swe-bench-smith` EnvPackage 并验证 `env sync`；全量 `image_tar` 入库与生产部署仍属运维后续。
- 一次性导入全部 ~52k instance / 250+ repo 镜像。
- 替换现有 SWE-bench-Pro 评测主路径。
- 在 Worker 内重写 SWE-smith 官方 harness；优先复用 / wrap `swesmith.harness.eval` 语义。
- 强制统一为 SWE-agent scaffold；本阶段 Agent 侧继续以 **OpenHands** 为主。

### 0.4 分阶段策略（冻结）

```text
Phase 1  Worker 本地直接支持 Smith（本地 catalog + 镜像，可不经 Hub）
    │
    ▼
Phase 2  多机功能服务全链路联调（Server / Worker / OpenHands / Adapter）
    │
    ▼
Phase 3  Rollout 产出与训练可用性验证（7142 训练侧消费或离线校验）
    │
    ▼
Phase Hub（已补齐注册） 环境注册到 Hub → Hub 模块拉取 / 分发隔离联调
```

**原则**：初期以 Worker 本机制品打通链路与训练验证为先；Hub 注册与拉取适配不阻塞 Phase 1–3。

---

## 1. 实机角色与链路（对照 secrets）

沿用现有四端 + Agent 池拓扑，Smith 不新增主机。

| 主机 | 角色 | Smith 联调职责 |
|------|------|----------------|
| **7143** `219.147.100.43:7143` | uenv-worker | Smith catalog / 镜像 / Runtime Gateway `:28097` / TrajectoryStore |
| **Server** `8.130.75.157` | adapter-core | `SubmitEpisode(env_type=swe)`、`DispatchEpisode`、`AgentControlService` |
| **208.77** `8.130.208.77` | OpenHands Agent 池 | `RegisterAgent` / `PollAgentJob`；经隧道访问 Gateway；产出 Agent 轨迹 |
| **7142** `219.147.100.43:7142` | VeRL / Adapter / LLM | 训练侧消费 Rollout；可选本地 LLM `:18888` / 单卡 vLLM |
| **Hub** `8.130.95.176` | uenv-hub | **Phase Hub 再接入**；Phase 1–3 可用本地 fixtures / 本机 EnvPackage 目录降级 |

关键 endpoint（与 secrets §1.2 一致）：

| 方向 | 地址 | 用途 |
|------|------|------|
| Worker ↔ Server | `8.130.75.157:8088` ↔ `219.147.100.43:28888` | Register / Dispatch / ReportResult |
| OpenHands → Gateway | `127.0.0.1:28097`（208.77 隧道）或公网 `:28097` | session / exec / submit / trajectory |
| Agent → Server | `8.130.75.157:8088` | RegisterAgent / PollAgentJob / CompleteAgentJob |
| Worker health / Gateway health | `:28777` / `:28097` | 探活 |

**推荐启动顺序（目标架构，与 secrets §1.2 SWE+Agent 一致）**：

1. Server `8.130.75.157:8088` 就绪  
2. Worker 7143 Register（含 `gateway_public_url`，加载 Smith catalog）  
3. 208.77 Agent Register（`OPENHANDS_AGENT_POLL=1`）  
4. Adapter / 脚本 `SubmitEpisode(swe, execution_mode=agent, benchmark_variant=smith)`

旁路调试仍可用 208.77 runner `:8888` + Gateway 隧道，不经 AgentJob。

---

## 2. 架构定位

### 2.1 调度键约定

| 项 | 冻结值 | 说明 |
|----|--------|------|
| `env_type` | **`swe`** | 不新增顶层 env_type；与 verified / lite / pro 并列 |
| `benchmark_variant` | **`smith`**（别名：`swe-smith` / `swe_smith` / `swe-bench-smith`） | Worker `BenchmarkVariant::Smith` |
| `execution_mode` | 初期 **`agent`**（OpenHands）；native/gold 用于 smoke | 训练 rollout 主路径走 Agent |
| 默认 grader | **`swesmith`**（新建） | wrap 官方 eval 语义；勿误用 `swebench_pro` |
| 工作区 | 以 Smith 镜像约定为准（通常接近 SWE-bench `/testbed`；实机校准后写入文档） | 禁止硬编码 Pro 的 `/app` |

### 2.2 目标数据流

```text
7142 Adapter / 评测脚本
        │  SubmitEpisode(env_type=swe, benchmark_variant=smith,
        │                execution_mode=agent, instance_id=…)
        ▼
8.130.75.157  uenv-adapter-core
        │  DispatchEpisode → 7143
        │  CreateAgentJob  → 208.77 poll
        ▼
7143 Worker                         208.77 OpenHands
  SweInstancePool(smith)              run_*_official / UEnvWorkspace
  Runtime Gateway :28097  ◄──隧道──►  gateway_tools / Agent loop
  TrajectoryStore                     CompleteAgentJob(patch, traj_ref)
        │
        ▼
EpisodeResult + TrajectoryBundle
        │
        ▼
Rollout 导出（JSONL）→ 7142 训练侧（SFT / VeRL AgentLoop 等）
```

### 2.3 与 Pro 路径的差异（必须显式处理）

| 维度 | SWE-bench-Pro（已有） | SWE-smith（本规划） |
|------|----------------------|---------------------|
| 用途 | 评测 / 对标 | **训练数据 / Rollout** |
| 数据规模 | 千级量级 public set | ~52k；初期只取子集 |
| 镜像 | `sweap-images` / Pro 命名空间 | `jyangballin/swesmith.x86_64.<repo>` |
| Catalog | Hub `swe-bench-pro` EnvPackage | Phase 1：Worker 本地；Phase Hub：独立 EnvPackage |
| 工作目录 | `/app` | 需按 Smith 镜像校准（默认按 `/testbed` 假设验证） |
| Grader | `swebench_pro` / 外部 pro_eval | `swesmith` / wrap `swesmith.harness.eval` |
| OpenHands driver | `run_swebenchpro_official.py` | 新增或扩展 Smith driver（复用 Gateway 契约） |
| 成功定义 | `resolved` 评测指标 | `resolved` **且**轨迹可转为训练样本 |

---

## 3. Phase 1 — Worker 直接支持 Smith

### 3.1 代码改动面（Worker 为主）

| 模块 | 改动 | 优先级 |
|------|------|--------|
| `uenv-worker/src/swe/variant.rs` | 增加 `BenchmarkVariant::Smith` + parse aliases | P0 |
| `config` / deploy yaml | `swe.variants` 允许 `"smith"`；7143 联调配置 | P0 |
| `dataset` / catalog 加载 | 支持 Smith instance schema（HF 字段 → `SweInstance`） | P0 |
| `image_cache` / `image_ref` | Smith 镜像命名解析；`local_only` 默认 | P0 |
| `grader.rs` | 新增 `SwesmithGrader`（或外部 Python wrap） | P0 |
| `runtime_gateway` | session 创建透传 `benchmark_variant=smith`；工作区路径按变体 | P0 |
| `repo_specs` | 按 Smith 高频 Python repo 补 TestRunner（可分期） | P1 |
| fixtures | `fixtures/swe/smith/` 最小 1–3 条真实 instance + 镜像拉取脚本 | P0 |

### 3.2 本地制品（不经 Hub）

初期在 7143 落地：

```text
/var/lib/uenv/envs/swe-bench-smith/0.1.0-local/
  catalog.json          # 子集 instance（建议 ≤20 条 smoke；训练子集另定）
  images.manifest       # instance_id → image ref / 可选 tar
  worker.overlay.yaml   # benchmark_variant=smith, image_pull_policy=local_only
  eval_spec/            # 可选：指向 swesmith harness 入口
```

Worker 配置示例（概念）：

```yaml
swe:
  variants: ["verified", "pro", "smith"]
  env_package_dirs:
    - /var/lib/uenv/envs/swe-bench-smith/0.1.0-local
runtime_gateway:
  enabled: true
  bind: "0.0.0.0:28097"
```

### 3.3 子集与镜像策略

| 项 | 建议 |
|----|------|
| Smoke 子集 | 1–3 个已验证可跑通的 Python repo instance（优先 `FAIL_TO_PASS` 规模适中） |
| 训练子集 | 按官方建议筛选（如 `.pr_` + FAIL_TO_PASS 数量区间）；规模由训练同学给定 |
| 镜像来源 | `docker pull jyangballin/swesmith.x86_64.<mirror_repo>`；国内 mirror 链复用 Pro 经验 |
| 磁盘 | 7143 系统盘余量需监控（secrets §1.4）；按 repo 镜像体积分期导入，禁止一次全量 |

### 3.4 Phase 1 验收

- [ ] `BenchmarkVariant::parse("smith")` 单测通过  
- [ ] 本地 catalog 加载后 Gateway `POST /runtime/v1/sessions` 成功  
- [ ] gold patch（若有）或最小 write → `submit` → `resolved` 可判定  
- [ ] `TrajectoryBundle` 落盘且 `benchmark_variant=smith`

---

## 4. Phase 2 — 多机功能服务全链路联调

### 4.1 联调矩阵

| 步骤 | 负责侧 | 动作 | 通过标准 |
|------|--------|------|----------|
| 2.1 | Worker | 7143 启 Worker + Gateway，Register 含 `swe` | health `:28777` ok；Server 可见 worker |
| 2.2 | Server | adapter-core 正常；Agent 池可注册 | `GET :50052/agents`（本机 admin）可见 208.77 |
| 2.3 | OpenHands | Smith driver + `workspace_dir` 校准；隧道 `:28097` | runner health `:8777`；Gateway health 经隧道 ok |
| 2.4 | Adapter/脚本 | 提交 1 条 Smith Agent episode | `status=completed`；有 `reward` / `resolved` |
| 2.5 | 全链路 | 连续 ≥3 条不同 instance（或同 instance 重跑） | 无 session 串线；trajectory_id 可回取 |

### 4.2 OpenHands / Agent 侧要点

复用 Pro 已落地能力，差异点集中在 driver 与指令：

| 项 | 要求 |
|----|------|
| Gateway API | 不变：`/runtime/v1/sessions|exec|read|write|submit|trajectories` |
| `CompleteAgentJob` | 继续传 `agent_id`（Pro 已修） |
| Driver | 新增 `run_swesmith_official.py`（或扩展现有 driver 按 variant 分支） |
| Instruction / cwd | 与 Smith 镜像仓库根一致；勿写死 `/app` |
| 超时 | 训练轨迹通常更长；参考 Pro：`OPENHANDS_RUN_TIMEOUT_SEC`、`MAX_OUTPUT_TOKENS` 可配置放大 |
| 并发 | 初期 `OPENHANDS_AGENT_MAX_CONCURRENT=1`；Worker `max_concurrent` 保持现网值再调 |

### 4.3 Adapter / Bridge

| 项 | 说明 |
|----|------|
| Payload | 显式 `env_type=swe`、`benchmark_variant=smith`、`instance_id`、`execution_mode=agent` |
| 透传 | Core 已有 `instance_id` / `benchmark_variant` 透传；确认 smith 不被归一成 pro |
| 评测脚本 | 新增 `evaluate_swesmith_uenv.py`（或扩展现有 swe 脚本）记录 request/result JSONL |

### 4.4 Phase 2 验收

- [x] 目标架构（Server 编排 AgentJob）跑通 ≥1 条 Smith（**gold**；`resolved=true reward=1.0`）
- [x]（可选）旁路 runner / `run-openhands-pro-20877.sh` gold 同步可跑
- [x] Worker 日志：`dispatch` → Gateway session → `submit` → `report_result`（trajectory `…00004`）
- [x] Server / Agent 无 `AGENT_MISMATCH`、无 `instance_id not in catalog`
- [ ] 真实 LLM Agent episode（非 gold）— 依赖可用 model endpoint，可后续补跑

---

## 5. Phase 3 — Rollout 训练可用性验证

### 5.1 Rollout 定义（本规划口径）

一次可训练 Rollout = **一条完整 Agent–环境交互轨迹** + **可验证奖励**：

| 字段族 | 来源 | 用途 |
|--------|------|------|
| `trajectory_id` / `instance_id` / `benchmark_variant=smith` | Worker TrajectoryBundle | 关联与去重 |
| `steps[]`（action / observation） | Gateway 捕获 | SFT / RL 状态动作序列 |
| `reward` / `resolved` | Grader | Rejection sampling / 过滤 |
| `artifact.git_diff` / patch | submit 产物 | 与官方 preds 对齐 |
| `episode_id` / `correlation_id` / `training_run_id` | Server / Adapter metadata | 训练作业追踪 |

参考样例结构见 `Docs/trajectory/trajectory-bundle.example.json`（Pro）；Smith 仅替换 `benchmark_variant` 与 instance/镜像相关字段。

### 5.2 导出与转换

```text
Worker TrajectoryStore
    → 导出 TrajectoryBundle JSON（或 Server 聚合后的 bundle）
    → 过滤 resolved==true（RSFT）或保留全量（RL）
    → 转换为训练侧格式：
         A. OpenHands / 通用 chat JSONL（messages + reward）
         B. 对齐 SWE-smith 官方 ft_xml / traj 格式（可选，便于对照论文流程）
    → 7142 侧：离线校验 schema +（可选）小步 SFT / VeRL 消费 smoke
```

### 5.3 训练侧验证清单（7142）

| # | 检查项 | 通过标准 |
|---|--------|----------|
| 1 | Schema | 必备字段齐全；`steps` 非空；`reward` 数值合法 |
| 2 | 对齐 | 同 instance 的 patch 可在 Worker 上复现 `resolved`（gold 或 Agent patch 回放） |
| 3 | 过滤 | `resolved=true` 子集可单独导出；比例与日志一致 |
| 4 | 消费 | 至少一种训练入口能读入（VeRL AgentLoop 样例 **或** 独立 SFT JSONL loader smoke） |
| 5 | 归因 | `training_run_id` / `correlation_id` 可从导出文件追溯到 Server episode |

### 5.4 Phase 3 验收

- [x] ≥1 条 Smith Agent rollout 完整导出（`trajectory_bundle` → `bundles.jsonl`）
- [x] ≥1 条 `resolved=true` 样本进入训练可读 JSONL（`chat_sft.resolved.jsonl`）
- [x] 离线 schema 校验通过（必备字段 / `steps` 非空 / `reward` 数值）；7142 可直接读同一 JSONL（1-step 训练 smoke 非必须）
- [x] 文档记录导出命令、路径约定、已知限制（见联调记录 §7）

---

## 6. Phase Hub — 原后续交接（现已补齐注册与隔离联调）

> 由 Hub 模块负责人适配；Worker / 联调侧只冻结契约。

### 6.1 建议注册形态

| 项 | 建议值 |
|----|--------|
| EnvPackage id | `swe-bench-smith` |
| 任务环境 registry | `swe` 的 `dataset` / `config_schema` 增枚举 `swe-bench-smith` |
| `worker_overlay.swe.benchmark_variant` | `smith` |
| 制品 | `catalog.json` + `images.manifest` +（按需）`image_tar` |
| Episode stack（可选） | `swe-bench-smith-openhands@x.y.z` |

### 6.2 交接清单（给 Hub）

1. Phase 1 本地目录中已验证的 catalog / overlay / 镜像命名规则。  
2. Worker 已支持的 `benchmark_variant` 别名表。  
3. 镜像体积与拉取策略（`local_only` vs Hub tar）。  
4. 与 Pro 包的隔离要求（禁止混用 catalog / grader）。  
5. 联调通过的最小子集 instance 列表（作为 Hub seed 候选）。

### 6.3 Worker 侧预留

- 继续通过既有 `EnvPackageDir` / `hub` pull 路径消费未来 Hub 包。  
- Phase 1–3 使用的本地路径可在 Hub 就绪后改为 `env sync` 目标目录，避免二次改调度键。

---

## 7. 风险与缓解

| 风险 | 影响 | 缓解 |
|------|------|------|
| Smith 镜像体积大、7143 磁盘不足 | 无法扩子集 | 严格子集；镜像按 repo 导入；监控 `df -h` |
| 工作目录与 Pro `/app` 混淆 | Agent 改错树、resolved=0 | driver 按 variant 分支；联调首条强制打印 cwd |
| Grader 与官方 `swesmith.harness.eval` 不一致 | 训练标签噪声 | 外部 wrap 官方 harness；用 gold/已知 patch 做对齐单测 |
| OpenHands 轨迹格式 ≠ 训练期望 | 7142 无法直接训 | Phase 3 明确转换层；保留原始 bundle + 转换后 JSONL |
| 全量 52k 过早入库 | Hub/磁盘/同步爆炸 | 本期禁止全量；Hub 阶段仍按子集发布 |
| 国内拉 Docker Hub 失败 | 环境起不来 | 复用 Pro mirror 链；关键镜像 `docker save` 备份 |

---

## 8. 工作项拆分（执行视图）

### 8.1 Worker

| ID | 项 | Phase |
|----|----|-------|
| W-1 | `BenchmarkVariant::Smith` + 配置 | 1 |
| W-2 | Smith catalog / instance 映射 + fixtures | 1 |
| W-3 | 镜像 ref + 本地 load 策略 | 1 |
| W-4 | `SwesmithGrader` / harness wrap | 1 |
| W-5 | Gateway 变体路径与工作区 | 1 |
| W-6 | 7143 部署配置与重启脚本备注 | 2 |
| W-7 | Trajectory 导出脚本 / 文档 | 3 |

### 8.2 OpenHands / Agent（208.77）

| ID | 项 | Phase |
|----|----|-------|
| A-1 | Smith driver + cwd/instruction | 2 |
| A-2 | AgentJob payload 带 `benchmark_variant=smith` | 2 |
| A-3 | 超时 / 并发 / LLM 配置文档化 | 2 |

### 8.3 Adapter / Bridge / Server

| ID | 项 | Phase |
|----|----|-------|
| B-1 | 提交脚本 / smoke：`benchmark_variant=smith` | 2 |
| B-2 | 确认透传与结果 JSONL | 2 |
| S-1 | 联调窗口确认 Agent 池与 Dispatch（通常无协议改动） | 2 |

### 8.4 训练验证（7142）

| ID | 项 | Phase |
|----|----|-------|
| T-1 | Rollout schema 校验工具 | 3 |
| T-2 | resolved 过滤 → 训练 JSONL | 3 |
| T-3 | 可选 1-step 训练消费 smoke | 3 |

### 8.5 Hub（后续负责人）

| ID | 项 | Phase |
|----|----|-------|
| H-1 | 注册 `swe-bench-smith` 包与 schema 枚举 | Hub |
| H-2 | 子集 catalog + 镜像分发 | Hub |
| H-3 |（可选）Episode stack seed | Hub |

---

## 9. 建议实施顺序（两周窗口示意）

| 日序 | 焦点 | 产出 |
|------|------|------|
| D1–D2 | W-1～W-4 + 1 个镜像导入 7143 | Worker 本地 session smoke |
| D3–D4 | W-5 + A-1；Gateway 联调 | OpenHands↔Smith 沙箱可交互 |
| D5–D6 | B-1 + 目标架构 E2E | 完整 Agent episode completed |
| D7–D8 | W-7 + T-1/T-2 | 可训练 JSONL 样例 |
| D9 | T-3（可选）+ 文档收口 | Phase 1–3 验收勾选 |
| 之后 | 交接 H-1～H-3 | Hub 模块适配 |

---

## 10. 参考链接

- SWE-smith 仓库：https://github.com/SWE-bench/SWE-smith  
- 训练指南：https://swesmith.com/guides/train_swe_agent/  
- 数据集：https://huggingface.co/datasets/SWE-bench/SWE-smith  
- 环境镜像资产：https://github.com/SWE-bench/SWE-smith-envs（`jyangballin/swesmith.x86_64.*`）  
- 本仓实机说明：`secrets/README.md`  
- 既有 Pro+OpenHands：`Docs/older/260627-swe-openhands-integration-plan.md`
