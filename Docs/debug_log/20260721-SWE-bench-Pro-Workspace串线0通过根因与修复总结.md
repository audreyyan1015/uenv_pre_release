# SWE-bench-Pro Workspace 串线 0 通过：核心问题与修复总结

- **日期**：2026-07-21 ~ 2026-07-22
- **范围**：UEnv SWE-bench-Pro + OpenHands Agent（Qwen3.6-35B-A3B）全量评测 `resolved=0`
- **来源**：汇总自 `Docs/worker/260721/` 四份报告（根因分析、调试规划、Smoke/调参、Phase2 Gold+LLM）
- **关联**：
  - [0 通过根因分析报告](../worker/260721/SWE-bench-Pro-0通过根因分析报告-20260721.md)
  - [Phase2 Gold 与 LLM 推进报告](../worker/260721/SWE-bench-Pro-Phase2-Gold与LLM推进报告-20260721.md)
  - [Smoke 与阶段 5 调参报告](../worker/260721/SWE-bench-Pro-Smoke与阶段5调参实验报告-20260721.md)
  - [Workspace/Session 串线调试规划](../worker/260721/SWE-bench-Pro-Workspace-Session串线调试规划.md)
  - [OpenHands 参数与 Workspace 排查建议](./20260721-SWE-bench-Pro-OpenHands参数与Workspace优先级排查建议.md)

---

## 1. 一句话结论

全量评测 **0 resolved** 的阻断根因不是 Worker 镜像选错，而是 **208.77 上 OpenHands Terminal/FileEditor 未真正经 Gateway 进入 instance 容器**（本地改 openlibrary / `/tmp/<repo>`，submit 却在远程正确容器判分）。P0 在 UEnv 集成层闭环后，Gold `resolved=1`，LLM LIMIT=10 达到 **`resolved=3/10`、`diff_gt0=8/10`**。

---

## 2. 核心问题清单

| 优先级 | 问题 | 现象 | 根因 |
|---|---|---|---|
| **P0** | Agent 工具未走 Gateway | stdout 大量 `openlibrary/`；`git_diff_bytes=0`（82/86）；全语言 0 resolved | 本机 `/app`=openlibrary；`register_tool` 闭包固化导致 monkeypatch 无效，仍用本地 executor |
| **P0′** | 可观测性不足，无法自证工具路径 | `reset_observation={}`；无 `tool_patch_status` / `workspace_probe` | attach 空 observation；patch 状态未落盘 |
| **P1-A** | ContextWindowExceeded | 全量 failed 中 ~43/46；小批量仍有 2/10 | `max_model_len=65536` + `max_tokens=8192` + 多轮 history |
| **P1-B** | 有 diff 仍未 resolved | Phase2b：5/8 completed 有 patch 未过测 | 模型能力 / prompt（P0 修好后才可评估） |
| **P2** | Session 销毁缺失 | 7143 stale 容器 Up 数小时 | job 结束未稳定 `DELETE /sessions`（非 openlibrary 污染直接原因） |

**已排除**：Worker catalog / 镜像 provision、Server→Worker `session_id` 映射（三端 id 一致，问题在 Agent 执行层）。

### 2.1 失败形态对照

| 类型 | 信号 | 归属 |
|---|---|---|
| T4 | stdout 出现其他 repo（openlibrary） | **P0** |
| T2 | completed + diff=0 + tests>0 | **P0** + PASS_TO_PASS |
| T3 | completed + diff=0 + tests=0 | **P0** 或无有效修改 |
| T1 | failed + ContextWindow | **P1-A** |
| T5 | diff>0 但 resolved=false | **P1-B** |

### 2.2 机制示意（修复前）

```text
208.77 OpenHands
  Terminal / FileEditor ──► 本机 /app 或 /tmp/<repo>   ← P0
  submit()              ──► Gateway ──► 7143 正确容器   ← 正常
结果：容器 git_diff=0，resolved 恒 false
```

---

## 3. 相应修复

### 3.1 P0：强制工具链走 Gateway（已闭环）

| 修复项 | 做法 | 位置 |
|---|---|---|
| Host 工作目录隔离 | `working_dir=/tmp/uenv-oh-local-ws`（占位），`container_working_dir=/app` | `uenv_runtime/workspace.py` |
| Registry 闭包修复（关键） | patch 后对 `terminal` / `file_editor` **`register_tool` 重注册**；`tool_patch_status` 经 `resolve_tool()` 检测 | `uenv_runtime/gateway_tools.py` |
| Path guard | 拦截 `/tmp` clone、误改非 `/app`；prompt 禁止 clone 到 `/tmp` | `gateway_tools.py`、`run_swebenchpro_official.py` |
| Host `/app` 污染清理 | 迁移 openlibrary 残留 + `chattr +i` | `deploy-openhands-20877.sh` |
| Workspace 自检 | `workspace_probe`（pwd / git remote / HEAD），错仓可 hard fail | `workspace_probe.py` 等（新增） |

