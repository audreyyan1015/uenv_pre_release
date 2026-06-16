# VeRL 真实测 GSM8K — 调整清单

> **版本**：2026-06-13（A100 四端实机验收：单条 GSM8K + 1-step GRPO）  
> **背景**：MVP Phase 0 已跑通 `env_type=math` 四端链路；2026-06-12 起集成路径切换为 **VeRL `UEnvAgentLoop` 预 rollout**（替代已移除的 `UEnvBridgeRewardManager` + `verl.py`）。2026-06-13 在 7142/7143/`8.130.86.71` 完成 AgentLoop 全栈实机验收（OpenRouter + math 插件判分 + 1-step GRPO）。详见 [更新日志.md](./更新日志.md) `2026-06-13 01:00`。  
> **相关文档**：[260530-full-stack-integration-gaps.md](./260530-full-stack-integration-gaps.md)、[worker-pool-layer-design.md §9](./worker-pool-layer-design.md)、[PROTOCOL.md](../PROTOCOL.md)、[260609-worker-full-chain-integration-summary.md](./260609-worker-full-chain-integration-summary.md)

---

## 1. 现状与目标差距

### 1.1 当前能做什么（代码已落地）

| 能力 | 状态 |
|------|------|
| VeRL `data_source=gsm8k` → `env_type=math` | ✅ `uenv-bridge/src/uenv/bridge/verl_agent_loop.py` |
| GSM8K parquet 转 VeRL 样本 | ✅ `uenv-bridge/scripts/prepare_verl_gsm8k_sample.py` |
| AgentLoop 预 rollout → adapter-core → Server → Worker | ✅ `UEnvAgentLoop` + `RustCoreEpisodeClient` |
| Bridge L1 字段映射（question / dataset / rule_reward） | ✅ `uenv-bridge/core/src/core.rs`（`sample_to_worker_payload` / `sample_to_worker_reward_config`） |
| math 插件按题 reset + GSM8K 判分 | ✅ `uenv-math-plugin` + `plugins/math/.../gsm8k/` |
| Worker 心跳负载 / 多步执行 / StreamReport | ✅ W-6～W-9（2026-06-11） |
| Worker LLM 专用配置 | ✅ `config/uenv-worker-llm.env`（见 §4.3） |
| 默认链路切真实模式 | ✅ `UENV_AGENT_LOOP_CLIENT=rust_core`、`UENV_ADAPTER_CORE_BACKEND=server` |
| A100 四端联通 + smoke | ✅ 2026-06-13：`verify_pre_rollout_rust_core_loop.py` → `completed` |
| 单条 1-step GRPO（AgentLoop 全栈） | ✅ 2026-06-13：训练 `1/1`；对比跑 Worker `reward=1.0` |
| Worker LLM 完整 GSM8K prompt | ✅ 2026-06-13：`verl_agent_loop._worker_llm_question`（含 `####` 指令） |

### 1.2 仍待实机验收（非代码阻塞）

| 项 | 说明 |
|----|------|
| §5.1 链路级 | ≥2 道**不同** GSM8K 题（当前仅验收 1 道 Natalia 样本） |
| §5.2 Benchmark 级 | ≥100 样本 acc 可复现 |
| 规模化 §8 | 多 Worker、多 Episode 并发、Hub 热路径（代码骨架有，未 E2E） |

### 1.3 目标数据流（真实 GSM8K — AgentLoop 路径，2026-06-12 冻结）

```text
GSM8K parquet（question + #### answer）
  → VeRL rollout worker：UEnvAgentLoop（拦截本地 vLLM 生成）
  → build_episode_request（env_type=math, rubric_config.ground_truth）
  → RustCoreEpisodeClient.ExecuteBatch
  → adapter-core sample_to_worker_payload / sample_to_worker_reward_config
       question + dataset=gsm8k + rule_reward.target
  → Server 调度 → Worker
  → ModelClient 调 Worker 侧 LLM（config/uenv-worker-llm.env）生成 action
  → math 插件 reset（加载本题）→ step（answers_match 判分）
  → trajectory（含 response_text / 可选 response_ids）+ reward
  → AgentLoopOutput.reward_score → VeRL GRPO（reward_manager=naive）
```

