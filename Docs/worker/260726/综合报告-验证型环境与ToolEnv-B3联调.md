# 综合报告：验证型环境 / ToolEnv Agent / 金标 Rubric（2026-07-25）

> 范围：按用户顺序完成 **B2 → 208.77 生产化 → math 收敛 → A 金标 → B3 联调 → 关 7142 临时 vLLM**。  
> 关联：
> - [跨模块调整清单](./跨模块调整清单-qa改造与ToolEnv-Agent.md)
> - [实施规划](./验证型环境改造与DSCode-Agent评测-实施规划.md)
> - **[Hub 待调整事宜（单独成文）](./Hub待调整事宜-qa制品与Rubric注册.md)**
> - **[前端观测面与系统能力差距（单独成文）](./前端观测面与系统能力差距-待补齐.md)**

---

## 1. 执行结论（TLDR）

| 序号 | 事项 | 状态 | 关键证据 |
|------|------|------|----------|
| B2 | DSCode Agent 轨道评测产品化 | ✅ | `run_dscodebench_agent_toolenv.sh` + `report_dscode_agentic.py`；7143 上 resume 5 题 `agentic_pass@1=0.8` |
| 208.77 | ToolEnv Agent 生产化 | ✅ | `/opt/uenv-toolenv/{app,venv,sandbox-venv,runs}` + systemd `uenv-toolenv-eval@` / `uenv-toolenv-poller` |
| math | 兼容收敛，确认可下线 | ✅ | Worker `types=["qa","code","swe"]`；`check_qa_math_convergence.py` → `qa_dispatchable=True math_retired=True` |
| A | verifiers Rubric 金标 | ✅ | 对齐率 **96.55%（56/58）**，**过宽=0**；修复 olympmath 子串判满分洞 |
| B3 | Server 编排 code/ToolEnv + 联调 | ✅ | `CodeAgentBackend`；poller `toolenv-default`；`numpy_1` mock **reward=1.0（20/20）** |
| GPU | 关 7142 临时 vLLM | ✅ | `:18088`/`:18099` 已释放；GPU 4/5 显存回到 0 MiB |
| FE | 可视化前端部署 + Obs 冒烟 | ⚠️ 半完成 | Server `8.130.75.157:8888` 可访问；Obs `:50053` + `/obs` 代理 REST/SSE **seed 通**；**尚未**用真实 Bridge/训练链路驱动 UI（见 §4.2、差距专文） |

---

## 2. 按模块汇总的调整

### 2.1 Proto / 契约

| 变更 | 说明 |
|------|------|
| `proto/uenv/v1/agent.proto` | `AgentJob` 新增 `string task_payload_json = 22`，供 code/ToolEnv 透传完整任务 JSON（SWE 路径留空） |
| Python stubs | OpenHands / ToolEnv 侧需重新 `protoc`；208.77 已把 stubs **合并进** `uenv-bridge/src/uenv/v1/`，避免与 `uenv.bridge` 包名冲突 |

### 2.2 Server（`uenv-server` / Adapter Core）

| 变更 | 路径 |
|------|------|
| `CodeAgentSpec` | `uenv-server/src/service/support.rs`：`execution_mode=agent` + `task_id` 解析；不依赖 gateway |
| `CodeAgentBackend` | `uenv-server/src/execution_backend.rs`：`env_type=code` 且 agent 模式时分流 |
| `submit_code_agent_episode` | `uenv-server/src/service/episode.rs`：占 Agent 槽 → enqueue AgentJob（无 worker gateway）→ 等 Complete |
| 部署 | 7142 `/data/ronghao/uenv` release 构建 → Server `/usr/local/bin/uenv-adapter-core`；`systemctl restart uenv-server` |
| 联调日志 | `code_agent_slot_acquired` → `code_agent_job_dispatched` → `agent_job_polled` → `code_agent_episode_completed` |

### 2.3 Bridge / Adapter Core 映射

