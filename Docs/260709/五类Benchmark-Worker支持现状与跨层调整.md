# 五类 Benchmark — Worker 支持现状与跨层调整

> 日期：2026-07-12（Hub `dscodebench@0.2.0` 全量预缓存入库；此前 07-11 Hub math/code@0.2.0 seed；07-09 merge + 7143 联调）  
> 范围：PubMedQA、SciTab、DSCodeBench、SWE-bench-Pro、OlymMATH-EASY/HARD  
> 对照文档：[DSCodeBench-CodeEnv-扩展规划草案](./DSCodeBench-CodeEnv-扩展规划草案.md)、[实机联调记录-code-env](./实机联调记录-code-env.md)、[Hub 环境标准化指南](../hub/uenv-hub环境标准化指南.md)

---

## 2026-07-12 进度快照

| 层 | 已完成 | 仍缺 / 下一步 |
|----|--------|---------------|
| **Worker** | 五类 Phase 1 就绪；DSCode 官方 harness + golden `ds_001` | 7143 `env sync` 消费 Hub 制品；Podman Phase 2 |
| **Hub** | `math@0.2.0` / `code@0.2.0`；`math-smoke-fixtures@0.1.0`；**`dscodebench@0.2.0` 全量**（benchmark + eval + wheels≈3.9GB） | **H-5**：SWE-Pro 真实 `image_tar` + catalog 替换占位 |
| **Bridge** | Core 字段透传；math 四 dataset / code smoke 透传 | **B-1/B-2**（显式 `dataset`、`dscodebench`→`code`）；**B-10** adapter-core 部署 |
| **uenv-server** | 调度通用，**源码无需改** | S-2/S-3 联调（EnvPackage 匹配、Agent E2E） |
| **SWE 镜像导入** | 7143 历史联调可用；多 mirror 脚本就绪 | 导入机按 mirror 链拉 `jefzda/sweap-images` → `docker save` → Hub `publish-image` |

**SWE 公网拉取线路（导入机 / 7143 历史实证，非生产运行时路径）**：优先脚本回退链  
`docker.m.daocloud.io` → `docker.nju.edu.cn` → **`dockerproxy.net`（实机成功 tag）** → 直连 Docker Hub（易 429）；daemon 另有 `docker.1ms.run`。生产目标仍是 Hub `image_tar` + `sync --docker-load`。详见 `scripts/pull-pro-image-7143.sh`。

---

## 总览结论

| # | Benchmark | 任务类型 | 推荐 `env_type` | Worker 侧 | 整体可用性 |
|---|-----------|----------|-----------------|-----------|------------|
| 1 | **PubMedQA** | 文本阅读理解（yes/no/maybe） | `math` | ✅ **已支持** | math backend + 7143 E2E 通过 |
| 2 | **SciTab** | 表格理解（支持/反驳/不足） | `math` | ✅ **已支持** | 复用 math env，三分类 backend 已落地 |
| 3 | **DSCodeBench** | 代码生成 + 官方测试 | `code` | ✅ **Phase 1 已支持** | Hub `dscodebench@0.2.0` 全量预缓存已就绪 |
| 4 | **SWE-bench-Pro** | 测试生成 / 程序修复 | `swe` | ✅ **已支持** | 7143 实机联调路径成熟 |
| 5 | **OlymMATH** | 奥赛级数学推理 | `math` | ✅ **已支持** | `\boxed{}` 提取 + 归一化；easy/hard split |

**Worker 侧**：5 项 Phase 1 就绪（DSCodeBench 含官方 harness；Podman 为 Phase 2）。

**跨层剩余（Blocking 闭环）**：主要在 **Bridge P0 代码**（B-1/B-2）、**Hub SWE Pro 镜像 tar**（H-5）、**Worker `env sync` 运维**；`uenv-server` **源码无需改**。

| 模块 | 代码要改？ | 当前状态 |
|------|-----------|----------|
| Worker | — | ✅ 五类 Phase 1 已就绪（DSCode 官方 harness ✅；Podman Phase 2） |
| Bridge | ⚠️ P0 | ✅ Core 透传已完成；⚠️ B-1/B-2 待做；B-10 adapter-core 部署 |
| Hub | ⚠️ 剩余 H-5 | ✅ `dscodebench@0.2.0` 全量预缓存已入库；⚠️ Pro 真实 image_tar 仍缺 |
| uenv-server | ✅ 否 | ✅ 仅 SWE 联调项（S-2/S-3） |
| proto / fixtures / VeRL 样例 | — | ✅ proto 不变；fixtures + smoke + VeRL JSON **已入库** |

---

## 2026-07-09 合并与实机联调摘要

### Merge 冲突处理

| 项 | 状态 |
|----|------|
| 冲突文件 | `uenv-worker/src/episode/payload.rs`（已解决） |
| 合并内容 | 保留 **code** 侧 `dscodebench` 归一化 + 字段透传；并入 **math** 侧 `pubmedqa` / `scitab` / `olymmath` 归一化 |
| 单元测试 | `cargo test -p uenv-worker --lib payload` 11 passed；`cargo test -p uenv-math-env` 14 passed |

### 7143 / Server 实机结果

| 步骤 | 结果 |
|------|------|
| Worker 7143 sync + `cargo build -p uenv-worker --release` | ✅ |
| Server `8.130.75.157` 部署 `uenv-adapter-core` → `/usr/local/bin/uenv-adapter-core` | ✅ |
| Bridge `response_text` 透传（免 LLM smoke） | ✅ `uenv-bridge/core/src/core.rs` |
| **Math 四 dataset E2E**（gsm8k / pubmedqa / scitab / olymmath-easy） | ✅ 全部 `reward=1.0` |
| fixtures + smoke + VeRL 参考样例入库 | ✅ 见下文「仓库已落地资产」 |

### 仓库已落地资产（非 Worker 代码）