**与旧路径差异（已废弃）**

| 旧（RewardManager） | 新（AgentLoop） |
|---------------------|----------------|
| VeRL vLLM 本地生成 → 解码 `uenv_response_text` → UEnv 只判分 | VeRL **不**在本地生成；UEnv/Worker **负责 rollout + 判分** |
| `verl.py` + `verl_reward_manager.py` | 已移除；见 `verl_agent_loop.py` |
| `UENV_BRIDGE_CLIENT=rust_core` | `UENV_AGENT_LOOP_CLIENT=rust_core` |
| Worker 原则上不调第二个 LLM | Worker **必须**配 LLM（`uenv-worker-llm.env`） |

**GSM8K 步数语义**：benchmark 为 **单轮 completion 判分**（`max_steps` 可 >1，但 math 插件第一步 `terminated=true`）；见 PROTOCOL §5「单轮」。

### 1.4 功能边界划分（平台 vs 环境）

| 层级 | 目录/组件 | 职责 | **不应包含** |
|------|-----------|------|--------------|
| **L1 Worker 平台** | `uenv-worker/src/episode/` | Episode 编排、`ModelClient` 调 LLM、`executor` reset/step、`RewardEngine` 采信插件 reward | GSM8K `####` 提取、dataset 判分规则 |
| **L2 MathEnv 制品** | `plugins/math/`、`uenv-math-plugin` | 题目加载、`dataset=gsm8k` 路由、`backends/gsm8k/scoring` 判分 | 调度、租约、StreamReport |
| **Bridge** | `uenv-bridge/core/src/core.rs` | VeRL envelope → L1 `payload` / `reward_config` 映射 | 环境内 step 语义 |
| **VeRL** | 7142 AgentLoop | 提交 Episode、消费 `AgentLoopOutput` | UEnv 环境判分 |

**判分权威（P-4，冻结）**：math 插件 `step.reward` 为唯一权威；`RewardEngine` 默认透传。

---

## 2. 分层调整清单

### 2.1 VeRL / 7142 运行环境（P0 — 部署）

| # | 调整项 | 说明 |
|---|--------|------|
| V-1 | **策略模型 + vLLM** | GRPO 训练容器内仍需 vLLM（AgentLoop 不替代训练侧引擎，但 rollout 生成交给 UEnv） |
| V-2 | **GSM8K 数据** | `train.parquet` / `test.parquet`；`prepare_verl_gsm8k_sample.py` |
| V-3 | **Python 依赖** | `grpcio`；`PYTHONPATH` 含 `uenv-bridge/src` |
| V-4 | **adapter-core 远端** | `UENV_ADAPTER_CORE_ENDPOINT=8.130.86.71:8088`；`UENV_ADAPTER_CORE_AUTO_START=0` |
| V-5 | **AgentLoop 客户端** | `UENV_AGENT_LOOP_CLIENT=rust_core`（默认已改，勿用 `fake`） |
| V-6 | **adapter-core backend** | `UENV_ADAPTER_CORE_BACKEND=server`（默认已改，勿用 `static_rollout`） |
| V-7 | **AgentLoop 注册** | `actor_rollout_ref.rollout.agent.default_agent_loop=uenv_agent`；配置 `configs/uenv-agent-loop.yaml` |
| V-8 | **容器 grpc** | VeRL 镜像内须 `grpcio>=1.80`（与 host 生成 stub 版本对齐）；脚本启动段已 `pip install` |
| V-9 | **GPU 选择** | 7142 默认 `CUDA_VISIBLE_DEVICES_IN_CONTAINER=0` 可能 OOM；实机用 **GPU 4**（`nvidia-smi` 选空闲卡） |
| V-10 | **response 长度** | `DATA_MAX_RESPONSE_LENGTH`（默认 32）；GSM8K 建议 **≥128**（对比跑 **256** 得 `reward=1`） |

参考脚本：`uenv-bridge/scripts/run_verl_grpo_1step_with_uenv_agent_loop.sh`；对比：`/root/UEnv/run-single-gsm8k-compare-7142.sh`（7142 实机）

---

