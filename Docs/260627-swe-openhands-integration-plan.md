# SWE-bench Pro + OpenHands 集成方案（最终版）

> **文档版本**：v2.0（整合归档）  
> **日期**：2026-06-27  
> **状态**：**已实装** — 208.77 OpenHands + 7143 Worker Gateway  
> **取代**：`260618-swe-bench-env-hub-worker-plan.md` §5.3–§5.6（SWE/OpenHands 部分）、`260625-openhands-official-integration-plan.md`、`260627-openhands-20877-migration.md`、`260625-swe-pro-trajectory-capture-architecture-discussion.md`  
> **关联**：`260627-swe-openhands-acceptance-report.md`（验收证据）、`secrets/README.md`、`integrations/openhands/PIN.md`

---

## 1. 架构总览

```text
┌─ 208.77（8.130.208.77）OpenHands Agent ────────────────────────┐
│  OpenHands/benchmarks + Software Agent SDK v1.27.0             │
│  run_swebenchpro_official.py + uenv_runtime shim               │
│  openhands-runner :8777 health / :8888 API                     │
│  uenv-gateway-tunnel → localhost:28097                         │
└────────────────────────────┬───────────────────────────────────┘
                             │ SSH 隧道 → 7143 Gateway :28097
┌────────────────────────────▼───────────────────────────────────┐
│  7143（10.10.20.143）uenv-worker                               │
│  runtime_gateway :28097 → SweInstancePool → Pro grader         │
│  TrajectoryStore → GET /runtime/v1/trajectories/{id}           │
└────────────────────────────┬───────────────────────────────────┘
                             │ catalog pull
┌────────────────────────────▼───────────────────────────────────┐
│  Hub 8.130.95.176:8088 — Pro 元数据 only                        │
└────────────────────────────────────────────────────────────────┘

7142（10.10.20.142）：VeRL + 本地 LLM（uenv-llm-gateway :18888），**不**承载 OpenHands。
Hub / Server：**不参与** OpenHands 安装与轨迹存储。
```

| 决策点 | 冻结值 |
|--------|--------|
| OpenHands 部署 | **208.77**（`/opt/openhands/benchmarks`） |
| 沙箱 | **7143** Runtime Gateway（208.77 不 pull 容器） |
| Benchmark | `OpenHands/benchmarks` → `benchmarks/swebenchpro/` |
| 驱动 | `integrations/openhands/run_swebenchpro_official.py` |
| Runtime 对接 | `UEnvWorkspace` + `gateway_tools` → `UEnvGatewayClient` |
| LLM | 7142 公网 `:18888/v1` 或 DashScope；与 Worker `llm.env` 分离 |
| 轨迹真值 | **7143 Worker 本机**；208.77 仅持 `TrajectoryRef` |
| CI 回归 | `run_swebench.py`（零 OpenHands 依赖 duck-type） |

**不采用**：7143 内装 openhands；Hub 下发 OpenHands；`run_pro_agent.py` 主路径；OpenHands monolith `app_server` 协议。

---

## 2. 端口与网络

### 2.1 208.77（阿里云 8C32G）

云安全组已开放：**22**，**5432**，**6379**，**8000**，**8077**，**8088**，**8099**，**8777**，**8888**。

| 用途 | bind | 公网 |
|------|------|------|
| OpenHands runner health | `0.0.0.0:8777` | `8.130.208.77:8777` |
| OpenHands runner API | `0.0.0.0:8888` | `8.130.208.77:8888` |

### 2.2 7143 Worker

| 用途 | 本机 bind | 说明 |
|------|-----------|------|
| Runtime Gateway | `0.0.0.0:28097` | `config/uenv-worker.deploy-7143-swe-pro.yaml` |
| gRPC / health | `28888` / `28777` | 不变 |
| TrajectoryRef 公网 URL | `UENV_SWE_GATEWAY_PUBLIC_URL` | `http://219.147.100.43:28097` |

> 公网 `:28099` 映射 **llm-relay**，**不是** Gateway。

### 2.3 208.77 → 7143 Gateway（SSH 隧道）

208.77 无法访问 A100 内网；A100 公网 `:28097` NAT 未开通。采用 **autossh**：

```text
208.77 127.0.0.1:28097 ──SSH──► 7142:7142 ──► 10.10.20.143:28097
```

| systemd 单元 | 说明 |
|-------------|------|
| `uenv-gateway-tunnel.service` | 模板：`scripts/openhands/uenv-gateway-tunnel.service` |
| `openhands-runner.service` | `scripts/openhands/openhands_runner.py` |

---

## 3. Gateway 契约