| 路径 | 内容 |
|------|------|
| `fixtures/math/samples/*.json` | pubmedqa / scitab / olymmath-easy smoke payload |
| `fixtures/code/samples/ds_smoke_001.json` | DSCodeBench inline test |
| `uenv-bridge/scripts/smoke_math_datasets_grpcurl.py` | math 四 dataset grpcurl E2E |
| `uenv-bridge/scripts/smoke_code_env_grpcurl.py` | code env grpcurl E2E |
| `uenv-bridge/scripts/samples/verl_benchmark_samples.json` | 五类 VeRL 单条样本参考 |
| `uenv-bridge/src/uenv/__init__.py` | 修复 async e2e 的 `from uenv.bridge` import |

```bash
# 7143 / 实机（需 grpcurl + Worker + adapter-core）
python3 uenv-bridge/scripts/smoke_math_datasets_grpcurl.py 8.130.75.157:8088
python3 uenv-bridge/scripts/smoke_code_env_grpcurl.py 8.130.75.157:8088

# VeRL async（需 mock LLM）
PYTHONPATH=uenv-bridge/src python3 uenv-bridge/scripts/verify_math_datasets_and_async_e2e.py
```

> grpcurl smoke 覆盖 **adapter-core + uenv-server 调度 + Worker**；不依赖 VeRL 训练栈。

---

## UEnv 环境能力对照

```
                    ┌─────────────────────────────────────────┐
  已支持             │ swe          SWE-bench-Pro（容器+pytest）│
                    ├─────────────────────────────────────────┤
  已支持             │ math         gsm8k / pubmedqa / scitab │
                    │              olymmath(-easy/-hard)       │
                    ├─────────────────────────────────────────┤
  Phase 1 已支持     │ code         DSCodeBench（官方 harness） │
                    │              Hub 全量包已入库；Worker sync 待做 │
                    └─────────────────────────────────────────┘
```

---

## 逐项详情

### 1. PubMedQA（文本阅读理解）

| 维度 | 说明 |
|------|------|
| 官方输出 | `yes` / `no` / `maybe` |
| 输入 | PubMed abstract 上下文 + 生物医学问题 |
| **Worker 现状** | ✅ `plugins/math/src/backends/pubmedqa/scoring.rs`；`score.rs` 按 `dataset=pubmedqa` 路由 |
| 映射 | `env_type=math`，`dataset=pubmedqa` |
| 7143 E2E | ✅ smoke case `pubmedqa` → `reward=1.0` |

#### 已实现

| 项 | 路径 |
|----|------|
| Backend | `plugins/math/src/backends/pubmedqa/scoring.rs` |
| 路由 | `plugins/math/src/score.rs` |
| manifest | `plugins/math/manifest.yaml` → `datasets: pubmedqa` |
| payload 归一化 | `uenv-worker/src/episode/payload.rs` → `PubMedQA` → `pubmedqa` |
| Bridge | `verl_agent_loop._env_type()` 含 `pubmedqa` → `math` |

#### 剩余（非阻塞）

| 项 | 内容 |
|----|------|
| Hub | 内网预缓存 math env 制品（manifest + 插件/可选 dataset 包）；Worker `sync` 后离线可用 |
| Fixtures | ✅ smoke JSON 已有（`fixtures/math/samples/`）；待补 textproto/pb 与 golden 批量评测 |

---

### 2. SciTab（表格理解）

| 维度 | 说明 |
|------|------|
| 官方输出 | claim 被表格 **支持 / 反驳 / 信息不足**（三分类） |
| 输入 | 科学论文表格 + 自然语言 claim |
| **Worker 现状** | ✅ `plugins/math/src/backends/scitab/scoring.rs`；表格文本随 `question` 传入 observation |
| 映射 | `env_type=math`，`dataset=scitab`（不再需独立 `reading` env） |
| 7143 E2E | ✅ smoke case `scitab` → `reward=1.0` |

#### 已实现

| 项 | 路径 |
|----|------|
| Backend | `plugins/math/src/backends/scitab/scoring.rs` |
| 路由 | `plugins/math/src/score.rs` |
| manifest | `datasets: scitab` |
| payload 归一化 | `scitab-dev` 等别名 → `scitab` |

#### 剩余（非阻塞）

| 项 | 内容 |
|----|------|
| 表格专用字段 | 可选拆分 `table_html` / `claim`（当前合并在 `question`） |
| 长表格截断 | 防 context 过长策略待产品化 |

---

### 3. DSCodeBench / DS-Bench（代码生成）

| 维度 | 说明 |
|------|------|
| 官方输出 | Python 代码 → 官方 test harness 执行 |
| **Worker 现状** | ✅ **Phase 1 完整功能已落地**（2026-07-11） |
| **Hub 现状** | ✅ **`dscodebench@0.2.0` 全量预缓存已入库**（2026-07-12，Hub `8.130.95.176`） |
| 已实现 | `plugins/code/`、`uenv-code-plugin`、`dataset=dscodebench`、官方风格 harness（`dscodebench_harness.py`）+ inline smoke |
| 7143 验证 | m4/m5 插件与 Executor 测试通过；code 预热池 dispatch 正常；golden `ds_001` 固定 seed 通过 |
| smoke | ✅ `smoke_code_env_grpcurl.py`（inline）；✅ `fixtures/code/samples/ds_001.json` + `fixtures/code/benchmark/stdlib/` |
| 未完成 | Worker 节点 `env sync` 消费全量包；Podman 沙箱（Phase 2）；Bridge B-1/B-2 |

#### Worker 已有能力

```
env_type=code → plugins/code → extract 代码 → Python 评测
  ├─ inline test_code（smoke / 联调）                              ✅
  └─ test_script_path + ground_truth_* + UENV_DSCODEBENCH_ROOT     ✅ 官方风格 harness
```

#### Worker 侧剩余