**未改**：OpenHands SDK / Worker Rust / vLLM / SWE 镜像；仅改 UEnv 集成层与评测脚本。

### 3.2 P0′：可观测性（已落地）

- run 目录落盘 `tool_patch_status.json`、`workspace_probe.json`
- smoke / monitor：`patch_ok`、仓库 probe、交叉污染扫描（T4）
- 编排：`swe_pro_smoke_and_tune.sh` + `swe_pro_monitor.sh`（心跳 / stall 告警）

### 3.3 P1-A：Context 参数（部分缓解）

| 动作 | 效果 |
|---|---|
| E3：`MAX_TOKENS` 8192→4096，thinking budget 降半 | ctx_fail 从 1→0（LIMIT=10），但当时仍 0 resolved（P0 未完全修完） |
| Phase2b：`max_output_tokens`→**2048**，`thinking_budget`→**1024** | 配合 P0 修复后小批量可用；ctx_fail 仍 2/10 |

### 3.4 判分链路（Gold）

- `AGENT_MODE=gold|llm`；Gold S1 qutebrowser：`resolved=true`，56/56，`reward=1.0`
- 证明 Adapter → Server → AgentJob → Gateway apply/submit → Worker grader 正常

---

## 4. 验证时间线

| 阶段 | 结果 | 说明 |
|---|---|---|
| 全量中断（132/731） | resolved=0，diff=0 极高 | 终止并取证，禁止继续全量 |
| Smoke S2/S3（阶段 5 前） | patch_ok、probe 正确，T4≈0 | P0 环境侧初见效；仍 diff=0 |
| 阶段 5 control/E3/E1 | 各组 resolved=0、diff=0 | 调参 alone 无效；暴露仍有本地 executor |
| Phase2 Gold | resolved=1/1 | 判分链路 OK |
| 修复前 LIMIT=10（`130454`） | resolved=0，diff=0 | registry 未重注册 |
| **Phase2b LIMIT=10**（`phase2b_20260722_132947`） | **resolved=3/10，diff_gt0=8/10，串线=0** | P0 闭环验收 |

Phase2b 参数：`iter=100`，`temp=1.0`，`top_p=0.95`，`max_tokens=2048`，`thinking_budget=1024`。

---

## 5. 仍开放项

| 项 | 建议 |
|---|---|
| ContextWindow（NodeBB 等） | condenser / 更高 ctx；或继续压低 output tokens |
| diff>0 未 resolved | 轨迹复盘 + prompt / 迭代对齐公开 OpenHands 口径 |
| Session destroy | job 结束保证 `DELETE /sessions`，清理 stale 容器 |
| 全量 731 | **暂停**；先扩 LIMIT=20~50 看 resolved 率稳定后再恢复 |

---

## 6. 经验与决策规则

1. **Worker provision 正确 ≠ Agent 在正确仓库改代码**；必须以 stdout / probe / `git_diff` 三端对齐。
2. **T4（错仓路径）未清零前，全量与纯调参均无意义。**
3. OpenHands：`monkeypatch Tool.create` 不够，必须 **`register_tool` 重注册**，并用与 `Agent._initialize` 相同的 `resolve_tool()` 自检。
4. 宿主机存在真实 `/app` 仓库时，LocalWorkspace 回退会系统性污染评测。
5. `tests_passed>0` 且 `diff=0` 多为 PASS_TO_PASS，不能当作 Agent 有效修复。

---

## 7. 关键产物路径

| 主机 | 路径 / 说明 |
|---|---|
| 7142 | 全量：`.../qwen3_6_..._20260721_094024/`；调参：`phase_tune_20260721_201827/`；Phase2b：`phase2b_20260722_132947/` |
| 7143 | `/var/log/uenv/worker-swe-pro.log`（`swe_session_provisioned`） |
| 208.77 | `/var/log/uenv/openhands-runs/agent-job-*/`（`tool_patch_status`、`workspace_probe`、`runner_stdout`） |

---

## 8. 修订记录

| 日期 | 说明 |
|---|---|
| 2026-07-26 | 自 `Docs/worker/260721` 四份报告提炼核心问题与修复，写入 `Docs/debug_log` |
