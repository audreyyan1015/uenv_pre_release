# 7143 SWE 容器残留与 `exec` 无超时诊断报告

> **日期**：2026-08-06  
> **范围**：A100 Worker `219.147.100.43:7143`；关联 Agent 池 `8.130.208.77`、Server `8.130.75.157`、训练侧 7142  
> **状态**：运维回收已完成；代码缺陷待与 Warmup/调度改造一并合入  
> **关联**：[SWE-GRPO 预热池与 Agent-Worker 调度讨论](./SWE-GRPO预热池与Agent-Worker调度讨论.md)

---

## 1. 摘要

2026-08-06 巡检发现 7143 上：

1. **两个 tenacity SWE 容器**已运行约 38–39 小时，内含多组 `pytest` **活进程**（非内核僵尸 `Z`），各约占满 1 核；合计约 **8 核 / load≈16**。
2. 另有 **teleport / ansible** 容器各运行约 2 天（无活跃 pytest，属未销毁残尸）。
3. 根因是 **SWE-smith + OpenHands Agent 探索性评测** 触发 tenacity「无限 retry」测试死循环，叠加 Worker **`exec_raw` 无超时、episode 结束后容器未可靠销毁**。
4. **同日 20:14 CST 已安全回收**上述残留；**未动**当时 GRPO 在用的 oauthlib 容器与 `uenv-worker` 进程。

---

## 2. 现场快照（回收前）

| 项 | 值 |
|----|-----|
| Worker | `uenv-worker` pid `1992938`，config `uenv-worker.deploy-7143-swe-pro.yaml`，health `ok` |
| Load | ≈16.1–16.4 |
| 挂死 pytest | 8 组（每容器 4 组），状态 `R`，CPU ≈99.9%/进程 |
| 内核僵尸 `Z` | 另有约 13 个 `ash`/`sh` defunct（父进程无关 `tail -f /dev/null`），**与 tenacity 残留不是同一类问题** |

残留容器：

| 容器名（前缀） | 时长 | 处理 |
|----------------|------|------|
| `uenv-swe-jd--tenacity-…8pa1fxvj-…1785877671…` | ~39h | **已 `docker rm -f`** |
| `uenv-swe-jd--tenacity-…hr16k2ip-…1785881971…` | ~38h | **已 `docker rm -f`** |
| `uenv-swe-instance-…teleport-…` | ~2d | **已 `docker rm -f`** |
| `uenv-swe-instance-…ansible-…` | ~2d | **已 `docker rm -f`** |
| `uenv-swe-oauthlib-…`（当时活跃） | 分钟级 | **保留** |

回收后（2026-08-06 20:14 CST）：tenacity/teleport/ansible 进程与容器清零；Worker health `ok`；仅保留 oauthlib 当前会话容器；load 开始回落。

---

## 3. 日志追溯：为何拉起这两个容器？

有完整日志，**非误启**。

### 3.1 Worker（`/var/log/uenv/worker-swe-pro.log`）

| session | instance_id | provision | trajectory sealed |
|---------|-------------|-----------|-------------------|
| `sess-…8pa1fxvj-213` | `jd__tenacity.0d40e76f.combine_file__8pa1fxvj` | 2026-08-04 **21:08** | 2026-08-04 **21:50**（`trj-…-00204`，upload acked） |
| `sess-…hr16k2ip-228` | `jd__tenacity.0d40e76f.combine_file__hr16k2ip` | 2026-08-04 **22:19** | 2026-08-04 **23:01**（`trj-…-00218`，upload acked） |

同一晚同一 instance 还有多次 session（210/211/226/227/229 等），属 GRPO/`n=4` 多采样常态。

### 3.2 Agent（208.77 `/var/log/uenv/openhands-runs/`）

| AgentJob 目录 | session | 结果 |
|---------------|---------|------|
| `agent-job-8b46bc97-…-20260805-050805` | `…8pa1fxvj-213` | `reward=1.0`，`123/123`，elapsed ≈2522s |
| `agent-job-17641943-…-20260805-061945` | `…hr16k2ip-228` | `reward=1.0`，`123/123`，elapsed ≈2531s |

### 3.3 挂死 `docker exec` 启动时间

与 Agent 探索阶段对齐（**不是** seal 当晚立刻挂死）：

- `8pa1fxvj`：2026-08-05 **05:08 / 05:18 / 05:28 / 05:39**（约每 10 分钟一组）
- `hr16k2ip`：2026-08-05 **06:20 / 06:30 / 06:40 / 06:51**

典型命令：

```text
python -m pytest tests/test_tenacity.py -v -k "retry_try_again or retry_until" 2>&1 | head -80
python3 -m pytest tests/test_tenacity.py::TestDecoratorWrapper::test_retry_until_exception_of_type_* -v --tb=short
```