| 项 | 内容 |
|----|------|
| W-1 | 7143：`uenv env sync dscodebench --version 0.2.0` → 解压 tar → `scripts/install_venv.sh` — 运维，非代码 |
| W-3 | Phase 2：Podman 沙箱 backend |
| W-4 | 持久化 `UENV_CODE_PLUGIN_BIN` / `UENV_DSCODEBENCH_ROOT` / `UENV_CODE_PYTHON` 至 `/root/.uenv-worker.env` |

#### 其他模块

| 模块 | 状态 |
|------|------|
| Bridge | ✅ code 字段透传；⚠️ P0：`dscodebench` 路由 + 显式 `dataset`；adapter-core 部署（B-10） |
| Hub | ✅ **`dscodebench@0.2.0` 全量**（H-2 完成）；`0.1.0` MVP 仍保留 |
| uenv-server | ✅ **代码无需改** |
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
| Hub swe 制品（H-5） | catalog/overlay 已有；**真实 Pro `image_tar` 仍缺**。导入机可用 `scripts/pull-pro-image-7143.sh`（mirror：DaoCloud → NJU → **dockerproxy.net 实机成功** → Hub 直连）拉 `jefzda/sweap-images` 后 `docker save` → `publish-image`；Worker 仅 `sync --docker-load` |
| Agent 编排 | SWE+Agent 全链路依赖 Server `AgentJob` + 208.77 poll 模式 |

---

### 5. OlymMATH-EASY / HARD（数学题求解）

| 维度 | 说明 |
|------|------|
| 官方输出 | 奥赛级自然语言计算题答案（常含 LaTeX、分数、区间） |
| **Worker 现状** | ✅ `plugins/math/src/backends/olymmath/scoring.rs` |
| 判分 | `\boxed{}` 提取 + 数字/分数/区间归一化（MVP，非 SymPy 符号等价） |
| split | `olymmath` / `olymmath-easy` / `olymmath-hard`；`EN-EASY` 等别名归一化 |
| 7143 E2E | ✅ smoke case `olymmath-easy` → `reward=1.0` |

#### 已实现

| 项 | 路径 |
|----|------|
| Backend | `plugins/math/src/backends/olymmath/scoring.rs` |
| 路由 | `plugins/math/src/score.rs` |
| payload 归一化 | `uenv-worker/src/episode/payload.rs` |

#### 剩余（非阻塞）

| 项 | 内容 |
|----|------|
| SymPy 等价 | 复杂 LaTeX 表达式可选 Python 子进程判分 |
| 官方 golden | 与 OlymMATH 官方 normalize 全量对齐验证 |

---

## Worker 配置现状（7143 实机）

当前 `config/uenv-worker.deploy-7143-swe-pro.yaml`：

```yaml
env:
  types: ["math", "code", "swe"]
```

| env_type | 插件/路径 | 支持的 dataset / variant |
|----------|-----------|--------------------------|
| `math` | `plugins/math` → `uenv-math-plugin` | `gsm8k`、`pubmedqa`、`scitab`、`olymmath` / `-easy` / `-hard`（✅） |
| `code` | `plugins/code` → `uenv-code-plugin` | `dscodebench`（✅ Phase 1 harness） |
| `swe` | Worker native（Docker） | `pro` / verified / lite（✅ pro 已联调） |

### Server 主机上的组件（勿混淆）

| 组件 | 仓库 | 五类 benchmark |
|------|------|----------------|
| **`uenv-server`** | `uenv-server/` | ✅ 代码无需改 |
| **`uenv-adapter-core`** | `uenv-bridge/core/` | ⚠️ Bridge 变更后须重部署（见 B-10） |

---

## 其他模块跨层调整（详细）

本节从 **Bridge → Server → Worker → Hub → 训练侧** 全链路，说明五类 benchmark 在 Worker 已就绪或 MVP 之后，**其他模块仍需补齐的工作**。Worker 判分逻辑本身不在此重复，重点放在 **payload 契约、制品发布、调度编排、Dataset 适配**。

> **读法说明**：下文「待办」分两类——**代码改动**（需改仓库、发版）与 **联调/部署**（能力已有，配好即可）。`uenv-server` 与 `uenv-adapter-core` 常同机部署，但属 **不同组件**，勿混为一谈。

### 各模块要改什么（简表）

| 模块 | 要不要改代码？ | 还要做什么（五类 benchmark） |
|------|---------------|------------------------------|
| **Bridge** | ⚠️ **要**（P0：显式 `dataset`、`dscodebench`→`code`） | 部署 `uenv-adapter-core`；VeRL Dataset / `extra_info` 对齐 |
| **Hub** | ⚠️ **H-5 仍要**（SWE Pro `image_tar`）；H-1/H-2/H-3 ✅ | 导入机 `publish-image` → Worker `env sync` |
| **uenv-server** | ✅ **不要**（无 benchmark 专用分支） | 联调：Worker 在线、SWE EnvPackage 版本匹配、可选 AgentJob E2E |
| **proto** | ✅ 不要 | — |
| **fixtures / smoke** | ✅ 样本 + smoke 已入库；SWE pro 仍缺最小 textproto |
| **VeRL / 训练侧** | ✅ 参考样例已入库；训练 parquet 需业务侧导出 |
| **运维 / Deploy** | — | 内网零 egress：Hub 预缓存 → Worker sync |

### 除 Bridge / Hub / uenv-server 外：能否现在完成？

| 模块 | 现在能否完成？ | 说明 |
|------|-------------|------|
| **proto** | ✅ 已完成 | 无需任何改动 |
| **fixtures / smoke** | ✅ 已入库 | JSON 样本 + math/code grpcurl smoke；SWE textproto 仍缺 |
| **VeRL 样例** | ✅ 已入库 | `uenv-bridge/scripts/samples/verl_benchmark_samples.json` |
| **verify 脚本 import** | ✅ 已修复 | `uenv-bridge/src/uenv/__init__.py` |
| **CI** | ⚠️ 视流水线 | 可加 `cargo test -p uenv-math-env` 等 job |
| **运维 / Hub sync** | ❌ 依赖内网 | 需导入机 + Hub + Worker 节点 |
| **VeRL 全链路训练** | ❌ 依赖 7142 LLM | 需 mock LLM / 真模型 + adapter-core |

