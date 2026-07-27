# 验证型环境改造与 DSCode Agent 评测 — 实施规划

> 日期：2026-07-22（2026-07-25 修订：联调目标路径）  
> 状态：规划（待实施 → 准备实机联调改造）  
> 前置结论：[Math环境与术语规范-可证明性与DSCode-Agent选型](./Math环境与术语规范-可证明性与DSCode-Agent选型.md)  
> **跨模块勾选清单**：[跨模块调整清单-qa改造与ToolEnv-Agent](./跨模块调整清单-qa改造与ToolEnv-Agent.md)  
> 关联：[五类 Benchmark Worker 支持现状](../260709/五类Benchmark-Worker支持现状与跨层调整.md)、[Hub 环境标准化指南](../../hub/uenv-hub环境标准化指南.md)

---

## 0. 目标与验收边界

| # | 目标 | 验收口径 |
|---|------|----------|
| **A** | 将现有 `env_type=math` 改造为基于公开环境（**verifiers SingleTurnEnv**）的单轮问答验证环境，并更名为更能体现语义的 `env_type` | 四 dataset（gsm8k / pubmedqa / scitab / olymmath）判分与 verifiers Rubric golden 对齐；调度键完成更名与兼容期 |
| **B** | 采用 **Verifiers ToolEnv** 作为 DSCodeBench 推荐 Agent，确认官方基线之外可跑 Agent 轨评测 | 轨道 A 单轮 pass@1 不变；轨道 B 独立指标可跑通 ≥1 库 smoke + 可选全量 |

**非目标（本规划不做）**：

- 用 OpenHands 跑 DSCode；
- 用 Agent 结果替换官方单轮主榜；
- 为每个 dataset 新建独立 `env_type`；
- 一次性删除全部历史 `math` 字符串（兼容期保留 alias）。

---

## 1. 工作流总览

```text
Phase 0  决策冻结（env_type 命名、兼容策略、Agent 分轨）
    │
    ├──────────────────────────────┐
    ▼                              ▼
Phase A  验证型环境改造            Phase B  DSCode ToolEnv Agent
  A1 脚手架 + verifiers 金标         B1 可行性确认（本文 §4）
  A2 Rubric 对齐四 dataset           B2 Bridge 轻量 ToolEnv 闭环
  A3 env_type 更名 + 兼容层          B3 独立指标轨与 smoke
  A4 Hub / 文档 / 部署切换           B4（可选）AgentJob 深度集成
    │                              │
    └──────────► Phase C 联调验收 ◄─┘
```

建议并行：A1–A2 与 B1–B2 可并行；A3 更名与 B3 指标轨在联调窗口合并验收。

---

## 2. Phase 0 — 决策冻结

### 2.1 新 `env_type` 命名

| 候选 | 含义 | 优劣 |
|------|------|------|
| **`qa`（推荐）** | 单轮问答评测 | 短、直观；与「数学」脱钩；与 `code`/`swe` 并列清晰 |
| `verifier` | 可验证奖励环境 | 对齐 verifiers / RLVR 术语；略长；易被误解为「只做数学验证」 |
| `single_turn` | 交互形态 | 语义准；过长，且未体现「问答/分类」任务 |

**冻结决策：目标调度键 = `qa`。**

产品表述：

> `env_type=qa`：基于可验证 Rubric 的**单轮问答 / 分类评测**任务环境（Task Environment）。  
> 历史名 `math` 仅作兼容 alias，不再作为新文档主称谓。

插件目录 / 二进制建议同步：

| 现状 | 目标 |
|------|------|
| `plugins/math/` | `plugins/qa/`（迁移后；过渡期可 soft-link 或双注册） |
| `uenv-math-plugin` | `uenv-qa-plugin` |
| Hub `math@0.2.0` | Hub `qa@0.3.0`（新 registry）；`math` yank 或标 deprecated |
| `MathEnv` 文案 | `QaEnv` / 「单轮问答验证环境」 |

### 2.2 兼容策略（必须）

| 层 | 策略 |
|----|------|
| Bridge `_env_type()` | `gsm8k/pubmedqa/scitab/olymmath` → 返回 **`qa`**；输入仍含 `math` 时归一为 `qa` |
| Worker 注册 | `env.types: ["qa", "code", "swe"]`；过渡期可额外注册 `math` 指向同一插件 |
| Server 调度 | 无硬编码；只匹配 Worker 上报的 `supported_env_types` |
| Payload / fixtures | 新样本写 `env_type=qa`；旧 fixture 批量改或脚本兼容双读 |
| 评测脚本 | `evaluate_*_uenv.py` 默认 `env_type=qa`；保留 `--env-type` 覆盖 |

