# Worker 并发僵死修复：问题与代码变更记录

> **版本**：2026-06-16  
> **范围**：`uenv-worker` 因 OpenRouter HTTP 无超时、信号量永久阻塞导致的 **假活（僵死）** 问题；本文仅记录 Worker 层问题与已实施修复。  
> **关联**：[全链路事件与 Server 建议](./260616-worker-deadlock-incident-and-server-recommendations.md)、[`secrets/README.md`](../secrets/README.md)

---

## 1. 背景

2026-06-16 在 A100 **7143**（`219.147.100.43`）发现 Worker 进程存活、`/health` 与 heartbeat 正常，但约 **8 小时**无新 dispatch 完成，Server 侧 batch 长期 pending。

| 项 | 值 |
|----|-----|
| 僵死 Worker ID | `cd8c4b91-1ad4-48e9-809c-33a6430b17cd`（PID 3079082，运行约 4 天） |
| 僵死窗口 | UTC 05:55 ~ 14:13 |
| 修复后 Worker ID | `a0f02c52-0b5c-4fa4-9932-3688019ffece`（UTC 14:32 部署） |
| 日志归档 | `tmp/worker-incident-20260616/` |

**直接表现：** `uenv_active_episode_count` 观测为 **10**（配置 `max_concurrent=4`）；最后一条有效 dispatch 日志后仅剩 heartbeat。

---

## 2. 根因机制

```
Server DispatchEpisode → Worker dispatch_received（日志）
                              ↓
                    semaphore.acquire()（4 槽满则永久等待）
                              ↓
                    execute_episode → ModelClient → OpenRouter HTTP
                              ↓
                    无 HTTP 超时 → 槽位永久占用
                              ↓
                    新请求全部排队；/health + heartbeat 仍正常 → 假活
```

**主因归属 Worker：** 同步阻塞执行路径 + 无 LLM HTTP 超时 + 信号量无 acquire 上限 + 失败路径 metrics/lease 泄漏。Server 仅用 heartbeat 判活属于次因（放大故障），见全链路文档。

---

## 3. 修复前代码问题

| # | 文件 | 问题 | 后果 |
|---|------|------|------|
| 1 | `episode/model_client.rs` | `reqwest::Client::new()` 无 connect/read 超时 | OpenRouter hang 时 LLM 调用永不返回 |
| 2 | `episode/model_client.rs` | `max_retries` 实机约 **30**，且无单次请求上限 | hang 被重试放大，占用并发槽更久 |
| 3 | `grpc_server/worker_service.rs` | `max_concurrent` 信号量耗尽后 `acquire` **无限阻塞** | 新 Dispatch 永久排队，无背压反馈 |
| 4 | `grpc_server/worker_service.rs` | 无 Episode 级总超时 | 单次 execute 可无限运行 |
| 5 | `grpc_server/worker_service.rs` | `execute_episode` 失败时 **未 `dec_active()`** | `uenv_active_episode_count` 泄漏（10 > 4） |
| 6 | `grpc_server/worker_service.rs` | acquire / execute 失败未清理 `active_leases` | lease 残留，影响重复派发语义 |
| 7 | 可观测性 | `dispatch_received` 在 acquire **之前** 打印 | 易误判为「已开始执行」 |

**说明：** `dispatch_episode` **同步 await** 整个 `execute_episode`（占满 gRPC handler）在本次 **未改架构**，靠超时 fail-fast 缓解；后台 spawn 列为后续可选改进。

---

## 4. 已实施修复（2026-06-16）

### 4.1 变更总览

| 改动 | 文件 | 说明 |
|------|------|------|
| LLM HTTP 超时 | `uenv-worker/src/llm.rs`、`episode/model_client.rs` | 默认 **120s**；带 `connect_timeout` + 整请求 `timeout` 的 `reqwest::Client` |
| 重试上限 | 同上 | 默认 **3** 次（原实机约 30） |
| 信号量 acquire 超时 | `grpc_server/worker_service.rs` | 默认 **30s** → `RESOURCE_EXHAUSTED` |
| Episode 总超时 | 同上 | 默认 **300s** → `DEADLINE_EXCEEDED` |
| active 计数 RAII | 同上 | `ActiveEpisodeGuard`：Drop 时必 `dec_active` |
| lease 清理 | 同上 | acquire / execute / episode 超时失败时 `clear_active_lease` |
| 日志 | 同上 | 新增 `dispatch_acquired`；acquire / episode 超时专用 `phase` |
| 配置模板 | `config/uenv-worker-llm.env.example` | 补充 LLM 超时与重试 env |

### 4.2 问题 → 修复对照

| 修复前问题 | 修复手段 |
|------------|----------|
| LLM HTTP 无超时 | `build_http_client()` + `UENV_LLM_HTTP_TIMEOUT_SECS`（默认 120s） |
| 重试 30 次放大 hang | `UENV_LLM_MAX_RETRIES` 默认 3 |
| 信号量 acquire 永久阻塞 | `tokio::time::timeout` 包裹 `acquire_owned`，30s 失败返回 |
| Episode 无总时限 | `tokio::time::timeout` 包裹 `execute_episode`，300s 失败返回 |
| `dec_active` 泄漏 | `ActiveEpisodeGuard` RAII |
| lease 残留 | 各失败分支调用 `clear_active_lease` |
| 难以区分「收到」与「开跑」 | acquire 成功后打 `phase=dispatch_acquired` |