### 跨层数据流与职责

> **部署模型**：整体在内网运行；Hub 负责 **提前缓存** 镜像、benchmark、插件与依赖包。Worker 经 Hub **一次性 sync**，Episode 热路径 **不访问公网**。详见 [Hub 环境标准化指南](../hub/uenv-hub环境标准化指南.md)。

```
[外网导入机，一次性]  docker save / git clone / pip wheel 打包
        │
        ▼ publish / publish-image
┌───────────────────┐     manifest + 制品字节（tar/catalog/benchmark/插件）
│ uenv-hub（内网）   │     EnvPackage + env registry（math/code manifest）
└─────────┬─────────┘
          │ uenv env sync（部署期）
          ▼
┌───────────────────┐
│ uenv-worker       │     本地 plugins/、/var/lib/uenv/envs/…
└─────────┬─────────┘
          ▲ gRPC EpisodeRequest（运行时只读本地）
┌─────────┴─────────┐
│ uenv-server       │     调度（源码无需为五类 benchmark 改动）
└─────────┬─────────┘
          ▲
┌─────────┴─────────┐
│ uenv-bridge       │     adapter-core + VeRL Agent Loop
└───────────────────┘
```

| 层级 | 职责 | 不应做的事 |
|------|------|------------|
| Bridge | 任务名 → `env_type`；payload 映射与字段透传 | 不判分、不拉 Hub 制品 |
| uenv-server | 调度、SWE AgentJob、轨迹索引 | 不解析答案、不下载镜像/benchmark |
| Hub | 内网预缓存：manifest + 镜像 tar + benchmark 包 | 不参与 Episode 热路径 |
| Worker | Reset/Step/Score；消费已 sync 本地制品 | 默认不访问公网 |

> **组件勿混**：`8.130.75.157:8088` 上是 **`uenv-adapter-core`（Bridge）**，与 **`uenv-server`（Rust 调度）** 是不同二进制。

---

### 1. uenv-bridge

Bridge 分两层：**Rust Adapter Core**（`uenv-bridge/core/`）负责 L1↔Worker payload 映射；**Python VeRL Agent Loop**（`verl_agent_loop.py`）负责训练样本 → `EpisodeRequest`。

#### 1.1 已完成（2026-07-09）

| 能力 | 文件 | 说明 |
|------|------|------|
| math 任务路由 | `verl_agent_loop.py` `_env_type()` | `gsm8k` / `pubmedqa` / `scitab` / `olymmath` → `math` |
| code 任务路由 | `_env_type()` | `humaneval` / `mbpp` / `code` → `code`（**尚未**显式匹配 `dscodebench` 字符串） |
| 通用 payload 映射 | `core.rs` `sample_to_worker_payload()` | `question` ← extra_info / env_config；`dataset` ← env_config / data_source |
| `response_text` 透传 | `core.rs` | 所有 env_type 均可从 `env_config.response_text` 注入，用于 smoke / 免 LLM 联调 |
| SWE 字段透传 | `core.rs` | `instance_id`、`benchmark_variant`、Agent 编排字段等 |
| code 字段透传 | `core.rs` | `task_id`、`test_code`、`test_script_path`、`library` 等 DSCodeBench 字段 |
| rubric → rule_reward | `sample_to_worker_reward_config()` | VeRL `reward_model.ground_truth` → Worker `target` |
| 单测 | `tests/test_verl_agent_loop.py` | pubmedqa / SciTab / OlymMATH 路由到 math |

#### 1.2 待办（共性）

| ID | 项 | 详细说明 | 优先级 |
|----|-----|----------|--------|
| B-1 | **`build_episode_request` 写入 `dataset`** | 当前 `env_config` 仅含 `task_name` / `data_source` / `raw_prompt`（见 `verl_agent_loop.py` L627–631）。Worker 侧 `normalize_dataset()` 虽可从 `data_source` 推断，但 VeRL 全链路应 **显式** 设置 `env_config.dataset`（如 `pubmedqa`、`scitab`、`olymmath-easy`、`dscodebench`），避免别名歧义。 | P0 |
| B-2 | **`dscodebench` → `code` 显式路由** | `_env_type()` 需增加 `dscodebench` / `ds-bench` / `dsbench` token，与 Worker `payload.rs` 归一化对齐。 | P0 |
| B-3 | **`swe` / `swe-bench` 路由** | 增加 `swe` / `swebench` / `swe-bench-pro` → `swe`，避免落回 `default_env_type=math`。 | P1 |
| B-4 | **math 专用字段透传（Adapter Core）** | 当前 math 无 SWE/code 式专用分支。若 Dataset 拆分字段（PubMedQA 的 `context`/`abstract`、SciTab 的 `table_html`/`claim`），需在 `sample_to_worker_payload` 增加 `env_type==math` 分支，或约定 Bridge 将其拼入 `question` 并在文档中固定格式。 | P2 |
| B-5 | **`reward_config` 类型对齐** | `build_episode_request` 对 math 使用 `reward_type: rubric` + `rubric_config`；Adapter Core 已转换为 `rule_reward`。需在 VeRL Dataset 中保证 `ground_truth` / `style` 与 backend 一致（如 SciTab 的 `supports` vs `refutes` vs `not enough info`）。 | P1 |
| B-6 | **VeRL Dataset → `extra_info` 契约** | `_metadata_extra_info()` 目前只默认填充 `question`。各 benchmark 的 `target`、`split`、`task_id` 等应通过 `sample_kwargs.extra_info` 进入 metadata，再由 Core 映射到 Worker payload / reward_config。 | P1 |
| B-7 | **配置文件 `task_to_env_type`** | 规划中的 `configs/uenv-agent-loop.yaml` 增加显式映射表（当前仅靠 `_env_type()` 启发式）。便于新增 benchmark 时不改 Python 代码。 | P2 |
| B-8 | **async / pass@k 结果字段** | `required_result_fields` 已含 `response_ids` / `trajectory` 等。多采样 pass@k 由训练侧聚合；Bridge 需文档化「单 episode reward 0/1」语义，避免训练代码误用 step reward 做 pass@k。 | P2 |
| B-9 | **`verify_math_datasets_and_async_e2e.py` 可运行性** | ✅ 已补 `uenv-bridge/src/uenv/__init__.py`；运行需 `PYTHONPATH=uenv-bridge/src` + mock LLM。与 grpcurl smoke 互补。 | ✅ / P1 实机 |
| B-10 | **`uenv-adapter-core` 部署**（联调，非 uenv-server） | 跑在 Server 主机（如 `8.130.75.157:8088`），二进制 `/usr/local/bin/uenv-adapter-core`；Bridge 字段变更后须重编译部署（math smoke 曾因此失败）。脚本：`scripts/deploy-adapter-core-75157.sh`。 | P0 |

