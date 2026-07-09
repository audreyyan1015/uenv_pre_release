# 五类 Benchmark — Worker 支持现状与跨层调整

> 日期：2026-07-09  
> 范围：PubMedQA、SciTab、DSCodeBench、SWE-bench-Pro、OlymMATH-EASY/HARD  
> 对照文档：[DSCodeBench-CodeEnv-扩展规划草案](./DSCodeBench-CodeEnv-扩展规划草案.md)、[实机联调记录-code-env](./实机联调记录-code-env.md)

---

## 总览结论

| # | Benchmark | 任务类型 | 推荐 `env_type` | Worker 侧 | 整体可用性 |
|---|-----------|----------|-----------------|-----------|------------|
| 1 | **PubMedQA** | 文本阅读理解（yes/no/maybe） | `math` 或新建 `reading` | ❌ **未支持** | 需新建 backend |
| 2 | **SciTab** | 表格理解（支持/反驳/不足） | 新建 `reading` / `table` | ❌ **未支持** | 需新建环境能力 |
| 3 | **DSCodeBench** | 代码生成 + 官方测试 | `code` | ⚠️ **部分支持（MVP）** | Worker 插件已落地；官方数据与全链路待补 |
| 4 | **SWE-bench-Pro** | 测试生成 / 程序修复 | `swe` | ✅ **已支持** | 7143 实机联调路径成熟 |
| 5 | **OlymMATH** | 奥赛级数学推理 | `math` | ❌ **未支持** | 仅有 `gsm8k` backend，需新增 `olymmath` |

**5 项中 Worker 侧完整就绪 1 项（SWE-bench-Pro），MVP 就绪 1 项（DSCodeBench），未支持 3 项。**

---

## UEnv 环境能力对照

```
                    ┌─────────────────────────────────────────┐
  已支持             │ swe          SWE-bench-Pro（容器+pytest）│
                    ├─────────────────────────────────────────┤
  MVP / 部分         │ code         DSCodeBench（插件+子进程评测）│
                    ├─────────────────────────────────────────┤
  仅 GSM8K           │ math         gsm8k 规则判分              │
                    │              ↑ OlymMATH / PubMedQA 可扩展  │
                    ├─────────────────────────────────────────┤
  未实现             │ reading/table SciTab / PubMedQA 专用     │
                    └─────────────────────────────────────────┘
```

---

## 逐项详情

### 1. PubMedQA（文本阅读理解）

| 维度 | 说明 |
|------|------|
| 官方输出 | `yes` / `no` / `maybe` |
| 输入 | PubMed abstract 上下文 + 生物医学问题 |
| **Worker 现状** | ❌ 无 `dataset=pubmedqa` backend；`math` 插件仅实现 `gsm8k` + 通用精确匹配 |
| 能否复用 math | **不充分**：三分类 + 摘要上下文需专用 prompt 模板与答案归一化（大小写、标点、同义表述） |
| 推荐映射 | `env_type=math`，`dataset=pubmedqa`（或 PRD 规划中的独立 `reading` env） |

#### Worker 侧待做

| 项 | 内容 |
|----|------|
| W-1 | `plugins/math/src/backends/pubmedqa/scoring.rs`：提取并归一化 yes/no/maybe |
| W-2 | `uenv-math-plugin` 按 `dataset` 路由到 pubmedqa scorer |
| W-3 | `build_reset_config` 透传 `context` / `abstract`（若与 `question` 分离） |
| W-4 | 单轮 episode + fixture |

#### 其他模块

| 模块 | 调整 |
|------|------|
| Bridge | `verl_agent_loop._env_type()` 增加 `pubmedqa` → `math`；`sample_to_worker_payload` 透传 abstract 字段 |
| Hub | math manifest `datasets` 增加 `pubmedqa`；可选数据集元数据 |
| Server | 无（按 `env_type=math` 调度） |
| 部署 | 无额外运行时依赖 |

---

### 2. SciTab（表格理解）

| 维度 | 说明 |
|------|------|
| 官方输出 | claim 被表格 **支持 / 反驳 / 信息不足**（三分类） |
| 输入 | 科学论文表格（HTML/LaTeX/结构化） + 自然语言 claim |
| **Worker 现状** | ❌ 完全未实现；无表格解析、无三分类判分 |
| 推荐映射 | 新建 **`env_type=reading`** 或 **`table`**（与 PRD「验证型」扩展一致）；`dataset=scitab` |

#### Worker 侧待做

