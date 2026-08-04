# SWE-smith Worker 数据集覆盖核验说明

> 日期：2026-08-04
> 面向对象：Worker / Server / Adapter
> 关联文件：`/data/ronghao/uenv/uenv-bridge/temp/catalog.json`

## 1. 背景

在 `verl_swesmith_grpo_train_20260804_102356` 训练中，大量 episode 返回：

```text
swe instance_id ... not in catalog (size=736)
```

为核验 Worker 当前实际加载的 SWE-smith 数据范围，已将 Worker 机器上的 catalog 文件复制到本地：

```text
Worker 原路径：
/var/lib/uenv/envs/swe-bench-smith/0.1.0-local/catalog.json

本地核验路径：
/data/ronghao/uenv/uenv-bridge/temp/catalog.json
```

## 2. 核验结论

`/data/ronghao/uenv/uenv-bridge/temp/catalog.json` 中只有 5 条 SWE-smith instance：

```text
oauthlib__oauthlib.1fd52536.combine_file__09vlzwgc
oauthlib__oauthlib.1fd52536.combine_file__0fceycuu
oauthlib__oauthlib.1fd52536.combine_file__0fukhdzk
oauthlib__oauthlib.1fd52536.combine_file__0hkl0pea
oauthlib__oauthlib.1fd52536.combine_file__0mvyid7d
```

该文件与本地 smoke catalog 一致：

```text
/data/ronghao/uenv/config/swe/smith-smoke.json
```

因此当前 Worker 上的 SWE-smith EnvPackage 更接近 smoke 数据包，而不是全量 SWE-smith 训练集。

报错中的 `catalog (size=736)` 不应理解为 SWE-smith catalog 有 736 条。更可能的情况是 Worker 将 SWE-bench Pro catalog 与 SWE-smith smoke catalog 合并后得到总 catalog size，其中 SWE-smith 部分只有 5 条。

## 3. 有效训练样本

将上述 5 条 catalog instance 与本地 SWE-smith raw 数据做交集：

```text
/data/ronghao/uenv/uenv-bridge/data/benchmarks/swesmith/raw/data/
```

核验结果：

| 项目 | 数量 |
|---|---:|
| Worker SWE-smith catalog instance | 5 |
| raw 数据中可找到的 catalog instance | 5 |
| `problem_statement` 非空的有效训练样本 | 4 |
| `problem_statement` 为空、跳过 | 1 |

有效训练样本为：

```text
oauthlib__oauthlib.1fd52536.combine_file__09vlzwgc
oauthlib__oauthlib.1fd52536.combine_file__0fceycuu
oauthlib__oauthlib.1fd52536.combine_file__0hkl0pea
oauthlib__oauthlib.1fd52536.combine_file__0mvyid7d
```

跳过样本为：

```text
oauthlib__oauthlib.1fd52536.combine_file__0fukhdzk
reason = empty_problem_statement
```

这里的“有效训练样本 4 条”指 4 个可作为 VeRL / UEnv 训练 episode 的 `instance_id`，不是 4 个 Docker 镜像。

这些样本大概率共用同一个镜像：

```text
jyangballin/swesmith.x86_64.oauthlib_1776_oauthlib.1fd52536:latest
```

SWE-smith 中一个镜像可以支持多个样本；样本之间由 `instance_id`、`problem_statement`、`FAIL_TO_PASS`、`PASS_TO_PASS`、patch / test 信息区分。

## 4. 已生成 smoke 交集数据集

基于 Worker catalog 与本地 raw 数据交集，已生成一个只包含可运行样本的 smoke 数据集：

```text
/data/ronghao/uenv/uenv-bridge/data/benchmarks/swesmith_train_smoke_catalog_intersection/
```

目录内容：

```text
train.parquet
test.parquet
source_rows.jsonl
dataset_summary.json
```

数据规模：

```text
train.parquet: 4 条
test.parquet: 4 条
```

该数据集可用于验证当前 Worker 侧 smoke catalog 下的 SWE-smith 训练链路，避免再次大量触发 `not in catalog`。

## 5. 后续建议

当前 Worker 侧如果只加载上述 5 条 SWE-smith smoke catalog，则 Adapter 侧正式训练数据必须先按 catalog 过滤，否则大部分样本会在 Worker 查表阶段失败。

后续有两种可选路线：

1. Worker 侧提供并加载全量 SWE-smith catalog，使其覆盖正式训练数据。
2. Adapter 数据准备阶段显式传入 Worker catalog，只生成 catalog 覆盖范围内的训练样本。

在全量 catalog 未对齐前，应只使用 `swesmith_train_smoke_catalog_intersection` 做链路 smoke，不应把它当作正式 SWE-smith 训练数据集。
