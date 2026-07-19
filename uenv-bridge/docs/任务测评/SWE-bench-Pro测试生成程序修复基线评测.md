# SWE-bench-Pro 测试生成/程序修复基线评测

> 日期：2026-07-12  
> 阶段：Eval-first，未进行后训练  
> 任务书条目：4. 测试生成/程序修复  
> Benchmark：SWE-bench-Pro public test split  
> 目标模型：`Qwen/Qwen3.6-35B-A3B`

## 1. 任务说明

SWE-bench-Pro 是长程软件工程任务评测。每条样本给定一个真实代码仓库、base commit、issue 描述、需求说明、接口信息和测试集合，模型需要生成一个 git unified diff patch。官方评测会把 patch 应用到对应实例镜像中，运行 fail-to-pass 与 pass-to-pass 测试，最终统计 resolved / resolve rate。

本阶段不进行后训练，只验证基准模型在 SWE-bench-Pro 上的 patch 生成链路，并尝试接通官方 local Docker evaluator。

## 2. 数据集

数据来源为 Hugging Face：

```text
ScaleAI/SWE-bench_Pro
split: test
```

本地数据已落地：

```text
data/benchmarks/swebenchpro/test.jsonl
data/benchmarks/swebenchpro/swe_bench_pro_full.csv
data/benchmarks/swebenchpro/dataset_summary.json
```

样本总数：731。

仓库分布：

| repo | 样本数 |
|---|---:|
| NodeBB/NodeBB | 44 |
| qutebrowser/qutebrowser | 79 |
| ansible/ansible | 96 |
| internetarchive/openlibrary | 91 |
| gravitational/teleport | 76 |
| navidrome/navidrome | 57 |
| element-hq/element-web | 56 |
| future-architect/vuls | 62 |
| protonmail/webclients | 65 |
| flipt-io/flipt | 85 |
| tutao/tutanota | 20 |

语言分布：

| 语言 | 样本数 |
|---|---:|
| Python | 266 |
| Go | 280 |
| JavaScript | 165 |
| TypeScript | 20 |

主要字段：

| 字段 | 说明 |
|---|---|
| `instance_id` | SWE-bench-Pro 实例 ID。 |
| `repo` | GitHub 仓库名。 |
| `base_commit` | 需要应用 patch 的基准 commit。 |
| `problem_statement` | issue 描述。 |
| `requirements` | 修复需要满足的需求。 |
| `interface` | 涉及的接口、路径和输入输出说明。 |
| `patch` | 官方 gold patch。 |
| `test_patch` | 官方测试补丁。 |
| `fail_to_pass` | 修复后必须通过的测试。 |
| `pass_to_pass` | 修复后不能回归的测试。 |
| `dockerhub_tag` | 官方实例镜像 tag。 |

## 3. 评价指标

官方主指标为 resolve rate：

```text
resolve_rate = resolved_count / evaluated_count
```

一条样本被判定为 resolved，需要满足：

1. 生成 patch 可以应用到实例仓库。
2. `fail_to_pass` 测试全部通过。
3. `pass_to_pass` 测试全部保持通过。

在官方 Docker evaluator 接通前，本地脚本只统计 patch 生成阶段的格式指标：

| 指标 | 说明 |
|---|---|
| `nonempty_patch_rate` | 能否从模型输出中抽取到非空 patch。 |
| `diff_git_patch_rate` | patch 是否包含 `diff --git` 文件 diff 头。 |
| `hunk_patch_rate` | patch 是否包含 `@@` hunk。 |
| `output_tokens_*` | 输出 token 数统计。 |

这些格式指标只能说明模型输出是否像 patch，不等同于官方 resolved 分数。

## 4. 评测实现

新增脚本：

```text
scripts/benchmark/evaluate_swebenchpro.py
scripts/benchmark/run_swebenchpro_baseline.sh
```

脚本分为四个阶段：

| 阶段 | 命令 | 说明 |
|---|---|---|
| 数据准备 | `prepare` | 从 `ScaleAI/SWE-bench_Pro` 下载 test split，保存 JSONL/CSV。 |
| Patch 生成 | `generate` | 使用 vLLM 加载 `Qwen/Qwen3.6-35B-A3B`，生成 patch。 |
| 官方资产下载 | `download-official-assets` | 从官方仓库下载每个实例的 `run_script.sh`、`parser.py`、Dockerfile。 |
| 结果汇总 | `summarize` | 汇总生成格式指标；若已有官方 evaluator 输出，则汇总 resolve rate。 |

