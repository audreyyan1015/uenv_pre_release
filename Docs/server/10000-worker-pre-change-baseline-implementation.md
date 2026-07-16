# 万 worker 改造前基准具体实施手册

更新时间：2026-07-16

适用仓库：`/home/uenv`

配套总方案：`Docs/server/10000-worker-scale-gap-list.md`
文档状态：可执行草案

## 1. 文档目的

本文档用于在万 worker 规模化改造开始前，建立一套可重复、可比较、可审计的 UEnv 控制面性能基准。

改造前基准不要求旧实现必须通过 10000 worker。它需要回答：

1. 当前代码最多能稳定维持多少 worker。
2. 注册、心跳、调度、dispatch、结果上报分别在什么规模出现性能拐点。
3. 首个瓶颈是 CPU、锁、内存、FD、网络、连接建立、日志还是存储。
4. 过载、批量掉线、重连和 Server 重启时，系统以什么方式失败。
5. 改造后相对于同一基准提升了多少，是否只是把瓶颈转移到了其他环节。

本文档只证明控制面容量，不证明完整万卡训练能力。模型推理、训练通信、GPU/NPU 拓扑、checkpoint、训练数据读取等数据面能力需要单独验收。

## 2. 当前起点与已知边界

截至 2026-07-16，当前实机状态如下：

| 项目 | 当前状态 | 对基准的影响 |
|---|---|---|
| Server 源码 | `/home/uenv` HEAD `229700b4bd963242a358daa568b6ed4bb2e0dee7` | 只能作为候选基线，尚未冻结。 |
| 工作树 | `uenv-bridge/core/src/core.rs` 存在未提交代码改动 | 未处理前禁止打基准标签。 |
| Server 二进制 | SHA256 `31b448dd0f04359c56d68de3e81463d7bb1e1cf9f1c7faa43f000c52766fe014` | 必须写入每次运行 manifest。 |
| Server 部署 | `uenv-server.service` 已恢复为 systemd 唯一托管，`NRestarts=0` | 部署基线已恢复，但 unit 还需要纳入仓库。 |
| 资源上限 | `nofile=1048576` | 可进入连接规模测试。 |
| 基准机规格 | 8 CPU、32 GiB 内存、Linux 6.8 | 所有对比必须保持同规格。 |
| native 路径 | 单条 `math` ExecuteBatch E2E 已通过 | 可作为 S00 冒烟。 |
| SWE 路径 | `swe-bench-pro@0.3.4` gold E2E 已通过 | 可作为真实 SWE 抽样，不用于 10k 容器压测。 |
| LLM Agent | 7142 LLM gateway `backend_ready=false` | 属于模型数据面阻塞，不阻塞 worker 控制面基准。 |
| 143 Worker | 实机 worker 可用，但代码不是主仓库最新版本 | 只做真实链路抽样，不作为 synthetic 10k 发生器。 |

现有 `uenv-server/stress_test` 脚本只能复用 protobuf 和 mock 逻辑，不能直接作为正式基准：

- `stress_test.py` 主要覆盖 50 个 mock worker、约 60 秒。
- `stress_test_1k.py` 实际为 64 个 worker、512 并发，不是 1000 worker。
- 旧脚本将整个 batch 延迟除以 sample 数，不能代表单 episode 端到端延迟。
- 旧脚本缺少 Server CPU、RSS、FD、锁等待、连接建立速率和容量一致性证明。

## 3. 基准完成定义

只有同时满足以下条件，才能宣布“改造前基准已建立”：

1. 基准源码、二进制、配置、systemd unit 和机器参数全部冻结并可追溯。
2. 压测发生器完成自校验，确认发生器 CPU、网络和 event loop 不是瓶颈。
3. `100/500/1000/2000/5000/7500/10000` 规模阶梯均有结果；旧实现无法继续时必须保存失败现场。
4. 注册、心跳、调度、ReportResult、组合负载和至少一种故障场景有原始指标。
5. 每个有效档位至少重复 3 次，关键结果离散度满足要求。
6. 报告能够给出最大稳定规模、性能拐点、首要瓶颈和资源斜率。
7. 所有命令、配置、日志、指标和报告均归档，可在同规格机器复现。

