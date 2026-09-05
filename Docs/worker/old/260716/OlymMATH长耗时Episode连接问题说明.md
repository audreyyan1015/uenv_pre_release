# OlymMATH 长耗时 Episode 连接问题说明

## 1. 背景

当前在使用 UEnv 链路对 OlymMATH 做全量测评：

```text
Adapter benchmark script
  -> Rust adapter core / Server
  -> Worker
  -> Adapter model gateway
  -> vLLM
```

本阶段不是后训练，而是通过 UEnv 框架调用 Worker 完成模型推理、reward 计算并返回 `EpisodeResult`。模型服务在 adapter 侧启动，Worker 通过 request 中的 model endpoint 访问该模型服务。

## 2. 结论摘要

当前问题的核心不是 adapter model gateway 或 vLLM 没有返回，而是 OlymMATH 在 thinking 模式下单个 episode 生成时间较长，Server -> Worker 的 gRPC/HTTP2 调用在等待 Worker 返回结果期间容易被取消或重置。

目前 adapter 侧已经验证：

```text
Worker 能成功访问 adapter model gateway
adapter model gateway 能成功访问 vLLM
vLLM 能返回 HTTP 200
长耗时 episode 更容易触发 h2 protocol error / CANCEL
降低 thinking budget 并去掉 reasoning 字段回传后，链路明显稳定
```

因此需要和 Server/Worker 侧重点确认：

```text
单 episode gRPC deadline / request timeout 是否过短
HTTP/2 keepalive / idle timeout 是否适合 80-100 秒以上长请求
EpisodeResult 回传体是否过大
Worker 长请求失败后，Server 是否能正确摘除、重连或恢复该 worker
```

如果后续要支持官方对齐口径的长输出配置，建议讨论是否将 Server -> Worker 的单 episode 允许时长增大到分钟级，并配套设置 keepalive 与失败恢复逻辑。

## 3. 现象

当使用较大的 thinking 配置时：

```text
max_tokens=32768
thinking_token_budget=16384
enable_thinking=true
```

单个 OlymMATH episode 的模型生成耗时通常在 80-100 秒左右。此时经常出现如下 server-worker gRPC/HTTP2 错误：

```text
dispatch_failed: code: 'Internal error',
message: "execute_episode_failed: code: 'The operation was cancelled',
message: \"h2 protocol error: http2 error\",
source: tonic::transport::Error(Transport, hyper::Error(Http2, Error { kind: Reset(StreamId(1), CANCEL, Remote) }))"
```

Server 侧会对同一个 episode 重试。如果连续多次失败，最终返回：

```text
episode ... exceeded max attempts (3)
```

从 adapter 侧看，这类样本会写成 `uenv_status=failed`。

## 4. 不是模型服务未被调用

该问题不是 model endpoint 不可达，也不是 vLLM 没有生成结果。

Adapter model gateway 日志显示，对应请求已经成功访问 vLLM，并返回 HTTP 200。例如：

```text
path=/v1/chat/completions
status_code=200
latency_ms≈80000-100000
error=""
```

也就是说，模型侧完成了生成。问题发生在 Worker 将结果通过 gRPC 返回 Server 的阶段，或者 Server 等待 Worker 返回结果的 HTTP/2 stream 被取消/重置。

## 5. 初步判断

目前的判断是：长耗时 episode 会放大 server-worker gRPC 长连接/stream 的稳定性问题。

可能原因包括：

1. Worker 调用模型期间，Server 到 Worker 的 gRPC 请求长时间处于等待状态。
2. HTTP/2 stream 在长时间无响应后被某一侧取消或重置。
3. thinking 输出较长时，Worker 返回的 `EpisodeResult` 可能包含较大的 `response_text`、`reasoning_content` 或其他中间字段，进一步增加回传压力。
4. 当前 Server/Worker 的 gRPC keepalive、timeout、max message size 或连接复用策略可能不适合 80-100 秒以上的单 episode 推理。

## 6. Adapter 侧已做的验证

### 6.1 保留完整 thinking 时不稳定

配置：

```text
max_tokens=32768
thinking_token_budget=16384
preserve_thinking=true
```

现象：

```text
单条生成约 80-100 秒
容易出现 h2 protocol error / CANCEL
多次 retry 后 episode failed
```

### 6.2 降低 thinking budget 后链路明显稳定

当前稳定运行配置：

```text
max_tokens=8192
thinking_token_budget=4096
enable_thinking=true
preserve_thinking=false
gateway strip_reasoning=true
```

效果：

```text
单条生成约 30-50 秒
目前已连续完成多条 episode
截至 2026-07-16 12:06，当前全量任务 15/400 completed，0 failed
```

当前运行目录：

