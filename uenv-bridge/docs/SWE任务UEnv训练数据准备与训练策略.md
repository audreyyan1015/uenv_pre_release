# SWE 任务 UEnv 训练数据准备与训练策略

> 日期：2026-07-31
> 目标任务：SWE-bench-Pro / 仓库级程序修复
> 基座模型：`Qwen/Qwen3.6-35B-A3B`
> 当前决策：先跳过 SFT，直接使用 UEnv + OpenHands + VeRL 做在线 RL/RLVR 小规模验证。

## 1. 当前结论

本阶段先不做 SFT。第一轮训练目标不是直接追求最终 SWE-bench-Pro 高分，而是验证下面三件事：

| 目标 | 说明 | 验证信号 |
|---|---|---|
| UEnv 在线环境能进入 VeRL 训练闭环 | rollout 从 VeRL AgentLoop 接出到 UEnv，Worker/OpenHands 执行真实仓库任务并返回 reward。 | request/result/gateway 数量对齐，VeRL step 正常完成。 |
| SWE reward 能被训练侧稳定消费 | Worker 返回 `resolved` 或等价 reward，Adapter 转成 VeRL `reward_score`。 | `critic/rewards/*`、`critic/score/*` 有有效数值。 |
| 训练后能对 SWE-bench-Pro 产生可测变化 | 使用固定 public test split 做训练外评测。 | 与当前基线 `106/731 = 14.50%` resolved 对比。 |

SFT 可以作为后续增强路线，但纯 SFT 本身不需要 UEnv、OpenHands 或在线环境 reward。当前更需要先证明 UEnv 的环境交互和可执行判分能被 VeRL 训练使用，因此第一轮采用在线 RL/RLVR。

## 2. 数据准备

### 2.1 数据集边界

SWE-bench-Pro public test split 已经作为基准模型评测集使用，当前结果为 `resolved=106/731`。这 731 条样本继续固定为最终评测集，不进入训练数据。

| 数据 | 当前用途 | 是否进入训练 |
|---|---|---|
| `data/benchmarks/swebenchpro/test.jsonl` | SWE-bench-Pro public test 全量评测 | 否 |
| `temp/benchmarks/swebenchpro/...` 已完成轨迹/结果 | 失败归因、prompt 与环境诊断 | 否，后续可筛成功轨迹做 SFT 资料 |
| 可执行 SWE 训练任务池 | 在线 RL 训练 | 是 |
| 外部公开 SWE trajectory 数据 | 后续 SFT、rejection SFT 或错误分析 | 当前阶段不使用 |

### 2.2 训练数据候选

第一轮在线 RL 需要“可执行任务”，即每条样本不仅有 issue 描述，还要能还原 repo、base commit、依赖环境和测试命令。推荐按下面顺序准备。

| 优先级 | 数据来源 | 适合用途 | 处理方式 |
|---:|---|---|---|
| P0 | Worker/EnvPackage 已支持的 SWE 样例池 | 最小训练 smoke | 先选 5 到 20 条已确认可执行、能返回 resolved/failed 的样本。 |
| P0 | SWE-Gym real tasks | 第一批可执行训练数据 | 其定位就是训练 SWE agents，包含真实任务、repo 环境和测试验证。 |
| P1 | SWE-smith task instances | 放大训练数据 | 可作为规模化任务来源，需先确认镜像、repo profile 和 UEnv EnvPackage 接入方式。 |
| P1 | 自建 issue-fix 任务 | 与 UEnv 环境最贴合的数据 | 从内部可控仓库构造 issue、base commit、测试与标准 patch。 |
| P2 | 公开 OpenHands/SWE-agent trajectory | 后续 SFT 或 reward model 分析 | 当前不直接进入在线 RL 训练集。 |

参考资料：SWE-bench 用真实 GitHub issue 评估模型生成 patch；SWE-Gym 提供约 2.4K 真实训练任务和可执行环境；SWE-smith 提供规模化 SWE 任务生成与公开 task/trajectory 资源。

### 2.3 原始任务字段

每条 SWE 训练样本至少需要准备下面字段。字段越完整，Worker 侧越容易复现环境，Adapter 侧也越容易做日志回溯。