兼容窗口建议：**至少一个全量评测周期**（约 2–4 周）后再 yank Hub `math`。

### 2.3 公开环境改造原则

| 原则 | 说明 |
|------|------|
| 金标来源 | **PrimeIntellect verifiers**（`SingleTurnEnv` + `Rubric`） |
| 契约对齐（辅） | OpenEnv `interface` schema，不替代 Rubric |
| 调度壳保留 | 仍走 UEnv Worker 插件 `Reset/Step/Close`；不把 Episode 热路径改成直接调 Python verifiers 进程为主路径（金标可用于对齐与可选 sidecar） |
| 判分权威 | 改造完成后：**以 verifiers Rubric 语义为准**；Rust backend 实现须与其一致或调用同源逻辑 |

---

## 3. Phase A — 验证型环境（`math` → `qa`）改造详细规划

### 3.1 目标架构

```text
                    EpisodeRequest(env_type=qa, dataset=…)
                                    │
                                    ▼
┌──────────────────────────────────────────────────────────────┐
│  uenv-qa-plugin（原 math 插件壳）                             │
│  Reset → Observation(question)                               │
│  Step(action=模型输出) → score_action(dataset, action, gt)   │
└──────────────────────────────┬───────────────────────────────┘
                               │ 语义对齐 / golden 测试
                               ▼
┌──────────────────────────────────────────────────────────────┐
│  verifiers 金标包（仓库内 vendored 或 Hub 制品）              │
│  plugins/qa/rubrics/                                         │
│    gsm8k.py      → MathRubric / math_verify                  │
│    olymmath.py   → MathRubric（boxed + 符号等价）            │
│    pubmedqa.py   → LabelRubric(yes/no/maybe)                 │
│    scitab.py     → LabelRubric(supports/refutes/nei)         │
│  SingleTurnEnv 仅用于离线对齐与 CI，不进热路径默认路径         │
└──────────────────────────────────────────────────────────────┘
```

**热路径选择（实施时二选一，推荐方案 1）：**

| 方案 | 做法 | 优点 | 缺点 |
|------|------|------|------|
| **A-热1（推荐）** | Rust 判分重写/对齐，Python Rubric 仅作 CI golden | 保持现有插件性能与隔离 | 需维护双实现一致性测试 |
| A-热2 | Step 调本地 Python 子进程执行 Rubric | 单一权威实现 | 延迟↑、依赖 Python 运行时 |

本规划默认 **A-热1**：Rust 实现 + Python Rubric golden 门禁。

### 3.2 阶段拆解

#### A0 — 仓库脚手架与依赖（约 1–2 天）

| ID | 任务 | 产出 | 负责层 |
|----|------|------|--------|
| A0-1 | 在仓库增加 `third_party` 或 `plugins/qa/verifiers_align/`：pin `verifiers` + `math_verify` 版本与安装脚本（内网 wheel 预缓存） | `requirements-qa-rubric.txt` + Hub wheel 清单 | Hub / Worker |
| A0-2 | 新建 `plugins/qa/` 目录骨架（可由 `plugins/math` 复制），manifest `env_type: qa` | `plugins/qa/manifest.yaml` | Worker |
| A0-3 | 编写四份 Rubric 参考脚本（可先空壳 + gsm8k MathRubric） | `rubrics/*.py` | Worker |
| A0-4 | CI job：`python -m pytest plugins/qa/tests/test_rubric_golden.py`（先 skip） | CI 占位 | CI |

#### A1 — 四 dataset Rubric 金标（约 3–5 天）

| ID | dataset | Rubric 设计 | 对齐样本 |
|----|---------|-------------|----------|
| A1-1 | `gsm8k` | verifiers `MathRubric` 或等价 `math_verify` | fixtures + 官方 test 抽样 ≥50 |
| A1-2 | `olymmath-easy/hard` | MathRubric + `\boxed{}`；失败样本进回归集 | 现有 OlymMATH 评测失败/通过各 ≥30 |
| A1-3 | `pubmedqa` | 自定义 `LabelRubric`：提取 yes/no/maybe，规则与现 Rust 别名表对齐后**冻结为金标** | `fixtures/math/samples/pubmedqa_smoke.json` + 评测集抽样 |
| A1-4 | `scitab` | 自定义 `LabelRubric`：supports / refutes / not enough info | SciTab smoke + 抽样 |

