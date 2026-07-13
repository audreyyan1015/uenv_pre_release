# OpenHands + SWE 轨迹链路检查报告

> **日期**：2026-06-27  
> **检查范围**：merge 完成后的本地代码、`origin/feature/worker-pool-260622` 与规划文档（`secrets/README.md`、`Docs/`）  
> **结论摘要**：**OpenHands 执行链路已打通**（208.77 → 7143 Gateway）；**轨迹 Server 统一聚合在代码层已合入，但部署配置与 OpenHands 端到端验收尚未对齐，当前默认仍走 Worker 本地真值 + Gateway GET**。

---

## 1. Git 与代码同步状态

| 项 | 状态 |
|----|------|
| 当前分支 | `feature/worker-pool-260622` |
| HEAD | `95c8389` — Merge `origin/bridge-alignment` into feature/worker-pool-260622 |
| 工作区 | clean（无未提交改动） |
| 与缓存远端 `origin/feature/worker-pool-260622` | **一致**（`95c8389`） |
| `git fetch origin` | **失败**（HTTP 认证失败）；以下远端对比基于**本地已缓存**的 remote ref |

### 1.1 相对 `origin/bridge-alignment` 的增量

merge 之后，feature 分支在 `bridge-alignment`（`bbdd277`）之上额外包含：

| 提交 | 说明 |
|------|------|
| `a86dcb5` | OpenHands 迁移至 208.77，脚本与文档重组 |
| `8137f12` | docs 更新 |
| `95c8389` | merge 提交（含 v2.2 轨迹聚合、`uenv-common`、server trajectory HTTP 等） |

`bridge-alignment` 侧轨迹相关核心提交已合入：`12faf9f`（v2.2 主体）、`49b2e0d`、`e2697bb`（native 路径修复与去重）。

### 1.2 本地编译注意

Windows 开发机未执行 `make proto` 时，`uenv-worker` 编译报错：`EpisodeResult` 缺少 `trajectory_id` / `trajectory_storage_url` 字段（proto 已改、`**/src/gen/` 被 gitignore）。**Linux 部署机需先 `make proto` 再 `cargo build`**，不代表 merge 代码逻辑缺失。

---

## 2. 目标架构（规划对齐）

依据 `secrets/README.md` §1–§2、`Docs/260627-swe-openhands-integration-plan.md`、`Docs/260625-trajectory-server-migration-evaluation.md`（v2.2 冻结）：

```text
┌─ 8.130.208.77 OpenHands ─────────────────────────────────────────┐
│  openhands-runner :8777 / :8888                                   │
│  run_swebenchpro_official.py + uenv_runtime shim                  │
│  UENV_GATEWAY = http://127.0.0.1:28097（SSH 隧道 → 7143）          │
└────────────────────────────┬─────────────────────────────────────┘
                             │ HTTP Gateway API
┌────────────────────────────▼─────────────────────────────────────┐
│  7143 uenv-worker — Runtime Gateway :28097                        │
│  SweInstancePool → seal TrajectoryBundle → 本地 bodies/           │
│  [v2.2] TrajectoryUploader → POST Server :8077（需配置启用）       │
└────────────────────────────┬─────────────────────────────────────┘
                             │ trajectory upload（可选）
┌────────────────────────────▼─────────────────────────────────────┐
│  8.130.75.157 uenv-adapter-core                                   │
│  gRPC :8088（ControlPlane / DispatchEpisode）                     │
│  HTTP :8077（轨迹聚合：trajectory.db + bodies/）                   │
└──────────────────────────────────────────────────────────────────┘

Hub 8.130.95.176:8088 — 仅 Pro catalog 元数据，不参与轨迹存储。
```

---

## 3. OpenHands + SWE 执行链路 — **已打通**

### 3.1 调用关系确认

| 问题 | 结论 | 证据 |
|------|------|------|
| OpenHands 是否部署在 **8.130.208.77**？ | **是** | `secrets/README.md` §1.1/§2.6；`config/openhands-20877.env.example` |
| OpenHands 是否通过 Gateway 调用 **7143 Worker**？ | **是** | 208.77 上 `UENV_GATEWAY=http://127.0.0.1:28097`，经 `uenv-gateway-tunnel.service` SSH 隧道到 `10.10.20.143:28097` |
| 208.77 公网 runner 入口？ | **8777** health / **8888** API | `scripts/openhands/openhands_runner.py`；验收报告 §0 |
| Worker 侧 Gateway 配置？ | `0.0.0.0:28097`，`api_key: swe-pro-secret` | `config/uenv-worker.deploy-7143-swe-pro.yaml` |

