# Worker 僵死事件总结与 Server 层改进建议

> **版本**：2026-06-16  
> **范围**：A100 7143 `uenv-worker` 假活导致全链路 batch 无返回；Worker 侧已修复并部署，Server 侧待跟进。  
> **关联**：[`secrets/README.md`](../secrets/README.md) 四端拓扑、[`260609-worker-full-chain-integration-summary.md`](./260609-worker-full-chain-integration-summary.md)

---

## 1. 事件摘要

| 项 | 值 |
|----|-----|
| **现象** | 7142 / VeRL 侧大量 batch 提交至 Server，长时间无 `execute_batch_done`；Server 称已转发 Worker，Worker 无 `report_result` |
| **典型 batch** | `verl-agent-loop-49dcc5ff`（Server `execute_batch_received` @ UTC 13:55:10，**Worker 无对应日志**） |
| **典型 request_id** | `cc61631c-d9a6-4107-b177-6af637ac391f`（Server / Worker 日志均未出现，可能停留在 Adapter 层命名） |
| **机器** | A100 **7143**（`219.147.100.43:28888`） |
| **僵死 Worker** | PID 3079082，ID `cd8c4b91-1ad4-48e9-809c-33a6430b17cd`，自 2026-06-12 运行约 4 天 |
| **僵死窗口** | 约 UTC **05:55 ~ 14:13**（约 8h：Worker 无新 dispatch 完成，仅 heartbeat） |
| **日志归档** | 本地 `tmp/worker-incident-20260616/`（含完整 `worker.log`、Server `uenv-adapter-core.log` 摘录） |

**恢复时间线：**

1. **UTC 14:13** — 手动 kill 僵死 Worker，重启（ID `3eb23c0a-...`），链路短暂恢复。  
2. **UTC 14:32** — 合并 Worker 代码修复后重新编译部署（ID `a0f02c52-0b5c-4fa4-9932-3688019ffece`），`/health` ok，`active_episode_count=0`。

---

## 2. 根因结论

### 2.1 责任划分

| 层级 | 结论 |
|------|------|
| **Worker** | **主因** — 同步阻塞 + 无 HTTP 超时 + 信号量排队 hang，导致假活 |
| **Server** | **次因** — 仅用 heartbeat 判断 Worker 健康，缺少吞吐/完成率感知与背压，故障被放大 |

不是「Server 调度选错 Worker」（四端拓扑仅一个 Worker），而是 **Worker 执行模型缺陷** + **Server 无法识别业务僵死**。

### 2.2 机制链（Worker）

```
VeRL batch → Server execute_batch → DispatchEpisode → Worker
                                                      ↓
                              dispatch_received（日志）
                                                      ↓
                              semaphore.acquire()（满则永久等待）
                                                      ↓
                              execute_episode → infer_action → OpenRouter HTTP
                                                      ↓
                              无 timeout → 4 槽 hang → 新请求全部排队
                                                      ↓
                              Server 等 StreamReport / report_result（永不返回）
                              /health + heartbeat 仍正常 → 假活
```

### 2.3 代码层具体问题（修复前）

| # | 位置 | 问题 |
|---|------|------|
| 1 | `model_client.rs` | `reqwest::Client::new()` **无 connect/read 超时**；`max_retries=30` 且无单次上限 → LLM 调用可无限 hang |
| 2 | `worker_service.rs` | `dispatch_episode` **同步 await** 整个 `execute_episode`，占满 gRPC handler |
| 3 | `worker_service.rs` | `max_concurrent=4` 信号量耗尽后，新请求 **无限阻塞** 在 `acquire`，非快速失败 |
| 4 | `worker_service.rs` | `execute_episode` 失败时 **未 `dec_active()`**，`uenv_active_episode_count` 泄漏（观测值 10 > 上限 4） |
| 5 | 可观测性 | `dispatch_received` 在 acquire **之前** 打印，易误判为「已受理」 |

### 2.4 日志与 metrics 证据

**Worker 最后有效 dispatch（UTC 05:55:05）：**

```
phase="dispatch_received" episode_id=95d6b954-db42-46b9-842f-06dfee41af0a
```

之后至重启前 **仅有 heartbeat**（约 8 小时）。

**僵死前 metrics：**

```
uenv_active_episode_count 10      # 配置 max_concurrent=4
uenv_instance_pool_size{active} 4
```

**Server（`/home/uenv-adapter-core.log`）：**

- 用户 batch `49dcc5ff`：`execute_batch_received` @ 13:55:10，**无** `execute_batch_done`
- 最后一次 `execute_batch_done` @ 09:44:19；之后约 4h+ 无 batch 完成
- 当日累计：`execute_batch_received` 577 vs `execute_batch_done` 445（约 132 pending）

