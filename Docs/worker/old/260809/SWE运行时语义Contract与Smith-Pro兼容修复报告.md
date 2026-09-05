# SWE 运行时语义 Contract 与 Smith/Pro 兼容修复报告

记录时间：2026-08-09

## 背景

SWE-smith GRPO 联调暴露出一个框架层缺口：Docker 镜像和 catalog 已经能封装 repo、依赖和测试环境，但 Worker 仍把“任务语义”写在代码分支里。SWE-bench Pro 与 SWE-smith 的 patch / workspace / reward 语义不同，之前缺少统一协议描述这些差异，导致 Smith 训练出现初始环境不是 buggy、gold patch 方向不一致、reward 全 0 等问题。

本次调整把这些语义提升为 `BenchmarkRuntimeContract`，由 catalog 或 EnvPackage `worker_overlay.swe.benchmark_runtime` 声明，Worker 按 contract 执行生命周期。

## 修复的核心缺口

### 1. 初始 workspace 状态缺少协议

旧逻辑：

- Worker 只按 `benchmark_variant` 做零散特判。
- SWE-smith 的“数据集 patch 是 clean -> buggy”没有成为环境协议。
- session reset 后如果未注入 bug patch，agent 看到的是 clean/fixed 状态。

新逻辑：

- 新增 `uenv-worker/src/swe/runtime_contract.rs`。
- contract 声明：

```json
{
  "initial_state": {
    "patch_semantics": "clean_to_buggy",
    "provision_patch": "apply_dataset_patch",
    "commit_after_provision": true
  }
}
```

- Worker provision 后统一调用 `apply_initial_state_contract()`。
- Smith 默认 contract 会正向应用数据集 patch，并提交 baseline，使 agent diff 表示 `buggy -> fixed`。
- recycle/reset 复用 session 时也重新应用 initial-state contract，避免复用路径丢失 bug baseline。

### 2. Gold patch 方向缺少统一入口

旧逻辑：

- `harness.rs` 和 `instance_pool.rs` 曾分别判断 Smith，路径不一致时容易一个正向 apply、一个反向 apply。

新逻辑：

- contract 声明：

```json
{
  "gold": { "patch_mode": "reverse_dataset_patch" }
}
```

- `SweSession::apply_gold_contract()` 是唯一 gold patch 入口。
- SWE-bench Pro 默认 `apply_dataset_patch`。
- SWE-smith 默认 `reverse_dataset_patch`。

### 3. Reward adapter 缺少可扩展协议

旧逻辑：

- Pro / Smith 的外部 reward adapter 由代码硬编码 env var。
- 新环境如果需要官方 harness，通常还要继续改 Worker 分支。

新逻辑：

- contract 声明：

```json
{
  "reward": {
    "adapter": "external_command",
    "command_env": "UENV_CUSTOM_REWARD_CMD",
    "authority": "custom_official"
  }
}
```

- 新增 `uenv-worker/src/swe/contract_eval.rs` 通用 external reward adapter。
- Worker 会向外部命令提供稳定输入：

```text
UENV_SWE_INSTANCE_ID
UENV_SWE_BENCHMARK_VARIANT
UENV_SWE_WORKSPACE_DIR
UENV_SWE_INSTANCE_JSON
UENV_SWE_MODEL_PATCH
UENV_SWE_TEST_OUTPUT_PATH
UENV_SWE_TEST_OUTPUT
```

- 外部命令 stdout 返回统一 JSON：

```json
{
  "resolved": true,
  "reward": 1.0,
  "per_test": [["test_id", true]]
}
```

- Pro / Smith 仍保留专用 adapter，但 env var 名也改由 contract 默认提供：
  - Pro: `UENV_SWE_PRO_EVAL_CMD`
  - Smith: `UENV_SWE_SMITH_EVAL_CMD`

## 默认兼容策略

### SWE-bench Verified / Lite

```json
{
  "workspace_dir": "/testbed",
  "initial_state": {
    "patch_semantics": "bug_to_fix",
    "provision_patch": "none",
    "commit_after_provision": false
  },
  "gold": { "patch_mode": "apply_dataset_patch" },
  "reward": { "adapter": "internal_pytest" }
}
```

### SWE-bench Pro