### 3.2 代码路径

1. **208.77**：`scripts/run-openhands-pro-20877.sh` → `integrations/openhands/run_swebenchpro_official.py`
2. **Python 客户端**：`UEnvGatewayClient` → `POST /runtime/v1/sessions` 等（`integrations/openhands/uenv_runtime/client.py`）
3. **7143 Gateway**：`uenv-worker/src/runtime_gateway/mod.rs` 路由至 `SweInstancePool`
4. **评测与轨迹封存**：`instance_pool.submit()` → `session.seal_trajectory()` → 返回 `trajectory_ref`

### 3.3 已验收项（文档证据）

`Docs/260627-swe-openhands-acceptance-report.md` v2.0（2026-06-27）：

| 验收项 | 状态 |
|--------|------|
| Hub Pro catalog | 通过 |
| 7143 Gateway + Pro grader | 通过 |
| 轨迹捕获（TrajectoryRef + GET bundle） | **通过** |
| OpenHands 官方 SDK gold（208.77） | **reward=1.0，56/56** |
| Runner 公网 8777/8888 | 通过 |

**说明**：上述验收验证的是 **Worker 本地轨迹真值 + Gateway GET**，未包含向 Server `:8077` 上传后的聚合查询验收。

---

## 4. 轨迹 Server 统一聚合 — **代码已合入，部署未闭环**

### 4.1 代码实现状态（merge 后已存在）

| 组件 | 文件 | 能力 |
|------|------|------|
| Worker 上传 | `uenv-worker/src/swe/trajectory_upload.rs` | seal 后 `enqueue` → spool → gzip POST `/control/v1/trajectories` |
| Worker 触发点 | `uenv-worker/src/swe/instance_pool.rs` | `submit()` 中 seal 成功后入队；`storage_kind=server` |
| Server 存储 | `uenv-server/src/trajectory.rs` | SQLite `trajectory.db` + `bodies/{id}.json` |
| Server 启动 | `uenv-bridge/core/src/main.rs` | `UENV_TRAJECTORY_*` 启用时 spawn HTTP `:8077` |
| 控制面关联 | `uenv-server/src/control_plane.rs` | native 路径 `ReportResult` ack 写 `episode_results.trajectory_id` |
| 共享契约 | `uenv-common/src/trajectory.rs` | `TrajectoryRef`、`UploadStatus` |
| 协议 | `proto/uenv/v1/episode.proto` | `EpisodeResult.trajectory_id` / `trajectory_storage_url` |
| 测试 | `uenv-worker/tests/trajectory_upload_e2e.rs` | worker seal → upload → server GET 回读 |

**启用条件**：

- **Server**：`UENV_TRAJECTORY_ENABLED`（默认 true）+ `UENV_TRAJECTORY_HTTP_LISTEN`（默认 `0.0.0.0:8077`）
- **Worker**：`UENV_TRAJECTORY_ENDPOINT` 非空（或 yaml `trajectory_upload.endpoint`）即启用上传

### 4.2 与 OpenHands 路径的衔接

Gateway `submit` 与 native `run_episode` **共用** `SweInstancePool.submit()` → 同一套 seal + upload 逻辑。  
OpenHands 路径额外依赖：

- `UENV_SWE_ARTIFACT_DIR` 已设置（否则不 seal、不上传）
- `UENV_TRAJECTORY_ENDPOINT` 指向 Server `http://8.130.75.157:8077`（或内网地址）
- `run_id`：Server 入库**强制非空**；Gateway 从 `X-UEnv-Run-Id` 读取，**OpenHands Python 客户端当前未发送该头**，Worker 会 fallback 为 `run-gw-{episode_id}`（`session.rs`）

### 4.3 端到端验证记录

`Docs/trajectory_v2.2_changes_summary.md` 第二部分记载：

| 路径 | 环境 | 结果 |
|------|------|------|
| gateway | 真实 docker，worker 143 → server **86.71** | 轨迹 acked 入库 |
| native | VeRL gRPC，correlation_id 作 run_id | 单条轨迹、episode_results 关联 |

**缺口**：无 **208.77 OpenHands → 7143 → Server 75.157** 三跳聚合的 documented E2E；且验证 Server IP 仍为旧地址 `8.130.86.71`（`secrets/README.md` 已注明迁移至 `8.130.75.157`）。