#### 1.3 分 Benchmark — Bridge 待办

| Benchmark | Bridge 待办 | 建议 `env_config` 字段 |
|-----------|-------------|----------------------|
| **PubMedQA** | B-1 写 `dataset=pubmedqa`；B-6 透传 `context`/`abstract`（或合并进 `question`） | `question`, `dataset`, `response_text`(smoke) |
| **SciTab** | 同上；B-4 若拆分表格字段 | `question` 或 `table_html`+`claim`, `dataset=scitab` |
| **DSCodeBench** | B-2 路由；B-1 `dataset=dscodebench`；code 分支已透传执行字段 | `task_id`, `library`, `test_code`/`test_script_path`, `num_tests`, `random_seed` |
| **SWE-bench-Pro** | B-3 路由；SWE 分支已透传 | `instance_id`, `benchmark_variant=pro`, `env_package_id`, `env_package_version` |
| **OlymMATH** | B-1 写 `dataset=olymmath-easy|hard`；split 别名与 Worker 归一化一致 | `question`, `dataset`, `target`(via reward_config) |

#### 1.4 验收方式

```bash
python3 uenv-bridge/scripts/smoke_math_datasets_grpcurl.py 8.130.75.157:8088
python3 uenv-bridge/scripts/smoke_code_env_grpcurl.py 8.130.75.157:8088
PYTHONPATH=uenv-bridge/src python3 uenv-bridge/scripts/verify_math_datasets_and_async_e2e.py
```

---

### 2. uenv-hub

Hub 在内网承担 **环境预缓存与制品分发**（不仅是 manifest/schema）。导入机一次性 publish 后，Worker 部署期 `uenv env sync`，Episode 运行时零 egress。

权威设计：[uenv-hub环境标准化指南](../hub/uenv-hub环境标准化指南.md)

#### 2.1 Hub 两类分发机制

| 机制 | API / CLI | 典型内容 | 适用 benchmark |
|------|-----------|----------|----------------|
| **Env registry** | `GET /api/v1/envs/{env_type}/versions/...` | math/code **manifest**、interface schema、可选 **插件制品 tar** | PubMedQA / SciTab / OlymMATH（math）；DSCodeBench（code） |
| **EnvPackage** | `GET /api/v1/packages/{id}/versions/...`、`uenv env sync` | **镜像 tar**、catalog.json、eval_spec、**benchmark 数据包**、worker overlay | SWE-bench-Pro；DSCodeBench 全量；未来 math dataset 离线包 |

#### 2.2 现状与差距（Hub `8.130.95.176`，2026-07-12：DSCode 全量已入库）

| env_type | 本地 `plugins/*/manifest.yaml` | Hub 当前能力 | 内网生产缺口 |
|----------|-------------------------------|-------------|-------------|
| `math` | ✅ gsm8k / pubmedqa / scitab / olymmath* | ✅ **`math@0.2.0` latest**（legacy `1.0.0` 已 yank）；✅ EnvPackage `math-smoke-fixtures@0.1.0`（pubmedqa/scitab/olymmath-easy 样本） | 全量评测集 tar / 插件二进制仍可选增强 |
| `code` | ✅ `plugins/code/manifest.yaml` | ✅ **`code@0.2.0` latest**；✅ EnvPackage **`dscodebench@0.2.0` 全量**（benchmark 1000 题 + eval 脚本 + 官方 wheels≈3.9GB，106 个 whl）；`0.1.0` MVP 仍保留 | Worker `uenv env sync dscodebench --version 0.2.0` 后解压 tar 并 `scripts/install_venv.sh` |
| `swe` | N/A（Worker native） | ✅ `swe-bench-pro@0.2.0` / verified + agent bridge | Pro **真实镜像 tar** 需导入机预置并 `publish-image` |

> 联调仓库若自带 `plugins/` 或已手工 sync 的 EnvPackage，Worker 可跳过 Hub HTTP；这属于 **开发便利**，不是内网生产路径。

**实机验收（Bearer token + `http://8.130.95.176:8088`）**：

| 资源 | 状态 |
|------|------|
| `GET /envs/math/versions/latest` | `0.2.0`，datasets 含 gsm8k/pubmedqa/scitab/olymmath* |
| `GET /envs/code/versions/latest` | `0.2.0`，datasets=`[dscodebench]` |
| `GET /packages/math-smoke-fixtures/versions/latest` | `0.1.0`，含三份 math smoke JSON |
| `GET /packages/dscodebench/versions/latest` | **`0.2.0` 全量**（benchmark + eval + wheels≈3.9GB） |
| `GET /packages/swe-bench-pro/versions/latest` | `0.2.0`（catalog/overlay；image_tar 仍可能缺） |

#### 2.3 待办（共性）

