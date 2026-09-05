# DSCodeBench UEnv 核验问题与修复

## 1. 背景

Adapter 侧在 `feature/verl-bridge-adapter` 归档了全量 1000 条 DSCodeBench UEnv 评测核验说明（`Docs/adapter/DSCodeBench-UEnv评测Worker核验问题说明.md`）。本轮在当前分支对报告中的问题做代码核验，并按优先级修复。

评测链路：

```text
Adapter -> Adapter Core / Server -> Worker code env -> Model Gateway -> harness -> Adapter
```

全量结果摘要（核验前 / 修复前口径）：

| problem_count | completed_count | failed_count | passed_count | pass@1 | error_count |
|---:|---:|---:|---:|---:|---:|
| 1000 | 1000 | 0 | 267 | 0.267 | 733 |

`failed_count=0` 表示调度层无失败；`error_count=733` 与 `execution_rate=0.267` 受返回口径影响，不能直接当作环境错误或执行成功率。

## 2. 核验结论

### 2.1 `tests_run=0` / AssertionError 口径问题 — 属实（P0）

因果链：

1. Adapter `inline_harness` 在 `_result.passed=false` 时 `raise AssertionError(json.dumps(_result))`。
2. Worker `evaluate_code.py` 捕获异常后一律返回 `tests_run=0, tests_passed=0`，真实 harness 结果被埋在 traceback 字符串里。
3. Adapter 用 `tests_run > 0` 统计 `execution_rate`，导致只有全通过样本被计为「已执行」。

因此：

- `execution_rate=0.267` 实质接近 pass@1，与直接 baseline 的 `execution_rate=0.877` **不可比**。
- `error_count=733` 混合了 wrong answer 与真实执行/依赖错误。

责任不完全在 Worker：Adapter wrapper 用异常表示「测试未通过」是主因；Worker 异常兜底把结构化结果抹掉是次因。Worker 侧 `path_harness` / `run_official_harness` 本就可正确返回 `_result`。

### 2.2 与 baseline 参数不同 — 属实

| 项 | 直接 baseline | 本轮 UEnv |
|---|---|---|
| Thinking | 关闭 | 开启 |
| `MAX_TOKENS` | 2048 | 32768 |
| 评测器 | 官方 `run_test.py` | Worker `dscodebench_harness` |

不能据此判定「UEnv 传输链路拉低效果」。同口径对比需补跑：thinking 关闭 + `MAX_TOKENS=2048` + `TEST_CASE_NUMBER=200`。

### 2.3 环境 / 依赖类问题 — 部分属实

| 现象 | 核验 |
|---|---|
| 早期 smoke `No module named 'numpy'` | 属实；后续已向 `UENV_CODE_PYTHON` venv 安装 10 库 |
| `scipy.mstats` / `from scipy import mstats` | 更像模型错误导入（正确路径为 `scipy.stats.mstats`），不是缺包 |
| `evaluation timed out after 300s` | 属实，与 code env 超时文案一致 |
| `pandas Series truth value ambiguous` | 属实，且为 harness bug：`values_equal` 未处理 Series，`1 if values_equal(...)` 触发 ambiguous |
| numpy array truth value ambiguous | 可能来自候选代码或比较路径漏洞，不全是缺包 |
| LightGBM / TF-Keras | 可能版本或模型代码问题，需结合版本与样例再判 |

### 2.4 修复优先级

1. **P0**：修正失败返回口径，透传 `tests_run` / `tests_passed`（必要时带 `error_category`）。
2. **P1**：修复 harness 对 pandas Series / array-like 的比较。
3. **P2**：环境版本对齐与同参数对比实验（运维/评测侧，非本轮代码范围）。

## 3. 修复计划

### 3.1 返回口径（P0）

- Adapter：`build_inline_harness_test_code` 不再对 `passed=false` 抛 `AssertionError`，将 `_result` 留在 namespace。
- Worker：`run_inline_tests` 优先读取 namespace 中的 `_result`；保留对旧版 AssertionError JSON 的解析兜底。
- harness / evaluate_code：补充 `error_category`，并经插件 `info` 透出。
- Adapter 指标：`error_count` 排除 `wrong_answer`；新增 `wrong_answer_count` / `error_category_counts`。

### 3.2 harness 比较（P1）

- `values_equal` 增加 `pd.Series` 分支（`assert_series_equal`）。
- 对 `result == ans` 产生的 ndarray / array-like 做安全布尔化，避免 `truth value is ambiguous`。

### 3.3 非本轮范围

- 依赖版本与 baseline 容器逐一对齐。
- 同口径关闭 thinking 的全量复跑。
- 将 `scipy.mstats` 当环境缺包修复（应先归为模型导入错误）。

## 4. 修复记录（2026-07-18）

### 4.1 已落地改动

