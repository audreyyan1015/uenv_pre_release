# SWE-smith 官方 Harness 判分对照校验

> 日期：2026-08-07  
> 目的：确认当前 UEnv SWE-smith reward 是否与官方 SWE-smith 判分标准一致，并判断什么判分标准可用于训练。  
> 结论：官方 harness 可作为最终 reward 的权威标准；当前 UEnv Worker 内部 `swesmith` grader + 当前镜像/catalog 组合不等价，不能作为 SWE-smith 最终训练 reward 的权威来源。

## 1. 对照对象

### 1.1 官方标准

官方入口来自 SWE-smith 仓库：

```bash
python -m swesmith.harness.eval \
  --dataset_path <dataset.json> \
  --predictions_path <predictions.json 或 gold> \
  --run_id <run_id> \
  --workers 1
```

官方流程：

```text
prediction patch
  -> 官方 RepoProfile 创建官方 eval container
  -> 应用 patch；gold 模式对 bug patch 自动 reverse apply
  -> 运行 profile.test_cmd
  -> swesmith.harness.grading.get_eval_report()
  -> get_resolution_status(report) == FULL
  -> resolved=true
```

官方 report 以 `FAIL_TO_PASS` / `PASS_TO_PASS` 的 success/failure 结构记录结果。

### 1.2 UEnv 当前标准

当前 UEnv Worker 路径：

```text
Runtime Gateway /submit
  -> uenv-worker/src/swe/session.rs evaluate()
  -> apply test_patch
  -> run resolved_test_command(TESTBED)
  -> uenv-worker/src/swe/grader.rs / harness.rs
  -> 所有 FAIL_TO_PASS 和 PASS_TO_PASS 被本地 parser 判为 passed
  -> resolved=true, reward=1.0
```

当前 `SwesmithGrader` 是 UEnv 内部实现。它的目标语义接近官方，但不是直接调用 `swesmith.harness.eval`。

## 2. 环境准备

在 Worker 7143 上建立临时 venv：

```text
/tmp/uenv-swesmith-official-check/venv
/tmp/uenv-swesmith-official-check/SWE-smith
```

安装官方 eval 必需依赖：

```bash
pip install -e /tmp/uenv-swesmith-official-check/SWE-smith
pip install datasets docker swebench tqdm unidiff ghapi rich python-dotenv
```

校验：

```text
swesmith=True
swebench=True
datasets=True
docker=True
unidiff=True
```

## 3. 校验样本

使用同一 SWE-smith instance：

```text
instance_id = oauthlib__oauthlib.1fd52536.combine_file__0fceycuu
```

注意：本地 fixture `fixtures/swe/smith_smoke_sample.json` 中该 instance 的 `patch` 是缩略版，包含 `...` 行，官方 harness 无法解析：

```text
Hunk diff line expected: ...
```

因此本次正式对照使用全量 catalog 中的完整 patch：

```text
/var/lib/uenv/envs/swe-bench-smith/0.1.0-local/catalog.json
```

抽取后的单实例 dataset：

```text
/tmp/uenv-swesmith-official-check/runs/dataset_full_smoke.json
```

样本信息：

```text
patch_len = 3487
test_patch_len = 0
FAIL_TO_PASS = 13
PASS_TO_PASS = 660
```

## 4. 官方 Harness 对照结果

### 4.1 官方 gold

命令：

```bash
python -m swesmith.harness.eval \
  --dataset_path /tmp/uenv-swesmith-official-check/runs/dataset_full_smoke.json \
  --predictions_path gold \
  --run_id uenv_official_full_gold_1786074948 \
  --workers 1 \
  --redo_existing
```

结果：

```text
Resolved 1/1 instances.
```

单实例 report：

```text
resolved = true
FAIL_TO_PASS success = 13
FAIL_TO_PASS failure = 0
PASS_TO_PASS success = 660
PASS_TO_PASS failure = 0
```

官方 test output 摘要：

```text
673 passed, 2 skipped
```

### 4.2 官方 empty patch

命令：

```bash
python -m swesmith.harness.eval \
  --dataset_path /tmp/uenv-swesmith-official-check/runs/dataset_full_smoke.json \
  --predictions_path /tmp/uenv-swesmith-official-check/runs/pred_empty.json \
  --run_id uenv_official_full_empty_1786074948 \
  --workers 1 \
  --redo_existing
```

结果：

```text
Resolved 0/1 instances.
```

单实例 report：

```text
resolved = false
FAIL_TO_PASS success = 0
FAIL_TO_PASS failure = 13
PASS_TO_PASS success = 660
PASS_TO_PASS failure = 0
```

这说明官方标准本身是可工作的：gold 能过，空 patch 不能过。

## 5. UEnv Gateway 对照结果

同一完整 instance、同一完整 patch，通过 UEnv Runtime Gateway 执行。

结果文件：

```text
/tmp/uenv-swesmith-official-check/runs/uenv_gateway_polled_results.json
```

### 5.1 UEnv gold

操作：