**已排除：** Worker 进程崩溃、端口未监听、Hub Token 缺失、7143 与 Server 网络不通（heartbeat 持续正常）。

---

## 3. Worker 侧已实施修复（2026-06-16）

> **独立文档：** Worker 问题与代码变更详见 [`260616-worker-concurrency-timeout-fix.md`](./260616-worker-concurrency-timeout-fix.md)。

| 改动 | 文件 | 说明 |
|------|------|------|
| LLM HTTP 超时 | `uenv-worker/src/llm.rs`、`episode/model_client.rs` | 默认 **120s**（`UENV_LLM_HTTP_TIMEOUT_SECS`）；复用带 timeout 的 `reqwest::Client` |
| 重试上限 | 同上 | 默认 **3** 次（`UENV_LLM_MAX_RETRIES`，原 30 次） |
| 信号量 acquire 超时 | `grpc_server/worker_service.rs` | 默认 **30s**（`UENV_WORKER_DISPATCH_ACQUIRE_TIMEOUT_SECS`）→ `RESOURCE_EXHAUSTED` |
| Episode 总超时 | 同上 | 默认 **300s**（`UENV_WORKER_EPISODE_TIMEOUT_SECS`）→ `DEADLINE_EXCEEDED` |
| active 计数 RAII | 同上 | `ActiveEpisodeGuard` 保证成功/失败均 `dec_active` |
| lease 清理 | 同上 | acquire / execute 失败时移除 `active_leases` |
| 日志 | 同上 | 新增 `dispatch_acquired`；acquire 超时打 `dispatch_acquire_timeout` |
| 配置模板 | `config/uenv-worker-llm.env.example` | 补充超时/重试 env 说明 |

