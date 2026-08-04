# SWE GRPO 训练缺少 rollout trace 诊断说明

## 1. 背景

Adapter 侧在调试 `Qwen/Qwen3.6-35B-A3B` + VeRL + UEnv + SWE-bench-Pro 训练链路时，已经确认 UEnv episode 提交、Worker/OpenHands 执行、gateway 到 VeRL 内部 vLLM 的模型访问链路可以跑通。但当前 Worker/OpenHands 返回的 `EpisodeResult` 没有包含 VeRL PPO/GRPO 训练所需的真实 response token trace，因此训练侧无法构造有效的 response batch。

该状态可以用于验证链路连通性，不能作为有效 SWE GRPO 训练结果。SWE/OpenHands 是多轮 agentic 任务，训练样本不是普通单轮 QA 的最终答案文本，而是一次 episode 中多次 LLM action 共同组成的 rollout trace。

## 2. 当前现象

同步链路 smoke 轮次：

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

## 3. 训练语义

VeRL/GRPO 训练需要知道模型在 rollout 阶段实际生成了哪些 token，以及这些 token 对应的 old policy logprob。对于普通单轮问答，这通常等价于一次模型回答；对于 SWE/OpenHands，这对应一次 episode 内所有 LLM action。

OpenHands 一条 episode 的典型流程是：

```text
初始 issue prompt
-> LLM 第 1 次生成：决定运行 ls/cat/grep 等工具
-> 环境返回 terminal/file observation
-> LLM 第 2 次生成：分析并编辑文件
-> 环境返回新的 observation
-> LLM 第 N 次生成：继续测试或 finish
```

Worker/OpenHands 应该返回所有 LLM 调用中 assistant completion 部分的 token trace，按发生时间顺序拼接。环境 observation、terminal 输出、文件内容、测试日志、system/user prompt 不应放入 `response_ids` 作为训练 token；它们只作为下一次 LLM 调用的上下文。

因此，正确的训练口径是：

```text
response_ids = turn_0 assistant tokens + turn_1 assistant tokens + ... + turn_N assistant tokens
response_mask = 与 response_ids 等长，参与训练的 assistant token 为 1
rollout_log_probs = 与 response_ids 等长，逐 token 对齐的 old policy logprob
```

只返回最后一次模型调用的 token trace 不够，因为前面每一次工具选择、文件阅读、代码编辑、测试执行决策都是模型动作，也应该参与 policy learning。只返回最终 summary 文本也不够，因为它不能代表 OpenHands 实际采取的多轮 action。

## 4. Worker/OpenHands 必须返回的字段

SWE GRPO 正式训练中，Worker 在完成 `AgentJob` 时必须通过 `AgentJobCompleteRequest` 返回以下 typed 字段：

```text
AgentJobCompleteRequest.rollout_trace.response_ids
AgentJobCompleteRequest.rollout_trace.response_mask
AgentJobCompleteRequest.rollout_log_probs
```

字段要求：

| 字段 | 必选 | 要求 |
|---|---|---|
| `response_ids` | 是 | 所有需要参与 PPO/GRPO loss 的 assistant 生成 token id，按 OpenHands 多轮 LLM 调用顺序拼接。 |
| `response_mask` | 是 | 与 `response_ids` 等长；参与训练的 assistant token 为 1。当前建议只返回需要训练的 assistant tokens，因此 mask 可全为 1。 |
| `rollout_log_probs` | 是 | 与 `response_ids` 严格等长、逐 token 对齐的 old policy logprob。当前 UEnv SWE GRPO 链路将该字段作为必选字段处理。 |

`rollout_log_probs` 在当前阶段设为必选，原因如下：

1. SWE/OpenHands 多轮调用包含 tool call、环境 observation、特殊 token 和 stop/finish 语义，仅凭最终文本在 Adapter 侧无法可靠恢复逐 token old logprob。
2. 当前训练链路需要用 `rollout_log_probs` 与 `response_ids` 对齐，证明这些训练 token 来自真实 rollout，而不是 pad fallback 或重新 tokenize 的近似结果。

后续如果协议明确支持 decoupled mode，可以只要求 Worker 返回完整 `response_ids/response_mask` 和可还原每轮上下文，由 VeRL 训练侧重算 old logprob。但当前 SWE GRPO 联调阶段以“Worker 返回 token ids + token logprobs”为准。

## 5. 多轮 trace 建议结构

建议 OpenHands driver 在 `submit_result.json` 或 `llm_rollout_trace.json` 中同时保存 per-turn 明细和聚合字段。per-turn 明细便于排查哪一次模型调用丢字段，聚合字段用于 `CompleteAgentJob` typed 字段回填。

示例结构：

