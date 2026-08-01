# SWE-smith 变更与联调报告（2026-08-01）

> 日期：2026-08-01～2026-08-02  
> 范围：Worker 支持 SWE-smith → OpenHands / Adapter 全链路 → Rollout 导出 → 7142 DeepSeek vLLM 真实 LLM Agent 正式轨迹  
> 规划：[SWE-smith环境支持与OpenHands-Rollout联调规划](./SWE-smith环境支持与OpenHands-Rollout联调规划.md)  
> 过程记录：[SWE-smith-7143联调记录](./SWE-smith-7143联调记录.md)  
> 拓扑：[secrets/README.md](../../../secrets/README.md)

---

## 1. 报告结论

| 阶段 | 目标 | 结果 |
|------|------|------|
| **Phase 1** | Worker 本地支持 `benchmark_variant=smith` | ✅ Gateway 负向/正向 gold 通过 |
| **Phase 2** | Server / Worker / OpenHands 目标架构联调 | ✅ `SubmitEpisode` gold：`resolved=true reward=1.0` |
| **Phase 3** | Rollout 导出与训练可读校验 | ✅ `chat_sft*.jsonl` + 7142 `schema_ok` |
| **LLM** | 拉起 vLLM + 真实 Agent 正式 seal 轨迹 | ✅ DeepSeek-V3 AWQ；轨迹 `…00045`（`resolved=false`，但正式产出） |
| **Hub** | EnvPackage 正式注册 | ⏳ 后置，不阻塞本期 |

**一句话**：Smith 环境契约与 Pro 共用 Gateway/Agent 栈已打通；gold 与真实 LLM 均可产出带 `benchmark_variant=smith` 的正式 TrajectoryBundle，并可导出训练侧可读 JSONL。

---

## 2. 变更清单

### 2.1 Worker（Rust）

| 文件 | 变更摘要 |
|------|----------|
| `uenv-worker/src/swe/variant.rs` | 新增 `BenchmarkVariant::Smith` 及别名解析 |
| `uenv-worker/src/swe/dataset.rs` | Smith catalog / 工作区 `/testbed` / grader 映射 |
| `uenv-worker/src/swe/grader.rs` | `SwesmithGrader`（pytest 口径） |
| `uenv-worker/src/swe/resettable.rs` | 空 `base_commit` → `git reset --hard HEAD` |
| `uenv-worker/src/swe/session.rs` | provision **正向注入造 bug patch**；`install_cmd` |
| `uenv-worker/src/swe/harness.rs` / `instance_pool.rs` | Smith gold 走 **`git apply -R`** |
| `uenv-worker/src/swe/mod.rs` | 模块导出 |
| `uenv-worker/src/config/mod.rs` | `env_package_dirs` 多包合并 |
| `uenv-worker/src/runtime.rs` / `main.rs` | 多 EnvPackage 加载进 catalog |

### 2.2 OpenHands / Agent Bridge

| 文件 | 变更摘要 |
|------|----------|
| `integrations/openhands/uenv_runtime/agent_job.py` | variant 归一化；smith 默认 `workspace_dir=/testbed` |
| `integrations/openhands/uenv_runtime/agent_client.py` | proto → AgentJob 时按 variant 解析 workspace |
| `integrations/openhands/run_swebenchpro_official.py` | smith reverse-gold；catalog 回退；**pre-submit 使用 `workspace_dir`（勿写死 `/app`）** |
| `integrations/openhands/run_swesmith_official.py` | **新增** Smith 薄封装（默认 `benchmark_variant=smith`） |
| `scripts/run-openhands-pro-20877.sh` | 支持 `UENV_BENCHMARK_VARIANT=smith`；避免误用 Pro 全量 catalog |

### 2.3 Adapter / 评测 / 导出

| 文件 | 变更摘要 |
|------|----------|
| `uenv-bridge/scripts/benchmark/evaluate_swesmith_uenv.py` | **新增**；默认 smith / `/testbed` / `swe-bench-smith` |
| `uenv-bridge/data/benchmarks/swesmith/smoke.jsonl` | **新增** oauthlib smoke 一行 |
| `scripts/export_swe_smith_instances.py` | Smith catalog 导出（含默认 `install_cmd`） |
| `scripts/export_swe_rollout_jsonl.py` | **新增** TrajectoryBundle → `chat_sft*.jsonl` |
| `scripts/train_smoke_rollout_jsonl.py` | **新增** 7142 离线训练可读 / schema smoke |
| `scripts/swe_gateway_demo.py` | `--benchmark-variant` / reverse-gold 演示 |
| `scripts/restart-worker-gateway-28097-7143.sh` | 启动时挂上 Smith EnvPackage |

### 2.4 配置 / Fixtures / 文档