| 变更 | 路径 |
|------|------|
| code env agent 字段透传 | `uenv-bridge/core/src/core.rs`：`execution_mode` / `agent_*` / `code_problem` / `test_script` 等从 `env_config` 提升到 Server payload |
| 默认 env | `verl_agent_loop.py`：`default_env_type` `math` → `qa` |
| 可选导入 | `uenv/bridge/__init__.py`：`verl_agent_loop` 改为可选（轻量 Agent 机可不装 verl） |

### 2.4 Worker / Plugin（qa 与金标）

| 变更 | 说明 |
|------|------|
| `plugins/qa/` | 加法式插件：`manifest.yaml` + `run.sh` 复用 `uenv-math-plugin` |
| 7143 配置 | `types: ["qa","code","swe"]`，**去掉 math** |
| olympmath 判分修复 | `plugins/math/.../scoring.rs`：拒绝空输出；去掉双向 `contains`；加 `numeric_equivalent` |
| 金标工具链 | `score_corpus.rs` + `data/alignment/qa_rubric_corpus.jsonl` + `verify_qa_rubric_alignment.py` |
| 金标结果 | 对齐 56/58；过严 2 条（自然语言无 `####`；故意拒绝长左侧赋值） |

### 2.5 Hub（本地仓库侧已改；实机见 Hub 专文）

| 变更 | 说明 |
|------|------|
| seed / template | 正式 `qa` seed；`math` 标 deprecated 兼容别名；templates 4→5 |
| 发布脚本 | `hub_publish_qa_env.py`（实机已发 `qa@0.2.0`） |
| **待办** | 见 [Hub待调整事宜](./Hub待调整事宜-qa制品与Rubric注册.md) |

### 2.6 Agent 机 208.77（ToolEnv 生产化 + B3 poller）

| 项 | 内容 |
|----|------|
| 布局 | `/opt/uenv-toolenv/{app,venv,sandbox-venv,runs}` |
| 脚本 | `scripts/deploy-toolenv-20877.sh`、`scripts/toolenv/bootstrap_toolenv_agent.sh` |
| 依赖 | `requirements-control.txt` / `requirements-sandbox.txt` / `-heavy.txt`（含 torch/tf） |
| 评测 oneshot | `uenv-toolenv-eval@.service` |
| 常驻 poller | `uenv-toolenv-poller.service` + `scripts/toolenv/toolenv_agent_poller.py` |
| 提交探针 | `scripts/toolenv/submit_code_agent_episode.py` |
| 隔离 | pool=`toolenv-default` / bridge=`uenv-agent-toolenv@1.0.0`，与 OpenHands 同机隔离 |

### 2.7 评测产品化（B2）

| 产物 | 作用 |
|------|------|
| `dscode_toolenv_agent.py` | `track=agentic`、`agentic_pass@1`、`--output-dir` / `results.jsonl` / `--resume`、分库聚合 |
| `run_dscodebench_agent_toolenv.sh` | 入口；输出根 `temp/benchmarks/dscodebench-agentic/` |
| `report_dscode_agentic.py` | `metrics.json` → `report.md` |
| 文档 | `uenv-bridge/docs/任务测评/DSCodeBench-Agent轨道评测(ToolEnv).md` |

### 2.8 基础设施 / GPU

| 项 | 结果 |
|----|------|
| 7142 临时 vLLM | `:18088` Qwen3-8B、`:18099` Qwen3-14B 已停；端口释放；GPU 4/5 显存清零 |
| 遗留 | 机器上存在大量历史 `<defunct>` vLLM 僵尸（不占显存）；如需清理可择机重启相关父进程 |

---

## 3. B3 联调记录（关键路径）

```
Client (submit_code_agent_episode)
  → Adapter Core (env_type=code, env_config.execution_mode=agent)
  → Server CodeAgentBackend
  → AgentJob(pool=toolenv-default, task_payload_json=…)
  → 208.77 uenv-toolenv-poller (Register/Poll/Complete)
  → ToolEnv 本地沙箱 + shim →（可选）Worker 官方 harness 二次判分
  → CompleteAgentJob → EpisodeResult
```

