# UEnv+VeRL 与原生 VeRL 对比实验配置对齐检查清单

本文档用于约束 SWE-smith 任务上 UEnv+VeRL 与原生 VeRL baseline 的对比实验。目标不是只让启动命令看起来相同，而是保证两侧实际进入 VeRL、OpenHands、worker/runtime 的关键变量一致。

## 1. 对齐原则

对比实验必须同时满足三层一致：

| 层级 | 要求 |
| --- | --- |
| VeRL 训练层 | 模型、算法、batch、rollout、seed、最大长度、保存/评估频率一致 |
| Agent/OpenHands 层 | `instance_id` 顺序、`max_steps`、LLM config、采样参数、gateway 地址口径一致 |
| Worker/runtime 层 | 镜像、harness、判分逻辑、并发槽位、超时策略一致或明确记录差异 |

只设置环境变量不等于实际对齐。对于已经生成好的 parquet，`extra_info` 中的字段会被 AgentLoop 读出来继续下发；如果不重新生成或修正 parquet，脚本环境变量可能不会覆盖已有数据字段。

## 2. 启动前检查

### 2.1 数据与样本顺序

| 检查项 | 目标值 / 要求 | 核验方式 |
| --- | --- | --- |
| 训练数据 | 使用同一份 ordered parquet | 对比 `data.train_files` 与宿主机 `DATA_DIR` |
| 样本顺序 | 按 parquet 行序一致 | 读取 `train.parquet` 前 N 行 `extra_info.instance_id` |
| 数据 shuffle | `data.shuffle=false` | VeRL 日志中 Hydra 配置为准 |
| 数据 seed | 固定为 `42`，或明确不参与 shuffle | VeRL 日志中 Hydra 配置为准 |
| rollout copy | `TRAIN_BATCH_SIZE * ROLLOUT_N` 一致 | 每 step 请求数一致 |

当前建议：UEnv 20 step 使用从原生实际 rollout 顺序抽出的数据目录：

```text
/data/ronghao/uenv/uenv-bridge/data/benchmarks/swesmith_native61_ordered_20260824_001426
```

注意：原生 baseline 的 `agent-loop-requests.jsonl` 中 `sample_index` 记录粒度与 UEnv 不同，不能只用该字段判断样本是否对齐。应以 parquet 行序和 `instance_id` 列表为准。

### 2.2 模型与训练超参

| 检查项 | 目标值 |
| --- | --- |
| 模型 | `/data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B` |
| 算法 | GRPO |
| `TRAIN_BATCH_SIZE` | `2` |
| `PPO_MINI_BATCH_SIZE` | `2` |
| `PPO_MICRO_BATCH_SIZE_PER_GPU` | `1` |
| `ROLLOUT_N` | `4` |
| 每 step episode 数 | `8` |
| `ROLLOUT_TEMPERATURE` | `1.0` |
| `MAX_PROMPT_LENGTH` | `8192` |
| `DATA_MAX_RESPONSE_LENGTH` | `8192` |
| `ROLLOUT_TP` | `8` |
| `NGPUS_PER_NODE` | `8` |
| `SAVE_FREQ` | 对比小跑可设为 `20` 或关闭；两侧必须一致 |
| `TEST_FREQ` | `-1`，训练中不做 eval |

### 2.3 rollout / vLLM 参数

| 检查项 | 目标值 |
| --- | --- |
| `actor_rollout_ref.rollout.max_model_len` | `262144` |
| `actor_rollout_ref.rollout.max_num_batched_tokens` | `65536` |
| `actor_rollout_ref.rollout.enable_chunked_prefill` | `True` |
| `actor_rollout_ref.rollout.enable_sleep_mode` | `True` |
| `actor_rollout_ref.rollout.free_cache_engine` | `True` |
| `actor_rollout_ref.rollout.calculate_log_probs` | `True` |
| `generation_config.max_new_tokens` | `8192` |
| `generation_config.logprobs` | `true` |

需要特别区分：