| 字段 | 类型 | 说明 |
|---|---|---|
| `instance_id` | string | 任务唯一 ID，贯穿 request、result、trajectory 和评测汇总。 |
| `repo` | string | GitHub 仓库名，例如 `django/django`。 |
| `repo_language` | string | 语言，用于后续分语言统计。 |
| `base_commit` | string | 任务起始 commit。 |
| `problem_statement` | string | issue 描述或任务说明。 |
| `dockerhub_tag` | string | 若任务依赖预构建镜像，需要记录镜像 tag。 |
| `env_package_id` | string | UEnv EnvPackage 名，例如 `swe-bench-pro` 或后续训练包名。 |
| `env_package_version` | string | EnvPackage 版本。 |
| `agent_bridge_id` | string | OpenHands agent bridge，例如 `uenv-agent-openhands`。 |
| `driver_entrypoint` | string | Worker 侧执行入口，例如 `run_swebenchpro_official.py`。 |
| `workspace_dir` | string | 容器内目标 repo 工作目录，当前 SWE 评测采用 `/app`。 |
| `llm_config_path` | string | OpenHands 侧 LLM 配置文件路径。 |
| `max_iterations` | int | OpenHands 最大工具调用/迭代次数。 |
| `expected_reward` | string | 训练时使用 `swe_resolved`，即测试通过为 1，否则为 0。 |

### 2.4 VeRL parquet 格式

训练数据最终需要转成 VeRL 能读取的 parquet。建议保留标准 VeRL 字段，同时把 SWE 专用字段放入 `extra_info`。

| VeRL 字段 | 建议值 |
|---|---|
| `data_source` | `uenv/swe-train` 或具体数据名，例如 `swe-gym`。 |
| `ability` | 短期可填 `agent`，使当前 Adapter 走 agent 路由；后续可显式支持 `swe`。 |
| `prompt` | chat messages。user 内容写入 issue 描述和最小执行要求。 |
| `reward_model.style` | `external`。 |
| `reward_model.ground_truth` | `resolved` 或 `instance_id`。 |
| `extra_info.dataset` | 数据集名。 |
| `extra_info.instance_id` | 原始任务 ID。 |
| `extra_info.repo` | 仓库名。 |
| `extra_info.base_commit` | 起始 commit。 |
| `extra_info.env_package_id` | UEnv EnvPackage 名。 |
| `extra_info.driver_entrypoint` | Worker driver。 |
| `extra_info.workspace_dir` | `/app`。 |
| `extra_info.llm_config_path` | OpenHands LLM config。 |
| `extra_info.max_iterations` | 例如 `20`、`40`、`60`。 |

SWE prompt 可以采用下面的基础模板。真正的仓库 checkout、测试执行和工具调用仍由 Worker/OpenHands 完成。

```text
You are fixing a real software issue in a checked-out repository.

Repository: {repo}
Base commit: {base_commit}
Workspace: {workspace_dir}

<issue_description>
{problem_statement}
</issue_description>

Implement the minimal fix to non-test source files. Use the available terminal
and file editing tools. When finished, provide the final patch summary.
```

### 2.5 Adapter 与 Worker 字段要求

当前 `scripts/benchmark/evaluate_swebenchpro_uenv.py` 已经能构造 `env_type=swe` 的 benchmark request。但 VeRL 训练入口 `UEnvAgentLoop` 当前更偏通用 QA/code/agent 路由，SWE 训练前需要确认下面能力。

| 项目 | 当前状态 | 训练前要求 |
|---|---|---|
| `env_type=swe` 路由 | benchmark 脚本支持；AgentLoop 训练路由需要确认 | 推荐在 AgentLoop 中显式支持 `swe`，或短期用 `ability=agent` 并透传 `task_name=swe-bench-pro`。 |
| SWE `extra_info` 透传 | benchmark request 手写完整字段；训练数据 parquet 需要自动转 request | Adapter 需要把 `instance_id/repo/base_commit/driver_entrypoint/workspace_dir/llm_config_path` 写入 `env_config` 和 `metadata`。 |
| 多轮 OpenHands token | Worker 返回 trajectory；训练侧需要明确 response token 表示范围 | 推荐先返回所有模型 assistant token 的拼接 `response_ids/response_mask`，而不是只返回最终 patch 文本。 |
| reward | benchmark 用 `resolved` | 训练 result 中 `summary.total_reward` 应为 `1.0/0.0`。 |
| 失败样本 | benchmark 会记录 failed | 训练阶段需要把超时、环境失败、模型失败区分开，环境失败可从训练样本池剔除。 |

## 3. 训练策略

### 3.1 总体阶段