```json
{
  "workspace_dir": "/app",
  "initial_state": {
    "patch_semantics": "bug_to_fix",
    "provision_patch": "none",
    "commit_after_provision": false
  },
  "gold": { "patch_mode": "apply_dataset_patch" },
  "reward": {
    "adapter": "internal_with_external_override",
    "command_env": "UENV_SWE_PRO_EVAL_CMD"
  }
}
```

### SWE-smith

```json
{
  "workspace_dir": "/testbed",
  "initial_state": {
    "patch_semantics": "clean_to_buggy",
    "provision_patch": "apply_dataset_patch",
    "commit_after_provision": true
  },
  "gold": { "patch_mode": "reverse_dataset_patch" },
  "reward": {
    "adapter": "internal_with_external_override",
    "command_env": "UENV_SWE_SMITH_EVAL_CMD",
    "authority": "official_swesmith"
  }
}
```

## 新环境接入方式

后续新的 SWE 类仿真环境如果符合已有默认语义，只需要配置 `benchmark_variant` / EnvPackage / catalog。

如果新环境有特殊语义，应优先在 catalog 单实例或 EnvPackage manifest 中声明 runtime contract，而不是改 Worker 核心代码：

```json
{
  "worker_overlay": {
    "swe": {
      "benchmark_variant": "verified",
      "benchmark_runtime": {
        "kind": "swe",
        "workspace_dir": "/workspace",
        "initial_state": {
          "patch_semantics": "clean_to_buggy",
          "provision_patch": "apply_dataset_patch",
          "commit_after_provision": true
        },
        "gold": { "patch_mode": "reverse_dataset_patch" },
        "reward": {
          "adapter": "external_command",
          "command_env": "UENV_CUSTOM_REWARD_CMD",
          "authority": "custom_official"
        }
      }
    }
  }
}
```

注意：当前 `benchmark_variant` registry 仍是 `verified/lite/pro/smith` 固定枚举。新的环境若只是 workspace、patch 方向、gold 方向、reward adapter 不同，可先通过现有 variant + runtime contract 覆盖接入；若需要新的 grader 名、镜像命名空间规则、catalog 路由名，则仍需要扩展 variant registry。这是后续可继续泛化的边界。

这样 Worker 生命周期自动适配：

```text
create session
  -> reset workspace
  -> apply initial_state.provision_patch
  -> optional baseline commit
  -> agent produces git diff from declared baseline
  -> apply test_patch
  -> run tests
  -> run reward adapter declared by contract
  -> save trajectory/artifacts
```

## Adapter 发起训练的注意事项

后续从 adapter 层发起 SWE-bench Pro / SWE-smith 训练时，需要跟随本 contract 协议调整训练样本、EnvPackage 和部署配置，但不应在 adapter 中继续复制 Worker 生命周期特判。

### 1. 训练样本和 EnvPackage 必须能触发正确 contract

adapter 生成训练 parquet / episode payload 时，应保证以下字段与目标环境一致：

- `benchmark_variant`
- `env_package_id` / `env_package_version`
- `dataset` / `data_source`
- `instance_id`
- EnvPackage manifest 中的 `worker_overlay.swe.benchmark_runtime`（如使用包级覆盖）
- catalog 单实例中的 `runtime_contract` / `benchmark_runtime`（如使用实例级覆盖）

对 SWE-smith，如果不显式写 `benchmark_runtime`，也必须保证 `benchmark_variant=smith`，这样 Worker 会使用 Smith 默认 contract：

- `initial_state.patch_semantics=clean_to_buggy`
- `initial_state.provision_patch=apply_dataset_patch`
- `initial_state.commit_after_provision=true`
- `gold.patch_mode=reverse_dataset_patch`
- `reward.command_env=UENV_SWE_SMITH_EVAL_CMD`

对 SWE-bench Pro，默认 contract 依赖 `benchmark_variant=pro`：

- `workspace_dir=/app`
- `initial_state.provision_patch=none`
- `gold.patch_mode=apply_dataset_patch`
- `reward.command_env=UENV_SWE_PRO_EVAL_CMD`

### 2. adapter 不再承担 patch 方向和 buggy 状态特判

adapter / bridge 侧不应再手写如下逻辑：

