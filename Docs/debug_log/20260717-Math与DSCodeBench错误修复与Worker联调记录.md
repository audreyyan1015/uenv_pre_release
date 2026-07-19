# Math 与 DSCodeBench 错误修复与 Worker 联调记录

- 日期：2026-07-17
- 处理范围：7143 Worker（`219.147.100.43:7143`）
- 关联报告：
  - [数学错误排查.md](../worker/260717/数学错误排查.md)（OlymMATH `step()` h2 CANCEL）
  - [DSCodeBench-UEnv评测Worker依赖问题说明.md](../worker/260717/DSCodeBench-UEnv评测Worker依赖问题说明.md)（`ModuleNotFoundError: numpy`）
  - [failed_requests_for_server_worker.md](../worker/260717/failed_requests_for_server_worker.md)

---

## 1. 概述

本轮修复了两个使 UEnv 评测无法有效运行的问题：

| 问题 | 类别 | 根因 | 处置 |
|---|---|---|---|
| OlymMATH 149/400 失败（ZH 占 147） | **源码缺陷** | Math 判分 `extract_boxed` 在多字节 UTF-8 边界内做字符串切片，触发 panic → h2 `CANCEL` → `ERR_ENV_STEP_FAILED` → Server 重试 3 次 → `5001` | 改为字节比较，杜绝 panic；`step()` 增加 `catch_unwind` 兜底；插件 `stderr` 继承以保留日志 |
| DSCodeBench `tests_run=0` | **环境缺陷** | `UENV_CODE_PYTHON` 指向的隔离 venv 未安装任何数据科学库 | 向该 venv 安装全量 DS 依赖 |

两个修复均已同步到 7143、重新编译、服务器侧联调确认生效，并完成 Worker 重启。

---

## 2. 问题一：Math OlymMATH `step()` panic

### 2.1 根因

判分链路：`uenv-math-plugin.step()` → `score_action(dataset, action, expected)` → `olymmath::answers_match` → `extract_solution` → `extract_boxed`。

原实现按字节游标 `i` 逐字节扫描，却用 `&text[i..i+7]` 对字符串切片：

```rust
while i + 7 <= bytes.len() {
    if &text[i..i + 7] == "\\boxed{" && (i == 0 || bytes[i - 1] != b'\\') {
```

当 `action` 含中文 / 部分 LaTeX 等多字节 UTF-8 字符时，`i` 会落在字符内部，`&text[i..i+7]` 直接 panic（`byte index is not a char boundary`）。该 panic 冒泡到 tonic 处理任务，h2 流被重置为 `CANCEL`，Worker 包装成 `execute_episode_failed`，Server 误当可重试的 `dispatch_failed` 并重复三次昂贵模型生成，最终返回 `5001`。

这解释了报告中的现象：失败集中在中文数据（ZH 147/149）、模型返回后约 1ms 在 `step()` 失败、Gateway 侧 698 次 HTTP 200 全部成功。

### 2.2 修复

`plugins/math/src/backends/olymmath/scoring.rs`：`extract_boxed` 全程改用**字节比较**（`&bytes[i..i+7] == b"\\boxed{"`），内容切片 `text[start..j]` 的 `start`/`j` 均落在 ASCII 花括号处，为合法字符边界；保留“取最后一个顶层 `\boxed{}`”语义。新增两条回归单测：

- `does_not_panic_on_multibyte_utf8`：`解：……组合数为 \binom{n}{2}，最终答案是 \boxed{\frac{7}{3}}。`
- `multibyte_without_boxed_falls_back`：纯中文、无 boxed，走 trim 兜底不 panic。

### 2.3 防御性加固

- `uenv-worker/src/bin/uenv-math-plugin.rs`：`step()` 用 `std::panic::catch_unwind` 包裹 `score_action`，任何未来判分异常都转为结构化返回（`reward=0.0` + `info["score_error"]="score_action_panic"`），不再让 panic 冒泡导致 h2 CANCEL。
- `uenv-worker/src/backend/process.rs`：插件子进程 `stderr` 由 `Stdio::null()` 改为 `Stdio::inherit()`，使插件 panic / `eprintln!` 落入 Worker 日志（此前报告指出 stderr 被丢弃导致无线索）。

> 说明：Server 侧“把确定性 `ERR_ENV_STEP_FAILED` 当作可重试 `dispatch_failed`”的重试策略属 Server 组件，本轮未改动；Worker 侧根因修复后已不会再触发该 panic 路径。

---

## 3. 问题二：DSCodeBench 依赖缺失

### 3.1 根因

Worker `code` 插件执行候选代码时使用 `UENV_CODE_PYTHON`（`/root/.uenv-worker.env` 中已设为
`/var/lib/uenv/envs/dscodebench/0.2.0/venv/bin/python`）。但该 venv 为隔离环境：

```
# pyvenv.cfg
include-system-site-packages = false
```

`site-packages` 下仅有 pip，`numpy` 等数据科学库全部缺失，候选代码首行 `import numpy` 即 `ModuleNotFoundError`，DSCodeBench harness 无从开始，`tests_run=0`。