实机样例（编排）：`code-agent-numpy_1-8e1c1c1c`，`status=completed`，约 **3.6s**（缺 `test_script` 时 `reward=0`）。  
实机样例（完整判分，2026-07-25 续）：`code-agent-numpy_1-f4f55b2b`，mock + 完整 `test_script` → **`reward=1.0`，`tests_passed=20/20`**（adapter-core 已含 `test_script` 透传并重新部署）。

---

## 4. 其他模块待调整（非 Hub）

Hub 侧待办见专文 [Hub待调整事宜](./Hub待调整事宜-qa制品与Rubric注册.md)。本节只列 **Server / Bridge / Worker / Agent 机 / OpenHands / 文档与 CI / 评测口径** 上仍未收口、但本轮联调已暴露的事项。

> **进度（2026-07-25 续 / 07-26 FE）**：§4.3–§4.5 已落地并验收；§4.1 / §4.6 / §4.7 与 Bridge 训练侧清查仍开放。  
> **前端**：Obs 部署 + seed 冒烟已做；**§4.2 Bridge 真实 UI 联调仍开放（P0）**；能力差距见 [前端观测面与系统能力差距-待补齐](./前端观测面与系统能力差距-待补齐.md)。

### 4.1 Server（控制面 / Adapter Core）

**现状**：B3 首轮二进制已部署；`CodeAgentBackend` 与 `submit_code_agent_episode` 已实机跑通。SWE 与 ToolEnv 共用 Agent 池机制，但观测与落盘粒度仍偏 SWE 视角。

| 待办 | 说明 | 建议优先级 |
|------|------|------------|
| 核对 `agent_job_pickup_timeout_secs` | 默认约 30s。ToolEnv poller 若短暂重启或网络抖动，可能误判「无人领取」而超时；生产应按 poller 心跳间隔与 `MAX_CONCURRENT` 再标定，必要时对 `toolenv-*` 池单独配置 | P1 |
| CodeAgent 轨迹 / 元数据落盘对齐 | SWE 路径有 gateway session、worker 绑定；Code 路径无 gateway，当前复用 `ResultPersistenceContext::swe_agent` 一类落盘。需确认 trajectory / admin 查询对 `agent-pool:toolenv-default` 可读、可区分，避免运维误当成 SWE 失败 | P1 |
| Admin 指标分池 | `/agents` 已能列出 pool，但缺少按 bridge（`uenv-agent-toolenv` vs `uenv-agent-openhands`）聚合的 pending / in-flight / 完成率 / 平均 reward。百卡联调时同机双 Agent 更依赖分池看板 | P2 |
| Complete 语义与二次判分 | 当前 reward 由 poller Complete 回填；poller 内部再调 Worker harness 是「Agent 侧自建」而非 Server 编排。文档与错误码需写清：Server 不保证 Worker 二次判分一定发生 | P2 |

**验收建议**：带完整 `test_script` 的 mock/LLM 各一题，admin 能按 pool 看到 job 生命周期；超时与 abandon 行为可复现且有日志关键字（已有 `code_agent_*`）。  
**备注**：mock + `test_script` 非零 reward 已在 §3 验证；LLM 题与落盘对齐仍待做。

### 4.2 Bridge（训练入口 / `uenv-bridge` + `core.rs`）

**现状**：`default_env_type` 已改为 `qa`；`core.rs` 含 `test_script` / `execution_mode` 透传；**adapter-core 已于 2026-07-25 续部署到 Server**（后含 Obs 的滚动见 2026-07-25 晚），B3 mock 非零 reward 已通。  
**前端（2026-07-25/26）**：Server 上 Vite 前端 `http://8.130.75.157:8888`、Obs `127.0.0.1:50053`、同源 `/obs` 代理已冒烟（seed `_orphan` + state/stream）。**这不等于 FE-2 真实链路联调**——UI 仍未由 Bridge `SubmitEpisode` / VeRL AgentLoop 真实事件驱动。