基准完成不等于系统达到生产万 worker。旧实现停在 3000 或 5000 worker 也可以形成有效基准。

## 4. Gate 0：冻结版本和部署

### 4.1 处理工作树

在 `/home/uenv` 执行：

```bash
cd /home/uenv
git status --short
git diff -- uenv-bridge/core/src/core.rs
git diff --check
```

必须确认 `uenv-bridge/core/src/core.rs` 的改动属于哪个版本：

- 属于当前功能：完成测试并提交。
- 不属于当前功能：不能直接删除，应先明确归属并从干净 worktree 构建基准。

文档、`__pycache__`、trajectory 运行数据不进入基准代码提交。

### 4.2 将部署文件纳入仓库

建议新增：

```text
deploy/systemd/uenv-server.service
```

仓库版本必须与 `/etc/systemd/system/uenv-server.service` 一致，并至少包含：

- 显式 `UENV_CONFIG_PATH`。
- 二进制和配置文件启动前检查。
- `Restart=on-failure`。
- `StartLimitIntervalSec=60`、`StartLimitBurst=5`。
- `KillMode=control-group`、`TimeoutStopSec=60`。
- `LimitNOFILE=1048576`。

### 4.3 建立基准标签

代码和部署文件提交后建立标签：

```bash
git tag -a baseline/10k-pre-20260716 -m "pre-change 10k worker control-plane baseline"
```

标签名称中的日期以实际冻结日为准。

### 4.4 生成 baseline manifest

每次压测必须生成 `manifest.json`，至少包含：

```json
{
  "baseline_id": "10k-pre-20260716",
  "git_sha": "<full sha>",
  "git_describe": "<tag or describe>",
  "git_diff_sha256": "<empty for clean tree>",
  "adapter_binary_sha256": "<sha256>",
  "server_config_sha256": "<sha256>",
  "systemd_unit_sha256": "<sha256>",
  "protocol_schema_sha256": "<sha256>",
  "host": {
    "cpu_count": 8,
    "memory_bytes": 32687362048,
    "kernel": "Linux 6.8.0-124-generic",
    "nofile": 1048576
  },
  "scenario": "S03-heartbeat-steady-5000",
  "started_at": "<RFC3339>",
  "load_generators": ["<host-a>", "<host-b>"]
}
```

建议实现：

```text
scripts/baseline/freeze-manifest.sh
```

脚本必须在发现非文档代码改动时失败退出。

## 5. 测试拓扑

### 5.1 正式拓扑

```text
                        ┌─────────────────────────┐
                        │ 指标与结果节点           │
                        │ Prometheus / artifacts  │
                        └───────────┬─────────────┘
                                    │ scrape / collect
┌──────────────────────┐            │             ┌──────────────────────┐
│ LoadGen-1            │            ▼             │ LoadGen-3            │
│ 2500 virtual workers ├──────► Benchmark Server ◄┤ 2500 virtual workers │
└──────────────────────┘        8C / 32 GiB        └──────────────────────┘
┌──────────────────────┐        server only        ┌──────────────────────┐
│ LoadGen-2            ├───────────────────────────┤ LoadGen-4            │
│ 2500 virtual workers │                           │ 2500 virtual workers │
└──────────────────────┘                           └──────────────────────┘
```

要求：

- 正式 10k 测试至少使用 4 个发生器，每台默认承载 2500 个虚拟 worker。
- 发生器与 Server 不部署在同一台机器。
- Server 机器不运行编译、日志分析或发生器任务。
- 所有机器使用 NTP/chrony 对时，误差控制在 100 ms 内。
- 发生器侧 CPU 长时间高于 70% 时，该轮结果无效，需要增加发生器节点。
- 发生器侧网络达到链路 70% 时，该轮结果无效，需要扩容或降低 payload。

### 5.2 机器数量与同机约束

正式建立“万 worker 改造前基准”时，最低资源口径为 5 台机器：