### 4.4 部署配置差距（阻塞「全链路 Server 聚合」）

| 配置/文档 | 当前值 | 应对齐 |
|-----------|--------|--------|
| `config/uenv-worker.deploy-7143-swe-pro.yaml` → `server.endpoint` | `8.130.86.71:8088` | **`8.130.75.157:8088`** |
| 同上 → `trajectory_upload` | **缺失** | 增加 `endpoint: "http://8.130.75.157:8077"` + token（env） |
| `config/uenv-worker.deploy-7143.yaml` | 旧 Server IP | 同上 |
| `config/uenv-server.deploy.yaml` | 注释/公网仍写 **86.71** | 更新为 **75.157** |
| `secrets/README.md` §8.1 Server 启动 | 仅 `UENV_ADDR=8088` | **补充** `UENV_TRAJECTORY_*`、数据目录、token |
| `secrets/README.md` §8.2 Worker 启动 | 无轨迹上传 env | **补充** `UENV_TRAJECTORY_ENDPOINT`、`UENV_SWE_ARTIFACT_DIR` |
| `config/uenv-worker.deploy-7143.env.example` | 无轨迹变量 | 补充示例 |
| `scripts/deploy-pro-python-openhands-7143.sh` | 未 export 轨迹上传 | 可选补充 |
| `Docs/260627-swe-openhands-integration-plan.md` §4 | 写「轨迹不经 Server/Hub」 | **与 v2.2 冲突**，需修订为「Worker seal + 可选上传 Server」 |

**当前默认行为**：未配置 `UENV_TRAJECTORY_ENDPOINT` 时，`TrajectoryUploader::from_env()` 返回 `None`，轨迹**仅落 Worker 本地**，通过 Gateway `GET /runtime/v1/trajectories/{id}` 读取——与 v2.2 方案中的 rollout 过渡态（`enabled=false` / 无 endpoint）一致。

---

## 5. 文档一致性矩阵

| 文档 | OpenHands 208.77 | Worker Gateway | Server 轨迹聚合 | 一致性 |
|------|------------------|----------------|-----------------|--------|
| `secrets/README.md` | ✅ | ✅ | ⚠️ 未写轨迹 HTTP/上传 | 缺轨迹部署章节 |
| `260627-swe-openhands-integration-plan.md` | ✅ | ✅ | ❌ §4 仍写本地真值 only | **需更新** |
| `260627-swe-openhands-acceptance-report.md` | ✅ | ✅ Gateway GET | ❌ 未验 Server | 验收范围不含 Server |
| `260625-trajectory-server-migration-evaluation.md` | 提及 run_id | ✅ Gateway 路径 | ✅ 冻结方案 | 与代码一致 |
| `trajectory_v2.2_changes_summary.md` | 未覆盖 208.77 | ✅ gateway E2E | ✅ 86.71 验证 | Server IP 已迁移 |

---

## 6. 总体结论

### 6.1 已确认打通

1. **OpenHands 在 208.77 上运行**，runner 公网 **8777/8888**。
2. **208.77 通过 SSH 隧道调用 7143 Worker Runtime Gateway（:28097）**，不在 208.77 本地 pull 沙箱容器。
3. **SWE-bench Pro 评测闭环**（gold → grader → reward）已在 208.77 + 7143 验收通过。
4. **轨迹在 Worker 侧 capture + seal** 已工作；Gateway 可 GET 完整 bundle。

### 6.2 尚未闭环（代码有、配置/验收无）

1. **Worker → Server `:8077` 轨迹上传**在仓库 yaml/脚本/secrets 指南中**未配置**，OpenHands 路径**无 documented 聚合验收**。
2. **多处 config 仍指向旧 Server `8.130.86.71`**，与 `secrets/README.md`（`8.130.75.157`）不一致。
3. **OpenHands Python 客户端未传 `X-UEnv-Run-Id`**，依赖 Worker fallback；按作业聚合需 driver/runner 补传。
4. **`260627-swe-openhands-integration-plan.md` §4** 与 merge 后的 v2.2 轨迹方案**文字冲突**。

### 6.3 判定（2026-06-27 部署后更新）

| 链路 | 判定 |
|------|------|
| OpenHands + SWE 执行 | **✅ 已打通** |
| Worker 本地轨迹存储 + Gateway GET | **✅ 已打通**（upload ack 后本地删除，改从 Server 读） |
| Worker 上传 → Server 统一聚合（OpenHands 路径） | **✅ 已打通并验收** |