| 待办 | 说明 | 状态 |
|------|------|------|
| 再发布一次 adapter-core（含 `test_script`） | 7142 重建 → Server 滚动；样例 `numpy_1` → `reward=1.0` | ✅ 已完成 |
| 训练侧全量确认走 `qa` | 除 `verl_agent_loop` 默认值外，检查历史 yaml / 作业脚本 / 数据集 loader 是否仍写死 `env_type=math` | ⬜ 仍开放（P0） |
| **基于当前前端做真实链路联调（Bridge 负责）** | 用现有 UI（`?run=<training_run_id>`）验收 Obs 事件来自真实训练/评测，而非 seed/fixture。见下方验收清单与 [差距专文](./前端观测面与系统能力差距-待补齐.md) | ⬜ **P0（Bridge）** |
| Sample → payload 契约文档 | 必填：`execution_mode`、`task_id`、`agent_pool_id`、`agent_bridge_*`、`ground_truth_code` / `test_script` | ⬜ P1 |
| 轻量 Agent 机依赖面 | bootstrap 已固化 agent stubs 合并进 `uenv/v1/`（见 §4.4） | ✅ 部分完成 |

#### Bridge 前端真实联调验收清单（待办细则）

> 责任人：**Bridge**。前置：Server Obs 已启、前端 `:8888` 可访问（已具备）。差距与补齐项见专文。

| # | 步骤 | 期望 |
|---|------|------|
| 1 | 7142 / 本地 Bridge 配置 `UENV_OBS_URL=http://8.130.75.157:50053`（若公网未放行 50053，则经 SSH 隧道或 Server 本机发起） | `obs_client` 非 no-op |
| 2 | 选定并固定 `training_run_id`（与 `UENV_TRAINING_RUN_ID` / VeRL batch metadata 一致）；打开 `http://8.130.75.157:8888/?run=<id>` | 顶栏显示该 id，**非** `_orphan` / Fixture |
| 3 | 跑最小真实流量：至少 1 条 `qa` native Episode，或 1 条已验证的 B3 `code` agent Episode | Server 日志有 submit/dispatch/complete；Obs 有对应 event |
| 4 | UI 工作流节点随链路变色（SUBMIT→…→DONE/FAILED）；树出现 worker / episode | FE-2.1 / FE-2.2 |
| 5 | Bridge 发 `RUN_STARTED` / `RUN_CLOSED` 时顶栏 `run_state` 与 Obs state 一致 | FE-2.3 |
| 6 | 断线重连后 SSE 仍能看到终态（可接受重推 `full_state`） | FE-1.3 实机确认 |

**明确未纳入本待办（记入差距专文）**：前端「开始/终止训练」按钮、日志/Metrics Tab、历史回放——系统侧亦无对应 Obs 控制 API，属 P1 补齐，不阻塞本轮「观测面真实联调」。

### 4.3 Worker / Plugin — ✅ 已完成

**现状（完成后）**：仓库与 7143 实机配置均已 `types=["qa","code","swe"]`（无 `math`）；金标契约与过严决策已成文。

| 待办 | 落地 | 状态 |
|------|------|------|
| 配置层彻底退役 `math` | `config/uenv-worker.deploy-7143-swe-pro.yaml` 去掉 `math`（与 standby / 7143 / 默认 yaml 一致）；实机 7143 配置已对齐并同步仓库文件 | ✅ |
| 插件二进制与金标版本绑定 | 新增 [`plugins/qa/RUBRIC.md`](../../../plugins/qa/RUBRIC.md)；`manifest.yaml` 增加 `runtime_plugin` / `compat_aliases` / `rubric_doc`；Hub 侧仍需吸收 digest（见 Hub 专文） | ✅（插件侧）；Hub 吸收 ⬜ |
| `math` 制品保留策略 | RUBRIC + `uenv-worker/README.md`：二进制可留回滚，**禁止 register**；误发 → Server `no worker supports env type` | ✅ |
| 金标过严 2 条产品决策 | **保持过严**：① 无 `####`/`\boxed{}` 的自然语言答案；② 长左侧赋值。禁止 silent 放宽；变更须走对齐脚本 + 新 version（见 RUBRIC.md） | ✅ |

