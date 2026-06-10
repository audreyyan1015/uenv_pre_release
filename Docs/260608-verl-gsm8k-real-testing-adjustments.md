# VeRL 真实测 GSM8K — 调整清单

> **版本**：2026-06-08  
> **背景**：MVP Phase 0 已跑通 `env_type=math` 四端链路（7142 VeRL → 8.130.86.71 adapter-core → 7143 Worker → math 插件），但 **GSM8K benchmark 语义尚未真实落地**。本文列出若要用 VeRL **真实测量 GSM8K 表现**（而非 stub 联调）需要调整的位置、优先级与验收标准。  
> **相关文档**：[260530-full-stack-integration-gaps.md](./260530-full-stack-integration-gaps.md)、[worker-pool-layer-design.md §9](./worker-pool-layer-design.md)、[PROTOCOL.md](../PROTOCOL.md)、[260609-worker-full-chain-integration-summary.md §1.2](./260609-worker-full-chain-integration-summary.md)

---

## 1. 现状与目标差距

### 1.1 当前能做什么

| 能力 | 状态 |
|------|------|
| VeRL `data_source=gsm8k` → `env_type=math` 映射 | ✅ `uenv-bridge/src/uenv/bridge/verl.py` |
| GSM8K parquet 转 VeRL 样本 | ✅ `uenv-bridge/scripts/prepare_verl_gsm8k_sample.py` |
| 四端 math 链路 E2E（固定 stub 题/答） | ✅ 已验收 `reward=1.0` |
| Bridge 本地 rubric 打分（不经 Worker） | ✅ `UENV_BRIDGE_CLIENT=math_proxy` |

### 1.2 当前不能代表真实 GSM8K 的原因

| 缺口 | 位置 | 影响 |
|------|------|------|
| math 插件写死固定题/答 `"20"` | `uenv-worker/src/bin/uenv-math-plugin.rs` | 所有样本同一道题 |
| `reset` 不读 Episode payload | 同上 + `episode/executor.rs` | 真实 GSM8K 题目无法注入环境 |
| Bridge → Worker payload 未映射 `question` + `rule_reward` | `uenv-bridge/core/src/core.rs`（缺 `l1_mapping.rs`） | Worker 判分格式与 Bridge rubric 不一致 |
| Worker 不消费 VeRL 已生成的 `response_text` | `uenv-worker/src/episode/model_client.rs` | 全链路时会重复调 LLM 或直接失败 |
| adapter-core 无独立 serve 模式开关 | `uenv-bridge/core/src/main.rs` | 脚本默认 `ADAPTER_CORE_REWARD_MODE=fixed`，非真实 Worker reward |

### 1.3 目标数据流（真实 GSM8K）

```text
GSM8K parquet（question + #### answer）
  → VeRL rollout（vLLM 生成 response_text）
  → UEnvBridgeRewardManager（解码 token → uenv_response_text）
  → VeRLAdapter（env_type=math, dataset=gsm8k, ground_truth）
  → adapter-core L1 映射（question + rule_reward.target）
  → Server 调度 → Worker
  → math 插件 reset（加载本题）→ step（比对 VeRL 答案 vs ground_truth）
  → reward 回写 VeRL rm_scores / acc
```

---

## 2. 分层调整清单

### 2.1 VeRL / 7142 运行环境（P0 — 部署，非代码）

| # | 调整项 | 说明 |
|---|--------|------|
| V-1 | **策略模型 + vLLM** | GRPO/评测需 VeRL 容器内 vLLM rollout；见 §4 LLM 说明 |
| V-2 | **GSM8K 数据** | `train.parquet` / `test.parquet`；可用 `prepare_verl_gsm8k_sample.py` 裁剪样本 |
| V-3 | **Python 依赖** | 7142 需 `grpcio` 等；`PYTHONPATH` 含 `uenv-bridge/src` |
| V-4 | **adapter-core 远端地址** | `UENV_ADAPTER_CORE_ENDPOINT=8.130.86.71:8088`（勿在 7142 再起 core） |
| V-5 | **RewardManager 模式** | 全栈：`UENV_BRIDGE_CLIENT=rust_core`；仅验 Bridge 接线：`math_proxy` |
| V-6 | **关闭 fake reward** | 勿用 `ADAPTER_CORE_REWARD_MODE=fixed`；需 Worker 真实返回（见 B-3） |