| 路径 | 说明 |
|------|------|
| `config/uenv-worker.deploy-7143-swe-pro.yaml` | `variants: [pro, smith]` + `env_package_dirs` |
| `config/swe/smith-smoke.json` | Smith catalog 样例 |
| `config/openhands-llm-swesmith-qwen36.json` | Smith LLM 配置模板（联调中曾切 DeepSeek） |
| `fixtures/swe/smith_smoke_sample.json` | 缩略 instance fixture |
| `Docs/worker/260801/*` | 规划、联调记录、本报告、artifacts |

---

## 3. 关键语义冻结（务必保留）

1. **调度键**：`env_type=swe` + `benchmark_variant=smith`（别名：`swe-smith` / `swesmith` / `swe-bench-smith`）。
2. **工作区**：`/testbed`（禁止硬编码 Pro 的 `/app`）。
3. **Smith `patch`**：数据集字段 = **造 bug 补丁**。  
   - provision：`git apply`（正向）+ `pip install -e .`  
   - gold：`git apply -R` + 常需 reinstall  
4. **grader**：`swesmith`（勿误用 `swebench_pro`）。
5. **Phase 1–3 不经 Hub**：本地 EnvPackage `/var/lib/uenv/envs/swe-bench-smith/0.1.0-local`。

---

## 4. 联调拓扑与探活

```text
7142 (vLLM DeepSeek :18888)
    ↑ OpenHands LLM
208.77 OpenHands Agent 池 ──隧道──► 7143 Gateway :28097
    ↑ PollAgentJob / CompleteAgentJob
Server 8.130.75.157:8088
    ↑ SubmitEpisode / Dispatch
Adapter / evaluate_swesmith_uenv.py
```

| 主机 | 角色 | 联调时状态 |
|------|------|------------|
| **7143** | Worker + Gateway | health `:28777` ok；Gateway `GET /runtime/v1/health` → `200 ok`（注意不是 `/health`） |
| **Server** | adapter-core | `:8088` / `:8077` 在听；Agent 池可注册 |
| **208.77** | OpenHands poller + 隧道 | `uenv-agent-poller` active；`:8777` ok；`uenv-gateway-tunnel` active |
| **7142** | DeepSeek vLLM + 训练可读 smoke | `vllm-dsv3-awq` + `uenv-llm-gateway` ready |

---

## 5. 联调证据摘要

### 5.1 Phase 1 — Gateway（7143）

样例 instance：`oauthlib__oauthlib.1fd52536.combine_file__0fceycuu`

| 场景 | 结果 |
|------|------|
| 负向（无 reverse gold） | `resolved=false reward=0.0 tests=0/13` |
| 正向（reverse gold） | `resolved=true reward=1.0 tests=13/13` |
| Catalog | Pro 731 + Smith 5 → `catalog=736` |
| Trajectory | `benchmark_variant=smith`；Server 上传 ack |

### 5.2 Phase 2 — OpenHands / SubmitEpisode（gold）

| 路径 | 结果 |
|------|------|
| 208.77 旁路 gold | `resolved=true reward=1.0`；`git apply -R` @ `/testbed`；`trj-…00003` |
| Adapter `SubmitEpisode` gold | `status=completed`；AgentJob `variant=smith workspace=/testbed`；`trj-…00004`；约 18.7s |

评测入口：

```bash
cd uenv-bridge
PROTOCOL_BUFFERS_PYTHON_IMPLEMENTATION=python PYTHONPATH=src \
  python3 scripts/benchmark/evaluate_swesmith_uenv.py \
  --endpoint 8.130.75.157:8088 --agent-mode gold --limit 1
```

### 5.3 Phase 3 — Rollout 导出

```bash
python3 scripts/export_swe_rollout_jsonl.py \
  --input-dir /var/log/uenv/openhands-runs \
  --variant smith --resolved-only --copy-bundles \
  --output-dir /var/lib/uenv/rollouts/swesmith-phase3-smoke
```

仓库样例：[`artifacts/swesmith-rollout-smoke/`](./artifacts/swesmith-rollout-smoke/)  
（gold 轨迹 `trj-…00004` → `chat_sft.resolved.jsonl`，`reward=1.0`）

7142：

```bash
python3 scripts/train_smoke_rollout_jsonl.py \
  --input .../chat_sft.resolved.jsonl \
  --output .../train_smoke_metrics.json
# → schema_ok=true
```

### 5.4 真实 LLM Agent（DeepSeek @7142）

**前提**：8×A100 曾被他户 Ray/VeRL 占满；Qwen 网关 `:18088` 掉线。腾卡后启动 DeepSeek-V3-0324-AWQ（约 10min 加载）。

