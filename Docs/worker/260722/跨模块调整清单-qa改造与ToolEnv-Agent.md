# 跨模块调整清单 — `math`→`qa` 改造 & DSCode ToolEnv Agent

> 日期：2026-07-25
> 状态：**已实机联调打通（qa 全链路 + ToolEnv on 208.77 + 真实 vLLM 小样本）**；余项见 §联调结果
> 前置规划：[验证型环境改造与DSCode-Agent评测-实施规划](./验证型环境改造与DSCode-Agent评测-实施规划.md)
> 选型结论：[Math环境与术语规范-可证明性与DSCode-Agent选型](./Math环境与术语规范-可证明性与DSCode-Agent选型.md)
> 实机参考：[secrets/README.md](../../../secrets/README.md)（7142 / 7143 / Server `8.130.75.157` / Hub `8.130.95.176` / Agent `8.130.208.77`）

---

## 联调结果（2026-07-25 实机）

### A. `math` → `qa` 单轮验证环境（全链路打通 ✅）

| 项 | 结果 |
|----|------|
| 落地方式 | **加法式、零 Rust 重编**：新增 `plugins/qa/{manifest.yaml,run.sh}`，`run.sh` 复用既有 `uenv-math-plugin`（判分按 `dataset` 路由，与 env_type 无关）；`math` 原样保留、可回滚 |
| Worker（7143） | `config/uenv-worker.deploy-7143-swe-pro.yaml` 的 `env.types` 增 `qa` → `["qa","math","code","swe"]`；重启后 `loaded_envs=code,math,qa`、`hub_manifest_pulled env_type=qa`、register 成功 |
| Hub（`8.130.95.176`） | 新脚本 `uenv-bridge/scripts/hub_publish_qa_env.py` 镜像 `math` 发布 `qa@0.2.0`（create_env + publish_version）；`GET /api/v1/envs/qa/versions/latest` = 200 |
| Bridge | `verl_agent_loop.py` `_env_type()` 将 gsm8k/pubmedqa/scitab/olymmath 归一到 `qa`；`reward_type` 分支含 `qa` |
| adapter-core | **无需改动**（仅 `swe`/`code` 有特殊分支，`qa` 走通用 passthrough） |
| Smoke | `uenv-bridge/scripts/smoke_qa_datasets_grpcurl.py`（envType=`qa`）四数据集 gsm8k/pubmedqa/scitab/olymmath-easy 全部 `status=completed, reward=1.0` |
| 免-LLM 说明 | smoke 走 Worker `model_client.rs` 的 rule_reward 短路（payload 无 question 时返回 target 作 action → 插件判分）；验证链路，非真实判分 |

> ⚠️ 启动坑（已解决）：Worker `serve` 对 `env.types` 的 Hub 强拉在 404 时**会硬失败**（不同于 swe 的 warn 降级），因此必须**先 Hub 发布 `qa`** 再把 `qa` 加入 `env.types` 重启。

### B. DSCode ToolEnv Agent（run_python + submit_code，部署于 208.77 ✅）

| 项 | 结果 |
|----|------|
| 脚本 | `uenv-bridge/scripts/benchmark/dscode_toolenv_agent.py`：run_python（本地沙箱迭代）+ submit_code（定稿）+ agentic_pass@1 |
| 官方判分接法 | **零控制面改动**：Worker 会向 `model_endpoint.url` 拉候选代码再用 inline_harness 判分 → 在 208.77 起 OpenAI 兼容 shim 返回 Agent 定稿代码，把 `model_endpoint` 指向 shim → Worker 用**同一官方 harness** 判 Agent 的代码 |
| 部署位置 | **208.77（Agent 机）**：agent 循环 + shim(`0.0.0.0:8099`，公网 `http://8.130.208.77:8099/v1`) 同机；worker(7143) 出站可达该 shim（已验证） |
| 依赖对齐 | 208.77 原 repo bridge/proto 版本与主线不一致；已将 **7143 可用的** `gen/adapter_core_pb2[_grpc].py`+`clients.py`+`protocol.py` 覆盖到 208.77，配 `PROTOCOL_BUFFERS_PYTHON_IMPLEMENTATION=python` 运行（原文件已 `.bak` 备份） |
| Smoke（mock） | 208.77 起 agent，`numpy_0`：2 轮 → Worker inline_harness `tests_passed=20/20`、`reward=1.0`（MockGroundTruthPolicy 确定性打通） |