```text
/data/ronghao/uenv/uenv-bridge/temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_thinking_strip_budget4096_full_20260716_120330
```

## 7. Gateway 对 reasoning / thinking 的处理方式

Adapter model gateway 位于 Worker 和 vLLM 之间，对 `/v1/chat/completions` 请求和响应做轻量改写。它的目标是：

```text
允许模型开启 thinking
控制 thinking token budget
决定是否把 reasoning 写入 content
决定是否把独立 reasoning 字段继续返回给 Worker
```

### 7.1 请求侧处理

Worker 请求 gateway 时，gateway 会根据启动参数改写发往 vLLM 的 JSON 请求：

```text
--enable-thinking
  -> chat_template_kwargs.enable_thinking=true

--disable-thinking
  -> chat_template_kwargs.enable_thinking=false

--preserve-thinking
  -> chat_template_kwargs.preserve_thinking=true

--thinking-token-budget 4096
  -> thinking_token_budget=4096
```

也就是说，thinking 是否开启、thinking token 预算是多少，是 adapter gateway 在转发到 vLLM 前补充到请求中的。

### 7.2 响应侧：独立字段与 content 的关系

对于 Qwen3 / vLLM reasoning 模式，模型响应中可能存在两类内容：

```text
message.content
  最终回答文本，通常是 Worker 做答案抽取和 reward 计算主要使用的字段。

message.reasoning / message.reasoning_content / message.reasoning_details
  推理过程或 thinking 内容，属于独立字段。
```

默认情况下，gateway 不会主动把独立 reasoning 字段写入 `content`。也就是说，如果只开启 thinking，但不开启 `--preserve-thinking`，则 Worker 正常情况下看到的 `content` 仍然是最终回答，不包含 `<think>...</think>`。

### 7.3 `--preserve-thinking` 的作用

如果 gateway 启动时打开：

```text
--preserve-thinking
```

gateway 会在响应侧读取 `message.reasoning`，并将其合并到 `message.content` 前面，格式类似：

```text
<think>
reasoning text
</think>

final answer content
```

这个模式适合需要在最终输出里观察模型思考过程的调试场景。但它会显著增大 `content` 长度，如果 Worker 再把完整 `response_text` 放入 `EpisodeResult` 回传，就会增加 Server/Worker gRPC 返回体大小和长连接压力。

### 7.4 `--strip-reasoning` 的作用

如果 gateway 启动时打开：

```text
--strip-reasoning
```

gateway 会从响应 JSON 中移除以下独立字段：

```text
message.reasoning
message.reasoning_content
message.reasoning_details
```

需要注意的是，`--strip-reasoning` 只删除独立字段，不会解析并删除已经写入 `content` 里的 `<think>...</think>` 文本。因此：

```text
preserve_thinking=false + strip_reasoning=true
  -> Worker 收到最终 content，不收到独立 reasoning 字段。

preserve_thinking=true + strip_reasoning=true
  -> Worker 收到带 <think>...</think> 的 content，但不收到独立 reasoning 字段。

preserve_thinking=true + strip_reasoning=false
  -> Worker 既收到带 <think>...</think> 的 content，也可能收到独立 reasoning 字段，返回体最大。
```

### 7.5 当前推荐配置

当前 OlymMATH UEnv 全量评测采用的是：

```text
enable_thinking=true
preserve_thinking=false
strip_reasoning=true
thinking_token_budget=4096
```

这代表：

```text
模型仍然可以使用 thinking 能力生成答案
thinking 过程不写入 content
独立 reasoning 字段不返回给 Worker
Worker 主要基于最终 content 做答案抽取和 reward 计算
```

这组配置可以减少 `EpisodeResult` 中的文本体积，降低 Server/Worker 长耗时 gRPC 回传压力。若后续需要保存完整 reasoning，建议不要放在同步 `EpisodeResult` 中回传，而是由 Worker 或 gateway 侧写入独立日志或对象存储，并在 `EpisodeResult` 中只返回日志路径或 trace id。

## 8. 希望 Server / Worker 侧协助确认的问题

### 8.1 是否存在 gRPC 请求超时或 idle timeout

请确认 Server 调用 Worker 的 gRPC client 是否配置了：

```text
request timeout
connect timeout
HTTP/2 keepalive interval
HTTP/2 keepalive timeout
idle timeout
```

如果存在 60 秒或 90 秒级别的默认值，可能会影响 OlymMATH 这类长耗时 episode。

### 8.2 是否需要增大连接时长

建议讨论是否将 Server -> Worker 的单 episode 调用允许时间提高到至少：

```text
5-10 分钟
```

理由：

```text
OlymMATH 官方对齐口径可能需要 max_tokens=32768
thinking 模式下单条推理可能超过 100 秒
后续更难 benchmark 或更大模型可能更慢
```

