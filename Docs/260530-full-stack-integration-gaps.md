# 真实全栈集成缺口清单（Bridge merge 后复核）

> **版本**：2026-05-30  
> **背景**：`feature/verl-bridge-adapter` 已 merge 进当前分支（commit `eb7211f`）；`uenv-server` L1 proto 已统一（commit `f35735c`）。本文对照 [先前全栈缺口讨论](../discussions/a100-server-worker-e2e/README.md) 与 [PROTOCOL.md](../PROTOCOL.md)，逐项复核 **仍缺失、仍阻塞「真实全栈」** 的部分。  
> **目标链路**：`VeRL → uenv-bridge → uenv-server → uenv-worker → plugins/gsm8k`（Hub 暂 Mock）

---

## 1. 结论摘要

| 层级 | merge 后新增能力 | 真实全栈是否打通 |
|------|------------------|------------------|
| **Bridge** | VeRL Adapter、Rust adapter core、Layer 1–3 基线（fake/math_proxy） | ❌ 未接 Server |
| **Server** | 统一 proto、`ControlPlaneService`、`SubmitEpisode`→Worker 派发 | ⚠️ 代码就绪，**无跨机验收** |
| **Worker** | gsm8k 插件、预热池、WAL、Mock/Remote 控制面 | ⚠️ Mock 路径通，**真实 Server 未验收** |
| **Hub** | HTTP REST 四 crate 可独立运行 | ❌ 未参与执行热路径 |

**一句话**：Bridge 已 merge，但 **Bridge ↔ Server ↔ Worker 三段尚未串联**；先前讨论的 P0 阻塞项 **大部分仍存在**，仅 **Server–Worker proto 分裂** 一项已解决。

---

## 2. 先前缺口逐项复核

### 2.1 已解决 ✅

| # | 先前缺口 | 当前状态 | 证据 |
|---|----------|----------|------|
| G1 | **uenv-server ↔ Worker proto 分裂** | ✅ 已统一 | `proto/uenv/v1/` 为权威；Server 实现 `ControlPlaneService` + `WorkerGrpcService` 客户端；旧 `uenv-server/proto/server.proto` 已删除 |
| G2 | **Server 无法按 Worker 契约派发** | ✅ 代码已补 | `SubmitEpisode` 填充 `dispatch_lease_id` → `DispatchEpisode` → 等待 `ReportResult` |

### 2.2 仍存在 — 阻塞真实全栈 🔴

| # | 缺口 | 说明 | 影响 |
|---|------|------|------|
| **B1** | **Bridge → Server 未对接** | Rust adapter core 仅有 `FakeEpisodeService` / `MathProxyEpisodeService`；`UENV_ADAPTER_CORE_REWARD_MODE=serve` **未实现**（`core/src/main.rs` 仅支持 `fixed` / `math_proxy`） | VeRL Layer 4「真实 Serve 联动」无法启动 |
| **B2** | **`EpisodeService` 真实实现缺失** | `core/src/server_api.rs` 中 `UEnvServeEpisodeService` 仍为文档示例（`todo!()`）；无 gRPC/进程内调用 `uenv-server` 的实现 | Bridge 无法在 Rust 侧触达 Server 调度链 |
| **B3** | **`GrpcEpisodeClient` 未实现** | `clients.py` 中 `_to_proto_request` / `_from_proto_result` 为 `NotImplementedError`；注释仍写「Serve proto 尚未可用」 | Python 直连接 Server 的路径不可用 |
| **B4** | **Bridge ↔ Server 字段映射未落地** | Bridge 使用 `request_id`（`core/src/protocol.rs`）；统一 L1 proto 使用 `episode_id` + `attempt_id`。Bridge README §映射仍引用已废弃的 server 字段（`protocol_version`、`env_config` 等） | 即使接 gRPC，也需重写映射层 |
| **B5** | **`env_type` 语义不一致** | Bridge/VeRL 将 GSM8K 映射为 `env_type=math`（`verl.py` `task_to_env_type: gsm8k→math`）；Worker Phase 0 仅注册/执行 `gsm8k` | Server 调度 `math` 请求 **找不到 Worker** |
| **B6** | **M7 真实 Server–Worker 跨机验收未完成** | MVP 清单 M7 联调项 `[ ]` 未勾选；仅 mock-scheduler 本机回归 | Server+Worker 组合 **无实机证据** |
| **B7** | **Bridge payload 与 Worker gsm8k 插件格式未对齐** | Worker 期望 `payload` JSON 含 `question` + `reward_config` 含 `rule_reward.target`（见 `fixtures/gsm8k/`）；Bridge payload 为 VeRL 结构（`env_config.response_text`、`rubric_config.ground_truth`） | 即便 `env_type` 对齐，插件/RuleReward 可能判分失败 |
| **B8** | **Unix 环境 gsm8k 集成测试未在 CI/实机留痕** | Windows 上 `m4`/`m5`/`m6` 为 `0 tests`（`cfg(unix)`） | 插件 UDS 链路缺 A100 回归记录 |