参考脚本：`uenv-bridge/scripts/run_verl_grpo_1step_with_bridge_reward.sh`

---

### 2.2 Bridge / adapter-core（P0 — 阻塞全栈 GSM8K）

| # | 文件/模块 | 调整内容 |
|---|-----------|----------|
| B-1 | **新增** `uenv-bridge/core/src/l1_mapping.rs` | 文档已引用但仓库缺失；实现 VeRL payload → L1 `EpisodeRequest` |
| B-2 | `l1_mapping` 字段映射 | `env_config.raw_prompt` / `extra_info.question` → Worker `payload.question` |
| B-3 | rubric → rule_reward | `reward_config.rubric_config.ground_truth` → `{"type":"rule_reward","target":"<gt>"}` |
| B-4 | dataset 字段 | `metadata.data_source` 含 gsm8k 时写 `payload.dataset="gsm8k"`（PROTOCOL 约定） |
| B-5 | `core.rs` `sample_to_episode_request` | 调用 `l1_mapping`，勿仅传 `env_config` 裸 JSON |
| B-6 | VeRL 预生成答案 | 若 `env_config.response_text` 存在，映射到 Worker 可读的 action 来源（见 W-2） |

**当前 `core.rs` 行为（需改）**：

```rust
// 现状：proto.payload = env_config only；reward_config = rubric 原样
payload: payload.get("env_config")...
reward_config: payload.get("reward_config")...
```

**目标 Worker 侧 proto 字段**：

```json
// payload（bytes）
{"question": "<GSM8K 题目文本>", "dataset": "gsm8k", "response_text": "<VeRL rollout 解码文本>"}

// reward_config（bytes）
{"type": "rule_reward", "target": "<#### 后标准答案>"}
```

---

### 2.3 Worker 运行时（P0）

| # | 文件 | 调整内容 |
|---|------|----------|
| W-1 | `episode/model_client.rs` | **优先**使用 `payload.response_text`（VeRL 已 rollout）作为 action；仅无 response 时才调 LLM |
| W-2 | `episode/model_client.rs` | 兼容 `rule_reward`：若仅有 target 且无 response，可 short-circuit（测试用） |
| W-3 | `episode/reward_engine.rs` | 可选：同时识别 `rubric_config.ground_truth`（防御性）；或完全依赖 B-3 映射 |
| W-4 | `episode/executor.rs` | `reset` 时将题目/seed/dataset 传入插件（扩展 Plugin IPC 或经 UDS 侧 channel） |
| W-5 | 7143 部署 env | `UENV_MATH_PLUGIN_BIN`、`UENV_PREWARM_ON_STARTUP=true`、Hub token（见 [secrets/README.md](../secrets/README.md)） |

**VeRL 路径下 Worker 不应再调第二个 LLM**（见 §4）。

---

### 2.4 math 插件（P0 — 真实 benchmark 核心）

| # | 文件 | 调整内容 |
|---|------|----------|
| P-1 | `uenv-math-plugin.rs` | `reset` 从请求/read 侧获取本题 `question`（非写死字符串） |
| P-2 | 按 `dataset` 路由 | `dataset=gsm8k` 走 GSM8K 解析逻辑（可先做单 backend，不必多目录） |
| P-3 | `step` 判分 | 提取 `####` 后数字/表达式，与 `expected` 比较（对齐 VeRL `extract_solution`） |
| P-4 | 与 Worker reward 分工 | **二选一冻结**：插件 `step.reward` 为准，或 Worker `RewardEngine` 为准；避免双处不一致 |
| P-5 | 可选 | `plugins/math/backends/gsm8k/` 独立模块（Phase 1 结构，MVP 可内联在 binary） |

**当前 stub（必须替换）**：

```rust
// uenv-worker/src/bin/uenv-math-plugin.rs
s.question = "If 3 books cost $12, ...".to_string();
s.answer = "20".to_string();
```

---

### 2.5 Hub / Server / 部署（P1 — 不阻塞首轮 benchmark）