父进程为当时的 `uenv-worker`（Gateway `docker exec` 路径）。

**时间线一句话**：8/4 晚 provision + seal → session/容器未销毁 → 8/5 凌晨 Agent 复用同 session 跑 LLM → 探索性 pytest 死循环并重试叠加 → 官方 submit 仍成功（reward=1.0）→ 孤儿 `docker exec` 与容器挂到 8/6 巡检。

---

## 4. 根因分析

### 4.1 触发条件（样本 + Agent 行为）

- Instance 属于 **tenacity**，FAIL_TO_PASS / 探索路径覆盖 `retry_until*` / `retry_try_again`。
- Agent 在修补 `retry_unless_exception_type` 等逻辑时，对**半成品代码**跑上述 pytest；补丁语义错误时 retry 谓词永不收敛 → **用户态忙等**（`R` + 100% CPU，`strace` 见大量 `pselect6`）。
- 部分命令带 `| head -80` 且**无** `timeout N` 包装；若 pytest 在产满 80 行前卡住，管道双方一直挂起。

### 4.2 Worker 缺陷（为何能挂 1.5 天）— 主因

`uenv-worker/src/swe/session.rs`：

```rust
fn exec_raw(&self, command: &str) -> Result<ExecResult, DynErr> {
    let out = Command::new(self.runtime.cli())
        .args(["exec", &self.container, "bash", "-lc", command])
        .output()  // 同步死等：无超时、无 kill
        ...
}
```

| 缺陷 | 说明 |
|------|------|
| **`timeout_sec` 未落地** | `CommandPolicyConfig.timeout_sec` 默认 120s 仅存在配置/单测，**未用于** `exec_raw` |
| **取消不杀子进程** | Agent/HTTP 侧超时重试后，宿主机旧 `docker exec` 仍存活 → ~10min 一组叠加 |
| **容器销毁不可靠** | `Drop` 可 `docker rm -f`，但 `keep`/未释放/池化路径下残尸常见；Worker 日志中 **destroy/rm 类消息计数为 0** |
| **无泄漏巡检** | 无 idle 超时扫尾、无「孤儿 docker exec」指标/告警 |

### 4.3 非根因澄清

| 说法 | 判定 |
|------|------|
| 「内核僵尸进程」 | **否**。挂死 pytest 为 `R`/`S` 活进程 |
| 「与当前 7142 GRPO 无关的误启」 | **否**。来自 8/4–8/5 SWE-smith Agent episode；submit 已成功 |
| 「必须停训才能清」 | **否**。可只 `rm -f` 非当前 instance 容器 |

---

## 5. 运维处置记录

**时间**：2026-08-06 20:14 CST（7143）

```bash
docker rm -f \
  uenv-swe-jd--tenacity-0d40e76f-combine-file--hr16k2ip-1992938-1785881971514193837 \
  uenv-swe-jd--tenacity-0d40e76f-combine-file--8pa1fxvj-1992938-1785877671006575784 \
  uenv-swe-instance-gravitational--teleport-… \
  uenv-swe-instance-ansible--ansible-…
# 保留当时 oauthlib 活跃容器与 uenv-worker
```

验收：

- tenacity / teleport / ansible 容器与 pytest 进程清零  
- `uenv-worker` 仍在；`curl :28777/health` → `ok`  
- 仅 oauthlib（或后续新 episode）容器保留  

---

## 6. 代码改进项（并入调度规划）

已写入 [SWE-GRPO预热池与Agent-Worker调度讨论.md](./SWE-GRPO预热池与Agent-Worker调度讨论.md) **§9**，与 Warmup/租约改造一并实施。摘要：

1. **P0** `exec_raw` 强制超时 + 超时杀进程树（落实 `timeout_sec`）  
2. **P0** episode/AgentJob/Gateway destroy 结束必 `destroy` 容器 + 可观测日志  
3. **P0** Worker 启动 reconcile + idle 扫尾（与规划中 WAL/残尸条目对齐）  
4. **P1** 请求取消时保证杀 `docker exec` 子树  
5. **P2** Agent/评测侧对 retry 类命令强制 `timeout`；泄漏指标与告警  

---

## 7. 附录：同期 7142 GRPO（对照）

巡检同日 7142 上 `ronghao` 正在跑：

- Run：`verl_swesmith_grpo_train_20260806_165600`  
- 实验：`qwen3_6_35b_a3b_swesmith_grpo_limit20_20260806_165600`  
- 进度约 **6/10**（随后进入后续 step）；与本报告残留 **无直接因果关系**，但说明 7143 残留会与**同链路后续 episode** 抢 CPU。

日志：`/data/ronghao/uenv/uenv-bridge/temp/logs/verl_layer4_agent_loop/verl_swesmith_grpo_train_20260806_165600.log`
