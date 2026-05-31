# 真实全栈集成缺口清单（Bridge merge 后复核）

> **版本**：2026-05-31（MathEnv 对齐修订）  
> **背景**：`feature/verl-bridge-adapter` 已 merge；L1 proto 已统一。本文对照 [a100-server-worker-e2e](../discussions/a100-server-worker-e2e/README.md)、[PROTOCOL.md](../PROTOCOL.md) 与 **[UEnv 设计 PRD v7.2 §3.2.5](./uenv-design-prd-v7.2.md)**，复核全栈缺口。  
> **目标链路**：`VeRL → uenv-bridge → uenv-server → uenv-worker → MathEnv（math）→ gsm8k benchmark`（Hub seed 已含 `math`）

---

## 0. 推进进度

| 阶段 | 状态 | 说明 |
|------|------|------|
| Step 1 Server + Worker（无 Bridge） | ✅ 完成 | M7 实机 2026-05-30（**历史**：`env_type=gsm8k`） |
| **Step 1b MathEnv 跨层对齐** | ✅ **代码完成** | M-1～M-6 已落地；待 A100 复验 `env_type=math` |
| Step 2 Bridge core serve mode | ⚠️ 待三联调 | serve + math 映射就绪 |
| Step 3 VeRL Layer 4 真实全栈 | ❌ 未验收 | 依赖 Step 2 |
| Step 4 Hub / 生产语义 | ❌ 未开始 | Y1、Y3–Y10、P2.* |

### 0.1 P0 工作清单

| 序号 | 工作项 | 状态 | 备注 |
|------|--------|------|------|
| P0-1 | A100 Server–Worker 实机验收 | ✅ | 2026-05-30；当时 `env_type=gsm8k` |
| P0-2 | `UEnvServeEpisodeService` | ✅ | `serve_client.rs` |
| P0-3 | Bridge ↔ L1 字段映射 | ✅ | `l1_mapping.rs` |
| P0-4 | 统一计算类 `env_type=math` | ✅ | `verl.py`、`plugins/math`、fixture/e2e |
| P0-5 | `ADAPTER_CORE_REWARD_MODE=serve` | ✅ | `main.rs` |
| P0-6 | payload 转换层 | ⚠️ | 已实现；需补 `payload.dataset`（M-4） |
| P0-7 | Layer 4 smoke 脚本 `serve` | ⚠️ | 待 math 语义实机 |
| P0-8 | Bridge serve 三联调 smoke | ❌ | 以 `env_type=math` 为验收基准 |

### 0.2 MathEnv 跨层对齐清单（PRD v7.2 → 实施）

> 详细 Worker 侧见 [worker-pool-layer-design.md §9.4](./worker-pool-layer-design.md)。

| # | 工作项 | 层级 | 状态 |
|---|--------|------|------|
| **M-1** | Worker 默认/注册 `env_type=math` | Worker | ✅ |
| **M-2** | `plugins/math/` + gsm8k backend | Worker/插件 | ✅ |
| **M-3** | fixture / e2e / grpcurl 改为 `math` + `dataset=gsm8k` | 测试 | ✅ |
| **M-4** | Bridge：`gsm8k→math`；mapping 写 `payload.dataset` | Bridge | ✅ |
| **M-5** | Worker Hub pull `math` manifest | Worker+Hub | ✅ |
| **M-6** | 移除 `env_type=gsm8k` alias | 全栈 | ✅ |

**跨层语义（冻结方向）**：

| 层 | `env_type` | GSM8K 表达 |
|----|------------|------------|
| Hub | `math` | manifest tags / examples |
| Bridge/VeRL | `math` | `data_source=gsm8k` → 不提升为 env_type |
| Server 调度 | `math` | — |
| Worker 注册/预热池 | `math` | 单 MathEnv 实例池 |
| payload | — | `"dataset": "gsm8k"` |

---

## 1. 结论摘要

| 层级 | 能力 | 真实全栈 |
|------|------|----------|
| **Bridge** | serve mode + L1 映射 | ⚠️ 待 math 对齐后三联调 |
| **Server** | SubmitEpisode → Worker | ✅ M7 已验收 |
| **Worker** | 插件/预热池/Remote 控制面 | ✅ M7 已验收；**env_type 待收敛 math** |
| **Hub** | seed 含 `math`/`code`/`agent` | ⚠️ 未参与 Worker 热路径 |

**一句话**：M7 证明 Server–Worker 可达；**下一优先级**是 PRD 对齐的 **`env_type=math` 全栈收敛**（M-1–M-4），再跑 Bridge serve 与 VeRL Layer 4。

---

## 2. 缺口逐项复核

### 2.1 已解决 ✅

| # | 缺口 | 证据 |
|---|------|------|
| G1 | Server ↔ Worker proto 统一 | `proto/uenv/v1/` |
| G2 | Server 按 Worker 契约派发 | `SubmitEpisode` 全链路 |
| G3 | M7 跨机验收 | 2026-05-30 e2e 记录 |
| G4 | Bridge serve mode 代码 | `serve_client.rs` + `main.rs` |
| G5 | `UEnvServeEpisodeService` | gRPC `SubmitEpisode` |
| G6 | Bridge ↔ L1 字段映射 | `l1_mapping.rs` |
| G8 | payload 判分格式 | `question` + `rule_reward.target` |