| ID | 项 | 详细说明 | 优先级 |
|----|-----|----------|--------|
| H-1 | **发布 math env v0.2.x（registry + 制品）** | ✅ **已完成（2026-07-11）**：`math@0.2.0` + `math-smoke-fixtures@0.1.0`；legacy `1.0.0` yank。可选增强：插件二进制 / 全量 dataset tar。 | ✅ / P2 增强 |
| H-2 | **发布 code / DSCodeBench EnvPackage** | ✅ **全量已完成（2026-07-12，Hub `8.130.95.176`）**：`dscodebench@0.2.0` 含 `benchmark.tar.gz`（1.2MB）、`eval-scripts.tar.gz`、`wheels.tar.gz`（≈3.9GB / 106 wheels，对齐官方 `requirements.txt`）。Worker：`sync` → 解压 → `scripts/install_venv.sh`。 | ✅ |
| H-3 | **更新 `seed.rs` 与导入脚本** | ✅ seed 已含 math/code@0.2.0、yank legacy、`config/benchmark/` fixture 包；新实例启动即对齐。 | ✅ |
| H-4 | **五类 benchmark 运维手册** | 在 `Docs/hub/` 补充：各 benchmark 在 Hub 上应缓存哪些制品、`uenv env sync` / `publish-image` 示例。 | P1 |
| H-5 | **SWE-bench-Pro 镜像与 catalog 全量入库** | ⚠️ **过渡中（2026-07-13）**：catalog `0.3.4`（731）+ Hub **3** 个 `image_tar`；Hub 全量入库已停（盘≈100G）。**7143** Docker data-root 已迁 `/data/docker`，后台 `pull-swe-pro-images-worker.sh` 本机全量直拉（~1.5TB）。Hub 扩容后再 `docker save`→`publish-image`。 | P0 / Worker 直拉中 |
| H-6 | **math dataset 离线评测包（可选）** | smoke 已入库；全量 held-out 评测集 tar 仍可选。 | P2 |
| H-7 | **fixtures 与 Hub examples 对齐** | manifest `examples[]` 与 `fixtures/math`、`fixtures/code` 一致；sync 后路径与文档中的 `test_script_path` 相对路径一致。 | P2 |

#### 2.4 分 Benchmark — Hub 预缓存清单

| Benchmark | Hub 应缓存的制品 | 2026-07-12 Hub 状态 | Worker sync 后本地路径（示例） |
|-----------|-----------------|---------------------|------------------------------|
| **PubMedQA** | math manifest + 可选样本 | ✅ `math@0.2.0` + smoke fixture | `plugins/math/` 或 sync 包 `samples/` |
| **SciTab** | 同上 | ✅ 同上 | 同上 |
| **DSCodeBench** | code EnvPackage：benchmark + 依赖 + eval | ✅ `dscodebench@0.2.0` 全量预缓存 | `/var/lib/uenv/envs/dscodebench/0.2.0/` → 解压后 `benchmark/` → `UENV_DSCODEBENCH_ROOT` |
| **SWE-bench-Pro** | EnvPackage：catalog + **image_tar** + eval_spec | ⚠️ catalog 有；**image_tar 仍缺** | `/var/lib/uenv/envs/swe-bench-pro/0.2.0/` |
| **OlymMATH** | math manifest + 可选题目包 | ✅ `math@0.2.0` + easy smoke | 同 math |

#### 2.5 内网部署工作流（运维）

```bash
# ① 导入机（可访问外网，一次性）
# DSCode：已在 Hub 完成；以下为 SWE Pro 镜像导入
bash scripts/pull-pro-image-7143.sh <dockerhub_tag>   # DaoCloud → NJU → dockerproxy → Hub 直连
docker save jefzda/sweap-images:<tag> -o swe-instance.tar

# ② Hub 主机（内网）
uenv env publish --manifest plugins/code/manifest.yaml    # registry（已完成）
uenv env publish-image swe-bench-pro 0.2.0 --tar swe-instance.tar   # H-5 待做
# DSCodeBench@0.2.0 已入库，无需重复 POST

# ③ Worker 节点（内网，部署/扩缩容）
uenv env sync swe-bench-pro --version 0.2.0 --docker-load
uenv env sync dscodebench --version 0.2.0
# sync 得到 *.tar.gz 后在包目录解压：
#   tar xzf benchmark.tar.gz && tar xzf eval-scripts.tar.gz && tar xzf wheels.tar.gz
#   bash scripts/install_venv.sh   # 离线安装官方依赖到本地 venv
export UENV_DSCODEBENCH_ROOT=/var/lib/uenv/envs/dscodebench/0.2.0/benchmark
export UENV_CODE_EVAL_SCRIPT=/var/lib/uenv/envs/dscodebench/0.2.0/scripts/evaluate_code.py
export UENV_CODE_PYTHON=/var/lib/uenv/envs/dscodebench/0.2.0/venv/bin/python
```

---

### 3. uenv-server（Rust 调度服务）

> **`uenv-server` 五类 benchmark：代码层面无需改动。** 下文 §3.2 仅为 **联调 / 配置 / 可选增强**，不是「还要改 Server 源码」。

#### 3.1 代码层面：无需新增开发

| 结论 | 说明 |
|------|------|
| 调度 | 已有 `env_type` + `supported_env_types` 匹配，math / code / swe 通用 |
| payload | 丰富字段走 JSON，**不改 proto、不加 benchmark 分支** |
| 新增 env | 仅当未来出现全新 `env_type` 时才可能动 Server；当前五类 **不涉及** |

#### 3.2 已有能力（直接复用）

| 能力 | 说明 |
|------|------|
| env_type 调度 | `scheduler/mod.rs` |
| Episode 提交 | adapter-core → `EpisodeService::submit_episode_batch` |
| SWE native | payload 转发；容器与 pytest 在 Worker |
| 轨迹 HTTP :8077 | `trajectory.rs` |
| SWE+Agent | `agent_job.rs` 已有 Poll 模式（208.77 OpenHands） |
| EnvPackage 匹配 | `scheduler` 可按 `env_package_id` / version 过滤 Worker |

