# SWE-bench-Pro UEnv 当前测试结果

> 统计时间：2026-07-20 08:35
> 运行状态：仍在运行中
> 测试目标：使用 UEnv 链路对 SWE-bench-Pro 全量测试集进行基准模型评测，观察当前模型在 Agent 型程序修复任务上的 resolved 表现、失败类型与链路稳定性。

## 1. 测试配置

当前运行目录：

```text
/data/ronghao/uenv/uenv-bridge/temp/benchmarks/swebenchpro/qwen3_6_35b_a3b_uenv_full_thinking8192_budget4096_20260719_205350
```

主要输入输出文件：

| 文件 | 说明 |
|---|---|
| `run.log` | 主运行日志，包含 tqdm 进度 |
| `uenv_requests.jsonl` | Adapter 发送到 UEnv 的 EpisodeRequest |
| `uenv_results.jsonl` | UEnv 返回的 EpisodeResult 整理结果 |

启动命令对应的核心参数如下：

| 参数 | 当前值 |
|---|---|
| 数据集 | `/data/ronghao/uenv/uenv-bridge/data/benchmarks/swebenchpro/test.jsonl` |
| 总样本数 | 731 |
| Adapter Core endpoint | `8.130.75.157:8088` |
| Model endpoint | `http://127.0.0.1:18194/v1` |
| Model name | `Qwen/Qwen3.6-35B-A3B` |
| Env package | `swe-bench-pro@0.3.4` |
| Agent bridge | `uenv-agent-openhands@1.0.0` |
| Agent pool | `openhands-default` |
| Driver entrypoint | `run_swebenchpro_official.py` |
| Workspace dir | `/app` |
| LLM config | `/root/UEnv/config/openhands-llm-qwen3-thinking-max-token-8192.json` |
| Batch size | 1 |
| Max tokens | 8192 |
| Thinking token budget | 4096 |
| Temperature | 0.0 |
| Top-p | 1.0 |
| Max iterations | 50 |
| Episode timeout | 7200 秒 |
| Client timeout | 7800 秒 |

当前进程仍在运行，运行时长约 11 小时 42 分钟。

## 2. 当前进度

截至 2026-07-20 08:35：

| 项目 | 数量 |
|---|---:|
| 总样本数 | 731 |
| 已发送 request | 179 |
| 已收到 result | 178 |
| 正在处理 | 1 |
| 已完成进度 | 178 / 731 |
| tqdm 进度 | 24% |
| 预计剩余时间 | 约 52 小时 31 分钟 |

## 3. 当前结果汇总

| 指标 | 数量 |
|---|---:|
| `uenv_status=completed` | 144 |
| `uenv_status=failed` | 34 |
| `resolved=true` | 0 |
| `resolved=false` | 178 |
| `uenv_reward=1.0` | 0 |
| `uenv_reward=0.0` | 178 |
| `trajectory_id` 为空 | 178 |

当前 resolved 通过率：

```text
0 / 178 = 0.00%
```

当前 reward accuracy：

```text
0 / 178 = 0.00%
```

## 4. 已处理样本分布

### 4.1 按 repo 统计

| Repo | 已返回结果数 |
|---|---:|
| `ansible/ansible` | 24 |
| `internetarchive/openlibrary` | 21 |
| `qutebrowser/qutebrowser` | 20 |
| `gravitational/teleport` | 19 |
| `future-architect/vuls` | 19 |
| `flipt-io/flipt` | 19 |
| `element-hq/element-web` | 15 |
| `navidrome/navidrome` | 14 |
| `NodeBB/NodeBB` | 13 |
| `protonmail/webclients` | 9 |
| `tutao/tutanota` | 5 |

### 4.2 按语言统计

| 语言 | 已返回结果数 |
|---|---:|
| Go | 71 |
| Python | 65 |
| JavaScript | 37 |
| TypeScript | 5 |

## 5. 失败类型统计

34 条 `failed` 中，错误类型如下：

| 失败类型 | 数量 | 说明 |
|---|---:|---|
| `ContextWindowExceededError` | 26 | OpenHands / LiteLLM 调用模型时，上下文长度超过 vLLM `65536` token 限制 |
| timeout | 2 | HTTP / socket 读取超时 |
| other | 6 | 其他运行错误，需要结合 Worker / OpenHands 轨迹继续排查 |

### 5.1 ContextWindowExceededError 示例

代表错误信息：

```text
This model's maximum context length is 65536 tokens.
However, you requested 8192 output tokens and your prompt contains at least 57345 input tokens,
for a total of at least 65537 tokens.
```

