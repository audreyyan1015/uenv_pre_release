# SWE-bench-Pro Workspace / Session 串线调试规划

- 日期：2026-07-21
- 范围：UEnv SWE-bench-Pro + OpenHands Agent 全量评测 `0 resolved` 问题
- 目标：在继续全量或调参前，**用可复现的打点与 smoke 用例**，确认 Agent 是否在**正确 instance 对应仓库**里操作；若否，定位 Session/Gateway/AgentJob 哪一层串线。
- 关联文档：
  - [SWE-bench-Pro OpenHands 参数与 Workspace 优先级排查建议](../../debug_log/20260721-SWE-bench-Pro-OpenHands参数与Workspace优先级排查建议.md)
  - [SWE-bench-Pro OpenHands 工作目录与路径异常诊断报告](../../adapter/20260719-174049-SWE-bench-Pro-OpenHands工作目录与路径异常诊断报告.md)
  - [Qwen3.6-35B-A3B SWE-Pro-OpenHands 公开评测参数对比](../../debug_log/20260721-Qwen3.6-35B-A3B-SWE-Pro-OpenHands公开评测参数对比.md)
  - [SWE-bench-Pro OpenHands Prompt 与可观测性修复记录](../../debug_log/20260720-SWE-bench-Pro-OpenHands-Prompt与可观测性修复记录.md)

---

## 1. 背景与已观测现象

### 1.1 全量进度快照（7142，2026-07-21）

运行目录：

```text
/data/ronghao/uenv/uenv-bridge/temp/benchmarks/swebenchpro/
  qwen3_6_35b_a3b_uenv_full_thinking8192_budget4096_multilang_observability_20260721_094024/
```

| 指标 | 数值 | 说明 |
|---|---:|---|
| 已写入结果 | 130+ | 全量 731 进行中 |
| `uenv_status=completed` | 85 | Episode 走完 |
| `uenv_status=failed` | 45 | 其中 **42** 条 `ContextWindowExceededError` |
| `resolved=true` | **0** | 无一通过 SWE Pro 判分 |
| `git_diff_bytes=0` | **81 / 85** completed | Agent 侧几乎无有效 patch |
| `tests_passed>0` 且 diff=0 | **27** | 多为 PASS_TO_PASS，非 FAIL_TO_PASS 修复 |
| Go completed | 30 | **0** 条 `tests_passed>0` |

### 1.2 208.77 实机异常（P0 证据）

路径：`/var/log/uenv/openhands-runs/agent-job-swebenchpro-instance_qutebrowser__qutebrowser-fd6790fe...-20260721-105142/`

| 字段 | 值 |
|---|---|
| `config_snapshot.json` → `instance` | `instance_qutebrowser__qutebrowser-fd6790fe...` |
| `session_id` | `sess-instance-qutebrowser--qutebrowser-fd6790fe...-339` |
| Agent stdout 中 `git diff` 路径 | **`openlibrary/...`**（非 qutebrowser） |
| `submit_result.json` 测试 | qutebrowser 的 `tests/unit/completion/...` |
| `reset_observation.json` | **`{}` 空对象**（无 workspace 自检信息） |

交叉污染统计（208.77 `runner_stdout.log`）：

| 任务 repo | 含 `openlibrary` 路径的 run 占比（抽样） |
|---|---|
| qutebrowser | **30 / 47** |
| ansible | 多条 run 中 27–95 处引用 |
| flipt (Go) | `git diff` 出现 openlibrary；`find *.go` 命中 `node_modules/...` |

### 1.3 7143 Worker 侧（镜像映射正常）

`/var/log/uenv/worker-swe-pro.log` 中，同一 qutebrowser instance 的 provision 记录：

```text
instance_id=instance_qutebrowser__qutebrowser-fd6790fe...
image=jefzda/sweap-images:qutebrowser.qutebrowser-qutebrowser__qutebrowser-fd6790fe...
container=uenv-swe-instance-qutebrowser--qutebrowser-fd6790fe...-1176395-...
msg="swe_session_provisioned"
```