| 角色 | 数量 | 默认负载 | 说明 |
|---|---:|---:|---|
| Benchmark Server | 1 | 只运行 `uenv-server.service` / `uenv-adapter-core` | 例如当前 `/home/uenv` 主机。 |
| LoadGen | 4 | 每台 2500 virtual workers | 合计 10000 worker，发生器自身不得饱和。 |

推荐资源口径为 6 台机器：

| 角色 | 数量 | 说明 |
|---|---:|---|
| Benchmark Server | 1 | 独占 Server 负载。 |
| LoadGen | 4 | 每台 2500 virtual workers。 |
| Observe/Collect | 1 | Prometheus、日志收集、结果归档和报告生成。 |

同机运行规则：

- `Server + LoadGen` 同机只允许用于工具预演和链路冒烟，不能作为正式 10k 基准。
- 同机预演建议限制在 `500-2000` worker，用于验证注册、心跳、dispatch、ReportResult 和指标链路。
- 同机预演必须标记为 `local_smoke_only=true`，报告中不能写成 Server 容量结论。
- 同机预演不能使用 `127.0.0.1` 结果推导真实多机网络成本。
- 同机出现 p99/p999 抖动时，优先视为 Server 与 LoadGen 争抢 CPU、FD、端口或网络栈队列导致，不能直接归因于 Server。

资源紧张时，可以先做 3 台机器的受限预演：

| 角色 | 数量 | 建议规模 | 结果用途 |
|---|---:|---:|---|
| Benchmark Server | 1 | server only | 验证部署、指标和服务稳定性。 |
| LoadGen | 2 | 每台 2500-5000 workers | 找到发生器和 Server 的早期拐点。 |

3 台机器结果只能称为“受限资源下的参考压测”，不能替代正式改造前基准。

推荐执行阶梯：

```text
阶段 0：Server + 同机 LoadGen，500-2000 worker，验证压测工具和协议链路。
阶段 1：Server 独立 + 1 台 LoadGen，1000-2500 worker，验证远端发生器与采集链路。
阶段 2：Server 独立 + 2 台 LoadGen，5000 worker，确认发生器未饱和。
阶段 3：Server 独立 + 4 台 LoadGen，10000 worker，产出正式基准。
```

### 5.3 没有独立 Server 时

允许在当前 8C/32 GiB Server 上做正式基准，但必须满足：

1. 安排维护窗口并停止真实训练提交。
2. 负载发生器仍放在其他机器。
3. 压测前确认真实 `active_episodes=0`、AgentJob 队列为空。
4. 归档压测开始前的 service、worker 和 Agent 状态。
5. 不允许在真实流量旁边并发运行完整 10k 压测。

可在独立端口启动 benchmark unit 做小规模工具调试，但调试结果不能作为正式基准：

```text
gRPC:      18088
trajectory:18077
admin:     15052
data dir:  /home/uenv/benchmark-data/<run-id>
log dir:   /home/uenv/benchmark-logs/<run-id>
```

## 6. 需要新增的仓库结构

```text
tools/scale-bench/
  Cargo.toml
  src/
    main.rs
    coordinator.rs
    virtual_worker.rs
    mock_worker_service.rs
    submitter.rs
    heartbeat.rs
    result_reporter.rs
    fault.rs
    metrics.rs
    manifest.rs
  tests/
    protocol_smoke.rs
    coordinator_limits.rs

config/benchmark/
  10k-pre-common.yaml
  scenarios/
    s00-smoke.yaml
    s01-registry-ladder.yaml
    s02-register-storm.yaml
    s03-heartbeat-steady.yaml
    s04-native-combined.yaml
    s05-result-payload.yaml
    s06-overload.yaml
    s07-drop-reconnect.yaml
    s08-server-restart.yaml
    s09-cancel-storm.yaml
    s10-admin-query.yaml
    s11-soak.yaml

scripts/baseline/
  preflight.sh
  freeze-manifest.sh
  collect-server.sh
  collect-loadgen.sh
  run-scenario.sh
  stop-scenario.sh
  package-results.sh

Docs/server/baselines/
  README.md
  <baseline-id>/
    manifest.json
    scenario.yaml
    commands.md
    report.md
    raw/
```