### 6.4 实机验收记录（2026-06-27）

| 项 | 结果 |
|----|------|
| Server `8.130.75.157:8077` 轨迹 HTTP | **运行中**（`trajectory_http_listening`） |
| 7143 Worker `trajectory_uploader_started` | **endpoint=http://8.130.75.157:8077** |
| 208.77 OpenHands gold | **reward=1.0，56/56** |
| Server 轨迹 GET/LIST | **server_verified=true** |
| 验收 run_id | `run-oh-20260627-181531-pro-gold` |
| 验收 trajectory_id | `trj-worker-7143-pro-1782555340535-00003` |

**修复项（本次实施）**：
- 配置对齐：`server.endpoint` → `8.130.75.157:8088`；yaml 增加 `trajectory_upload.endpoint`
- OpenHands 客户端注入 `X-UEnv-Run-Id`；驱动 GET 路径修正为 `/control/v1/trajectories/{id}`（非 `/body`）
- upload ack 后从 Server 读取 bundle（Worker 本地已删）
- 新增部署脚本：`deploy-adapter-core-75157.sh`、`deploy-openhands-20877.sh`、`deploy-trajectory-chain.sh`、`verify-openhands-trajectory-e2e-20877.sh`

---

## 7. 建议后续动作（按优先级）

### P0 — 配置对齐（7143 + Server）

**7143** `/root/.uenv-worker.env` 或 yaml 补充：

```bash
export UENV_SWE_ARTIFACT_DIR=/var/lib/uenv/swe-artifacts
export UENV_TRAJECTORY_ENDPOINT=http://8.130.75.157:8077
export UENV_TRAJECTORY_TOKEN=<与 Server 一致>
export UENV_SWE_GATEWAY_PUBLIC_URL=http://219.147.100.43:28097
```

**Server 75.157** 启动 `uenv-adapter-core` 时补充：

```bash
export UENV_ADDR=0.0.0.0:8088
export UENV_TRAJECTORY_ENABLED=1
export UENV_TRAJECTORY_HTTP_LISTEN=0.0.0.0:8077
export UENV_TRAJECTORY_DATA_DIR=/var/lib/uenv/trajectory-data
export UENV_TRAJECTORY_TOKEN=<shared-secret>
```

并将 `config/uenv-worker.deploy-7143-swe-pro.yaml` 中 `server.endpoint` 改为 `8.130.75.157:8088`。

### P1 — OpenHands 路径 E2E 验收

1. 208.77 跑 gold：`bash /root/UEnv/scripts/run-openhands-pro-20877.sh gold`
2. 7143 日志确认 `trajectory_uploader_started`、`trajectory_upload_acked`
3. Server 验证：

```bash
curl -sS -H "X-Trajectory-Token: $TOKEN" \
  "http://8.130.75.157:8077/control/v1/trajectories?run_id=<run_id>&limit=10"
curl -sS -H "X-Trajectory-Token: $TOKEN" \
  "http://8.130.75.157:8077/control/v1/trajectories/<trajectory_id>/body"
```

### P2 — 文档与客户端

1. 更新 `260627-swe-openhands-integration-plan.md` §4 轨迹章节，引用 v2.2 Server 聚合。
2. `UEnvGatewayClient.create_session()` 支持可选 `run_id` 头（或 runner 生成 `UENV_RUN_ID` 注入）。
3. `secrets/README.md` §8 增加轨迹 HTTP 与上传 env 说明。
4. 更新 `config/uenv-server.deploy.yaml` 与 worker deploy yaml 中的 Server IP。

---

## 8. 检查方法说明

- 静态代码审阅：`uenv-worker` gateway/swe/trajectory、`uenv-server/trajectory.rs`、`uenv-bridge/core/src/main.rs`
- 配置与脚本：`config/*.yaml`、`scripts/run-openhands-pro-20877.sh`、`scripts/deploy-pro-python-openhands-7143.sh`
- 规划对照：`secrets/README.md`、`Docs/260627-*`、`Docs/260625-trajectory-server-migration-evaluation.md`、`Docs/trajectory_v2.2_changes_summary.md`
- Git：`git status`、`git log`、`git rev-parse` 对比缓存 remote ref
- **未执行**：远端机器 SSH 探活、实机 curl 验证（需联调环境权限）

---

## 9. 变更记录

| 版本 | 日期 | 说明 |
|------|------|------|
| v1.0 | 2026-06-27 | merge 完成后首次链路审计 |
