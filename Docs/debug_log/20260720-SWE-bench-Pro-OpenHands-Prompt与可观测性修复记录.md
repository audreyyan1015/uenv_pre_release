# SWE-bench-Pro OpenHands Prompt 与可观测性修复记录

- 日期：2026-07-20
- 背景：全量 UEnv SWE-bench-Pro 评测（184+ 条）`resolved=0`；Worker/OpenHands 链路排查确认基础设施正常，主因是 **旧版 Python 导向 prompt 未部署**、**catalog 缺 `repo_language`**、**Adapter 未回填 `trajectory_id`/测试摘要**
- 关联：
  - [20260719-174049-SWE-bench-Pro-OpenHands工作目录与路径异常诊断报告.md](../adapter/20260719-174049-SWE-bench-Pro-OpenHands工作目录与路径异常诊断报告.md)
  - [20260720-SWE-bench-Pro-UEnv当前测试结果.md](../adapter/20260720-SWE-bench-Pro-UEnv当前测试结果.md)

---

## 1. 排查结论（修复前）

| 检查项 | 结论 |
|---|---|
| Worker 镜像 / instance 映射 | 正常（`swe_session_provisioned` 与 catalog 一致） |
| OpenHands 工作目录 | 正常（`Working directory: /app`） |
| 测试执行与判分 | 正常（gold/e2e 历史可通过；LLM 运行有 `tests_total>0`） |
| 0 resolved 主因 | 模型未产出可通过测试的 patch；Go/JS 仓库被旧 prompt 引导搜 `*.py` |
| Adapter `trajectory_id` 全空 | **可观测性 bug**：Server 有值，Adapter Core 转 SampleResult 时未透传 |

208.77 上 205 条 LLM `submit_result` 分布：`zero_pass=142`、`partial=63`、`all_pass=0`（LLM 全量尚无 resolved）。Go 仓库日志中 `.py` 提及均值 52 次、`.go` 不足 1 次。

---

## 2. 代码改动

### 2.1 OpenHands prompt（`integrations/openhands/run_swebenchpro_official.py`）

- `_infer_repo_language()`：优先读 catalog `repo_language`，否则按 `repo` 启发式（flipt/vuls/teleport→go 等）
- `_build_instruction()`：按语言选择 `*.py` / `*.go` / `*.js|ts`；要求 `pwd`、`git rev-parse`、`ls` 自检；禁止搜索 `/opt/openhands` 等 Agent 主机目录

### 2.2 Catalog 补齐 `repo_language`

- `scripts/export_swe_pro_instances.py`：导出时写入 `repo_language`
- **新增** `scripts/enrich_swe_pro_catalog.py`：从 `test.jsonl` 合并 `repo_language` 到已有 catalog（注：该脚本在 `.gitignore` 的 `/scripts/*` 规则下，未纳入 git 跟踪）

### 2.3 OpenHands → Server 元数据回填

- `scripts/openhands/openhands_runner.py`：`CompleteAgentJob` 附带 `metadata`（`tests_passed`、`tests_total`、`git_diff_nonempty`、`git_diff_bytes`）
- `integrations/openhands/uenv_runtime/agent_client.py`：支持 `metadata` 参数

### 2.4 Adapter 可观测性

- `uenv-bridge/core/src/core.rs`：将 `trajectory_id` 与 `metadata` 嵌入 `trajectory_json` 根对象（无需改 adapter_core.proto）
- `uenv-bridge/src/uenv/bridge/protocol.py`：`EpisodeResult` 增加 `trajectory_id`、`metadata`
- `uenv-bridge/src/uenv/bridge/clients.py`：从 `trajectory_json` 解析上述字段
- `uenv-bridge/scripts/benchmark/evaluate_swebenchpro_uenv.py`：`result_to_row` 输出 `trajectory_id`、`tests_passed/total`、`git_diff_*` 列

### 2.5 小修复