第一轮训练采用“环境 smoke -> 1-step RL smoke -> 小样本短跑 -> 参数放大 -> 固定评测”的路线。

| 阶段 | 数据规模 | 训练步数 | 目标 | 通过标准 |
|---|---:|---:|---|---|
| A. 环境 smoke | 5 到 20 条 | 0 | 只验证 Worker/OpenHands 可执行，排除环境坏样本。 | 每条样本能返回 completed/failed/resolved，不出现调度或路径错误。 |
| B. 1-step RL smoke | 2 到 4 条 | 1 | 验证 VeRL 能消费 SWE result 并完成一次 update。 | VeRL step 完成，reward 有效，request/result 数量对齐。 |
| C. 10-step 小跑 | 20 到 100 条 | 10 | 观察耗时、显存、reward 分布和失败类型。 | 无 OOM；失败主要来自模型未解决，而不是环境错误。 |
| D. 子集训练 | 100 到 500 条 | 50 到 200 | 观察 resolved/reward 是否有趋势。 | 固定验证子集 resolved 有可解释变化。 |
| E. 训练外评测 | 731 条 | 0 | 回归 SWE-bench-Pro public test。 | 与基线 `106/731` 对比。 |

### 3.2 第一轮训练模式

第一轮使用同步 GRPO。同步模式下，rollout 完成后再进行 logprob/ref/update，版本关系最清楚，便于判断 reward、数据和环境是否工作正常。

| 模式 | 何时使用 | 说明 |
|---|---|---|
| 同步 GRPO | 第一轮 SWE 训练 | 建立可信基线，降低版本和 queue 变量。 |
| One-step off-policy | 同步 SWE 小跑稳定后 | 用于验证 rollout 与 update 重叠是否降低 idle。 |
| Fully async | 同步和 one-step 都有可解释结果后 | 用于长程 SWE rollout 场景的吞吐优化。 |

### 3.3 第一轮保守参数

SWE 评测时使用 `MAX_TOKENS=8192`、`THINKING_TOKEN_BUDGET=4096`、vLLM `max_model_len=131072`。训练阶段同时有 actor/ref/old logprob/update 权重和 KV cache 显存压力，第一轮先用更保守配置验证闭环。

| 参数 | 建议起点 | 说明 |
|---|---:|---|
| `TRAINING_STEPS` | `1` 或 `10` | 先 1-step，再 10-step。 |
| `TRAIN_BATCH_SIZE` | `4` | 与 8 卡整除约束匹配。 |
| `PPO_MINI_BATCH_SIZE` | `4` | 保持简单。 |
| `ROLLOUT_N` | `2` | 每 step 有 8 条 episode，满足 8 卡 batch 约束。 |
| `PPO_MICRO_BATCH_SIZE_PER_GPU` | `1` | 降低显存压力。 |
| `ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU` | `1` | 降低 logprob 显存压力。 |
| `REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU` | `1` | 降低 ref 显存压力。 |
| `MAX_PROMPT_LENGTH` | `2048` 到 `4096` | SWE issue prompt 比数学更长。 |
| `DATA_MAX_RESPONSE_LENGTH` | `2048` | 第一轮先保证训练可跑；稳定后再放大。 |
| `ROLLOUT_TP` | `8` | 沿用当前稳定的单个 8 卡 TP endpoint。 |
| `ROLLOUT_GPU_MEMORY_UTILIZATION` | `0.20` | 沿用 Qwen3.6-35B 当前稳定配置。 |
| `ROLLOUT_ENABLE_SLEEP_MODE` | `False` | 当前稳定配置关闭 sleep/free-cache。 |
| `ROLLOUT_FREE_CACHE_ENGINE` | `False` | 当前稳定配置关闭 sleep/free-cache。 |
| `ROLLOUT_CALCULATE_LOG_PROBS` | `False` | 同步模式由训练侧重算 logprob。 |

8GPU 下需要满足 VeRL batch 约束：`TRAIN_BATCH_SIZE * ROLLOUT_N` 应能被数据并行大小整除。上表中 `4 * 2 = 8`，适合作为第一轮起点。

### 3.4 参数放大顺序

每次只调整一个主要变量，便于定位问题。

