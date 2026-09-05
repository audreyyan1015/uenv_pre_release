# SWE-bench-Pro 全量评测 0 通过根因分析报告

- **日期**：2026-07-21
- **范围**：UEnv SWE-bench-Pro + OpenHands Agent 全量评测（Qwen3.6-35B-A3B）
- **关联规划**：[SWE-bench-Pro Workspace Session 串线调试规划](./SWE-bench-Pro-Workspace-Session串线调试规划.md)
- **执行动作**：已终止 7142 全量进程（132/731 处中断）

---

## 1. 执行摘要

**根本原因（P0，阻断性）**：OpenHands Agent 在 **208.77 本机 `/app` 目录**（已确认为 `internetarchive/openlibrary` 仓库）上执行 terminal / file_editor 工具调用，**未通过 UEnv Gateway 路由到 7143 上对应 instance 的 Docker 容器**。而 `submit` 判分仍走 Gateway 进入正确容器，导致：

- Agent 改的是 **错误的本地 openlibrary 仓库**；
- 容器内 **git diff 为空**（82/86 completed）；
- 偶发 **PASS_TO_PASS**（27 条 completed 有 tests_passed 但 diff=0）；
- **`resolved=true` 始终为 0**。

Worker 侧 **镜像选择与容器 provision 正常**，Server→Worker session 映射在日志层面也一致；问题出在 **208.77 Agent 工具链未真正走 Gateway**（规划文档 P0-A4）。

**次要原因（P1，放大失败率）**：46 条 failed 中 **43 条** 为 `ContextWindowExceededError`（`max_model_len=65536` + `max_tokens=8192` + 多轮 history），与 workspace 串线无关，但会进一步压低通过率。

**结论**：在修复 P0 之前，继续全量或调参 **均无意义**；必须先让 Agent 工具链 100% 经 Gateway 操作远程容器，并清理 208.77 本机 `/app` 污染源。

---

## 2. 本次调试动作

| 步骤 | 位置 | 操作 | 结果 |
|---|---|---|---|
| 终止全量 | 7142 | `pkill` evaluate / podman run（run `20260721_094024`） | **已停止**（132/731，podman Killed） |
| 结果快照 | 7142 | 分析 `uenv_results.jsonl` | 见 §3 |
| Worker 日志 | 7143 | `swe_session_provisioned`、容器 probe | provision 正常；无 destroy 日志 |
| 交叉污染扫描 | 208.77 | 463 runs，`runner_stdout.log` vs `config_snapshot.json` | 见 §4 |
| 本机 workspace 取证 | 208.77 | `/app` git remote、run stdout | **本机 `/app` = openlibrary** |
| Gateway 隧道 | 208.77 | `uenv-gateway-tunnel`、`uenv-agent-poller` | 均为 **active** |

---

## 3. 7142 全量结果快照（终止时）

运行目录：

```text
/data/ronghao/uenv/uenv-bridge/temp/benchmarks/swebenchpro/
  qwen3_6_35b_a3b_uenv_full_thinking8192_budget4096_multilang_observability_20260721_094024/
```

| 指标 | 数值 | 说明 |
|---|---:|---|
| 已写入结果 | **132** / 731 | 终止前进度 |
| `uenv_status=completed` | **86** | Episode 走完 |
| `uenv_status=failed` | **46** | 其中 **43** 条 ContextWindowExceededError |
| **`resolved=true`** | **0** | 无一通过 SWE Pro 判分 |
| completed 且 `git_diff_bytes=0` | **82 / 86** | 容器内几乎无 patch |
| completed 且 `tests_passed>0` 且 diff=0 | **27** | 多为 PASS_TO_PASS |
| completed 且 `git_diff_bytes>0` | **4** | 均未 resolved |
| Go（flipt）completed | 8 | **0** 条 tests_passed>0；7/8 diff=0 |

**按 repo（completed）**：