### C. 真实 vLLM 小样本联调（2026-07-25 ✅）

| 项 | 结果 |
|----|------|
| vLLM | 在 **7142** 起单卡服务（DeepSeek-V3 TP8 未起，避免占满 8 卡）：`Qwen3-8B` → `:18088`（GPU4）、`Qwen3-14B` → `:18099`（GPU5）；`hb_eval_env` vLLM 0.22.1 |
| Agent 调用 | `--policy llm --llm-endpoint http://10.10.20.142:18099/v1 --llm-model Qwen3-14B`；`chat_template_kwargs.enable_thinking=false`；`run_python` 用 DSCodeBench venv |
| 难例（前 3 题 numpy_0/1/2） | 全链路 `completed`；14B：`numpy_0` 已能生成顶层 `weighting_function`，但仍 wrong_answer；`numpy_2` **28/50**；`agentic_pass@1=0`（题难，符合预期） |
| **短题小样本（5 题）** | 选 GT 较短的 `numpy_3/10/11/17/25`：**4/5 全过（50/50）**，`agentic_pass@1=0.8`；仅 `numpy_25` 13/50 |
| 复现命令（7143） | 见下 |

```bash
# 7142：Qwen3-14B 已监听 :18099（GPU5）；8B 在 :18088（GPU4）
# 7143：
CODE_PY=/var/lib/uenv/envs/dscodebench/0.2.0/venv/bin/python
cd /root/UEnv
PROTOCOL_BUFFERS_PYTHON_IMPLEMENTATION=python PYTHONPATH=uenv-bridge/src \
  python3 uenv-bridge/scripts/benchmark/dscode_toolenv_agent.py \
  --endpoint 8.130.75.157:8088 \
  --data /tmp/dscode_easy5.json \
  --limit 5 --policy llm \
  --llm-endpoint http://10.10.20.142:18099/v1 --llm-model Qwen3-14B \
  --python-bin "$CODE_PY" --max-tokens 2048 --max-turns 4 \
  --shim-host 127.0.0.1 --shim-port 18899 --num-tests 50 \
  --output /tmp/toolenv-llm-easy/metrics.json
```

> 说明：官方网关 `219.147.100.43:18888` 的 backend（DeepSeek-V3 `:8000`）当前 `inactive`；小样本联调直接打单卡 Qwen vLLM，不经过该网关。用完可 `fuser -k 18088/tcp 18099/tcp` 释放 GPU。

### 余项 / 待决

1. ~~真实智能体运行~~ → **已用 7142 Qwen3-14B 完成小样本**（§C）；全量评测 / 更大模型另开。
2. **208.77 生产化**：建议干净 venv + 同步主线 bridge + systemd；真实 LLM 跑时可把 `--llm-endpoint` 指到 `http://219.147.100.43:18099/v1`。
3. **B3（Server 编排 AgentJob for code）**：仍待确认后再改共享控制面。
4. **`math` 兼容期收敛**：训练侧切 `qa` 后再下线 `math`。
5. **DeepSeek-V3 网关**：若要用正式 `uenv-llm-gateway:18888`，需 `systemctl start vllm-dsv3-awq`（TP8，占满 8×A100）。

---

## 0. 规划合理性确认（联调前）

| 项 | 判断 | 说明 |
|----|------|------|
| `math` → verifiers Rubric + 更名 `qa` | **合理** | 单轮可验证问答的真实语义；Hub 仍可走 env registry；Worker 热路径保留插件壳 |
| DSCode 官方单轮基线保留 | **合理** | 与官方 pass@1 可比；Agent 必须分轨 |
| ToolEnv 作 DSCode Agent | **合理** | 比 OpenHands 轻，贴合单函数 DS 题 |
| **ToolEnv 单独部署到 Agent 机（208.77）** | **合理且应升为联调目标路径** | 与现有 SWE OpenHands 池同机角色分离：`agent_pool_id` / bridge 包独立；终局 harness 仍在 7143 `code` env |
| 原规划「B 轻量仅 Bridge 脚本」 | **可作开发脚手架，不宜作实机终态** | 联调改造以 **208.77 Agent 池 + Server AgentJob** 为准；Bridge 脚本用于本地/小样本冒烟 |