**每个 Rubric 必须交付：**

1. `score(completion: str, answer: str) -> float`（0.0 / 1.0）；  
2. `extract(completion: str) -> Optional[str]`（可测）；  
3. 与当前 Rust `answers_match` 的 **diff 报告**（一致率、不一致样例表）；  
4. 不一致时的 **裁决规则**：以 Rubric 为准 → 改 Rust；或文档化「保留 UEnv 别名扩展」并写入 Rubric。

#### A2 — Rust 判分改造与 golden 门禁（约 3–5 天）

| ID | 任务 | 验收 |
|----|------|------|
| A2-1 | 按 A1 diff 修正 `plugins/qa/src/backends/*/scoring.rs` | 同批样本与 Rubric 一致率 ≥ 99.5%（或 100% 于冻结集） |
| A2-2 | olymmath：引入 `math_verify` 子进程或绑定为 fallback matcher（P1） | 复杂 LaTeX 假阴性下降；有 golden |
| A2-3 | `cargo test -p uenv-qa-env` + Python golden 双跑 | CI 绿 |
| A2-4 | 保留 `plugins/math` 为 deprecated symlink / 双 manifest 指向同一实现（过渡） | Worker 同时识别 `math`/`qa` 或仅 `qa`+Bridge 归一 |

#### A3 — `env_type` 更名跨层清单（约 2–4 天）

按依赖顺序改，**禁止**只改文档不改路由。

| 层 | 改动点 | 说明 |
|----|--------|------|
| **插件** | `plugins/qa/manifest.yaml`；`uenv-qa-plugin` bin；`UENV_QA_PLUGIN_BIN` | 环境变量新旧双读 |
| **Worker** | `config/*.yaml` `env.types`；`WarmupPool` prewarm；fixture 路径 `fixtures/qa/` | 可从 `fixtures/math` 迁移并留 README 指向 |
| **Bridge** | `verl_agent_loop._env_type()`：数学/问答类 → `qa`；`default_env_type=qa`；`reward_type` 分支 `env_type == "qa"` | `math` token 映射到 `qa` |
| **Adapter Core** | 若有 `env_type=="math"` 硬编码，改为 `qa` 或 `is_qa_env()` | 查 `core.rs` / `l1_mapping.rs` |
| **Hub** | seed `qa@0.3.0`；`math` 标记 deprecated / yank 计划；smoke package 改名 | `math-smoke-fixtures` → `qa-smoke-fixtures` |
| **评测脚本** | `evaluate_pubmedqa_uenv.py` 等默认 `env_type=qa` | smoke grpcurl 同步 |
| **文档** | 五类 Benchmark 总览表、PROTOCOL、评测 doc | 统一称 `qa` |
| **部署** | 7143 `/root/.uenv-worker.env`、deploy yaml | 滚动：先双注册再摘 `math` |

**归一化伪代码（Bridge）：**

```python
QA_TOKENS = {"qa", "math", "gsm8k", "pubmedqa", "scitab", "olymmath", ...}

def _env_type(...):
    if any(t in lowered for t in QA_TOKENS):
        return "qa"
    ...
```

#### A4 — OpenEnv 契约（可选，P2，约 1–2 天）

| ID | 任务 |
|----|------|
| A4-1 | 为 `qa` 写 `interface.action/observation/state` JSON Schema（单轮：action=文本答案，observation=题目，state=dataset+target） |
| A4-2 | Hub publish 校验通过；文档示例 `uenv env init` 对齐 |
| A4-3 | （可选）`openenv import` verifiers 包作对外互操作 demo，**不替代** Worker 热路径 |

#### A5 — 验收标准（Phase A）

| # | 标准 |
|---|------|
| A-V1 | `env_type=qa` + 四 dataset smoke → `reward=1.0`（现有 golden 答案） |
| A-V2 | Rubric vs Rust 冻结集一致率达标；报告入库 `Docs/worker/260722/` 或 `Docs/debug_log/` |
| A-V3 | Bridge 路由：pubmedqa/scitab/olymmath/gsm8k → `qa`；旧 `math` 输入不报错 |
| A-V4 | 7143 Worker `supported_env_types` 含 `qa`；Hub `GET /envs/qa/versions/latest` 可用 |
| A-V5 | 文档与五类矩阵已改称「qa 单轮问答验证环境」 |