## 7. Rust 压测工具设计

### 7.1 为什么使用 Rust

不能为 10000 个 worker 启动 10000 个 Python 进程。正式工具应使用 Tokio task 表示 worker 身份，以避免发生器自身进程数和解释器开销污染结果。

### 7.2 组件职责

| 组件 | 职责 |
|---|---|
| `coordinator` | 分配 worker ID 范围、统一开始时间、场景阶段和停止信号。 |
| `virtual_worker` | 保存 worker ID、epoch、load、generation 和生命周期。 |
| `mock_worker_service` | 接收 Server 的 DispatchEpisode/CancelEpisode，按配置延迟完成。 |
| `heartbeat` | 按当前协议发送心跳，支持固定周期、抖动、暂停和恢复。 |
| `result_reporter` | 生成不同大小的 EpisodeResult 并调用 ReportResult。 |
| `submitter` | 生成 unary batch、stream 和 async submit 负载。 |
| `fault` | 注入掉线、延迟、重连、乱序心跳、重复结果和旧 epoch。 |
| `metrics` | 输出发生器自身 CPU、task、RPC、延迟和错误，证明发生器未饱和。 |
| `manifest` | 写出运行配置、版本和主机信息。 |

### 7.3 两种 worker 仿真模式

#### registry-mode

- 多个 worker ID 可以共享少量 mock dispatch endpoint。
- 主要测量 registry、心跳、调度扫描、锁竞争和容量一致性。
- 适合 30k/100k registry-only 测试。
- 不能用于证明 10000 个真实 endpoint/连接的网络成本。

#### connection-mode

- 每个虚拟 worker 维持独立身份和心跳流。
- 按当前实现模拟 ReportResult、dispatch/cancel 的建连行为。
- endpoint 可以按 shard 分组，但必须记录实际 TCP 连接数和建连速率。
- 用于 10k 稳态连接、短连接风暴和 Server 重启重连测试。

报告必须标明使用哪种模式，禁止将 registry-mode 的结果描述为“10000 条真实连接通过”。

### 7.4 worker 身份与时序

- worker ID：`bench-<run-id>-<shard>-<index>`。
- 默认容量：每 worker `max_concurrent=1`；另设 `4/8/16` 容量档位。
- 默认心跳：5 秒。
- 稳态心跳抖动：`±20%`，避免发生器人为形成同相尖峰。
- 注册风暴场景关闭抖动，按 60 秒窗口均匀或突发注册。
- 所有 episode ID、lease、result idempotency key 必须全局唯一。
- 故障场景需要保留旧 epoch/generation，用于发送可识别的过期消息。

## 8. 测量专用代码改动

改造前允许增加“只测量、不改业务语义”的 instrumentation commit。该提交必须作为后续改造的共同基线，并验证性能开销。

### 8.1 必须增加的指标

建议指标名：

```text
uenv_worker_registry_total{state}
uenv_worker_capacity_total{state}
uenv_worker_load_total{state}
uenv_scheduler_reserve_seconds
uenv_scheduler_lock_wait_seconds
uenv_scheduler_candidates_scanned
uenv_scheduler_no_candidate_total
uenv_admission_capacity
uenv_admission_used
uenv_admission_wait_seconds
uenv_admission_reject_total{reason}
uenv_rpc_requests_total{method,code}
uenv_rpc_latency_seconds{method}
uenv_connections_active{direction}
uenv_connections_created_total{direction}
uenv_active_episodes
uenv_pending_results
uenv_completed_async_entries
uenv_runtime_tasks
uenv_blocking_tasks
uenv_agent_jobs{state,pool}
uenv_trajectory_write_queue_depth
uenv_trajectory_write_seconds
```

限制：

- `worker_id`、`episode_id`、`job_id` 不得作为 Prometheus label。
- histogram bucket 必须覆盖微秒级调度到秒级排队。
- 高频 heartbeat 成功日志需要采样、聚合或降为 debug，否则日志本身会成为瓶颈。