**实机职责划分（冻结）：**

```text
7142 Adapter / 评测脚本
        │ EpisodeRequest(env_type=code|qa, execution_mode=…)
        ▼
8.130.75.157  Server / adapter-core
        │ 调度 Worker；execution_mode=agent 时发 AgentJob
        ├──────────────────────────────┐
        ▼                              ▼
219…:7143 Worker                    8.130.208.77 Agent 池
  qa 插件判分 / code harness          ToolEnv poller（run_python+submit）
  （SWE gateway 仍给 OpenHands）      与 OpenHands 同机、不同 bridge/pool
        ▲
        │ Hub sync 制品
8.130.95.176 Hub
  qa registry + rubric/wheels 包
  code/dscodebench 包（已有）
  uenv-agent-toolenv bridge 包（新增）
```

---

## 1. 勾选总览

| 模块 | `qa` 改造 | ToolEnv@208.77 | 优先级 |
|------|-----------|----------------|--------|
| **Hub** | ✅ 新增/调整 | ✅ 新增 bridge 包 | P0 |
| **Worker 7143** | ✅ | ⚠️ 终局 harness + 可选 REPL 下沉 | P0 |
| **Bridge / Adapter（7142 + Core）** | ✅ | ✅ Agent 字段透传 + 评测脚本 | P0 |
| **Server（75.157）** | ⚠️ 部署确认为主 | ✅ AgentJob / pool 识别 | P0 |
| **Agent 机 208.77** | — | ✅ **ToolEnv 主部署点** | P0 |
| **proto** | ❌ 通常不改 | ⚠️ 仅当 AgentJob 缺字段时扩展 JSON | P2 |
| **fixtures / smoke / CI** | ✅ | ✅ | P1 |
| **Docs / PROTOCOL / secrets** | ✅ | ✅ | P1 |
| **VeRL / 训练 Dataset** | ✅ 路由键 | ⚠️ 可选 agent 样本 | P1 |

图例：✅ 必改 · ⚠️ 条件改 / 联调确认 · ❌ 不改

---

## 2. Hub（`8.130.95.176`）— 注册与制品

### 2.1 `qa` 环境（原 math）

| ID | 动作 | 细节 | 状态 |
|----|------|------|------|
| H-QA-1 | **新增 env registry：`qa@0.3.0`（或下一可用版本）** | `uenv env publish`；manifest：`env_type=qa`，datasets=`gsm8k/pubmedqa/scitab/olymmath*`，`interface`/`config_schema` 对齐 | ☐ |
| H-QA-2 | **seed.rs / 启动种子** | 新 Hub 实例默认含 `qa`；幂等 create+publish | ☐ |
| H-QA-3 | **兼容 `math`** | 过渡期保留 `math@0.2.0`；changelog 标注 deprecated → 指向 `qa`；窗口后 yank | ☐ |
| H-QA-4 | **smoke fixtures 包** | `math-smoke-fixtures` → `qa-smoke-fixtures@0.x`（或双发）；样本 `env_type=qa` | ☐ |
| H-QA-5 | **Rubric / verifiers 依赖制品（推荐 EnvPackage）** | 例如 `qa-rubric-align@0.1.0`：`verifiers`/`math_verify` wheels + `rubrics/*.py`；Worker CI/对齐机 `sync` 后离线可用 | ☐ |
| H-QA-6 | **examples[] / Hub 文档** | manifest examples 与 `fixtures/qa` 一致；运维手册补 `qa` sync 示例 | ☐ |

**验收：** `GET /api/v1/envs/qa/versions/latest` 返回 200；`/envs/math/...` 过渡期仍可读。

### 2.2 ToolEnv Agent bridge（DSCode）