**结论**：Worker catalog / 镜像选择 / 容器命名 **看起来正确**；问题更可能出在 **Session 生命周期、Gateway 路由、Agent attach、或 OpenHands 工具链** 的某一环。

---

## 2. 怀疑问题清单（按优先级）

### P0-A：Gateway Session 与 Agent 操作容器不一致

**怀疑**：OpenHands 通过 `session_id` 执行 `exec/read/write` 时，实际落在 **另一个 instance 的容器**（常见为 openlibrary 残留），而 `submit` 仍按 catalog 中 instance 跑测试。

**支持证据**：

- 非 openlibrary 任务的 stdout 出现 `/app/openlibrary/...`
- `git_diff_bytes=0` 比例极高（Agent 改错仓 / 未改当前仓）
- `reset_observation.json` 为空，启动时未校验 `/app` 内容

**需排除的变体**：

| 变体 | 描述 |
|---|---|
| P0-A1 | Server 下发的 `AgentJob.session_id` 与 Worker 刚 provision 的 session 不一致 |
| P0-A2 | Gateway `sessions/{id}/exec` 路由到错误 `SweSession`（HashMap 键冲突 / 过期 session 未销毁） |
| P0-A3 | OpenHands `attach_session()` 跳过 create，但未向 Gateway 校验 session 是否仍有效且 instance 匹配 |
| P0-A4 | 208.77 上 **Terminal/FileEditor 未全部走 Gateway**（patch 未生效或部分工具本地执行） |

**代码锚点**：

- Server 创建 session 并写入 AgentJob：`uenv-server/src/service/episode.rs`（`create_session_for_episode` → `AgentJob.session_id`）
- OpenHands attach：`integrations/openhands/uenv_runtime/client.py` → `attach_session()`（`observation={}`，无校验）
- Gateway exec：`uenv-worker/src/runtime_gateway/mod.rs` → `pool.exec(&session_id, ...)`
- Session 池：`uenv-worker/src/swe/instance_pool.rs` → `sessions: HashMap<session_id, Arc<SweSession>>`

---

### P0-B：Session 复用 / 销毁不及时导致跨 job 污染

**怀疑**：上一 job 的 session 或容器状态被下一 job 误用；或 Agent 侧 conversation / 工具输出携带上一任务路径（history 污染）。

**支持证据**：

- 多个不同 repo 的 stdout 均出现 openlibrary 路径
- 全量 batch_size=1，但 Agent poller 与 Worker session 池可能存在 **异步释放窗口**

**验证方向**：

- 同一 `session_id` 是否被两个不同 `run_id` / `instance_id` 使用
- job 结束 `DELETE /sessions/{id}` 是否一定执行
- 容器是否在 `destroy` 后仍被 exec 命中

---

### P0-C：缺少启动前 Workspace 自检，错误进入 LLM 阶段

**怀疑**：`/app` 内容与 `instance_id` / `base_commit` 不一致时仍继续跑 Agent，导致后续所有指标失真。

**支持证据**：

- `reset_observation.json` 为 `{}`
- 排查文档要求但未落地的 `pwd` / `git rev-parse` / `remote -v` 自检

---

### P1-A：上下文窗口不足（与 workspace 无关）

**怀疑**：`max_model_len=65536` + `max_tokens=8192` 导致多轮 agent history 触顶。

**支持证据**：45 failed 中 **42** 条 `ContextWindowExceededError`（input ~57345 + output 8192 > 65536）。

**失败形态**：无 trajectory、无 submit、与 P0 的「completed 但 diff=0」可区分。

---

### P1-B：模型未产出有效 patch（能力 + prompt + 语言）

**怀疑**：在 workspace **正确** 的前提下，模型仍大量 `git_diff_bytes=0` 或改错文件。