| Repo | completed | resolved | diff=0 |
|---|---:|---:|---:|
| ansible | 17 | 0 | 17 |
| qutebrowser | 12 | 0 | 12 |
| flipt | 8 | 0 | 7 |
| NodeBB | 5 | 0 | 5 |

**T2 典型样例**（qutebrowser）：

- `instance_qutebrowser__qutebrowser-f91ace96...`
- `git_diff_bytes=0`，`tests_passed=52/56`，`resolved=false`
- 说明：容器基线测试部分通过，但 Agent 未在容器内产生有效 FAIL_TO_PASS patch。

---

## 4. 208.77 交叉污染扫描（实机 2026-07-21）

| 任务 repo | 总 run 数 | stdout 含 openlibrary 的 run 数 | 污染率 |
|---|---:|---:|---:|
| qutebrowser | 63 | **37** | **59%** |
| flipt-io | 43 | **35** | **81%** |
| ansible | 61 | **42** | **69%** |
| gravitational | 50 | **40** | **80%** |
| nodebb | 0 | 0 | — |

**可观测性缺口**：

| 检查项 | 结果 |
|---|---|
| `reset_observation.json` 为空 `{}` | **448 / 448**（100%） |
| `tool_patch_status.json` 落盘 | **0** |
| `workspace_probe.json` | **未实现** |

---

## 5. 根因证据链（P0）

### 5.1 208.77 本机 `/app` 是 openlibrary，不是当前 instance 仓库

```text
/app  git remote: https://github.com/internetarchive/openlibrary.git
HEAD: cd2b2b6e3e6e6dd102bacecc93500f0ce1ac0318
Modify: 2026-07-21 15:05:12  （评测进行中仍被写入）
```

本机存在 `openlibrary/` 目录树，与 qutebrowser / flipt / ansible 等 instance **无关**。

### 5.2 qutebrowser 任务的 Agent stdout 显示 **本地** 执行

Run 目录（208.77）：

```text
/var/log/uenv/openhands-runs/agent-job-swebenchpro-instance_qutebrowser__qutebrowser-ff1c025...-20260721-191134/
```

| 字段 | 值 |
|---|---|
| `config_snapshot.instance` | qutebrowser instance（正确） |
| `agent_job.session_id` | `sess-instance-qutebrowser--...-449`（与 7143 provision 一致） |
| `agent_job.gateway_url` | `http://127.0.0.1:28097` |
| stdout 首段 | 大量 `openlibrary/plugins/...` 路径 |
| Terminal 元数据 | `Working directory: /app` + **`Python interpreter: .../software-agent-sdk/.venv/bin/python`**（208.77 本机 venv） |
| `submit_result` | `tests_passed=0/1537`，`resolved=false` |

**关键判据**：若 terminal 经 `UEnvGatewayTerminalExecutor` 执行，命令应在 **7143 容器**内运行，不应出现 208.77 SDK venv 的 Python 路径。该路径是 OpenHands **本地 TerminalTool** 的典型输出。

### 5.3 7143 Worker provision 正常（排除 Worker 镜像映射错误）

同一 qutebrowser instance 的 Worker 日志：

```text
instance_id=instance_qutebrowser__qutebrowser-ff1c025...
image=jefzda/sweap-images:qutebrowser.qutebrowser-...
container=uenv-swe-instance-qutebrowser--...
msg="swe_session_provisioned"
```

7143 上 ansible 残留容器 workspace probe：`git remote = ansible/ansible`，**无 openlibrary**。

### 5.4 机制解释：「本地改仓 + 远程判分」

```text
┌─────────────────────────────────────────────────────────────────┐
│ 208.77 OpenHands Agent                                          │
│  Terminal / FileEditor  ──► 本机 /app (openlibrary)  ◄── P0 根因 │
│  submit()              ──► Gateway ──► 7143 正确容器  ◄── 正常   │
└─────────────────────────────────────────────────────────────────┘
         │                                    │
         ▼                                    ▼
   修改 openlibrary 本地文件            容器 git diff = 0
   stdout 出现 openlibrary 路径         tests 仅 P2P / 全失败
                                       resolved = false
```

