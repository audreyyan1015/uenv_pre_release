# Math 环境与术语规范 — 可证明性与 DSCode Agent 选型

> 日期：2026-07-22  
> 范围：UEnv `env_type=math` 性质确认、公开验证环境改造选型、狭义/广义概念划分、DSCodeBench Agent 评测建议  
> 实施规划：[验证型环境改造与DSCode-Agent评测-实施规划](./验证型环境改造与DSCode-Agent评测-实施规划.md)（`math`→`qa` 改造 + ToolEnv 分轨）  
> 跨模块清单：[跨模块调整清单-qa改造与ToolEnv-Agent](./跨模块调整清单-qa改造与ToolEnv-Agent.md)  
> 关联文档：[五类 Benchmark Worker 支持现状](../260709/五类Benchmark-Worker支持现状与跨层调整.md)、[Hub 环境标准化指南](../../hub/uenv-hub环境标准化指南.md)、[DSCodeBench 基线评测](../../../uenv-bridge/docs/任务测评/DSCodeBench代码生成基线评测.md)

---

## 1. 背景与结论摘要

本轮讨论围绕三个问题展开：

| # | 问题 | 结论 |
|---|------|------|
| 1 | `math` 是自研还是改造？ | **自研通用验证型环境**；benchmark 数据来自公开集，判分与插件壳为自写 |
| 2 | 狭义/广义「环境」如何命名？ | 狭义 = **Task Environment**；Agent + 轨迹 + 调度 = **Episode Stack**；勿混称「广义环境」 |
| 3 | DSCodeBench 是否应接 Agent？ | **官方可比基线保持单轮**；若做 Agentic 实验，推荐 **Verifiers `ToolEnv` / OpenEnv `coding_env`**，不用 OpenHands 作主路径 |

---

## 2. Math 环境性质确认

### 2.1 自研通用验证型，而非基于现成环境改造

UEnv 的 `env_type=math`（MathEnv）在 PRD 中属于「3 核心自研」之一的**验证型**环境，manifest 描述为：

> `MathEnv — universal validation/computation environment`

实现位于 `plugins/math/` + `uenv-math-plugin`，通过 `dataset` 路由到不同 backend 的**规则判分**，而非包装 HuggingFace OpenEnv 镜像或第三方 MathEnv 发行版。

### 2.2 当前承载的 benchmark（不止 GSM8K）

`math` 被设计为**单轮可验证问答**的通用容器，同一 `env_type` 下挂多个 dataset：

| dataset | 任务类型 | 判分方式 | 实现路径 |
|---------|----------|----------|----------|
| `gsm8k` | 小学数学应用题 | `####` 提取 + 数值归一化精确匹配 | `backends/gsm8k/scoring.rs` |
| `pubmedqa` | 生物医学阅读理解 | 从自由文本提取 `yes` / `no` / `maybe` | `backends/pubmedqa/scoring.rs` |
| `scitab` | 科学表格 claim 验证 | 三分类：`supports` / `refutes` / `not enough info` | `backends/scitab/scoring.rs` |
| `olymmath` / `-easy` / `-hard` | 奥赛级数学题 | `\boxed{}` 提取 + LaTeX 归一化（MVP，非 SymPy） | `backends/olymmath/scoring.rs` |

因此，`math` 的准确语义是：

- **不是**「只做数学题的环境」；
- **而是**「单轮生成 → 规则提取 → 0/1 可验证奖励」的**通用验证器环境（Verifier Environment）**。

与 `code`（执行型）、`swe`（容器 + pytest 修复型）形成能力分层，见 [五类 Benchmark 文档](../260709/五类Benchmark-Worker支持现状与跨层调整.md) 总览表。

### 2.3 可证明性缺口

由于判分逻辑（标签提取、归一化、别名表）均为自实现，外部无法直接断言：

> 「UEnv math 的 reward 与某公开 benchmark 官方 harness 完全一致。」

尤其在 `olymmath`（自写 LaTeX 归一化）与 `pubmedqa` / `scitab`（自由文本标签提取）上，需要与公开 scorer 做 **golden 对齐** 才能对外证明可比性。

---

## 3. 最符合改造需求的公开环境选型

