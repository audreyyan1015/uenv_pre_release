# SWE-bench-Pro OpenHands 参数与 Workspace 优先级排查建议

- 日期：2026-07-21
- 背景：当前 UEnv 接入 SWE-bench-Pro 全量评测已运行 100+ 条样本，仍为 `0 resolved`；Worker 侧认为可能与评测参数未对齐公开配置有关。
- 关联文档：
  - [20260721-Qwen3.6-35B-A3B-SWE-Pro-OpenHands公开评测参数对比.md](./20260721-Qwen3.6-35B-A3B-SWE-Pro-OpenHands公开评测参数对比.md)
  - [20260720-SWE-bench-Pro-OpenHands-Prompt与可观测性修复记录.md](../debug_log/20260720-SWE-bench-Pro-OpenHands-Prompt与可观测性修复记录.md)

---

## 1. 当前判断

我的判断是：**参数问题确实存在，但不应先把 `0 resolved` 全部归因于参数；当前更需要优先核验 OpenHands 的 workspace/repo 映射是否正确。**

原因是二者造成的失败形态不同：

| 问题类型 | 典型表现 | 当前是否观测到 | 优先级 |
|---|---|---:|---:|
| 参数不足，例如 context、max tokens、max iterations 不够 | `ContextWindowExceededError`、提前达到最大轮数、patch 不完整 | 是 | P1 |
| workspace/repo 映射错误 | Agent 在错误仓库里搜索/编辑，最终测试在另一个仓库执行，`git_diff_bytes` 多数为 0 | 是 | **P0** |

参数会影响通过率高低；但如果 workspace 错了，任务在原则上就很难 solved。也就是说，参数调大之前，必须先证明模型确实在当前 instance 对应的 repo 里操作。

---

## 2. 参数问题确实需要后续调整

Worker 文档中提到的参数差异是合理的。当前 UEnv 配置与公开 SWE/OpenHands/Qwen 参考配置存在明显差距：

| 配置项 | 当前值 | 风险 |
|---|---:|---|
| vLLM `max_model_len` | `65536` | SWE 多轮 agent history 容易超过上下文 |
| `MAX_TOKENS` | `8192` | 单轮输出可能不够，且与输入 history 叠加后容易触顶 |
| `THINKING_TOKEN_BUDGET` | `4096` | thinking 空间有限 |
| `MAX_ITERATIONS` | `50` | 复杂 Pro 任务可能没来得及完成 |
| `temperature/top_p` | `0.0/1.0` | 与 Qwen Model Card 的 Pro 口径不同，但与部分 OpenHands 论文默认口径一致 |

当前日志里已经出现多条类似错误：

```text
ContextWindowExceededError:
This model's maximum context length is 65536 tokens.
However, you requested 8192 output tokens and your prompt contains at least 57345 input tokens.
```

因此后续参数上可以考虑：

```text
max_model_len >= 131072
MAX_ITERATIONS=100
MAX_TOKENS=8192 或 16384
THINKING_TOKEN_BUDGET=4096 或 8192
```

但这一步应该在 workspace 验证通过之后再做，否则无法判断新的结果是参数影响还是环境映射错误。

---

## 3. 为什么当前更怀疑 Workspace/Repo 映射

从当前 SWE-bench-Pro 运行结果看，截至 124 条样本：

| 指标 | 数值 |
|---|---:|
| 已写入结果 | `124 / 731` |
| `completed` | `82` |
| `failed` | `42` |
| `resolved=True` | `0` |
| 有 `trajectory_id` | `82` |
| `git_diff_bytes=0` | `78` 条 completed |
| 非零 diff | 4 条，仍全部未通过 |

同时，远端 OpenHands run 目录中观察到一个更强的异常：**非 OpenLibrary 任务的 stdout 里出现 `/app/openlibrary/...` 搜索路径。**

示例现象：

```text
instance_qutebrowser__qutebrowser-...
```

对应问题和测试是 qutebrowser，但 OpenHands stdout 中出现：

```text
/app/openlibrary/catalog/utils/tests/test_catalog_utils.py
/app/openlibrary/tests/fastapi/test_monthly_logins.py
/app/openlibrary/core/...
```

类似现象也出现在 ansible、navidrome、element-web 等非 OpenLibrary instance 中。

