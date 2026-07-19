# OlymMATH UEnv 评测 22 条 Episode 失败 Server/Worker 核验说明

## 1. 背景

本轮 OlymMATH 通过 UEnv 完成全量 400 条样本评测，链路如下：

```text
Adapter -> Adapter Core / Server -> Worker math plugin -> Model Gateway -> vLLM -> Worker reward -> Adapter
```

本次结果目录：

```text
/data/ronghao/uenv/uenv-bridge/temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_thinking_max32768_budget16384_full_20260718_223005/
```

核心配置：

| 配置 | 值 |
|---|---|
| 模型 | `Qwen/Qwen3.6-35B-A3B` |
| Adapter Core / Server | `8.130.75.157:8088` |
| Worker | `worker-7143-pro`，endpoint `219.147.100.43:28888` |
| Model Gateway | `http://10.10.20.142:18094/v1` |
| 数据集 | OlymMATH EN/ZH EASY/HARD 全量 400 题 |
| Thinking | 开启 |
| `MAX_TOKENS` | 32768 |
| `THINKING_TOKEN_BUDGET` | 16384 |
| `TIMEOUT_SECONDS` | 7200 |

总体结果：

| requests | results | completed | failed | reward accuracy | parse rate |
|---:|---:|---:|---:|---:|---:|
| 400 | 400 | 378 | 22 | 0.6175 | 0.8950 |

22 条 failed 并不是模型答案错误或 OlymMATH 判分错误，而是 Server/Worker 派发阶段失败：

```text
error_code=5001
error_message=episode ... exceeded max attempts (3)
```

## 2. 失败样本列表

22 条失败样本全部来自 EN-EASY 的连续区间：

| qid | request_id | subject | elapsed_ms | Adapter 侧错误 |
|---|---|---|---:|---|
| `OlymMATH-EASY-64-EN` | `olymmath-OlymMATH-EASY-64-EN-45605aa4` | `Combinatorics` | 63251 | `episode olymmath-OlymMATH-EASY-64-EN-45605aa4 exceeded max attempts (3)` |
| `OlymMATH-EASY-65-EN` | `olymmath-OlymMATH-EASY-65-EN-78c81518` | `Algebra` | 129 | `episode olymmath-OlymMATH-EASY-65-EN-78c81518 exceeded max attempts (3)` |
| `OlymMATH-EASY-66-EN` | `olymmath-OlymMATH-EASY-66-EN-87a82079` | `Combinatorics` | 127 | `episode olymmath-OlymMATH-EASY-66-EN-87a82079 exceeded max attempts (3)` |
| `OlymMATH-EASY-67-EN` | `olymmath-OlymMATH-EASY-67-EN-3249d7fa` | `Geometry` | 135 | `episode olymmath-OlymMATH-EASY-67-EN-3249d7fa exceeded max attempts (3)` |
| `OlymMATH-EASY-68-EN` | `olymmath-OlymMATH-EASY-68-EN-21391b2f` | `Combinatorics` | 125 | `episode olymmath-OlymMATH-EASY-68-EN-21391b2f exceeded max attempts (3)` |
| `OlymMATH-EASY-69-EN` | `olymmath-OlymMATH-EASY-69-EN-da0bed41` | `Geometry` | 130 | `episode olymmath-OlymMATH-EASY-69-EN-da0bed41 exceeded max attempts (3)` |
| `OlymMATH-EASY-70-EN` | `olymmath-OlymMATH-EASY-70-EN-ff7a8ff9` | `Geometry` | 135 | `episode olymmath-OlymMATH-EASY-70-EN-ff7a8ff9 exceeded max attempts (3)` |
| `OlymMATH-EASY-71-EN` | `olymmath-OlymMATH-EASY-71-EN-da5a7bf7` | `Geometry` | 130 | `episode olymmath-OlymMATH-EASY-71-EN-da5a7bf7 exceeded max attempts (3)` |
| `OlymMATH-EASY-72-EN` | `olymmath-OlymMATH-EASY-72-EN-24b85488` | `Geometry` | 133 | `episode olymmath-OlymMATH-EASY-72-EN-24b85488 exceeded max attempts (3)` |
| `OlymMATH-EASY-73-EN` | `olymmath-OlymMATH-EASY-73-EN-9d0da7fc` | `Algebra` | 135 | `episode olymmath-OlymMATH-EASY-73-EN-9d0da7fc exceeded max attempts (3)` |
| `OlymMATH-EASY-74-EN` | `olymmath-OlymMATH-EASY-74-EN-2bd28ecc` | `Combinatorics` | 133 | `episode olymmath-OlymMATH-EASY-74-EN-2bd28ecc exceeded max attempts (3)` |
| `OlymMATH-EASY-75-EN` | `olymmath-OlymMATH-EASY-75-EN-17746bc9` | `Geometry` | 128 | `episode olymmath-OlymMATH-EASY-75-EN-17746bc9 exceeded max attempts (3)` |
| `OlymMATH-EASY-76-EN` | `olymmath-OlymMATH-EASY-76-EN-e84639ec` | `Algebra` | 132 | `episode olymmath-OlymMATH-EASY-76-EN-e84639ec exceeded max attempts (3)` |
| `OlymMATH-EASY-77-EN` | `olymmath-OlymMATH-EASY-77-EN-e1b6b977` | `Algebra` | 130 | `episode olymmath-OlymMATH-EASY-77-EN-e1b6b977 exceeded max attempts (3)` |
| `OlymMATH-EASY-78-EN` | `olymmath-OlymMATH-EASY-78-EN-a25632f7` | `Combinatorics` | 131 | `episode olymmath-OlymMATH-EASY-78-EN-a25632f7 exceeded max attempts (3)` |
| `OlymMATH-EASY-79-EN` | `olymmath-OlymMATH-EASY-79-EN-72046719` | `Combinatorics` | 130 | `episode olymmath-OlymMATH-EASY-79-EN-72046719 exceeded max attempts (3)` |
| `OlymMATH-EASY-80-EN` | `olymmath-OlymMATH-EASY-80-EN-c3cb7eb1` | `Algebra` | 129 | `episode olymmath-OlymMATH-EASY-80-EN-c3cb7eb1 exceeded max attempts (3)` |
| `OlymMATH-EASY-81-EN` | `olymmath-OlymMATH-EASY-81-EN-2afb9ace` | `Geometry` | 134 | `episode olymmath-OlymMATH-EASY-81-EN-2afb9ace exceeded max attempts (3)` |
| `OlymMATH-EASY-82-EN` | `olymmath-OlymMATH-EASY-82-EN-24c311d1` | `Algebra` | 126 | `episode olymmath-OlymMATH-EASY-82-EN-24c311d1 exceeded max attempts (3)` |
| `OlymMATH-EASY-83-EN` | `olymmath-OlymMATH-EASY-83-EN-996cfcab` | `Geometry` | 130 | `episode olymmath-OlymMATH-EASY-83-EN-996cfcab exceeded max attempts (3)` |
| `OlymMATH-EASY-84-EN` | `olymmath-OlymMATH-EASY-84-EN-fa08a176` | `Algebra` | 131 | `episode olymmath-OlymMATH-EASY-84-EN-fa08a176 exceeded max attempts (3)` |
| `OlymMATH-EASY-85-EN` | `olymmath-OlymMATH-EASY-85-EN-d13ef47e` | `Geometry` | 130 | `episode olymmath-OlymMATH-EASY-85-EN-d13ef47e exceeded max attempts (3)` |

