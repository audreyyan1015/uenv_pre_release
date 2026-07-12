# DSCodeBench / DS-Bench 接入 UEnv 规划草案

> 日期：2026-07-09  
> 范围：UEnv 全栈扩展支持 DSCodeBench（数据科学代码生成 benchmark）  
> 权威参考：[DSCodeBench](https://github.com/ShuyinOuyang/DSCodeBench)、[worker-pool-layer-design.md §9.5](../worker-pool-layer-design.md)、[uenv-design-prd-v7.2 §3.2.5](../uenv-design-prd-v7.2.md)

---

## 一句话结论

DSCodeBench 属于 **执行型 CodeEnv**（`env_type=code`），判分依赖 **官方测试 harness 在沙箱内执行模型生成代码**，与 GSM8K（`env_type=math`、规则字符串匹配）路径不同。  
**主要工作量在 Worker 侧**：新建 `plugins/code/` 插件、扩展 payload 透传、注册独立预热池与运行时依赖；Server / Proto 基本可复用。

> **Worker Phase 1（2026-07-11）**：脚手架 / payload / 官方风格 harness（对齐上游 `run_test.py` 单题语义）/ HealthCheck / golden fixture **已完成**。剩余主要为 Hub 全量制品 sync 与 Phase 2 Podman；Bridge B-1/B-2 仍待做。

---

## 背景与定位

| 项 | 说明 |
|----|------|
| Benchmark | DSCodeBench（DS-Bench），1,000 道真实 GitHub 数据科学编程题 |
| 覆盖库 | NumPy、Pandas、SciPy、Scikit-learn、TensorFlow、PyTorch、Matplotlib、Seaborn、Keras、LightGBM |
| 评测方式 | 模型输出 Python 代码 → 官方 test case script 动态生成测试 → 全部通过则 reward=1 |
| UEnv 映射 | `env_type=code`，`payload.dataset=dscodebench`（**不占用** L1 调度键） |
| 与 GSM8K 关系 | GSM8K 是 math 环境下的 dataset backend；DSCodeBench 是 code 环境下的 dataset backend |

---

## 架构总览

```
VeRL Dataset
    ↓
uenv-bridge          env_type=code, payload 含 task_id / test_script 等
    ↓
uenv-server          按 env_type=code 调度（逻辑不变）
    ↓
uenv-worker          独立 code WarmupPool → plugins/code 子进程
    ↓
plugins/code         reset → step（提取代码 → 沙箱执行官方 harness → reward）
    ↓
Python 沙箱          DSCodeBench benchmark_construction_evaluation
```

---

## 一、Worker 侧调整（主要）

### 1.1 新建 `plugins/code/` 插件（核心交付物）

当前仓库仅有 `plugins/math/`（规则判分）与 `plugins/swe/`（OpenEnv 风格，不走 L2 UDS IPC）。**CodeEnv 需新建完整 L2 插件**，参考 `plugins/math/` + `uenv-math-plugin` 模式。

#### 目录结构（草案）

```
plugins/code/
├── manifest.yaml              # env_type: code, datasets: [dscodebench]
├── Cargo.toml
├── run.sh                     # 启动 uenv-code-plugin
└── src/
    ├── lib.rs
    └── backends/
        ├── mod.rs
        └── dscodebench/
            ├── mod.rs
            ├── extract.rs     # 从模型输出提取 Python 代码块
            ├── executor.rs    # 调用官方 harness / subprocess 执行测试
            └── scoring.rs     # 解析 harness 输出 → reward (0/1)
```

#### `manifest.yaml` 要点

```yaml
env_type: code
version: "0.1.0"
supported_backends:
  - process          # Phase 1 MVP；生产建议 podman
ipc: proto-uds
entry: ./run.sh
description: CodeEnv — code execution + unit-test reward environment
tags:
  - code
  - execution
datasets:
  - dscodebench
```

#### `PluginService` 行为（`uenv-code-plugin` 二进制）

| RPC | DSCodeBench 行为 |
|-----|------------------|
| `Reset` | 从 `{uds_path}.episode.json` 读取题目；observation 返回 problem description（prompt） |
| `Step` | 接收 model action → `extract` 代码 → `executor` 跑官方测试 → 返回 reward + info → `terminated=true` |
| `Close` | 清理临时文件 / 沙箱会话 |
| `HealthCheck` | 探活；可选校验 Python 依赖与 benchmark 数据路径 |

**单轮 episode**：与 GSM8K 相同，通常 `max_steps=1`（问题 → 代码 → 测试 → 结束）。

#### DSCodeBench backend  url executor 设计要点

- **复用官方逻辑**：封装 DSCodeBench 仓库 `benchmark_construction_evaluation/` 中的评测脚本，避免自写 test runner。
- **测试规模**：每题平均 ~200 个动态测试，需支持 `num_tests`、`random_seed` 等可配置项（与官方一致）。
- **代码提取**：支持 markdown code fence、` ```python ` 块、纯文本等多种模型输出格式。
- **超时与资源限制**：单题执行 timeout（建议可配置，如 60s–300s）；限制 stdout/stderr 大小，防止日志撑爆。
- **step info 字段**（写入 `StepResponse.info` JSON）：

  ```json
  {
    "dataset": "dscodebench",
    "task_id": "ds_001",
    "library": "pandas",
    "passed": true,
    "tests_run": 200,
    "tests_passed": 200,
    "execution_time_ms": 1234,
    "error": null
  }
  ```

#### 沙箱 / Backend 选型

| 阶段 | 方案 | 说明 |
|------|------|------|
| Phase 1 MVP | `process` + 子进程隔离 | 开发联调；**内网生产**由 Hub 预缓存 Python 依赖包，Worker sync 后离线执行 |
| Phase 2 生产 | `podman` 容器 | 对齐 PRD CodeEnv 规划；参考 `uenv-worker/src/swe/` 容器 exec 模式 |
| 不推荐 | 复用 `env_type=swe` | 单函数题过重；破坏 env_type 语义 |

---

### 1.2 Cargo / Workspace 集成

| 文件 | 改动 |
|------|------|
| 根 `Cargo.toml` | `members` 增加 `"plugins/code"` |
| `uenv-worker/Cargo.toml` | 新增 `[[bin]] name = "uenv-code-plugin"`，依赖 `uenv-code-env`（或内联 crate） |
| `plugins/code/Cargo.toml` | 新建 crate，导出 backend 模块供 binary 使用 |

---

### 1.3 Worker 配置与注册

| 文件 | 改动 |
|------|------|
| `config/uenv-worker.yaml` | `env.types: ["math", "code"]` |
| `UENV_ENV_TYPES` 环境变量 | 同上，逗号分隔 |
| `src/main.rs` | `supported_env_types` 从配置读取，注册到 ControlPlane |
| `src/runtime.rs` | 为 `code` 创建独立 `WarmupPool` 实例（与 math 分池，互不干扰） |

Worker 启动后 ControlPlane 上报示例：

```text
supported_env_types: ["math", "code"]
```

---

### 1.4 Payload 透传：`build_reset_config` 扩展

**现状**（`uenv-worker/src/episode/payload.rs`）仅转发 `question`、`dataset`、`target`、`seed`，不足以支撑 DSCodeBench。

**需扩展字段**（从 L1 `EpisodeRequest.payload` JSON 写入 episode config）：

| 字段 | 类型 | 说明 |
|------|------|------|
| `task_id` | string | DSCodeBench 题目 ID |
| `library` | string | 所属库（pandas / numpy / …） |
| `test_script_path` | string | 官方 test case script 路径（或 Hub 制品内相对路径） |
| `ground_truth_path` | string | 可选，对齐验证用 |
| `num_tests` | int | 测试用例数量（默认与官方一致） |
| `random_seed` | int | 随机种子，控制测试输入生成 |
| `timeout_secs` | int | 单题执行超时（可选，默认 120） |
| `benchmark_root` | string | benchmark 数据根目录（可选，默认环境变量） |

**实现建议**：

```rust
// payload.rs — 在现有 question/dataset/target 之后追加
for key in [
    "task_id", "library", "test_script_path", "ground_truth_path",
    "num_tests", "random_seed", "timeout_secs", "benchmark_root",
] {
    if let Some(v) = payload_json.get(key) {
        config[key] = v.clone();
    }
}
```

`normalize_dataset` 可增加 `dscodebench` / `ds-bench` 别名归一化。

---

### 1.5 Episode 执行路径（基本复用）

| 模块 | 是否改动 | 说明 |
|------|----------|------|
| `src/episode/executor.rs` | 基本不变 | reset → ModelClient.infer → plugin.step → ReportResult |
| `src/episode/model_client.rs` | 可选 | code 任务 prompt 若需 Worker 侧重写，在此扩展；否则 Bridge 侧已带完整 question |
| `src/episode/reward_engine.rs` | 不变 | 默认采信插件 `step.reward`；DSCodeBench 判分在插件内完成 |
| `src/plugin/host.rs` | 不变 | 扫描 `plugins/code/manifest.yaml`，spawn 子进程 |
| `src/pool/warmup_pool.rs` | 不变 | 按 `env_type=code` 独立分池 |
| `src/hub/env_resolver.rs` | 轻量 | Hub pull/sync 时识别 `code` env_type；拉取 **manifest + benchmark 制品** 到本地 |

---

### 1.6 运行时依赖与部署

Worker 节点上的 Python 与 10 库应来自 **Hub 预缓存的依赖制品**（venv/wheel tar），经 `uenv env sync` 落到本地；Episode 运行时不再 pip install 或访问 PyPI。

内网导入机（一次性，可访问外网）负责：

- 克隆 [DSCodeBench](https://github.com/ShuyinOuyang/DSCodeBench) 并打包 `benchmark/`
- 锁定版本的 pandas / numpy / … wheel
- `uenv env publish` 将 code EnvPackage（manifest + benchmark tar + 依赖 tar + 插件）入库 Hub

Worker 部署：

```bash
uenv env sync dscodebench --version 0.1.0
export UENV_DSCODEBENCH_ROOT=/var/lib/uenv/envs/dscodebench/0.1.0/benchmark
```

开发联调可暂时使用仓库内 `plugins/code/` + 手工路径，**不等同于内网生产路径**。

**EnvPackage 应包含的制品**

| 类别 | 内容 |
|------|------|
| Python 生态 | Python 3.10+ 及 NumPy、Pandas、SciPy、Scikit-learn、TensorFlow、PyTorch、Matplotlib、Seaborn、Keras、LightGBM（版本与 DSCodeBench 官方 README 对齐），以 wheel/venv tar 形式入库 |
| Benchmark | `benchmark/` 数据目录、`benchmark_construction_evaluation/` 评测脚本 |
| 插件 | `uenv-code-plugin`、`evaluate_code.py` |

**环境变量（Worker sync 后）**

| 变量 | 说明 |
|------|------|
| `UENV_DSCODEBENCH_ROOT` | Hub sync 后的 benchmark 根路径（包内相对路径即可） |
| `UENV_CODE_PYTHON` | Python 解释器路径（默认 `python3`） |
| `UENV_CODE_TIMEOUT_SECS` | 全局默认超时 |
| `UENV_CODE_SANDBOX` | `process` / `podman`（Phase 2） |

---

### 1.7 Worker 侧测试计划

| 层级 | 内容 | 参考现有测试 |
|------|------|--------------|
| 单元 | 代码提取、timeout、reward 解析、dataset 归一化 | `plugins/math/src/backends/gsm8k/scoring.rs` tests |
| 插件 IPC | code 插件 reset/step 往返 | `tests/m4_plugin_host_process.rs` |
| 预热池 | code env_type 独立 acquire/release | `tests/m6_warmup_pool.rs` |
| 集成 | executor 全链路（mock LLM + 真实 harness） | `tests/m5_episode_executor.rs` |
| 对齐 | 与 DSCodeBench 官方 pass@1 golden 对比 | 新建 `tests/dscodebench_golden.rs` |

**Fixture**（新建）：

```
fixtures/code/
├── episode_001.pb           # env_type=code, dataset=dscodebench
├── expected_result_001.pb
└── samples/
    └── ds_001.json            # 最小样本，含 test_script_path
```

---

### 1.8 Worker 侧实施顺序（建议）

| 序号 | 任务 | 依赖 | 状态（2026-07-11） |
|------|------|------|-------------------|
| W-1 |  scaffold `plugins/code/` + manifest + run.sh | — | ✅ |
| W-2 | 实现 `uenv-code-plugin` Reset/Step/Close | W-1 | ✅（Close 清状态；HealthCheck 校验 python/脚本/ROOT） |
| W-3 | 集成 DSCodeBench 官方 harness（executor） | W-2 + benchmark 数据 | ✅ `dscodebench_harness.py`（对齐 `run_test.py` 单题语义；**非**不存在的 `evaluate.py`） |
| W-4 | 扩展 `build_reset_config` payload 透传 | — | ✅ 含 `ground_truth_code/path` |
| W-5 | 配置 `env.types: [code]` + WarmupPool 分池验证 | W-1 | ✅ m6 math/code 独立池 |
| W-6 | 单元 / 插件 / E2E 测试 + golden 对齐 | W-3 | ✅ `dscodebench_golden` + stdlib fixture `ds_001` |
| W-7 | Phase 2：Podman 沙箱 backend | W-3 | ❌ 未做 |

---

## 二、其他模块调整（简要）

### 2.1 `uenv-server` — 无需改动

- 按 `env_type=code` 调度 Worker，逻辑与 math 相同。
- Worker 注册 `supported_env_types` 含 `code` 即可。

### 2.2 `uenv-bridge` — 轻到中等

| 项 | 改动 |
|----|------|
| `verl_agent_loop.py` | `_env_type()` 增加 `dscodebench` / `ds-bench` → `code` |
| `core/src/core.rs` | `sample_to_worker_payload` 为 `env_type=code` 透传 DSCodeBench 字段（参照 SWE 分支） |
| `configs/uenv-agent-loop.yaml` | `task_to_env_type.dscodebench: code` |
| VeRL Dataset | 样本 `env_config` 对齐 Worker payload 契约 |

### 2.3 `uenv-hub` — 内网预缓存（非仅 manifest）

整体在内网部署时，Hub 负责 **提前抓取并缓存** DSCodeBench 所需制品，Worker 部署期 sync，运行时零 egress：

| 项 | 改动 |
|----|------|
| code env registry | manifest 从 placeholder → 真实插件描述（`datasets: [dscodebench]`、`config_schema`） |
| **EnvPackage 制品** | `benchmark/` tar、Python 依赖 tar、插件二进制、`evaluate_code.py` |
| 发布流程 | 导入机打包 → Hub `publish` / artifact POST → Worker `uenv env sync dscodebench` |
| scaffold | 模板 `code` 对齐实际目录与制品清单 |

详见 [`Docs/hub/uenv-hub环境标准化指南.md`](../hub/uenv-hub环境标准化指南.md)。

### 2.4 `plugin_proto/` / `proto/` — 基本不变

- L2 `PluginService`（Reset/Step/Close）与 L1 gRPC 协议可复用。
- 丰富 step info 通过 `StepResponse.info` JSON 扩展，无需改 proto。

### 2.5 文档与 CI — 轻量

- 更新 `Docs/更新日志.md`、Bridge 联调文档中的 payload 字段表。
- CI 增加 code 插件编译与单元测试 job（可选 Podman 集成测试）。

---

## 三、Payload 契约（跨层对齐）

### L1 `EpisodeRequest`

```json
{
  "env_type": "code",
  "payload": {
    "request_id": "req-001",
    "dataset": "dscodebench",
    "question": "Given a DataFrame df, compute ...",
    "task_id": "ds_001",
    "library": "pandas",
    "test_script_path": "benchmark/pandas/ds_001_test.py",
    "num_tests": 200,
    "random_seed": 42
  },
  "reward_config": {
    "type": "rule_reward"
  }
}
```

### 插件 episode config（Worker `build_reset_config` 输出）

与 payload 字段一致，附加 `target`（若 reward_config 有 ground_truth 则写入，DSCodeBench 通常不需要）。

### `StepResponse` 判分

- `reward`: `1.0`（全部测试通过）或 `0.0`
- `terminated`: `true`
- `info`: 含 `tests_run = raw`、`execution_time_ms`、`error` 等

---

## 四、与现有能力对比

| 维度 | GSM8K (math) | DSCodeBench (code) |
|------|--------------|-------------------|
| env_type | `math` | `code` |
| dataset | `gsm8k` | `dscodebench` |
| 判分 | 字符串匹配（插件内） | 沙箱执行官方测试（插件内） |
| 预热池 | math 独立池 | code 独立池 |
| Backend | process | process（MVP）→ podman（生产） |
| Worker payload | question + target | question + task_id + test_script + … |
| 运行时依赖 | 无 | Python + 10 数据科学库 |

---

## 五、风险与待决项

| 项 | 说明 | 建议 |
|----|------|------|
| 执行耗时 | 每题 ~200 测试，Episode 延迟显著高于 GSM8K | 预热池 size、并发上限单独调优；metrics 区分 code/math |
| 依赖版本 | 10 库版本漂移导致与官方结果不一致 | 锁定 Docker 镜像版本；golden 测试对齐 |
| 安全 | 模型生成代码不可信 | Phase 2 必须上 Podman；MVP 仅限内网 |
| pass@k | 训练可能需要多次采样 | 当前单轮 step 返回 0/1；pass@k 由 Bridge/训练侧聚合 |
| Hub 制品体积 | benchmark 数据 + 脚本 + Python 依赖较大 | **Hub 内网预缓存**全量 tar；导入机一次性外网抓取；Worker 仅 `sync`，不实时下载 |

---

## 六、验收标准（Worker 层）

1. Worker 注册 `supported_env_types` 含 `code`，Server 可将 DSCodeBench Episode 调度到该 Worker。
2. `env_type=code` + `dataset=dscodebench` 完整跑通：Register → Dispatch → Reset → Infer → Step → ReportResult。
3. 插件 `step.reward` 与 DSCodeBench 官方 harness 在固定 seed 下结果一致（golden 样本）。
4. code 预热池 hit/miss 可观测；插件 crash 不拖垮 Worker（与 math 同级隔离）。
5. step info 含测试通过数、耗时、错误信息，便于 trajectory 分析与调试。

---

## 附录：关键代码路径索引

| 主题 | 路径 |
|------|------|
| Math 插件参考 | `plugins/math/`、`uenv-worker/src/bin/uenv-math-plugin.rs` |
| GSM8K 判分 | `plugins/math/src/backends/gsm8k/scoring.rs` |
| Payload 构建 | `uenv-worker/src/episode/payload.rs` |
| Episode 编排 | `uenv-worker/src/episode/executor.rs` |
| Reward 平台层 | `uenv-worker/src/episode/reward_engine.rs` |
| 插件托管 | `uenv-worker/src/plugin/host.rs` |
| 预热池 | `uenv-worker/src/pool/warmup_pool.rs` |
| SWE 容器执行参考 | `uenv-worker/src/swe/harness.rs`、`session.rs` |
| Worker 配置 | `config/uenv-worker.yaml` |
| 设计文档 | `Docs/worker-pool-layer-design.md` §9.5 |