#### 3.3 联调与配置（非代码改动）

| ID | 项 | 类型 | 说明 | 优先级 |
|----|-----|------|------|--------|
| S-1 | Worker 注册 `env.types` | 配置 | 7143 已 `[math, code, swe]` | — |
| S-2 | SWE EnvPackage 版本一致 | 联调 | 请求与 Worker 本地 sync 的包 id/version 对齐，否则调度报「包不匹配」 | P1 |
| S-3 | SWE+Agent 全链路 E2E | 联调 | `execution_mode=agent` 时验证 AgentJob、`trajectory_id` 回写 | P1 |
| S-4 | 轨迹按 benchmark 检索（可选） | 增强 | 轨迹索引写入 `dataset` 等字段 | P2 |
| S-5 | 日志可观测性（可选） | 增强 | 失败日志带 `env_type` + payload 内 `dataset` | P2 |

> **adapter-core 部署**见 Bridge **B-10**（同属控制面主机，组件归属 Bridge，不是 uenv-server）。

#### 3.4 分 Benchmark — uenv-server 侧

| Benchmark | uenv-server 代码 | 联调注意 |
|-----------|-------------------|----------|
| **PubMedQA / SciTab / OlymMATH** | 无 | math Worker 在线即可 |
| **DSCodeBench** | 无 | code Worker 在线；必要时调大 `timeout_seconds` |
| **SWE-bench-Pro** | 无 | EnvPackage 匹配（S-2）；走 Agent 时做 S-3 |

---

### 4. proto / plugin_proto

| 项 | 结论 |
|----|------|
| L1 gRPC | `EpisodeRequest` / `EpisodeResult` **无需改**；丰富字段走 JSON `payload` |
| L2 Plugin UDS | `Reset` / `Step` / `Close` **无需改**；step `info` 用 JSON 扩展 |
| Adapter Core proto | `SampleEnvelope.envType` + base64 `payloadJson` 已够用 |

**待办**：仅在 product 需要强类型校验时，才考虑在 Hub `config_schema` 或 OpenAPI 层补充 JSON Schema，**不必改 .proto**。

---

### 5. fixtures 与 smoke 脚本

| Benchmark | 已有 | 待补 |
|-----------|------|------|
| **GSM8K** | `fixtures/math/episode_001.*` | — |
| **PubMedQA / SciTab / OlymMATH** | `fixtures/math/samples/*.json` | 分 dataset 的 textproto / `.pb` |
| **DSCodeBench** | `ds_smoke_001.json`、`ds_001.json`、`benchmark/stdlib/`、`episode_001.textproto` | 更多官方题 golden；7143 全量包 smoke |
| **SWE-bench-Pro** | `fixtures/swe/swe_pro_instances.json` | 最小 pro textproto + smoke 脚本 |
| **smoke 脚本** | `smoke_math_datasets_grpcurl.py`、`smoke_code_env_grpcurl.py` | swe pro grpcurl smoke |

| ID | 项 | 状态 |
|----|-----|------|
| F-1 | 每 benchmark ≥1 JSON smoke | ✅ math/code 已入库 |
| F-2 | textproto / pb + Hub examples 对齐 | ⚠️ 待补 |
| F-3 | 7143 定期 smoke checklist | ⚠️ 运维流程 |

---

### 6. 部署与 Worker 运行时配置

与 benchmark 相关的 **非代码** 配置，常在联调中阻塞全链路。

| 变量 / 配置 | 适用 | 说明 |
|-------------|------|------|
| `UENV_MATH_PLUGIN_BIN` | math 全系列 | 指向 `uenv-math-plugin`；7143 需写入 `/root/.uenv-worker.env` |
| `UENV_CODE_PLUGIN_BIN` | DSCodeBench | 指向 `uenv-code-plugin` |
| `UENV_CODE_EVAL_SCRIPT` | DSCodeBench | 默认 `plugins/code/scripts/evaluate_code.py` |
| `UENV_DSCODEBENCH_ROOT` | DSCodeBench 全量 | **Hub sync 后的** benchmark 根目录（如 `/var/lib/uenv/envs/dscodebench/0.2.0/benchmark`）；本地联调可用 `fixtures/code/benchmark/` |
| `UENV_HUB_ENDPOINT` | Worker / CLI | 内网 Hub 地址；启用后部署期 pull/sync 制品 |
| `swe.env_package_dir` | SWE-pro | **Hub sync 后的** EnvPackage 本地目录（如 `/var/lib/uenv/envs/swe-bench-pro/0.2.0`） |
| `env.types: [math, code, swe]` | 7143 Worker | `config/uenv-worker.deploy-7143-swe-pro.yaml` |
| `UENV_ADAPTER_CORE_ENDPOINT` | Bridge / 训练 | 默认 `8.130.75.157:8088` |

---

### 7. VeRL Dataset / 训练侧

训练框架不经过 UEnv 仓库，但 **Dataset 字段设计** 决定 Bridge 能否正确组包。

#### 7.1 推荐样本形状（math 类 benchmark）

```python
{
    "data_source": "pubmedqa",           # 或 task_name / ability，供 _env_type()
    "prompt": [...],                     # chat messages
    "reward_model": {
        "style": "rule",
        "ground_truth": "yes",           # → Worker target
    },
    "extra_info": {
        "dataset": "pubmedqa",           # 建议显式（对应 Bridge B-1）
        "question": "Context: ...\nQuestion: ...",
        "max_steps": 1,
    },
}
```

#### 7.2 分 env_type 要点

| env_type | Dataset 必带字段 | 常见坑 |
|----------|------------------|--------|
| `math` | `ground_truth` + `dataset` + 题目文本 | 仅 `data_source` 不设 `dataset` 时依赖 Worker 别名推断 |
| `code` | `task_id`、`test_code` 或 `test_script_path`、`library` | 缺 `test_*` 则插件无法评测 |
| `swe` | `instance_id`、`benchmark_variant=pro` | 缺 EnvPackage 版本导致调度失败 |

