# SWE-smith 官方镜像与 Reward 对齐修复记录

记录时间：2026-08-07 13:34 IST

## 背景

此前 UEnv SWE-smith 训练链路使用 `jyangballin/swesmith...` 镜像命名空间。官方 harness 对照校验显示，同一 `oauthlib__oauthlib.1fd52536.combine_file__0fceycuu` 实例在官方 `swebench/swesmith...` 镜像中 gold patch 可 `resolved=true`，但 UEnv 当前环境中 gold patch 仍被判 `reward=0.0`。这说明 reward 全 0 不只是模型没有解题，也包含镜像 / 判分环境未对齐问题。

## 本次代码调整

### Worker 数据集入口

文件：`uenv-worker/src/swe/dataset.rs`

- 新增官方 SWE-smith 镜像前缀：`swebench/swesmith.`
- 保留历史前缀识别：`jyangballin/swesmith.`
- `SweInstance::image_ref()` 对 `benchmark_variant=smith` 强制归一化：
  - 旧 `jyangballin/swesmith...` 自动转换为 `swebench/swesmith...`
  - 缺失 `image_cache_key` 时按 `instance_id` 派生官方镜像
  - 缺失 tag 时补齐 `:latest`
- `image_namespace_consistent()` 对 Smith 收紧为必须以 `swebench/swesmith.` 开头。

### SWE-smith catalog 导出

文件：`scripts/export_swe_smith_instances.py`

- 导出 EnvPackage/catalog 时统一输出官方 `swebench/swesmith...` 镜像。
- 兼容旧 parquet / cache 中的 `jyangballin/swesmith...` 字段，导出时自动改写。
- 当输入缺失镜像字段时，按 SWE-smith 官方 profile 命名规则从 `instance_id` 派生。

### VeRL 训练数据准备

文件：`uenv-bridge/scripts/utils/prepare_verl_swesmith_train.py`

- `extra_info.dockerhub_tag` 与 `extra_info.image_cache_key` 统一使用官方镜像。
- 兼容旧数据源字段，避免新生成的 GRPO parquet 继续写入旧镜像。

### Smoke 配置与 fixture

文件：

- `config/swe/smith-smoke.json`
- `fixtures/swe/smith_smoke_sample.json`

已将显式镜像引用从 `jyangballin/swesmith...` 更新为 `swebench/swesmith...`。

### 官方 harness reward adapter

文件：

- `uenv-worker/src/swe/smith_eval.rs`
- `uenv-worker/src/swe/session.rs`
- `scripts/eval_swesmith_official_reward.py`
- `scripts/restart-worker-gateway-28097-7143.sh`

新增 `UENV_SWE_SMITH_EVAL_CMD`：

```bash
export UENV_SWESMITH_REPO=/tmp/uenv-swesmith-official-check/SWE-smith
export UENV_SWE_SMITH_EVAL_CMD="/tmp/uenv-swesmith-official-check/venv/bin/python /root/UEnv/scripts/eval_swesmith_official_reward.py"
```

Worker submit 会在执行安装、test patch 和内部测试之前，先捕获当前 `/testbed` 的 `git diff` 作为 agent/model patch。内部测试仍会执行并保存诊断输出；Smith 实例随后把预先捕获的 model patch 交给官方 SWE-smith harness 单实例评测，并用官方 report 的 `ids_resolved` 决定最终 `reward`。

不能在内部测试之后再取 `git diff`：`pip install -e .`、测试缓存或运行期文件可能污染工作区，导致传给官方 harness 的 patch 不是 agent 实际产物。

官方 harness adapter 当前只返回实例级 `resolved`，不返回每个 pytest node 的状态。Worker 因此保留内部 pytest parser 的 `per_test` 统计用于诊断和前端展示，但最终 `resolved/reward` 以官方 harness 为准。

未配置该环境变量时，Worker 回退到内部 pytest 日志 parser。

### Gold patch 方向修正

文件：

- `uenv-worker/src/swe/harness.rs`
- `scripts/swe_gateway_demo.py`

SWE-smith 官方 harness 对照显示，`patch` 应作为预测补丁正向传入 `model_patch`。此前 UEnv 对 Smith gold 默认 `git apply -R`，与官方 gold 对照方向相反。本次改为正向应用。

### 移除 Worker provision 阶段的 bug patch 预注入

文件：`uenv-worker/src/swe/session.rs`

此前 Worker 创建 Smith session 时会先把数据集 `patch` 应用到容器中，把官方“预测补丁”变成环境基线。这使得：

- fresh session 的初始 `git diff` 不为空；
- empty submit 可能把基线 bug patch 当作 model patch 提交给官方 harness；
- UEnv 的任务语义变成“在已注入 bug 的环境里修复”，不再是官方 SWE-smith 的 clean base + `model_patch` 评测语义。

本次移除该预注入逻辑。Smith session provision 后保持官方镜像/仓库基线，agent/model 的实际修改才会作为 `model_patch` 交给官方 harness。

## Reward 对齐状态

本次修复完成两层对齐：

1. 镜像引用统一到官方 `swebench/swesmith...`。
2. 生产部署可通过 `UENV_SWE_SMITH_EVAL_CMD` 使用官方 SWE-smith harness 作为最终 reward。

当前 `SwesmithGrader` 仍保留为 Worker 内部日志解析器，语义是以 FAIL_TO_PASS / PASS_TO_PASS 全通过作为 `resolved=true`。它只能作为 fallback / 诊断信号；启用外部 adapter 后，最终 reward 以官方 SWE-smith harness 为准：

```text
python -m swesmith.harness.eval
swesmith.harness.grading.get_eval_report
```

注意：官方 harness adapter 会重新运行单实例官方评测，成本高于内部 parser。训练吞吐优化可以后续通过缓存、异步 reward 或 partial reward 解决，但最终二值 resolved 不应再由内部 parser 独自决定。

## 验收标准

1. Worker 创建 SWE-smith session 时实际使用 `swebench/swesmith...` 镜像。
2. 旧 catalog 中的 `jyangballin/swesmith...` 不再导致 Worker 拉取旧镜像。
3. 新导出的 catalog / VeRL parquet 不再写入旧镜像。
4. 对照实例 gold patch 在 UEnv Gateway 中应得到 `resolved=true` / `reward=1.0`；empty patch 应保持 `resolved=false` / `reward=0.0`。