| 字段 | 含义 |
| --- | --- |
| `DATA_MAX_RESPONSE_LENGTH` | VeRL 训练侧 response 张量最大长度 |
| `generation_config.max_new_tokens` | 下发给 agent/OpenHands 的单次模型生成上限 |
| `UENV_MODEL_GATEWAY_MAX_TOKENS` | adapter gateway 对 OpenAI 请求的额外截断上限 |

如果要与原生 baseline 严格对齐，三者不应互相矛盾。若 `generation_config.max_new_tokens=8192`，但 `UENV_MODEL_GATEWAY_MAX_TOKENS=4096`，则实际模型请求会被 gateway 截断到 4096，不能算完全对齐。

### 2.4 OpenHands LLM config

| 检查项 | 原生 baseline 实际值 | UEnv 目标值 |
| --- | --- | --- |
| `agent_job.json.llm_config_path` | `/root/UEnv/config/openhands-llm-20877.json` | `/root/UEnv/config/openhands-llm-20877.json` |
| `thinking_token_budget` | 不应存在或应为 `None` | 不应存在或应为 `None` |

当前已发现的未对齐点：

- 原生 baseline 的远端 `agent_job.json` 实际使用 `/root/UEnv/config/openhands-llm-20877.json`。
- UEnv 复用的 ordered parquet 中，`extra_info.llm_config_path` 仍是 `/root/UEnv/config/openhands-llm-qwen3-thinking-max-token-8192.json`。
- 在 UEnv 路径中，`SWE_LLM_CONFIG_PATH` 主要用于数据 prepare 阶段；如果 `SWE_PREPARE_DATA=0` 直接复用 parquet，它不会自动覆盖 parquet 里的 `extra_info.llm_config_path`。

因此，启动 UEnv 对齐实验前应先生成一份修正后的 ordered parquet，或重新 prepare 数据，确保 `extra_info.llm_config_path` 已经写成 `/root/UEnv/config/openhands-llm-20877.json`。

## 3. 启动后检查

### 3.1 VeRL Hydra 配置核验

启动后必须从训练日志中确认以下字段，而不是只看 shell 环境变量：

| 字段 | 期望 |
| --- | --- |
| `data.train_files` | 指向对齐后的 ordered parquet |
| `data.train_batch_size` | `2` |
| `data.shuffle` | `False` |
| `data.max_prompt_length` | `8192` |
| `data.max_response_length` | `8192` |
| `actor_rollout_ref.rollout.n` | `4` |
| `actor_rollout_ref.rollout.temperature` | `1.0` |
| `actor_rollout_ref.rollout.tensor_model_parallel_size` | `8` |
| `trainer.total_training_steps` | 本轮为 `20` |
| `trainer.test_freq` | `-1` |

### 3.2 AgentLoop request 核验

从 `agent-loop-requests.jsonl` 抽样检查：

| 字段 | 期望 |
| --- | --- |
| `payload.env_config.instance_id` | 与 ordered parquet 对应行一致 |
| `payload.env_config.llm_config_path` | `/root/UEnv/config/openhands-llm-20877.json` |
| `payload.episode_config.max_steps` | `50` |
| `payload.model_endpoint.generation_config.max_new_tokens` | `8192` |
| `payload.model_endpoint.generation_config.temperature` | `1.0` |
| `payload.model_endpoint.generation_config.logprobs` | `true` |
| `model_endpoint` | worker 可访问的 gateway URL |

### 3.3 远端 worker/OpenHands 核验

从远端实际 job 输出目录抽样检查：

| 文件 / 字段 | 期望 |
| --- | --- |
| `agent_job.json.llm_config_path` | `/root/UEnv/config/openhands-llm-20877.json` |
| `agent_job.json.generation_config.max_new_tokens` | `8192` |
| `effective_llm_config.json` | 不含 `thinking_token_budget=4096` |
| driver 命令行 `--max-iterations` | `50` |
| driver 命令行 `--gateway` | 指向同一 runtime gateway |

## 4. 运行中指标检查

### 4.1 episode 与 step 数

本轮 20 step 对齐实验的期望规模：