这里不建议只做“无限增大超时”这一项。更稳妥的做法是同时设置：

```text
request deadline: 覆盖最长单 episode 推理时间，并留出回传和重试余量
HTTP/2 keepalive interval: 在长时间无 response body 的情况下维持连接活性
HTTP/2 keepalive timeout: 避免对端短暂无响应时过早断开
worker-side execution timeout: 与 server-side deadline 对齐，避免一侧已取消另一侧仍在执行
```

如果先做工程验证，可以先把单 episode deadline 设到 10 分钟，并观察 32768 max_tokens / 16384 thinking budget 是否还能稳定复现 h2 reset。

### 8.3 是否需要开启 keepalive

如果当前 Server -> Worker 是一个长时间等待的 unary gRPC 调用，建议考虑开启 HTTP/2 keepalive，避免长时间没有 response frame 时连接被中间层或对端认为空闲。

需要确认：

```text
Server gRPC client keepalive
Worker gRPC server keepalive
是否允许 keepalive without active streams
中间网络 / proxy / LB 是否有 idle timeout
```

### 8.4 是否需要调整 max message size

如果 Worker 返回的 `EpisodeResult` 包含完整模型输出、reasoning、trajectory 等字段，需要确认：

```text
gRPC max receiving message size
gRPC max sending message size
EpisodeResult 中 response_text / trajectory / metadata 的大小
```

对于 reasoning 类模型，建议 Worker 侧不要默认把完整 reasoning 写入必须回传字段。可以只返回：

```text
final response content
reward
必要的 trajectory 字段
finish_reason
token ids / logprob 等训练确实需要的字段
```

完整 reasoning 如需观测，建议走独立日志/对象存储，而不是放在同步 gRPC 返回结果中。

### 8.5 Worker 长任务失败后的连接恢复

曾观察到某次长耗时 episode 失败后，后续样本快速出现：

```text
dispatch_failed: transport error
```

且这些请求没有打到 adapter gateway。这说明某些情况下 Worker gRPC channel 可能进入不可用状态，Server 仍继续调度到该 worker，导致后续 episode 快速失败。

建议确认：

```text
Worker transport error 后是否会被标记为 temporarily unavailable
Server 是否会重建 gRPC channel
失败 worker 是否有熔断 / 冷却 / 重新注册机制
Worker 侧是否在长请求异常后关闭了 gRPC server 或连接
```

### 8.6 建议 Server / Worker 侧增加的日志

为了确认问题发生点，建议在 Server/Worker 侧按 `request_id` 或 `episode_id` 打印以下日志：

```text
Server 发起 Worker 调用的时间
Worker 收到 episode 的时间
Worker 开始请求 model endpoint 的时间
Worker 收到 model endpoint 响应的时间
Worker 开始计算 reward 的时间
Worker 准备返回 EpisodeResult 的时间
Server 收到 EpisodeResult 的时间
失败时的 gRPC status code / h2 reset reason / retry attempt
返回 EpisodeResult 的字节大小
```

如果 Server 侧日志显示 Worker 已返回但 Server 接收失败，重点查 message size 和 Server 接收配置。如果 Worker 侧日志显示模型已经返回但 gRPC 返回阶段失败，重点查 Worker server keepalive、deadline 和 response size。如果 Worker 根本没有收到后续请求，重点查 Server 侧 worker channel 的恢复和熔断策略。

## 9. 建议的下一步

1. Server / Worker 侧确认 gRPC timeout、keepalive、max message size 配置。
2. 用同一个问题样本复现：

```text
OlymMATH-HARD-57-EN
```

该样本在大 budget 下多次触发 h2 reset，在 `thinking_token_budget=4096` 下可以完成。

3. 对比两组配置：

```text
不稳定配置：
max_tokens=32768
thinking_token_budget=16384
preserve_thinking=false
strip_reasoning=true

较稳定配置：
max_tokens=8192
thinking_token_budget=4096
preserve_thinking=false
strip_reasoning=true
```

4. 如果希望支持官方长 token 口径，需要优先增强 Server -> Worker 长耗时 episode 的连接稳定性。

## 10. 当前 Adapter 侧临时方案

Adapter 侧当前采用以下规避方案继续推进评测：

```text
gateway strip reasoning 字段
降低 thinking_token_budget 到 4096
降低 max_tokens 到 8192
开启 resume，失败后只跳过 completed 样本
```

该方案可以先跑通全量 UEnv 测评，但它不是最终官方对齐口径。若后续需要恢复 `max_tokens=32768`、`thinking_token_budget=16384`，需要 Server / Worker 侧配合解决长耗时 gRPC 连接稳定性问题。