| ID | 动作 | 细节 | 状态 |
|----|------|------|------|
| H-TE-1 | **新增 Agent bridge 包：`uenv-agent-toolenv@1.0.0`** | 对标现有 `uenv-agent-openhands@1.0.0`；含 ToolEnv runner、`run_python`/`submit_code` driver、poll 循环、依赖 pin | ☐ |
| H-TE-2 | **seed 写入 packages** | Hub 启动种子或手动 `publish`；与 openhands bridge **并存、不互相覆盖** | ☐ |
| H-TE-3 | **208.77 消费路径** | `uenv agent-bridge sync uenv-agent-toolenv --version 1.0.0` → `/opt/uenv/agent-bridges/uenv-agent-toolenv/`（路径与 openhands 对称） | ☐ |
| H-TE-4 | **（可选）ToolEnv REPL 依赖包** | 若 REPL 在 208.77 本地执行：预缓存与 DSCode 对齐的精简 venv/wheels（**禁止**在 Agent 机临时 pip 公网装包作生产路径） | ☐ |
| H-TE-5 | **catalog / overlay 元数据** | bridge_id、version、支持的 `agent_kind=toolenv`、所需 Worker 能力（`code` harness）写进包 manifest | ☐ |

**验收：** Hub `GET /api/v1/packages/uenv-agent-toolenv/versions/latest` 可用；208.77 sync 后目录非空。

### 2.3 已有、通常无需重发

| 资源 | 说明 |
|------|------|
| `code@0.2.0` / `dscodebench@0.2.0` | 终局 harness 继续用；除非 ToolEnv 要求额外字段进 config_schema |
| `uenv-agent-openhands` | **保留**；与 ToolEnv 分 pool，禁止混用同一 `agent_pool_id` 跑 DSCode |

---

## 3. Worker（7143）

### 3.1 `qa` 改造

| ID | 动作 | 细节 | 状态 |
|----|------|------|------|
| W-QA-1 | 插件目录 / 二进制 | `plugins/qa` + `uenv-qa-plugin`；过渡期 `math` alias 或双注册同实现 | ☐ |
| W-QA-2 | 配置 | `env.types` 含 `qa`（如 `["qa","code","swe"]`）；`UENV_QA_PLUGIN_BIN`（兼容读 `UENV_MATH_PLUGIN_BIN`） | ☐ |
| W-QA-3 | 预热池 | WarmupPool prewarm `qa` | ☐ |
| W-QA-4 | 判分对齐 | Rust backends 与 Hub/仓库 Rubric golden 一致 | ☐ |
| W-QA-5 | Hub sync | `uenv env sync` 消费 `qa` manifest（及可选 rubric 包） | ☐ |
| W-QA-6 | 部署文件 | `config/uenv-worker.deploy-7143*.yaml`、`/root/.uenv-worker.env` | ☐ |

### 3.2 ToolEnv 相关（Worker 侧）

| ID | 动作 | 细节 | 状态 |
|----|------|------|------|
| W-TE-1 | **终局 harness 不变** | `env_type=code` + dscodebench；支持 `response_text` / skip-infer 注入最终代码 | ☐ |
| W-TE-2 | **Register 能力声明** | 确保 Server 能把 code episode 派到本机；AgentJob 场景下 gateway/session 若复用需文档化 | ☐ |
| W-TE-3 | **（可选）受限 `exec_python`** | 仅当决定「REPL 下沉 Worker、208.77 只做 LLM 循环」时开放 Runtime Gateway；默认 **REPL 在 208.77** 则本项不做 | ☐ |
| W-TE-4 | 依赖一致性 | 记录 DSCode venv 版本指纹，供 208.77 REPL 对齐校验 | ☐ |

---

## 4. Bridge / Adapter Core（7142 + `8.130.75.157`）

### 4.1 `qa`

