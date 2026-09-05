# SWE-bench-Pro Phase2：Gold 验收与 LLM 小批量验证（20260721–22）

- Phase2 实验根目录：`/data/ronghao/uenv/uenv-bridge/temp/benchmarks/swebenchpro/phase2_20260721_233556/`
- Phase2b 小批量（**有效批次**）：`.../phase2b_20260722_132947/exp_limit10_iter100_temp1_localws`
- 关联：[Smoke/阶段5报告](./SWE-bench-Pro-Smoke与阶段5调参实验报告-20260721.md) · [调试规划](./SWE-bench-Pro-Workspace-Session串线调试规划.md) · [0通过根因](./SWE-bench-Pro-0通过根因分析报告-20260721.md)

---

## 1. 验收结论（resolved 已变化）

| 阶段 | resolved | diff>0 | 说明 |
|---|---:|---:|---|
| **Gold S1（qutebrowser）** | **1/1** | 1/1 | `tests=56/56`，`reward=1.0`；判分链路验收 |
| **LLM LIMIT=10（phase2b，registry 修复后）** | **3/10** | **8/10** | 首次出现 LLM `resolved=true` |
| LLM LIMIT=10（修复前 `130454`） | 0/2 | 0/2 | 工具仍走本地 executor，`diff=0` |

**判分链路已验证通过**：Adapter → Server → AgentJob → Gateway apply/submit → Worker grader 可给出 `resolved=true`。

**LLM 小批量已越过「resolved 恒为 0」门槛**：修复后 Agent 在容器 `/app` 内产生有效 diff，且部分实例全测通过。

---

## 2. Phase2b 小批量明细（`phase2b_20260722_132947`）

参数：`LIMIT=10`，`iter=100`，`temp=1.0`，`top_p=0.95`，`max_tokens=2048`，`thinking_budget=1024`，`local-ws` + gateway 工具修复已部署。

| # | repo | status | resolved | diff | tests | 备注 |
|---|---|---|---:|---:|---|---|
| 1 | NodeBB/NodeBB | failed | ✗ | — | — | ContextWindowExceeded（修复前 config 仍 4096 output） |
| 2 | qutebrowser/qutebrowser | completed | **✓** | 4373 | 56/56 | S1 同类，**resolved** |
| 3 | NodeBB/NodeBB | completed | ✗ | 1708 | 0/186 | 有 diff，测例未过 |
| 4 | qutebrowser/qutebrowser | completed | ✗ | 1796 | 192/202 | 有 diff，差 10 测例 |
| 5 | ansible/ansible | completed | ✗ | 973 | 62/175 | 有 diff |
| 6 | ansible/ansible | failed | ✗ | — | — | ContextWindowExceeded |
| 7 | internetarchive/openlibrary | completed | **✓** | 973 | 9/9 | **resolved** |
| 8 | qutebrowser/qutebrowser | completed | **✓** | 10140 | 32/32 | **resolved** |
| 9 | gravitational/teleport | completed | ✗ | 704 | 0/43 | 有 diff，测例 0 过 |
| 10 | navidrome/navidrome | completed | ✗ | 17055 | 0/3 | 有 diff，测例 0 过 |

汇总：**completed=8，failed=2（均为 ctx_fail），resolved=3，diff_gt0=8，openlibrary 串线=0**。

结果文件：`phase2b_20260722_132947/.../uenv_results.jsonl`

---

## 3. 本次修复内容（代码与运维）

### 3.1 根因：OpenHands Tool Registry 闭包固化（P0，本次关键）

**现象**：`tool_patch_status.json` 显示 `UEnvGatewayTerminalExecutor` / `UEnvGatewayFileEditorExecutor`，`patch_ok=true`，但 Agent 实际仍用本地 `TerminalExecutor` / `FileEditorExecutor`，在 208.77 宿主机 `/tmp/<repo>` 或历史 `/app` 上改代码；Worker 轨迹仅含 probe/submit 的 gateway exec，`git_diff` 在容器内为 0。

**原因**：OpenHands `register_tool()` 在 **import 时** 将 `Tool.create` 闭包进 registry；后续仅 monkeypatch `TerminalTool.create` / `FileEditorTool.create` **不会** 更新 `resolve_tool()` 实际调用的 resolver。`Agent._initialize()` 走 registry，故仍绑定本地 executor。

**修复**（`integrations/openhands/uenv_runtime/gateway_tools.py`）：

1. patch 后对 `terminal` / `file_editor` 调用 `register_tool(...)` **重注册**
2. `collect_tool_patch_status` 改为经 `resolve_tool()` 检测（与 `Agent._initialize` 同路径）
3. terminal 增加 `/tmp` clone 软拦截；file_editor 保留 `/app` path guard
4. `run_swebenchpro_official.py` prompt 明确禁止 `git clone` 到 `/tmp`

