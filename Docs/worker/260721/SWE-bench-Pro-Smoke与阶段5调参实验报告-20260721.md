# SWE-bench-Pro Smoke 与阶段 5 调参实验报告

- 日期：2026-07-21
- 实验目录（7142）：`/data/ronghao/uenv/uenv-bridge/temp/benchmarks/swebenchpro/phase_tune_20260721_201827/`
- 编排脚本：`uenv-bridge/scripts/benchmark/swe_pro_smoke_and_tune.sh` + `swe_pro_monitor.sh`（30s 心跳 + 208.77 状态 + 20min stall 告警）
- 前置：全量 731 条已 **停止**；P0 workspace/patch 修复已部署 208.77

---

## 1. 执行摘要

| 结论 | 说明 |
|---|---|
| **P0 环境修复有效** | smoke S2/S3：`patch_ok=True`、workspace_probe 仓库正确、非 openlibrary 任务 stdout 无交叉污染 |
| **通过率未改善** | control / E3 / E1 三组各 10 条，`resolved=0`，`git_diff_bytes>0` 亦为 0 |
| **E3 稳定性略优** | 降 `MAX_TOKENS=4096` + thinking budget 2048：**ContextWindow 失败 0**（control 为 1） |
| **E1 未带来收益** | vLLM `max-model-len=131072` 后 ctx 失败 **2** 条，仍 0 resolved |
| **暂不恢复全量** | 按规划决策树：在 `diff=0 + resolved=0` 且调参无改善前，不应无参全量 |

---

## 2. Smoke 矩阵（§5 S1/S2/S3）

| 用例 | instance | 状态 | patch_ok | workspace_probe | openlibrary stdout | resolved | 备注 |
|---|---|---|---:|---|---:|---:|---|
| **S1** qutebrowser | `...v059c6fdc...` | **failed** | — | — | — | 0 | `error 2003` pickup 超时（全量残留 job 占用 agent） |
| **S2** flipt (Go) | `...c12967bc...` | completed | True | flipt-io/flipt ✓ | 0 | 0 | 测试 0/1，无 patch |
| **S3** NodeBB (JS) | `...04998908...` | completed | True | NodeBB/NodeBB ✓ | 误报¹ | 0 | probe 确认容器内为 NodeBB |

¹ monitor 脚本对 `runner_stdout.log` 做 `grep -ci openlibrary`，S3 verify 报 16 为 **grep 误报**（实际 stdout 无 openlibrary 路径；workspace_probe 已证实 `https://github.com/NodeBB/NodeBB.git`）。

**T4 交叉污染验收**：S2/S3 **通过**（修复后非 openlibrary 任务不再出现 openlibrary 路径）。

**S1 重试**：编排器首轮未含 `wait_agent_idle`（已补脚本）；S1 需在 agent 空闲后单独重跑（见 §6）。

---

## 3. 阶段 5 对比实验（各 LIMIT=10, MAX_ITERATIONS=30）

| 实验 | 参数 | n | completed | failed | **resolved** | diff>0 | ctx_fail |
|---|---|---:|---:|---:|---:|---:|---:|
| **control** | t8192, budget4096, ctx65536 | 10 | 9 | 1 | **0** | 0 | 1 |
| **E3** | t4096, budget2048, ctx65536 | 10 | 10 | 0 | **0** | 0 | **0** |
| **E1** | t8192, budget4096, **ctx131072** | 10 | 8 | 2 | **0** | 0 | 2 |

- 三组 **instance 集合完全相同**（overlap 10/10），可直接对比。
- E3 相对 control：**失败率下降**（0 vs 1），但 **仍未产出 patch / 通过**。
- E1 扩大 context **未改善**，反而 ctx 失败增至 2（可能与 vLLM 重启后 gateway 预算未同步等有关）。

### 3.1 典型失败样例

- **ContextWindow**（control）：`instance_navidrome__navidrome-7073d18b...` → `ContextWindowExceededError`
- **Agent 无 patch**（S2 flipt）：`submit_result` 测试 0/1；stdout 多为 `grep` 探索，未见有效 `file_editor` 修改

---

## 4. 实时监控机制（已落地）

`7142` 上每个 eval 阶段：

1. `run.log` — evaluate 主进程输出  
2. `monitor.log` — 30s 轮询：`results=N completed/failed/resolved/diff_gt0/ctx_fail` + 208.77 最新 run 的 `patch_ok` / `submit` 状态  
3. `orchestrator.log` — 阶段切换与汇总  
4. stall ≥1200s 打印 `WARN stall`（本轮未触发）

查看命令（7142）：

```bash
BASE=/data/ronghao/uenv/uenv-bridge/temp/benchmarks/swebenchpro/phase_tune_20260721_201827
tail -f $BASE/orchestrator.log
tail -f $BASE/exp_e3_limit10_iter30_t4096/monitor.log
```

---

## 5. 根因分层更新

| 层级 | 状态 | 证据 |
|---|---|---|
| **P0 错仓/未走 Gateway** | ✅ 已修复 | patch_ok、probe 正确 repo、T4=0 |
| **P1-A ContextWindow** | 部分缓解 | E3 ctx_fail=0；仍有个别 navidrome 等超长 prompt |
| **P1-B 模型不产出 patch** | 🔴 主阻塞 | 全部实验 `diff_gt0=0`；agent 多停留在搜索/理解 |
| **P1-C 模型能力/参数** | 待验证 | 公开 OpenHands 基线参数尚未完全对齐 |

---

## 6. 下一步建议（未恢复全量）

按规划决策树 `workspace_probe 通过 + git_diff=0 + resolved=0` → **P1-B / 能力层**：

1. **S1 重跑**（agent 空闲 + `wait_agent_idle`）补全 smoke 矩阵  
2. **Gold mode smoke**（1 条）：验证 grader 链路能给出 `resolved=true`  
3. **Prompt / iteration 对齐**：对照 [公开评测参数对比](../../debug_log/20260721-Qwen3.6-35B-A3B-SWE-Pro-OpenHands公开评测参数对比.md) 检查 `MAX_ITERATIONS`（公开可能 100）、system prompt、submit 时机  
4. **轨迹分析**：对 208.77 `openhands-runs` 统计 `file_editor` vs `terminal` 调用比，确认 agent 是否过早 submit  
5. **E2/E4**（若继续调参）：提高 `MAX_ITERATIONS=50/100`；或对齐 thinking budget 与 llm-config  
6. **服务端 pickup 超时**：将 `agent_job_pickup_timeout_secs` 从 30s 提至 120s，避免 smoke 与全量切换时 2003

**验收门槛**：smoke + 小批量（≥10）出现 `resolved≥1` 或 `diff_gt0` 比例显著高于当前 **0%** 后，再恢复全量 731。

---

## 7. 产物路径

| 路径 | 内容 |
|---|---|
| `phase_tune_20260721_201827/smoke_S{1,2,3}/` | Smoke 结果 |
| `phase_tune_20260721_201827/exp_control_*` | 对照组 |
| `phase_tune_20260721_201827/exp_e3_*` | E3 降 token |
| `phase_tune_20260721_201827/exp_e1_*` | E1 扩 context |
| `phase_tune_20260721_201827/orchestrator.log` | 全流程日志 |

关联：[P0 根因报告](./SWE-bench-Pro-0通过根因分析报告-20260721.md) · [调试规划](./SWE-bench-Pro-Workspace-Session串线调试规划.md)
