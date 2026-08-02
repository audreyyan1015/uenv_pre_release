# UEnv 压测发现并修复的问题

## worker/agent 失联后已分配任务停滞

- 现象：一台 worker 失联后，健康 worker 空闲但后续任务不再派发；已领取任务可能一直滞留到 4 小时 episode 超时。
- 原因：旧逻辑只在选择新 agent 时排除 stale agent，不会回收 `in_flight` job 及其 reservation。
- 修复：在健康 agent 的 heartbeat/poll 时检查过期分配；超过心跳超时和 grace period 后，将原任务重新放回 pending、释放 reservation/load，并以新的 `run_id` 重新进入原 pool。过期分配的迟到结果会被拒绝。
- 可观测性：增加 `agent_job_stale_reclaimed_requeued` 日志和 `stale_reclaimed_jobs` 指标。
- 验证状态：定向测试和 `swe_agent_orchestration` 已通过；尚未部署到生产服务。

## runtime gateway 同步评测触发 HTTP 超时

- 现象：61 个 SWE episode 在 `POST /runtime/v1/sessions/{id}/submit` 的读响应阶段报 `TimeoutError: timed out`。
- 原因：UEnv runtime gateway 同步等待测试、评分和 artifact 完成后才回复；客户端单次 HTTP 等待为 600 秒，超时后还会重试。
- 修复：gateway 提交接口立即返回 `running`，后台执行评测；客户端每 2 秒查询一次，直到拿到最终结果（总等待上限默认 6 小时）。
- 生效条件：需要重新编译并部署 `uenv-worker` 到两台 worker，再重新启动压测。