### 4.3 关键代码位置

**LLM 配置默认值**（`llm.rs`）：

```rust
pub const DEFAULT_LLM_HTTP_TIMEOUT_SECS: u64 = 120;
pub const DEFAULT_LLM_MAX_RETRIES: usize = 3;
```

**HTTP Client**（`model_client.rs`）：

```rust
Client::builder()
    .connect_timeout(connect_timeout)
    .timeout(request_timeout)
    .build()
```

**Dispatch 超时与 Guard**（`worker_service.rs`）：

- `DEFAULT_DISPATCH_ACQUIRE_TIMEOUT_SECS = 30`
- `DEFAULT_EPISODE_TIMEOUT_SECS = 300`
- `ActiveEpisodeGuard` 在 `inc_active()` 后持有，函数返回或错误时自动 `dec_active`

---

## 5. 环境变量

| 变量 | 默认值 | 作用 |
|------|--------|------|
| `UENV_LLM_HTTP_TIMEOUT_SECS` | 120 | 单次 LLM HTTP 请求上限（connect + 整请求 deadline） |
| `UENV_LLM_MAX_RETRIES` | 3 | LLM 失败重试次数（间隔 2s） |
| `UENV_WORKER_DISPATCH_ACQUIRE_TIMEOUT_SECS` | 30 | 等并发槽最长时间；超时返回拥塞错误 |
| `UENV_WORKER_EPISODE_TIMEOUT_SECS` | 300 | 单次 `execute_episode` 总 wall time 上限 |

模板见 [`config/uenv-worker-llm.env.example`](../config/uenv-worker-llm.env.example)。后两个变量在 Worker 进程 env 中配置（如 7143 的 `/root/.uenv-worker.env`）。

**注意：** Worker 侧 Episode 截止 **尚未**读取 `EpisodeRequest.timeout_seconds`，仍用全局 `UENV_WORKER_EPISODE_TIMEOUT_SECS`。多 env 并存时需取各 workload 上限，或待 per-request 改造（见 §8）。

---

## 6. 对外错误与日志

| 场景 | gRPC Status | message | Worker 日志 `phase` |
|------|-------------|---------|---------------------|
| 等并发槽超时 | `RESOURCE_EXHAUSTED` | `max_concurrency_acquire_timeout` | `dispatch_acquire_timeout` |
| 信号量已耗尽（非超时） | `RESOURCE_EXHAUSTED` | `max_concurrency_reached` | — |
| Episode 总超时 | `DEADLINE_EXCEEDED` | `episode_timeout` | `episode_timeout` |
| execute 失败 | `INTERNAL` | `execute_episode_failed: ...` | `dispatch_failed` |
| 真正开始执行 | — | — | `dispatch_acquired` |

Server 应将这些 gRPC 错误转为 batch 失败路径，而非无限等待 Dispatch stream（Server 侧待跟进，见全链路文档 §4）。

---

## 7. 部署与验证

**7143 部署步骤：**

```bash
cd /root/UEnv
cargo build -p uenv-worker --release
# 停止旧进程后
source /root/.uenv-worker.env
nohup ./target/release/uenv-worker --config config/uenv-worker.deploy-7143.yaml serve \
  >> /var/log/uenv/worker-stdout.log 2>&1 &
```

**验证：**

```bash
curl -s http://127.0.0.1:28777/health                    # ok
curl -s http://127.0.0.1:28777/metrics | grep active_episode   # 空闲时应为 0
grep 'phase="dispatch_acquired"' /var/log/uenv/worker.log | tail -3
```

**调参：** 默认 300s / 120s 为平台级安全网，适用于所有 `env_type`。慢 workload 或误判风险见全链路文档 [§3.1 超时误判与调参](./260616-worker-deadlock-incident-and-server-recommendations.md#31-超时误判与调参)。

---

## 8. 本次未做的 Worker 改动

| 项 | 说明 |
|----|------|
| `dispatch_episode` 后台 spawn | handler 仍同步 await 至 Episode 结束；靠超时释放槽位 |
| honor `EpisodeRequest.timeout_seconds` | Episode 截止仍仅读 Worker 全局 env |
| `/metrics` 扩展 | 如 `semaphore_available`、最老 in-flight episode 年龄 |
| env plugin 子进程树清理 | Worker 退出时清理 plugin 子进程 |
| `dispatch_received` 日志时机 | 仍在 acquire 前打印；靠 `dispatch_acquired` 辅助判断 |

---

## 9. 参考

| 资源 | 路径 |
|------|------|
| 全链路事件 + Server 建议 | [`260616-worker-deadlock-incident-and-server-recommendations.md`](./260616-worker-deadlock-incident-and-server-recommendations.md) |
| 7143 Worker 配置 | [`config/uenv-worker.deploy-7143.yaml`](../config/uenv-worker.deploy-7143.yaml) |
| LLM env 模板 | [`config/uenv-worker-llm.env.example`](../config/uenv-worker-llm.env.example) |
| 事件日志包 | `tmp/worker-incident-20260616/` |

---

## 10. 变更记录

| 日期 | 说明 |
|------|------|
| 2026-06-16 | 初版：Worker 僵死问题、修复对照、env、错误码、部署验证 |