### 3.2 修复

向该 venv 安装 DSCodeBench 全量依赖（pip 走仓库既有的清华镜像）：

```bash
V=/var/lib/uenv/envs/dscodebench/0.2.0/venv
$V/bin/pip install numpy pandas scipy scikit-learn matplotlib seaborn lightgbm torch tensorflow keras
```

安装结果（关键版本）：`numpy-2.5.1 pandas-3.0.3 scipy-1.18.0 scikit-learn-1.9.0 matplotlib-3.11.0 seaborn-0.13.2 lightgbm-4.6.0 torch-2.13.0 tensorflow-2.21.0 keras-3.15.0`（含配套 nvidia-cu13 依赖）。

> 此项为环境侧配置，`UENV_CODE_PYTHON` 已在 `/root/.uenv-worker.env` 生效，无需改源码。

---

## 4. 代码变更清单

| 文件 | 变更 |
|---|---|
| `plugins/math/src/backends/olymmath/scoring.rs` | `extract_boxed` 改字节比较；新增 2 条多字节回归单测 |
| `uenv-worker/src/bin/uenv-math-plugin.rs` | `step()` 增加 `catch_unwind` 判分兜底 |
| `uenv-worker/src/backend/process.rs` | 插件子进程 `stderr` 改 `Stdio::inherit()` |

---

## 5. 同步、编译与联调

### 5.1 同步与编译

- 三个源文件经 `scp` 同步到 7143 `/root/UEnv`（`/root/UEnv` 为文件部署、非 git）。
- 服务器 `bash scripts/gen-worker-proto.sh && cargo build -p uenv-worker --release` 通过（增量约 15s）；
- 新二进制 `target/release/{uenv-worker,uenv-math-plugin,uenv-code-plugin}` 时间戳 `2026-07-17T14:10`。

### 5.2 联调验证（服务器侧）

- **Math**：`cargo test -p uenv-math-env` → 16/16 通过，含中文+LaTeX 回归（与 `step()` 判分同一路径）。
- **DSCodeBench**：
  - venv 10 个库 `import` 全部 OK（numpy/pandas/scipy/sklearn/matplotlib/seaborn/lightgbm/tensorflow/keras/torch）；
  - 用 venv python 直跑 `plugins/code/scripts/evaluate_code.py`（真实执行子进程路径）的 numpy/pandas 样本：

    ```json
    {"passed": true, "tests_run": 2, "tests_passed": 2, "execution_time_ms": 196, "error": null}
    ```

    即 `tests_run` 由 `0` 变为 `>0`，不再出现 `ModuleNotFoundError`。

---

## 6. Worker 重启与运维清理

1. 停止旧 Worker，并清理历史泄漏的 **671 个 math + 55 个 code** 孤儿插件进程及陈旧 socket。
2. 清理陈旧 WAL 噪声：删除 `/tmp/uenv/wal-swe/olymmath-OlymMATH-EASY-23-EN-772b8813__1__worker-7143-pro__lease-27.wal`（Jul 16 旧 run、Server 已 `DUPLICATE_REJECTED`），消除持续每数秒一次的 `wal_replay_report_failed` 告警。
3. 通过团队标准脚本 `scripts/restart-worker-gateway-28097-7143.sh`（`SKIP_REBUILD=1`）拉起新二进制。

重启后日志确认（healthy）：

```
hub_manifest_pulled → worker_start(register_endpoint=219.147.100.43:28888, server_endpoint=8.130.75.157:8088)
→ warmup_pool_prewarmed_on_startup(warmup_size=4) → register → heartbeat...
runtime_gateway_start(gateway_addr=0.0.0.0:28097, catalog=731)
```

- health `ok`；`28097/28777/28888` 均监听；
- 预热池 `math=4 / code=4`，均为新 Worker 子进程、运行 14:10 重建的插件二进制；
- 新进程环境含 `UENV_CODE_PYTHON=/var/lib/uenv/envs/dscodebench/0.2.0/venv/bin/python`；
- 重启后 `wal_replay_report_failed / DUPLICATE_REJECTED` 计数为 **0**。

---

## 7. 遗留与后续

- `hub_pull_failed_using_local_manifest`（`swe` env 在 Hub 返回 404，降级本地 manifest）为既有、与本次无关的告警，不阻塞注册/Episode。
- 全链路正式 smoke（Adapter@7142 → Server → Worker）由 Adapter 侧按 [DSCodeBench-UEnv评测Worker依赖问题说明.md](../worker/260717/DSCodeBench-UEnv评测Worker依赖问题说明.md) §6 与 OlymMATH 脚本重跑收口；Worker 侧修复已就绪并生效。
- 若后续 DSCodeBench 依赖需要固化/复现，建议将 venv 依赖清单纳入 EnvPackage 或部署脚本，避免重装。

---

## 8. 回滚方式

- 源码：三处改动均为独立小改，可用 git 反向 patch；服务器侧 `.bak` 备份已在验证通过后删除。
- 环境：DS 依赖安装在独立 venv，回滚仅需删除该 venv 或清空其 `site-packages`，不影响系统 python。