**支持证据**：

- Python partial pass 但 diff=0（PASS_TO_PASS）
- Go 30 条 completed 全部 `tests_passed=0`
- 旧版 prompt 曾固定搜 `*.py`（已部署 multilang prompt，需 smoke 确认生效）

---

### P2：评测参数与 Qwen 官方口径不一致

**怀疑**：temperature、iterations、context 等未对齐，会 **压低** pass rate，但 **不能单独解释** openlibrary 路径出现在 qutebrowser 任务中。

详见：[公开评测参数对比](../../debug_log/20260721-Qwen3.6-35B-A3B-SWE-Pro-OpenHands公开评测参数对比.md)

---

## 3. 失败形态分类（用于对号入座）

| 类型 | 关键信号 | 首要怀疑 |
|---|---|---|
| **T1** | `failed` + `ContextWindowExceededError` | P1-A 参数 |
| **T2** | `completed` + `git_diff_bytes=0` + `tests_passed>0` | P0 串线 或 P1-B（仅 P2P 通过） |
| **T3** | `completed` + `git_diff_bytes=0` + `tests_passed=0` | P0 串线 或 P1-B |
| **T4** | stdout 出现 **其他 repo** 路径（如 qutebrowser 任务里的 `openlibrary/`） | **P0-A/B** |
| **T5** | `git_diff_bytes>0` 但 `resolved=false` | P1-B 改错 / 测试未过（workspace 可能正确） |

**调参验证前提**：T4 必须为 0，否则全量继续跑没有意义。

---

## 4. 调试总原则

1. **先停或缩小全量**：在 P0 未澄清前，不要继续消耗 731 条样本。
2. **一次只改一个变量**：smoke 阶段固定 `max_iterations=5` 或 `10`，先验证环境，再谈参数。
3. **三端日志用同一 correlation id 对齐**：`run_id` / `episode_id` / `session_id` / `job_id` / `trajectory_id`。
4. **每条 smoke 必须落盘 workspace 自检 JSON**，不可仅依赖 OpenHands stdout。

---

## 5. Smoke 用例矩阵

选 3 个不同语言、此前均 `0 resolved` 的 instance：

| 代号 | repo | 语言 | instance 示例 | 期望 `/app` 特征 |
|---|---|---|---|---|
| **S1** | `qutebrowser/qutebrowser` | python | `instance_qutebrowser__qutebrowser-f91ace96223cac8161c16dd061907e138fe85111-v059c6fdc75567943479b23ebca7c07b5e9a7f34c` | 含 `qutebrowser/` Python 包；**不应**含 `openlibrary/` |
| **S2** | `flipt-io/flipt` | go | `instance_flipt-io__flipt-c12967bc73fdf02054cf3ef8498c05e25f0a18c0-...`（或 catalog 中任意 flipt） | 大量 `*.go`；**不应**含 openlibrary |
| **S3** | `NodeBB/NodeBB` | js | `instance_NodeBB__NodeBB-04998908ba6721d64eba79ae3b65a351dcfbc5b5-vnan` | `package.json` / NodeBB 结构 |

**运行方式**（7142）：

```bash
cd /data/ronghao/uenv/uenv-bridge

LIMIT=1 INSTANCE_ID='<上表 instance_id>' \
MAX_ITERATIONS=10 \
UENV_ROLLOUT_MODEL_ENDPOINT='http://10.10.20.142:18097/v1' \
bash scripts/benchmark/run_swebenchpro_uenv_baseline.sh
```

每条样本单独目录，便于与 208.77 / 7143 日志对齐。

---

## 6. 打点与验证步骤

### 阶段 0：测试前基线采集（不改代码也可做）

