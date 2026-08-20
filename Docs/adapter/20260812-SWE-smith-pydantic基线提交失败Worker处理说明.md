# SWE-smith pydantic 样本基线提交失败 Worker 处理说明

## 1. 问题现象

本轮 SWE-smith GRPO 训练在 run `verl_swesmith_grpo_train_20260812_090844` 中断。

训练本身已经完成到 `global_step=49`，随后在第 50 个 rollout batch 中，有 4 条 episode 在 Worker 侧失败。失败样本均为：

```text
pydantic__pydantic.acb0f10f.combine_module__htbb7vzl
```

Worker 返回的核心错误是：

```text
benchmark baseline commit failed (code 1)
```

具体失败信息中包含：

```text
don't commit to branch ... Failed
end-of-file-fixer ... files were modified by this hook
trailing-whitespace ... files were modified by this hook
Lint ... /bin/sh: 1: uv: not found
Typecheck ... Executable `uv` not found
```

## 2. 影响

这类失败发生在 Worker 执行 SWE-smith 环境准备或基线提交阶段，episode 尚未形成有效 OpenHands 轨迹。

Adapter 侧已启用 `zero_reward` 失败兜底策略，失败 episode 会被转换为 0 reward。当前 Adapter 已补齐失败兜底的 pad token、mask 和 logprob 字段，避免单条 Worker 失败直接打断 VeRL 后处理。

但从训练数据质量看，如果某类样本持续在 Worker 初始化阶段失败，它会稳定贡献 0 reward，不能产生有效训练信号。因此仍需要 Worker 侧修复根因。

## 3. Worker 侧建议修改

建议 Worker 侧重点核验 pydantic SWE-smith 镜像或运行时环境中的基线提交流程：

1. 确认 `pydantic__pydantic.acb0f10f.combine_module__htbb7vzl` 对应镜像内是否缺少 `uv`。
2. 确认 baseline commit 阶段是否会触发项目自身的 pre-commit hooks。
3. 如果 baseline commit 只是为了固定环境状态，建议避免触发业务仓库 hooks，或在该阶段显式绕过 hooks。
4. 如果 SWE-smith 官方流程要求运行相关工具，建议在镜像或运行时中补齐 `uv` 等必要依赖。
5. 确认 `end-of-file-fixer`、`trailing-whitespace` 这类会修改文件的 hook 不会污染 baseline commit 逻辑。

## 4. Adapter 侧已处理内容

Adapter 侧已补齐失败 episode 的 fallback 结果：

```text
response_ids      = [pad_token_id]
response_mask     = [0]
response_logprobs = [0.0]
reward            = 0.0
```

其中 `response_mask=[0]` 表示该 fallback token 不参与有效训练更新，`response_logprobs=[0.0]` 仅用于满足 VeRL 在开启 rollout logprobs 时的张量拼接要求。