这类现象不是 `temperature`、`max_tokens` 或 `thinking_budget` 能直接解释的。更像是：

1. Worker 没有把当前 instance 对应 repo 正确 checkout/mount 到 `/app`；
2. OpenHands agent 操作的 `/app` 和最终测试执行的 `/app` 不是同一个目录；
3. 多个任务之间复用了旧 workspace，导致 `/app` 残留 OpenLibrary；
4. run_id、instance_id、repo、workspace_dir 在 Worker 内部发生错配。

如果该问题存在，即使模型输出能力足够，也会出现：

```text
题目要求修 qutebrowser
Agent 实际查看 /app/openlibrary
最终测试跑 qutebrowser
git diff 为空或不是有效 patch
resolved=False
```

---

## 4. 建议先做的最小验证

建议 Worker 侧先不要直接全量改大参数重跑，而是选 3 个不同语言/仓库的样本做 workspace 自检：

| 样本类型 | 示例 |
|---|---|
| Python | qutebrowser |
| Go | flipt / teleport |
| JS/TS | NodeBB / element-web |

在每个 OpenHands job 启动后、模型执行前，Worker 侧强制打印以下信息，并写入对应 run 目录：

```bash
pwd
git -C /app rev-parse --show-toplevel
git -C /app rev-parse HEAD
git -C /app remote -v
ls -la /app | head -50
find /app -maxdepth 2 -type d | head -80
```

同时打印当前 job 元数据：

```text
run_id
episode_id
instance_id
repo
base_commit
workspace_dir
llm_config_path
model_endpoint
```

然后核对：

| 核验项 | 期望 |
|---|---|
| `instance_id` | 与 Adapter request 一致 |
| `repo` | 与当前 instance 对应，例如 qutebrowser 任务应是 `qutebrowser/qutebrowser` |
| `/app` 内容 | 与当前 repo 匹配，不应残留其他 repo |
| `/app` HEAD | 与当前 `base_commit` 对齐，或至少是 Worker catalog 中对应 commit |
| Agent 操作目录 | 与测试执行目录、git diff 采集目录一致 |
| `llm_config_path` | 使用 request 中传入的配置，而不是旧默认配置 |

---

## 5. 验证后的决策

### 5.1 如果 Workspace 验证失败

优先修复 Worker/OpenHands 侧 workspace 管理：

1. 每个 instance 启动前强制清理或重新挂载工作区；
2. 确保 `/app` 指向当前 instance 对应 repo；
3. 确保 Agent 操作、测试执行、git diff 采集使用同一个 repo 根目录；
4. 防止不同 job 之间复用污染 workspace；
5. 再重新小样本测试，确认非 OpenLibrary 样本不再出现 `/app/openlibrary/...`。

此时不建议先调大参数，因为参数调整无法修复“在错误仓库里操作”的问题。

### 5.2 如果 Workspace 验证通过

再进入参数对齐实验：

1. 将 `max_model_len` 提升到 `131072` 或更高；
2. 将 `MAX_ITERATIONS` 提到 `100`；
3. 保持 `temperature=0.0/top_p=1.0` 作为 OpenHands 生态口径；
4. 对比 `MAX_TOKENS=8192/16384` 与 `THINKING_TOKEN_BUDGET=4096/8192`；
5. 统计 `ContextWindowExceededError`、`git_diff_bytes>0`、`tests_passed/tests_total`、`resolved` 的变化。

---

## 6. 给 Worker 侧的结论

当前参数确实和 Qwen 官方 / OpenHands 公开评测口径存在差距，特别是 `65K context`、`max_iterations=50`、`MAX_TOKENS=8192` 会造成上下文超长和复杂任务未完成，这部分后续需要调参。

但当前 `0 resolved` 不能只归因于参数。日志中已经看到多个非 OpenLibrary 任务的 OpenHands stdout 出现 `/app/openlibrary/...`，这更像 workspace/repo 映射或复用污染问题。建议 Worker 侧先对少量 qutebrowser、flipt、NodeBB 等样本做启动前 workspace 自检，确认 `/app` 的 repo、HEAD、remote、instance_id、base_commit、测试目录和 git diff 采集目录全部一致。只有先证明 Agent 确实在正确仓库里操作，后续调大 context、tokens、iterations 才能得到可信的对比结果。