| 项目 | 数值 |
| --- | --- |
| `TRAINING_STEPS` | `20` |
| `TRAIN_BATCH_SIZE` | `2` |
| `ROLLOUT_N` | `4` |
| 预期 episode 总数 | `20 * 2 * 4 = 160` |

如果 `agent-loop-requests.jsonl` 的总请求数明显偏离 160，需要先停止并排查，不应继续拿结果做对比。

### 4.2 性能对比口径

优先比较以下指标：

| 指标 | 读取位置 |
| --- | --- |
| 单 step 总耗时 | VeRL `Training Progress` 或 trainer timing |
| rollout 耗时 | `timing_s/gen` |
| update 耗时 | `timing_s/update_actor`、`timing_s/update_weights` |
| gateway 请求大小 | `model-gateway.jsonl.request_bytes` |
| gateway 响应耗时 | `model-gateway.jsonl.duration_ms` |
| episode 完成/失败 | `agent-loop-results.jsonl.status` |
| resolved / reward | `agent-loop-results.jsonl.metadata` 与 VeRL reward 日志 |

如果 UEnv 明显慢于原生 baseline，优先排查：

- OpenHands LLM config 是否仍开启 thinking budget。
- `request_bytes` 是否显著大于原生 baseline。
- 某些 episode 是否出现长尾，导致 `timing_s/gen` 拉长。
- worker 实际并发槽位是否与原生 baseline 的执行方式一致。
- runtime gateway、server、adapter gateway 是否引入额外等待。

## 5. 不可比条件

出现以下任一情况，本轮结果不能作为公平对比结论：

- 两侧 `instance_id` 顺序不同。
- 两侧 `llm_config_path` 不同，尤其一侧开启 thinking budget，另一侧未开启。
- 两侧 `generation_config.max_new_tokens` 或 gateway token clamp 不同。
- 一侧训练中做 eval 或保存 checkpoint，另一侧没有。
- 一侧 worker 并发槽位、runtime gateway 或 harness 版本不同且未记录。
- 一侧单条 episode 失败被 zero reward，另一侧直接重试或 fail-fast。
- 日志中无法确认 Hydra 实际配置。

## 6. 当前 20 step UEnv 重跑建议

在启动前先准备一份新的对齐数据目录，要求：

| 字段 | 目标 |
| --- | --- |
| 来源 | `swesmith_native61_ordered_20260824_001426` |
| `extra_info.llm_config_path` | `/root/UEnv/config/openhands-llm-20877.json` |
| `extra_info.max_steps` / `max_iterations` | `50` |
| 行序 | 保持不变 |

启动参数建议：

| 参数 | 目标值 |
| --- | --- |
| `TRAINING_STEPS` | `20` |
| `TRAIN_BATCH_SIZE` | `2` |
| `ROLLOUT_N` | `4` |
| `DATA_SHUFFLE` | `false` |
| `TRAIN_SEED` / `DATA_SEED` / loader seed | `42` |
| `ROLLOUT_SEED` | 与原生 baseline 保持一致，优先 `42` |
| `SWE_PREPARE_DATA` | `0`，但前提是 parquet 已修正 |
| `UENV_EPISODE_MAX_STEPS_OVERRIDE` | `50` |
| `UENV_AGENT_LOOP_FAILED_EPISODE_POLICY` | `zero_reward` |
| `TEST_FREQ` | `-1` |
| `SAVE_FREQ` | 两侧一致；20 step 小跑可设为 `20` |

启动后先等到第一轮 rollout 开始，再检查 `agent-loop-requests.jsonl` 的前 8 条是否满足本清单第 3.2 节。未满足时应立即停止，不继续跑满 20 step。

## 7. 2026-08-24 对齐重跑阻塞记录

本节记录 `verl_swesmith_grpo_uenv_align20_20260824_122210` 的核验结论。该 run 不能作为 UEnv+VeRL 与原生 VeRL 的公平对比结果。

### 7.1 现象

