# SWE-smith 7143 环境支持与联调记录

> 日期：2026-08-01
> 状态：**Phase 1–3 已跑通**；真实 LLM Agent 正式轨迹已产出（DeepSeek-V3 AWQ @7142）
> 规划：[SWE-smith环境支持与OpenHands-Rollout联调规划](./SWE-smith环境支持与OpenHands-Rollout联调规划.md)
> 变更与联调报告：[SWE-smith变更与联调报告](./SWE-smith变更与联调报告.md)
> 拓扑：[secrets/README.md](../../../secrets/README.md)

---

## 1. 结论

| 项 | 结果 |
|----|------|
| Worker `BenchmarkVariant::Smith` | ✅ 单测 + 实机 |
| 本地 EnvPackage | ✅ `/var/lib/uenv/envs/swe-bench-smith/0.1.0-local`（5 条 oauthlib，镜像本机已有） |
| Catalog 合并 Pro+Smith | ✅ 启动日志 `731 + 5 → catalog=736` |
| Gateway provision `/testbed` | ✅ |
| 负向（无 gold） | ✅ `resolved=false reward=0.0 tests=0/13` |
| 正向（reverse gold） | ✅ `resolved=true reward=1.0 tests=13/13` |
| Trajectory 落盘 + Server 上传 ack | ✅ `benchmark_variant=smith` |
| OpenHands 旁路 gold（208.77→隧道→Gateway） | ✅ `resolved=true reward=1.0 tests=13/13`；`git apply -R` @ `/testbed` |
| Adapter `SubmitEpisode` 目标架构 gold | ✅ Server→Worker→AgentJob→OpenHands；`status=completed` `reward=1.0` |
| Phase 3 Rollout 导出 | ✅ `scripts/export_swe_rollout_jsonl.py` → `chat_sft.resolved.jsonl`（smith / resolved=true） |
| OpenHands 服务探活（2026-08-01 晚） | ✅ 见 §5.3 |
| 7142 DeepSeek vLLM 拉起 | ✅ `vllm-dsv3-awq` + gateway `:18888` ready |
| 真实 LLM Agent 正式轨迹 | ✅ `…00045` variant=smith seal+server_verified（resolved=false） |
| 7142 训练可读 smoke | ✅ `schema_ok=true`（chat_sft.jsonl） |
| Hub 注册 | ⏳ 按规划后续交接 |

---

## 2. 关键实现要点

1. **`benchmark_variant=smith`**（别名 `swe-smith` / `swesmith` 等），grader=`swesmith`（复用 pytest 口径）。
2. **工作区 `/testbed`**；空 `base_commit` → `git reset --hard HEAD`。
3. **Smith `patch` 语义**：数据集字段是**造 bug 补丁**。provision 时 Worker **正向**注入；gold 验收用 **`git apply -R`**。
4. 注入 bug / 还原后执行 `install_cmd`（默认 `pip install -e . -q`）。
5. 多 EnvPackage：`env_package_dir`（Pro）+ `env_package_dirs`（Smith）合并 catalog。

---

## 3. 7143 部署位置

| 路径 | 说明 |
|------|------|
| `/var/lib/uenv/envs/swe-bench-smith/0.1.0-local/` | 本地 EnvPackage |
| `config/uenv-worker.deploy-7143-swe-pro.yaml` | `variants: [pro, smith]` + `env_package_dirs` |
| `scripts/export_swe_smith_instances.py` | 从 HF parquet 导出 catalog（`--only-local-images`） |
| `scripts/restart-worker-gateway-28097-7143.sh` | 自动带上 Smith 包路径 |

重启：

```bash
# 7143
SKIP_REBUILD=1 bash scripts/restart-worker-gateway-28097-7143.sh
```

Gateway smoke：

```bash
IID=oauthlib__oauthlib.1fd52536.combine_file__0fceycuu
# 正向：reverse gold → reward=1.0（smoke 时可将该 instance 的 PASS_TO_PASS 置空以加速）
python3 scripts/swe_gateway_demo.py \
  --endpoint 127.0.0.1:28097 --api-key swe-pro-secret \
  --instance "$IID" \
  --instances /var/lib/uenv/envs/swe-bench-smith/0.1.0-local/catalog.json \
  --benchmark-variant smith
```

---

## 4. 实机证据（2026-08-01）