| 文件 | 变更 |
|---|---|
| `uenv-bridge/scripts/benchmark/evaluate_dscodebench_uenv.py` | inline harness 不再 raise；透传 `worker_error_category`；指标拆分 wrong_answer / error_category |
| `plugins/code/scripts/evaluate_code.py` | 读取 namespace `_result`；AssertionError JSON 兜底；异常分类 |
| `plugins/code/scripts/dscodebench_harness.py` | Series / array-like 安全比较；`evaluate_problem` 输出 `error_category` |
| `plugins/code/src/backends/dscodebench/executor.rs` | `EvaluationResult.error_category`；超时等失败带 category |
| `plugins/code/src/backends/dscodebench/scoring.rs` | `StepInfo` 透出 `error_category` |
| `uenv-worker/src/bin/uenv-code-plugin.rs` | step `info` 写入 `error_category` |

### 4.2 返回语义（修复后）

| 场景 | `passed` | `tests_run` | `error_category` |
|---|---|---:|---|
| 全部通过 | true | 200 | 空 |
| 部分测试失败 | false | 200 | `wrong_answer` |
| 候选代码无输出 | false | ≥0 | `candidate_runtime_error` |
| 缺包 / 导入失败 | false | 0 | `dependency_error` |
| 单题超时 | false | 0 | `timeout` |
| harness 自身异常 | false | 0 | `harness_error` |

### 4.3 验证

本地已验证：

- `cargo test -p uenv-code-env --lib`：7/7 通过（含 inline / official harness / timeout）。
- Python 冒烟：`_result` 结构化失败保留 `tests_run=200`；旧 AssertionError JSON 可解析；pandas Series / numpy 比较不触发 ambiguous。

部署注意：Worker 机器需同步 `plugins/code/scripts/{evaluate_code,dscodebench_harness}.py` 与重新编译的 `uenv-code-plugin` / `uenv-code-env`，Adapter 侧同步评测脚本后，再跑全量才能看到新口径指标。

### 4.4 修复后预期效果

在同样模型输出下复跑或重评时：

1. wrong answer 样本顶层应出现真实 `tests_run` / `tests_passed`，不再统一为 0。
2. `execution_rate` 将接近「成功跑完 harness 并产出测试结果」的比例，可与 baseline 口径对照（仍需同生成参数才有可比性）。
3. `error_count` 不再把 `wrong_answer` 算进环境/执行错误；可用 `error_category_counts` 拆分。
4. pandas Series 比较失败应落为 `wrong_answer`（测试未通过），而不是 harness 抛 ambiguous 异常。

### 4.5 仍待事项（P2）

1. 在 Worker 节点记录 `UENV_CODE_PYTHON` 与 10 库版本，并与 baseline `EVAL_IMAGE` 对齐。
2. 同口径补跑：`ENABLE_THINKING=0` + `MAX_TOKENS=2048` + `TEST_CASE_NUMBER=200`。
3. 对残留 `dependency_error` / `scipy.mstats` 样例做抽检，区分模型导入错误与真实缺包。
4. Server 侧 `5001 all workers at capacity` 在 Worker 空闲时仍出现：疑似 OlymMATH 租约未释放，需 Server 侧排查（见 §5.4）。

## 5. 服务器同步与联调（2026-07-18 / 07-19）

参考 `secrets/README.md` 四端拓扑：Worker **7143**、Adapter **7142**、Core **`8.130.75.157:8088`**。

### 5.1 同步与编译

| 目标 | 操作 |
|---|---|
| 7143 `/root/UEnv` | scp：`dscodebench_harness.py`、`evaluate_code.py`、`executor.rs`、`scoring.rs`、`uenv-code-plugin.rs` |
| 7143 编译 | `cargo build -p uenv-worker --release`（约 15s）；二进制时间戳 **23:56** |
| 7143 重启 | `SKIP_REBUILD=1 bash scripts/restart-worker-gateway-28097-7143.sh`；pid 新进程；health `ok`；监听 `28097/28777/28888` |
| 7142 Adapter | scp：`evaluate_dscodebench_uenv.py` → `/data/ronghao/uenv/uenv-bridge/scripts/benchmark/` |

重启后日志确认：

```text
hub_manifest_pulled (math/code)
worker_start(register_endpoint=219.147.100.43:28888, server_endpoint=8.130.75.157:8088)
warmup_pool_prewarmed_on_startup(warmup_size=4)
register → heartbeat...
UENV_CODE_PYTHON=/var/lib/uenv/envs/dscodebench/0.2.0/venv/bin/python
```

### 5.2 Worker 侧数据驱动验证（覆盖本次改动）

venv：`numpy 2.5.1` / `pandas 3.0.3`。用真实 `numpy_0` 样本 + 修复后的 inline wrapper（无 AssertionError）直跑 `evaluate_code.py`：

