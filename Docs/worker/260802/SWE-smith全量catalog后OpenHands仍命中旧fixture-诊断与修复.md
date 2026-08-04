# SWE-smith 全量 catalog 后 OpenHands 仍命中旧 fixture — 诊断与修复

> 日期：2026-08-04  
> 关联：
> - [Adapter 诊断说明](../../adapter/20260804-SWE-smith全量catalog修复后仍命中旧fixture诊断说明.md)
> - [Worker 全量 catalog 补齐报告](./SWE-smith全量catalog补齐与Worker重启报告.md)
> 主机：7143 Worker / 208.77 OpenHands Agent / Server `8.130.75.157`

---

## 1. 结论

| 项 | 结果 |
|---|---|
| Adapter 字段 | ✅ 已正确传 `env_package_id=swe-bench-smith`、`benchmark_variant=smith` |
| Worker 内存 catalog | ✅ 已加载 Smith **59136**（合并 Gateway **59867**） |
| AgentJob 是否注入 catalog 路径 | ❌ **否**。proto `AgentJob` 无 `instances_catalog` 字段；`_job_from_proto` 亦不填 |
| OpenHands driver 实际读谁 | ❌ 优先 `/root/UEnv/fixtures/swe/smith_catalog.json`（**5** 条 smoke） |
| 208.77 是否有全量 EnvPackage | ❌ 无 `/var/lib/uenv/envs/swe-bench-smith/...` |
| 修复 | ✅ Gateway 新增单样本查询 + driver/脚本 Gateway 回退；已部署 7143/208.77 |

**一句话**：Worker 全量 catalog 只服务 Gateway 查表/provision；Agent 侧 driver 仍走本地 smoke fixture，且 AgentJob 未注入 catalog 路径。现改为本地 miss 时经 Gateway 拉取单条 instance，避免在 208.77 加载 4.9GiB JSON。

---

## 2. 根因链

```text
Adapter  → Server AgentJob(instance_id, env_package_id, variant=smith)
                │  （无 instances_catalog）
                ▼
208.77 poller → run-openhands-pro-20877.sh
                │  .env 默认 UENV_SWE_INSTANCES=pro-full-731.json
                │  smith 分支回退 fixtures/swe/smith_catalog.json（5 条）
                ▼
run_swebenchpro_official.py
                │  _smith_catalog_candidates 原顺序：fixture → smoke → EnvPackage
                │  且 208.77 无 EnvPackage 文件
                ▼
_load_catalog → not in .../smith_catalog.json
```

证据（训练 `verl_swesmith_grpo_train_20260804_134850`）：

```text
instance 'pytest-dev__iniconfig...' not in /root/UEnv/fixtures/swe/smith_catalog.json
```

---

## 3. 修复内容

### 3.1 Worker Gateway（7143）

新增：

```text
GET /runtime/v1/instances/{instance_id}
```

- 从内存 EnvPackage catalog 返回 Agent 所需字段（`problem_statement` / `patch` / `repo` / `FAIL_TO_PASS` 等）
- **故意清空 `PASS_TO_PASS`**，避免单行数 MB 的传输与内存放大
- 需 `X-API-Key`

相关文件：

- `uenv-worker/src/runtime_gateway/mod.rs`
- `uenv-worker/src/swe/instance_pool.rs`（`get_instance`）
- `uenv-worker/src/swe/dataset.rs`（`Serialize`）

### 3.2 OpenHands driver / 脚本（208.77）

| 文件 | 变更 |
|---|---|
| `integrations/openhands/run_swebenchpro_official.py` | EnvPackage 优先于 fixture；本地 miss → Gateway `get_instance` → 写 `instance_catalog.json`；>64MiB 本地 catalog 不整文件加载 |
| `integrations/openhands/uenv_runtime/client.py` | `get_instance()` |
| `scripts/run-openhands-pro-20877.sh` | 从 AgentJob 读 variant；smith+AgentJob 不再绑 smoke / Pro catalog |
| `scripts/openhands/openhands_runner.py` | smith 任务清除环境中的 Pro `UENV_SWE_INSTANCES` |

### 3.3 部署状态（2026-08-04）

| 节点 | 动作 |
|---|---|
| 7143 | 已 rebuild `uenv-worker` 并重启；catalog=**59867** |
| 208.77 | 已同步 driver/client/脚本；`uenv-agent-poller` 已 restart |
| Hub `8.130.95.176:8088` | 当时不可达；7143 临时 `hub.enabled: false`（备份 `*.bak-hub`），否则 prewarm 会因 Hub 网络错误直接 `serve failed` |

---

## 4. 验收

7143 / 208.77 隧道：

```text
GET /runtime/v1/instances/pytest-dev__iniconfig.16793ead.combine_module__lxshiekf
→ instance_id 命中，problem_statement 长度 845
```

208.77：

```text
UEnvGatewayClient.get_instance(...) → GATEWAY_FETCH_OK
agent poller active；gateway tunnel health=ok
```

---

## 5. Adapter 侧建议

1. 可重新拉起正式 SWE-smith 训练；首轮应看到 `catalog_resolve.json` 中 `catalog_source` 形如 `gateway:http://127.0.0.1:28097 -> .../instance_catalog.json`，而不再是 fixture 路径。
2. 若仍失败，优先查 208.77 run 目录下的 `catalog_resolve.json` / `runner_stderr.log`。
3. Hub 恢复后，可将 7143 `hub.enabled` 改回 `true`（或恢复 `*.bak-hub`）并 `SKIP_REBUILD=1` 重启 Worker。

---

## 6. 后续优化（非阻塞）

1. proto `AgentJob` 增加 `instances_catalog` 或直接下发 instance JSON（彻底去掉 Agent 侧查表）。
2. Hub 宕机时 Worker prewarm 不应把网络错误提升为进程失败（与 404 WARN 降级对齐）。
3. 长期：按 `instance_id` 可随机访问的 catalog 索引，替代 4.9GiB 整文件。