```text
swe_catalog_loaded_from_env_package package_id=swe-bench-pro count=731
swe_catalog_loaded_from_env_package package_id=swe-bench-smith count=5
runtime_gateway_start catalog=736

negative: resolved=False reward=0.0 tests=0/13
positive:  resolved=True  reward=1.0 tests=13/13
trajectory_id=trj-worker-7143-pro-1785568288822-00002
  variant=smith resolved=True reward=1.0 steps=4
trajectory_upload_acked → http://8.130.75.157:8077
```

样例 fixture（缩略）：`fixtures/swe/smith_smoke_sample.json`。

---

## 5. Phase 2 实机证据（2026-08-01）

### 5.1 OpenHands 旁路 gold（208.77）

```bash
export UENV_GATEWAY=http://127.0.0.1:28097
export UENV_BENCHMARK_VARIANT=smith
export UENV_PRO_INSTANCE=oauthlib__oauthlib.1fd52536.combine_file__0fceycuu
bash /root/UEnv/scripts/run-openhands-pro-20877.sh gold
```

```text
gold_apply: cd /testbed && git apply -R ...  exit_code=0  reverse=true
resolved=true reward=1.0 tests=13/13
trajectory_id=trj-worker-7143-pro-1785570444226-00003
benchmark_variant=smith workspace_dir=/testbed
```

### 5.2 目标架构 SubmitEpisode（Adapter → Server → Worker → Agent）

```bash
# 本机（protobuf 新版本需纯 Python 实现）
cd uenv-bridge
PROTOCOL_BUFFERS_PYTHON_IMPLEMENTATION=python PYTHONPATH=src \
  python3 scripts/benchmark/evaluate_swesmith_uenv.py \
  --endpoint 8.130.75.157:8088 --agent-mode gold --limit 1
```

```text
AgentJob: variant=smith workspace=/testbed mode=gold gateway=http://127.0.0.1:28097
EpisodeResult: status=completed resolved=true reward=1.0 tests=13/13
trajectory_id=trj-worker-7143-pro-1785570520367-00004
elapsed≈18.7s
```

脚本：`uenv-bridge/scripts/benchmark/evaluate_swesmith_uenv.py`（默认 `smith` / `/testbed` / `swe-bench-smith`）。
Driver：`run_swebenchpro_official.py`（smith 分支 reverse-gold）+ `run_swesmith_official.py` 薄封装。


### 5.3 OpenHands / 208.77 服务探活（2026-08-01 23:35 CST）

| 组件 | 状态 | 说明 |
|------|------|------|
| `uenv-agent-poller` | **active** | `openhands_runner.py`；Server 注册 `openhands-default` heartbeat≈3s、`stale=false` |
| runner health `:8777` | **ok** | `{"status":"ok","service":"openhands-runner"}` |
| runner API `:8888` | 在听（根路径 404 正常） | 旁路 HTTP；近期仍在收 Pro llm job |
| `uenv-gateway-tunnel` | **active** | `127.0.0.1:28097` → 7143 Gateway（经 7142） |
| Gateway 探活 | **ok** | 正确路径：`GET /runtime/v1/health` → `200 ok`（`/health` 会 404，勿用） |
| `openhands-runner.service` | inactive | 预期：由 `uenv-agent-poller` 拉起 runner，不必单独 enable |
| 近期作业 | 正常 | 23:12–23:22 连续完成多条 Pro `mode=llm` AgentJob（reward=0 为任务未解决，非服务故障） |

7143 Worker health `:28777` ok；磁盘仍约 **92%**。Server `:8088` / `:8077` 在听。

### 5.4 真实 LLM Agent 轨迹（2026-08-02）

**vLLM**：7142 `vllm-dsv3-awq` + `uenv-llm-gateway` `:18888`（`deepseek-v3-0324-awq`）已拉起；此前 Qwen `:18088` 掉线且 8 卡曾被 `ronghao` Ray/VeRL 占满，腾卡后启动 DeepSeek。

**旁路 OpenHands llm**（poller 临时停，避免抢 Agent）：

```bash
export UENV_BENCHMARK_VARIANT=smith
export OPENHANDS_LLM_CONFIG=/root/UEnv/config/openhands-llm-swesmith-dsv3.json
export UENV_SWE_INSTANCES=/root/UEnv/fixtures/swe/smith_catalog.json
bash /root/UEnv/scripts/run-openhands-pro-20877.sh llm
```