| # | 调整项 | 说明 |
|---|--------|------|
| H-1 | Hub manifest | 已有 `math` seed；无需为 GSM8K 单独 env_type |
| H-2 | 制品同步 P2-2 | Hub 下发插件包仍未实现；继续用 Worker 本地 `plugins/` + `UENV_MATH_PLUGIN_BIN` |
| H-3 | `8.130.86.71` | `uenv-adapter-core` 监听 `8088`；Worker `server.endpoint` 指向此处 |
| H-4 | `8.130.95.176` Hub | Worker `hub.enabled` + `UENV_HUB_TOKEN` 拉 manifest |
| H-5 | Bridge serve 三联调 | P0-8：实机记录 adapter-core → Worker 全链路 GSM8K 样本 |

配置参考：`config/uenv-server.deploy.yaml`、`config/uenv-worker.deploy-7143.yaml`

---

## 3. 推荐实施顺序

```text
Phase A（最小可测 GSM8K acc，可暂不训 GRPO）
  B-1～B-6  l1_mapping + core 接入
  W-1       model_client 读 response_text
  W-3       reward_engine 对 ground_truth 判分
  → VeRL 单步 rollout + rust_core + Worker，对比 gsm8k ground_truth 算 acc

Phase B（插件语义完整）
  P-1～P-4  math 插件读题 + GSM8K 判分
  W-4       reset 传题给插件

Phase C（训练闭环）
  V-1～V-6  7142 完整 GRPO + 远端 core
  H-5       三联调验收 + 记录 logs

Phase D～F（规模化真实训练）
  见 §8.3：多 step → 多 Worker/多 Episode → Hub 热路径
```

---

## 4. LLM 配置说明（是否必须）

### 4.1 结论摘要

| 场景 | VeRL 侧 LLM（vLLM） | Worker 侧 LLM |
|------|---------------------|---------------|
| **VeRL GRPO 训练 / 带 rollout 的 GSM8K 评测** | **必须** | **不应需要**（改 W-1 后） |
| **Bridge-only：`math_proxy`** | 必须（VeRL 仍要生成答案） | 不需要 |
| **Bridge-only：`fixed` fake reward** | 必须（rollout 仍跑） | 不需要 |
| **无 VeRL、grpcurl 直打 Worker** | 不需要 | **可能需要**（当前 `model_client` 缺 `response_text` 时会调 HTTP LLM） |

### 4.2 VeRL 侧 — 必须配置 LLM

VeRL 的 GSM8K 流程是：

1. 读入 prompt（GSM8K 题目）
2. **策略模型通过 vLLM rollout 生成 completion**
3. `UEnvBridgeRewardManager` 解码 `responses` token → `uenv_response_text`
4. 再把样本交给 UEnv 环境 **打分**

因此只要走 **真实 VeRL 训练或评测**，就必须在 VeRL 容器/7142 上配置：

- 模型权重（如 `Qwen2.5-0.5B-Instruct`）
- vLLM rollout（`actor_rollout_ref.rollout.name=vllm`）
- GPU（`CUDA_VISIBLE_DEVICES`）

这与 UEnv Worker **无关**；UEnv 只负责对已生成答案判分。

### 4.3 Worker 侧 — 正常不应再配 LLM

设计意图（VeRL 集成）：

- **答案由 VeRL rollout 产生**，经 Bridge 写入 `env_config.response_text`
- Worker 应 **直接拿该文本做 rule_reward 匹配**，不应再调 `model_endpoint/chat/completions`

当前代码 gap：`model_client.rs` 未读 `response_text`，且 `core.rs` 传给 Worker 的 `payload` 不含完整字段 → 可能误走 LLM 路径或失败。

**修复 W-1 + B-1～B-6 后，测 GSM8K 不需要在 Worker/7143 上单独部署 LLM。**

### 4.4 例外：纯 Worker 联调（不经 VeRL）

若用 grpcurl / fixture 直调 Server→Worker，且 payload 里没有 `response_text`，则 `model_client` 会尝试 HTTP 调 LLM（需 `payload.model_endpoint` + `question`）。这不是 VeRL 测 GSM8K 的推荐路径。

---

## 5. 验收标准

### 5.1 链路级（Phase A 完成）