#### 7.3 参考与待办

| ID | 项 | 状态 |
|----|-----|------|
| T-1 | 五类 benchmark VeRL 单条样本 | ✅ `uenv-bridge/scripts/samples/verl_benchmark_samples.json` |
| T-2 | GRPO / async rollout 实机 | ⚠️ math 有 `verify_math_datasets_and_async_e2e.py`；code/swe 待对等脚本 |
| T-3 | pass@k 聚合语义文档化 | ⚠️ 训练侧对单 episode 0/1 reward 做 pass@k |

---

### 8. CI 与文档

| ID | 项 | 说明 |
|----|-----|------|
| D-1 | CI 增加 `uenv-math-env` + `payload` 归一化测试 | merge 已验证；应进 PR CI |
| D-2 | CI 增加 `uenv-code-env` 编译测试 | DSCodeBench MVP |
| D-3 | 更新 `Docs/更新日志.md` | 记录 math 多 dataset + code env |
| D-4 | Bridge payload 字段表 | 在 `Docs/hub/` 或 Bridge README 维护 **按 env_type 的 env_config 字段表** |
| D-5 | 实机联调记录 | math 四 dataset 结果可追加独立 `实机联调记录-math-datasets.md`（可选） |

---

### 9. 跨层待办总表（按优先级）

| 优先级 | 模块 | ID | 内容 | 影响 benchmark |
|--------|------|-----|------|----------------|
| **P0** | Bridge | B-1, B-2, **B-10** | dataset 显式化；dscodebench 路由；**adapter-core 部署** | 全部 E2E |
| **P0** | Hub | H-5 | SWE Pro 镜像 tar（H-1/H-2 已完成） | SWE |
| **P0** | Deploy | — | Worker `uenv env sync` 消费 Hub 已缓存制品 | 全部 |
| **P1** | Bridge | B-3, B-5, B-6, B-9 | swe 路由、reward、extra_info、async e2e | SWE + 训练 |
| **P1** | Hub | H-4, H-6 | 运维手册、math 全量 dataset 包（H-3 seed ✅） | SWE + code + math |
| **P1** | uenv-server | S-2, S-3 | EnvPackage 匹配、Agent E2E（**联调，非改代码**） | SWE-pro |
| **P1** | Fixtures | F-2 | textproto/pb、Hub examples 对齐 | math + code |
| **P2** | Bridge | B-4, B-7, B-8 | math 字段拆分、配置映射、pass@k 文档 | SciTab + 训练 |
| **P2** | Hub | H-4, H-7 | 文档、manifest examples | 运维 |
| **P2** | uenv-server | S-4, S-5 | 轨迹 dataset、日志（可选增强） | 可观测性 |

---

## 其他模块共性调整（速查）

| 模块 | 状态 / 待办 |
|------|-------------|
| **uenv-bridge** | ⚠️ 代码：B-1/B-2；部署：**adapter-core（B-10）**；见 §1 |
| **uenv-hub** | ✅ math/code registry + DSCode 全量包；⚠️ **H-5** SWE Pro `image_tar`；见 §2 |
| **uenv-server** | ✅ **代码无需改**；⚠️ 仅 SWE 联调（EnvPackage 匹配、Agent E2E）；见 §3 |
| **proto** | ✅ 不变 |
| **fixtures / smoke** | ✅ JSON + math/code smoke 已入库；⚠️ SWE textproto、F-2/F-3 |
| **VeRL Dataset** | ✅ 参考 JSON 已入库；⚠️ 业务 parquet + async 实机（见 §7） |
| **Docs/260709** | 本文档 + [实机联调记录-code-env](./实机联调记录-code-env.md) |

详细说明见上文 **「其他模块跨层调整（详细）」** 各节。

---

## 跨层调整优先级建议（按 Benchmark）

| 优先级 | Benchmark | 跨层重点（Worker 已就绪后的剩余工作） |
|--------|-----------|--------------------------------------|
| **P0** | DSCodeBench | Hub 全量包 ✅；下一步 Worker `sync 0.2.0` + Bridge `dscodebench`→`code`（B-1/B-2） |
| **P0** | SWE-bench-Pro | Hub Pro **真实**镜像 tar（H-5，mirror 导入）+ catalog；Worker `sync --docker-load`；EnvPackage 调度匹配 |
| **P1** | PubMedQA / SciTab / OlymMATH | Hub math registry ✅；Bridge 显式 `dataset`；VeRL Dataset + fixtures |
| **P2** | SciTab | Bridge 拆分 `table_html`/`claim` 字段（当前 question 合并不阻塞） |

---

## 与规划草案的关系

| 文档 | 内容 |
|------|------|
| [DSCodeBench-CodeEnv-扩展规划草案](./DSCodeBench-CodeEnv-扩展规划草案.md) | DSCodeBench Worker 详细设计（已实现 MVP） |
| [实机联调记录-code-env](./实机联调记录-code-env.md) | DSCodeBench 7143 验证结果 |
| **本文档** | 五类 benchmark 横向支持矩阵与跨层待办 |

---

## 验收标准（按 benchmark）

| Benchmark | Worker 层验收 | 7143 状态 |
|-----------|---------------|-----------|
| PubMedQA | `env_type=math` + `dataset=pubmedqa` → yes/no/maybe 判分 | ✅ smoke |
| SciTab | `env_type=math` + `dataset=scitab` → 三分类 claim 判分 | ✅ smoke |
| DSCodeBench | `env_type=code` → 官方 harness pass@1 与 golden 对齐 | ✅ Phase 1（inline + `ds_001` golden）；⚠️ 全量包待 7143 `sync` |
| SWE-bench-Pro | `env_type=swe` + `variant=pro` → patch 应用 + pytest reward=1 | ✅ 历史联调 |
| OlymMATH | `env_type=math` + `dataset=olymmath-*` → 等价答案判分 | ✅ smoke（easy） |
