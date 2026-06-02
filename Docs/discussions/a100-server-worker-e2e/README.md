# A100 实机联调讨论：Server + Worker 真实组合

> **状态**：讨论中 / 待执行  
> **创建**：2026-05-30  
> **目标**：在两台 A100 Linux 测试机上完成 **uenv-server + uenv-worker** 真实组合的 GSM8K 全链路联调；**Bridge 与 Hub 层先用 Mock 方式模拟**，不阻塞 Server-Worker 验收。

---

## 1. 背景与决策

### 1.1 已完成

- Worker 侧 MVP：控制面、预热池、WAL、gsm8k 插件（Linux UDS）
- Proto 统一：`proto/uenv/v1/` 为 L1 权威；`uenv-server` 已迁移至 `ControlPlaneService` + `WorkerGrpcService`
- Mock 路径回归：`uenv-mock-scheduler` + `uenv-worker` 本机/混沌测试通过

### 1.2 本次联调范围（In Scope）

| 组件 | 部署 | 说明 |
|------|------|------|
| **uenv-server** | 机器 A（7143） | 真实调度控制面 |
| **uenv-worker** | 机器 B（7142） | gsm8k 执行 |
| **Bridge** | Mock | 直接用 `grpcurl` / 小型 Rust/Python 客户端调用 `SubmitEpisode` |
| **Hub** | Mock / 跳过 | 不启动真实 Hub；Worker 使用本地 `plugins/gsm8k/manifest.yaml` |

### 1.3 不在本次范围（Out of Scope）

- 真实 `uenv-bridge` 嵌入 ROLL/VeRL 训练循环
- Hub pull 环境定义、镜像分发
- 多步 Episode / PACING StreamReport
- Podman 后端

---

## 2. 推荐拓扑

```
┌──────────────────────────── 机器 A (SSH :7143) ────────────────────────────┐
│  uenv-server :50051                                                         │
│    · UEnvService      ← Mock 客户端 (grpcurl / 脚本)                         │
│    · ControlPlaneService ← Worker 注册/心跳/ReportResult                     │
└───────────────────────────────────┬─────────────────────────────────────────┘
                                    │ 50051 / 50052 互通
┌──────────────────────────── 机器 B (SSH :7142) ────────────────────────────┐
│  uenv-worker :50052                                                         │
│    · WorkerGrpcService  ← Server 主动 Dispatch                               │
│    · plugins/gsm8k 子进程                                                    │
└─────────────────────────────────────────────────────────────────────────────┘
```

SSH 与密钥说明：[secrets/README.md](../../secrets/README.md)（目录已加入 `.gitignore`）

---

## 3. 当前仍缺失的内容

### 3.1 P0 — 阻塞实机联调

| # | 缺失项 | 说明 | 负责建议 |
|---|--------|------|----------|
| 1 | **跨机 SubmitEpisode 端到端脚本** | 需可重复执行的 fixture 提交工具（等价 mock 的 `episode_001.pb` 字段），经 Server `SubmitEpisode` 触发完整链路 | Server/Worker |
| 2 | **Worker 心跳 load 真实上报** | 当前 Worker `load` 恒 0；Server 调度仅依赖本地 increment/decrement，与心跳不一致 | Worker |
| 3 | **Unix 环境实机回归证据** | Windows CI 上 gsm8k 插件测试为 `0 tests`；须在 A100 上跑通 `m5`/`m6` 集成测试并留痕 | Worker |
| 4 | **防火墙 / 端口验收清单** | A→B:50052、B→A:50051 需实测；Worker `endpoint` 必须填 **机器 B 对外可达 IP** | 运维 |
| 5 | **联调记录模板落地** | 首次跨机 Server+Worker 日志交叉验证（M7 清单 `[ ]` 项） | 全员 |

### 3.2 P1 — 不阻塞首轮联调，但影响生产语义

| # | 缺失项 | 说明 |
|---|--------|------|
| 6 | **Bridge Mock 客户端** | 最小 Python/Rust 客户端：读 fixture → `SubmitEpisode` → 打印 `EpisodeResult`；替代真实 Adapter |
| 7 | **Hub Mock 策略文档化** | 明确「本地 manifest 降级」为 Phase 0 默认；Hub 种子无 `gsm8k`，无需启动 Hub |
| 8 | **ResourceSpec 注册** | Worker Register 时 `resource` 字段未填；Server 调度未做资源过滤 |
| 9 | **DrainCommand / epoch 联动** | 心跳响应中的 Drain、epoch 变更后 Worker 再注册 — 未组合验收 |
| 10 | **StreamReport report_type** | proto 已扩展 `ReportType`；Worker 仍主要写 `phase` 字符串 |

### 3.3 P2 — 后续迭代

