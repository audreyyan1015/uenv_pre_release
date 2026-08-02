# SWE GRPO 训练缺少 rollout trace 诊断说明

## 1. 背景

Adapter 侧在调试 `Qwen/Qwen3.6-35B-A3B` + VeRL + UEnv + SWE-bench-Pro 训练链路时，已经确认 UEnv episode 提交、Worker/OpenHands 执行、gateway 到 VeRL 内部 vLLM 的模型访问链路可以跑通。但当前 Worker 返回的 `EpisodeResult` 没有包含 VeRL PPO/GRPO 训练所需的真实 response token trace，因此训练侧会退化成 pad fallback。

该状态可以用于验证链路连通性，不能作为有效 SWE GRPO 训练结果。

## 2. 165918 轮次报错原因

失败日志：

```text
/data/ronghao/uenv/uenv-bridge/temp/logs/verl_layer4_agent_loop/verl_sleep_reuse_probe_20260802_165918.log
```

关键报错：

```text
RuntimeError: The size of tensor a (410) must match the size of tensor b (4) at non-singleton dimension 2
```

位置在 VeRL 训练侧重算 old log prob：

```text
ray_trainer.py:_compute_old_log_prob
WorkerDict.actor_rollout_ref_compute_log_prob
Qwen3_5MoeForConditionalGeneration -> apply_rotary_pos_emb
```

该轮日志中没有 `UENV_PATCH_VERL_TEXT_ONLY_PROCESSOR=enabled`，同时模型目录存在 `preprocessor_config.json`，其中声明了 `processor_class=Qwen3VLProcessor`。实际模型本体是文本 MoE：

```text
model_type=qwen3_5_moe
architectures=["Qwen3_5MoeForConditionalGeneration"]
```

因此 VeRL 可能按 VL processor 路径构造了多模态 position ids，而训练侧文本 MoE forward 期望普通文本 position ids，最终在 rotary embedding 处维度不匹配。

Adapter 已增加 `UENV_PATCH_VERL_TEXT_ONLY_PROCESSOR`，将该 checkpoint 在 VeRL 中按 text-only processor 处理。后续 `verl_swe_fix_smoke_20260802_182210` 日志中可以看到 patch 已在 TaskRunner、WorkerDict 和 AgentLoopWorker 生效：

```text
UEnv patch: treating qwen3_5_moe checkpoint as text-only; VeRL hf_processor returns None.
```

## 3. 当前语义阻塞

跑通轮次：

```text
/data/ronghao/uenv/uenv-bridge/temp/logs/verl_layer4_agent_loop/verl_swe_fix_smoke_20260802_182210.log
/data/ronghao/uenv/uenv-bridge/temp/logs/layer4_distributed/verl_swe_fix_smoke_20260802_182210/agent-loop-results.jsonl
```

该轮 8 条 episode 均完成，gateway 均为 HTTP 200，VeRL 也完成了 `old_log_prob`、`ref`、`update_actor`、`update_weights`。但 `agent-loop-results.jsonl` 中每条结果都是：

```text
response_source=empty
used_pad_fallback=true
rollout_log_probs_len=0
response_ids=[]
verl_response_ids=[248044]
trajectory=[]
```

这说明 Adapter 没有拿到 OpenHands/Worker 侧真实模型输出 token，只能填入 1 个 pad/eos token 让 VeRL batch 形状继续走完。此时 `response_length/mean=1.0`，reward 也不能反映模型真实动作对应的 token 概率，训练没有实际意义。

## 4. Worker/OpenHands 侧需要返回的字段

SWE/OpenHands 训练需要 Worker 在完成 `AgentJob` 时返回模型生成的 token trace，至少包括：

```text
AgentJobCompleteRequest.rollout_trace.response_ids
AgentJobCompleteRequest.rollout_trace.response_mask
AgentJobCompleteRequest.rollout_log_probs
```

字段要求：

| 字段 | 要求 |
|---|---|
| `response_ids` | 所有需要参与 PPO/GRPO loss 的 assistant 生成 token id，推荐拼接整条 OpenHands 轨迹中的模型 assistant 输出。 |
| `response_mask` | 与 `response_ids` 等长；需要训练的 token 为 1，不训练的 token 为 0。 |
| `rollout_log_probs` | 与 `response_ids` 对齐的 old policy logprob；若暂时无法返回，至少先返回 `response_ids/response_mask`，由 VeRL 侧重算 old log prob。 |
| `rollout_param_version` / `rollout_policy_version` | 如模型响应中已经带版本信息，建议随结果透传，便于后续异步和 stale 判断。 |

当前 `uenv/integrations/openhands/uenv_runtime/agent_client.py::complete_agent_job()` 已有这些参数入口，`uenv/scripts/openhands/openhands_runner.py` 也会尝试从 `submit_result.json` 或 `trajectory_bundle.json` 读取 rollout trace 后上报。缺口主要在 OpenHands/SWE runner 侧是否能从每次 LLM 调用中收集 token ids/logprobs，并写入上述结果文件。

## 5. Adapter 当前处理

Adapter 已增加默认 fail-fast 保护：

```text
UENV_REQUIRE_SWE_RESPONSE_TRACE=1
```

当 `env_type=swe` 且 `EpisodeResult` 中没有 typed `rollout_trace.response_ids` 时，Adapter 会拒绝继续训练，避免静默使用 pad fallback。只做链路 smoke 时可以临时设置：

```text
UENV_REQUIRE_SWE_RESPONSE_TRACE=0
```

但该模式只能验证链路，不应作为正式训练配置。

## 6. 建议核验

Worker/OpenHands 侧可以按以下顺序核验：

1. 确认 OpenHands 调用模型时是否请求并拿到了 logprobs/token ids。
2. 确认 `submit_result.json` 或 `trajectory_bundle.json` 是否写入 `response_ids`、`response_mask`、`rollout_log_probs`。
3. 确认 `openhands_runner.py` 读取后是否传给 `complete_agent_job()`。
4. 确认 Server 返回给 Adapter 的 `EpisodeResult.trajectory.steps[*].rollout_trace` 不为空。

Adapter 侧可用以下结果文件复核：

```text
/data/ronghao/uenv/uenv-bridge/temp/logs/layer4_distributed/<RUN_ID>/agent-loop-results.jsonl
```

期望正式训练时看到：

```text
response_source=rollout_trace
used_pad_fallback=false
response_ids_len > 0
rollout_log_probs_len == response_ids_len
```