该错误说明：部分 SWE-bench-Pro 样本在 OpenHands 多轮交互后，累计 prompt 已接近模型上下文上限；即使 `max_tokens` 已降到 8192，仍然可能超过 `max_model_len=65536`。

已观察到该类错误出现在以下 repo：

| Repo | 示例 instance |
|---|---|
| `NodeBB/NodeBB` | `instance_NodeBB__NodeBB-04998908ba6721d64eba79ae3b65a351dcfbc5b5-vnan` |
| `internetarchive/openlibrary` | `instance_internetarchive__openlibrary-111347e9583372e8ef91c82e0612ea437ae3a9c9-v2d9a6c849c60ed19fd0858ce9e40b7cc8e097e59` |
| `gravitational/teleport` | `instance_gravitational__teleport-e6d86299a855687b21970504fbf06f52a8f80c74-vce94f93ad1030e3136852817f2423c1b3ac37bc4` |
| `navidrome/navidrome` | `instance_navidrome__navidrome-27875ba2dd1673ddf8affca526b0664c12c3b98b` |

### 5.2 Timeout 示例

当前 timeout 类错误主要表现为 Python `urllib` / socket 读取超时：

```text
TimeoutError: timed out
```

这类错误可能与单个 OpenHands 任务执行时间过长、远端服务响应超时、模型请求时间过长或 Worker/Server 连接等待有关。

### 5.3 completed 但 unresolved

当前 144 条样本返回 `uenv_status=completed`，但全部 `resolved=false`。

这说明这些样本从 UEnv 角度完成了 episode，但最终 SWE 判分没有通过。由于当前 `trajectory_id` 为空，Adapter 侧暂时不能直接判断它们属于以下哪种情况：

1. OpenHands 没有生成任何 patch。
2. OpenHands 生成了 patch，但改错文件。
3. patch 修改了正确文件，但测试仍未通过。
4. 测试执行或结果收集存在问题。

结合之前 Worker 侧诊断报告中 qutebrowser 和 flipt 的样例，当前更可能存在“模型在正确仓库中没有定位到正确修改点”的情况，但需要具体 trajectory / git diff 进一步确认。

## 6. 耗时统计

| 指标 | 当前值 |
|---|---:|
| 平均每样本耗时 | 235.86 秒 |
| 最短样本耗时 | 35.04 秒 |
| 最长样本耗时 | 1394.54 秒 |

当前 tqdm 末尾显示：

```text
178/731 [11:39:44<52:30:54, 341.87s/it]
```

说明后续样本平均耗时可能比当前全局平均值更高。若保持该速度，全量完成还需要较长时间。

## 7. 当前判断

1. 当前 SWE-bench-Pro UEnv 全量测试仍在正常推进，没有整体中断。
2. 目前结果全为 `resolved=false`，通过率暂时为 0。
3. `failed` 主要由上下文窗口超限导致；这与 SWE-bench-Pro + OpenHands 多轮工具调用会不断累积上下文有关。
4. `completed` 样本全部 unresolved，说明仅降低 `max_tokens` 不能解决当前效果问题。
5. 当前 Adapter 侧缺少 `trajectory_id` / `git_diff` / `modified_files` / `tests_summary`，因此无法对 144 条 completed unresolved 做更细粒度归因。
6. 当前配置已经使用 `/app` 作为 workspace，不再是旧的 `/workspace` 配置问题。

## 8. 后续建议

短期建议继续保留当前测试，至少收集更多失败分布；但如果目标是尽快定位 0 resolved 的原因，应优先推动 Worker / Server 返回以下字段：

| 字段 | 用途 |
|---|---|
| `trajectory_id` / `trajectory_uri` | 定位 OpenHands 完整执行轨迹 |
| `git_diff` | 判断是否生成 patch |
| `modified_files` | 判断是否改错路径 |
| `tests_summary` | 判断测试失败类型 |
| `agent_id` / `agent_job_id` | 关联具体 OpenHands AgentJob |

拿到这些信息后，才能区分模型能力问题、OpenHands 工具调用问题、路径选择问题和判分链路问题。

对于 `ContextWindowExceededError`，后续可考虑：

1. 降低 `max_tokens`。
2. 降低 `max_iterations`。
3. 调整 OpenHands 历史截断策略。
4. 使用更大上下文的模型 endpoint。
5. 针对 SWE-bench-Pro 单独设计更短的 prompt / instruction。

## 9. 小结

当前 SWE-bench-Pro UEnv 全量测试已经返回 178 条结果，但暂未出现 resolved 样本。链路层面仍在运行，主要失败来自上下文窗口超限；completed 样本全部 unresolved，需要 Worker / Server 回填 OpenHands 轨迹与 patch 信息后才能继续定位。