| # | 缺失项 |
|---|--------|
| 11 | Hub 发布 `gsm8k` manifest + Worker pull |
| 12 | 真实 GEMAdapter / VeRLAdapter |
| 13 | 多步 Episode + PACING |
| 14 | WorkerPoolRegistry 只读查询与 warm 状态 |

---

## 4. Mock 层替代方案

### 4.1 Bridge Mock

**目标**：不依赖 `uenv-bridge` 包，验证 `UEnvService.SubmitEpisode`。

**方案 A — grpcurl（最快）**

```bash
# 在机器 A 或开发机，向 Server 提交（需准备 JSON 与 proto include 路径）
grpcurl -plaintext -import-path ./proto -proto proto/uenv/v1/server.proto \
  -import-path ./proto -proto proto/uenv/v1/episode.proto \
  -d @  <机器A_IP>:50051 uenv.v1.UEnvService/SubmitEpisode <<'EOF'
{
  "episode_id": "gsm8k-e2e-001",
  "attempt_id": 1,
  "env_type": "gsm8k",
  "payload": "{\"question\":\"If 3 books cost $12, what is the cost of 5 books?\"}",
  "mode": "MODE_SINGLE",
  "max_steps": 1,
  "correlation_id": "e2e-trace-001",
  "timeout_seconds": 120,
  "reward_config": "{\"type\":\"rule_reward\",\"target\":\"20\"}"
}
EOF
```

**方案 B — 小型 Rust/Python 客户端 crate**（待建）：读取 `fixtures/gsm8k/episode_001.textproto` 转换后调用。

### 4.2 Hub Mock

**策略**：不启动 `uenv-hub`。

| Hub 能力 | Mock 替代 |
|----------|-----------|
| 环境 manifest | `plugins/gsm8k/manifest.yaml` |
| interface schema | 本地约定 + `fixtures/gsm8k/` |
| 版本解析 | Worker 配置 `UENV_ENV_TYPES=gsm8k` 静态列表 |
| 模板/scaffold | 跳过 |

Hub 恢复接入时参考：[PROTOCOL.md](../../PROTOCOL.md) §6、[260528-1722-worker-next-phase-plan.md](../260528-1722-worker-next-phase-plan.md) §3.8。

---

## 5. 实机联调步骤（草案）

### 5.1 机器 A — Server

```bash
cd UEnv && make proto
cargo build -p uenv-server --release

./target/release/uenv-server -b 0.0.0.0:50051
```

### 5.2 机器 B — Worker

```bash
export UENV_SCHEDULER_MODE=remote
export UENV_SERVER_ENDPOINT=<机器A_IP>:50051
export UENV_WORKER_LISTEN=0.0.0.0:50052
export UENV_ENV_TYPES=gsm8k
export UENV_PLUGIN_DIR=./plugins

./target/release/uenv-worker serve --config config/uenv-worker.yaml
```

> `config/uenv-worker.yaml` 中 `server.endpoint` 须指向机器 A。

### 5.3 验收检查

1. Worker 日志：`control_plane_mode_remote`、register ok  
2. Server 日志：`control_plane_register`  
3. Mock 客户端 `SubmitEpisode` → `EpisodeResult.status=completed`、`summary.total_reward=1.0`  
4. `curl http://<机器B>:19090/metrics | grep uenv_episode`  
5. 保存双方日志片段至本目录 `records/`（待建）

---

## 6. 讨论议题（待收敛）

- [ ] Worker `endpoint` 注册为内网 IP 还是公网 IP（取决于 A→B 路由）
- [ ] 是否在 Server 侧内置「单测 fixture 自动 Submit」模式，减少 grpcurl 手工成本
- [ ] 联调通过后是否将 M7 清单 `[ ]` 项勾选并合并进 [worker-pool-mvp-checklist.md](../worker-pool-mvp-checklist.md)
- [ ] Bridge Mock 客户端放在 `uenv-bridge/examples/` 还是独立 `tools/submit-fixture/`

---

## 7. 相关文档

- [PROTOCOL.md](../../PROTOCOL.md) — 统一协议规范
- **[260530-full-stack-integration-gaps.md](../260530-full-stack-integration-gaps.md)** — Bridge merge 后全栈缺口复核（**推荐阅读**）
- [worker-pool-mvp-checklist.md](../worker-pool-mvp-checklist.md) — M7 退出标准
- [260528-1722-worker-next-phase-plan.md](../260528-1722-worker-next-phase-plan.md) — 下一阶段规划
- [secrets/README.md](../../secrets/README.md) — A100 SSH 与 mock-scheduler 联调（proto 统一前编写，Server 联调以本文为准）

---

## 8. 会议 / 讨论记录

| 日期 | 参与者 | 结论 |
|------|--------|------|
| 2026-05-30 | — | 初稿：确定 Server+Worker 实机范围；Bridge/Hub Mock；列出 P0 缺失项 |

后续讨论请在本目录追加 `YYYY-MM-DD-<topic>.md` 或在 §8 表格中更新。
