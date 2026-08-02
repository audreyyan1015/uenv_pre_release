# UEnv 大规模压测待办

1. [~] 修复 SWE-bench Pro 在 worker 失联后的批次停滞：代码已完成，待真实故障注入验收。

   - 2026-07-28 实现记录（隔离工作树 `feature/scale-bench-10k-workers`，未部署/重启生产服务）：
     - UEnv：`AgentJobQueue` 会在任一健康 agent 的 heartbeat 或 poll 时检查 stale lease；默认 `heartbeat_timeout_secs=30` 加 `agent_job_reclaim_grace_secs=15` 后，对 SQLite 旧 `(agent_id, run_id)` 做条件迁移 `leased -> pending`，生成新的 `run_id` 后重新入原 pool。旧 agent 的迟到完成因 run_id 不匹配被拒绝，不会覆盖新 lease；同时释放旧 agent 的 reservation/load。
     - 可观测性：增加 `agent_job_stale_reclaimed_requeued` 日志和累计 `stale_reclaimed_jobs`，隔离 SWE 配置启用 loopback admin HTTP（gRPC 8099 的相邻端口），`/agents` 会返回该计数及 pending/running/in-flight 快照。
     - 驱动：`swebench_pro_pressure.py` 改为有界滑动 batch 窗口（默认一轮 worker capacity，可由 `--max-in-flight-batches` 覆盖），持续写入 `driver-progress.jsonl`，明确区分 `rpc_dispatched` 与 `resolved`；ExecuteBatch 没有 accept-only 回执，`server_accepted` 明确标为未知，需以 admin 快照为准。
     - 主机级证据：已加入独立 `CentralHostWatchdog`（默认 5 秒）。它使用独立、指纹校验的 SSH 连接采集每个 worker 的 load/meminfo、CPU/内存/IO PSI、cgroup memory/pids events、磁盘、Docker 容器/最近事件和 kernel journal tail，并持续追加到隔离 server 的 `{server_run}/host-watchdog.jsonl`；结束后回传本地 artifact。`.129` 本轮的 45 个 Exited SWE 容器已删除；另 4 个 `Dead` 对象确认是缺失 RW layer 的损坏 Docker metadata，停止 Docker 后仅删除这 4 个精确 `/var/lib/docker/containers/<id>` 目录并重启 Docker，现 `docker ps -a` 已为空。
     - 定向验证已通过：`stale_agent_job_is_released_and_reassigned_with_a_fresh_lease`、`stale_agent_lease_requeue_requires_the_old_lease_and_issues_a_new_one`、全量 `swe_agent_orchestration`（7/7）以及 `cargo check -p uenv-adapter-core`。远程稳定工具链未安装 rustfmt，因此未执行 rustfmt 格式化。

   - 现象与证据：2026-07-28 的干净重跑 `swebench-pro-pressure-sync-a8-c2-20260728-140338-c036c791` 在 64 workers、`agents_per_node=8`、`concurrency=2`、2560 Episodes 下完成 1539 个 Episode（`attempt_failed=0`）后停滞。worker `8.145.51.129` 在负载约 73、46 个容器、available memory 约 2.9 GiB 时 SSH/TCP 22 超时；另一台 `8.130.65.20` 的 32 workers 仍正常心跳且 `load=0`，但 1539 之后不再获得新任务。生产 `/usr/local/bin/uenv-adapter-core` 始终正常，问题位于隔离 SWE run。

   - 压测驱动待办（`uenv_stress/scale/swebench_pro_pressure.py` 及相关 CLI）：
     - 明确记录并落盘每个时刻的 `driver_submitted`、`server_accepted`、`pending`、`in_flight`、`completed`、`retrying`；当前只看 `episode_completed` 无法证明剩余 1021 个 Episode 是未提交还是已进入 server 后卡住。
     - 审查 `--episode-batch-size 128` 的窗口推进：不能让少量未返回 future 阻塞后续 Episode 的提交；采用可取消的滑动窗口，保持健康 agent/worker 有可领取任务。
     - 在 worker 失联、资源阈值触发或无完成超过阈值时，采集 driver 栈、server `/status`/`/agents` 快照、worker 容器数、available memory、CPU/IO PSI 和日志，而不是只等待 batch timeout。
     - 重启后 preflight 必须检查 `docker ps -a`；只删除已经核对为 `uenv-swe-instance-*` 的 Exited/Dead 残留容器，不能只检查运行中的容器。

   - UEnv server 待办（`uenv-server` 的 `AgentRegistry` / `AgentJobQueue` / episode 编排）：
     - 现有心跳超时只会在后续 `pick_agent` 时排除 stale agent；已领取的 `AgentJobQueue.in_flight` 不会因 worker/agent stale 自动 `abandon + requeue`。`abandon` 目前仅由编排层 deadline 调用，`default_timeout_secs=14400` 会使失联任务和 reservation 最多滞留 4 小时。
     - 实现独立于 Episode timeout 的 stale-worker recovery：心跳超时后停止新派发；经过短 grace period，原子释放 stale agent 的 reservation/admission permit，以新 `attempt` 和新 dispatch lease 将未完成任务重新入队，保留幂等与迟到结果隔离。
     - 暴露并持久化可诊断指标：按 worker/agent 的 stale 状态、reservation、pending/in-flight job、lease、已重派次数；隔离 server 的 admin HTTP 端口必须可配置并可从压测采集（本轮 8099 是 gRPC，不能直接查询 `/agents`）。

   - 验收：
     - 人为使一台 worker 在运行中失联后，健康 worker 能在 heartbeat timeout + grace period 内接手任务；不等待 14400 秒，不重复执行同一 lease。
     - `driver_submitted=2560`、server 的 pending/in-flight/completed/retry 计数可对账；健康 worker 空闲时不得存在未解释的未提交或不可派发任务。
     - 先以 64 registered workers、`agents_per_node=8`、`concurrency=1`、1600 Episodes（跨过 1500）做控制变量验证；c1 的 `default_timeout_secs`/`batch_timeout` 应在 review 后相应提高。通过后再恢复 c2/2560，必要时增加第 3 台 worker 或加入每机容器/资源准入上限。