### 8.2 instrumentation 开销验收

在 1000 worker 档位分别运行：

1. 未开启 metrics scrape。
2. 开启 1 秒 scrape。

若吞吐下降超过 3% 或 P99 上升超过 5%，需要优化指标实现后才能建立正式基准。

## 9. 场景配置格式

`config/benchmark/10k-pre-common.yaml` 示例：

```yaml
run:
  baseline_id: 10k-pre-20260716
  warmup_secs: 180
  steady_secs: 600
  repetitions: 3

server:
  grpc_endpoint: 8.130.75.157:8088
  admin_endpoint: http://8.130.75.157:50052
  metrics_endpoint: http://8.130.75.157:50052/metrics

workers:
  count: 1000
  shards: 4
  mode: connection
  max_concurrent: 1
  heartbeat_interval_ms: 5000
  heartbeat_jitter_ratio: 0.20
  register_window_secs: 60
  env_types: [mock]

episode:
  execution_latency_ms: 100
  in_flight_target: 1000
  result_payload_bytes: 1024
  submit_mode: unary
  batch_size: 64

connection:
  behavior: current
  report_reconnect: true
  dispatch_endpoint_cache: false

abort:
  server_rss_bytes: 25769803776
  host_available_memory_bytes: 4294967296
  fd_usage_ratio: 0.80
  server_cpu_ratio: 0.95
  server_cpu_hold_secs: 300
  health_failure_secs: 30
  error_ratio: 0.10
  error_hold_secs: 60

artifacts:
  output_dir: /var/tmp/uenv-baseline/<run-id>
  sample_interval_secs: 1
```

每个场景文件只覆盖与 common 不同的字段。完整运行配置必须在开始时展开并归档，不能只保存引用链。

## 10. 必跑场景

### S00：工具与协议冒烟

| 参数 | 值 |
|---|---|
| worker | 1、10、100 |
| episode | 每档 100 条 |
| 时长 | 每档 3 分钟 |
| 成功判据 | registry、容量、dispatch、report、release 全部一致，无错误。 |

S00 未通过时禁止进入规模阶梯。

### S01：registry 阶梯

```text
100 → 500 → 1000 → 2000 → 5000 → 7500 → 10000
```

- 只注册，不提交 episode。
- 每档稳定 10 分钟。
- 记录注册 P50/P95/P99、RSS、FD、registry 数和每 1000 worker 内存斜率。
- 补做 30000、100000 registry-mode，仅用于容量模型。

### S02：注册风暴

- 10000 worker 在 60 秒内完成注册，平均约 167 register/s。
- 另做 10 秒突发档位，观察尖峰而不作为生产目标。
- 记录连接建立速率、注册错误、Server CPU、锁等待和容量一致性。

### S03：稳态心跳

- 10000 worker、5 秒心跳，理论约 2000 heartbeat/s。
- 先关闭 episode，再叠加 10%、50%、100% in-flight。
- 记录 heartbeat ACK P99、scheduler lock wait、日志量和连接建立速率。

### S04：native 组合负载

- 10000 worker、10000 in-flight。
- execution latency 分为 `10 ms / 100 ms / 1 s / 30 s`。
- submit、dispatch、ReportResult 持续并发。
- unary batch、stream、async submit 分开执行。
- 每种入口分别输出 admission、dispatch、结果 ACK 和端到端 P99。

### S05：结果 payload 分档

```text
1 KiB → 64 KiB → 1 MiB → 协议允许最大值
```

- 每档固定完成 QPS，测序列化、网络、缓存和存储成本。
- 记录单结果内存增量、网络带宽和 ReportResult ACK P99。

### S06：超载

- worker 容量固定 10000。
- 提交 30000 episode。
- 旧实现可能出现等待 task、active map 或内存持续增长；必须记录而不是掩盖。
- 停止上游后观察 30 分钟，确认能否回落。

### S07：批量掉线与重连

- 稳态下同时暂停 30% worker 心跳。
- 等待超过 heartbeat timeout，记录 ready/stale/capacity/admission 变化。
- 恢复后在 60 秒窗口内重连。
- 检查容量是否精确回补，是否重复增加 permit。