- Smith 正向 apply 数据集 patch 造 buggy 状态。
- Smith gold patch 反向 apply。
- Pro / Smith workspace 目录的临时分支判断。
- 根据 Pro / Smith 在 adapter 内选择 reward patch 方向。

这些语义已由 Worker 的 `BenchmarkRuntimeContract` 统一驱动。adapter 只负责把足够的语义字段传给 Worker；如果 adapter 再做一层 patch 方向转换，容易造成双重 apply / reverse apply，重新引入 reward 全 0 或初始环境非 buggy 的问题。

### 3. 官方 reward adapter 仍需要部署环境变量

contract 只声明外部 reward adapter 的入口名，真正命令仍由 Worker 运行环境提供：

- SWE-bench Pro 默认读取 `UENV_SWE_PRO_EVAL_CMD`
- SWE-smith 默认读取 `UENV_SWE_SMITH_EVAL_CMD`
- 自定义环境读取 contract 中的 `reward.command_env`

因此 adapter 发起训练前，部署脚本 / Worker service 环境需要确认相应 env var 已注入。若 EnvPackage 或 catalog 使用自定义 `reward.command_env`，adapter 不需要理解命令内容，但训练部署必须提供同名环境变量。

### 4. 联调检查清单

从 adapter 侧重新发起训练前，建议至少检查：

- Worker 日志中加载 EnvPackage 时 `runtime_contract=true`，或实例按 `benchmark_variant` 派生默认 contract。
- 首个 Smith session provision 后存在 baseline commit，agent diff 是 `buggy -> fixed`。
- gold 验证统一走 `SweSession::apply_gold_contract()`。
- `agent-loop-results.jsonl` 中 trajectory / reward 字段正常落盘。
- 如果使用官方 harness，外部 reward 命令 stdout 返回 `{resolved, reward, per_test}` JSON。

## 修改范围

- `uenv-worker/src/swe/runtime_contract.rs`
  - 新增 runtime contract 数据结构和 Verified/Pro/Smith 默认语义。
- `uenv-worker/src/swe/contract_eval.rs`
  - 新增通用 external reward command 协议。
- `uenv-worker/src/swe/dataset.rs`
  - `SweInstance` 支持 `runtime_contract` / `benchmark_runtime` 字段。
  - `workspace_dir()` 优先读 contract。
  - `InstanceStore` 支持注入 EnvPackage 默认 contract。
- `uenv-worker/src/swe/env_package.rs`
  - 读取 `worker_overlay.swe.benchmark_runtime` / `runtime_contract`。
- `uenv-worker/src/runtime.rs`
  - 加载 EnvPackage catalog 时将包级 contract 注入实例。
- `uenv-worker/src/swe/session.rs`
  - provision / recycle 按 contract 建立初始状态。
  - reward 选择按 contract 驱动。
- `uenv-worker/src/swe/harness.rs`
  - native gold 改为 `apply_gold_contract()`。
- `uenv-worker/src/swe/instance_pool.rs`
  - Gateway/native episode gold 改为 `apply_gold_contract()`。
- `uenv-worker/src/swe/pro_eval.rs`
  - Pro 外部 adapter 支持由 contract 指定 env var 名。
- `uenv-worker/src/swe/smith_eval.rs`
  - Smith 外部 adapter 支持由 contract 指定 env var 名。

## 验证

本地用临时 `grpcio-tools` protoc wrapper 跑过 Worker lib 测试：

```text
PROTOC=/tmp/uenv-protoc-wrapper \
PROTOC_INCLUDE=/tmp/uenv-grpc-tools/grpc_tools/_proto \
cargo test -p uenv-worker --lib

result: ok. 144 passed; 0 failed
```

说明：

- macOS 本机没有系统 `protoc`。
- 使用 `/tmp` 下临时 Python target 安装 `grpcio-tools`，未修改仓库文件或系统全局环境。

## 结论

本次修复的不是某个 Docker 镜像问题，而是 Worker 对 SWE 类环境的“任务语义契约”缺失问题。调整后，SWE-bench Pro 与 SWE-smith 的 buggy 状态、gold patch 方向、reward adapter 不再依赖散落的 Smith/Pro 特判，而由 runtime contract 统一驱动。后续新 SWE 类仿真环境只要能用该协议表达其 patch、workspace 和 reward 语义，原则上不需要再改 Worker 核心生命周期代码。