| 顺序 | 调整项 | 放大方式 | 观察指标 |
|---:|---|---|---|
| 1 | 样本数 | `20 -> 100 -> 500` | 环境失败率、平均 episode 时间、resolved 分布。 |
| 2 | 训练步数 | `1 -> 10 -> 50 -> 200` | reward 曲线、KL、loss、OOM。 |
| 3 | response length | `2048 -> 4096 -> 8192` | `response_length/clip_ratio`、OpenHands 是否截断、step 时间。 |
| 4 | OpenHands iterations | `20 -> 40 -> 60` | 工具调用完成率、timeout、patch 是否非空。 |
| 5 | batch/rollout_n | `4x2 -> 4x4 -> 8x2` | 吞吐、显存、Server/Worker 并发压力。 |
| 6 | 并行模式 | `sync -> one-step -> fully async` | trainer idle、单位 resolved 成本、stale 样本影响。 |

### 3.5 评测与回归

训练过程中保留两类评测：

| 评测 | 数据 | 频率 | 用途 |
|---|---|---|---|
| 训练内验证 | 从训练任务池切出的固定 dev 子集 | 每个阶段结束 | 判断训练是否改善同分布任务。 |
| 训练外评测 | SWE-bench-Pro public test 731 条 | 关键 checkpoint 后 | 与当前基线 `106/731 = 14.50%` 对比。 |

同时保留五类 benchmark 的回归入口。SWE 训练可能提升软件工程任务，但也可能影响数学、医疗 QA、表格理解和代码生成格式，因此阶段性模型需要回归五类指标。

## 4. 需要记录的指标

### 4.1 VeRL 训练指标

| 指标 | 作用 |
|---|---|
| `critic/rewards/mean` | 当前训练 reward 均值。 |
| `critic/score/mean` | reward manager 聚合后的 score。 |
| `response_length/mean` | 观察输出长度和截断趋势。 |
| `response_length/clip_ratio` | 判断 `DATA_MAX_RESPONSE_LENGTH` 是否过短。 |
| `timing_s/gen` | rollout/UEnv/OpenHands 总耗时。 |
| `timing_s/ref` | ref logprob 耗时。 |
| `timing_s/update_actor` | actor 更新耗时。 |
| `timing_s/update_weights` | 权重同步耗时。 |
| `timing_s/step` | 单 step 总耗时。 |

### 4.2 UEnv / SWE 指标

| 指标 | 作用 |
|---|---|
| request/result 数量 | 检查 Adapter 与 Server/Worker 是否对齐。 |
| `uenv_status` | 区分 completed、failed、timeout、environment error。 |
| `resolved` | SWE 主指标，最终 reward 来源。 |
| `git_diff_bytes` | 判断 OpenHands 是否产生实际 patch。 |
| `tests_passed/tests_total` | 判断失败是完全未执行、部分通过还是全部通过。 |
| `trajectory_id` | 回溯 OpenHands 工具调用和最终 patch。 |
| `elapsed_ms` | 评估单样本成本。 |
| `terminate_reason` | 区分正常完成、达到 max iterations、上下文超长或超时。 |

## 5. 第一轮命令模板

下面命令是第一轮 1-step SWE RL smoke 的建议形态。正式运行前需要先准备 `DATA_DIR` 下的 SWE VeRL parquet，并确认 Adapter 已能把 SWE `extra_info` 透传到 Worker。

```bash
cd /data/ronghao/uenv/uenv-bridge

RUN_ID=swe_grpo_35b_1step_sync_$(date +%Y%m%d_%H%M%S) \
MODEL_PATH=/data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B \
CONTAINER_MODEL_PATH=/models/modelscope/Qwen/Qwen3___6-35B-A3B \
DATA_DIR=/data/ronghao/uenv/uenv-bridge/data/swe_train_smoke \
CONTAINER_DATA_DIR=/data/swe_train_smoke \
SERVER_ADAPTER_CORE_ENDPOINT=8.130.75.157:8088 \
UENV_MODEL_GATEWAY_ENABLED=1 \
UENV_MODEL_GATEWAY_PORT=18194 \
UENV_MODEL_GATEWAY_PUBLIC_URL=http://10.10.20.142:18194/v1 \
UENV_AGENT_LOOP_PARALLEL_MODE=sync \
UENV_AGENT_LOOP_TIMEOUT_SECONDS=7200 \
TRAINING_STEPS=1 \
TRAIN_BATCH_SIZE=4 \
PPO_MINI_BATCH_SIZE=4 \
PPO_MICRO_BATCH_SIZE_PER_GPU=1 \
ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=1 \
REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=1 \
ROLLOUT_N=2 \
ROLLOUT_TP=8 \
ROLLOUT_CALCULATE_LOG_PROBS=False \
MAX_PROMPT_LENGTH=4096 \
DATA_MAX_RESPONSE_LENGTH=2048 \
TEST_FREQ=-1 \
SAVE_FREQ=-1 \
PODMAN_GPU_ARGS='nvidia.com/gpu=all' \
CUDA_VISIBLE_DEVICES_IN_CONTAINER=0,1,2,3,4,5,6,7 \
NGPUS_PER_NODE=8 \
RAY_NUM_CPUS=32 \
ROLLOUT_GPU_MEMORY_UTILIZATION=0.20 \
ROLLOUT_ENABLE_SLEEP_MODE=False \
ROLLOUT_FREE_CACHE_ENGINE=False \
EXTRA_VERL_ARGS='+actor_rollout_ref.model.override_config.attn_implementation=sdpa actor_rollout_ref.rollout.max_model_len=6144 actor_rollout_ref.rollout.max_num_batched_tokens=6144 actor_rollout_ref.actor.fsdp_config.optimizer_offload=True' \
./scripts/run_layer4_distributed.sh
```