- [ ] 输入 2 道不同 GSM8K 题，Worker 日志中 **不同** `question` / `target`
- [ ] VeRL 故意答错一题 → `reward=0`；答对 → `reward=1`
- [ ] `episode_results.jsonl` 中 acc 与手工核对 `ground_truth` 一致
- [ ] 不再依赖 stub 固定题 `"If 3 books cost $12..."`

### 5.2 Benchmark 级（Phase B+C 完成）

- [ ] `test.parquet` 全量或 ≥100 样本，acc 可复现（记录模型名、checkpoint、seed）
- [ ] VeRL `critic/score/mean` 与 UEnv 返回 reward 均值一致（非 `fixed` 常数）
- [ ] 1-step GRPO 能完成且 reward 来自 Worker 而非 `ADAPTER_CORE_FAKE_REWARD`

### 5.3 规模化级（Phase D～F 完成，见 §8）

- [ ] 单 Worker 多 step：`max_steps>1` 的 GSM8K Episode 完整轨迹可回放
- [ ] 多 Worker：≥2 Worker 并行接单，Server 日志可见分发到不同 `worker_id`
- [ ] 多 Episode 并发：单 Worker 并行 Episode 数达到 `max_concurrent` 且无 lease 冲突
- [ ] Hub 热路径：spawn 阶段从 Hub 拉取插件/镜像，不依赖节点预置 `UENV_MATH_PLUGIN_BIN`
- [ ] VeRL batch 并发：batch size > 1 时 reward/acc 与 Worker 侧一致

---

## 6. 快速对照：改哪些文件

| 优先级 | 路径 |
|--------|------|
| P0 | `uenv-bridge/core/src/l1_mapping.rs`（新建） |
| P0 | `uenv-bridge/core/src/core.rs` |
| P0 | `uenv-bridge/core/src/lib.rs` |
| P0 | `uenv-worker/src/episode/model_client.rs` |
| P0 | `uenv-worker/src/bin/uenv-math-plugin.rs` |
| P1 | `uenv-worker/src/episode/executor.rs` |
| P1 | `uenv-worker/src/episode/reward_engine.rs` |
| P1 | `uenv-worker/src/control_plane/client.rs`（W-6 load、W-7 resource） |
| P1 | `uenv-worker/src/grpc_server/worker_service.rs`（W-8 StreamReport） |
| P2 | `uenv-worker/src/hub/mod.rs`（H-6 热路径拉制品） |
| P2 | `uenv-server/` 调度器（S-1～S-3 多 Worker/并发 Episode） |
| 规划 | [260609-worker-full-chain-integration-summary.md §1.2](./260609-worker-full-chain-integration-summary.md) → §8 待优化清单 |
| 部署 | `config/uenv-worker.deploy-7143.yaml`、`secrets/README.md` §1.6 |
| 脚本 | `uenv-bridge/scripts/run_verl_grpo_1step_with_bridge_reward.sh`（endpoint + 去 fixed） |

---

## 7. 相关命令备忘

```bash
# 准备 GSM8K VeRL 样本
python uenv-bridge/scripts/prepare_verl_gsm8k_sample.py \
  --input /workspace/data/gsm8k/test.parquet \
  --output ./tmp/gsm8k_test.parquet \
  --n 100

# 7142：全栈 reward（需 Phase A 代码落地后）
export UENV_BRIDGE_CLIENT=rust_core
export UENV_ADAPTER_CORE_ENDPOINT=8.130.86.71:8088
export UENV_ADAPTER_CORE_AUTO_START=0   # 使用远端 core，勿本地起

# 7143 Worker 必配
export UENV_MATH_PLUGIN_BIN=/path/to/uenv-math-plugin
export UENV_PREWARM_ON_STARTUP=true
export UENV_HUB_TOKEN=<from Hub>
```

---

## 8. 规模化真实训练待补齐规划

> **来源**：[260609-worker-full-chain-integration-summary.md §1.2](./260609-worker-full-chain-integration-summary.md)（全链路联调 Mock/Stub 逐项转化）  
> **目标**：Hub 参与 Episode **热路径**；支持 **多 Worker、多环境实例、多 step、多 Episode** 并发的真实 GSM8K 训练/评测（非单 Worker 单步 stub）。  
> **与 §2 关系**：§2 聚焦「首轮真实 GSM8K 判分」最小闭环；本节聚焦规模化与热路径制品/调度缺口。