- `integrations/openhands/uenv_runtime/workspace.py`：`git_diff()` 改为 `git diff`（不再 `git diff -- /app`）

---

## 3. 生产部署（2026-07-20）

| 主机 | 动作 | 状态 |
|---|---|---|
| **208.77** | 同步 `run_swebenchpro_official.py`、`workspace.py`、`agent_client.py`、`openhands_runner.py` | ✅ |
| **208.77** | `pro-full-731.json` 合并 `repo_language`（731/731） | ✅ |
| **208.77** | 重启 `uenv-agent-poller`（见 §4.3） | ✅ |
| **208.77** | 重编 agent proto stub（见 §4.2） | ✅ |
| **7142** | 同步 `evaluate_swebenchpro_uenv.py`、`clients.py`、`protocol.py` | ✅ |
| **Server `8.130.75.157`** | 重编并替换 `uenv-adapter-core`（Jul 20 15:16，`systemctl restart uenv-server`） | ✅ |

### 3.1 Catalog enrich 命令（已在 7142 执行）

```bash
# 从 208.77 拉 catalog → 7142 用 jsonl enrich → 推回 208.77
sshpass -p '...' scp root@8.130.208.77:/root/UEnv/config/swe/pro-full-731.json /tmp/pro-full-731.json
python3 scripts/enrich_swe_pro_catalog.py \
  --catalog /tmp/pro-full-731.json \
  --jsonl /data/ronghao/uenv/uenv-bridge/data/benchmarks/swebenchpro/test.jsonl \
  --out /tmp/pro-full-731.enriched.json
sshpass -p '...' scp /tmp/pro-full-731.enriched.json root@8.130.208.77:/root/UEnv/config/swe/pro-full-731.json
```

验证：`flipt` 样例 `repo_language=go`；`grep _infer_repo_language` 在 208.77 部署文件存在。

### 3.2 Adapter Core 重编（已在 Server 执行）

```bash
# 在含完整 monorepo 且已安装 protoc 的机器上
cd /home/uenv-frontend-add
cp /tmp/core.rs uenv-bridge/core/src/core.rs
cargo build -p uenv-adapter-core --release
cp target/release/uenv-adapter-core /usr/local/bin/uenv-adapter-core
systemctl restart uenv-server   # 勿用 nohup，否则会监听 50051 而非 8088
```

**注意**：首次误用 `nohup` 启动 adapter-core 导致监听 **50051** 而非 **8088/8077**，后改 `systemctl restart uenv-server` 恢复正常。

### 3.3 Agent proto stub 重编（208.77，见 §4.2）

```bash
cd /root/UEnv
cp /tmp/{agent,episode,common}.proto proto/uenv/v1/
cp /tmp/agent_client.py integrations/openhands/uenv_runtime/agent_client.py
/root/uenv-agent-venv/bin/python -m grpc_tools.protoc \
  -I=proto proto/uenv/v1/common.proto proto/uenv/v1/episode.proto proto/uenv/v1/agent.proto \
  --python_out=integrations/openhands/uenv_runtime/gen \
  --grpc_python_out=integrations/openhands/uenv_runtime/gen
systemctl restart uenv-agent-poller
```

---

## 4. Smoke 联调中发现的问题

### 4.1 模型网关端口错误（7142 smoke）

| 项 | 说明 |
|---|---|
| **现象** | smoke 使用 `UENV_ROLLOUT_MODEL_ENDPOINT=http://127.0.0.1:18194/v1`，进程挂起 40+ 分钟无 `uenv_results.jsonl` |
| **根因** | **18194** 无服务；可用端口为 **18094**（`Qwen/Qwen3.6-35B-A3B`） |
| **修复** | 杀掉卡住的 smoke 进程，改用 `18094` 重启 |

### 4.2 Agent proto stub 过旧（208.77，**P0 隐藏 bug**）