| 步骤 | 位置 | 操作 | 通过标准 |
|---|---|---|---|
| 0.1 | 7142 | 记录 smoke 的 `uenv_request_id`、`run_id`（来自 request payload） | 三条 smoke 各有唯一 id |
| 0.2 | 7143 | `grep '<instance_id>' /var/log/uenv/worker-swe-pro.log \| grep swe_session_provisioned` | `image=` 与 catalog dockerhub_tag 一致 |
| 0.3 | 208.77 | 跑完后读 `/var/log/uenv/openhands-runs/agent-job-*<instance_id>*/config_snapshot.json` | `instance` / `session_id` / `gateway` 与 Server 一致 |
| 0.4 | 208.77 | 读同目录 `runner_stdout.log`，`grep openlibrary` | **S1/S2/S3 均应为 0 匹配** |
| 0.5 | 7142 | 读 `uenv_results.jsonl` 对应行 | 记录 `git_diff_bytes`、`tests_passed/total`、`trajectory_id` |

---

### 阶段 1：Worker / Gateway 层打点（推荐实现）

**目标**：证明 `session_id → container → instance_id` 映射全程一致。

#### 1.1 Worker provision 增强（`uenv-worker/src/swe/session.rs`）

在 `swe_session_provisioned` 日志中 **追加**（或单独 `swe_workspace_probe` 事件）：

```bash
git -C /app rev-parse --show-toplevel
git -C /app rev-parse HEAD
git -C /app remote get-url origin 2>/dev/null || git -C /app remote -v
ls /app | head -20
```

| 字段 | 写入位置 |
|---|---|
| `git_toplevel` | tracing + 可选写入 `ResetObservation.workspace_probe` |
| `git_head` | 与 catalog `base_commit` 比对 |
| `git_remote` | 与 `repo` 字段比对（允许 URL 形式差异） |
| `top_level_entries` | 快速识别 openlibrary / qutebrowser 等 |

**通过标准**：`git_remote` 或目录特征与 **当前 instance 的 repo** 一致；`git_head` 等于或 explainable 地接近 `base_commit`。

#### 1.2 Gateway exec / submit 关联日志（`runtime_gateway/mod.rs`）

每次 `exec` / `submit` 记录：

```text
session_id, instance_id, run_id, episode_id, command_prefix(前80字符)
```

**通过标准**：同一 smoke run 内所有 exec 的 `instance_id` 相同，且等于 AgentJob.instance_id。

#### 1.3 Session 销毁（`destroy`）

在 `DELETE /sessions/{id}` 路径打：

```text
session_id, instance_id, container_name, released=true/false
```

**通过标准**：下一 job 不应再 exec 到已 destroy 的 session_id；若 exec 返回 404，OpenHands 应 fail fast 而非静默使用错误输出。

---

### 阶段 2：OpenHands / Driver 层打点（推荐实现）

**目标**：在 **第一次 LLM tool call 之前** 强制 workspace 自检，并落盘。

#### 2.1 修改 `run_swebenchpro_official.py`

在 `with ws:` 进入后、发送 instruction 前：

```python
probe = ws.execute_command(
    "pwd && git -C /app rev-parse --show-toplevel && "
    "git -C /app rev-parse HEAD && git -C /app remote -v && ls -la /app | head -30"
)
_save_json(out / "workspace_probe.json", {
    "instance_id": args.instance,
    "session_id": ws.session.session_id,
    "repo": row.get("repo"),
    "base_commit": row.get("base_commit"),
    "exit_code": probe.exit_code,
    "stdout": probe.stdout,
    "stderr": probe.stderr,
})
```

**硬失败条件（建议）**：若 stdout 含 `openlibrary` 且 `repo` 不是 `internetarchive/openlibrary`，或 `git_head` 与 `base_commit` 前缀不匹配 → **直接 abort**，返回 infrastructure error，不调用 LLM。

#### 2.2 修复 `attach_session`（`client.py`）

当前 `attach_session()` 设置 `observation={}`，导致 `reset_observation.json` 为空。

建议：