### 2.2 Bridge / adapter-core（P0 — ✅ 2026-06-12 已落地）

| # | 文件/模块 | 状态 | 说明 |
|---|-----------|------|------|
| B-1 | `core.rs` `sample_to_worker_payload` | ✅ | 内联实现（原规划 `l1_mapping.rs` 已合并入 `core.rs`） |
| B-2 | `question` 映射 | ✅ | Core：`extra_info.question` → `raw_prompt` 回退；**2026-06-13** AgentLoop 构建时将 parquet **完整 user prompt**（含 `####` 指令）写入 `extra_info.question` |
| B-3 | rubric → rule_reward | ✅ | `rubric_config.ground_truth` → `rule_reward.target` |
| B-4 | `dataset` 规范化 | ✅ | `openai/gsm8k` 等 → `payload.dataset="gsm8k"` |
| B-5 | `sample_to_episode_request` | ✅ | 调用上述映射，不再传裸 `env_config` |
| B-6 | `response_text` | ✅ | `env_config.response_text` → payload 顶层（兼容后 rollout 判分路径） |
| B-7 | 批量 trace | 待办 P1 | `correlation_id` / sample_index 透传（代码有，规模化待验） |
| B-8 | 真实链路默认 | ✅ | 默认 `rust_core` + `server`；`static_rollout` 仅显式调试 |

**Worker 侧目标 proto 字段**（映射后）：

```json
// payload
{"question": "<GSM8K 题目>", "dataset": "gsm8k", "model_endpoint": "...", "model_name": "..."}

// reward_config
{"type": "rule_reward", "target": "<#### 后标准答案>"}
```

单测：`execute_batch_maps_verl_math_payload_to_worker_contract`（`uenv-bridge/core`）。

---

### 2.3 Worker 运行时（P0）

| # | 文件 | 状态 | 说明 |
|---|------|------|------|
| W-1 | `model_client.rs` | ✅ | 首步优先 `response_text`；有 LLM 配置时调 HTTP 生成 |
| W-2 | `model_client.rs` | ✅ 2026-06-15 | **优先使用 Episode `model_endpoint` / `model_name` / `generation_config`**；API Key 仍来自 `uenv-worker-llm.env`；仅无 LLM 且无 question 时 `rule_reward` 短路 |
| W-3 | `reward_engine.rs` | ✅ | 采信插件 `step.reward` |
| W-4 | `executor.rs` / `payload.rs` | ✅ | reset 传题；`dataset` 二次规范化；`step.info.response_text` |
| W-5 | 7143 部署 | ✅ 2026-06-13 | `UENV_MATH_PLUGIN_BIN`、`UENV_PREWARM_ON_STARTUP`、`UENV_HUB_TOKEN`；`plugins/math/run.sh` 须 **LF**（禁 CRLF） |
| W-12 | Worker LLM `.env` | ✅ 2026-06-12 | 默认 **OpenRouter**；`config/uenv-worker-llm.env`；详见 [secrets/README.md §1.7](../secrets/README.md) |
| W-13 | Worker LLM prompt | ✅ 2026-06-13 | `verl_agent_loop._worker_llm_question`：Worker OpenRouter 收到与 parquet `prompt` 一致的题干 |

**LLM 配置（7143，OpenRouter）**：

```bash
cp config/uenv-worker-llm.env.example config/uenv-worker-llm.env
# 编辑 UENV_LLM_API_KEY（勿提交仓库）
```

| 变量 | 默认值 | 含义 |
|------|--------|------|
| `UENV_LLM_PROVIDER` | `openrouter` | LLM 提供商 |
| `UENV_LLM_ENDPOINT` | `https://openrouter.ai/api/v1` | OpenRouter API |
| `UENV_LLM_MODEL_NAME` | `qwen/qwen-2.5-7b-instruct`（模板默认）；实机可用 `qwen/qwen3-max` | 模型 slug；**非** `qwen3-max-thinking`（无独立 thinking 通道） |
| `UENV_LLM_API_KEY` | — | **必填** |
| `UENV_LLM_HTTP_REFERER` / `UENV_LLM_APP_TITLE` | 可选 | OpenRouter 归因头 |
| `UENV_WORKER_LLM_ENV` | `config/uenv-worker-llm.env` | 覆盖 env 文件路径 |