| 项目 | 结果 |
| --- | --- |
| UEnv run id | `verl_swesmith_grpo_uenv_align20_20260824_122210` |
| 结果条数 | 前 3 个 step 共 24 条 episode |
| 结果状态 | 24 条全部 `failed` |
| 主要错误 | `litellm.ServiceUnavailableError: ... 503 ... {'error': 'backend_starting'}` |
| 本地 model gateway | 只有一次人工 `GET /v1/models`，没有 OpenHands 的 `POST /v1/chat/completions` |

这说明本轮并没有真正形成“UEnv 下发 episode 后，OpenHands 访问训练机 rollout gateway 生成”的闭环。

### 7.2 已确认的对齐项

本轮启动后从日志中确认，以下训练侧字段已经按对齐要求生效：

| 字段 | 实际值 |
| --- | --- |
| `data.train_batch_size` | `2` |
| `actor_rollout_ref.rollout.n` | `4` |
| 每 step episode 数 | `8` |
| `trainer.total_training_steps` | `20` |
| `data.shuffle` | `false` |
| seed | `42` |
| `data.max_response_length` | `8192` |
| `generation_config.max_new_tokens` | `8192` |
| `episode_config.max_steps` | `50` |

前 8 条 `instance_id` 也与原生 baseline 的 ordered parquet 行序一致。

### 7.3 未对齐根因

本轮请求的 `AgentJob` 中虽然包含：

```text
model_endpoint = http://10.10.20.142:18088/v1
model_name = /models/modelscope/Qwen/Qwen3___6-35B-A3B
generation_config.max_new_tokens = 8192
llm_config_path = /root/UEnv/config/openhands-llm-20877.json
```

但 208.77 agent 机器上的实际 job 输出显示：

```text
config_snapshot.json.llm_model = openai/deepseek-v3-0324-awq
/root/UEnv/config/openhands-llm-20877.json.base_url = http://219.147.100.43:18888/v1
```

也就是说，OpenHands 最终读取的是远端 `llm_config_path` 中的旧 DeepSeek/18888 配置，而不是 `AgentJob.model_endpoint` 中的训练机 Qwen gateway。当前 208.77 上的 `/root/UEnv/integrations/openhands/run_swebenchpro_official.py` 版本只把 `agent_job.llm_config_path` 赋给 `args.llm_config`，没有把 `agent_job.model_endpoint` 覆盖写入临时 `effective_llm_config.json`。

本地仓库中的 `integrations/openhands/run_swebenchpro_official.py` 已经具备 `_write_effective_llm_config(...)` 逻辑，会把 `AgentJob.model_endpoint`、`model_name` 和 `generation_config` 覆盖到 OpenHands LLM config；但 208.77 实际运行的远端 `/root/UEnv` 版本尚未包含这段逻辑。

### 7.4 下一轮启动前必须修复

下一轮 UEnv 对齐实验必须先满足以下条件，否则不继续跑满：

| 检查项 | 通过标准 |
| --- | --- |
| 远端 driver 版本 | 208.77 实际调用的 `run_swebenchpro_official.py` 能根据 `AgentJob.model_endpoint` 生成 `effective_llm_config.json` |
| 远端生效 LLM config | job 输出目录存在 `effective_llm_config.json`，且 `base_url` 指向本轮训练机 model gateway |
| `config_snapshot.json.llm_model` | 应为本轮 Qwen 模型，而不是 `openai/deepseek-v3-0324-awq` |
| 模型请求链路 | 本地 `model-gateway.jsonl` 出现 OpenHands 的 `POST /v1/chat/completions` |
| 网络连通性 | agent 机器能访问最终写入 `effective_llm_config.json.base_url` 的地址 |

如果远端 `/root/UEnv` 暂时不能直接更新，应使用非覆盖方式部署一份新 driver，并让 runner 指向该新路径；同时记录部署路径和启动方式。

### 7.5 Poller 隔离要求

208.77 上同时存在两类 OpenHands poller：