1. 增加 Gateway API：`GET /runtime/v1/sessions/{id}` 返回 `instance_id`、`observation`、可选 `workspace_probe`；或
2. attach 后立即 exec 阶段 2.1 的 probe 命令，写入 `reset_observation.json`。

#### 2.3 确认 UEnv 工具 patch 生效（208.77）

在 run 目录写 `tool_patch_status.json`：

```python
from uenv_runtime.gateway_tools import patch_openhands_tools_for_uenv
patch_openhands_tools_for_uenv()  # 已有
# 追加：记录 TerminalTool / FileEditorTool 的 executor 类名
```

**通过标准**：executor 为 `UEnvGateway*`，而非本地 `LocalWorkspace` 默认实现。

---

### 阶段 3：Server / AgentJob 链路对齐（只读验证）

| 步骤 | 命令 / 位置 | 验证内容 |
|---|---|---|
| 3.1 | Server 日志 `gateway_session_create_done` | 同一 `episode_id` 的 `session_id` + `instance_id` |
| 3.2 | 208.77 `agent_job.json` | `session_id` 与 3.1 一致 |
| 3.3 | 7143 `swe_session_provisioned` | 同一 `session_id`（episode_id 字段）与 3.1 一致 |
| 3.4 | 208.77 `config_snapshot.json` | `session_id` 与 3.1 一致 |

**失败示例**：3.1 为 qutebrowser session，但 3.3 的 container 镜像为 openlibrary → Server→Worker 请求错 instance。

---

### 阶段 4：交叉污染自动化扫描（208.77 一键脚本）

在 208.77 执行：

```bash
RUNS=/var/log/uenv/openhands-runs
for repo in qutebrowser flipt-io nodebb ansible; do
  total=$(grep -rl "$repo" "$RUNS"/*/config_snapshot.json 2>/dev/null | wc -l)
  cross=$(grep -rl "$repo" "$RUNS"/*/runner_stdout.log 2>/dev/null \
    | while read f; do grep -qi openlibrary "$f" && echo 1; done | wc -l)
  echo "$repo total_runs=$total openlibrary_cross=$cross"
done
```

**阶段 4 通过标准**（smoke 后）：`openlibrary_cross=0`（除 openlibrary instance 自身）。

---

## 7. 决策树（smoke 结束后）

```text
                    ┌─ workspace_probe 失败（错 repo / 错 HEAD）
                    │     → 修 P0-A/B/C，禁止全量
                    │
smoke S1/S2/S3 ─────┼─ workspace_probe 通过，但 git_diff=0、resolved=0
                    │     → P0 排除；进入 P1-B prompt/能力 或 小步调参（阶段 5）
                    │
                    └─ workspace_probe 通过，且出现非零 diff 或 partial F2P
                          → P0 基本排除；再跑 10–20 条扩大样本，然后调参（阶段 5）
```

### 阶段 5：参数对照实验（仅 P0 通过后）

固定 workspace 自检通过，再 A/B：

| 实验 | 变更 | 观察指标 |
|---|---|---|
| E1 | `max_model_len` 65536 → 131072 | T1 失败率下降 |
| E2 | `MAX_ITERATIONS` 50 → 100 | 长任务 diff 非空率 |
| E3 | `MAX_TOKENS` 8192 → 16384 | ContextWindow 错误 vs 延迟 |
| E4 | temperature 0.0 → 1.0 | 与 Qwen 官方口径对齐（可选） |

每组 **≥10 条** 已验证 workspace 的样本再统计 `resolved` / `git_diff_bytes>0`。

---

## 8. 关键路径与账号（联调）

| 角色 | 主机 | 路径 / 端口 |
|---|---|---|
| Adapter 评测 | 7142 `219.147.100.43:7142` | 结果：`.../temp/benchmarks/swebenchpro/...` |
| Worker + Gateway | 7143 `219.147.100.43:7143` | Gateway `:28097`；日志 `/var/log/uenv/worker-swe-pro.log` |
| OpenHands Agent | 208.77 `8.130.208.77` | poller：`uenv-agent-poller`；runs：`/var/log/uenv/openhands-runs/` |
| Server | `8.130.75.157:8088` | 日志关键字：`gateway_session_create_done` |