### 2.3 仍存在 — 不阻塞首轮 Server–Worker 联调，但阻塞生产全栈 🟡

| # | 缺口 | 说明 |
|---|------|------|
| **Y1** | **Hub 未接入 Worker** | Worker 无 `uenv-hub-client` 依赖；启动不 pull manifest |
| **Y2** | **Hub seed 无 `gsm8k`** | 种子为 `math`/`code`/`agent`；与 Worker `env_type=gsm8k` 不一致 |
| **Y3** | **Worker 心跳 `load` 恒 0** | `control_plane/client.rs` 写死 `load: 0`；Server 调度依赖本地 increment/decrement，与心跳不一致 |
| **Y4** | **`ResourceSpec` 未注册** | `RegisterWorkerRequest.resource` Worker 未填；Server 无资源过滤 |
| **Y5** | **`DrainCommand` / epoch 联动未验收** | Worker 未处理 Drain；epoch 变更后再注册未组合测试 |
| **Y6** | **Server 高级 RPC 未实现** | `SubmitEpisodeStream`、`SubmitBatch`、异步模式均为 `unimplemented` |
| **Y7** | **`StreamReport.report_type` 未填充** | proto 已扩展；Worker 仍主要写 `phase` 字符串 |
| **Y8** | **Bridge Layer 4 文档与实现漂移** | README 描述 `serve` mode，代码未提供；映射表与 [PROTOCOL.md](../PROTOCOL.md) 不一致 |
| **Y9** | **GEMAdapter / 非 VeRL 框架** | 仅 VeRL 路径有实现；ROLL GEM 等未接 |
| **Y10** | **跨 crate 集成测试缺失** | 无 `bridge→server→worker` 自动化测试；无 e2e fixture 脚本（讨论文档 P0-1） |

---

## 3. 分层对接状态图

```text
┌─────────────┐     adapter_core.proto      ┌──────────────────┐
│ VeRL        │ ──────────────────────────► │ Rust adapter core│
│ (Python)    │      (本地 gRPC, ✅)         │ fake/math_proxy  │
└─────────────┘                             └────────┬─────────┘
                                                     │
                              EpisodeService trait   │  ❌ serve 未实现
                                                     ▼
                                            ┌──────────────────┐
                                            │ uenv-server      │
                                            │ UEnvService        │ ◄── ❌ Bridge 未接
                                            │ ControlPlane       │ ◄── ✅ Worker 可接
                                            └────────┬─────────┘
                                                     │ WorkerGrpcService ✅
                                                     ▼
                                            ┌──────────────────┐
                                            │ uenv-worker      │
                                            │ gsm8k plugin     │ ◄── ⚠️ env_type/payload 待对齐
                                            └──────────────────┘

┌─────────────┐
│ uenv-hub    │  HTTP REST ✅ 独立可运行
└─────────────┘  ❌ 不在 Episode 热路径
```

---

## 4. 需要对接 / 补全的工作清单

### 4.1 P0 — 打通「Bridge → Server → Worker → gsm8k」最小闭环

| 序号 | 工作项 | 建议做法 | 依赖 |
|------|--------|----------|------|
| P0-1 | **A100 Server–Worker 实机验收** | 按 [a100-server-worker-e2e](../discussions/a100-server-worker-e2e/README.md) §5；grpcurl `SubmitEpisode`（`env_type=gsm8k`） | 网络/端口 |
| P0-2 | **实现 `UEnvServeEpisodeService`** | 在 `uenv-bridge/core` 新增：gRPC 客户端调 `UEnvService.SubmitEpisode`，或链接 `uenv-server` crate（进程内） | P0-1 验证 Server API |
| P0-3 | **Bridge ↔ L1 proto 字段映射** | `request_id` → `episode_id`；`attempt_id=1`；`payload`/`reward_config` 转为 Worker 可消费的 JSON | [PROTOCOL.md](../PROTOCOL.md) §4.1 |
| P0-4 | **统一 GSM8K `env_type`** | 方案 A：Bridge 对 gsm8k 数据集发 `env_type=gsm8k`；方案 B：Worker 同时注册 `math` 并路由到 gsm8k 插件（需 Worker 改造） | 团队对齐 |
| P0-5 | **`ADAPTER_CORE_REWARD_MODE=serve`** | `core/src/main.rs` 增加分支，挂载 `UEnvServeEpisodeService` | P0-2 |
| P0-6 | **payload 转换层** | VeRL `env_config` + `ground_truth` → Worker `question` + `rule_reward.target` | P0-3、P0-4 |
| P0-7 | **Layer 4 smoke 脚本更新** | `run_verl_grpo_1step_with_bridge_reward.sh` 支持 `serve` + 指向真实 Server 地址 | P0-5 |