| 项 | 内容 |
|----|------|
| W-1 | 新建 `plugins/reading/`（或扩 math，但不推荐混用） |
| W-2 | SciTab backend：表格序列化进 observation；step 内三分类匹配 |
| W-3 | payload 契约：`table_html` / `table_id`、`claim`、`gold_label` |
| W-4 | 可选：表格渲染/截断策略（防 context 过长） |

#### 其他模块

| 模块 | 调整 |
|------|------|
| Bridge | 新 env_type 映射；payload 透传表格与 claim 字段 |
| Hub | 新 env 注册 + manifest；SciTab 数据制品说明 |
| Server | Worker 注册 `supported_env_types` 含新 env_type |
| Worker 配置 | `env.types` 增加 `reading`；独立 WarmupPool |

---

### 3. DSCodeBench / DS-Bench（代码生成）

| 维度 | 说明 |
|------|------|
| 官方输出 | Python 代码 → 官方 test harness 执行 |
| **Worker 现状** | ⚠️ **MVP 已落地**（2026-07-09） |
| 已实现 | `plugins/code/`、`uenv-code-plugin`、`dataset=dscodebench`、`evaluate_code.py` |
| 7143 验证 | m4/m5 插件与 Executor 测试通过；code 预热池 dispatch 正常 |
| 未完成 | 官方 `benchmark/` 数据未部署；`test_script_path` 全量模式待实机；Podman 沙箱（Phase 2） |

#### Worker 已有能力

```
env_type=code → plugins/code → extract 代码 → Python 评测
  ├─ inline test_code（smoke / 联调）     ✅
  └─ test_script_path + UENV_DSCODEBENCH_ROOT  ⚠️ 待部署数据
```

#### Worker 侧剩余

| 项 | 内容 |
|----|------|
| W-1 | 7143 安装 DSCodeBench 官方 benchmark + 10 库 Python 环境 |
| W-2 | 对接官方 `benchmark_construction_evaluation/evaluate.py` |
| W-3 | Phase 2：Podman 沙箱 backend |
| W-4 | 持久化 `UENV_CODE_PLUGIN_BIN` 至 `/root/.uenv-worker.env` |

#### 其他模块

| 模块 | 状态 |
|------|------|
| Bridge | ✅ code 字段透传已加；待 `dscodebench` → `code` 显式映射与 VeRL Dataset 适配 |
| Hub | ⚠️ code env 仍为 placeholder manifest，需发布真实版本 |
| Server | ✅ 无需改；⚠️ adapter-core 需重编译以含 Bridge 透传 |
| 配置 | ✅ `env.types` 已含 `code`（`deploy-7143-swe-pro.yaml`） |

详见：[实机联调记录-code-env](./实机联调记录-code-env.md)

---

### 4. SWE-bench-Pro（测试生成 / 程序修复）

| 维度 | 说明 |
|------|------|
| 评测方式 | 容器内 apply patch → pytest → 解析报告 |
| **Worker 现状** | ✅ **已支持**（native 路径，非 L2 UDS 插件主路径） |
| 架构 | `env_type=swe` → `EpisodeExecutor.execute_swe_episode` → Docker 容器 + harness |
| 7143 配置 | `env.types: [math, code, swe]`；`swe.env_package_dir`；Runtime Gateway `:28097` |
| 变体 | `benchmark_variant=pro`（与 verified/lite 同框架） |

#### Worker 已有组件

| 组件 | 路径 |
|------|------|
| SWE 执行分支 | `uenv-worker/src/episode/executor.rs` |
| Harness / pytest | `uenv-worker/src/swe/harness.rs`、`session.rs` |
| 实例池 / 镜像 | `uenv-worker/src/swe/instance_pool.rs` |
| Runtime Gateway | `uenv-worker/src/runtime_gateway/mod.rs` |
| EnvPackage | `/var/lib/uenv/envs/swe-bench-pro/0.2.0` |
| OpenHands 集成 | `integrations/openhands/` + 208.77 Agent 池 |

#### Worker 侧待优化（非阻塞）

| 项 | 说明 |
|----|------|
| Hub swe manifest | Hub 上 `swe` 版本 404 时降级本地（日志已见 `hub_pull_failed`） |
| Agent 编排 | SWE+Agent 全链路依赖 Server `AgentJob` + 208.77 poll 模式 |

#### 其他模块

| 模块 | 状态 |
|------|------|
| Bridge | ✅ swe payload 字段透传（`instance_id`、`benchmark_variant` 等） |
| Hub | ⚠️ 发布 swe-bench-pro catalog / manifest |
| Server | ✅ 调度 + 可选 AgentJob 编排 |
| Agent 池 | 208.77 OpenHands（旁路或 Server poll 模式） |