### S08：Server 重启

- 在 50% 和 100% in-flight 下分别执行一次正常 restart 和一次 kill -9。
- 记录 RTO、worker 重连分布、active/pending 丢失、重复执行和 late result。
- 该场景需要维护窗口，并明确接受旧实现可能丢状态。

### S09：取消风暴

- 创建 10000 active episode 后集中取消。
- 记录 cancel RPC、dispatch cancel、连接数、runtime task 和 late-result 缓存。

### S10：管理接口影响

- 在 1000/5000/10000 worker 下分别调用 `/status`、`/agents` 和 ListWorkers。
- 测量响应大小、序列化 CPU、耗时和对调度 P99 的影响。
- 管理接口压测与业务流量并发执行一次。

### S11：稳定性

- 在最大稳定档位运行 2 小时。
- 若 2 小时无持续增长，再在最大稳定档位或业务目标档位运行 24 小时。
- 记录 RSS、FD、task、缓存、日志、WAL和磁盘增长斜率。

## 11. Agent 与 SWE 的独立基准

worker 基准和 Agent 基准必须分开，不允许用 10000 worker 的结果代替 10000 Agent。

### 控制面 synthetic Agent

| 场景 | 规模 | 目的 |
|---|---:|---|
| A01 注册和心跳 | 10000 Agent | Agent registry、heartbeat 和 pool capacity。 |
| A02 空轮询 | 10000 Agent、3 秒 poll | 建立当前短轮询 QPS 和 CPU 基线。 |
| A03 任务突发 | 10000 AgentJob | pending、poll、in-flight 和 complete。 |
| A04 多 pool | 1000 pool、混合 bridge | pool/bridge 匹配扫描和公平性。 |
| A05 批量重连 | 10000 Agent/60秒 | 注册风暴和容量恢复。 |

### 真实 SWE 抽样

- 真实容器规模不与 synthetic Agent 数量绑定。
- 每个重要构建至少跑：
  - 1 条 gold E2E。
  - 1 条失败注入 E2E。
  - LLM backend ready 后 1 条受控多轮 LLM E2E。
- 当前 LLM backend 不在线时，报告必须标记为数据面环境阻塞，不能将其写成控制面失败。

## 12. 指标采集

### 12.1 Server 侧每秒采集

建议 `scripts/baseline/collect-server.sh` 采集：

```bash
pidstat -h -r -u -w -p <server-pid> 1
cat /proc/<server-pid>/status
cat /proc/<server-pid>/limits
ls /proc/<server-pid>/fd | wc -l
ss -s
ss -tanp
sar -n DEV,TCP,ETCP 1
vmstat 1
iostat -xz 1
```

同时抓取：

- `/metrics` 原始文本或 Prometheus TSDB。
- `/status`、`/agents` 的低频快照，默认 10 秒一次。
- systemd status 和 `NRestarts`。
- adapter 日志起止 offset，不直接复制整个历史日志。
- trajectory DB/WAL、日志目录和数据目录大小。

### 12.2 发生器侧采集

- 发生器进程 CPU/RSS/task。
- 发出和完成 RPC 数。
- event loop scheduling delay。
- 连接数和网络吞吐。
- coordinator 到 shard 的时钟偏差。
- 本地队列水位和丢弃数。

任何发生器节点 CPU 超过 70%、网络超过 70% 或本地队列增长时，该轮标记为 `loadgen_saturated`，不能用于 Server 容量结论。

## 13. 执行流程

### 13.1 Preflight

`scripts/baseline/preflight.sh` 必须检查：

1. Git 工作树和 baseline tag。
2. 运行二进制、配置和 unit 哈希。
3. systemd 只有一个 MainPID。
4. 目标端口只有该 MainPID 监听。
5. `NRestarts=0`。
6. `nofile=1048576`。
7. active episode、pending result、AgentJob 是否为空。
8. Server 和所有 LoadGen 时钟是否同步。
9. 磁盘、内存和网络余量。
10. Prometheus/collector 是否可写。