```json
{
  "turns": [
    {
      "turn_index": 0,
      "response_ids": [101, 102],
      "logprobs": [-0.1, -0.2],
      "finish_reason": "tool_calls"
    },
    {
      "turn_index": 1,
      "response_ids": [201, 202, 203],
      "logprobs": [-0.3, -0.4, -0.5],
      "finish_reason": "stop"
    }
  ],
  "rollout_trace": {
    "response_ids": [101, 102, 201, 202, 203],
    "response_mask": [1, 1, 1, 1, 1]
  },
  "rollout_log_probs": [-0.1, -0.2, -0.3, -0.4, -0.5]
}
```

长度约束：

```text
len(rollout_trace.response_ids) == len(rollout_trace.response_mask)
len(rollout_trace.response_ids) == len(rollout_log_probs)
```

## 6. 当前代码入口与缺口

当前 `uenv/integrations/openhands/uenv_runtime/agent_client.py::complete_agent_job()` 已有以下参数入口：

```text
parallel_mode
rollout_log_probs
response_ids
response_mask
```

`uenv/scripts/openhands/openhands_runner.py` 也会尝试从 `submit_result.json`、`llm_rollout_trace.json` 或 `trajectory_bundle.json` 读取 rollout trace 后上报。

### 2026-08-02 修复状态

缺口已在 OpenHands 侧补齐：

1. `integrations/openhands/uenv_runtime/llm_rollout.py` 的 `RolloutTraceCollector` 已从 Ark-only 扩展为 OpenAI-compatible / vLLM 可用：
   - 每次真实 LLM 调用强制 `logprobs=true`
   - 优先解析 provider 返回的 `token_id` / `token_id:N` / `uenv_response_ids`
   - Ark 仍走 `/tokenization`
   - OpenAI-compatible 可回退 `/tokenize` 或 LLM config 中的 HF `tokenizer`
2. `run_swebenchpro_official.py` 在 llm 模式 finalize 后写入 `llm_rollout_trace.json`，并合并进 `submit_result.json`
3. `openhands_runner.py` 读取上述字段回填 `CompleteAgentJob`；默认 `UENV_REQUIRE_SWE_RESPONSE_TRACE=1`，llm 模式缺 trace 时 fail-fast
4. 7142 DeepSeek vLLM 已启用 `--return-tokens-as-token-ids --max-logprobs 20`

实机 smith llm smoke（`/var/log/uenv/openhands-runs/rollout-trace-smoke-20260802-234803`）：

```text
turns=3
response_ids_len=6144
rollout_log_probs_len=6144
aligned=true
source=openai_chat_logprobs+token_ids
turn_id_sources=provider_token_ids × 3
```

VeRL 训练侧自建 vLLM 也需要能返回 token ids（推荐同样加 `--return-tokens-as-token-ids`），否则 OpenHands 只能依赖 `/tokenize` 或 config 里的 `tokenizer=`。

## 7. Adapter 当前处理

Adapter 已增加默认 fail-fast 保护：

```text
UENV_REQUIRE_SWE_RESPONSE_TRACE=1
```

当 `env_type=swe` 且 `EpisodeResult` 中没有 typed `rollout_trace.response_ids` 时，Adapter 会拒绝继续训练，避免静默使用 pad fallback。只做链路 smoke 时可以临时设置：

```text
UENV_REQUIRE_SWE_RESPONSE_TRACE=0
```

该模式只能验证链路，不应作为正式训练配置。正式训练中期望 Adapter 看到：

```text
response_source=rollout_trace
used_pad_fallback=false
response_ids_len > 0
rollout_log_probs_len == response_ids_len
```

## 8. 建议核验

Worker/OpenHands 侧可以按以下顺序核验：

1. 确认 OpenHands 每一次真实 LLM 调用都请求了 `logprobs=true`，并拿到了 completion token 的 logprob。
2. 确认每一次真实 LLM 调用都能拿到 response token ids；如果 provider 只返回 token string，需要使用同一 tokenizer 或 provider tokenization 接口恢复 token ids，并检查与 logprob 长度一致。
3. 确认 `submit_result.json` 或 `llm_rollout_trace.json` 写入 per-turn `response_ids/logprobs`，以及聚合后的 `rollout_trace.response_ids`、`rollout_trace.response_mask`、`rollout_log_probs`。
4. 确认 `openhands_runner.py` 读取后传给 `complete_agent_job()`。
5. 确认 Server 返回给 Adapter 的 `EpisodeResult.trajectory.steps[*].rollout_trace` 不为空，且 `EpisodeResult.rollout_log_probs` 不为空。

Adapter 侧可用以下结果文件复核：

```text
/data/ronghao/uenv/uenv-bridge/temp/logs/layer4_distributed/<RUN_ID>/agent-loop-results.jsonl
```

正式训练前，结果文件中至少应满足：

```text
status=completed
response_source=rollout_trace
used_pad_fallback=false
len(response_ids) > 0
rollout_log_probs_len == len(response_ids)
```