**验收**：7143 `types` 无 math；`plugins/qa/RUBRIC.md` 可查。

### 4.4 Agent 机 208.77（ToolEnv）— ✅ 已完成（LLM 端点待填）

**现状（完成后）**：bootstrap 固化 stubs；沙箱 requirements md5；同机隔离巡检；mock 非零 reward 已通。实机 **暂留 `POLICY=mock`**（7142 临时 vLLM 已释放，无稳定推理端；模板默认已是 `POLICY=llm` + 占位 URL）。

| 待办 | 落地 | 状态 |
|------|------|------|
| 生产切回 `POLICY=llm` | `config/uenv-toolenv.env.example` 默认 `POLICY=llm`，`LLM_ENDPOINT` 改为占位符（去掉已死的 `:18099`）。实机 `/etc/uenv-toolenv.env` 注明 mock 直至填入稳定端点；**填好后改 `POLICY=llm` 并 `systemctl restart uenv-toolenv-poller` 即可** | ✅ 模板；实机待填端点 |
| stubs 合并写入 bootstrap | `bootstrap_toolenv_agent.sh` §3b：protoc → 合并进 `app/uenv-bridge/src/uenv/v1/`；unit `PYTHONPATH` 不含 `…/gen` | ✅ |
| poller 二次判分补齐 | 完整 `test_script` 样例 `code-agent-numpy_1-f4f55b2b`：**reward=1.0，20/20** | ✅ |
| 与 OpenHands 资源隔离巡检 | bootstrap §5：检查 `:8099` vs `:8888/:8777`；pool/bridge 对照打印 | ✅ |
| 沙箱 lock 指纹 | bootstrap 写 `/opt/uenv-toolenv/runs/sandbox-requirements.md5`（sandbox / heavy） | ✅ |

**验收**：`uenv-toolenv-poller` active；Server `/agents` 可见 `toolenv-default`；mock 非零 reward 已验证。

### 4.5 OpenHands 集成 — ✅ 已完成

| 待办 | 落地 | 状态 |
|------|------|------|
| 统一 regeneratestubs | `Makefile` `proto-agent-python` 改为生成 `gen/uenv/v1/`（含 common/episode/agent）；本地与 208.77 均已含 `task_payload_json` | ✅ |
| 文档标明字段用途 | `uenv_runtime/agent_job.py` 模块文档 + [`integrations/openhands/README.md`](../../../integrations/openhands/README.md)「Agent 池控制面」表（SWE vs ToolEnv） | ✅ |
| 同机双 bridge 版本策略 | README / env.example / agent_job 文档：两侧 `agent_bridge_version` **独立演进** | ✅ |

### 4.6 文档 / CI

**现状**：本综合报告与 Hub 专文已落盘；跨模块清单与实施规划中的「进行中」状态位可能仍停留在本轮开始前。

| 待办 | 说明 | 建议优先级 |
|------|------|------------|
| 回写状态位 | 更新 [跨模块调整清单](./跨模块调整清单-qa改造与ToolEnv-Agent.md)、[实施规划](./验证型环境改造与DSCode-Agent评测-实施规划.md)：B2/208.77/math/A/B3/GPU 标完成，并链到本文 | P1 |
| 单测覆盖 | `execution_backend` 已加 code agent 选择用例；建议再补：`core.rs` 对 `test_script`/`execution_mode` 的透传单测、poller 载荷解析的轻量单测（可 mock gRPC） | P2 |
| Proto 生成进 CI | `make proto-agent-python`（或等价）失败则阻断；防止 stub 与 `.proto` 漂移 | P2 |