| 项 | 值 |
|----|-----|
| 模型 | `deepseek-v3-0324-awq` via `http://219.147.100.43:18888/v1` |
| 驱动 | `run_swesmith_official.py` / `mode=llm`，`MAX_ITERATIONS=12` |
| `trajectory_id` | `trj-worker-7143-pro-1785605632110-00045` |
| `benchmark_variant` | `smith` |
| `server_verified` | `true` |
| `resolved` / `reward` | `false` / `0.0`（`0/13`） |
| `git_diff` | ~3487B（`oauthlib/oauth1/rfc5849/__init__.py`） |
| pre-submit remote | `https://github.com/swesmith/oauthlib__oauthlib.1fd52536` @ `/testbed` |

仓库样例：[`artifacts/swesmith-llm-dsv3-smoke/`](./artifacts/swesmith-llm-dsv3-smoke/)

**过程缺陷与修复**：首轮 LLM 在 pre-submit 因写死 `git -C /app` 失败；已改为 `git -C {workspace_dir}` 后重跑成功 seal。

---

## 6. 已知限制与后续

| 项 | 说明 |
|----|------|
| Hub 注册 | `swe-bench-smith` EnvPackage / 镜像分发由 Hub 模块承接 |
| 全量 Smith | 本期仅本地 5 条 oauthlib 子集；7143 磁盘约 92%，扩集前需空间 |
| LLM `resolved` | 真实 Agent smoke 未修好 bug（正常）；正式轨迹链路已验证 |
| SubmitEpisode(llm) | Agent 池常被外部 Pro 任务占用；本期 LLM 用旁路验证，目标架构可在池空闲时复跑 |
| Gateway 探活路径 | 正确为 `/runtime/v1/health`，根路径 `/health` 会 404 |
| protobuf | 本机新版 protobuf 与旧 `*_pb2.py` 不兼容时需 `PROTOCOL_BUFFERS_PYTHON_IMPLEMENTATION=python` |
| 7143 代码树 | 实机较旧时曾需注释 `set_wal_quarantined_records` 才能编译 |

---

## 7. 复现命令速查

```bash
# 7143 重启 Worker+Gateway（含 Smith 包）
SKIP_REBUILD=1 bash scripts/restart-worker-gateway-28097-7143.sh

# Gateway gold
python3 scripts/swe_gateway_demo.py \
  --endpoint 127.0.0.1:28097 --api-key swe-pro-secret \
  --instance oauthlib__oauthlib.1fd52536.combine_file__0fceycuu \
  --instances /var/lib/uenv/envs/swe-bench-smith/0.1.0-local/catalog.json \
  --benchmark-variant smith

# 208.77 旁路 Smith（gold / llm）
export UENV_GATEWAY=http://127.0.0.1:28097
export UENV_BENCHMARK_VARIANT=smith
export UENV_SWE_INSTANCES=/root/UEnv/fixtures/swe/smith_catalog.json
bash /root/UEnv/scripts/run-openhands-pro-20877.sh gold   # 或 llm

# 7142 拉起 DeepSeek（需 8 卡空闲、模型已就位）
bash /root/UEnv/scripts/uenv-llm-gateway/start-vllm-when-ready-7142.sh
bash /root/UEnv/scripts/uenv-llm-gateway/smoke-test-7142.sh
```

---

## 8. Artifacts 索引

| 目录 | 内容 |
|------|------|
| [`artifacts/swesmith-rollout-smoke/`](./artifacts/swesmith-rollout-smoke/) | gold 轨迹导出（resolved=true） |
| [`artifacts/swesmith-llm-dsv3-smoke/`](./artifacts/swesmith-llm-dsv3-smoke/) | 真实 LLM 轨迹 bundle / submit / chat_sft |

实机落盘（参考）：

- 208.77：`/var/lib/uenv/rollouts/swesmith-phase3-smoke/`、`.../swesmith-llm-dsv3-smoke/`
- 7143 EnvPackage：`/var/lib/uenv/envs/swe-bench-smith/0.1.0-local/`
- OpenHands 运行目录：`/var/log/uenv/openhands-runs/`、`/tmp/smith-oh-llm-dsv3c/`

---

## 9. 验收对照（规划 checklist）

| 规划项 | 状态 |
|--------|------|
| Worker provision ≥1 真实 Smith instance | ✅ |
| gold / Agent 可 submit 出 resolved + trajectory | ✅ gold；LLM seal ✅（resolved 未过） |
| 目标架构 SubmitEpisode ≥1 条 | ✅ gold |
| TrajectoryBundle 导出 + resolved 过滤 JSONL | ✅ |
| 7142 可读 + 字段校验 | ✅ |
| Hub 注册 | ⏳ 后续 |

**报告完。**