任一硬检查失败，禁止开始正式压测。

### 13.2 小规模校准

```bash
cargo build --release -p uenv-scale-bench
scripts/baseline/run-scenario.sh s00-smoke --workers 100
scripts/baseline/run-scenario.sh s01-registry-ladder --workers 500
scripts/baseline/run-scenario.sh s03-heartbeat-steady --workers 1000
```

校准阶段需要人工核对：

- coordinator 统计与 Server registry 数完全相等。
- 期望 heartbeat QPS 与实际误差不超过 2%。
- mock dispatch 数与 ReportResult 数一致。
- 没有重复 episode、lease 或 result idempotency key。

### 13.3 正式阶梯

每个档位：

1. 清空上一轮 synthetic registry 或重启隔离 benchmark Server。
2. 生成新 run ID 和 manifest。
3. 启动 Server collector。
4. 启动所有 LoadGen shard，等待 ready barrier。
5. 同步进入 warmup。
6. 进入 steady 并打时间标记。
7. 停止新流量，等待 drain。
8. 导出 Server 和 LoadGen 指标。
9. 检查资源与进程残留。
10. 打包本轮 artifacts。

同一档位连续执行 3 次。若三次结果离散过大，先排查噪声，禁止直接取最好的一次。

### 13.4 故障场景

故障注入必须由 coordinator 发出并记录时间戳。禁止人工临时执行而不留命令记录。

Server restart/kill、网络丢包和磁盘故障属于破坏性场景，只允许在明确维护窗口或隔离环境执行。

## 14. 自动停止条件

以下任一条件触发时，coordinator 停止增加负载并进入采集/清理阶段：

| 条件 | 默认阈值 |
|---|---:|
| Server 可用内存 | `< 4 GiB` |
| Server RSS | `> 24 GiB` |
| FD 使用率 | `> 80%` |
| Server CPU | `> 95%` 持续 5 分钟且吞吐不增长 |
| 健康检查失败 | 持续 30 秒 |
| RPC 错误率 | `> 10%` 持续 60 秒 |
| LoadGen CPU | `> 70%` 持续 60 秒，标记结果无效 |
| LoadGen 网络 | `> 70%` 持续 60 秒，标记结果无效 |
| RSS/task/队列 | 连续 10 分钟单调增长且停止上游后不回落 |

触发停止条件不是测试失败本身，而是一个必须保存的容量边界证据。

## 15. 有效运行判据

一轮运行只有满足以下条件才标记为 `valid`：

1. manifest 完整，代码和配置哈希可确认。
2. Server 只有一个受托管进程，期间未发生非场景计划内重启。
3. LoadGen 未饱和。
4. 注册数、容量和预期规模一致，误差有解释。
5. 原始指标时间范围覆盖 warmup、steady、drain。
6. 所有临时进程、端口和文件均清理或按 run ID 归档。

三次重复运行的吞吐、P99、RSS原则上差异不超过 10%。超过 10% 时标记 `unstable_measurement`，需要重测。

## 16. 结果分级与核心产出

每个规模档位标记：

| 等级 | 定义 |
|---|---|
| Stable | 稳态 10 分钟，错误率低于 1%，资源无持续增长。 |
| Degraded | 仍能服务，但 P99 明显上升、CPU 超过 85% 或出现容量抖动。 |
| Failed | 健康失败、错误率超过 10%、OOM、FD 耗尽、容量错误或无法 drain。 |

最终报告必须给出：

```text
W_register_max      最大可注册 worker 数
W_heartbeat_stable  最大稳定心跳 worker 数
W_inflight_stable   最大稳定 in-flight 数
W_knee              首个明显性能拐点
Throughput_max      最大持续吞吐
CPU_per_1k          每增加 1000 worker 的 CPU 增量
RSS_per_1k          每增加 1000 worker 的 RSS 增量
FD_per_1k           每增加 1000 worker 的 FD 增量
Register_P99
Heartbeat_P99
Reserve_P99
Dispatch_P99
ReportResult_ACK_P99
RTO / RPO
First_bottleneck
Failure_mode
```