```text
create session
write /tmp/gold.patch
git apply -R /tmp/gold.patch
pip install -e .
POST /runtime/v1/sessions/{id}/submit
GET  /runtime/v1/sessions/{id}/submit until completed
```

结果：

```text
resolved = false
reward = 0.0
tests_passed = 531
tests_total = 673
FAIL_TO_PASS 前 13 项全部 passed
PASS_TO_PASS 只有 518/660 passed
```

### 5.2 UEnv empty patch

结果：

```text
resolved = false
reward = 0.0
tests_passed = 518
tests_total = 673
FAIL_TO_PASS 0/13 passed
PASS_TO_PASS 518/660 passed
```

## 6. 关键差异

### 6.1 官方镜像与 UEnv 镜像不同

同一 instance，官方 profile 解析为：

```text
official image_name = swebench/swesmith.x86_64.oauthlib_1776_oauthlib.1fd52536
official test_cmd = source /opt/miniconda3/bin/activate; conda activate testbed; pytest --disable-warnings --color=no --tb=no --verbose
min_testing = False
timeout = 90
```

UEnv 全量 catalog 当前记录：

```text
image_cache_key = jyangballin/swesmith.x86_64.oauthlib_1776_oauthlib.1fd52536:latest
```

Worker 当前使用的是 `jyangballin/...` 镜像；官方 harness 拉取并使用的是 `swebench/...` 镜像。

这直接解释了 gold 结果差异：

| 路径 | 镜像 | gold F2P | gold P2P | resolved |
|------|------|----------|----------|----------|
| 官方 harness | `swebench/swesmith...` | 13/13 | 660/660 | true |
| UEnv Gateway | `jyangballin/swesmith...` | 13/13 | 518/660 | false |

### 6.2 UEnv 当前最终 reward 过严且环境不对齐

UEnv 的 all-pass 规则本身与官方最终 resolved 语义方向一致，但当前运行环境导致 PASS_TO_PASS 中 142 项在 gold 下仍失败。只要 P2P 未全过，UEnv 就会把 gold 判为 0。

因此目前的问题不是“是否应该要求 P2P”，而是 UEnv 目前运行的镜像 / 测试环境 / profile 与官方 harness 不一致。

### 6.3 当前 fixture 不可作为官方对照输入

`fixtures/swe/smith_smoke_sample.json` 的 patch 是缩略版，不能作为官方 harness 输入。后续 smoke fixture 必须改为完整 unified diff，或者标明它只用于 UEnv 内部轻量演示，不可用于官方 eval 对照。

## 7. 可用判分标准

### 7.1 最终 reward 标准

最终训练 reward 的权威标准应使用官方 SWE-smith harness：

```text
swesmith.harness.eval / swesmith.harness.grading.get_eval_report
```

判断标准：

```text
get_resolution_status(tests_status) == FULL
```

也就是官方 report 中：

```text
FAIL_TO_PASS failure = 0
PASS_TO_PASS failure = 0
```

### 7.2 UEnv 可接受实现方式

UEnv 可以继续本地执行，但必须满足以下条件之一：

1. **直接调用官方 harness 作为 reward adapter**  
   Worker submit 或离线 reward adapter 调用 `python -m swesmith.harness.eval` 或其内部 API，官方 report 作为最终 reward 来源。

2. **严格复刻官方 profile 环境**  
   Worker 使用官方 profile 解析出的镜像、test command、log parser 和 report 逻辑，并用官方 gold/empty 对照持续回归。

当前 UEnv 内部 parser 只能作为性能优化或 partial signal 来源，不能作为 SWE-smith 最终 resolved 的唯一权威。

### 7.3 训练用 partial reward

最终 reward 应对齐官方二值 resolved，但训练阶段可以额外引入 partial reward，例如：

- F2P success ratio
- P2P maintenance ratio
- patch applies
- diff nonempty
- timeout / context overflow penalty

这些 partial reward 必须作为训练 shaping 明确标注，不能混同官方 resolved。

## 8. 需要修复的事项

| 模块 | 必须调整 |
|------|----------|
| EnvPackage / catalog | 把 SWE-smith image namespace 与官方 profile 对齐，优先使用 `swebench/swesmith...` |
| Worker grader | 增加官方 harness reward adapter，或把当前 `SwesmithGrader` 标为非权威 fallback |
| Smoke fixture | 替换缩略 patch，保证官方 harness 可解析 |
| CI / 回归 | 固定 gold/empty 双样本：gold 必须 resolved=true，empty 必须 resolved=false |
| 训练日志 | 同时记录 official_resolved、uenv_resolved、F2P/P2P success/failure，便于发现口径漂移 |

## 9. 对当前 GRPO reward 全 0 的影响

这次对照说明，当前 GRPO reward 全 0 至少包含两层问题：

1. 模型/Agent 生成的 patch 没有 solved。
2. 更严重的是，当前 UEnv 环境下 gold patch 对该 instance 也会被判 `reward=0`，说明 reward 环境本身不可信。

因此在修复官方 harness 对齐前，当前 SWE-smith GRPO 的 reward 不能用于正常训练。