### 2.2 已解决但需修订 ⚠️

| # | 原结论 | 修订（PRD v7.2） |
|---|--------|------------------|
| **G7** | `gsm8k→gsm8k` 已对齐 | **撤回**：Hub/PRD/Bridge 权威为 **`math`**；GSM8K 为 dataset；见 M-4 |

### 2.3 仍存在 — 阻塞真实全栈 🔴

| # | 缺口 | 说明 |
|---|------|------|
| **B0** | ~~MathEnv 跨层未收敛~~ | ✅ M-1～M-6 已落地；待 A100 math 复验 |
| **B1** | Bridge 三联调未验收 | serve 路径无实机记录 | P0-8 |
| **B2** | `GrpcEpisodeClient` 未实现 | P1；主路径 Rust core | |
| **B3** | Unix math 插件集成测试缺 CI 留痕 | `cfg(unix)` | |

### 2.4 仍存在 — 不阻塞首轮联调 🟡

| # | 缺口 | 说明 |
|---|------|------|
| **Y1** | Hub 未接入 Worker 热路径 | 无 manifest pull |
| ~~Y2~~ | ~~Hub seed 无 gsm8k~~ | **关闭**：Hub 已有 `math`；应对齐 Worker 拉 `math`（M-5） |
| **Y3** | 心跳 `load` 恒 0 | |
| **Y4** | `ResourceSpec` 未注册 | |
| **Y5** | Drain / epoch 未验收 | |
| **Y6** | Server 高级 RPC 未实现 | |
| **Y7** | `StreamReport.report_type` 未填充 | |
| **Y8** | Bridge README 映射漂移 | |
| **Y9** | 非 VeRL 框架 | |
| **Y10** | 跨 crate 集成测试 | |

---

## 3. 分层对接状态图

```text
VeRL ──► Bridge ──► adapter_core(serve) ──► uenv-server ──► uenv-worker
              │              │                    │              │
              │              │                    │              └── MathEnv (env_type=math)
              │              │                    │                    └── dataset=gsm8k (payload)
              └── env_type=math (目标)             └── 调度 math Worker
Hub seed: math ✅ ──────────────────────────────► Worker pull (M-5 未实现)
```

---

## 4. 工作清单

### 4.1 P0 — Bridge → Server → Worker → MathEnv

| 序号 | 工作项 | 状态 |
|------|--------|------|
| P0-1 | Server–Worker 实机（历史 gsm8k） | ✅ |
| P0-2 ~ P0-3, P0-5 | serve + 映射 | ✅ |
| **P0-4** | **`env_type=math` 全栈对齐** | 📋 M-1–M-4 |
| P0-6 | payload + `dataset` | ⚠️ |
| P0-7 ~ P0-8 | smoke / 三联调 | ❌（math 基准） |

**Step 2 复现（math 基准，待 M-3 更新脚本）**：

```bash
# grpcurl 示例（目标形态）
# env_type=math, payload={"question":"...","dataset":"gsm8k"}, reward_config=rule_reward
bash Docs/discussions/a100-server-worker-e2e/scripts/submit-episode-grpcurl.sh 127.0.0.1:50051

UENV_ADAPTER_CORE_REWARD_MODE=serve UENV_SERVER_ENDPOINT=127.0.0.1:50051 \
  ./target/release/uenv-adapter-core
```

### 4.2 P1 / P2

| 序号 | 工作项 | 状态 |
|------|--------|------|
| P1-1 ~ P1-4, P1-6 | Bridge/Worker 完善 | ❌ |
| P1-5 | M7 日志交叉验证 | ✅ |
| P2-1 | Hub **`math`** manifest 与 Worker 插件对齐 | ⚠️ Hub seed 已有；Worker manifest 待 M-2 |
| P2-2 | Worker Hub pull | ❌ |
| P2-3 ~ P2-5 | 批量 RPC / 多步 / ROLL | ❌ |

---

## 5. 推荐验收顺序

```text
Step 1   Server + Worker（gsm8k env_type）           ✅ 2026-05-30 历史

Step 1b  MathEnv 跨层对齐（math + dataset=gsm8k）   📋 M-1–M-4
         └─ unblock: B0, G7 修订, P0-4

Step 2   Bridge serve 三联调（math）                 ❌
         └─ unblock: B1, P0-8

Step 3   VeRL Layer 4                                ❌

Step 4   Hub pull + 生产语义                          ❌
```

---

## 6. 相关文档

| 文档 | 关系 |
|------|------|
| [uenv-design-prd-v7.2.md](./uenv-design-prd-v7.2.md) | **MathEnv / env_type 权威** |
| [worker-pool-layer-design.md §9](./worker-pool-layer-design.md) | Worker MathEnv 收敛与 M-1–M-6 |
| [PROTOCOL.md](../PROTOCOL.md) | L1 proto；`env_type` 示例待更新为 math |
| [260528-1722-worker-next-phase-plan.md §3.3](./260528-1722-worker-next-phase-plan.md) | MathEnv 能力边界（与本文 M 清单互补） |

---

## 7. 变更记录

| 日期 | 变更 |
|------|------|
| 2026-05-30 | 初版 |
| 2026-05-31 | serve mode 实现；P0-1–P0-6 |
| 2026-05-31 | **MathEnv 代码落地（M-1～M-6）**；Step 1b ✅；B0 关闭 |