---

### 2.4 math 插件（P0 — ✅）

| # | 项 | 状态 |
|---|-----|------|
| P-1 | reset 读 `{uds}.episode.json` 注入 `question` | ✅ |
| P-2 | `dataset=gsm8k` 路由 | ✅ |
| P-3 | `answers_match`（`####` 提取） | ✅ |
| P-4 | 插件 `step.reward` 权威 | ✅ 冻结 |
| P-5 | `uenv-math-env` crate 结构 | ✅ |
| P-6 | `step.info.response_text` | ✅ 2026-06-12 |

---

### 2.5 Hub / Server / 部署（P1）

| # | 调整项 | 说明 |
|---|--------|------|
| H-1～H-4 | Hub / Server 地址 | 见 `config/uenv-worker.deploy-7143.yaml` |
| H-5 | AgentLoop 三联调实机记录 | ✅ 2026-06-13：7142→`8.130.86.71:8088`→7143；日志见 §5.4 |
| H-6 | Hub 热路径拉制品 | 规划项，见 §8 |

---

## 3. 推荐实施顺序（2026-06-12 更新）

```text
Phase A（代码）— ✅ 已完成
  B-1～B-6、W-1～W-4、W-12、P-1～P-6、默认真实链路

Phase B（实机 smoke）— ✅ 2026-06-13 基本完成
  7143：uenv-worker-llm.env + math 插件 + OpenRouter
  71：adapter-core 拉起（未改代码）+ Worker 注册
  7142：smoke + 单条样本；§5.1 尚差「≥2 道不同题」

Phase C（Benchmark + GRPO）— 部分 ✅
  单条 1-step GRPO 已跑通（§5.4）；≥100 样本 acc 仍待 §5.2

Phase D～F（规模化）
  见 §8：多 Episode / 多 Worker / Hub 热路径
```

---

## 4. LLM 配置说明

### 4.1 结论摘要（AgentLoop 路径）

| 场景 | VeRL 容器 vLLM | Worker 侧 LLM（`uenv-worker-llm.env`） |
|------|----------------|----------------------------------------|
| **AgentLoop 真实 GSM8K 全栈** | 训练侧需要；**rollout 由 Worker 调 OpenRouter** | **必须**（`UENV_LLM_API_KEY`） |
| **grpcurl 直打 Worker（无 LLM）** | 不需要 | 不需要（`rule_reward` 短路） |
| **Python 单测** | 不需要 | 不需要（注入 `RecordingEpisodeClient`） |

### 4.2 VeRL 侧

- 容器内仍需策略模型与 vLLM 引擎（GRPO 训练、logprob 等）。
- **Rollout completion 不在 VeRL 本地生成**：`UEnvAgentLoop.run()` 将样本交给 UEnv，从 `EpisodeResult` 取 `response_ids` / `reward_score`。

### 4.3 Worker 侧 — 必须配置 LLM（全栈 AgentLoop）

1. AgentLoop 提交的 Episode **不含**预生成 `response_text`
2. `ModelClient` 使用 `config/uenv-worker-llm.env` 调 **OpenRouter** `POST /chat/completions`
3. 生成文本作为 action → math 插件 GSM8K 判分
4. `step.info.response_text` 回传；AgentLoop 可 fallback 用 tokenizer 编码为 `response_ids`

**注意**：`UENV_ROLLOUT_MODEL_ENDPOINT` / `model_name` 由 Adapter 写入 Episode；Worker `ModelClient` **优先应用 Episode 配置**调 OpenRouter。`UENV_LLM_API_KEY` 仅在 Worker 本地 env，不进 Bridge。

### 4.4 已移除的假链路

以下模式**不应**用于生产 GSM8K 评测（仅保留代码供显式单测注入）：

