# DSCodeBench 评测返回口径修复与全链路联调记录

- 日期：2026-07-18
- 处理范围：7143 Worker（`219.147.100.43:7143`）、7142 Adapter（`219.147.100.43:7142`）、Server（`8.130.75.157:8088`）
- 关联报告：
  - [DSCodeBench-UEnv核验问题与修复.md](../worker/260718/DSCodeBench-UEnv核验问题与修复.md)（核验结论 + 修复 + 联调详情）
  - [DSCodeBench-UEnv评测Worker依赖问题说明.md](../worker/260717/DSCodeBench-UEnv评测Worker依赖问题说明.md)（前一轮依赖缺失）
  - 上游核验文档：`feature/verl-bridge-adapter:Docs/adapter/DSCodeBench-UEnv评测Worker核验问题说明.md`

---

## 1. 概述

Adapter 侧完成 DSCodeBench 全量 1000 条 UEnv 评测后，归档了一份核验文档，指出 `pass@1=0.267`、`error_count=733` 等指标存在口径问题。本轮先核验其结论真伪，再按优先级修复，并同步到服务器完成全链路联调。

| 问题 | 类别 | 根因 | 处置 |
|---|---|---|---|
| wrong answer 顶层塌成 `tests_run=0` | **返回口径缺陷（P0）** | Adapter inline wrapper 对 `passed=false` 抛 `AssertionError`；Worker 异常兜底一律记 `tests_run=0`，真实结果被埋进 traceback | 去掉 AssertionError，改为透传 namespace 中 `_result`；Worker 解析并结构化返回 |
| `error_count` 混入答案错误 | **指标口径缺陷（P0）** | 指标把 wrong answer 与环境/执行错误混算 | 新增 `error_category`，指标拆分 `wrong_answer_count` / `error_category_counts` |
| pandas Series 比较报 ambiguous | **harness 缺陷（P1）** | `values_equal` 未处理 `pd.Series`，`result == ans` 得到 Series 后布尔化触发 ambiguous | 补 Series 分支 + array-like 安全布尔化 |

三项修复均已同步 7143/7142、重新编译 Worker、重启并全链路验证生效。

> 核验附带结论：`scipy.mstats` 类报错更像模型错误导入（正确路径 `scipy.stats.mstats`），非环境缺包；UEnv 与直接 baseline 生成参数不同（thinking、`MAX_TOKENS`），pass@1 不可直接对比。均记录在关联核验文档，不在本轮代码范围。

---

## 2. 核验：报告结论是否属实

对照代码逐条核验，结论「核心判断成立」：

1. **返回口径问题属实（P0）**。因果链在代码中完全对应：
   - `evaluate_dscodebench_uenv.py` 的 `build_inline_harness_test_code` 对 `not _result.get("passed")` 抛 `AssertionError`。
   - `plugins/code/scripts/evaluate_code.py` 顶层 `except Exception` 一律 `_result(False, 0, 0, ...)`。
   - `execution_rate` 以 `tests_run > 0` 统计，导致仅全通过样本被计为「已执行」。
   - 结论：`execution_rate=0.267` 实为近似 pass@1，与 baseline 的 `0.877` 不可比；`error_count=733` 混合答案错误与真实错误。
2. **参数不同不可直接比 pass@1 属实**。
3. **环境类问题部分属实**：早期确有缺 numpy（前一轮已装）；`pandas Series ambiguous` 是 harness bug（本轮修）；`scipy.mstats` 更像模型导入错误。

---

## 3. 代码变更清单

| 文件 | 变更 |
|---|---|
| `uenv-bridge/scripts/benchmark/evaluate_dscodebench_uenv.py` | inline wrapper 不再 raise，`_result` 留在 namespace；row 透传 `worker_error_category`；`compute_metrics` 拆出 `wrong_answer_count` / `error_category_counts`，`error_count` 排除 wrong answer；CSV 增列 |
| `plugins/code/scripts/evaluate_code.py` | `run_inline_tests` 优先读取 namespace `_result`；保留旧版 AssertionError JSON 解析兜底；候选/测试 exec 分别捕获并按 `_classify_exception` 分类；各失败路径带 `error_category` |
| `plugins/code/scripts/dscodebench_harness.py` | `values_equal` 新增 `pd.Series` 分支与 `_as_bool_equal` array-like 安全布尔化；`evaluate_problem` 输出 `error_category`（`_classify_harness_error`） |
| `plugins/code/src/backends/dscodebench/executor.rs` | `EvaluationResult` 增 `error_category`；新增 `fail_result` 帮助函数，超时→`timeout`、解析/spawn 失败→`harness_error` 等带分类 |
| `plugins/code/src/backends/dscodebench/scoring.rs` | `StepInfo` 增 `error_category` 透出；单测补字段 |
| `uenv-worker/src/bin/uenv-code-plugin.rs` | step `info` 写入 `error_category` |

### 3.1 修复后返回语义

| 场景 | `passed` | `tests_run` | `error_category` |
|---|---|---:|---|
| 全部通过 | true | 200 | 空 |
| 部分测试失败 | false | 200 | `wrong_answer` |
| 候选代码无输出 | false | ≥0 | `candidate_runtime_error` |
| 缺包 / 导入失败 | false | 0 | `dependency_error` |
| 单题超时 | false | 0 | `timeout` |
| harness 自身异常 | false | 0 | `harness_error` |

---

## 4. 本地验证