### 8.1 Worker 层待优化（`uenv-worker`）

| # | 来源（§1.2） | 现状 | 待办 | 优先级 | 规模化关联 |
|---|--------------|------|------|--------|------------|
| W-1 | `ModelClient` | `rule_reward` + `target` 时直接把 target 当 action，不调 LLM | 优先读 `payload.response_text`（VeRL rollout）；无 response 再调 LLM；保留 short-circuit 仅作联调 | P0 | 多 Episode 并发时不重复调 LLM |
| W-2 | `ModelClient` | 同上（测试捷径） | 与 W-1 合并验收：VeRL 路径禁止误走 LLM | P0 | 见 §2.3 |
| W-3 | `RewardEngine` | 仅 `rule_reward`；Bridge `rubric_config` 未映射时 fallback 插件 step reward | 识别 `rule_reward` / 防御性兼容 `ground_truth`；与 P-4 冻结单一判分源 | P0 | 多 step 每步 reward 一致 |
| W-4 | `uenv-math-plugin`（reset 传题） | reset 不传 Episode payload | `executor` reset 时将 `question`/`dataset`/`seed` 传入插件 IPC | P0 | 多环境实例各自加载不同题目 |
| W-6 | 心跳 `load` | 恒为 `0` | 上报真实活跃 Episode 数（如 `Semaphore` 占用或 executor 计数） | P1 | **多 Worker** 负载感知调度前提 |
| W-7 | `RegisterWorker.resource` | 发送 `None` | 填充 `ResourceSpec`（CPU/内存/GPU）；与 7143 实机资源对齐 | P1 | Server 按资源筛选 Worker |
| W-8 | `StreamReport` | 仅填 `phase`；`report_type` 等 PRD 字段默认 | 多 step 时按步推送 `PROGRESS`/`STEP_COMPLETE`；填 `worker_active_episodes`、`correlation_id` 等 | P1 | **多 step** 流式进度；VeRL/Server 可观测 |
| W-9 | Episode 步数 | 仅 `execute_single_round`（1 step） | 实现 `execute_multi_step`：`reset → (infer → step)* → release`；尊重 `max_steps`/`terminated` | P0 | **多 step** Agent 训练核心 |
| W-10 | Podman 后端 | 7143 仅用 `process` | 验收 Podman 插件 spawn/teardown；Hub 镜像 digest 驱动容器 backend | P2 | **多环境实例** 隔离与扩缩 |
| W-11 | `registry/worker_pool.rs` | 占位注释 | 实现或移除占位；若保留则接入实例池 metrics/注册（与 warmup_pool 职责对齐） | P2 | 多实例池可观测与调试 |

**Worker 层验收勾选（规模化）**

- [ ] W-6：`WorkerHeartbeat.load` 随并发 Episode 变化，非恒 0
- [ ] W-7：`RegisterWorker` 携带非空 `resource`
- [ ] W-8：多 step Episode 产生 ≥2 条 `StreamReport`，且 `report_type` 非 UNSPECIFIED
- [ ] W-9：同一 Episode `max_steps>1` 时日志可见多轮 step，最终 `trajectory.steps` 长度 > 1
- [ ] W-10（可选）：Podman backend 跑通 math 插件一 Episode

---

### 8.2 其他模块层待优化（Bridge / Hub / Server / math 插件 / VeRL）

#### 8.2.1 math 插件（`uenv-math-plugin` / `plugins/math`）

| # | 来源（§1.2） | 现状 | 待办 | 优先级 | 规模化关联 |
|---|--------------|------|------|--------|------------|
| P-1 | `uenv-math-plugin` | reset 写死固定题，答案恒 `"20"` | reset 从 IPC/Worker 读取本题 `question` | P0 | 每 Episode 不同 GSM8K 题 |
| P-2 | 同上 | 不读 Episode `payload` | 支持 `dataset=gsm8k` 路由与题目注入 | P0 | 真实 benchmark |
| P-3 | 同上 | stub 判分 | `step` 提取 `####` 后与 `expected` 比较（对齐 VeRL `extract_solution`） | P0 | 多 step 每步可判分 |
| P-4 | 与 W-3 分工 | 双处判分风险 | 冻结：插件 `step.reward` **或** Worker `RewardEngine` 唯一为准 | P0 | 避免多 step reward 漂移 |
| P-5 | 结构 | 单 binary 内联 | 可选 `plugins/math/backends/gsm8k/` 模块化 | P2 | 多 dataset 扩展 |