| 类型 | 典型目录 / 环境 | 说明 |
| --- | --- | --- |
| 标准 UEnv poller | `UENV_AGENT_BRIDGE_DIR=/root/UEnv/integrations/openhands`，日志在 `/var/log/uenv/openhands-runs` / `openhands-extra-*` | 本轮 UEnv job 实际由这组 poller 执行 |
| 原生 baseline poller | `UENV_AGENT_BRIDGE_DIR=/root/uenv-native-swe-agentloop-20260823_231433/integrations/openhands`，日志在 `/var/log/uenv/native-openhands-runs/...` | 原生 VeRL baseline 使用的 4 个 slot |

两类 poller 当前都注册到 `openhands-default` pool。后续对比实验必须避免不同版本的 poller 同时领取同一批 UEnv job，否则无法确认某条 episode 实际由哪套 driver 执行。

推荐做法：

| 方案 | 要求 |
| --- | --- |
| 独占标准 UEnv poller | 暂停或隔离 baseline poller；重启标准 UEnv poller，使其 `UENV_AGENT_BRIDGE_DIR` 指向包含 `effective_llm_config` 逻辑的新 bridge 目录 |
| 专用 agent pool | 为对齐实验创建专用 `agent_pool_id`，只让新版本 poller 注册到该 pool；数据 parquet 中写入该 pool，或由 Adapter 运行时覆盖为该 pool |

无论采用哪种方案，启动后都要从 208.77 job 目录中检查：

```text
agent_job.json.agent_bridge_id
config_snapshot.json.llm_model
effective_llm_config.json.base_url
runner_stdout.log / runner_stderr.log
```

只有确认 job 被预期 poller 接走，且模型 base_url 指向本轮训练机 gateway，才继续跑完整 20 step。

### 7.6 当前采用的专用 pool 方案

本次选择“专用 agent pool”方案，不暂停现有 `openhands-default` 旧 poller。已在 208.77 上新增 4 个非覆盖 runner 进程：

| 项目 | 值 |
| --- | --- |
| 专用 pool | `openhands-uenv-align-qwen` |
| agent id | `openhands-20877-align-slot1` ~ `openhands-20877-align-slot4` |
| server endpoint | `8.130.75.157:8088` |
| bridge 目录 | `/root/uenv-native-swe-agentloop-20260823_231433/integrations/openhands` |
| runner 日志 | `/var/log/uenv/openhands-uenv-align-qwen/slot*.runner.log` |
| job 输出目录 | `/var/log/uenv/openhands-uenv-align-qwen/slot*/agent-job-*` |

runner health 已显示：

```text
agent_pool_id = openhands-uenv-align-qwen
poll_enabled = true
registered = true
```

本地 Adapter 侧同步增加了 `UENV_AGENT_POOL_ID` 覆盖项。后续 UEnv 对齐实验启动时必须显式设置：

```text
UENV_AGENT_POOL_ID=openhands-uenv-align-qwen
UENV_MODEL_GATEWAY_PUBLIC_URL=http://127.0.0.1:18088/v1
```

这样即使复用旧 ordered parquet，`extra_info.agent_pool_id=openhands-default` 也不会影响实际下发的 `env_config.agent_pool_id`；但 `metadata.extra_info` 中仍保留原始数据，便于追踪数据来源。

`UENV_MODEL_GATEWAY_PUBLIC_URL` 使用 `127.0.0.1:18088` 是因为 208.77 上已有反向隧道监听：

```text
208.77:127.0.0.1:18088 -> 训练机:127.0.0.1:18088
```

这样 OpenHands 读取 `effective_llm_config.json.base_url` 后，会通过隧道访问本轮训练启动的 model gateway。

启动后第一批请求必须检查：

| 检查项 | 期望 |
| --- | --- |
| `agent-loop-requests.jsonl.payload.env_config.agent_pool_id` | `openhands-uenv-align-qwen` |
| 208.77 job 目录 | 出现在 `/var/log/uenv/openhands-uenv-align-qwen/slot*/` |
| 208.77 `effective_llm_config.json.base_url` | 指向本轮训练机 model gateway |
| 本地 `model-gateway.jsonl` | 出现 OpenHands `POST /v1/chat/completions` |