## 3. Server 日志证据

Server 侧日志来源：

```bash
journalctl --no-pager -u uenv-frontend-add-adapter \
  --since "2026-07-18 23:55:00" \
  --until "2026-07-19 00:40:00"
```

### 3.1 第 64 条首先出现 HTTP2 broken pipe

第 64 条 `olymmath-OlymMATH-EASY-64-EN-45605aa4` 的生命周期如下：

```text
2026-07-18 23:56:59 CST attempt 1 dispatch 到 worker-7143-pro
2026-07-18 23:56:59 - 23:57:59 CST 持续收到 stream_report running
2026-07-18 23:58:02 CST attempt 1 失败
```

Server 日志中的失败原因为：

```text
dispatch_failed:
code: 'Unknown error',
message: "h2 protocol error: error reading a body from connection",
source: hyper::Error(Body, Error { kind: Io(Custom { kind: BrokenPipe, error: "stream closed because of a broken pipe" }) })
```

随后第 64 条的 attempt 2 和 attempt 3 在几十毫秒内失败：

```text
attempt 2: dispatch_failed: transport error
attempt 3: dispatch_failed: transport error
next_attempt=4
```

Server 代码在 attempt 超过 `max_attempts=3` 后返回：

```text
episode ... exceeded max attempts (3)
```

### 3.2 第 65-85 条快速消耗完重试次数

第 65-85 条均出现同样现象：

```text
attempt 1: dispatch_failed: transport error
attempt 2: dispatch_failed: transport error
attempt 3: dispatch_failed: transport error
```

聚合统计：

| 项 | 数量 |
|---|---:|
| failed episode | 22 |
| dispatch attempt | 66 |
| failed attempt | 66 |
| HTTP2 broken pipe | 1 |
| generic transport error | 65 |

这说明失败是一个连续的 Server/Worker transport 故障窗口，而不是单个题目内容或判分逻辑问题。

### 3.3 Worker 在故障后发生重注册，但一度被拒绝

在第 85 条最后一次失败后，Server 侧出现 Worker 重注册记录：

