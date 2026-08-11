# SWE-smith 长测试列表导致 Worker 命令过长说明

> 记录日期：2026-08-11
> 面向对象：Worker 侧
> 关联 run：`verl_swesmith_grpo_train_20260811_120115`
> 结论摘要：本轮中断不是模型能力问题，也不是 gRPC 4 MiB 问题，而是 Worker 在执行某个 SWE-smith 样本时构造的测试命令过长，触发了 `docker exec spawn failed: Argument list too long`。

## 1. 问题说明

本轮训练在第一轮 rollout 后中断，前 4 条 episode 已完成，后 4 条集中失败在同一个样本：

```text
pyparsing__pyparsing.533adf47.combine_file__dsi7jva0
```

Worker 返回的直接错误是：

```text
gateway HTTP 500: docker exec spawn failed: Argument list too long (os error 7)
```

这表示不是测试断言失败，而是测试命令在启动阶段就过长，`docker exec` 无法把整条命令作为参数启动。

## 2. 证据

该样本的测试集合很大：

```text
fail_to_pass_count = 476
pass_to_pass_count = 1315
```

同一 batch 中后 4 条结果都返回相同错误，且没有产生有效 trajectory：

```text
status = failed
response_ids = []
rollout_log_probs_len = 0
used_pad_fallback = true
```

这说明失败点在 Worker 执行测试命令之前，而不是模型生成阶段。

## 3. 需要 Worker 侧处理的方向

请 Worker 侧把长测试列表改成更稳妥的输入方式，避免把大量 pytest node id 直接拼进单条 `docker exec` 参数。

建议方向：

1. 将测试列表写入文件，再由测试入口读取。
2. 对超长测试集合分批执行。
3. 保留统一的失败回传格式，方便 Adapter 在单条 episode 失败时做容错。

如果 Worker 侧不做这类处理，后续类似样本仍可能在“启动测试”阶段直接失败，进而影响整个 GRPO step。