---

## 4. Phase B — DSCodeBench + Verifiers ToolEnv Agent

### 4.1 可行性结论（确认）

**可以。** 在保留官方单轮基线之外，**可以**结合推荐 Agent（Verifiers `ToolEnv`）做 DSCodeBench 测试，但必须满足：

| 条件 | 说明 |
|------|------|
| **分轨** | 轨道 A = 官方可比 `pass@1`；轨道 B = `agentic_pass@1`（或 `pass@1_agent_maxN`），**禁止混表** |
| **终局判分不变** | Agent 多轮只改「如何得到候选代码」；最终仍走现有 `env_type=code` + `dscodebench_harness` |
| **工具边界** | 仅 Python REPL + submit；不含 bash 全仓编辑、browser（区别于 OpenHands） |
| **主榜归属** | 对外主报告仍以轨道 A 为准；轨道 B 标为「Agentic 扩展设定」 |

因此：

- **是否可以结合 Agent 测 DSCodeBench？** → **可以，作为扩展轨。**  
- **是否应用 Agent 替代官方基线？** → **不可以。**

### 4.2 推荐 Agent 与集成方式

**Agent：Verifiers `ToolEnv`**

| 工具 | 作用 |
|------|------|
| `run_python(code)` | 在与 DSCode 依赖一致的 venv 中执行，返回 stdout/stderr/exit |
| `submit_code(code)` | 结束多轮；触发 UEnv `code` env harness 全量评测 |

**集成路径（分两期）：**

```text
【B-轻量 · 推荐先做】
Bridge/脚本侧 ToolEnv 循环（多轮 LLM + REPL）
        │ submit_code
        ▼
现有 EpisodeRequest(env_type=code, dataset=dscodebench, …)
        │
        ▼
Worker code plugin → dscodebench_harness → reward
（可选：跳过 Worker 内二次 LLM，直接 response_text=提交代码）

【B-深度 · 可选后期】
execution_mode=agent + 轻量 code-agent driver
        │
        ▼
Server AgentJob（复用 SWE 编排模式，换 driver，不用 OpenHands）
```

### 4.3 与当前官方基线的差异（实施视角）

| 维度 | 轨道 A（现状 / 官方基线） | 轨道 B（ToolEnv Agent） |
|------|---------------------------|-------------------------|
| 脚本 | `evaluate_dscodebench_uenv.py` | 新建 `evaluate_dscodebench_agent_uenv.py` |
| LLM 次数 | 1 | ≤ `max_agent_steps`（建议默认 5） |
| 中间执行 | 无 | 每步 REPL（**非正式测试**，可限时/限内存） |
| 终局 harness | 200 tests / 题 | **相同** |
| Episode 构造 | Worker 内 Infer + Step | Adapter 注入 `response_text`=最终代码，或 `skip_infer=true` 类字段 |
| 指标 | `pass@1` | `agentic_pass@1`、`avg_steps`、`tool_calls`、`repl_error_rate` |
| 成本 | 低 | 显著更高（步数 × token × REPL） |

### 4.4 阶段拆解

#### B0 — 设计冻结与依赖（约 1 天）

| ID | 任务 | 产出 |
|----|------|------|
| B0-1 | 冻结工具 schema、`max_agent_steps=5`、`repl_timeout_secs`、禁止安装新包 | 规格写入本目录短文或本节 |
| B0-2 | 确认 REPL 使用与 Worker 相同的 DSCode venv（`UENV_CODE_PYTHON` / Hub sync 路径） | 环境一致性说明 |
| B0-3 | 指标字典与 JSON schema（`metrics_agent.json`） | 字段表 |

#### B1 — 轻量闭环 MVP（约 3–5 天）

| ID | 任务 | 验收 |
|----|------|------|
| B1-1 | 实现 ToolEnv 包装：tools=`run_python`/`submit_code`；LLM 走现有 Model Gateway | 单题本地可跑 |
| B1-2 | `submit_code` 后调用现有 Adapter/`EpisodeRequest`，`response_text` 注入最终代码，**Worker 不再二次生成** | reward 与单轮脚本同一 harness |
| B1-3 | 日志：每步 tool call、observation 截断入库 | `uenv_agent_trace.jsonl` |
| B1-4 | smoke：每库 1 题或 `numpy` 5 题 | 无崩溃；有 metrics |