| 项 | 说明 |
|---|---|
| **现象** | Server 日志显示 `agent_job_polled`，但 208.77 无新 OpenHands run 目录；smoke 长时间阻塞 |
| **日志** | `PollAgentJob failed: Protocol message AgentJob has no "model_endpoint_config" field.` |
| **根因** | Server 下发的 `AgentJob` 含 `model_endpoint_config`（`proto/uenv/v1/agent.proto` field 13），208.77 上 `uenv_runtime/gen` 的 stub 为旧版，反序列化失败 |
| **表现** | poller 领取 job → 解析失败 → `registration invalidated: poll_failed` → 重新注册 → 循环；**job 实际从未执行** |
| **修复** | 同步最新 proto 到 208.77，用 `uenv-agent-venv` 重编 stub，重启 `uenv-agent-poller` |

此问题解释了为何此前多次 smoke「Worker 已 provision、Server 已 poll」，但 OpenHands 侧始终没有新 run 目录。

### 4.3 Runner 服务入口不一致（208.77）

| 项 | 说明 |
|---|---|
| **现象** | `systemctl is-active openhands-runner` 为 **inactive**，但 `pgrep` 可见 `scripts/openhands/openhands_runner.py` 进程 |
| **说明** | Poll 模式应使用 **`uenv-agent-poller.service`**（`ExecStart=.../scripts/openhands/openhands_runner.py`），而非旧的 `openhands-runner.service`（指向 `services/openhands-runner/openhands_runner.py`） |
| **处理** | 以 `uenv-agent-poller` 为准；`deploy-openhands-20877.sh` 在 `OPENHANDS_ENABLE_POLL=1` 时会 disable 旧 service |

### 4.4 Gateway 隧道健康检查误判

| 项 | 说明 |
|---|---|
| **现象** | `curl http://127.0.0.1:28097/health` 在 208.77 返回 **404**（带 API Key 亦然） |
| **说明** | 隧道已连通（有 TCP 响应）；7143 Worker gateway 对无 Key 请求返回 401，对 `/health` 可能返回 404，**不等于隧道断开** |
| **建议** | 用 `ss -tlnp | grep 28097` 或带正确 API Key 的业务接口验证 |

### 4.5 Adapter Python 直跑需设 protobuf 实现

| 项 | 说明 |
|---|---|
| **现象** | 7142 主机 `PYTHONPATH=src python3 evaluate_swebenchpro_uenv.py` 时 `RustCoreEpisodeClient` 的 grpc stub 为 `None` |
| **修复** | 设置 `PROTOCOL_BUFFERS_PYTHON_IMPLEMENTATION=python` |

### 4.6 core.rs 编译错误（已修）

首次重编 adapter-core 时 `result.summary` 被 move 导致 borrow checker 报错；改为先取 `total_reward` 再 `unwrap_or_default()`。

---

## 5. Smoke 验证结果（2026-07-20 18:00 CST）

**用例**：`instance_flipt-io__flipt-a42d38a1bb1df267c53d9d4a706cf34825ae3da9`，`max_iterations=5`，模型网关 `18094`

| 验证项 | 修复前 | Smoke 结果 |
|---|---|---|
| `trajectory_id` | 全空 | ✅ `trj-worker-7143-pro-1784541602272-00210` |
| `tests_passed` / `tests_total` | 未回填 | ✅ `0 / 3` |
| `git_diff_nonempty` / `git_diff_bytes` | 缺失 | ✅ `0 / 0` |
| `repo_language` | 缺失 | ✅ `go` |
| `uenv_status` | — | ✅ `completed` |
| Prompt（instruction.txt） | `find ... '*.py'` | ✅ `labeled as 'go'; prioritize Go files such as '*.go'` |
| Prompt 路径限制 | 无 | ✅ 禁止搜索 `/opt/openhands` 等 |
| Poller 回填 | proto 失败 | ✅ `completed ... trajectory_id=trj-... acked=True`（约 32s 完成） |