超时误判风险与各层对齐公式见 [§3.1](#31-超时误判与调参)。

**7143 部署：** `cargo build -p uenv-worker --release` 成功后重启；当前 Worker ID `a0f02c52-0b5c-4fa4-9932-3688019ffece`。

### 3.1 超时误判与调参

引入超时后，**可能**把「仍在正常进行、只是偏慢」的 Episode 判为失败；是否误判取决于 **哪一层超时** 以及 **各层是否对齐**。本节适用于 **所有 `env_type` / workload**（单步或多步、任意 plugin），与具体数据集无关；默认 300s / 120s 是 **平台级安全网**，不是为某一类 env 定制。

#### 3.1.1 各层超时与误判风险

| 层级 | 环境变量 / 机制 | 默认 | 误判「推理慢」？ | 说明 |
|------|-----------------|------|------------------|------|
| **LLM 单次 HTTP** | `UENV_LLM_HTTP_TIMEOUT_SECS` | 120s | **会** | 单次 OpenRouter completion 若合法地 >120s（大 `max_tokens`、模型排队、网络慢），会在出结果前被切断并进入重试 |
| **Episode 总耗时** | `UENV_WORKER_EPISODE_TIMEOUT_SECS` | 300s | **会** | 从 acquire → reset → LLM → plugin step 的 **总 wall time** 超 5min → `DEADLINE_EXCEEDED("episode_timeout")`，与「是否仍在正常推理」无关 |
| **并发槽排队** | `UENV_WORKER_DISPATCH_ACQUIRE_TIMEOUT_SECS` | 30s | **一般不算** | 量的是 **等信号量槽位** 的时间，不是单次推理时长；30s 仍拿不到槽 → `RESOURCE_EXHAUSTED("max_concurrency_acquire_timeout")`，属于背压 |
| **Server 等 ReportResult** | `EpisodeRequest.timeout_seconds`（0 则 300s） | 300s | **会** | `uenv-server` 在 `submit_episode` 内等 `report_result`；若阈值小于 Worker 实际完成时间，会出现 **Worker 仍在跑、Server 已报 `episode execution timeout`** |

**结论：**

- 容易因「推理偏慢但正常」误判的，主要是 **HTTP 单次超时** 与 **Episode / Server 总超时**。
- **acquire 超时** 判的是 Worker 拥塞，不是「这次推理太慢」；频繁触发应加 Worker、降 submit 速率或 Server 背压，而非单纯加大 Episode 超时。

#### 3.1.2 默认配置下的内在张力

Worker `ModelClient` 的 LLM 调用逻辑为：最多 **3 次重试**，每次 HTTP 最多 **120s**，重试间隔 **2s**（见 `model_client.rs`）。该逻辑对 **所有 env** 共用，与 `env_type`、`max_steps` 无关。

理论 LLM 段上限：

```
LLM_max ≈ max_retries × http_timeout + (max_retries − 1) × retry_sleep
        ≈ 3 × 120 + 2 × 2 = 364s
```

而 Episode 总超时仅 **300s**。因此可能出现：**前两次 OpenRouter 偏慢/超时重试，第三次本可成功，但 Episode 总超时先触发** — 对外仍是 `episode_timeout`，属于 **重试预算与总超时未对齐**，而非「推理必然需要 >5 分钟」。

另：Episode 总超时还包含 **池化 reset、plugin step** 等，LLM 只是其中一段；多步 env 或冷启动会进一步压缩 LLM 可用时间。

#### 3.1.3 正确失败 vs 误判

| 场景 | 判定 |
|------|------|
| OpenRouter TCP/HTTP **挂死**（本次僵死根因） | 超时后 fail-fast — **预期行为**，非误判 |
| 4 槽占满且均在 hang | acquire 30s 失败 — **背压**，避免 Server 无限堆 Dispatch |
| 单次 completion **合法地** >120s 但最终会成功 | HTTP 超时 — **可能误判**；需加大 `UENV_LLM_HTTP_TIMEOUT_SECS` 或减少重试 |
| OpenRouter **排队慢**但仍在响应 | 与 hang 难以区分；当前实现为 **整请求 deadline**，不区分 stall vs slow stream |
| 单步 Episode、小模型、较小 `max_tokens` | 默认 300s / 120s **往往够用** |
| 大模型、长输出、多步 Episode | 默认偏紧，需 **按 workload 显式调大**（见 §3.1.5、§3.1.7） |

#### 3.1.4 调参公式与推荐对齐

**Worker Episode 下限（避免重试被总超时截断）：**

```
UENV_WORKER_EPISODE_TIMEOUT_SECS
  ≥ max_retries × UENV_LLM_HTTP_TIMEOUT_SECS
    + (max_retries − 1) × 2
    + plugin_reset_buffer
```

其中 `plugin_reset_buffer` 建议 **30~60s**（池化 reset + env plugin step；多步 env 按 `max_steps` 线性放大）。

**Server 应 ≥ Worker + buffer：**

```
EpisodeRequest.timeout_seconds（或 Server 默认）
  ≥ UENV_WORKER_EPISODE_TIMEOUT_SECS + 30~60s
```

**Dispatch / batch 硬超时（§4.1.1）** 应 **≥ Server 的 `timeout_seconds`**，避免 Server 已 fail 而 Adapter 仍等 batch。

**acquire 超时** 与推理时长 **独立**；仅当「排队等槽」成为瓶颈时再单独调整 `UENV_WORKER_DISPATCH_ACQUIRE_TIMEOUT_SECS`（默认 30s 一般不必随 LLM 变慢而增大）。

#### 3.1.5 按 workload 的参考值

下表为 **估算起点**，新 `env_type` 上线时应按 §3.1.4 公式与实测 latency 复核，而非照搬某一 env 的固定值。

| workload 特征 | `HTTP` | `max_retries` | `Episode` | `Server timeout` | 备注 |
|---------------|--------|---------------|-----------|------------------|------|
| **轻量：单步、小 `max_tokens`、快 plugin** | 120 | 3 | 300 | 360 | 平台默认值；偶发 `episode_timeout` 时优先查重试截断（§3.1.2） |
| **中等：较大 `max_tokens` 或慢模型** | 180~240 | 2~3 | 600 | 660 | 先保证 `Episode ≥ LLM_max + buffer` |
| **重量：多步 env（`max_steps` > 1）** | 按单步估 | 2~3 | `max_steps` × 单步预算 + buffer | 同上 + buffer | Episode 预算随步数线性放大 |

调参后建议在 Worker 日志中核对：`phase="episode_timeout"`、`dispatch_acquire_timeout`、以及 `model client connection error (attempt N)` 的比例，区分「真 hang」与「慢但可完成」。

#### 3.1.6 错误码与日志（排障）

| 现象 | gRPC / 错误 | Worker 日志 `phase` |
|------|-------------|---------------------|
| Episode 总超时 | `DEADLINE_EXCEEDED` / `"episode_timeout"` | `episode_timeout` |
| 并发槽排队超时 | `RESOURCE_EXHAUSTED` / `"max_concurrency_acquire_timeout"` | `dispatch_acquire_timeout` |
| Server 侧等结果超时 | `"episode execution timeout"` | Worker 可能仍在执行或已本地超时 |

#### 3.1.7 多 env / 新 env 上线时的超时策略

超时配置分 **两类**，不必每增加一个 `env_type` 就改 Worker 二进制：

| 类别 | 配置入口 | 作用范围 | 何时调整 |
|------|----------|----------|----------|
| **平台保底** | Worker env：`UENV_LLM_HTTP_TIMEOUT_SECS`、`UENV_LLM_MAX_RETRIES`、`UENV_WORKER_DISPATCH_ACQUIRE_TIMEOUT_SECS` | 全 Worker 共用 | 防 hang / 僵死；**极少**因新 env 而改，除非单次 LLM 经常超过 HTTP 上限 |
| **Episode 预算** | `EpisodeRequest.timeout_seconds`（提交侧 / Adapter / VeRL） | **可按请求** | **每个新 env 上线时** 按 `max_steps`、模型、`generation_config` 估算并下发 |

**当前实现缺口（待跟进）：** `EpisodeRequest.timeout_seconds` 已在 proto 中定义，Server 会用于等 `report_result` 与 lease；**Worker 侧 Episode 截止仍只读** `UENV_WORKER_EPISODE_TIMEOUT_SECS` **全局 env**，尚未 honor 请求级 `timeout_seconds`。因此在新 env 接入前：

1. 提交侧为每个 batch / env 设置合理的 `timeout_seconds`（Server 层对齐）；
2. Worker env 中的 `UENV_WORKER_EPISODE_TIMEOUT_SECS` 取 **当前 Worker 上所有 env 所需上限**，或按 env 拆分 Worker 实例；
3. 长期应在 Worker 实现：`episode_timeout = max(请求 timeout_seconds, env 默认)`，与 Server lease 一致，避免快慢 env 共用单一全局上限。

新 env 预算估算：

```
timeout_seconds ≈ max_steps × (单步 LLM + plugin step) + reset_buffer
```

其中单步 LLM 可参考 §3.1.4 的 `LLM_max`；多 env 并存时取 **最大值** 作为 Worker 全局下限，或依赖上述 per-request 改造。

**后续 Worker 可选改进（未做）：**

- `dispatch_episode` 改为后台 spawn，handler 立即返回 stream（架构级）
- `/metrics` 暴露 `semaphore_available`、最老 in-flight episode 年龄
- Worker 退出时清理 env plugin 子进程树
- Worker `dispatch_episode` honor `EpisodeRequest.timeout_seconds`（请求级 Episode 超时）

---

## 4. Server 层改进建议

> 以下针对 **`uenv-adapter-core`（`8.130.86.71:8088`，Adapter + ControlPlane 合一）** 与调度路径。Worker 修复后若 Server 仍无限等待 Dispatch stream，batch 仍会在 Server 堆积。

### 4.1 P0 — 必须做

#### 4.1.1 Dispatch / batch 完成超时

- 对单次 `DispatchEpisode` gRPC 流设 **硬超时**：**≥ `EpisodeRequest.timeout_seconds`（默认 300s）**，并与 Worker `UENV_WORKER_EPISODE_TIMEOUT_SECS` **同源配置**（见 [§3.1.4](#314-调参公式与推荐对齐)）。**禁止**仅把 Server 调到 10min 而 Worker 仍为 300s，否则会出现 Worker 已 `episode_timeout`、Server 仍在等的 **状态不一致**。
- 对 `execute_batch` 整体设 **batch 级超时**（建议 ≥ 单 Episode 超时 × batch 内最大并行度预期，或按 batch 总 wall time 上限）；超时后：
  - 标记 batch **failed / partial**
  - 向 Adapter 返回可识别错误（含 `batch_id`）
  - **释放** Server 侧 in-flight 计数，避免永久占用调度槽

Worker 现已可能返回 `DEADLINE_EXCEEDED` / `RESOURCE_EXHAUSTED`；Server 须正确处理并 fail-fast，不能无限阻塞在 `execute_batch` 内。慢 workload 应 **同步调大** Worker / Server / batch 三层超时，而非只放宽 Server 一层（详见 [§3.1](#31-超时误判与调参)）。

#### 4.1.2 Worker 业务健康度（超越 heartbeat）

当前仅依赖 `RegisterWorker` + `WorkerHeartbeat` 判断 Worker **存活**，无法发现 **业务僵死**。

建议增加 **Worker 业务健康** 维度：

| 指标 | 建议阈值（示例） | 动作 |
|------|------------------|------|
| 距上次 `report_result` 时间 | > 5 min 且 in-flight > 0 | 标记 **degraded** |
| 距上次 `DispatchEpisode` 流结束 | > episode 超时 + buffer | 强制 cancel / 记失败 |
| `execute_batch_done` 速率 | N 分钟为 0 且 received 持续增长 | **暂停**向该 Worker 派发新 Episode |
| Worker 上报 load / active | 长期等于 max 且无完成 | 告警 + 限流 |

heartbeat 正常但 **零吞吐** 时，应视为 **不可用**，不再向其 Dispatch 新 work。

### 4.2 P1 — 强烈建议

#### 4.2.1 背压（Backpressure）

- Adapter → Server：`execute_batch_received` 过多 pending 时，**拒绝或延迟**新 batch（HTTP/gRPC `RESOURCE_EXHAUSTED` 或队列上限）。
- Server → Worker：在途 `DispatchEpisode` 数 **≤ Worker `max_concurrent` + 小 buffer**（如 +1~2），与 Worker 信号量语义对齐。
- 避免 VeRL 侧无限 submit 导致 Server 内存中堆积数百个「已 received 未完成」batch（本次观测约 132 pending/日）。

#### 4.2.2 在途 Dispatch 生命周期管理

- 维护 **in-flight 表**：`episode_id` / `batch_id` / `dispatch_started_at` / `worker_id`。
- 超时或 Worker 不可用时：**主动 cancel** gRPC 流、写 WAL/日志、触发 batch 失败路径。
- 与 Worker 侧 `dispatch_lease_id` / lease 过期语义一致，避免 Server 重复派发与 Worker lease 冲突。

#### 4.2.3 日志与 trace 关联

- Server 日志统一贯穿：`request_id` / `batch_id` / `episode_id` / `worker_id`。
- 本次 `cc61631c-...` 未出现在 Server adapter-core 日志，说明 **跨层 ID 对齐** 不足；建议在 Adapter → Server → Worker 全链路透传 `correlation_id`。

### 4.3 P2 — 运维与可观测性

| 项 | 建议 |
|----|------|
| **Metrics** | `uenv_server_pending_batches`、`uenv_server_oldest_pending_batch_age_sec`、`uenv_server_dispatch_in_flight`、`uenv_server_worker_report_result_rate` |
| **告警** | pending batch > N；oldest pending > T；Worker heartbeat ok 但 report_rate = 0 |
| **进程 hygiene** | `8.130.86.71` 上勿同时保留 debug `uenv-adapter-core :50051` 与 release `:8088`（易混淆排查） |
| **可视化** | 与 [UEnv 可视化规划](./UEnv可视化实现规划v1.0.md) 对齐：Worker 节点展示 active_episode、最近 report 时间、是否 degraded |

### 4.4 Server 侧「不需要改」的部分

- **多 Worker 负载均衡算法** — 四端联调仅 **一个** Worker，非选路错误。
- **Hub / Adapter 协议** — 本次 batch 已到达 Server，问题在 Server ↔ Worker 执行段。

---

## 5. 联调自检清单（复现后验证）

**Worker 正常：**

```bash
curl -s http://219.147.100.43:28777/health          # ok
curl -s http://219.147.100.43:28777/metrics | grep active_episode   # 空闲时应为 0
grep register /var/log/uenv/worker.log | tail -1
```

**Server 正常：**

```bash
grep execute_batch_done /home/uenv-adapter-core.log | tail -3
# 新 batch 应在合理时间内出现 execute_batch_done
```

**端到端：** 单条 VeRL smoke 后，Server 与 Worker 日志应同时出现同一 `batch_id` / `episode_id` 的 received → completed / report_result。

---

## 6. 参考

| 资源 | 路径 |
|------|------|
| 四端部署与端口 | [`secrets/README.md`](../secrets/README.md) |
| 本地事件日志包 | `tmp/worker-incident-20260616/` |
| Worker 部署脚本 | [`scripts/deploy_worker_7143_fix.py`](../scripts/deploy_worker_7143_fix.py) |
| Worker 配置 | [`config/uenv-worker.deploy-7143.yaml`](../config/uenv-worker.deploy-7143.yaml) |

---

## 7. 变更记录

| 日期 | 说明 |
|------|------|
| 2026-06-16 | 初版：事件总结、Worker 根因与修复、Server 层 P0–P2 建议 |
| 2026-06-16 | 增补 §3.1 超时误判与调参；§4.1.1 与 Worker/Server 超时对齐说明 |
| 2026-06-16 | §3.1 改为通用 workload / 多 env 表述；新增 §3.1.7 新 env 超时策略 |
| 2026-06-16 | 新增独立文档 [`260616-worker-concurrency-timeout-fix.md`](./260616-worker-concurrency-timeout-fix.md) |
