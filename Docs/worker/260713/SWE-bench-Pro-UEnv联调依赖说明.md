# SWE-bench-Pro UEnv 联调依赖说明

## 1. Adapter 侧边界

Adapter 侧负责把 SWE-bench-Pro 样本构造成 UEnv `EpisodeRequest`，通过 Rust adapter core 提交给 Server，并记录 `EpisodeResult`、请求日志和评测指标。对于 SWE-bench-Pro，当前推荐走 `env_type=swe`、`execution_mode=agent`、`mode=llm` 的 UEnv Agent 路线，由 Server/Worker/Agent 侧完成环境创建、OpenHands 执行、模型调用和结果回填。

Adapter 不应直接修改 Server、Worker、OpenHands Agent 的实现逻辑；如果联调发现非 adapter 问题，应先记录依赖项并交由对应模块处理。

## 2. 当前全量测试的非 Adapter 前置依赖

### 2.1 Worker 需要覆盖全量 SWE-bench-Pro 环境 catalog

全量 SWE-bench-Pro 测试集包含多条 instance。Adapter 可以逐条提交请求，但 Worker 侧必须能识别这些 `instance_id`，并能找到对应环境包、镜像、仓库状态和测试入口。

如果 Worker catalog 只预置少量样例，则全量运行会在 Server/Worker 侧返回类似 `instance_id not in catalog` 的错误。这不是 adapter payload 格式本身能解决的问题。

期望 Worker/Server 侧提供：

- 全量 SWE-bench-Pro instance catalog。
- 每个 instance 对应的环境包版本、镜像或按需拉取策略。
- catalog 覆盖率与缺失 instance 的可观测日志。

### 2.2 Agent 完成回调需要与 Server 协议一致

Server 对 Agent job completion 有身份校验时，OpenHands Agent 侧需要使用与 Server 一致的 proto/stub，并在 `CompleteAgentJob` 时回填注册后的 `agent_id`。否则可能出现 Agent 已经跑完任务，但 Server 不 ack，导致 job 无法被正确释放或计入结果。

期望 Agent/Server 侧对齐：

- `AgentJobCompleteRequest` 的字段定义。
- Agent 注册后使用 Server 返回的 `agent_id`。
- completion ack 失败时输出明确日志，包括 expected/report agent id。

### 2.3 模型服务需要支持 thinking 与较大输出长度

SWE-bench-Pro 的修复任务通常需要较长上下文和多轮工具调用。为了按“thinking 开启、max tokens 较大或参考官方值”的口径测试，Agent 使用的 LLM 配置需要明确：

- thinking 未被关闭。
- `max_output_tokens` / `max_tokens` 设置为较大值，例如 32768。
- 模型 endpoint 稳定可用，不应长期返回 backend starting / unavailable。
- OpenHands 的总运行超时、单次 LLM 请求超时与 max tokens 匹配。

### 2.4 并发容量和超时策略

全量 SWE-bench-Pro 运行耗时较长，Worker 与 Agent pool 需要明确容量限制。Adapter 可以设置 batch size 和 client timeout，但实际并发上限由 Server/Worker/Agent pool 决定。

期望 Server/Worker/Agent 侧提供：

- 当前 agent pool 容量。
- Worker 可并发 episode 数。
- 每个 instance 的运行超时和失败重试策略。

## 3. Adapter 侧可以继续做的工作

在不修改非 adapter 代码的前提下，Adapter 侧可以继续完善：

- SWE-bench-Pro UEnv 提交脚本。
- 请求与结果 JSONL 记录。
- `resolved`、reward、status、错误分布等指标汇总。
- 单样例 smoke 与全量运行命令文档。
- 对非 adapter 依赖的错误分类和证据记录。

---

## 4. 核验结论（2026-07-13）

对照仓库代码 + Hub `8.130.95.176` + Worker 7143 实机状态，对 §2 逐条判定：

| 条目 | 是否属实 | 证据摘要 | 需要落实的改动 |
|------|----------|----------|----------------|
| **§2.1 全量 catalog** | ✅ **属实（阻塞全量）** | Hub/7143 `swe-bench-pro@0.2.0` catalog **仅 1 条**（qutebrowser）；`images.manifest` 无 tar；`pull_policy=local_only`；本机 Docker 仅约 3 个 `sweap-images` tag。全量会触发 Worker `instance_id … not in catalog`。 | **运维/Hub（H-5）**：导出全量（或计划子集）Pro catalog → 导入机 mirror 拉镜像 → `docker save` → `publish-image` → Worker `sync --docker-load`。非 Adapter 代码。 |
| **§2.2 CompleteAgentJob agent_id** | ✅ **属实（代码缺口）** | Server `agent_job.rs` 校验 `req.agent_id == inflight.agent_id`，不匹配返回 `AGENT_MISMATCH`。但 `integrations/openhands/uenv_runtime/agent_client.py::complete_agent_job` **未传 `agent_id`**；生成 stub `AgentJobCompleteRequest` **仅含字段 1–6**，缺 proto 字段 7（`agent_id`）。`openhands_runner.py` 调用处亦未传。 | **Agent 侧必改**：① 按 `proto/uenv/v1/agent.proto` 重新生成 stub；② `complete_agent_job(..., agent_id=...)`；③ runner 传入注册后的 `agent_id`；④ 208.77 部署更新。 |
| **§2.3 thinking / max tokens** | ✅ **属实（配置依赖）** | 属 OpenHands/LLM 网关与 runner 超时配置，不是 UEnv Worker 判分逻辑。 | **运维配置**：确认 endpoint、thinking、`max_tokens≈32768`、与 `OPENHANDS_RUN_TIMEOUT_SEC` 匹配。无强制仓库核心代码改动。 |
| **§2.4 并发与超时** | ✅ **属实（容量规划）** | Adapter 只能控 client batch；真实上限在 Server Agent 池、Worker episode 并发、Gateway/容器资源。 | **运维文档化**：写清 pool 容量、Worker 并发、单 instance 超时/重试；按需调大。 |