#### 8.2.2 Bridge / adapter-core（`uenv-bridge`）

| # | 来源（§1.2） | 现状 | 待办 | 优先级 | 规模化关联 |
|---|--------------|------|------|--------|------------|
| B-1～B-6 | `RewardEngine` / payload | 见 §2.2 | 落地 `l1_mapping`：`question` + `rule_reward` + `response_text` | P0 | 多 Episode 批量样本字段完整 |
| B-7 | 规模化 | 单样本联调为主 | 批量 Episode 请求时保持 `correlation_id`/样本 id 透传至 Worker | P1 | **多 Episode** trace |
| B-8 | adapter-core | 无独立 serve 模式文档化 | 明确 `rust_core` + 远端 Worker reward；禁止生产路径 `fixed` | P0 | 训练闭环 |

#### 8.2.3 Hub（`uenv-hub`）— 热路径参与

> **职责边界（math 环境与 Hub）**  
> Hub 是 **环境元数据注册中心**（版本、manifest、`image.url/digest`、interface schema、resources），类比 Docker Hub 索引层；**不参与** Episode 调度，**不在** reset/step 热路径执行业务逻辑。  
> `env_type=math` 仍是 L1 调度键；GSM8K 为 payload 内 `dataset=gsm8k`，**无需**在 Hub 单独注册第二个 env_type。

**要迁移 / 补齐的（Phase F 及相关项）**

| 项 | 说明 | 关联编号 |
|----|------|----------|
| **发布链路** | math 从「仓库手动 scp binary + `UENV_MATH_PLUGIN_BIN`」→ `uenv env build` 构建 OCI 镜像 → 推 registry → `uenv env publish` 在 Hub 登记 `math@x.y.z` | H-6、H-7 |
| **Worker H-6** | spawn/acquire 前按 Hub manifest **拉制品**（OCI 镜像或 process 用 binary/tarball），本地 cache；不再硬依赖节点预置 `UENV_MATH_PLUGIN_BIN` | H-6、§5.3 |
| **PodmanBackend** | 从占位实现到可跑通 math 容器：`image.digest` pull/run → 容器内 PluginService over UDS | W-10 |
| **math 插件业务（P-1～P-4）** | reset 读题、GSM8K 判分、与 Worker 判分源冻结——属 **环境镜像 / 制品内代码**，与 Hub 服务代码库无关 | P-1～P-4 |

**明确不需要迁移的**

- 把 `uenv-math-plugin.rs` 挪进 `uenv-hub-server` 代码库（Hub 只存注册信息与制品索引，不托管运行时源码）
- 让 Hub 在 Episode 里执行 reset/step（Hub **永远不进** step 热路径；Worker 经 UDS 调环境实例）

**目标热路径（spawn 阶段，Hub 参与制品解析，不参与 step）**

```text
Episode → Worker WarmupPool.acquire(math)
  → GET Hub .../math/versions/latest（或 pin 版本）
  → 按 manifest.backend 拉制品并 spawn 实例
       podman：pull image.url@digest → 跑容器（math 逻辑在镜像内）
       process：拉 binary/包到 cache → exec entry（如 ./run.sh）
  → PluginHost UDS：reset → step* → release
```

| # | 来源（§1.2） | 现状 | 待办 | 优先级 | 规模化关联 |
|---|--------------|------|------|--------|------------|
| H-2 | Hub 集成 | 仅启动时 HTTP 拉 manifest 元数据 | 见 H-6 扩展为热路径制品同步 | P1 | 脱离本地 `plugins/` 硬编码 |
| H-6 | Hub 集成 | **不下载**镜像/插件包 | spawn/acquire 前从 Hub 拉取插件包或 OCI 镜像（digest 校验）；缓存到 Worker 本地 | P1 | **Hub 进入 Episode 热路径** |
| H-7 | Hub 集成 | 版本仅在启动 merge | 每次 spawn 按 `env_type@version` 解析；Episode 可带版本 pin | P1 | 多环境类型/版本并存 |
| H-8 | Hub 集成 | Hub 不参与 dispatch | Server/Worker 调度链路透传 Hub manifest 摘要（version、backend、resources） | P2 | 多 Worker 统一制品源 |

