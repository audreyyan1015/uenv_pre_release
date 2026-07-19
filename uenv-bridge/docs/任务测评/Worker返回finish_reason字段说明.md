# Worker 返回 `finish_reason` 字段说明

> 面向对象：UEnv Worker 协作组
> 背景：PubMedQA、SciTab 等 UEnv 全量评测需要判断模型输出是否因为 `max_tokens` 被截断
> 建议优先级：高

## 1. 问题背景

当前 Adapter 侧可以拿到 Worker 返回的 `EpisodeResult`，其中包含 `response_text`、reward、trajectory 等信息。但在 PubMedQA / SciTab thinking 模式评测中，Adapter 侧无法直接知道模型生成是否被 `max_tokens` 截断。

目前只能通过输出文本是否包含 `</think>` 来间接判断：

```text
没有 </think> -> 可能被截断或未完整收束
```

这种判断不够可靠。更准确的方式是 Worker 在调用 OpenAI-compatible 模型 endpoint 后，把上游模型响应里的 `finish_reason` 写入 `EpisodeResult`。

## 2. 字段来源

Worker 调用模型时通常访问：

```text
POST /v1/chat/completions
```

OpenAI-compatible 响应中，每个 choice 会包含 `finish_reason`：

```json
{
  "choices": [
    {
      "index": 0,
      "message": {
        "role": "assistant",
        "content": "..."
      },
      "finish_reason": "length",
      "stop_reason": null
    }
  ],
  "usage": {
    "prompt_tokens": 1234,
    "completion_tokens": 1024,
    "total_tokens": 2258
  }
}
```

常见取值：

| 字段 | 常见值 | 含义 |
|---|---|---|
| `finish_reason` | `stop` | 正常遇到 stop token / stop sequence，生成完整结束 |
| `finish_reason` | `length` | 命中 `max_tokens` 或模型长度限制，输出被截断 |
| `finish_reason` | `tool_calls` | 模型转入工具调用 |
| `finish_reason` | `content_filter` | 输出被内容过滤中断 |
| `stop_reason` | 任意字符串或 null | vLLM 等后端可能额外返回的停止原因 |

对当前 benchmark 来说，最关键的是：

```text
finish_reason == "length"
```

这可以直接作为输出被截断的证据。

## 3. 建议放置位置

建议不要把 `finish_reason` 放在 `EpisodeResult` 顶层，而是放在每个 `StepRecord.info` 中。

原因：

1. `finish_reason` 描述的是一次模型生成的结束原因。
2. 一个 episode 未来可能有多轮、多步，每一步都可能调用一次模型。
3. 放在 `StepRecord.info` 中可以自然支持多步 episode。

推荐结构：

```json
{
  "request_id": "pubmedqa-xxx",
  "status": "completed",
  "trajectory": {
    "steps": [
      {
        "step_index": 0,
        "action": "...",
        "reward": 1.0,
        "terminated": true,
        "truncated": true,
        "info": {
          "response_text": "...",
          "finish_reason": "length",
          "stop_reason": "",
          "prompt_tokens": "1234",
          "completion_tokens": "1024",
          "total_tokens": "2258"
        }
      }
    ],
    "total_reward": 1.0,
    "total_steps": 1
  },
  "summary": {
    "total_reward": 1.0,
    "total_steps": 1,
    "terminate_reason": "completed"
  }
}
```

注意：当前 Python Adapter 的 `StepRecord.info` 类型是 `dict[str, str]`，因此建议 Worker 写入字符串形式：

```json
{
  "finish_reason": "length",
  "completion_tokens": "1024"
}
```

## 4. Worker 侧处理要求

Worker 在调用模型后，建议按以下逻辑写回：

```python
choice = response["choices"][0]
message = choice.get("message") or {}
usage = response.get("usage") or {}

response_text = message.get("content", "")
finish_reason = choice.get("finish_reason") or ""
stop_reason = choice.get("stop_reason") or ""

step.info["response_text"] = response_text
step.info["finish_reason"] = str(finish_reason)
step.info["stop_reason"] = str(stop_reason) if stop_reason is not None else ""
step.info["prompt_tokens"] = str(usage.get("prompt_tokens", ""))
step.info["completion_tokens"] = str(usage.get("completion_tokens", ""))
step.info["total_tokens"] = str(usage.get("total_tokens", ""))

if finish_reason == "length":
    step.truncated = True
```

建议保留已有字段：

| 字段 | 是否必需 | 说明 |
|---|---|---|
| `response_text` | 必需 | Adapter 和 benchmark driver 用它解析最终答案 |
| `finish_reason` | 必需 | 判断是否被 `max_tokens` 截断 |
| `stop_reason` | 可选 | vLLM / SGLang 等后端可能提供更细停止原因 |
| `prompt_tokens` | 推荐 | 便于分析输入长度是否过长 |
| `completion_tokens` | 推荐 | 便于判断是否打满 `max_tokens` |
| `total_tokens` | 推荐 | 便于统计整体 token 消耗 |
| `truncated` | 推荐 | 当 `finish_reason == "length"` 时设置为 true |

## 5. Adapter 侧消费方式

Adapter 侧无需改通信结构，只需要从 `EpisodeResult.trajectory.steps[-1].info` 读取：

```python
step = result.trajectory.steps[-1]
finish_reason = step.info.get("finish_reason", "")
completion_tokens = step.info.get("completion_tokens", "")
is_truncated = step.truncated or finish_reason == "length"
```

benchmark 输出中可以增加：

```json
{
  "response_text": "...",
  "finish_reason": "length",
  "completion_tokens": "1024",
  "truncated": true
}
```

这样 PubMedQA / SciTab 文档中的截断统计可以从真实 `finish_reason` 得到，而不是依赖 `</think>` 间接推断。

## 6. 验收标准

Worker 改动完成后，任选一条 PubMedQA 或 SciTab 样本运行 UEnv 链路，Adapter 侧 `uenv_results.jsonl` 应能看到：

```json
{
  "finish_reason": "stop",
  "completion_tokens": "128",
  "truncated": false
}
```

当刻意设置较小 `max_tokens`，例如 `MAX_TOKENS=16`，应能看到：

```json
{
  "finish_reason": "length",
  "completion_tokens": "16",
  "truncated": true
}
```

如果上述字段能够随 `EpisodeResult` 返回，Adapter 侧即可精确统计：

```text
真实截断条数 = count(finish_reason == "length" or truncated == true)
真实截断比例 = 真实截断条数 / 样本总数
```

## 7. 兼容性说明

该方案不要求变更 `EpisodeResult` 顶层 schema，也不要求新增 gRPC 字段。只是在已有 `StepRecord.info` 中补充若干 key，因此对现有 Adapter、Server、Worker 的协议兼容性影响较小。

如果未来 Server / Worker 定义了强类型 `GenerationMetadata`，可以再把 `finish_reason`、token usage、model version 等字段从 `info` 迁移到强类型结构中。当前阶段建议先用 `StepRecord.info` 快速打通。
