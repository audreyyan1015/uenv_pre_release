# SWE GRPO rollout-trace 补齐变更与联调报告（2026-08-02）

> 日期：2026-08-02  
> 范围：OpenHands 真实 LLM → `CompleteAgentJob` typed rollout fields → Adapter/VeRL GRPO 可训 token trace  
> 诊断原文：[Docs/adapter/20260802-SWE-GRPO训练缺少rollout-trace诊断说明.md](../../adapter/20260802-SWE-GRPO训练缺少rollout-trace诊断说明.md)  
> 相关前情：[Docs/worker/260801/SWE-smith变更与联调报告.md](../260801/SWE-smith变更与联调报告.md)  
> 拓扑：[secrets/README.md](../../../secrets/README.md)

---

## 1. 报告结论

| 项 | 结果 |
|------|------|
| 根因确认 | Gateway Trajectory 不含 LLM token；训练字段须由 OpenHands 在每次真实 LLM 调用采集并经 `CompleteAgentJob` 回填 |
| 代码缺口补齐 | ✅ `RolloutTraceCollector` 从 Ark-only 扩展为 OpenAI-compatible / vLLM |
| 落盘与回填 | ✅ `llm_rollout_trace.json` + `submit_result.json`；runner 读入 typed 字段 |
| 服务器同步 | ✅ 208.77 OpenHands / runner；7142 vLLM 启用 `--return-tokens-as-token-ids` |
| 实机联调 | ✅ smith llm smoke：`response_ids`/`rollout_log_probs` 对齐非空 |

**一句话**：SWE GRPO 所需的多轮 assistant `response_ids` + `rollout_log_probs` 已可由 OpenHands 真实产出；Adapter 侧不再必须依赖 pad fallback 才能凑 batch 形状。

---

## 2. 问题回顾

Adapter 同步链路 smoke（`verl_swe_fix_smoke_20260802_182210`）中 8 条 episode 均完成，但结果为：

```text
response_source=empty
used_pad_fallback=true
response_ids=[]
rollout_log_probs_len=0
```

训练语义要求一次 episode 内**全部** LLM assistant completion 按时间顺序拼接：

```text
response_ids = turn_0 + turn_1 + … + turn_N
response_mask = 与 ids 等长（当前全 1）
rollout_log_probs = 与 ids 等长、逐 token 对齐
```

环境 observation / prompt **不得**进入 `response_ids`。

协议入口此前已具备（`agent_client.complete_agent_job` / runner 读取），缺口在 **driver 未从 vLLM/OpenAI 路径采集并对齐 token ids**。既有 collector 仅服务 Ark（`thinking` 关闭 + `/tokenization`）。

---

## 3. 变更清单

### 3.1 OpenHands / Agent Bridge

| 文件 | 变更摘要 |
|------|----------|
| `integrations/openhands/uenv_runtime/llm_rollout.py` | 通用化：`logprobs` 强制；解析 `token_id` / `token_id:N` / `uenv_response_ids`；Ark `/tokenization`；OpenAI `/tokenize` 或 HF `tokenizer=` 回退；非 Ark 不强制 `thinking` |
| `integrations/openhands/run_swebenchpro_official.py` | 与 208 现网 driver 对齐（workspace probe / smith reverse-gold）；llm 模式安装 collector → `finalize()` → 写 `llm_rollout_trace.json` 并 `submit_doc.update` |
| `integrations/openhands/tests/test_client_smoke.py` | 补充 openai `token_id` / `uenv_response_ids` 单测 |
| `scripts/openhands/openhands_runner.py` | 优先读 `llm_rollout_trace.json`；默认 `UENV_REQUIRE_SWE_RESPONSE_TRACE=1`，llm 完成但无 `response_ids` 时 fail-fast |

### 3.2 推理侧（7142）

| 项 | 变更 |
|------|------|
| `vllm-dsv3-awq.service` | 增加 `--return-tokens-as-token-ids --max-logprobs 20` |
| LLM JSON（208.77 `config/openhands-llm-*.json`） | openai 模型补 `return_tokens_as_token_ids: true`（collector 默认也会在 extra_body 请求） |

### 3.3 文档

| 路径 | 说明 |
|------|------|
| `Docs/adapter/20260802-SWE-GRPO训练缺少rollout-trace诊断说明.md` | 增补「2026-08-02 修复状态」与核验口径 |
| `Docs/worker/260802/SWE-GRPO-rollout-trace补齐变更与联调报告.md` | 本报告 |

---

## 4. 采集与回填链路（冻结）

```text
OpenHands LLM.completion / acompletion
  └─ RolloutTraceCollector.install（logprobs=true；可选 return_tokens_as_token_ids）
       └─ record() 每轮 logprobs + token ids
            └─ finalize() 多轮拼接
                 ├─ llm_rollout_trace.json
                 └─ submit_result.json（合并 rollout_trace / rollout_log_probs）
                      └─ openhands_runner._read_rollout_fields
                           └─ CompleteAgentJob
                                ├─ rollout_trace.response_ids / response_mask
                                └─ rollout_log_probs
```