若出现 OOM，优先将 `actor_rollout_ref.rollout.max_model_len` 和 `DATA_MAX_RESPONSE_LENGTH` 降到 `4096/1024` 组合，再确认链路；若 reward 长期为 0 且大量 `git_diff_bytes=0`，优先检查 prompt、OpenHands LLM config、workspace 映射和工具调用轨迹。

## 6. 风险与待确认问题

| 问题 | 影响 | 建议处理 |
|---|---|---|
| SWE reward 稀疏 | 大部分样本 reward 可能为 0，PPO 信号弱。 | 先选 Worker 已确认可解或可执行的子集，逐步放大；保留 `git_diff/tests` 辅助指标。 |
| 环境失败混入模型失败 | 训练会把环境错误误当模型能力差。 | 训练前先跑环境 smoke，剔除镜像缺失、repo checkout 失败、测试脚本错误样本。 |
| OpenHands 多轮 token 与 VeRL response 对齐 | PPO 需要明确训练哪些 token。 | Worker/Adapter 约定返回所有 assistant 生成 token 的 `response_ids/response_mask`。 |
| 长上下文 OOM | SWE prompt 和工具轨迹长，训练侧显存远高于评测侧。 | 从 `2048` response 起步，稳定后再放大到 `4096/8192`。 |
| timeout 与 h2 cancel | 长任务会放大 Server/Worker/gateway 超时问题。 | 第一轮降低 `max_iterations`，确认链路后再恢复到评测口径 `60`。 |
| 与评测口径不一致 | 训练时短 token/少 iteration 可能和最终 SWE-bench-Pro 评测差距较大。 | 训练稳定后逐步把 `MAX_TOKENS/max_iterations` 拉近评测配置。 |
| 多任务遗忘 | SWE 训练可能影响数学、QA 和代码生成。 | 关键 checkpoint 后跑五类 benchmark 固定子集回归。 |

## 7. 后续执行清单

1. 准备 5 到 20 条 SWE 可执行训练样本，确认不包含 SWE-bench-Pro public test。
2. 将样本转成 VeRL parquet，保留 `prompt/reward_model/extra_info`。
3. 补齐或确认 AgentLoop 训练路径的 `env_type=swe` 与 SWE `extra_info` 透传。
4. 使用 benchmark driver 先做环境 smoke，排除环境坏样本。
5. 跑 1-step 同步 GRPO smoke，检查 VeRL reward、request/result/gateway 日志。
6. 跑 10-step 小样本训练，统计 reward、resolved、git diff、timeout 和 step 时间。
7. 选择一个 checkpoint 跑 SWE-bench-Pro public test 固定评测，与 `106/731` 基线对比。
8. 同步基线可信后，再引入 one-step off-policy 或 fully async 做吞吐对照。

## 8. 参考资料

- 本地稳定训练配置：`docs/Qwen3.6-35B VeRL稳定训练配置.md`
- 本地 SWE 基线评测：`docs/任务测评/SWE-bench-Pro测试生成程序修复基线评测.md`
- 本地 SWE request builder：`scripts/benchmark/evaluate_swebenchpro_uenv.py`
- SWE-bench: <https://github.com/swe-bench/SWE-bench>
- SWE-Gym: <https://github.com/SWE-Gym/SWE-Gym>
- SWE-smith: <https://github.com/SWE-bench/SWE-smith>