- `cargo test -p uenv-code-env --lib`：7/7 通过（inline / official harness / timeout）。
- Python 冒烟：结构化失败保留 `tests_run=200`；旧 AssertionError JSON 可解析；pandas Series / numpy 比较不触发 ambiguous。

---

## 5. 服务器同步与全链路联调

参考 `secrets/README.md`：Worker **7143**、Adapter **7142**、Core **`8.130.75.157:8088`**。

### 5.1 同步与编译

- scp 5 个 Worker 文件到 7143 `/root/UEnv`，scp Adapter 脚本到 7142 `/data/ronghao/uenv/uenv-bridge`。
- 7143 `cargo build -p uenv-worker --release`（约 15s），二进制 23:56 重建。
- `SKIP_REBUILD=1 bash scripts/restart-worker-gateway-28097-7143.sh` 重启；health `ok`；`register` + `heartbeat` 正常；`UENV_CODE_PYTHON` 指向 dscodebench venv（numpy 2.5.1 / pandas 3.0.3）。

### 5.2 Worker 侧数据驱动验证（真实 numpy_0 样本）

| 候选 | 结果 |
|---|---|
| ground truth | `passed=true, tests_run=5, tests_passed=5` |
| 可运行但答案 ×2 | `passed=false, tests_run=5, tests_passed=0, error_category=wrong_answer` |
| 语法错误 | `passed=false, tests_run=0, error_category=candidate_runtime_error` |
| Series / ndarray 比较 | 不触发 ambiguous |

### 5.3 全链路 UEnv smoke（打通）

```text
LIMIT=1 LIBRARY=numpy ENABLE_THINKING=1 MAX_TOKENS=8192 THINKING_TOKEN_BUDGET=4096 TEST_CASE_NUMBER=5
→ uenv_status=completed
→ tests_run=5, tests_passed=0
→ worker_error_category=wrong_answer
→ metrics: execution_rate=1.0, error_count=0, wrong_answer_count=1, error_category_counts={"wrong_answer":1}
```

一条结果同时验证三项修复端到端生效：`tests_run` 不再塌成 0、`error_category` 一路透传、指标拆分正确。

---

## 6. 联调排障（5001 根因链）

首次 smoke 连续 `5001`，逐层定位：

| 层 | 现象 | 根因 | 处理 |
|---|---|---|---|
| Server 调度 | `no worker available: all workers at capacity` | Worker 被标 **`draining`**：重启时旧进程 SIGTERM 优雅排空的 drain 晚于新进程 register 到达（`register_worker` 对有 active load 的同名 worker 保留旧记录并标 draining） | `kill -9` 干净结束旧进程 + 全新启动 → Server `/workers` 状态回 `ready` |
| Server 调度 | 偶发 `all workers at capacity` | `worker_is_eligible` 兜底：单 Worker 被 OlymMATH 长 episode 占用；`is_worker_degraded`（有负载且 > `worker_degraded_threshold_secs=400s` 未 report）不 eligible，`classify_no_candidate` 兜底归为 `AllWorkersAtCapacity` | 观测确认多数时刻 `degraded=False`、`load=1<4`，属并发竞争非回归 |
| Worker 执行 | `exceeded max attempts (3)` | code episode 已 dispatch/acquire，但 `model_callback` 返回 `ERR_MODEL_CALL_FAILED: response missing choices[0].message.content` | 模型对「thinking 关 + `MAX_TOKENS` 过小」返回空 content（Qwen3 推理模型内容全在 reasoning 或被截断）；改用 thinking 开 + `MAX_TOKENS=8192` 后正常 |

关键点：

1. Server admin 为本机监听：`curl http://127.0.0.1:50052/workers`（经 7143 → sshpass 登 Server）。
2. `draining` 是重启竞态常见后果；用 `kill -9` 避免旧进程发优雅 drain，或重启后 `curl /workers` 确认 `status=ready`。
3. 调度器把 degraded / 资源不匹配 / 容量满统一兜底为 `AllWorkersAtCapacity`，排查需结合 `load` / `last_report_secs` 与 Worker 日志。
4. code episode 的 `exceeded max attempts` 属模型生成参数问题，与本次 harness 改动无关。

---

## 7. 结论与后续

- 三项修复（P0 返回口径、P0 指标拆分、P1 Series 比较）已在生产 venv 与全链路验证通过。
- `5001` 系列问题均与本次改动无关：先是 draining 重启竞态，后是生成参数导致模型空 content。

后续（P2，非本轮代码）：

1. 记录并对齐 Worker `UENV_CODE_PYTHON` 与 baseline `EVAL_IMAGE` 的 10 库版本。
2. 同口径补跑对比：`ENABLE_THINKING` 与 `MAX_TOKENS` 对齐后再比 UEnv vs 直接 baseline。
3. 抽检残留 `dependency_error` / `scipy.mstats` 样例，区分模型导入错误与真实缺包。

### 运维注意

1. 重启 Worker 建议 `kill -9` 旧进程以免 `draining` 竞态。
2. 单 Worker + 长 OlymMATH 运行时 DSCodeBench 全链路会争容量，批量评测建议错峰或加 Worker。
3. DSCodeBench 生成务必开 thinking 且 `MAX_TOKENS` 足够（成功全量参考：thinking 开、`MAX_TOKENS=32768`、`THINKING_TOKEN_BUDGET=16384`）。
4. 7142 评测镜像 `localhost/uenv-bridge-verl:layer4-build` 已缺失，可用主机 `PYTHONPATH=src python3` 直跑，脚本已挂载 `/data/ronghao`。