官方 evaluator 文件已放在：

```text
/data/ronghao/third_party/SWE-bench_Pro-os
```

说明：官方 local Docker evaluator 需要拉取 `jefzda/sweap-images:<dockerhub_tag>` 实例镜像。当前本机 DockerHub 镜像访问存在阻塞，因此还没有得到有效的官方 resolved 分数。

## 5. 全量基线配置

| 配置 | 值 |
|---|---|
| 评测口径 | 直接 vLLM 生成 patch + 官方 local Docker evaluator 分批按需评测 |
| 模型 | `Qwen/Qwen3.6-35B-A3B` |
| 生成镜像 `GEN_IMAGE` | `localhost/vllm-openai:v0.19.0-cu130` |
| 评测镜像 `EVAL_IMAGE` | `localhost/uenv-bridge-verl:layer4-build` |
| 模型目录 `MODEL_DIR` | `/data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B` |
| GPU | 8 张 A100 |
| Tensor parallel | 8 |
| `MAX_MODEL_LEN` | 16384 |
| `MAX_TOKENS` | 4096 |
| `TEMPERATURE` | 0.2 |
| `TOP_P` | 1.0 |
| Thinking mode | 关闭，`DISABLE_THINKING=1` |
| 数据集 | SWE-bench-Pro public test split，全量 731 条 |
| 输出目录 | `temp/benchmarks/swebenchpro/qwen3_6_35b_a3b_full/` |
| 官方 evaluator 资产 | `/data/ronghao/third_party/SWE-bench_Pro-os` |
| 主镜像源 | `docker.1panel.live/jefzda` |
| 重试镜像源 | `hub.rat.dev/jefzda` |
| 分批大小 | 首轮 `BATCH_SIZE=10`，失败重试 `BATCH_SIZE=5` |
| evaluator 并发 | `OFFICIAL_NUM_WORKERS=1` |
| 镜像清理 | `CLEAN_IMAGES_AFTER_BATCH=1` |
| 后训练 | 未进行 SFT/RL，Eval-first 基线 |

## 6. 运行命令

### 6.1 数据准备

```bash
cd /data/ronghao/uenv/uenv-bridge

RUN_GENERATE=0 \
RUN_SUMMARIZE=0 \
./scripts/benchmark/run_swebenchpro_baseline.sh
```

### 6.2 生成 smoke

```bash
cd /data/ronghao/uenv/uenv-bridge

RUN_PREPARE=0 \
RUN_OFFICIAL_EVALUATE=0 \
LIMIT=2 \
OUTPUT_DIR=/data/ronghao/uenv/uenv-bridge/temp/benchmarks/swebenchpro/qwen3_6_35b_a3b_smoke_limit2 \
./scripts/benchmark/run_swebenchpro_baseline.sh
```

关键参数：

| 参数 | 值 |
|---|---|
| `GEN_IMAGE` | `localhost/vllm-openai:v0.19.0-cu130` |
| `EVAL_IMAGE` | `localhost/uenv-bridge-verl:layer4-build` |
| `MODEL_DIR` | `/data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B` |
| `TENSOR_PARALLEL_SIZE` | `8` |
| `MAX_MODEL_LEN` | `16384` |
| `MAX_TOKENS` | `4096` |
| `TEMPERATURE` | `0.2` |
| `TOP_P` | `1.0` |
| `DISABLE_THINKING` | `1` |

### 6.3 全量 patch 生成命令

该命令只生成 patch 和格式指标，不运行官方 Docker evaluator：

```bash
cd /data/ronghao/uenv/uenv-bridge

nohup env RUN_PREPARE=0 \
RUN_OFFICIAL_EVALUATE=0 \
LIMIT= \
OUTPUT_DIR=/data/ronghao/uenv/uenv-bridge/temp/benchmarks/swebenchpro/qwen3_6_35b_a3b_full \
TENSOR_PARALLEL_SIZE=8 \
MAX_MODEL_LEN=16384 \
MAX_TOKENS=4096 \
./scripts/benchmark/run_swebenchpro_baseline.sh \
> /data/ronghao/uenv/uenv-bridge/temp/benchmarks/swebenchpro/qwen3_6_35b_a3b_full.log 2>&1 &
```