```text
trajectory_id=trj-worker-7143-pro-1785605632110-00045
benchmark_variant=smith
resolved=false reward=0.0 tests=0/13   # Agent 未修好，但正式 seal/上传成功
server_verified=true
git_diff≈3487B（oauthlib/oauth1/rfc5849/__init__.py 有改动）
pre_submit: remote=https://github.com/swesmith/oauthlib__oauthlib.1fd52536 @ /testbed
```

修复：`run_swebenchpro_official.py` pre-submit 勿写死 `git -C /app`（Smith 为 `/testbed`）。

导出：`/var/lib/uenv/rollouts/swesmith-llm-dsv3-smoke/`；仓库样例 `Docs/worker/260801/artifacts/swesmith-llm-dsv3-smoke/`。
7142 训练可读 smoke：`train_smoke_rollout_jsonl.py` → `schema_ok=true`。

### 5.5 Phase 3 Rollout 导出


```bash
python3 scripts/export_swe_rollout_jsonl.py \
  --input-dir /var/log/uenv/openhands-runs \
  --variant smith --resolved-only --copy-bundles \
  --output-dir /var/lib/uenv/rollouts/swesmith-phase3-smoke
```

实机产物（208.77）：`/var/lib/uenv/rollouts/swesmith-phase3-smoke/`
仓库样例：`Docs/worker/260801/artifacts/swesmith-rollout-smoke/`

```text
trajectory_id=trj-worker-7143-pro-1785570520367-00004
variant=smith resolved=true reward=1.0 step_count=4
outputs: bundles.jsonl / chat_sft.jsonl / chat_sft.resolved.jsonl
```

已知限制：gold 轨迹的 `steps` 为 Gateway 捕获（provision/exec/write），非完整 LLM 多轮对话；LLM Agent 轨迹可复用同一导出脚本。`git_diff` 在 reverse-gold 场景可能为空（工作区已还原）。

---

## 6. 已知限制 / 下一步

| 项 | 说明 |
|----|------|
| 全量 P2P | oauthlib 单条 `PASS_TO_PASS` 可达数百；smoke 曾临时清空 P2P 验证 F2P；完整 catalog 已恢复 |
| LLM Agent episode | Phase 2/3 验收用 **gold**；真实 LLM 路径依赖模型 endpoint，可复用同一 SubmitEpisode |
| 7142 1-step 训练 | 可选；当前已交付可读 JSONL + schema 校验 |
| Hub | `swe-bench-smith` 包注册由 Hub 模块承接 |
| 磁盘 | 7143 `/` 约 92% 占用；扩子集前先清理或外置盘 |
| 208.77 env | `.openhands-20877.env` 默认 `UENV_SWE_INSTANCES=pro-full-731`；Smith 时 shell 会改绑 smith catalog |

---

## 7. 涉及代码（本轮）

### Worker（Phase 1）
- `uenv-worker/src/swe/variant.rs` — `Smith`
- `uenv-worker/src/swe/dataset.rs` — workspace / namespace / grader
- `uenv-worker/src/swe/grader.rs` — `SwesmithGrader`
- `uenv-worker/src/swe/resettable.rs` — 空 commit → HEAD
- `uenv-worker/src/swe/session.rs` — provision 注入 bug；`apply_patch_reverse`
- `uenv-worker/src/swe/harness.rs` / `instance_pool.rs` — smith gold 走 reverse
- `uenv-worker/src/config/mod.rs` / `runtime.rs` / `main.rs` — 多 EnvPackage 合并
- `scripts/export_swe_smith_instances.py`、`swe_gateway_demo.py`、重启脚本与 deploy yaml

### OpenHands / Adapter（Phase 2）
- `integrations/openhands/uenv_runtime/agent_job.py` — variant 归一化；smith → `/testbed`
- `integrations/openhands/uenv_runtime/agent_client.py` — proto → AgentJob workspace
- `integrations/openhands/run_swebenchpro_official.py` — smith reverse-gold + catalog 解析
- `integrations/openhands/run_swesmith_official.py` — 薄封装
- `scripts/run-openhands-pro-20877.sh` — `UENV_BENCHMARK_VARIANT=smith`
- `uenv-bridge/scripts/benchmark/evaluate_swesmith_uenv.py` + `data/benchmarks/swesmith/smoke.jsonl`
- `config/swe/smith-smoke.json`

### Rollout（Phase 3）
- `scripts/export_swe_rollout_jsonl.py` — TrajectoryBundle → `chat_sft*.jsonl`
