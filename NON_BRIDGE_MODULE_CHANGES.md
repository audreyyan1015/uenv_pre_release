# 非 uenv-bridge 模块改动说明

日期：2026-08-17

本文档说明当前 `feature/verl-bridge-adapter` 分支中，除 `uenv-bridge` 之外的两个本地提交。它们已经按类型独立提交，便于后续 review、cherry-pick 或回退。

## 提交概览

当前非 `uenv-bridge` 改动对应两个 commit：

| Commit | 类型 | 涉及模块 | 主要作用 |
| --- | --- | --- | --- |
| `de449ea` `chore(proto): allow proto3 optional codegen` | 构建兼容 | `uenv-server`, `uenv-worker` | 让当前环境的 `protoc 3.12.4` 可以编译包含 `proto3 optional` 字段的 proto |
| `944d389` `test(server): update worker status obs fixtures` | 测试适配 | `uenv-server` | 更新 obs 测试 fixture，适配 `WorkerSnapshot` / `WorkerStatusObservation` 当前字段 |

## `de449ea`：proto3 optional codegen 兼容

修改文件：

- `uenv-server/build.rs`
- `uenv-worker/build.rs`

改动内容：

```rust
.protoc_arg("--experimental_allow_proto3_optional")
```

### 背景

当前环境里的 `protoc` 版本是：

```text
libprotoc 3.12.4
```

仓库内 proto 已经使用 `proto3 optional` 字段，例如：

- `proto/uenv/v1/episode.proto`
- `proto/uenv/v1/agent.proto`

在该版本 `protoc` 下，如果不传 `--experimental_allow_proto3_optional`，直接编译会失败：

```text
This file contains proto3 optional fields, but --experimental_allow_proto3_optional was not set.
```

### 影响范围

这是构建期改动，只影响 Rust protobuf/gRPC 代码生成：

- 不改变 server/worker 的运行时业务逻辑。
- 不改变 proto 文件本身。
- 不改变生成代码的提交策略；仓库仍由 `build.rs` 在构建时生成。

### 是否必要

对当前环境是必要的。若目标机器仍使用 `protoc 3.12.x` 这类需要显式开启 proto3 optional 的版本，缺少该提交会导致 `cargo build` / `cargo test` 在 proto codegen 阶段失败。

如果未来统一升级到较新的 `protoc`，该参数通常仍可保留，主要是兼容旧环境。

## `944d389`：server obs 测试 fixture 适配

修改文件：

- `uenv-server/src/obs/tests.rs`
- `uenv-server/src/obs/worker_status.rs`

### 背景

`uenv-server` 当前的 worker/obs 数据结构已经包含更多 worker 运行时字段，例如：

- `platform_features`
- `backend_kinds`
- `trajectory_schemas`
- `tool_schemas`
- `package_states`
- `pool_summary`
- `pool_slots`
- `active_episodes`

测试代码里的手写 fixture 还停留在旧字段集合上。直接跑 `cargo test --workspace --lib --bins` 时，会遇到两类问题：

1. `WorkerSnapshot` 测试构造缺少新增字段，导致测试编译不完整。
2. `global_worker_snapshot_is_overlaid_on_run_state` 断言 busy worker 包含 `ep-worker-status`，但测试输入没有显式给 worker snapshot 填 `active_episodes`。

### 改动内容

`uenv-server/src/obs/worker_status.rs`：

- 在测试 helper 构造 `WorkerSnapshot` 时补齐新增字段，统一使用空 `Vec`。

`uenv-server/src/obs/tests.rs`：

- 测试 helper 增加 `active_episodes` 参数。
- busy worker fixture 显式填入 `ep-worker-status`，与后续断言保持一致。

### 影响范围

这是测试代码改动：

- 不改变 server 运行时逻辑。
- 不改变 worker 调度逻辑。
- 不改变 obs 数据合并逻辑。
- 只让现有测试 fixture 与当前结构体字段和断言语义保持一致。

### 是否必要

对运行时不是必要的；对当前分支完整通过本地 Rust 测试是必要的。

如果 PR 或合入策略严格要求本轮只包含 `uenv-bridge` 目录内代码，可以把该提交单独拿出来处理。但如果要求当前分支在当前环境下能完整通过：

```bash
cargo test --workspace --lib --bins
```

则建议保留该提交，或在 server 侧以单独 PR/commit 合入。

## 与 uenv-bridge 改动的关系

这两个提交都不是 `uenv-bridge` 功能的一部分：

- `de449ea` 是跨 crate 的构建兼容修复。
- `944d389` 是 server 测试 fixture 修复。

它们被单独提交的原因是避免与后续 `uenv-bridge` 相关提交混在一起。当前分支后续的 bridge 提交包括：

- `dfef686` `fix(bridge): align agent loop with VeRL runtime`
- `423e19d` `fix(bridge): support updated VeRL patch points`
- `3d85c2f` `feat(bridge): add native Ascend GRPO runner`
- `ad791c0` `test(bridge): refresh pre-rollout verifier`

## 建议处理方式

建议保留 `de449ea`，因为它解决当前环境下 proto codegen 的实际构建问题。

`944d389` 可以根据合入策略选择：

- 若目标是当前分支整体测试通过：保留。
- 若目标是严格 bridge-only PR：将它拆到单独的 server 测试修复 PR，或在提交历史中从本次 bridge 改动序列里移出。

