# uenv-worker — UEnv Worker Pool 执行层

Worker 是 UEnv **Layer 2 Worker Pool** 的执行节点：gRPC **Server** 接收 Scheduler 主动下发的 `DispatchEpisode`，并通过 ControlPlane **Client** 上报 `RegisterWorker` / `Heartbeat` / `ReportResult`。

权威设计文档：[Docs/worker-pool-layer-design.md](../Docs/worker-pool-layer-design.md)

## 职责

- **Episode 执行**：`EpisodeExecutor` 管理 reset → N×step → close（M2+）
- **模型回调**：`ModelClient` 直连推理服务（HTTP/gRPC）
- **预热池**：`WarmupPool` 本地持有 Warm 实例；缺实例时自行 `spawn`；Episode 结束归还 Warm 复用（M6+）
- **Hub 元数据**：`EnvResolver` 在 spawn 前拉取/合并 Hub manifest（M-5+）；制品仍用本地 `plugins/`
- **插件子进程**：`ProcessBackend` + `plugins/math/`（M4+）；**非**内嵌 Python 主路径
- **Worker WAL**：断连重放（schema M1 冻结，持久化 M8）

## 模块结构（design §13）

```
src/
├── cli/                 # serve / version / health
├── config/              # YAML/JSON（ADR-002）
├── runtime.rs
├── control_plane/       # RegisterWorker / Heartbeat / ReportResult
├── grpc_server/         # DispatchEpisode / HealthCheck
├── episode/             # executor, model_client
├── pool/                # warmup_pool
├── hub/                 # env_resolver, hub pull
├── plugin/              # host, instance, arpc (L2)
├── backend/             # process, podman
├── wal/
└── logging/
```

## CLI

```bash
# 启动 Worker（M2 实现完整运行时）
uenv-worker serve --config config/uenv-worker.yaml

uenv-worker version
uenv-worker health
```

## 配置

主示例：`config/uenv-worker.yaml`（或 `config/uenv-worker.json`）。

## 环境插件与按需拉起

Phase 0 环境：`plugins/math/`（`env_type=math`, `ipc=proto-uds`）；GSM8K 为 `payload.dataset=gsm8k`。

默认 **`prewarm_on_startup: false`**：Worker 启动不预创建实例；首条 `DispatchEpisode(env_type=math)` 时从池 acquire（池空则 spawn）。可选 Hub：

```bash
UENV_HUB_ENDPOINT=http://127.0.0.1:8080
UENV_ENV_TYPES=math
UENV_PREWARM_ON_STARTUP=false   # 或 true 恢复启动即 prewarm
uenv-worker serve --config config/uenv-worker.yaml
```

`uenv-worker/python/` 为历史内嵌环境路径，**非 MVP 主路径**（Phase 1+ 或 legacy）。

## Mock 联调

MVP 阶段使用独立 crate `uenv-mock-scheduler` 作为 ControlPlane，无需完整 `uenv-server`：

```bash
uenv-mock-scheduler serve --fixture-dir ./fixtures/math
uenv-worker serve --config config/uenv-worker.yaml
```

## M7 联调前配置切换（Worker 侧）

```bash
# 真实 Server 控制面地址
UENV_SERVER_ENDPOINT=<uenv-server-host:50051>

# Worker gRPC 对外可达地址（供 Scheduler 直连 DispatchEpisode）
UENV_WORKER_LISTEN=0.0.0.0:50052

# 可观测端口（Prometheus + 健康检查）
UENV_METRICS_LISTEN=0.0.0.0:19090
UENV_HEALTH_LISTEN=0.0.0.0:19090
```

## 本机预联调（当前决议）

在真实 `uenv-server` 未就绪前，先使用**本机 IP + 端口**模拟 remote 形态进行预联调：

```bash
UENV_SCHEDULER_MODE=remote
UENV_SERVER_ENDPOINT=<LOCAL_IP>:50051
UENV_WORKER_LISTEN=0.0.0.0:50052
```

- 目的：验证 Register / Heartbeat / Dispatch / Report 链路与 endpoint 回连可达性。
- 边界：该方案仅是预联调，**不等同于** M7「真实 Server 联调」验收完成；后续仍需与真实 `uenv-server` 补做一次日志交叉验证。

- `GET /metrics`：Prometheus 文本指标（含 `uenv_warmup_pool_hit_total`、`uenv_warmup_pool_miss_total`、`uenv_instance_pool_size{status}`）
- `GET /health`：返回 `ok`

## 术语对照

| 设计文档 | 本仓库 |
|----------|--------|
| `uenv-adapter` | `uenv-bridge` |
| `WarmupPool` | `src/pool/warmup_pool.rs` |
| `ModelClient` | `src/episode/model_client.rs` |