id 解析优先级：

1. choice / raw 上的 `uenv_response_ids` 或 `token_ids`
2. 每条 logprob 的 `token_id` 字段或 `token_id:N` 字符串
3. Ark：`/tokenization`
4. OpenAI-compatible：`/tokenize` → 否则 HF `tokenizer`（config）

长度约束（fail）：`len(response_ids) == len(response_mask) == len(rollout_log_probs)`。

---

## 5. 同步与运维

| 主机 | 动作 |
|------|------|
| **208.77** | tar 同步 `integrations/openhands` + `scripts/openhands`；确认存在 `llm_rollout.py`；重启 `uenv-agent-poller` |
| **7142** | 修复并重写 `vllm-dsv3-awq` ExecStart（含 token_ids 开关）；`systemctl restart vllm-dsv3-awq`；gateway `:18888` 探活至 `deepseek-v3-0324-awq` READY |

说明：`scripts/deploy-openhands-20877.sh` 经 7142 jump 的密钥登录 208 当前不可用，本次用 `secrets/_ssh208.py` put/run 同步。

---

## 6. 联调证据

### 6.1 单测

```text
python3 -m pytest integrations/openhands/tests/test_client_smoke.py -q
→ 7 passed, 1 skipped
```

### 6.2 vLLM logprobs 形态（7142 本机）

```text
token 形态: ["token_id:23166", "token_id:3", ...]
token_id 字段: null（由字符串解析）
logprob: 逐 token 浮点非空
```

### 6.3 实机 smith llm smoke

| 项 | 值 |
|------|------|
| 输出目录 | `/var/log/uenv/openhands-runs/rollout-trace-smoke-20260802-234803` |
| 实例 | `oauthlib__oauthlib.1fd52536.combine_file__0fceycuu` |
| 配置 | `openhands-llm-swesmith-dsv3.json` → `219.147.100.43:18888` |
| `MAX_ITERATIONS` | 3 |
| turns | 3 |
| `response_ids_len` | 6144 |
| `rollout_log_probs_len` | 6144 |
| aligned | true |
| source | `openai_chat_logprobs+token_ids` |
| turn_id_sources | `provider_token_ids` × 3 |
| trajectory_id | `trj-worker-7143-pro-1785686290503-00093` |
| server_verified | true |
| CompleteAgentJob 载荷形态 | `complete_agent_job_payload_ok=true` |
| reward / resolved | `0.0` / `false`（本次验证字段完备性，非修 bug 成功率） |

落盘文件：`llm_rollout_trace.json`、`submit_result.json`（含聚合 `rollout_trace`）、`trajectory_bundle.json`。

---

## 7. Adapter / 训练侧注意事项

正式训练请保持：

```text
UENV_REQUIRE_SWE_RESPONSE_TRACE=1
```

期望 `agent-loop-results.jsonl`：

```text
response_source=rollout_trace
used_pad_fallback=false
len(response_ids) > 0
rollout_log_probs_len == len(response_ids)
```

仅链路 smoke 时可临时 `UENV_REQUIRE_SWE_RESPONSE_TRACE=0`（会 pad fallback，**不可**当有效 GRPO 样本）。

**VeRL 自建 vLLM** 若作为 OpenHands `base_url`，同样建议：

```text
--return-tokens-as-token-ids --max-logprobs 20
```

否则需保证 `/tokenize` 可用，或在 LLM JSON 配置 `tokenizer=`（与训练 tokenizer 一致），并仍满足 id/logprob 等长。

---

## 8. 残留与后续

| 项 | 状态 |
|------|------|
| OpenHands → CompleteAgentJob 字段完备 | ✅ 已验证 |
| Adapter 端端到端再跑一轮 GRPO smoke 确认 `response_source=rollout_trace` | ⏳ 训练侧复跑 |
| Hub EnvPackage 正式注册 | ⏳ 不阻塞本缺口 |
| DeepSeek smoke 出现长重复输出（拉满 max tokens） | 模型行为问题，与 trace 采集无关；正式跑可下调 `max_output_tokens` / 迭代 |

---

## 9. 验收清单

- [x] 每次真实 LLM 调用请求并拿到 content logprobs  
- [x] 每次调用拿到与 logprobs 等长的 response token ids  
- [x] `llm_rollout_trace.json` / `submit_result.json` 含 per-turn 与聚合字段  
- [x] runner 可读并构成 `CompleteAgentJob` typed 载荷  
- [ ] Adapter `agent-loop-results.jsonl` 显示 `response_source=rollout_trace` 且 `used_pad_fallback=false`（待训练侧复验）