### 6.4 官方 evaluator 分批按需评测

SWE-bench-Pro 官方 evaluator 需要按样本拉取 `jefzda/sweap-images:<dockerhub_tag>` 实例镜像。731 个镜像全部预拉会占用 TB 级 Docker root 空间，因此当前采用“分批评测、按需拉取、每批结束清理镜像”的方式。

新增脚本：

```text
scripts/benchmark/run_swebenchpro_official_batches.py
scripts/benchmark/run_swebenchpro_official_batches.sh
```

首轮建议使用 `docker.1panel.live/jefzda`：

```bash
cd /data/ronghao/uenv/uenv-bridge

BATCH_ROOT=/data/ronghao/uenv/uenv-bridge/temp/benchmarks/swebenchpro/qwen3_6_35b_a3b_full/official_eval_batches_1panel \
DOCKERHUB_USERNAME=docker.1panel.live/jefzda \
BATCH_SIZE=10 \
OFFICIAL_NUM_WORKERS=1 \
CLEAN_IMAGES_AFTER_BATCH=1 \
OUTPUT_DIR=/data/ronghao/uenv/uenv-bridge/temp/benchmarks/swebenchpro/qwen3_6_35b_a3b_full \
./scripts/benchmark/run_swebenchpro_official_batches.sh
```

如果首轮出现镜像拉取失败，脚本会输出：

```text
official_eval_batches_1panel/pull_failed_instance_ids.txt
```

第二轮只重跑镜像失败样本，并换用 `hub.rat.dev/jefzda`：

```bash
cd /data/ronghao/uenv/uenv-bridge

BATCH_ROOT=/data/ronghao/uenv/uenv-bridge/temp/benchmarks/swebenchpro/qwen3_6_35b_a3b_full/official_eval_batches_hubrat_retry \
EXTRA_MERGE_ROOTS=/data/ronghao/uenv/uenv-bridge/temp/benchmarks/swebenchpro/qwen3_6_35b_a3b_full/official_eval_batches_1panel \
INSTANCE_ID_FILE=/data/ronghao/uenv/uenv-bridge/temp/benchmarks/swebenchpro/qwen3_6_35b_a3b_full/official_eval_batches_1panel/pull_failed_instance_ids.txt \
DOCKERHUB_USERNAME=hub.rat.dev/jefzda \
BATCH_SIZE=5 \
OFFICIAL_NUM_WORKERS=1 \
CLEAN_IMAGES_AFTER_BATCH=1 \
OUTPUT_DIR=/data/ronghao/uenv/uenv-bridge/temp/benchmarks/swebenchpro/qwen3_6_35b_a3b_full \
./scripts/benchmark/run_swebenchpro_official_batches.sh
```

查看最终合并结果：

```bash
cat /data/ronghao/uenv/uenv-bridge/temp/benchmarks/swebenchpro/qwen3_6_35b_a3b_full/official_eval_batches_hubrat_retry/official_metrics.json
cat /data/ronghao/uenv/uenv-bridge/temp/benchmarks/swebenchpro/qwen3_6_35b_a3b_full/official_eval_batches_hubrat_retry/pull_failed_instance_ids.txt
```

断点续跑说明：

1. `status=done` 的 batch 会自动跳过。
2. `status=failed` 或 `status=running` 的 batch 会重新进入调度。
3. 重新调度 failed/running batch 时，脚本会先扫描该 batch 目录下已有的 `*_output.json`。已有 output 的 instance 会被视为已完成，不再提交给官方 evaluator。
4. 对这些已完成 instance，脚本会按官方逻辑从 output 中重新计算 resolved 结果，并写入该 batch 的 `eval_results.json`。
5. 只有没有 `*_output.json` 的 instance 会被重新运行。若需要强制重跑全部 case，可设置 `REDO_BATCHES=1` 或 `SKIP_COMPLETED_INSTANCES=0`。

## 7. 当前结果

### 7.1 数据准备结果

已完成 `ScaleAI/SWE-bench_Pro` public test split 下载和本地落地：

| 项 | 值 |
|---|---:|
| 样本数 | 731 |
| 仓库数 | 11 |
| Python 样本数 | 266 |
| Go 样本数 | 280 |
| JavaScript 样本数 | 165 |
| TypeScript 样本数 | 20 |