这解释了规划文档中所有矛盾现象：

- Worker provision 日志正确，但 stdout 错仓（T4）；
- `git_diff_bytes=0` 极高；
- `tests_passed>0` 但 diff=0（T2）；
- 全语言、全 repo **0 resolved**。

### 5.5 为何 `patch_openhands_tools_for_uenv()` 未生效（待代码确认）

代码中已有 Gateway patch（`integrations/openhands/uenv_runtime/gateway_tools.py`），且 `run_swebenchpro_official.py` 在启动时调用。但实机表现仍为本地 Terminal。

**可能原因（按优先级）**：

1. **patch 运行时未命中 `isinstance(ws, UEnvWorkspace)`**（例如 PYTHONPATH / `uv run` 导致模块双加载，isinstance 失败，回退 LocalWorkspace executor）；
2. OpenHands SDK 部分工具路径未经过 `TerminalTool.create` patch 点；
3. **`os.path.isdir` 对 `/app` 的 monkey-patch** 使 SDK 认为本机 `/app` 合法，加剧本地执行；
4. 缺少 `tool_patch_status.json` 落盘，**无法从 run 目录自证 patch 是否生效**。

---

## 6. 次要因素（P1，非 0 通过主因）

### 6.1 ContextWindowExceededError（43/46 failed）

典型错误：

```text
maximum context length is 65536 tokens
requested 8192 output + 57345 input >= 65537
```

配置：`MAX_TOKENS=8192`，vLLM `max_model_len=65536`，`MAX_ITERATIONS=50` 多轮 history 触顶。

**影响**：这些样本通常无 submit 或无有效 trajectory，与 P0 的「completed 但 diff=0」形态不同；修复 P0 后仍需调大 context 或降低 max_tokens。

### 6.2 Session 销毁缺失

- 7143 存在 **Up 10 hours** 的 stale ansible 容器；
- Worker 日志 **无** `session_destroy` / `DELETE /sessions` 记录；
- 加剧资源泄漏风险，但 **不是本次 openlibrary 污染的直接原因**（污染来自 208.77 本机 `/app`）。

### 6.3 模型能力 / prompt（P1-B）

在 P0 修复前无法评估。当前 4 条 diff>0 均未 resolved，说明即使偶发产生 patch 也未通过 FAIL_TO_PASS。

---

## 7. 失败形态归类

| 类型 | 数量（132 样本） | 根因 |
|---|---:|---|
| **T4** stdout 含其他 repo（openlibrary） | 208.77 扫描 59–81%/repo | **P0 本地执行** |
| **T2** completed + diff=0 + tests>0 | 27 | **P0** + P2P |
| **T3** completed + diff=0 + tests=0 | 55+ | **P0** + 无有效修改 |
| **T1** failed + ContextWindow | 43 | **P1-A 参数** |
| **T5** diff>0 但 resolved=false | 4 | P1-B（P0 修复后才可评估） |

---

## 8. 修复建议（按优先级）

### P0-1：强制 Agent 工具链走 Gateway（必须）

1. Driver 启动时落盘 `tool_patch_status.json`，记录 Terminal/FileEditor executor 类名；
2. 在第一次 tool call 前执行 `workspace_probe`（Gateway exec：`pwd`、`git remote`、`ls /app`），与 `instance_id` / catalog `repo` 比对，**错仓 hard fail**；
3. 排查 `isinstance(ws, UEnvWorkspace)` 是否在 `uv run` 子进程中失败；必要时改为 `'UEnvWorkspace' in type(ws).__name__` 或注册 hook；
4. **删除或移走 208.77 本机 `/app/openlibrary`**，避免 LocalWorkspace 回退时污染；或改为不存在的路径作为 Agent 本机 cwd。

### P0-2：Session 生命周期

1. 确保 job 结束 `DELETE /sessions/{id}` 并打 `session_destroy` 日志；
2. 清理 7143 stale 容器。