---

### 5. OlymMATH-EASY / HARD（数学题求解）

| 维度 | 说明 |
|------|------|
| 官方输出 | 奥赛级自然语言计算题答案（常含 LaTeX、分数、区间） |
| **Worker 现状** | ❌ 无 `dataset=olymmath` backend |
| math 插件现状 | 仅 `gsm8k`（`####` 提取 + 归一化）+ 通用 `trim` 精确匹配 |
| 差距 | OlymMATH 需 **LaTeX/符号数学等价判定**（SymPy 或官方 normalize），非 GSM8K 规则可满足 |

#### Worker 侧待做

| 项 | 内容 |
|----|------|
| W-1 | `plugins/math/src/backends/olymmath/scoring.rs`：答案提取 + 等价判定 |
| W-2 | 可选 SymPy 子进程判分（类似 code 的 Python executor） |
| W-3 | `dataset` 区分 `olymmath-easy` / `olymmath-hard` |
| W-4 | payload：`question`、`split`（easy/hard）、`target` |

#### 其他模块

| 模块 | 调整 |
|------|------|
| Bridge | `olymmath` / `olym_math` → `env_type=math` |
| Hub | math manifest `datasets` 增加 olymmath |
| Server | 无 |
| 部署 | 可选 Python + SymPy（若用符号等价） |

---

## Worker 配置现状（7143 实机）

当前 `config/uenv-worker.deploy-7143-swe-pro.yaml`：

```yaml
env:
  types: ["math", "code", "swe"]
```

| env_type | 插件/路径 | 支持的 dataset / variant |
|----------|-----------|--------------------------|
| `math` | `plugins/math` → `uenv-math-plugin` | `gsm8k`（✅）；pubmedqa、olymmath（❌） |
| `code` | `plugins/code` → `uenv-code-plugin` | `dscodebench`（⚠️ MVP） |
| `swe` | Worker native（Docker） | `pro` / verified / lite（✅ pro 已联调） |

---

## 跨层调整优先级建议

| 优先级 | Benchmark | 理由 |
|--------|-----------|------|
| P0 | DSCodeBench | Worker MVP 已有；补 Bridge 映射 + Hub manifest + 官方数据部署即可闭环 |
| P0 | SWE-bench-Pro | 已支持；仅需 Hub manifest 与 Agent 编排按需启用 |
| P1 | OlymMATH | 可复用 math env_type + 新 backend，改动面小于新建 env |
| P1 | PubMedQA | 同上，三分类 backend 相对简单 |
| P2 | SciTab | 需新 env 能力与表格 payload，改动面最大 |

---

## 其他模块共性调整（未支持项通用）

| 模块 | 共性工作 |
|------|----------|
| **uenv-bridge** | `verl_agent_loop._env_type()` 任务名映射；`sample_to_worker_payload` 按 env_type 透传专用字段；VeRL Dataset → `env_config` 对齐 |
| **uenv-hub** | 各 env manifest 的 `datasets` / `config_schema` / examples 发布 |
| **uenv-server** | 仅新 env_type 时需 Worker 注册对应类型；调度逻辑不变 |
| **proto** | 一般不变；丰富 info 走 JSON |
| **fixtures** | 每 benchmark 至少 1 组 `episode_*.textproto` + smoke 样本 |
| **Docs/260709** | 每 benchmark 落地后追加实机联调记录 |

---

## 与规划草案的关系

| 文档 | 内容 |
|------|------|
| [DSCodeBench-CodeEnv-扩展规划草案](./DSCodeBench-CodeEnv-扩展规划草案.md) | DSCodeBench Worker 详细设计（已实现 MVP） |
| [实机联调记录-code-env](./实机联调记录-code-env.md) | DSCodeBench 7143 验证结果 |
| **本文档** | 五类 benchmark 横向支持矩阵与跨层待办 |

---

## 验收标准（按 benchmark）

| Benchmark | Worker 层验收 |
|-----------|---------------|
| PubMedQA | `env_type=math` + `dataset=pubmedqa` → yes/no/maybe 判分与官方一致 |
| SciTab | 新 env → 三分类 claim 判分 |
| DSCodeBench | `env_type=code` → 官方 harness pass@1 与 golden 对齐 |
| SWE-bench-Pro | `env_type=swe` + `variant=pro` → patch 应用 + pytest reward=1 |
| OlymMATH | `env_type=math` + `dataset=olymmath-*` → 等价答案判分 |