| 候选 | 结果 |
|---|---|
| ground truth | `passed=true, tests_run=5, tests_passed=5` |
| 可运行但答案 ×2 | `passed=false, tests_run=5, tests_passed=0, error_category=wrong_answer` |
| 语法错误候选 | `passed=false, tests_run=0, error_category=candidate_runtime_error` |
| Series / ndarray 比较 | 不触发 `truth value is ambiguous` |

结论：**P0 返回口径与 P1 Series 比较在 7143 生产 venv 上已生效**；wrong answer 不再塌缩为 `tests_run=0`。

### 5.3 全链路 UEnv smoke（已打通）

7142 主机直跑（镜像 `localhost/uenv-bridge-verl:layer4-build` 已不存在，故用 `PYTHONPATH=src python3 scripts/benchmark/evaluate_dscodebench_uenv.py`）。

排障过程见 §5.5。最终以「thinking 开 + 足够 max_tokens」跑通 1 条 `numpy_0`：

```text
LIMIT=1 LIBRARY=numpy ENABLE_THINKING=1 MAX_TOKENS=8192 THINKING_TOKEN_BUDGET=4096 TEST_CASE_NUMBER=5
→ uenv_status=completed
→ tests_run=5, tests_passed=0
→ worker_error_category=wrong_answer
→ metrics: execution_rate=1.0, error_count=0, wrong_answer_count=1, error_category_counts={"wrong_answer":1}
```

**这条结果同时验证了本次三项修复的端到端生效**：

1. `tests_run=5`（非 0）——P0 返回口径修复生效，wrong answer 不再塌缩。
2. `worker_error_category=wrong_answer` 一路从 harness → 插件 `info` → Adapter row 透传成功。
3. `execution_rate=1.0` / `error_count=0` / `wrong_answer_count=1`——指标拆分正确，答案错误不再计入环境错误。

产物路径：`temp/benchmarks/dscodebench/smoke_koujing_4_*/`。

### 5.4 联调结论

- 部署链路（Worker 重编译/重启/注册、Adapter 脚本同步）完成。
- Worker 侧 harness 修复（P0/P1）在生产 venv 与全链路上均验证通过。
- 唯一未定量项仍是「同口径 UEnv vs 直接 baseline」的对比实验（P2，需批量运行）。

### 5.5 联调排障记录（5001 根因链）

首次 smoke 连续 `5001`，逐层定位如下：

| 层 | 现象 | 根因 | 处理 |
|---|---|---|---|
| Server 调度 | `5001 no worker available: all workers at capacity` | Worker 被标 **`draining`**：重启时旧进程 SIGTERM 优雅排空的 drain 晚于新进程 register 到达 | `kill -9` 干净结束旧进程 + 全新启动 → Server `/workers` 状态回 `ready` |
| Server 调度 | 偶发 `all workers at capacity` | `worker_is_eligible` 兜底：单 Worker 被 OlymMATH 长 episode 占用，`is_worker_degraded`（有负载且 >`worker_degraded_threshold_secs=400s` 未 report）时不 eligible，兜底归类为 capacity | 观测确认多数时刻 `degraded=False`、`load=1<4`；属并发竞争，非回归 |
| Worker 执行 | `5001 exceeded max attempts (3)` | code episode 已 dispatch/acquire，但 `model_callback` 返回 `ERR_MODEL_CALL_FAILED: response missing choices[0].message.content` | 模型对「thinking 关 + `MAX_TOKENS` 过小」的请求返回空 content（Qwen3 推理模型内容全在 reasoning 或被截断）；改用 thinking 开 + `MAX_TOKENS=8192` 后 content 正常 |

排障关键点：

1. Server admin 为本机监听：`curl http://127.0.0.1:50052/workers`（经 7143 → sshpass 登 Server）。
2. `draining` 状态是重启竞态的常见后果；用 `kill -9` 避免旧进程发优雅 drain，再启动即可清除。
3. 调度器把「degraded / 资源不匹配 / 容量满」统一兜底为 `AllWorkersAtCapacity`，排查时需结合 `/workers` 的 `load`、`last_report_secs` 与 Worker 日志一起看。
4. code episode 的 `5001 exceeded max attempts` 与本次 harness 改动无关，属模型生成参数问题。

### 5.6 运维注意

1. 重启 Worker 建议 `kill -9` 旧进程以免 `draining` 竞态（或重启后 `curl /workers` 确认 `status=ready`）。
2. 单 Worker + 长 OlymMATH 运行时，DSCodeBench 全链路会与之竞争容量；批量评测建议错峰或加 Worker。
3. DSCodeBench 生成务必开 thinking 且 `MAX_TOKENS` 足够（参考成功全量：thinking 开、`MAX_TOKENS=32768`、`THINKING_TOKEN_BUDGET=16384`），否则模型返回空 content 触发 `ERR_MODEL_CALL_FAILED`。
4. 7142 评测镜像缺失时可用主机 Python 直跑；脚本已挂载到 `/data/ronghao`。