| 方法 | 路径 | 说明 |
|------|------|------|
| POST | `/runtime/v1/sessions` | 创建 session |
| POST | `…/exec` \| `…/read` \| `…/write` | Agent 步 |
| POST | `…/submit` | 评测；返回 `trajectory_ref` |
| GET | `/runtime/v1/trajectories/{id}` | 完整轨迹 bundle |
| GET | `/runtime/v1/trajectories` | 本 Worker 列表 |

Pro 工作区 **`/app`**（非 `/testbed`）。鉴权：`X-API-Key: swe-pro-secret`。

---

## 4. 轨迹捕获（Worker 本地真值）

轨迹 **不** 经 Server/Hub；执行 session 的 Worker 落盘：

```text
${UENV_SWE_ARTIFACT_DIR}/
  index/by-id/{trajectory_id}.json   # TrajectoryRef
  bodies/{trajectory_id}.json        # TrajectoryBundle（含 StepTrace）
```

`TrajectoryRef` 字段：`trajectory_id`、`worker_id`、`gateway_base_url`、`instance_id`、`reward`、`step_count`。

OpenHands 客户端 submit 后保存 ref；step 正文向 Gateway GET。

---

## 5. Pin 与配置

详见 `integrations/openhands/PIN.md`：

| 组件 | 值 |
|------|-----|
| Benchmarks SHA | `82687c83dfcc193989336f41d235612c02f2c044` |
| SDK SHA | `43376f1868ffd702746080714a59c16d3f69ec12` |
| SDK 版本 | v1.27.0 |

**208.77 配置**（`config/`）：

| 文件 | 用途 |
|------|------|
| `openhands-20877.env.example` | → `/root/.openhands-20877.env` |
| `openhands-llm-20877.json.example` | OpenHands LLM JSON |
| `uenv-worker.deploy-7143-swe-pro.yaml` | 7143 Pro Worker + Gateway |
| `swe/pro-python-smoke.json` | smoke 实例 catalog |

**环境变量（208.77）**：

| 变量 | 值 |
|------|-----|
| `UENV_GATEWAY` | `http://127.0.0.1:28097` |
| `UENV_GATEWAY_API_KEY` | 与 7143 yaml 一致 |
| `OPENHANDS_RUNNER_*_BIND` | `8777` / `8888` |

---

## 6. 部署与运维

```bash
# 开发机 → 208.77（经 7142 跳板）
UENV_SSH_KEY=secrets/*_8.142 bash scripts/deploy-openhands-20877.sh

# 7143 重启 Gateway（7143 本机）
bash scripts/restart-worker-gateway-28097-7143.sh

# 208.77 跑评测
bash /root/UEnv/scripts/run-openhands-pro-20877.sh gold
MAX_ITERATIONS=50 bash /root/UEnv/scripts/run-openhands-pro-20877.sh llm

# Runner API
curl http://8.130.208.77:8777/health
curl -X POST http://8.130.208.77:8888/v1/runs \
  -H 'Content-Type: application/json' -d '{"mode":"gold"}'
```

**7143 Pro 一键验收**（duck-type，无 OpenHands 包）：

```bash
bash scripts/deploy-pro-python-openhands-7143.sh
```

---

## 7. 代码布局

| 路径 | 角色 |
|------|------|
| `integrations/openhands/uenv_runtime/client.py` | Gateway HTTP 客户端 |
| `integrations/openhands/uenv_runtime/workspace.py` | SDK `LocalWorkspace` → Gateway |
| `integrations/openhands/uenv_runtime/gateway_tools.py` | terminal/file_editor shim |
| `integrations/openhands/run_swebenchpro_official.py` | **主驱动** |
| `integrations/openhands/run_swebench.py` | duck-type CI 回归 |
| `uenv-worker/src/runtime_gateway/` | Gateway 服务 |
| `uenv-worker/src/swe/` | pool / grader / trajectory |
| `scripts/openhands/` | HTTP runner + SSH 隧道 unit |
| `scripts/uenv-llm-gateway/` | 7142 LLM 网关 + vLLM 部署 |

---

## 8. 边界与风险

| 主题 | 策略 |
|------|------|
| VeRL 训练 | 7142 → Server；与 SWE Gateway **并行** |
| OpenReward | 官方托管；Worker 不实现 |
| 镜像 pull | 7143；多 mirror 见验收报告 §4.8 |
| benchmarks↔SDK 漂移 | 双 SHA pin；升级必跑 gold |
| LLM patch 质量 | Agent/模型问题；非 Gateway 回归 |

---

## 9. 变更记录

| 版本 | 日期 | 说明 |
|------|------|------|
| v1.x | 2026-06-18–25 | 7143 Gateway 联调、轨迹、7142 OpenHands 试点 |
| v2.0 | 2026-06-27 | OpenHands 迁至 **208.77**；Gateway **:28097**；runner **8777/8888**；文档整合 |