OpenHands run 目录：`/var/log/uenv/openhands-runs/agent-job-swebenchpro-instance_flipt-io__flipt-a42d38a1bb1df267c53d9d4a706cf34825ae3da9-55580351-20260720-175936`

**说明**：smoke 样本 `resolved=false` 符合预期（`max_iterations=5` 过短、模型能力限制）；本次验证目标是**链路与可观测性**，非 resolved 率。

---

## 6. 验证与预期效果（全量）

| 项 | 修复前 | 修复后预期 |
|---|---|---|
| Go 实例 instruction | `find ... '*.py'` | `prioritize Go files *.go` + `repo_language=go` |
| catalog | 731 条 `repo_language=None` | 731 条已补齐 |
| Adapter 结果 | `trajectory_id` 全空 | 非空；`tests_passed/total`、`git_diff_*` 有值 |
| AgentJob 执行 | proto 解析失败，job 未跑 | poller 正常领取并执行 |
| `git_diff` 自检（OpenHands） | 可能因路径参数漏 diff | `git diff` 正常 |

**注意**：prompt/catalog 修复**不保证**立即提升 resolved 率，但应显著减少 Go/JS 仓库中无效的 Python 搜索；resolved 仍主要取决于模型能力与上下文预算（`ContextWindowExceededError` 仍会出现）。

---

## 7. 全量跑批状态

| 项 | 状态 |
|---|---|
| 已完成 | **184/731**（旧 prompt + 旧 catalog，无 `trajectory_id` 等字段） |
| 当前 | 无全量进程在跑 |
| 续跑 | 新 prompt/catalog/proto 仅对新 job 生效；已完成的 184 条不会自动更新 |

---

## 8. 后续建议

1. **P0**：将 agent proto 重编纳入 `deploy-openhands-20877.sh` 常规步骤（或与 monorepo proto 版本校验），避免 Server 升级后 208.77 stub 再次落后。
2. **P1**：决定是否从第 185 条续跑全量，或清空重跑 731 条。
3. **P1**：全量中观察 Go 仓库 `runner_stdout.log` 是否出现 `*.go` / `rg`（对比修复前 `.py` 占比）。
4. **P1**：对 `ContextWindowExceededError` 样本考虑降 `max_iterations` 或加大上下文模型。
5. **P2**：将 `enrich_swe_pro_catalog.py` 纳入 catalog 发布流程；考虑移出 `.gitignore` 或放到 `uenv-bridge/scripts/` 以便版本管理。
6. **P2**：本地代码改动 git commit（9 个修改文件 + 本文档）。

---

## 9. 变更文件清单

| 文件 | 说明 |
|---|---|
| `integrations/openhands/run_swebenchpro_official.py` | 语言感知 prompt + repo 启发式 |
| `integrations/openhands/uenv_runtime/workspace.py` | `git_diff` 修复 |
| `integrations/openhands/uenv_runtime/agent_client.py` | CompleteAgentJob metadata |
| `scripts/openhands/openhands_runner.py` | 回填 tests/diff metadata |
| `scripts/export_swe_pro_instances.py` | 导出 `repo_language` |
| `scripts/enrich_swe_pro_catalog.py` | **新增** catalog enrich（gitignore） |
| `uenv-bridge/core/src/core.rs` | trajectory_json 透传 id/metadata |
| `uenv-bridge/src/uenv/bridge/protocol.py` | EpisodeResult 字段扩展 |
| `uenv-bridge/src/uenv/bridge/clients.py` | 解析 trajectory_json 扩展 |
| `uenv-bridge/scripts/benchmark/evaluate_swebenchpro_uenv.py` | 结果列扩展 |
| `Docs/debug_log/20260720-SWE-bench-Pro-OpenHands-Prompt与可观测性修复记录.md` | 本文档 |

**208.77 生产侧额外操作**（非 git 文件）：`proto/uenv/v1/*.proto` 同步 + `integrations/openhands/uenv_runtime/gen/` 重编。