**关键实现约束：**

```text
正式 harness 只在 submit 后跑一次。
REPL 步骤不得调用 dscodebench 官方 200-case 全量测试（成本过高），
仅允许短脚本自检 / 打印 shape / 小样例。
```

#### B2 — 评测脚本与分轨报告（约 2–3 天）

| ID | 任务 |
|----|------|
| B2-1 | `run_dscodebench_agent_uenv_baseline.sh`（默认小样本；全量需显式 `--full`） |
| B2-2 | 输出目录与轨道 A 隔离，例如 `…/dscodebench/…_agent_toolenv_…/` |
| B2-3 | 文档更新：基线评测 doc 增加「轨道 B」章节，强调不可混报 |
| B2-4 | （可选）同模型同题对比表：`pass@1` vs `agentic_pass@1` |

#### B3 — 深度集成（可选，P2，约 1–2 周）

| ID | 任务 | 前置 |
|----|------|------|
| B3-1 | 定义 `agent_bridge` 包：`uenv-agent-toolenv`（对标 `uenv-agent-openhands`） | B1 稳定 |
| B3-2 | Server `AgentJob` 支持 `agent_kind=toolenv` | B3-1 |
| B3-3 | Worker Runtime Gateway 暴露受限 `exec_python`（若 REPL 下沉到 Worker） | 安全评审 |
| B3-4 | 与 SWE OpenHands 池隔离（独立 pool_id） | — |

**本规划实机联调以 B3（208.77 Agent 池）为目标交付；B1/B2 作为开发/冒烟脚手架保留。跨模块注册项见[跨模块调整清单](./跨模块调整清单-qa改造与ToolEnv-Agent.md)。**

#### B4 — 验收标准（Phase B）

| # | 标准 |
|---|------|
| B-V1 | 轨道 A 全量脚本行为与指标定义不变 |
| B-V2 | 轨道 B smoke（≥1 库）完成，终局 harness 与 A 同源 |
| B-V3 | 报告中明确 `agentic_pass@1`，无与 `pass@1` 混列 |
| B-V4 | 文档写明：推荐 Agent = Verifiers ToolEnv；OpenHands 不用于 DSCode 主路径 |

### 4.5 风险与缓解

| 风险 | 影响 | 缓解 |
|------|------|------|
| Agent 轨成本过高 | 全量 1000×5 步不可承受 | 默认小样本；全量需审批；限制 `max_agent_steps` |
| REPL 与 harness 环境不一致 | 假通过 / 假失败 | 强制同一 venv；CI 校验 `sys.path`/`numpy.__version__` |
| 模型在 REPL 中「偷看」过大测试 | 泄漏 | REPL 不挂载官方 test_script；仅用户自写短码 |
| 与 SWE AgentJob 资源争用 | 池打满 | 独立 pool / 限流 |
| 误用 OpenHands | 语义跑偏 | 文档与脚本入口禁止默认绑定 OpenHands |

---

## 5. 跨层改动清单（速查）

### 5.1 Phase A（`qa`）必改模块

| 模块 | 必改？ | 要点 |
|------|--------|------|
| `plugins/math` → `plugins/qa` | ✅ | manifest、backends、golden |
| `uenv-worker` bin / config | ✅ | plugin 名、`env.types`、env 变量 |
| `uenv-bridge` VeRL / 评测脚本 | ✅ | `_env_type` → `qa` |
| `uenv-bridge/core` | ⚠️ | 扫 `math` 硬编码 |
| `uenv-hub` seed / fixtures | ✅ | `qa@0.3.0` |
| `uenv-server` | ❌ 源码通常无需改 | 仅部署侧确认 Worker 注册 |
| proto | ❌ | `env_type` 仍为 string |
| Docs / PROTOCOL | ✅ | 术语与矩阵 |

### 5.2 Phase B（ToolEnv）必改模块

