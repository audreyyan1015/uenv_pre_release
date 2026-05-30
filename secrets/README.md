# A100 测试机联调指南

本目录存放两台 A100 测试机的 SSH 私钥。**请勿将私钥提交到公开仓库或分享给无关人员。**

---

## 1. 机器与密钥对照

| 角色建议 | 主机 | SSH 端口 | 用户 | 私钥文件 |
|----------|------|----------|------|----------|
| 机器 A（控制面 / Hub） | `219.147.100.43` | **7143** | `root` | `9aa460dab6678381f86a1022b8a54c9f_32e42d1c7902ce68ba6719d551645e02_8.143` |
| 机器 B（Worker 执行层） | `219.147.100.43` | **7142** | `root` | `2a9f778a35e7d08c738c79493ba643ef_65c3b455afbe3c81a8a757c01b0faae8_8.142` |

两台机器共用同一公网 IP，通过 **不同 SSH 端口** 区分实例。

---

## 2. SSH 登录

### Linux / macOS / WSL / Git Bash

```bash
# 进入仓库根目录
cd /path/to/UEnv

# 设置私钥权限（仅首次）
chmod 600 secrets/9aa460dab6678381f86a1022b8a54c9f_32e42d1c7902ce68ba6719d551645e02_8.143
chmod 600 secrets/2a9f778a35e7d08c738c79493ba643ef_65c3b455afbe3c81a8a757c01b0faae8_8.142

# 登录机器 A（7143）
ssh -i secrets/9aa460dab6678381f86a1022b8a54c9f_32e42d1c7902ce68ba6719d551645e02_8.143 \
    -p 7143 root@219.147.100.43

# 登录机器 B（7142）
ssh -i secrets/2a9f778a35e7d08c738c79493ba643ef_65c3b455afbe3c81a8a757c01b0faae8_8.142 \
    -p 7142 root@219.147.100.43
```

### Windows PowerShell

```powershell
ssh -i secrets\9aa460dab6678381f86a1022b8a54c9f_32e42d1c7902ce68ba6719d551645e02_8.143 `
    -p 7143 root@219.147.100.43

ssh -i secrets\2a9f778a35e7d08c738c79493ba643ef_65c3b455afbe3c81a8a757c01b0faae8_8.142 `
    -p 7142 root@219.147.100.43
```

### 可选：写入 `~/.ssh/config` 简化登录

```
Host uenv-a100-7143
    HostName 219.147.100.43
    Port 7143
    User root
    IdentityFile /path/to/UEnv/secrets/9aa460dab6678381f86a1022b8a54c9f_32e42d1c7902ce68ba6719d551645e02_8.143

Host uenv-a100-7142
    HostName 219.147.100.43
    Port 7142
    User root
    IdentityFile /path/to/UEnv/secrets/2a9f778a35e7d08c738c79493ba643ef_65c3b455afbe3c81a8a757c01b0faae8_8.142
```

之后可直接 `ssh uenv-a100-7143` / `ssh uenv-a100-7142`。

---

## 3. 推荐联调拓扑（GSM8K / MathEnv）

GSM8K 插件依赖 **Linux + Unix Domain Socket**（`proto-uds`），必须在 Linux 环境（本 A100 机或 WSL2）上跑 Worker。

```
┌─────────────────────────────────────┐     gRPC ControlPlane      ┌─────────────────────────────────────┐
│  机器 A (7143)                       │ ◄────────────────────────── │  机器 B (7142)                       │
│  uenv-mock-scheduler  :50051         │   Register / Heartbeat /    │  uenv-worker          :50052         │
│  （可选）uenv-hub       :8080         │   ReportResult              │  plugins/gsm8k 子进程                  │
└─────────────────────────────────────┘                             └─────────────────────────────────────┘
         │ 主动 DispatchEpisode ──────────────────────────────────────────────► Worker gRPC :50052
```

| 组件 | 建议部署位置 | 默认端口 | 说明 |
|------|-------------|----------|------|
| `uenv-mock-scheduler` | 机器 A | `50051` | **当前可跑通 GSM8K 全链路的控制面** |
| `uenv-worker` | 机器 B | `50052`（gRPC）、`19090`（metrics/health） | 执行 gsm8k 插件 |
| `uenv-hub`（可选） | 机器 A | `8080` | 环境元数据注册，**当前 Worker 尚未接入** |
| `uenv-server`（暂不建议） | — | `50051` | 与 Worker **proto 尚未对齐**，见 §6 |

> **单机快速验证**：若两台机器网络互通不便，也可在同一台 Linux 机器上同时启动 mock-scheduler 与 worker，仅将 `server.endpoint` 改为 `127.0.0.1:50051`。

---

## 4. 环境准备（两台机器均需）

```bash
# 依赖（Ubuntu/Debian 示例）
apt-get update && apt-get install -y build-essential pkg-config libssl-dev protobuf-compiler git curl

# Rust（若未安装）
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"

# 同步代码（任选其一）
git clone <repo-url> UEnv && cd UEnv
# 或 scp/rsync 从本机推送

# 生成 proto 并编译
make proto
cargo build -p uenv-mock-scheduler -p uenv-worker --release
```