### 3.1 需求画像（由当前 math 用法推导）

| 维度 | 要求 |
|------|------|
| 交互形态 | **单轮**（`max_steps=1`），与 pubmedqa / scitab / gsm8k / olymmath 一致 |
| 奖励类型 | **规则可验证**（0/1 或离散分类），非 LLM-as-judge 为主 |
| 多 dataset | 同一环境壳下，按任务挂不同 **Rubric / Parser**，而非每种 bench 一个独立 env_type |
| 任务谱系 | 数值/math + 短标签分类 + 结构化提取，覆盖当前五类 math dataset |
| 与 UEnv 关系 | 宜作**判分金标 / 契约对齐层**，而非替换 Worker 调度与 Hub 分发 |

### 3.2 候选公开环境对比

| 项目 | 类型 | 优势 | 对 math 多 dataset 的不足 |
|------|------|------|---------------------------|
| [PrimeIntellect **verifiers**](https://github.com/PrimeIntellect-ai/verifiers) | RLVR 环境库 | `SingleTurnEnv` + 可插拔 `Rubric`；文档明确支持 Q&A、**text classification**；内置 `MathRubric` | 需为 pubmedqa/scitab/olymmath 各写 Rubric，非开箱即用 |
| [HuggingFace **OpenEnv**](https://github.com/huggingface/OpenEnv) | Gym 风格 env 框架 | `reset/step/state` 契约与 UEnv Hub 标准化方向一致；支持 `openenv import` Verifiers/ORS | 偏「环境封装与分发」，判分仍依赖底层 Rubric |
| [OpenReward](https://openreward.ai/) GSM8K 等 | 托管验证环境 | `math_verify` 等价判定，工业界 RLVR 参考实现 | 以 GSM8K 等 math 为主，**无 pubmedqa/scitab 一等公民** |
| [reasoning-gym](https://github.com/open-thought/reasoning-gym) | 程序化推理环境 | 无限合成 + cascade scorer，适合 math RL 训练 | 偏合成数学题，**不覆盖**生物医学 QA、表格 claim 三分类 |
| OpenEnv `qed_math_env`（PR #865） | 证明/答案混合 | `math_verify` + LLM judge rubric | 过重；含 judge，不符合当前「纯规则」主路径 |

### 3.3 推荐结论：**PrimeIntellect verifiers（主）+ OpenEnv（契约包装，辅）**

**最符合「在保留 UEnv math 壳的前提下做可证明改造」的公开环境是 PrimeIntellect verifiers。**

理由：

1. **`SingleTurnEnv` 与当前 math 语义完全一致**  
   一次 prompt → 一次 completion → Rubric 打分，对应 UEnv 的 `Reset → Infer → Step → reward`。

2. **原生支持「单轮 + 多任务类型」扩展模式**  
   官方文档将 SingleTurnEnv 用于 Q&A、**text classification**、summarization 等；通过 `dataset` 字段 + 自定义 `Rubric(funcs=[...])` 即可覆盖 pubmedqa（三分类）、scitab（三分类）、gsm8k/olymmath（math rubric），与 UEnv `dataset` 路由模型同构。

3. **内置 `MathRubric` 可直接对齐 gsm8k / olymmath**  
   支持 `\boxed{}` 提取与符号等价（可对接 `math_verify`），用于 golden 对齐与逐步替换自写 `olymmath` 归一化。

4. **OpenEnv 可作为对外契约层，而非判分来源**  
   UEnv Hub 已对齐 OpenEnv `interface`（Action/Observation/State）；可用 `openenv import` 将 verifiers 环境包装为标准 Gym API，与 [标准化环境定义规范](../../hub/260716-标准化环境定义规范.md) 一致，但**判分逻辑仍以 verifiers Rubric 为权威**。

### 3.4 建议改造路径（不改 `env_type=math` 调度键）

```
┌─────────────────────────────────────────────────────────────┐
│  UEnv 保留层（不变）                                         │
│  env_type=math → uenv-math-plugin → dataset 路由            │
└───────────────────────────┬─────────────────────────────────┘
                            │ 判分对齐
                            ▼
┌─────────────────────────────────────────────────────────────┐
│  verifiers 金标层（新增）                                    │
│  gsm8k/olymmath  → MathRubric / math_verify                 │
│  pubmedqa        → 自定义 Rubric（yes/no/maybe 提取）        │
│  scitab          → 自定义 Rubric（supports/refutes/nei）     │
└───────────────────────────┬─────────────────────────────────┘
                            │ 可选
                            ▼
┌─────────────────────────────────────────────────────────────┐
│  OpenEnv 契约层（可选）                                      │
│  openenv import → Hub interface schema 对齐                 │
└─────────────────────────────────────────────────────────────┘
```

**落地步骤（建议优先级）**：

| 优先级 | 动作 | 产出 |
|--------|------|------|
| P0 | 用 verifiers `MathRubric` 对 gsm8k + olymmath-easy 抽样做 golden 对比 | 对齐率报告；定位自写 scorer 偏差 |
| P0 | 为 pubmedqa / scitab 实现 verifiers `Rubric`，与 `plugins/math` 同批样本对比 | 可证明的 0/1 一致性 |
| P1 | 引入 `math_verify` 库替换 olymmath MVP 归一化（或作为 fallback matcher） | 复杂 LaTeX 可证明性 |
| P2 | 可选 `openenv import` 生成标准 interface，写入 Hub manifest 示例 | 对外生态互操作 |

**不推荐的改造方向**：

- 用 OpenReward 单独环境替换整个 math → 无法覆盖 pubmedqa/scitab；
- 用 reasoning-gym 替换 math → 任务域不匹配；
- 为每个 dataset 新建独立 `env_type` → 破坏当前「通用验证器」设计与 Bridge 路由。

---

## 4. 术语规范：狭义环境 vs 配套栈

### 4.1 问题

当前「环境」一词在不同语境下混指：

- Worker 插件里的 `reset/step/score`；
- 含 Agent、reward 计算、轨迹保存的整条 Episode 链路。

需要稳定术语，避免与 OpenEnv / Gym 的 Environment 概念冲突。

### 4.2 推荐三层命名

```text
┌─ Platform / Control Plane ───────────────────────────────────┐
│  uenv-server 调度、uenv-hub 制品、轨迹索引、Prometheus 指标   │
└──────────────────────────────────────────────────────────────┘
                              │
┌─ Agent Scaffold（策略侧）────────────────────────────────────┐
│  OpenHands CodeAct、未来 Code REPL Agent、VeRL Agent Loop    │
│  职责：多轮推理、工具调用、消息历史；不定义 benchmark 判分规则  │
└──────────────────────────────────────────────────────────────┘
                              │
┌─ Task Environment（狭义环境）────────────────────────────────┐
│  plugins/math、plugins/code、SWE Docker+pytest               │
│  职责：observation、action 执行、规则/执行型 reward           │
│  对应 Hub env_type、OpenEnv Environment、Gymnasium Env       │
└──────────────────────────────────────────────────────────────┘
```

| 概念 | 建议中文 | 建议英文 | UEnv 对应 |
|------|----------|----------|-----------|
| 狭义环境 | **任务环境** / 验证器环境 | **Task Environment** / Verifier Env | `env_type` + Worker 插件 / SWE harness |
| Agent 与工具 | **Agent 脚手架** | **Agent Scaffold** | `integrations/openhands/`、`execution_mode=agent` |
| 广义配套 | **Episode 栈** / Rollout 栈 | **Episode Stack** / Rollout Stack | Bridge + Server + Worker 编排 + 轨迹 + reward 汇总 |
| 整体产品 | **环境执行平台** | **Environment Execution Platform** | UEnv 全栈 |

### 4.3 使用约定（写入后续文档）

1. **`env_type` 仅指 Task Environment 能力类**（`math` / `code` / `swe`），不涵盖 Agent。
2. **`dataset` 指 Task Environment 内的 benchmark 路由键**（如 `pubmedqa`、`dscodebench`）。
3. **不说「广义环境」**；Agent、轨迹、调度统称 **Episode Stack**。
4. **SWE + OpenHands** 是「Task Environment（swe）+ Agent Scaffold（OpenHands）」组合，不是把 OpenHands 算作环境本身。

---

## 5. DSCodeBench：Agent 选型与使用方式

### 5.1 当前评测方式（应保持为官方主基线）

当前 UEnv 全链路评测为**单轮代码生成**，与 DSCodeBench 官方 harness 语义一致：

```text
Adapter 构造 EpisodeRequest
  → Adapter Core / Server 调度
  → Worker code env 经 Model Gateway 调 vLLM（单次生成）
  → 抽取 Python 代码块
  → dscodebench_harness 执行（每题 200 tests）
  → reward 0/1
```

脚本：`uenv-bridge/scripts/benchmark/evaluate_dscodebench_uenv.py`  
文档：[DSCodeBench 代码生成基线评测](../../../uenv-bridge/docs/任务测评/DSCodeBench代码生成基线评测.md)

| 属性 | 当前实现 |
|------|----------|
| 交互轮次 | **1 轮**（一次 completion） |
| Agent | **无**（Worker 内嵌 LLM 调用 + harness） |
| 工具 | 无 REPL / 无文件编辑 |
| 主指标 | `pass@1`（与官方可比） |
| 评测模式 | `inline_harness`（Adapter 内联 `test_code`） |

**结论：该路径是 DSCodeBench 官方可比基线，应继续作为主报告口径。**

### 5.2 最推荐的 Agent（用于 Agentic 扩展实验）

若目标是评测「多轮写码、试运行、修正」能力（**非**官方 pass@1 主榜），推荐：

| 优先级 | Agent / 环境 | 推荐场景 | 不推荐原因（若误用） |
|--------|--------------|----------|----------------------|
| **首选** | **Verifiers `ToolEnv`**（Python REPL 工具） | 数据科学单函数题；多轮 `run_python` → 观察 stderr/输出 → 再提交 | — |
| **次选** | **OpenEnv `coding_env`**（沙箱 Python 执行） | 与 OpenEnv 生态对齐；smolagents 执行 stdout/stderr | 需额外包装进 UEnv Episode Stack |
| 对比实验 | **mini-SWE-agent** | 极简 bash loop，适合 agent 脚手架 ablation | 非 DSCode 官方设定 |
| **不作为 DSCode 主路径** | **OpenHands CodeAct** | 仅用于与 SWE 能力对照 | 面向仓库级 issue 修复，工具过重，与单函数 DS 题不匹配 |

**最终推荐：Verifiers `ToolEnv`（Python REPL + submit_code）**

- 与 DSCode「写 Python、看运行结果、改代码」的认知模型一致；
- 脚手架比 OpenHands 轻，不易把 pass@1 与「仓库导航能力」混淆；
- 与 verifiers 生态统一，便于和 math 侧 Rubric 对齐方法论。

### 5.3 Agent 模式 vs 当前单轮模式

| 维度 | 当前单轮（`evaluate_dscodebench_uenv.py`） | Agent 模式（建议：`ToolEnv`） |
|------|-------------------------------------------|------------------------------|
| **执行入口** | Adapter → `EpisodeRequest` → Worker `code` env | Adapter / Agent Loop 多轮调工具 |
| **LLM 调用** | Worker 内 1 次 `Model Gateway` | 每步 1 次，共 N 步（如 3–8） |
| **代码执行** | 生成后一次性 harness | 每步可在 REPL 试探，最终 `submit_code` 再跑全量 harness |
| **观测** | 无中间反馈 | 每步 stdout/stderr/异常 |
| **reward** | harness 全量测试结果 | 仍以**最终提交代码**的 harness 为准（保持可比） |
| **轨迹** | 单条 completion | 多步 tool call 轨迹（可入轨迹服务） |
| **与官方可比** | ✅ 直接可比 `pass@1` | ⚠️ 需单独报告为「agentic pass@k」，不与官方单轮混表 |
| **UEnv 改造量** | 已落地 | 需新增 `execution_mode=agent` 或 code 侧 multi-step 协议（可参考 SWE AgentJob） |

### 5.4 建议使用方式（分轨报告）

#### 轨道 A — 官方基线（继续当前做法）

```bash
# 全量 1000 题，单轮，UEnv code env
bash uenv-bridge/scripts/benchmark/run_dscodebench_uenv_baseline.sh
```

- 指标：`pass@1`、`execution_rate`、`error_category`
- 不与 Agent 结果混报

#### 轨道 B — Agentic 实验（可选，独立指标）

概念流程：

```text
1. 初始化 ToolEnv：tools = [run_python, submit_code]
2. 每题 max_steps = 5（可配置）
3. 每步：LLM 选择工具 → REPL 执行 → observation 回传
4. 收到 submit_code 后，仍走 UEnv code env / dscodebench_harness 做最终 0/1 判分
5. 记录 trajectory_id、步数、工具调用次数
```

**与 SWE OpenHands 的区别**：

| 项 | SWE + OpenHands | DSCode + ToolEnv（建议） |
|----|-----------------|--------------------------|
| Task Environment | `swe`（Docker 仓库 + pytest） | `code`（Python harness） |
| Agent | OpenHands CodeAct（bash + 文件编辑 + browser） | Verifiers ToolEnv（**仅 Python REPL**） |
| 任务粒度 | 多文件仓库修复 | 单函数数据科学题 |
| Server 路径 | `AgentJob` + 208.77 poll | 可复用 AgentJob 模式，但应换轻量 driver |
| 主指标 | patch resolve rate | agentic pass@1（需单独定义） |

**集成 UEnv 的两种路径**：

1. **轻量（推荐先做）**：在 Bridge 侧用 verifiers `ToolEnv` 跑通多轮，最终一步仍调用现有 `EpisodeRequest` + `code` env harness 出分；不改动 `env_type=code` 判分语义。
2. **深度（对齐 SWE）**：`execution_mode=agent` + 专用 code agent driver（非 OpenHands），由 Server `AgentJob` 调度；工作量大，适合 Phase 2。

### 5.5 明确不采纳的方案

| 方案 | 原因 |
|------|------|
| 用 OpenHands 作为 DSCode 默认 Agent | 面向 SWE 仓库任务，工具集与启动成本与 DSCode 单函数题不匹配 |
| 用 Agent 结果替代当前单轮主榜 | 破坏与 DSCodeBench 官方 pass@1 的可比性 |
| 为 DSCode 新建 `env_type=agent` | 判分仍在 code harness，无需新 env_type；Agent 属于 Scaffold 层 |

---

## 6. 行动项汇总

| ID | 模块 | 内容 | 优先级 |
|----|------|------|--------|
| M-1 | math / 文档 | 对外将 `math` 表述为「通用验证器环境（Verifier Env）」，不仅限数学 | P1 |
| M-2 | math / 判分 | 引入 verifiers Rubric 金标，覆盖 gsm8k / pubmedqa / scitab / olymmath | P0 |
| M-3 | math / 判分 | olymmath 对接 `math_verify` 或 verifiers `MathRubric` | P1 |
| M-4 | Hub | 可选 OpenEnv interface 对齐 verifiers 包装环境 | P2 |
| T-1 | 术语 | 团队文档统一 Task Environment / Episode Stack / Agent Scaffold | P1 |
| D-1 | DSCode | 主榜继续 `evaluate_dscodebench_uenv.py` 单轮基线 | — |
| D-2 | DSCode | 若做 Agent 实验：以 Verifiers `ToolEnv` 为首选，独立指标轨 | P2 |
| D-3 | DSCode | 不复用 OpenHands 作为 DSCode 主 Agent | — |

---

## 7. 参考链接

- [PrimeIntellect verifiers](https://github.com/PrimeIntellect-ai/verifiers) — SingleTurnEnv、Rubric、ToolEnv
- [Verifiers Single-Turn 指南](https://primeintellect-ai-verifiers.mintlify.app/guides/single-turn)
- [HuggingFace OpenEnv](https://github.com/huggingface/OpenEnv) — Gym API、`openenv import`
- [reasoning-gym](https://github.com/open-thought/reasoning-gym) — 合成 math RL（补充训练，非 math 全替代）
- [OpenReward GSM8K](https://openreward.ai/GeneralReasoning/GSM8K) — math_verify 参考实现
- [DSCodeBench 官方仓库](https://github.com/ShuyinOuyang/DSCodeBench)
- [mini-SWE-agent](https://github.com/SWE-agent/mini-swe-agent) — 轻量 agent 对比实验可选