| 模式 | 原用途 | 现状 |
|------|--------|------|
| `UENV_AGENT_LOOP_CLIENT=fake` | 本地假 rollout | 默认改为 `rust_core` |
| `UENV_ADAPTER_CORE_BACKEND=static_rollout` | adapter-core 返回固定 token/reward | 默认改为 `server` |
| `UENVBridgeRewardManager` / `math_proxy` | 不经 Worker 打分 | 已移除 |
| `ADAPTER_CORE_REWARD_MODE=fixed` | 假 reward | 已废弃 |

### 4.5 实机踩坑：`reward=0` 不等于链路失败（2026-06-13）

| 现象 | 原因 | 处置 |
|------|------|------|
| Worker `reward=0` 但 `dispatch_completed` | GSM8K `answers_match` 判错（业务语义） | 查 `target`、模型输出是否含 `#### <答案>` |
| `response_length/clip_ratio=1.0` | **顶满** `data.max_response_length`（默认 32），非溢出 | 提高 `DATA_MAX_RESPONSE_LENGTH`（对比 **256**） |
| 模型长推理但无 `#### 72` | ① token 太短；② 曾只传裸 `question` 无格式指令 | W-13 + `DATA_MAX_RESPONSE_LENGTH` |
| `plugin math-1 not ready` | Windows 同步的 `run.sh` 带 CRLF | 7143 上 `sed -i 's/\r$//'` |
| vLLM Engine core init failed | GPU0 仅剩 ~2GiB 空闲 | `CUDA_VISIBLE_DEVICES_IN_CONTAINER=4` |
| `RustCoreEpisodeClient requires stub` | 容器 `grpcio` 1.76 与 stub 不兼容 | 容器内 `pip install grpcio>=1.80` |

---

## 5. 验收标准

### 5.1 链路级（Phase B）

- [ ] 输入 2 道不同 GSM8K 题，Worker 日志中 **不同** `question` / `target`
- [x] 同一题配置不当 → `reward=0`；修正配置后 → `reward=1`（2026-06-13，Natalia / `target=72`）
- [x] 使用 parquet 真实题干，**非** stub 默认题 `"If 3 books cost $12..."`
- [x] AgentLoop → Worker → OpenRouter → math 插件全链路 `completed`（smoke + GRPO）

### 5.2 Benchmark 级（Phase C）

- [ ] `test.parquet` 全量或 ≥100 样本，acc 可复现
- [x] 单样本 `AgentLoopOutput.reward_score` 与 Worker 一致（对比跑均为 **1.0**，非固定常数 stub）
- [x] 1-step GRPO 完成且 reward 来自 Worker math 插件（`critic/rewards/mean=1.0`）

### 5.3 规模化级（Phase D～F，见 §8）

- [ ] 多 Worker / 多 Episode 并发 / Hub 热路径 / batch 并发（与 2026-06-11 §8 相同，仍待 E2E）

### 5.4 实机记录摘要（2026-06-13，A100）

| 轮次 | 配置要点 | Worker reward | VeRL `critic/rewards/mean` | 日志（7142） |
|------|----------|---------------|----------------------------|--------------|
| smoke | `verify_pre_rollout_rust_core_loop.py` | 0.0（模型未按 `####` 答对） | — | — |
| GRPO v1 | `max_response_length=32`，裸 `question` | **0.0** | **0.0** | `single-gsm8k-grpo-gpu4-v3.log` |
| GRPO 对比 | `DATA_MAX_RESPONSE_LENGTH=256`，完整 prompt + `####` 指令，GPU 4 | **1.0** | **1.0** | `single-gsm8k-compare-256.log` |

样本：GSM8K train 第 1 条（Natalia，`ground_truth=72`）。7143 episode 配置示例：

```json
{"dataset":"gsm8k","question":"... May? Let's think step by step and output the final answer after ####.","target":"72"}
```

---

## 6. 快速对照：关键文件