| ID | 动作 | 细节 | 状态 |
|----|------|------|------|
| B-QA-1 | `_env_type()` | gsm8k/pubmedqa/scitab/olymmath/**math** → **`qa`** | ☐ |
| B-QA-2 | `default_env_type` | 改为 `qa` | ☐ |
| B-QA-3 | reward 分支 | `reward_type: rubric` 条件改为 `env_type == "qa"`（兼容旧 `math`） | ☐ |
| B-QA-4 | Adapter Core | 扫除 `env_type=="math"` 硬编码；部署新 `uenv-adapter-core` 到 75.157 | ☐ |
| B-QA-5 | 评测脚本 | `evaluate_*_uenv.py` / smoke grpcurl 默认 `env_type=qa` | ☐ |

### 4.2 ToolEnv

| ID | 动作 | 细节 | 状态 |
|----|------|------|------|
| B-TE-1 | Episode 字段 | `execution_mode=agent`、`agent_bridge_id=uenv-agent-toolenv`、`agent_pool_id`（如 `toolenv-dscode`）、与 SWE 字段对称透传 | ☐ |
| B-TE-2 | 评测入口 | `evaluate_dscodebench_agent_uenv.py`：组 Agent Episode 或先本地 ToolEnv 冒烟再切 208.77 | ☐ |
| B-TE-3 | 指标分轨 | 输出 `agentic_pass@1` 等，目录与单轮基线隔离 | ☐ |
| B-TE-4 | `dscodebench`→`code` 路由 | 确保 B-2（既有 P0）已落地，避免落回 `qa`/`math` | ☐ |

---

## 5. Server（`8.130.75.157`）

| ID | 动作 | 细节 | 状态 |
|----|------|------|------|
| S-QA-1 | 源码 | **通常无需改**；确认调度按 Worker `supported_env_types` 匹配 `qa` | ☐ |
| S-QA-2 | 运维 | Worker 重新 Register 后 admin/日志可见 `qa` | ☐ |
| S-TE-1 | AgentControl | 支持第二套 Agent 注册：`agent_pool_id=toolenv-dscode`，bridge=`uenv-agent-toolenv` | ☐ |
| S-TE-2 | AgentJob 派发 | `execution_mode=agent` + code/dscodebench 时路由到 ToolEnv 池，**不得**派到 OpenHands 池 | ☐ |
| S-TE-3 | 运维探活 | `GET http://127.0.0.1:50052/agents` 同时可见 openhands 与 toolenv | ☐ |
| S-TE-4 | （若缺字段） | AgentJob payload 增加 `task_id`/`library`/`response` 回写约定；优先走 JSON 扩展，慎改 proto | ☐ |

---

## 6. Agent 机（`8.130.208.77`）— ToolEnv 主部署

> **硬约束：** Verifiers ToolEnv（`run_python` + `submit_code`）**单独部署在本机**，与 OpenHands 共存但进程/环境/pool 隔离。

| ID | 动作 | 细节 | 状态 |
|----|------|------|------|
| A-TE-1 | 目录与 venv | `/opt/uenv/agent-bridges/uenv-agent-toolenv` + 独立 Python venv（含 `verifiers`） | ☐ |
| A-TE-2 | Hub sync | `uenv agent-bridge sync uenv-agent-toolenv` | ☐ |
| A-TE-3 | systemd | 新建 `uenv-agent-toolenv-poller.service`（对标 `uenv-agent-poller`）；**不要**塞进 openhands-runner | ☐ |
| A-TE-4 | 环境变量 | `UENV_SERVER_ENDPOINT=8.130.75.157:8088`；`AGENT_POOL_ID=toolenv-dscode`；`AGENT_BRIDGE_ID=uenv-agent-toolenv`；`AGENT_BRIDGE_VERSION=…` | ☐ |
| A-TE-5 | LLM | 指向与评测一致的 Model Gateway / vLLM（可经 7142 或现有中继）；与 OpenHands LLM 配置分离文件（如 `/root/.toolenv-20877.env`） | ☐ |
| A-TE-6 | `run_python` | 本地受限 REPL：超时、无外网 pip、不挂载官方 200-case test_script；依赖与 7143 DSCode venv 指纹对齐 | ☐ |
| A-TE-7 | `submit_code` | 结束多轮后把最终代码交回 Server/Worker：经 AgentJob Complete + Worker `code` harness；或约定回写字段由 Server 转 Episode Step | ☐ |
| A-TE-8 | 资源隔离 | 与 OpenHands 限流/并发分开；208.77 仅 8C32G，ToolEnv 并发建议 ≤2 先 smoke | ☐ |
| A-TE-9 | 探活 | 独立 health 端口或 systemd status；写入 secrets/运维文档 | ☐ |

**启动顺序（DSCode + ToolEnv）：**

```text
① Server 75.157 就绪
② Worker 7143 Register（含 code，且 DSCode venv 可用）
③ 208.77 ToolEnv poller RegisterAgent
④ Adapter/评测脚本 SubmitEpisode(code, execution_mode=agent, agent_bridge=toolenv, …)
```

---

## 7. proto / fixtures / CI / 文档 / 训练侧

| ID | 模块 | 动作 | 状态 |
|----|------|------|------|
| P-1 | proto | 默认不改；AgentJob 扩展优先 JSON | ☐ |
| F-1 | fixtures | `fixtures/qa/`（可由 math 迁移）；smoke JSON `env_type=qa` | ☐ |
| F-2 | smoke 脚本 | `smoke_math_datasets_*` → `smoke_qa_datasets_*`（或参数化） | ☐ |
| C-1 | CI | `uenv-qa-env` 单测 + Rubric golden pytest | ☐ |
| C-2 | CI | ToolEnv 单元（mock LLM）可选 | ☐ |
| D-1 | Docs | 五类矩阵、PROTOCOL、评测 doc、`secrets/README` 增 ToolEnv@208.77 节 | ☐ |
| D-2 | Docs | 本清单与实施规划交叉引用；联调记录另存 | ☐ |
| T-1 | VeRL Dataset | `extra_info.dataset` + 路由到 `qa`；agent 样本另字段 `execution_mode` | ☐ |

---

## 8. 联调改造建议顺序（结合实机）

| 步 | 内容 | 主机 |
|----|------|------|
| 1 | Hub 发布 `qa` +（可选）rubric 包；seed 更新 | Hub |
| 2 | 本地/7143 落地 `plugins/qa`，双注册兼容 `math` | 开发机 → 7143 |
| 3 | Bridge/Core 路由改 `qa`，部署 adapter-core | 7142 → 75.157 |
| 4 | 7143 smoke：四 dataset `env_type=qa` | 7143 + 75.157 |
| 5 | Hub 发布 `uenv-agent-toolenv`；208.77 sync + poller | Hub → 208.77 |
| 6 | Server 确认双 Agent 池；DSCode Agent smoke（1 库） | 75.157 + 208.77 + 7143 |
| 7 | 文档与清单勾选；轨道 A/B 指标说明入库 | Docs |

---

## 9. 明确不做 / 易错点

| 项 | 说明 |
|----|------|
| 用 OpenHands 跑 DSCode | ❌ |
| ToolEnv 装进 7143 当「主 Agent 进程」 | ❌（Agent 在 208.77；Worker 只做环境与终局判分） |
| ToolEnv 与 OpenHands 共用同一 `agent_pool_id` | ❌ |
| Agent 轨结果写入官方 `pass@1` 主表 | ❌ |
| 未发 Hub bridge 包就在 208.77 手搓不可复现目录 | ⚠️ 仅允许短时开发，联调验收必须以 Hub sync 为准 |
| 过早 yank `math` | ⚠️ 至少一个评测周期兼容 |

---

## 10. 与实施规划的关系

| 文档 | 职责 |
|------|------|
| [实施规划](./验证型环境改造与DSCode-Agent评测-实施规划.md) | 阶段、里程碑、技术选型、验收口径 |
| **本文（跨模块清单）** | **按模块勾选：Hub/Worker/Bridge/Server/208.77/文档** 联调改造必改项 |
| [选型文档](./Math环境与术语规范-可证明性与DSCode-Agent选型.md) | 为何选 verifiers / ToolEnv |

联调开始后：每完成一项将 ☐ 改为 ✅，并在修订记录追加日期与执行人。

### 修订记录

| 日期 | 说明 |
|------|------|
| 2026-07-25 | 初版：确认规划合理性；ToolEnv 主部署钉在 208.77；拆出 Hub/各模块注册与调整清单 |