### 4.2 P1 — Bridge 完善与 Server 语义补全

| 序号 | 工作项 | 说明 |
|------|--------|------|
| P1-1 | 实现 `GrpcEpisodeClient` proto 转换 | 生成 Python stub（`make proto-bridge`），完成 `_to_proto_request` / `_from_proto_result` |
| P1-2 | 更新 Bridge README 映射表 | 与 [PROTOCOL.md](../PROTOCOL.md) 及统一 `episode.proto` 对齐 |
| P1-3 | Worker 心跳真实 `load` | 上报 `active_episode_count` / `max_concurrent` |
| P1-4 | Worker 注册 `ResourceSpec` | A100 机器 GPU 信息填入 `RegisterWorkerRequest.resource` |
| P1-5 | M7 清单勾选与日志交叉验证 | 补 Server/Worker 联调记录至 `Docs/discussions/a100-server-worker-e2e/records/` |
| P1-6 | 跨 crate 集成测试 | `tools/submit-fixture` 或 `uenv-bridge/examples/` 读 fixture → Server → 断言 reward |

### 4.3 P2 — Hub 与生产化（全栈扩展）

| 序号 | 工作项 | 说明 |
|------|--------|------|
| P2-1 | Hub 发布 `gsm8k` manifest | 与 `plugins/gsm8k/manifest.yaml` 对齐 |
| P2-2 | Worker Hub pull + 本地降级 | 见 [260528-1722-worker-next-phase-plan.md](./260528-1722-worker-next-phase-plan.md) §3.8 |
| P2-3 | Server `SubmitBatch` / 流式 RPC | VeRL batch 训练规模需要 |
| P2-4 | 多步 Episode + `StreamReport.report_type` | PRD F-14 |
| P2-5 | GEMAdapter / ROLL 路径 | PRD F-05 |

---

## 5. Bridge merge 后已具备、但尚未用于全栈的能力

以下能力 **已实现**，全栈对接时应直接复用，避免重复建设：

| 能力 | 位置 | 验证状态 |
|------|------|----------|
| VeRL `DataProto` → `EpisodeRequest` | `uenv-bridge/src/uenv/bridge/verl.py` | Python 单测 13 passed |
| Python → Rust core 本地 gRPC | `adapter_core.proto` + `RustCoreEpisodeClient` | Layer 3 脚本通过 |
| Rust batch 语义与 `request_id` 对齐校验 | `uenv-bridge/core/src/core.rs` | Rust 单测 5 passed |
| VeRL 1-step/2-step GRPO（bridge-only） | `run_verl_grpo_1step_with_bridge_reward.sh` | fixed/math_proxy 已验证 |
| `EpisodeService` trait 边界 | `core/src/server_api.rs` | 接口稳定，待 Serve 实现 |
| Server `SubmitEpisode` 完整链路 | `uenv-server/src/service.rs` | 编译通过，待实机 |
| Worker Mock/Remote 控制面 | `uenv-worker` | mock 混沌 8 passed |

---

## 6. 推荐验收顺序

```text
Step 1  Server + Worker（无 Bridge）
        grpcurl SubmitEpisode(env_type=gsm8k) → reward=1.0
        └─  unblock: B6, B8

Step 2  Bridge core serve mode（无 VeRL）
        adapter_core → UEnvServeEpisodeService → Server → Worker
        └─  unblock: B1, B2, B5, B7（映射层）

Step 3  VeRL Layer 4
        main_ppo + UEnvBridgeRewardManager + serve mode
        └─  真实全栈首通

Step 4  Hub / 生产语义
        Y1–Y10, P2.*
```

---

## 7. 与相关文档的关系

| 文档 | 关系 |
|------|------|
| [PROTOCOL.md](../PROTOCOL.md) | L1 协议权威；Bridge 映射 **须以此为准** 修订 |
| [discussions/a100-server-worker-e2e/README.md](../discussions/a100-server-worker-e2e/README.md) | Server–Worker 实机步骤；本文 P0-1 执行手册 |
| [worker-pool-mvp-checklist.md](./worker-pool-mvp-checklist.md) | M7 `[ ]` 项对应本文 B6 |
| [uenv-bridge/README.md](../uenv-bridge/README.md) | Bridge 四层验证；Layer 4 阻塞原因见本文 B1–B2 |
| [260528-1722-worker-next-phase-plan.md](./260528-1722-worker-next-phase-plan.md) | Hub、心跳、多步等 P1/P2 规划 |

---

## 8. 变更记录

| 日期 | 变更 |
|------|------|
| 2026-05-30 | 初版：Bridge merge + Server proto 统一后复核；标记 G1/G2 已解决；新增 B1–B8、Y1–Y10 与 P0–P2 对接清单 |