### 7.2 Qwen patch 生成 smoke

结果路径：

```text
temp/benchmarks/swebenchpro/qwen3_6_35b_a3b_smoke_limit2/generations.json
temp/benchmarks/swebenchpro/qwen3_6_35b_a3b_smoke_limit2/patches.json
temp/benchmarks/swebenchpro/qwen3_6_35b_a3b_smoke_limit2/generation_metrics.json
```

生成指标：

| 样本数 | nonempty_patch_rate | diff_git_patch_rate | hunk_patch_rate | output_tokens_min | output_tokens_max | output_tokens_avg |
|---:|---:|---:|---:|---:|---:|---:|
| 2 | 1.000 | 1.000 | 1.000 | 1228 | 4096 | 2662.00 |

观察：

1. 两条样本都能抽取到非空 `diff --git` patch，并包含 hunk。
2. 第 1 条样本触达 `MAX_TOKENS=4096`，说明 SWE-bench-Pro patch 生成可能需要更长输出或更强约束。
3. 第 2 条样本生成内容包含测试文件修改倾向，说明“patch 格式正确”并不代表可通过官方 resolved 评测。

### 7.3 官方 evaluator 验证状态

已下载全量官方评测资产：

```text
run_scripts/<instance_id>/run_script.sh
run_scripts/<instance_id>/parser.py
dockerfiles/base_dockerfile/<instance_id>/Dockerfile
dockerfiles/instance_dockerfile/<instance_id>/Dockerfile
```

并完成全量 patch 生成：

| 指标 | 值 |
|---|---:|
| 样本数 | 731 |
| nonempty_patch_rate | 1.000 |
| diff_git_patch_rate | 1.000 |
| hunk_patch_rate | 1.000 |
| output_tokens_min | 99 |
| output_tokens_max | 4096 |
| output_tokens_avg | 1836.66 |

曾使用 `docker.1ms.run/jefzda` 尝试全量官方 evaluator。该次运行得到 `resolve_rate=0`，但不能视为有效模型分数，原因是 724/731 个样本在拉取实例镜像时 404，只有 7 条真正进入容器测试。

后续镜像源探测结果：

| 镜像源 | 代表性 manifest 探测 | 结论 |
|---|---:|---|
| `docker.1ms.run/jefzda` | 33/33 失败 | 不再作为主源。 |
| `docker.1panel.live/jefzda` | 33/33 成功 | 首轮主源，但需要控制频率避免 Cloudflare 429。 |
| `hub.rat.dev/jefzda` | 33/33 成功 | 失败重试备选源。 |
| `docker.m.daocloud.io` | 非白名单拒绝 | 不适用。 |
| `dockerproxy.com` / `dockerpull.*` | 本机超时 | 不适用。 |

新增 batch runner smoke 已完成：

```text
temp/benchmarks/swebenchpro/qwen3_6_35b_a3b_full/official_eval_batches_smoke
```

该 smoke 验证了 batch 切分、官方 evaluator 调用、日志记录、pull-failed 识别、镜像清理和结果合并流程。由于 1panel 当时返回 429，样本未进入真实测试容器；该失败被记录到 `pull_failed_instance_ids.txt`，可用第二轮换源重试。

后续又使用已失败的 `batch_00011` 目录验证了 case 级断点续跑：该 batch 中已有 5 个 instance 产出 `*_output.json`，重新调度时脚本只提交剩余 5 个 instance，同时把已有 5 个 output 补算进 `eval_results.json`。

## 8. 结论

当前已经完成 SWE-bench-Pro 数据集下载、字段确认、patch 生成脚本编写，以及 Qwen3.6-35B-A3B 的 2 条样本生成 smoke。模型能够生成可抽取的 unified diff patch，但 smoke 中已经出现输出过长和修改测试文件的风险，后续需要依赖官方 evaluator 才能得到真实 resolve rate。

官方 evaluator 侧当前不再采用一次性全量预拉镜像的方案，而是采用第 6.4 节的分批按需评测方案。该方案可以在当前 Docker root 空间有限的情况下继续推进全部 731 个 case，并通过 `pull_failed_instance_ids.txt` 将网络/镜像失败样本与真实未 resolved 样本区分开。
