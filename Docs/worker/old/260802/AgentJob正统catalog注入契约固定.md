# AgentJob 正统 catalog 注入契约固定

> 日期：2026-08-04  
> 关联：
> - [全量 catalog 后仍命中旧 fixture](./SWE-smith全量catalog后OpenHands仍命中旧fixture-诊断与修复.md)
> - [Smith 全量 catalog 补齐](./SWE-smith全量catalog补齐与Worker重启报告.md)

---

## 1. 结论

正式路径已固定为：**Worker for-episode → Server AgentJob → OpenHands driver**，用 `instance_catalog_json` 携带单样本 mini catalog，不再依赖 Agent 主机 fixture / 全量 EnvPackage。

| 层 | 字段 / 行为 | 状态 |
|---|---|---|
| Proto `AgentJob` | `instances_catalog=23`（可选路径提示）、`instance_catalog_json=24`（正式载荷） | ✅ 已定稿 |
| Worker Gateway | `ForEpisodeResp.instance_catalog_json`；`PASS_TO_PASS` 置空 | ✅ 已部署 7143 并验收 |
| Server | for-episode 解析 → 写入 SWE `AgentJob.instance_catalog_json` | ✅ 代码已改；**待 Server 可 SSH/重启后部署** |
| Python | `AgentJob` / `_job_from_proto` / driver 优先消费 JSON | ✅ 已同步 208.77 |

Worker 侧验收（2026-08-04）：`for-episode` 对 Smith 样本返回 `instance_catalog_json`（约 3KB），含 `problem_statement` / `FAIL_TO_PASS` / `image_cache_key`，`PASS_TO_PASS=[]`。

---

## 2. 契约

### 2.1 形状

```json
{
  "<instance_id>": {
    "instance_id": "...",
    "repo": "...",
    "version": "...",
    "base_commit": "...",
    "problem_statement": "...",
    "patch": "...",
    "test_patch": "...",
    "FAIL_TO_PASS": ["..."],
    "PASS_TO_PASS": [],
    "benchmark_variant": "smith",
    "image_cache_key": "jyangballin/swesmith....:latest",
    "test_cmd": "...",
    "install_cmd": "..."
  }
}
```

也允许裸 `SweInstance` 对象（含匹配的 `instance_id`）；driver 两种都接受。

### 2.2 Driver 解析优先级

1. **`AgentJob.instance_catalog_json`**（正统）
2. 本地 catalog（含 `instances_catalog` 路径提示 / EnvPackage）
3. Gateway `GET /runtime/v1/instances/{id}`（过渡回退）

产物：`catalog_resolve.json` 中 `catalog_source` 应为  
`agent_job.instance_catalog_json -> ...`，且 `has_agent_job_catalog_json=true`。

### 2.3 数据流

```text
Adapter Episode(env_package_id, instance_id, variant)
        │
        ▼
Server  create_session_for_episode → Worker POST .../for-episode
        │                              └─ instance_catalog_json (mini)
        ▼
Server  AgentJob.instance_catalog_json = session.instance_catalog_json
        │
        ▼
208.77  PollAgentJob → _job_from_proto → run_swebenchpro_official.py
        │
        ▼
写 mini catalog 到 output_dir，再跑 OpenHands
```

---

## 3. 改动清单

| 路径 | 说明 |
|---|---|
| `proto/uenv/v1/agent.proto` | 字段 23/24 |
| `uenv-worker/.../runtime_gateway/mod.rs` | for-episode 附带 mini catalog |
| `uenv-server/src/ports.rs` / `support.rs` / `episode.rs` | 解析并填入 AgentJob |
| `integrations/openhands/uenv_runtime/agent_job.py` | dataclass + from_dict |
| `integrations/openhands/uenv_runtime/agent_client.py` | `_job_from_proto` |
| `integrations/openhands/run_swebenchpro_official.py` | 优先消费 JSON |
| `integrations/openhands/uenv_runtime/gen/.../agent_pb2*.py` | 208.77 venv 再生 stubs |
| `uenv-worker/.../control_plane/client.rs` | `UENV_WORKER_REGISTER_TIMEOUT_SECS`（默认 10s），避免 Server 半开连接卡死 Gateway 启动（代码已改；7143 当前仍跑含 for-episode 的既有二进制，全量重编待远端 tree 对齐后） |

---

## 4. 部署与验收

### 已完成（2026-08-04 续）

| 项 | 结果 |
|---|---|
| Server 75.157 | ✅ 已同步 `uenv-server`/`proto`/`uenv-bridge/core`，`cargo build -p uenv-adapter-core --release`，经 **`systemctl restart uenv-server`** 换上新二进制 |
| Worker Register | ✅ `worker-7143-pro` 已注册（`server_epoch` 对齐）；`server.endpoint` 已恢复 `8.130.75.157:8088` |
| for-episode（7143 本机） | ✅ `instance_catalog_json` 非空（~3108B） |
| for-episode（Server 经 `:28097` 隧道） | ✅ 同字段可达，Server 可写入 AgentJob |
| OpenHands Agent | ✅ admin `/agents` 见 `openhands-default` 在线 |
| 208.77 driver/stubs | ✅ 此前已 put |

正式 episode 跑完后，在 Agent 产物看 `catalog_resolve.json`：`catalog_source` 应以 `agent_job.instance_catalog_json` 开头。

### 运维备注

- **Repo 根目录**：Server 上为 **`/home/uenv`**（不是 `/home/uenv/UEnv`）；进程由 **`uenv-server.service`** 托管，二进制 **`/usr/local/bin/uenv-adapter-core`**。
- **Obs 临时关闭**：启动曾卡在 Obs 初始化（`:8088` 迟迟不 bind）。已加  
  `/etc/systemd/system/uenv-server.service.d/override.conf`：`UENV_OBS_ENABLED=0`。需要前端观测时再打开并排查 `obs.db`。
- **Gateway 隧道**：`uenv-gateway-tunnel-7143.service`（Server `127.0.0.1:28097` → 7143）。
- 7143：`hub.enabled: false` 仍为现场临时项；勿用本仓库 yaml 的 `true` 盲目覆盖。
- 重启 Worker 勿对含 `uenv-worker` 字样的整段 SSH 命令 `pkill -f`；按 `/proc/PID/exe` 匹配二进制。

---

## 5. 与「Gateway 回退」的关系

Gateway 单样本 GET / driver 回退仍保留，作为 AgentJob 未注入或旧 Server 二进制时的兼容路径。  
新 Server + 新 Worker 上线后，正式流量应走 `instance_catalog_json`，fixture 不应再出现在 `catalog_source` 中。