SSH 见 [secrets/README.md](../../../secrets/README.md)。

---

## 9. 交付物清单（调试完成后应有）

| 交付物 | 位置 | 用途 |
|---|---|---|
| `workspace_probe.json` | 208.77 每个 run 目录 | 证明 `/app` 与 instance 一致 |
| `reset_observation.json` | 同上 | 非空，含 issue + probe 摘要 |
| Worker `swe_workspace_probe` 日志 | 7143 | 与 OpenHands 侧交叉验证 |
| Gateway exec 关联日志 | 7143 | session_id 全链路一致 |
| Smoke 三实例对比表 | 本文档附录或新 debug_log | 作为是否恢复全量的依据 |
| 交叉污染扫描结果 | 208.77 脚本输出 | T4 是否为 0 |

---

## 10. 附录 A：三端日志对齐示例

对一条 smoke（以 S1 qutebrowser 为例），应能串起：

```text
7142  uenv_requests.jsonl
  └─ request_id / payload.metadata.instance_id / env_config.workspace_dir

7143  worker-swe-pro.log
  └─ swe_session_provisioned
       episode_id=sess-instance-qutebrowser--...-<N>
       instance_id=instance_qutebrowser__...
       container=uenv-swe-instance-qutebrowser--...
       image=jefzda/sweap-images:qutebrowser...

Server  gateway_session_create_done
  └─ episode_id / session_id / instance_id / run_id

208.77  agent_job.json + config_snapshot.json
  └─ session_id / instance / gateway

208.77  workspace_probe.json
  └─ git_head / git_remote / stdout（无 openlibrary）

7143  swe_trajectory_sealed
  └─ trajectory_id（可选）

7142  uenv_results.jsonl
  └─ trajectory_id / git_diff_bytes / tests_passed / resolved
```

**任意一环 instance_id 或 session_id 不一致，即按 P0 处理。**

---

## 11. 附录 B：当前不建议做的事

| 动作 | 原因 |
|---|---|
| 继续全量 731 条 | T4 交叉污染未清零，样本无效 |
| 只调大 max_tokens / context | 不能修复错仓操作 |
| 以 `tests_passed>0` 判断 Agent 有效 | 可能是 PASS_TO_PASS，且 diff 仍可为 0 |
| 仅看 Worker provision 日志宣告「环境正常」 | 208.77 已证明 provision 对但 stdout 错仓 |

---

## 12. 后续代码改动建议（按实施顺序）

1. **Driver**：`workspace_probe.json` + 错仓 hard fail（`run_swebenchpro_official.py`）
2. **Gateway/Worker**：provision 后 probe + exec 关联日志（Rust）
3. **Client**：`attach_session` 后校验或拉取 observation（Python）
4. **Adapter**：`workspace_dir` 默认改为 `/app`（与 Pro 一致，元数据一致）
5. **通过 P0 后**：再改 vLLM context / iterations（7142 脚本与 208.77 llm config）

---

## 13. 修订记录

| 日期 | 说明 |
|---|---|
| 2026-07-21 | 初版：基于 130 条全量结果、7143 provision 日志、208.77 openhands-runs 实机抽查编写 |
| 2026-07-22 | **P0 闭环**：OpenHands `register_tool` 闭包导致 patch 无效已修复；LLM LIMIT=10 `resolved=3/10`、`diff_gt0=8/10`（`phase2b_20260722_132947`）。改动仅限 `integrations/openhands/*` 集成层，未动 SDK/Worker 框架源码。详见 [Phase2 报告](./SWE-bench-Pro-Phase2-Gold与LLM推进报告-20260721.md) |