| 模块 | 必改？ | 要点 |
|------|--------|------|
| Bridge 新评测脚本 | ✅ | ToolEnv 循环 + 注入 `response_text` |
| `plugins/code` / harness | ❌ 终局不变 | 可选增加 `skip_infer` 文档约定 |
| Worker Model Gateway | ❌ | Agent 侧可直连同一 Gateway |
| Server AgentJob | 仅 B3 | 轻量路径不依赖 |
| OpenHands 集成 | ❌ | 明确不接入 |

---

## 6. 里程碑与建议排期

| 里程碑 | 内容 | 建议工期 | 依赖 |
|--------|------|----------|------|
| **M0** | 冻结 `qa` 命名 + 双轨指标字典 | 0.5 天 | — |
| **M1** | A0+A1：Rubric 四 dataset + diff 报告 | 1 周 | 内网 verifiers/math_verify wheels |
| **M2** | A2+A3：Rust 对齐 + `env_type=qa` 跨层切换（含兼容） | 1 周 | M1 |
| **M3** | A5 验收：7143 smoke 四 dataset + Hub `qa` | 2–3 天 | M2 |
| **M4** | B1+B2：ToolEnv smoke + 独立报告脚本 | 1 周 | 可与 M2 并行 |
| **M5** | （可选）B3 AgentJob 深度集成 | 1–2 周 | M4 |
| **M6** | yank Hub `math` / 移除双注册 | 兼容窗口后 | M3 稳定运行 ≥2 周 |

并行建议：M1∥M4 启动；M3 与 M4 smoke 共用联调窗口。

---

## 7. 交付物清单

| 交付物 | 路径（建议） |
|--------|----------------|
| 本实施规划 | `Docs/worker/260722/验证型环境改造与DSCode-Agent评测-实施规划.md`（本文） |
| Rubric 对齐报告 | `Docs/worker/260722/qa-rubric-golden-对齐报告.md`（实施后补） |
| `plugins/qa` + rubrics | `plugins/qa/` |
| Hub `qa@0.3.0` | Hub registry |
| DSCode Agent 脚本 | `uenv-bridge/scripts/benchmark/evaluate_dscodebench_agent_uenv.py` |
| DSCode Agent 文档节 | 更新 `uenv-bridge/docs/任务测评/DSCodeBench代码生成基线评测.md` |
| 五类矩阵更新 | `Docs/worker/260709/五类Benchmark-Worker支持现状与跨层调整.md`（`math`→`qa`） |

---

## 8. 决策记录（写入规划即生效）

| # | 决策 | 选择 |
|---|------|------|
| D1 | 新 env_type 名称 | **`qa`** |
| D2 | 公开环境改造基座 | **verifiers SingleTurnEnv + Rubric**；OpenEnv 仅契约可选 |
| D3 | 热路径判分 | Rust 对齐 + Python Rubric CI golden（方案 A-热1） |
| D4 | `math` 处理 | 兼容期 alias → 窗口后 yank |
| D5 | DSCode 官方基线 | **保持单轮**，不接 Agent |
| D6 | DSCode Agent | **可以扩展测试**；推荐 **Verifiers ToolEnv**；独立指标轨 |
| D7 | OpenHands 用于 DSCode | **否**（主路径） |
| D8 | Phase B 默认交付深度 | ~~轻量 Bridge 仅作开发脚手架~~ → **实机联调以 208.77 Agent 池 + AgentJob 为目标（B3）**；Bridge 脚本保留冒烟 |
| D9 | ToolEnv 部署位置 | **单独部署在 Agent 机 `8.130.208.77`**，与 OpenHands 同机隔离（独立 bridge / pool / systemd） |

---

## 9. 下一步（实施入口）

1. **按[跨模块清单](./跨模块调整清单-qa改造与ToolEnv-Agent.md)评审 Hub/208.77 注册项**，确认 D1–D9。  
2. **开 M0/M1**：冻结命名 → 拉 verifiers 依赖 → 先做 gsm8k + pubmedqa Rubric diff。  
3. **并行准备 ToolEnv@208.77**：Hub `uenv-agent-toolenv` 包骨架 + poller 模板（不必等 qa 全量完成）。  
4. 实机联调顺序见跨模块清单 §8；过程记录另存本目录，决策变更走修订记录。

### 修订记录

| 日期 | 说明 |
|------|------|
| 2026-07-22 | 初版：基于选型文档落地为可执行实施规划 |
| 2026-07-25 | 联调准备：确认规划合理；ToolEnv 主路径钉 208.77；增补跨模块清单链接与 D9 |