| 优先级 | 路径 |
|--------|------|
| ~~P0~~ ✅ | `uenv-bridge/core/src/core.rs`（L1 映射） |
| ~~P0~~ ✅ | `uenv-bridge/src/uenv/bridge/verl_agent_loop.py` |
| ~~P0~~ ✅ | `uenv-bridge/src/uenv/bridge/agent_loop_clients.py` |
| ~~P0~~ ✅ | `uenv-bridge/configs/uenv-agent-loop.yaml` |
| ~~P0~~ ✅ | `uenv-worker/src/episode/model_client.rs` |
| ~~P0~~ ✅ | `uenv-worker/src/llm.rs` + `config/uenv-worker-llm.env.example` |
| ~~P0~~ ✅ | `uenv-worker/src/bin/uenv-math-plugin.rs` |
| 部署 | `config/uenv-worker.deploy-7143.yaml`、`config/uenv-worker-llm.env` |
| 脚本 | `run_verl_grpo_1step_with_uenv_agent_loop.sh`、`verify_pre_rollout_rust_core_loop.py`、`run-single-gsm8k-compare-7142.sh`（实机） |
| P2 | Hub 热路径、多 Worker 规模化（§8） |

**已移除**：`verl.py`、`verl_reward_manager.py`、`run_verl_grpo_1step_with_bridge_reward.sh`（RewardManager 路径）。

---

## 7. 相关命令备忘

```bash
# 准备 GSM8K VeRL 样本
python uenv-bridge/scripts/prepare_verl_gsm8k_sample.py \
  --input /workspace/data/gsm8k/test.parquet \
  --output ./tmp/gsm8k_test.parquet \
  --n 100

# 7143 Worker：LLM + 插件
cp config/uenv-worker-llm.env.example config/uenv-worker-llm.env
# 编辑 UENV_LLM_API_KEY（见 secrets/README.md §1.7）
export UENV_MATH_PLUGIN_BIN=/path/to/uenv-math-plugin
export UENV_PREWARM_ON_STARTUP=true
export UENV_HUB_TOKEN=<from Hub>

# 7142：AgentLoop 全栈
export UENV_AGENT_LOOP_CLIENT=rust_core
export UENV_ADAPTER_CORE_ENDPOINT=8.130.86.71:8088
export UENV_ADAPTER_CORE_AUTO_START=0
export UENV_ADAPTER_CORE_BACKEND=server
export CUDA_VISIBLE_DEVICES_IN_CONTAINER=4   # 按 nvidia-smi 选空闲 GPU

# 预 rollout 链路验证（需已注册 Worker）
PYTHONPATH=uenv-bridge/src python uenv-bridge/scripts/verify_pre_rollout_rust_core_loop.py

# 1-step GRPO smoke（默认 max_response_length=32）
uenv-bridge/scripts/run_verl_grpo_1step_with_uenv_agent_loop.sh

# 1-step GRPO 对比跑（推荐 GSM8K 实机参数）
export DATA_MAX_RESPONSE_LENGTH=256
export SAMPLE_COUNT=1 TRAIN_BATCH_SIZE=1 TRAINING_STEPS=1
bash /root/UEnv/run-single-gsm8k-compare-7142.sh
```

---

## 8. 规模化真实训练待补齐规划

> 与 2026-06-11 版 §8 一致；代码骨架（W-6～W-9、Server `join_all`、Worker `Semaphore`）已具备，**实机 E2E 仍待验收**。  
> GSM8K benchmark 本身为单步判分；`max_steps>1` 对 math 环境无多轮交互语义。

### 8.1～8.3 摘要

| 阶段 | 内容 | 状态 |
|------|------|------|
| Phase D | 真实 GSM8K + 平台多步循环 | 代码 ✅；单条 AgentLoop+GRPO E2E ✅（2026-06-13）；批量待验 |
| Phase E | 多 Worker / 多 Episode | 代码 ✅；E2E 待验 |
| Phase F | Hub 热路径 | 待开发/验收 |

**Bridge 勾选更新（2026-06-12）**

- [x] B-1～B-6：Bridge payload 含 `question` / `dataset=gsm8k` / `rule_reward.target`
- [x] B-8：默认 `rust_core` + `server` backend
- [ ] H-6、S-2、S-3、V-7：规模化实机（不变）

---

*维护：2026-06-13 单条全栈已验收；规模化/Benchmark 通过后请继续同步 [260530-full-stack-integration-gaps.md](./260530-full-stack-integration-gaps.md)、[260609-worker-full-chain-integration-summary.md](./260609-worker-full-chain-integration-summary.md) 与 [更新日志.md](./更新日志.md)。*