**验证**（208.77 实机）：`Agent._initialize` 后 `tools_map` 为 `UEnvGatewayTerminalExecutor` / `UEnvGatewayFileEditorExecutor`；新 run stdout 出现 `/app/src/...` 路径，不再出现 `/tmp/qutebrowser/...` 本地编辑。

### 3.2 既有修复（Phase2 前半段，仍保留）

| 项 | 文件 | 说明 |
|---|---|---|
| Host 工作目录隔离 | `workspace.py` | `working_dir=/tmp/uenv-oh-local-ws`（占位），`container_working_dir=/app` |
| Gateway 工具 patch | `gateway_tools.py` | terminal/file_editor 路由到 Gateway |
| Prompt / path guard | `run_swebenchpro_official.py` | 禁 openlibrary、禁只改 tests、workspace probe |
| 208.77 部署隔离 | `deploy-openhands-20877.sh` | 迁移 host `/app` openlibrary 残留 |
| Gold 模式 | `evaluate_swebenchpro_uenv.py` 等 | `AGENT_MODE=gold\|llm` |

### 3.3 运维与参数

- 重启 Worker（7143）、Server（75.157）、Agent poller（208.77），消除 stale agent / dispatch 卡住
- LLM config `openhands-llm-qwen3-temp1-topp095.json`：`max_output_tokens` 4096→**2048**，`thinking_token_budget`→**1024**（缓解 65536 ctx 顶满）
- 新增脚本 `uenv-bridge/scripts/benchmark/swe_pro_phase2b_limit10.sh`

---

## 4. 是否修改了「框架源代码」

**未修改**以下框架/核心组件源码：

| 组件 | 是否改动 |
|---|---|
| OpenHands SDK / `openhands-tools`（vendor） | **否** — 仅运行时 monkeypatch + `register_tool` 重注册 |
| `uenv-worker` / `uenv-adapter-core`（Rust） | **否** |
| vLLM / Gateway Rust 实现 | **否** |
| SWE 镜像 / env package | **否** |

**仅改动 UEnv 集成层与评测脚本**（均为本仓库 adapter/bridge 代码）：

```
integrations/openhands/run_swebenchpro_official.py
integrations/openhands/uenv_runtime/gateway_tools.py
integrations/openhands/uenv_runtime/workspace.py
integrations/openhands/uenv_runtime/workspace_probe.py   (新增)
integrations/openhands/uenv_runtime/workspace_utils.py   (新增)
integrations/openhands/tests/test_workspace_probe.py       (新增)
scripts/deploy-openhands-20877.sh
uenv-bridge/scripts/benchmark/evaluate_swebenchpro_uenv.py
uenv-bridge/scripts/benchmark/run_swebenchpro_uenv_baseline.sh
uenv-bridge/scripts/benchmark/swe_pro_phase2b_limit10.sh  (新增)
```

另在 **208.77 运行时** 更新了 `config/openhands-llm-qwen3-temp1-topp095.json`（部署机配置，非框架源码）。

---

## 5. 仍存在的问题（P1/P2）

| 问题 | 影响 | 建议 |
|---|---|---|
| ContextWindowExceeded | 2/10 failed；NodeBB 等长上下文实例易顶满 65536 | 已降 output tokens；后续考虑 condenser / 更高 ctx vLLM |
| diff>0 但 resolved=0 | 5/8 completed 有 patch 未过测 | 模型能力 + prompt；非路径串线 |
| NodeBB tests 0/186 | 有 diff 但全挂 | 单独轨迹复盘 |
| 全量 731 | — | **仍暂停**；建议先再跑 20–50 条确认 resolved 率稳定后再扩 |

---

## 6. 下一步

1. 将 `gateway_tools.register_tool` 重注册修复合入并同步 208.77（避免手工 scp）
2. 用 `max_output_tokens=2048` 重跑曾 ctx_fail 的 NodeBB 实例，确认不再顶满
3. 扩 LIMIT=20~50，观察 resolved 率与 ctx_fail 比例
4. 更新 [0通过根因报告](./SWE-bench-Pro-0通过根因分析报告-20260721.md) 闭环状态（已见该文档 §后续）

---

## 7. 修订记录

| 日期 | 说明 |
|---|---|
| 2026-07-21 | 初版：Gold resolved=1；LLM 仍 52/56 |
| 2026-07-22 | Phase2b LIMIT=10：**resolved=3/10，diff_gt0=8/10**；记录 registry 闭包根因与修复范围；明确未改框架源码 |