```text
2026-07-18 23:58:05 CST worker registered worker-7143-pro endpoint=219.147.100.43:28888
2026-07-18 23:58:05 CST worker_reregister_rejected_active_lease active_load=1
2026-07-18 23:58:05 CST control_plane_register accepted=false
```

这表示 Worker 已经尝试重新注册，但 Server 认为同名 Worker 仍有 active lease，因此拒绝新注册。该设计用于避免新 Worker 接管旧 dispatch lease 后造成结果串线，但在本次故障中导致一段时间内 Worker 未能恢复为可派发状态。

### 3.4 第 86 条说明恢复点

第 86 条 `olymmath-OlymMATH-EASY-86-EN-19d3c2b6` 在本地结果中是 completed，但耗时约 2189s。Server 日志显示：

```text
2026-07-18 23:58:05 CST attempt 1: dispatch_failed: transport error
2026-07-18 23:58:05 CST attempt 2: dispatch_failed: transport error
2026-07-19 00:33:03 CST worker registered accepted=true
2026-07-19 00:33:04 CST attempt 3 dispatch
2026-07-19 00:34:34 CST episode_completed
```

这进一步说明，第 64-85 条失败不是数据问题；第 86 条之所以成功，是因为它的最后一次 attempt 等到了 Worker 重新 accepted 后才派发成功。

## 4. 当前判断

当前证据指向：

```text
Worker/Server gRPC 或 HTTP2 transport 在 2026-07-18 23:58 CST 左右断开；
Server 随后对同一 worker 的重注册先因 active lease 拒绝；
第 64-85 条在 Worker 未恢复可派发前快速消耗完 3 次 attempt；
Adapter 最终收到 exceeded max attempts (3)。
```

因此，这 22 条 failed 不应归因为：

1. OlymMATH 题目难度或题面内容。
2. 模型输出不正确。
3. Adapter model gateway 无法访问 vLLM。
4. OlymMATH reward / answer parser 判错。

更合理的归因是：

```text
Server -> Worker 派发链路的临时连接故障 + Worker 重注册 active lease 处理窗口。
```

## 5. 希望 Server/Worker 侧核验的问题

请 Worker 侧重点查看 `2026-07-18 23:56:59 - 23:58:05 CST` 附近日志：

1. `worker-7143-pro` 是否发生进程重启、panic、OOM、容器重启或网络重连。
2. Worker gRPC server 是否记录 HTTP2 reset、broken pipe、CANCEL、transport error。
3. 第 64 条是否已经进入 Worker 执行阶段，是否已经调用 model gateway。
4. 如果第 64 条已进入模型生成，是否因为长生成、连接超时或客户端断开导致 response body broken pipe。
5. Worker 重注册时为什么 Server 仍认为存在 `active_load=1`。

请 Server 侧重点核验：

1. `transport error` 是否应该立即消耗 episode attempt。
2. 当 Worker 发生 broken pipe / transport error 后，Server 是否应临时标记该 worker unavailable，而不是继续快速派发下一条 episode。
3. `worker_reregister_rejected_active_lease` 后，active lease 的释放或过期时间为什么接近 35 分钟。
4. 是否需要对 transport error 增加 backoff，避免 22 条 episode 在 2-3 秒内连续耗尽所有 attempt。
5. 是否需要把 attempt-level 的失败原因、worker_id、dispatch_lease_id 返回到 EpisodeResult，方便 Adapter 侧区分模型失败和派发失败。

## 6. 建议修复方向

短期建议：

1. 对 `dispatch_failed: transport error` 增加退避重试，不要在同一 Worker 不健康窗口内立即连续派发。
2. transport error 后将对应 Worker 标记为 temporarily unavailable，等待 heartbeat / register accepted 后再恢复调度。
3. Worker 重注册被 `active_lease` 拒绝时，Server 应记录旧 lease 的 episode_id、lease_id 和预计释放时间。
4. Adapter/Server 返回结果中补充 attempt 明细，至少包括 `attempt_id`、`worker_id`、`dispatch_lease_id`、`dispatch_error`。

中期建议：

1. 对长耗时 episode 的 streaming/report_result 通道做更稳健的 keepalive 和 deadline 配置。
2. 明确区分 `model_generation_failed`、`worker_execution_failed`、`server_dispatch_failed` 三类失败。
3. 对 active lease 增加可观测指标，例如当前 active lease 数、最老 active lease 年龄、draining worker 数。
4. 支持对 `server_dispatch_failed` 类失败样本做 Adapter 侧 resume/retry，而不把它们计入模型能力指标。

## 7. 结论

这 22 条 OlymMATH failed 的直接原因是 Server/Worker 派发链路在 23:58 CST 附近发生 HTTP2/gRPC transport 故障，并伴随 Worker 重注册被 active lease 拒绝。失败样本集中连续出现，且第 86 条在 Worker 重新 accepted 后成功完成，说明问题属于服务稳定性与重试策略问题，不属于模型能力或 OlymMATH 判分问题。