创建日志目录：

```bash
sudo mkdir -p /var/log/uenv /tmp/uenv/wal
sudo chown -R "$USER" /var/log/uenv /tmp/uenv
```

---

## 5. 分步启动与验证

### 5.1 机器 A：启动 Mock Scheduler

```bash
cd UEnv

# 编辑 config/uenv-mock-scheduler.yaml，确认 fixture_dir 指向 ./fixtures/gsm8k
UENV_MOCK_LISTEN=0.0.0.0:50051 \
UENV_LOG_FILE=/var/log/uenv/mock-scheduler.log \
  ./target/release/uenv-mock-scheduler serve --config config/uenv-mock-scheduler.yaml
```

另开终端确认监听：

```bash
ss -tlnp | grep 50051
tail -f /var/log/uenv/mock-scheduler.log
```

### 5.2 机器 B：启动 Worker

将 `config/uenv-worker.yaml` 中 `server.endpoint` 改为 **机器 A 的内网/可达 IP:50051**（不要用 `127.0.0.1`，除非同机部署）。

```bash
cd UEnv

export UENV_SCHEDULER_MODE=remote
export UENV_SERVER_ENDPOINT=<机器A_IP>:50051
export UENV_WORKER_LISTEN=0.0.0.0:50052
export UENV_ENV_TYPES=gsm8k
export UENV_PLUGIN_DIR=./plugins
export UENV_WARMUP_POOL_SIZE=2
export UENV_MAX_CONCURRENT=4
export UENV_METRICS_LISTEN=0.0.0.0:19090
export UENV_LOG_FILE=/var/log/uenv/worker.log

./target/release/uenv-worker serve --config config/uenv-worker.yaml
```

### 5.3 连通性检查

**在机器 B 上**（Worker 侧）：

```bash
# Worker 健康与指标
curl -s http://127.0.0.1:19090/health
curl -s http://127.0.0.1:19090/metrics | grep uenv_

# 确认能连到机器 A 的控制面（需 grpcurl）
grpcurl -plaintext <机器A_IP>:50051 list
```

**在机器 A 上**（Scheduler 侧）：

```bash
# 查看已注册 Worker
grpcurl -plaintext -d '{"env_types":["gsm8k"]}' \
  127.0.0.1:50051 uenv.scheduler.v1.ControlPlaneService/ListWorkers
```

### 5.4 期望的 GSM8K 全链路日志

Mock Scheduler 启动后会从 `fixtures/gsm8k/episode_001.pb` 自动派发任务。成功时两侧日志应出现：

| 阶段 | 机器 A（mock-scheduler） | 机器 B（worker） |
|------|--------------------------|------------------|
| 注册 | `RegisterWorker accepted` | `control_plane_mode_remote` / register ok |
| 心跳 | heartbeat ack | heartbeat loop |
| 派发 | dispatch to `<worker_endpoint>` | `phase=dispatch_received` |
| 执行 | — | `acquire` → `reset` → `model_callback` → `step` → `release` |
| 回报 | `ReportResult ack` | `phase=dispatch_completed`, `warmup_hit=true/false` |

Metrics 验收：

```bash
curl -s http://<机器B_IP>:19090/metrics | grep -E 'uenv_episode_total|uenv_warmup_pool'
```

### 5.5 自动化回归（推荐在机器 B 上执行）

```bash
cd UEnv
cargo test -p uenv-mock-scheduler --test m1_contract_chaos_tests -- --nocapture
cargo test -p uenv-worker --test m5_episode_executor -- --nocapture
cargo test -p uenv-worker --test m6_warmup_pool -- --nocapture
```

---

## 6. 当前接口对接状态（2026-05-30）

merge 后代码已合入同一仓库，但 **跨组件生产级对接尚未全部完成**。联调前请了解以下边界：

### ✅ 已对齐、可联调

| 链路 | Proto / 接口 | 状态 |
|------|-------------|------|
| Mock Scheduler ↔ Worker 控制面 | `proto/uenv/v1/scheduler.proto` → `ControlPlaneService` | ✅ 已实现 |
| Mock Scheduler → Worker 派发 | `uenv-worker/proto/worker_service.proto` → `WorkerGrpcService.DispatchEpisode` | ✅ 已实现 |
| Worker ↔ gsm8k 插件 | `plugin_proto/` UDS | ✅ 已实现（**仅 Linux**） |
| Episode 消息 | `proto/uenv/v1/episode.proto`（含 `dispatch_lease_id`） | ✅ Mock 路径已用 |
| Hub HTTP API | `uenv-hub` 四 crate + REST `/api/v1/*` | ✅ Hub 自身可独立启动 |

### ❌ 尚未对接 / 阻塞真实全栈