### P0-3：可观测性（规划文档阶段 1–2）

1. Worker provision 后写 `workspace_probe`；
2. Gateway exec 关联 `session_id + instance_id`；
3. 修复 `attach_session()` 空 `observation={}`，改为 attach 后立即 probe。

### P1：Context 参数（P0 通过 smoke 后）

- `max_model_len` 65536 → 131072，或 `MAX_TOKENS` 8192 → 4096；
- smoke 固定 `MAX_ITERATIONS=10` 验证 workspace 后再恢复。

---

## 9. 决策

| 动作 | 建议 |
|---|---|
| 继续全量 731 条 | **禁止** |
| 仅调 max_tokens / context | **不足**，不能修复错仓 |
| 以 Worker provision 日志判断环境正常 | **不足**，208.77 stdout 已证伪 |
| 下一步 | 实现 P0-1 smoke（S1/S2/S3），确认 T4=0 后再谈参数 |

---

## 10. 附录：三端对齐样例（qutebrowser ff1c025）

```text
7143  worker-swe-pro.log
  └─ swe_session_provisioned
       instance_id=instance_qutebrowser__qutebrowser-ff1c025...
       image=jefzda/sweap-images:qutebrowser...
       session_id=sess-instance-qutebrowser--...-449

208.77  agent_job.json
  └─ session_id / gateway_url 与上一致 ✓

208.77  runner_stdout.log
  └─ openlibrary 路径 128 处 ✗
  └─ Python interpreter = 208.77 本机 SDK venv ✗

7142  uenv_results.jsonl
  └─ git_diff_bytes=0, resolved=false
```

**Worker/Gateway 映射一致，Agent 工具执行层不一致 — 这是 0 通过的直接原因。**

---

## 11. 后续修复与验证状态（2026-07-22）

P0「Agent 工具未走 Gateway」已在 **UEnv 集成层**闭环，**未修改 OpenHands SDK / Worker Rust 框架源码**。

### 11.1 修复项对照

| 原 P0 项 | 修复 | 代码位置 |
|---|---|---|
| LocalWorkspace 指向 host `/app`（openlibrary） | `working_dir` 改为 `/tmp/uenv-oh-local-ws`，`container_working_dir=/app` | `uenv_runtime/workspace.py` |
| monkeypatch 后仍走本地 executor | `register_tool()` **重注册** terminal/file_editor（registry 闭包固化） | `uenv_runtime/gateway_tools.py` |
| 误改 `/tmp/<repo>`，容器 diff=0 | path guard + prompt 禁 clone 到 `/tmp` | `gateway_tools.py`、`run_swebenchpro_official.py` |
| host `/app` 污染 | 部署脚本迁移 + `chattr +i` | `deploy-openhands-20877.sh`（运维） |

### 11.2 验证结果

| 验证 | 结果 |
|---|---|
| Gold mode | `resolved=true`，56/56（Phase2） |
| LLM LIMIT=10（`phase2b_20260722_132947`） | **resolved=3/10**，**diff_gt0=8/10**；openlibrary 串线=0 |
| 修复前小批量（`130454`） | resolved=0，diff=0（印证 P0 未修完） |

详见：[Phase2 推进报告](./SWE-bench-Pro-Phase2-Gold与LLM推进报告-20260721.md)。

### 11.3 P1 仍开放

- ContextWindowExceeded：phase2b 中 2/10；已将 `max_output_tokens` 降至 2048（208.77 config）
- diff>0 但 resolved=0：模型 patch 质量，非路径问题

**全量 731**：在更大样本（20–50）稳定 resolved 前仍不建议恢复。

---

## 12. 修订记录

| 日期 | 说明 |
|---|---|
| 2026-07-21 | 终止全量 132/731；三端实机取证；确认 P0 为 208.77 本地 /app 执行，非 Worker 镜像错误 |
| 2026-07-22 | 补充 §11：registry 重注册修复闭环；LLM resolved=3/10 小批量验证；明确未改框架源码 |