### 4.7 评测口径（B2 产品化后续）

**现状**：Agent 轨指标为 `agentic_pass@1`，官方轨为 DSCodeBench 原口径；文档已分轨，但仍易被对外材料混写。

| 待办 | 说明 | 建议优先级 |
|------|------|------------|
| 报告强制分轨并列 | `report_dscode_agentic.py` 输出中明确「不可与官方轨直接对比」；若并列官方数，需标注数据来源与样本集合是否相同 | P1 |
| 样本集合冻结 | 产品化后应用固定 seed / library 过滤 / limit 写入 `metrics.json`，保证 resume 与复现可比 | P1 |
| 与 B3 编排口径关系 | B2 是「Agent 机自扫数据集」；B3 是「Server 下派 AgentJob」。两套入口的 pass@1 不应混池平均，除非显式声明同一题集与同一 policy | P1 |

---

## 4.8 待调整一览（优先级速查）

| 优先级 | 模块 | 一句话 | 状态 |
|--------|------|--------|------|
| P0 | Bridge | 带 `test_script` 的 adapter-core 再发布到 Server | ✅ |
| P0 | Bridge | 训练/作业侧清掉残留 `env_type=math` | ⬜ |
| **P0** | **Bridge** | **基于当前前端 UI 做真实链路观测联调（非 seed/fixture）** | **⬜** |
| P0 | 208.77 | bootstrap 固化 stubs + mock 非零 reward | ✅ |
| P0 | 208.77 | 填入稳定 LLM 端点后切 `POLICY=llm` | ⬜（模板已就绪） |
| P1 | Server / FE | 观测与控制能力补齐（日志/Metrics/start-stop 等，见差距专文） | ⬜ |
| P1 | Server | pickup 超时与 CodeAgent 落盘/观测对齐 | ⬜ |
| P1 | Worker / Plugin | 全模板去掉 math；Rubric 绑定；过严决策 | ✅ |
| P1 | OpenHands | regeneratestubs + 字段/双 bridge 文档 | ✅ |
| P1 | 文档/评测 | 状态位回写；Agent/官方轨强制分轨 | ⬜ |
| P2 | 各模块 | admin 分池指标、CI proto 门禁 | ⬜ |

---

## 5. 回滚要点

| 组件 | 回滚 |
|------|------|
| Server 二进制 | `/usr/local/bin/uenv-adapter-core.bak-b3-*` / `bak-b3b-*` / `bak-obs-*` |
| Worker types | 临时加回 `math`（不推荐） |
| olympmath 判分 | **不要**回退子串修复（属正确性修复） |
| ToolEnv poller | `systemctl disable --now uenv-toolenv-poller` |
| Obs DB | 重放阻塞时可轮转 `/home/uenv/obs-data/obs.db`（曾有 `obs.db.bak-heavy-*`） |

---

## 6. 建议下一步（本轮范围外 / 仍开放）

1. ~~带 `test_script` 的 B3 mock 再跑一题，确认 `reward=1.0`。~~ **已完成**（`numpy_1` → 1.0 / 20/20）。  
2. Hub 侧按专文更新制品与注册契约（吸收 `plugins/qa/RUBRIC.md`）。  
3. 为 208.77 配置稳定推理端后切 `POLICY=llm`，跑一小样本 Agent 评测。  
4. Bridge 训练侧清查残留 `math`；Server 落盘/分池指标；文档状态位回写。  
5. **Bridge：按 §4.2 清单用 `8.130.75.157:8888` 做真实 Episode → Obs → UI 联调**（见 [差距专文](./前端观测面与系统能力差距-待补齐.md)）。  
6. 主线合入：proto + server + core + poller + qa 收敛 + 金标修复 + OpenHands stubs。
