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

### 已完成

- 7143：重编并重启 Worker；`for-episode` 返回非空 `instance_catalog_json`
- 208.77：proto / agent_job / agent_client / driver / pb2 已 put

### 阻塞：Server `8.130.75.157`

当前从开发机与 A100 跳板：**SSH banner / admin:50052 超时**；Worker→`:8088` 可建连但 Register 无响应。  
正统端到端（Poll 到带 catalog JSON 的 AgentJob）必须在 Server 恢复后：

```bash
# 在 8.130.75.157 上
cd /home/uenv/UEnv   # 或实际 repo 根
# 同步含本改动的 uenv-server + proto 后：
bash scripts/deploy-adapter-core-75157.sh
```

然后恢复 Worker `server.endpoint: "8.130.75.157:8088"`（若曾临时改为黑洞口），并带：

```bash
export UENV_WORKER_ALLOW_DEGRADED_START=1
export UENV_WORKER_REGISTER_TIMEOUT_SECS=10
```

E2E 检查：`catalog_resolve.json` 的 `catalog_source` 以 `agent_job.instance_catalog_json` 开头。

### 运维备注（7143）

- `hub.enabled: false` 仍为临时项（Hub 不可用）；勿用本仓库 yaml 的 `true` 覆盖现场。
- 重启脚本里勿对含 `uenv-worker.*serve` 的整段 SSH 命令行 `pkill -f`（会误杀会话）；按 PID 杀或写独立 start 脚本。

---

## 5. 与「Gateway 回退」的关系

Gateway 单样本 GET / driver 回退仍保留，作为 AgentJob 未注入或旧 Server 二进制时的兼容路径。  
新 Server + 新 Worker 上线后，正式流量应走 `instance_catalog_json`，fixture 不应再出现在 `catalog_source` 中。