#### 8.2.4 Server / 调度（`uenv-server` / adapter-core Scheduler）

| # | 来源（§1.2 影响面） | 现状 | 待办 | 优先级 | 规模化关联 |
|---|---------------------|------|------|--------|------------|
| S-1 | 心跳 `load`（Worker 侧 W-6） | 调度不感知 Worker 负载 | 消费 `WorkerHeartbeat.load`，负载均衡/背压 | P1 | **多 Worker** |
| S-2 | §1.3 单 Worker | 仅 1 条 `RegisterWorker` 验收 | 多 Worker 注册、按 `env_type`/资源筛选、`DispatchEpisode` 并行 | P1 | **多 Worker** |
| S-3 | 单 step stub | 单 Episode 串行思维 | 并发下发多 Episode（尊重 Worker `max_concurrent` + lease） | P1 | **多 Episode** |
| S-4 | `ResourceSpec`（W-7） | 注册资源为空 | 调度器匹配 `Episode.resource_spec` 与 Worker `resource` | P2 | 异构集群 |
| S-5 | Hub 热路径（H-6～H-8） | Server 不 orchestrate 制品 | 可选：Server 下发 `env_version`/镜像 digest，Worker spawn 前向 Hub 解析 | P2 | Hub + Server 协同 |

#### 8.2.5 VeRL / 7142 运行环境

| # | 来源 | 现状 | 待办 | 优先级 | 规模化关联 |
|---|------|------|------|--------|------------|
| V-1～V-6 | 见 §2.1 | 部署项 | GRPO/评测 vLLM、GSM8K 数据、远端 core | P0 | 训练必需 |
| V-7 | 规模化 | 1-step smoke 为主 | rollout batch 并发多 prompt → 多 Episode 并行经 Bridge 打分 | P1 | **多 Episode** 训练吞吐 |
| V-8 | 多 step | 单轮 completion | 若 env 支持 multi-step：rollout 与 env step 语义对齐（或保持 1-step env + 单 completion 判分，文档冻结） | P1 | 与 W-9 设计对齐 |

**其他模块层验收勾选（规模化）**

- [ ] P-1～P-3：≥2 道不同 GSM8K 题，插件 reset 题目不同且判分正确
- [ ] B-1～B-6：Bridge payload 含 `question`/`response_text`/`rule_reward.target`
- [ ] H-6：新 Worker 节点无本地预置 `UENV_MATH_PLUGIN_BIN` 仍可通过 Hub 跑通 Episode
- [ ] S-2：≥2 Worker 同时注册，Server 可将 Episode 分发到不同 Worker
- [ ] S-3：同一 Worker `max_concurrent>1` 时并行 Episode 均完成且 lease 不冲突
- [ ] V-7：VeRL batch size > 1 时 acc/reward 与逐条核对一致

---

### 8.3 推荐实施顺序（在 §3 Phase A～C 之后）

```text
Phase D（真实 GSM8K + 多 step）
  B-1～B-6, W-1, W-3, W-4, W-9, P-1～P-4
  → 单 Worker 上多 step GSM8K Episode，trajectory.steps > 1

Phase E（多 Episode / 多 Worker 并发）
  W-6, W-7, S-1, S-2, S-3, B-7, V-7
  → ≥2 Worker、Worker 内并行 ≥2 Episode，负载与 lease 正常

Phase F（Hub 热路径 + 多实例制品）
  H-6, H-7, H-8, W-10, S-5（可选）
  → spawn 时 Hub 拉制品；多 Worker 共享 Hub 版本源；Podman 路径可选验收
```

---

*维护：全栈 GSM8K 代码落地后，请同步更新 [260530-full-stack-integration-gaps.md](./260530-full-stack-integration-gaps.md) 与 [更新日志.md](./更新日志.md)。*