### 边界补充

1. **Native vs Agent**：§2.2/§2.3 仅影响 `execution_mode=agent`。baseline 若走 native（预填 patch/gold），可绕过 Agent 回调与 thinking，但仍受 §2.1 catalog/镜像约束。
2. **Adapter 路由**：提交脚本若**显式** `env_type=swe`，不依赖 VeRL `_env_type()`，则 Bridge B-3 不阻塞本路径；Rust core 对 swe 字段透传已有。
3. **占位 catalog**：仓库 fixture / 部分 `pro.json` 仍可能是 example-go 占位；EnvPackage 上的 qutebrowser 才是历史联调真实 instance——全量仍远不够。

### 建议落地顺序

1. **P0 代码**：修 OpenHands `CompleteAgentJob` 的 `agent_id` + 重生 stub（§2.2）。
2. **P0 制品**：计划跑的 Pro catalog + 镜像 tar 入库（§2.1 / H-5）。
3. **P1 配置**：LLM thinking / max tokens / 超时与并发（§2.3–§2.4）。
4. Adapter 侧继续做 §3 的提交脚本与指标汇总即可。

---

## 5. 已落实（2026-07-13）

| 项 | 状态 | 说明 |
|----|------|------|
| §2.2 `agent_id` | ✅ 已修并部署 | `agent_client.complete_agent_job(..., agent_id=)`；`openhands_runner` 传入注册 id；208.77 用 `/root/uenv-agent-venv` 重生 stub；`uenv-agent-poller` 已重启并 Register 成功 |
| §2.1 catalog 子集 | ✅ 部分落地 | Hub + 7143 EnvPackage catalog **1→3**（NodeBB / qutebrowser / ansible，对应当前本机已有镜像）；**仍无 `image_tar`**，`local_only` 依赖本机 docker images；全量 H-5 仍待做 |
| §2.3 / §2.4 配置 | ✅ 已写入 env | 208.77：`OPENHANDS_RUN_TIMEOUT_SEC=7200`、`OPENHANDS_MAX_OUTPUT_TOKENS=32768`、`OPENHANDS_AGENT_MAX_CONCURRENT=1`；`config/openhands-20877.env.example` 已补充注释 |
| 7143 Worker | ✅ 已恢复 | 重启时补齐缺失的 `UENV_CODE_PLUGIN_BIN`；日志确认 `swe_catalog_loaded_from_env_package count=3`、Gateway `:28097` |
| 部署脚本 | ✅ 已加固 | `scripts/deploy-openhands-20877.sh`：可选文件打包、远端 venv 生成 stub、`ExecStart` 指向 venv python |

**仍未完成**：全量 SWE-Pro catalog + 镜像 tar 入库（H-5）；thinking 是否真正开启取决于所用 LLM 网关配置（需对照 `openhands-llm-20877.json`）。

---

## 6. H-5 执行进展（2026-07-13）

| 项 | 状态 |
|----|------|
| 全量 catalog | ✅ Hub + 7143 `swe-bench-pro@0.3.4` 含 **731** instances；Worker `UENV_SWE_ENV_PACKAGE` 已指向 `0.3.4` |
| Hub `image_tar` | ✅ 仅 **3** 个（NodeBB / ansible / qutebrowser，约 1.9GB）；**全量 Hub 入库已停**（盘不够） |
| Hub 磁盘 | 根盘 **~100G**；全量 ≈ **1.5TB**，扩到 **≥2TB** 后再走 `import-swe-pro-h5.sh` / `publish-image` |

### 6.1 过渡方案：Worker 本机全量直拉（进行中）

Hub 扩盘前，在 **7143** 保持 `local_only`，把镜像预热进本机 Docker（不经 Hub tar）：

| 项 | 状态 |
|----|------|
| Docker data-root | ✅ 已迁到 **`/data/docker`**（`/data` ~14T / 约 12T 可用）；脚本 `scripts/migrate-docker-dataroot.sh` |
| 全量 pull | 🔄 后台跑 `scripts/pull-swe-pro-images-worker.sh`（731，缺约 728）；mirror：dockerproxy → DaoCloud → NJU → 1ms → Hub |
| 进度 | `tail -f /var/log/uenv/swe-pro-worker-pull.log`；`wc -l /data/uenv/swe-pro-pull/progress.jsonl`；`docker images jefzda/sweap-images -q \| wc -l` |
| 旧路径备份 | `/var/lib/docker.bak.*`（确认无误后可删） |

Hub 扩容后：可从 7143 `docker save` → `publish-image` 回填 Hub，其它节点再 `sync --docker-load`。