## 17. Artifacts 目录规范

```text
Docs/server/baselines/<baseline-id>/<scenario>/<run-id>/
  manifest.json
  expanded-scenario.yaml
  commands.md
  summary.json
  server/
    pidstat.log
    vmstat.log
    iostat.log
    ss.log
    metrics.prom
    status.jsonl
    agents.jsonl
    service-status.txt
    adapter-log.txt
  loadgen/
    shard-00.jsonl
    shard-01.jsonl
    shard-02.jsonl
    shard-03.jsonl
  analysis/
    percentiles.json
    resource-slopes.json
    errors.json
    charts/
```

原始文件只追加，不在分析阶段覆盖。分析脚本输出到 `analysis/`。

## 18. 改造前后比较规则

改造后复测必须保持：

- 相同机器规格和内核参数。
- 相同 baseline 工具版本。
- 相同场景展开配置。
- 相同 worker 数、心跳间隔、payload 和故障时序。
- 相同 metrics scrape 周期。
- 相同日志级别和采样策略。

若必须改变任一条件，报告需要单独列出，禁止直接计算提升百分比。

建议比较表：

| 指标 | 改造前 | 改造后 | 变化 | 结论 |
|---|---:|---:|---:|---|
| 最大稳定 worker |  |  |  |  |
| Heartbeat P99 @10k |  |  |  |  |
| Reserve P99 @10k |  |  |  |  |
| RSS @10k |  |  |  |  |
| FD @10k |  |  |  |  |
| 建连速率 @10k |  |  |  |  |
| 30% 掉线容量收缩时间 |  |  |  |  |
| Server restart RTO |  |  |  |  |

## 19. 五天落地计划

| 日期/阶段 | 主要任务 | 交付物 | Gate |
|---|---|---|---|
| D1 | 处理未提交代码、提交 unit、冻结标签 | baseline tag、manifest | G0 |
| D2 | 补核心 metrics 和 collector | instrumentation commit、指标字典 | G1 |
| D3 | 实现 Rust LoadGen 注册/心跳/dispatch/report | S00、S01、S03 可运行 | G2 |
| D4 | 运行 100～10000 阶梯与组合负载 | 原始 artifacts、容量拐点 | G3 |
| D5 | 运行掉线/重连/重启，生成报告 | 改造前基准报告 | G4 |

如果 LoadGen 或指标系统在 D3 未通过自校验，不得为了赶进度直接开始 10k 测试。

## 20. 责任分工建议

| 角色 | 责任 |
|---|---|
| Server 开发 | instrumentation、容量不变量、Server 日志和问题定位。 |
| 压测工具开发 | Rust LoadGen、fault 注入、发生器自监控。 |
| 运维 | 基准机器、systemd、内核参数、网络、Prometheus和维护窗口。 |
| 测试 | 场景执行、结果有效性判定、复测和 artifacts 完整性。 |
| SWE/Agent 负责人 | synthetic Agent场景和真实 gold/LLM抽样。 |

## 21. 开始改造前的最终检查单

- [ ] `uenv-bridge/core/src/core.rs` 的代码改动已明确并提交。
- [ ] systemd unit 已纳入仓库。
- [ ] 工作树中的运行数据和缓存不进入代码提交。
- [ ] baseline tag 已建立。
- [ ] manifest 能从干净环境生成。
- [ ] instrumentation 开销已验证。
- [ ] LoadGen 在 1000 worker 下完成自校验。
- [ ] 发生器侧不存在 CPU/网络瓶颈。
- [ ] 完整规模阶梯已运行并重复 3 次。
- [ ] 过载、掉线/重连和 Server 重启至少各运行一次。
- [ ] 原始指标、日志、配置和命令均已归档。
- [ ] 报告已明确最大稳定规模、性能拐点和首要瓶颈。
- [ ] 真实 native 和 SWE gold 冒烟已通过。
- [ ] LLM Agent 的 backend 环境状态已单独标注。

完成上述检查后，才能开始 scheduler 索引、连接复用、分层背压等万 worker 改造，并以本基准作为回归对照。