| 缺口 | 说明 | 联调影响 |
|------|------|----------|
| **uenv-server ↔ Worker proto 分裂** | Server 仍用 `uenv-server/proto/server.proto`（`WorkerRegistration` / `WorkerExecution`），Worker 用 `scheduler.proto` + `worker_service.proto`；`EpisodeRequest`/`EpisodeResult` 字段也不一致 | **不能**直接用 `uenv-server` 替代 mock-scheduler |
| **M7 真实 Server 验收未完成** | 清单 M7 联调项 `[ ]` 未勾选；仅本机 mock 回归通过 | 跨机真实 Server 链路缺证据 |
| **Worker 未接入 Hub** | `uenv-worker/Cargo.toml` 无 `uenv-hub-client` 依赖；启动不 pull Hub manifest | Hub 可单独跑，但不参与 Episode 执行 |
| **Hub seed 无 gsm8k** | Hub 种子数据为 `math`/`code`/`agent`，Worker Phase 0 用 `env_type=gsm8k` | 需在 Hub 手动 publish gsm8k，或等 Worker 接入后再对齐 |
| **uenv-bridge 未接 GSM8K** | 训练框架 → Server 的 Python 适配层尚无 gsm8k 样例 | 完整「训练侧 → Server → Worker」链路未通 |
| **心跳语义简化** | Worker `load` 恒 0、`DrainCommand` 未处理、`ResourceSpec` 未填 | 不影响 GSM8K 单轮执行，影响调度感知 |

### 结论：现在能跑通什么？

- **可以跑通**：`uenv-mock-scheduler` + `uenv-worker` + `plugins/gsm8k` 的 **Register → Heartbeat → Dispatch → Execute → Report** 全链路（Linux）。
- **暂不能跑通**：`uenv-server` + `uenv-worker` 真实组合；`uenv-hub` 驱动的 Worker 环境发现；`uenv-bridge` 训练框架端到端。

---

## 7. （可选）启动 Hub 做独立验证

Hub 与 Worker 执行链路 **当前无硬依赖**，可先在机器 A 验证 Hub 自身：

```bash
cd UEnv/uenv-hub
UENV_HUB_AUTH__REQUIRE_TOKEN=false cargo run -p uenv-hub-server

# 另开终端
curl -s http://127.0.0.1:8080/healthz
curl -s http://127.0.0.1:8080/api/v1/envs
```

CLI（需先 `cargo build -p uenv-hub-client`）：

```bash
export UENV_HUB_ENDPOINT=http://127.0.0.1:8080
./target/debug/uenv hub status
./target/debug/uenv env list
```

---

## 8. 防火墙与端口放行

跨机联调时，需确保以下端口在 **机器 A ↔ 机器 B** 之间可达：

| 方向 | 端口 | 用途 |
|------|------|------|
| B → A | `50051` | Worker 注册/心跳/上报 → ControlPlane |
| A → B | `50052` | Scheduler 主动 DispatchEpisode |
| 运维 | `19090` | Worker metrics / health（可选外网暴露） |
| 可选 | `8080` | Hub HTTP |

```bash
# 示例：ufw 放行（按实际安全策略调整）
ufw allow from <对端IP> to any port 50051
ufw allow from <对端IP> to any port 50052
```

---

## 9. 常见问题

| 现象 | 排查 |
|------|------|
| Worker 注册失败 `UNAVAILABLE` | 检查 `UENV_SERVER_ENDPOINT` 是否可达；机器 A mock-scheduler 是否监听 `0.0.0.0:50051` |
| Dispatch 超时 | 检查 A→B 的 `50052` 是否放行；`UENV_WORKER_LISTEN` 是否为 Worker 对外 IP |
| 插件启动失败 | 确认在 Linux 运行；`UENV_PLUGIN_DIR` 指向含 `plugins/gsm8k/run.sh` 的目录；`chmod +x plugins/gsm8k/run.sh` |
| `warmup_hit=false` 持续 | 首次 Episode 冷创建属正常；第二次应为 `true` |
| 使用 uenv-server 报错 | **预期行为**——需先统一 proto，暂用 mock-scheduler |

---

## 10. 联调记录模板

每次跨机验收建议留存：

```
日期：
分支/提交：
机器 A IP:端口：
机器 B IP:端口：
控制面：mock-scheduler / uenv-server（注明）
Episode ID：
Worker endpoint：
结果：success / fail
warmup_hit：
reward / status：
异常与处置：
```

---

## 参考文档

- [worker-pool-mvp-checklist.md](../Docs/worker-pool-mvp-checklist.md) — M7 联调退出标准
- [260528-1722-worker-next-phase-plan.md](../Docs/260528-1722-worker-next-phase-plan.md) — proto 对齐与 Hub 接入规划
- [uenv-worker/README.md](../uenv-worker/README.md) — Worker 配置说明
- [uenv-mock-scheduler/README.md](../uenv-mock-scheduler/README.md) — Mock 控制面说明
- [uenv-hub/README.md](../uenv-hub/README.md) — Hub 独立部署
