# UEnv Worker Pool 层设计说明

> **文档版本**：v1.3  
> **依据方案**：[UEnv — 下一代分布式训练环境框架方案 v7.1](./UEnv%20—%20下一代分布式训练环境框架方案-v7.1.pdf)  
> **适用范围**：Worker Pool 层（Layer 2 环境执行层）的实现与协作边界  
> **最后更新**：2026-05-25  
> **v1.3 变更**：补充 UEnv 平台级日志目录约定（`/var/log/uenv/`、`tail -f`）；各服务 CLI 命令规范；YAML/JSON 配置文件格式（ADR-002）  
> **实施前置**：[worker-pool-pre-mvp-architecture-adjustment.md](./worker-pool-pre-mvp-architecture-adjustment.md) — MVP 代码开发前须完成的框架对齐  
> **v1.2 变更**：冻结插件 Runtime 生命周期（1 进程 = 1 实例）；Dispatch 租约语义；插件崩溃恢复；控制面与插件 IPC 双协议层边界  
> **v1.1 变更**：冻结 Scheduler/Worker 控制面模型；收敛 MVP 插件与容错语义；提前冻结 WAL schema

---

## 1. 定位与职责边界

### 1.1 在整体架构中的位置

UEnv 采用「控制平面 / 数据平面分离」架构。Worker Pool 属于 **Layer 2 环境执行层**，承担 **资源目录（Resource Registry）** 与 **执行节点托管** 职责；**调度决策** 始终在 UEnv Server / Scheduler，本层 **不做二次调度**。

```
训练框架 ──Adapter──► UEnv Server（Scheduler / 控制面）
                              │
              ┌───────────────┼───────────────────────────────┐
              │ 查询 worker 清单 / capacity / env 能力         │
              ▼               │                               │
     Worker Pool（资源目录）   │  直连 DispatchEpisode         │
     worker 注册 / 心跳聚合    └──────────────► Worker（gRPC Server）
              │                                      │
              │                                      ├── 预热池 / Episode 执行 / 插件
              │                                      └── 数据平面 → 模型推理服务
              └── 返回 endpoint / load / warm pool 状态
```

**冻结的控制面模型（v1.1，与 v7.1 PDF 对齐）**：

| 角色 | 定位 | 通信方向 |
|------|------|----------|
| **UEnv Server / Scheduler** | 集中式调度控制面 | 查询 Worker Pool → **直连** 目标 Worker `DispatchEpisode` |
| **Worker Pool** | Resource Registry / Resource Provider | 维护 worker 清单、capacity、env 能力、预热池元数据、健康状态；**不转发 Episode 数据流** |
| **Worker** | 可被调度的执行节点（**gRPC Server**） | 主动 `RegisterWorker` / `Heartbeat` / `ReportResult`；被动接收 `DispatchEpisode`，流式 `StreamReport` |

**明确不采用**：

- `Worker = gRPC Client` + `subscribe_dispatch` 拉取任务
- `Server → WorkerPool → Worker` 二次调度 / Episode 转发（双重调度、热点、tracing 断裂）

### 1.2 Worker Pool 层核心职责

| 职责 | 说明 | 方案章节参考 |
|------|------|-------------|
| Episode 执行 | 接收 `EpisodeRequest`，执行 reset → N×step → close 完整循环 | §4.4、§7.1 |
| 环境实例生命周期 | 创建、复用、归还、回收环境实例 | §9.2 |
| **预热池管理** | 提前创建并保持 Warm 实例，降低冷启动延迟（**本层负责**） | §6.4 |
| 后端抽象 | 通过 `Backend` trait 屏蔽 Process / Podman 差异 | §7.2、§7.3 |
| 插件化环境接入 | 以插件形式挂载多语言实现的环境实例 | 本文 §3 |
| Worker 控制面服务 | Worker 暴露 gRPC Server：`DispatchEpisode`、`HealthCheck`；Server 主动下发任务 | §5.3、§7.1 |
| 向 Server 主动上报 | `RegisterWorker`、`Heartbeat`、`ReportResult`、`StreamReport` | §5.3、§7.1 |
| 资源目录（Worker Pool） | worker 清单、env 能力聚合、capacity、预热池状态、生命周期；**供 Scheduler 查询，不做调度** | 本文 §1.4、§7 |
| 本地容错 | 断连 WAL、重连与状态同步；**禁止默认 `env.step()` 重试**；Episode 重试由 Scheduler 统一控制 | §10.5、§10.7、§11 |
| 可观测性 | Linux 文本日志（`/var/log/uenv/`）、CLI 服务入口、YAML/JSON 配置、Prometheus 指标、trace_id 传播 | §2.2、§2.5、§2.6、§14 |

### 1.3 不属于本层的职责

- Episode **调度决策**、Worker 候选打分、affinity 选择（Scheduler / UEnv Server）
- Worker Pool **二次分发** Episode（`Server → Pool → Worker` 数据面转发）
- 训练框架协议转换（Training Adapter，Layer 4）
- 环境元数据持久化与发布（UEnvHub，Layer 1）
- 训练算法与模型权重管理

### 1.4 Worker 控制面能力清单（冻结）

Worker 作为 **gRPC Server**，对外必须提供：

| RPC / 能力 | 方向 | 必须 |
|------------|------|------|
| `DispatchEpisode` | Server → Worker | 是 |
| `StreamReport`（随 Dispatch 流） | Worker → Server | 是 |
| `HealthCheck` | 探活 | 是 |
| `RegisterWorker` | Worker → Server（主动） | 是 |
| `Heartbeat` | Worker ↔ Server（主动保活） | 是 |
| `ReportResult` | Worker → Server（主动） | 是 |

`DispatchEpisode` 语义与 v7.1 一致：

```proto
rpc DispatchEpisode(EpisodeRequest) returns (stream StreamReport);
```

即 **Server 调用 Worker**，Worker 在执行过程中流式回报进度；最终结果经 `ReportResult` 主动上报（可带 WAL 重放）。

---

## 2. 技术栈与实现约束

### 2.1 语言与 Edition

| 项 | 要求 |
|----|------|
| Worker Pool **框架代码** | **Rust 2024**（`edition = "2024"`） |
| 异步运行时 | Tokio |
| 与 Scheduler 控制通道 | gRPC + Protobuf（与方案 v7.1 契约一致） |
| 环境插件 IPC（MVP） | **Protobuf over Unix Domain Socket** + 子进程（见 §3） |
| Worker 控制面 | gRPC Server（`DispatchEpisode` 等）+ gRPC Client（`RegisterWorker` / `Heartbeat` / `ReportResult`） |

> **说明**：框架主体用 Rust 2024 编写；具体环境实现可通过插件以 Python、Rust 或其他语言交付，经统一插件宿主加载。

### 2.2 日志格式（强制，ADR 冻结）

> **ADR-001：Logging Format Decision** — Worker Pool 层日志落盘格式以本节为准；后续 tracing / ELK / Loki / Promtail 集成均不得引入 JSON 落盘分裂。

系统日志 **统一使用 Linux 传统文本 `.log` 文件**，**禁止**将运行日志落盘为 JSON。

**规范行格式**：

```
timestamp LEVEL target k=v k=v msg="..."
```

示例：

```
2026-05-25T12:00:00.123456+08:00 INFO uenv.worker.episode trace_id=t-abc episode_id=ep-abc123 worker_id=worker-1 env_type=gsm8k msg="Episode completed" duration_ms=523 reward=1.0
```

| 要求 | 说明 |
|------|------|
| 单行日志 | **必须**；禁止多行 stacktrace 落盘 |
| `trace_id` | **必须**（可从 gRPC metadata / `EpisodeRequest` 传播） |
| `episode_id` | Episode 相关日志 **必须** |
| `worker_id` | Worker 进程日志 **必须** |
| JSON blob | **禁止** 作为日志行主体或附件块 |
| multiline stacktrace | **禁止** 落盘；错误摘要写入单行 `msg` |
| 落盘路径 | 统一目录 `/var/log/uenv/`；**每服务独立 `.log` 文件**（见下表）；支持 `logrotate` |
| 结构化字段 | 以 `key=value` 附在行内，便于 `grep` / `awk` |
| 级别 | ERROR / WARN / INFO / DEBUG / TRACE |
| 与方案差异 | v7.1 §14.1 建议 JSON 结构化日志；**本层实现以 ADR-001 为准** |

**平台级日志目录（冻结）** — 所有 UEnv 组件落盘至 `/var/log/uenv/`，**禁止**将运行日志写入 stdout-only 而无持久化（开发模式可经 `--log-file -` 写 stderr，生产必须落盘）：

| 组件 | 默认日志文件 | 本层/MVP 范围 |
|------|-------------|---------------|
| Worker | `/var/log/uenv/worker.log` | **本层** |
| Mock Scheduler | `/var/log/uenv/mock-scheduler.log` | **MVP** |
| UEnv Server / Scheduler | `/var/log/uenv/scheduler.log` | 跨层约定（Server 团队） |
| Training Adapter | `/var/log/uenv/adapter.log` | 跨层约定（Layer 4） |
| Worker Pool Registry | `/var/log/uenv/pool-registry.log` | 跨层约定（若独立进程） |

路径可通过 `UENV_LOG_FILE` 或配置文件 `logging.file` 覆盖；**禁止**多服务共写同一 `.log` 文件。

**运维约定（`tail -f`）** — 日志为 append-only 单行文本，标准 Linux 运维即可：

```bash
# 实时跟踪 Worker 日志
tail -f /var/log/uenv/worker.log

# 按 episode 过滤（另开终端）
grep 'episode_id=ep-abc123' /var/log/uenv/worker.log

# 多服务并行跟踪
tail -f /var/log/uenv/worker.log /var/log/uenv/mock-scheduler.log
```

`logrotate` 示例：`/etc/logrotate.d/uenv` 对 `/var/log/uenv/*.log` 按日或按大小轮转，`copytruncate` 或 `create` 均可；轮转后 `tail -f` 需重新打开文件（或使用 `tail -F` 跟踪 inode 变化）。

环境变量示例：

```bash
UENV_LOG_LEVEL=INFO
UENV_LOG_FILE=/var/log/uenv/worker.log
# 不使用 UENV_LOG_FORMAT=json
```

### 2.3 序列化与通信约束（继承方案）

- 控制平面：gRPC 双向流 + Protobuf，不使用 JSON 作为 RPC 载荷
- 不使用 Redis / Kafka / RabbitMQ 等消息中间件
- Episode 级调度粒度（非单 step 远程调用）

### 2.4 双协议层边界（冻结）

Worker Pool 内存在 **两套独立协议栈**，必须严格分层、互不泄漏：

| 层 | 参与方 | 协议 | 载荷 | 演进范围 |
|----|--------|------|------|----------|
| **L1 控制面** | Scheduler ↔ Worker | gRPC + Protobuf | `EpisodeRequest`、`DispatchEpisode`、`ReportResult` 等 | 随 UEnv Server 版本演进 |
| **L2 插件 IPC** | Worker ↔ 插件子进程 | Protobuf over UDS（MVP） | `reset` / `step` / `close` / `health_check` | **仅 Worker 内部** |

**边界规则（必须遵守）**：

1. **Scheduler 不感知插件 IPC** — 控制面 proto 中不得出现 UDS 路径、插件 PID、插件侧 message 类型等实现细节。
2. **插件协议不得泄漏到控制面** — `EpisodeRequest` 只含 `env_type` 等业务字段；Worker 在本地将 Episode 绑定到 **进程级实例**（见 §3.5）。
3. **插件 IPC 可替换，不影响 L1** — MVP 为 Proto/UDS；后续可换 Cap'n Proto、shared memory、QUIC 等，**无需修改** Scheduler ↔ Worker proto。
4. **观测与 tracing** — `trace_id` / `episode_id` 在 L1 传播；L2 日志可作为 Worker 子 span 字段，但不回传 Scheduler。

```
Scheduler ──[L1 gRPC]──► Worker ──[L2 Proto/UDS]──► Plugin 子进程
              ▲                         │
              │                         └── 替换 L2 不影响 L1
              └── 永不直达 Plugin
```

### 2.5 CLI 命令行接口（冻结）

> **原则**：UEnv **每个可独立部署的服务** 必须提供 **CLI 二进制 + 子命令**，禁止仅能通过库 API 或隐式入口启动。开发、测试、生产使用同一套 CLI 契约。

**通用约定**：

| 项 | 要求 |
|----|------|
| 二进制命名 | `uenv-<component>`，如 `uenv-worker`、`uenv-mock-scheduler` |
| 主入口 | `<binary> serve` — 启动长期运行服务（默认子命令可设为 `serve`） |
| 配置 | 全局 `--config <path>`；未指定时按 §2.6 默认路径加载 |
| 日志 | `--log-level` / `--log-file` 覆盖配置文件与环境变量 |
| 帮助 | `--help` / `-h`；子命令级 `--help` |
| 退出码 | `0` 成功；`1` 一般错误；`2` 用法错误；`130` SIGINT |

**本层与 MVP 服务 CLI 契约**：

| 二进制 | 子命令 | 说明 |
|--------|--------|------|
| `uenv-worker` | `serve` | 启动 Worker gRPC Server + ControlPlane 客户端 + 运行时 |
| `uenv-worker` | `version` | 输出 `protocol_version`、crate 版本、git commit（若有） |
| `uenv-worker` | `health` | 本地探活：gRPC `HealthCheck` 或 HTTP `/health`（若暴露） |
| `uenv-mock-scheduler` | `serve [--fixture-dir DIR]` | 启动 Mock ControlPlane + 主动 Dispatch |
| `uenv-mock-scheduler` | `version` | 输出版本与 proto 版本 |

示例：

```bash
# 使用默认配置文件启动 Worker
uenv-worker serve

# 显式指定配置与日志
uenv-worker serve --config /etc/uenv/worker.yaml --log-file /var/log/uenv/worker.log

# Mock Scheduler（M1 起）
uenv-mock-scheduler serve --fixture-dir ./fixtures/gsm8k
```

**跨层服务（非本层实现，命名对齐）** — 供联调与运维统一：

| 二进制 | 子命令 | 默认日志 |
|--------|--------|----------|
| `uenv-server` | `serve` | `/var/log/uenv/scheduler.log` |
| `uenv-adapter` | `serve [--framework roll\|verl\|...]` | `/var/log/uenv/adapter.log` |

配置优先级（与 §12 一致）：**CLI 参数 > 环境变量 > 配置文件 > 默认值**。

### 2.6 配置文件格式（冻结，ADR-002）

> **ADR-002：Configuration Format Decision** — 服务级配置 **必须** 支持 **YAML** 与 **JSON** 两种文件格式（按扩展名或 `--config` 路径自动识别：`.yaml`/`.yml`/`.json`）。**禁止**将运行日志落盘为 JSON（ADR-001 与 ADR-002 独立：配置可读 JSON，日志仍用 `.log` 文本）。

**默认配置文件路径**（`serve` 未传 `--config` 时按序查找，命中即用）：

1. `./uenv-<component>.yaml`（当前工作目录）
2. `/etc/uenv/<component>.yaml`
3. `./uenv-<component>.json`
4. `/etc/uenv/<component>.json`

示例：Worker 默认尝试 `/etc/uenv/worker.yaml`。

**Worker 配置示例（YAML，推荐）**：

```yaml
# /etc/uenv/worker.yaml
server:
  endpoint: "localhost:50051"
worker:
  listen: "0.0.0.0:50052"
  id: "auto"
  max_concurrent: 4
scheduler:
  mode: "remote"          # remote | mock
env:
  types: ["gsm8k"]
  plugin_dir: "./plugins"
  backend: "process"
pool:
  warmup_size: 2
  max_idle_time: 300
  cool_timeout: 60
  max_episode_count: 1000
logging:
  level: "INFO"
  file: "/var/log/uenv/worker.log"
wal:
  dir: "/tmp/uenv/wal"
```

**等价 JSON 示例**（字段名与 YAML 相同，嵌套结构一致）：

```json
{
  "server": { "endpoint": "localhost:50051" },
  "worker": { "listen": "0.0.0.0:50052", "id": "auto", "max_concurrent": 4 },
  "scheduler": { "mode": "remote" },
  "env": { "types": ["gsm8k"], "plugin_dir": "./plugins", "backend": "process" },
  "pool": { "warmup_size": 2, "max_idle_time": 300, "cool_timeout": 60, "max_episode_count": 1000 },
  "logging": { "level": "INFO", "file": "/var/log/uenv/worker.log" },
  "wal": { "dir": "/tmp/uenv/wal" }
}
```

**配置文件 ↔ 环境变量映射**（实现必须支持双向覆盖，env 优先于文件）：

| 配置键（YAML/JSON） | 环境变量 |
|---------------------|----------|
| `server.endpoint` | `UENV_SERVER_ENDPOINT` |
| `worker.listen` | `UENV_WORKER_LISTEN` |
| `worker.id` | `UENV_WORKER_ID` |
| `worker.max_concurrent` | `UENV_MAX_CONCURRENT` |
| `scheduler.mode` | `UENV_SCHEDULER_MODE` |
| `env.types` | `UENV_ENV_TYPES`（逗号分隔） |
| `env.plugin_dir` | `UENV_PLUGIN_DIR` |
| `env.backend` | `UENV_BACKEND` |
| `pool.warmup_size` | `UENV_WARMUP_POOL_SIZE` |
| `pool.max_idle_time` | `UENV_MAX_IDLE_TIME` |
| `pool.cool_timeout` | `UENV_COOL_TIMEOUT` |
| `pool.max_episode_count` | `UENV_MAX_EPISODE_COUNT` |
| `logging.level` | `UENV_LOG_LEVEL` |
| `logging.file` | `UENV_LOG_FILE` |
| `wal.dir` | `UENV_WAL_DIR` |

插件 **`manifest.yaml`** 仍仅描述环境插件元数据（§3），**不是** Worker 服务配置文件；二者职责分离。

---

## 3. 环境插件架构（多语言实例）

### 3.1 设计目标

- Worker 框架（Rust 2024）保持稳定 ABI 与生命周期管理
- 环境实例可用 **其他语言** 实现（如 Python MathEnv / GSM8K）
- 通过 **插件** 接入，避免将业务环境逻辑焊死在 Worker 主 crate 中

### 3.0 MVP 收敛策略（Phase 0，冻结）

MVP 阶段 **仅保留一条插件路径**，避免双协议 / 多 ABI 并行带来的调试与 tracing 分裂：

| 维度 | MVP 选择 | 非 MVP（后续） |
|------|----------|----------------|
| IPC 载荷 | **Protobuf** | Cap'n Proto |
| 传输 | **Unix Domain Socket** | loopback TCP |
| 插件形态 | **子进程**（`ProcessBackend`） | `cdylib` 进程内加载 |
| 语言 | 单语言先闭环（建议 Rust 或 Python 二选一落地 GSM8K） | 多语言并行 |

```
MVP 唯一路径：ProcessBackend + Protobuf over UDS + 子进程插件
```

### 3.2 插件模型（进程拓扑）

```
┌─────────────────────────────────────────────────────────┐
│              uenv-worker（Rust 2024 宿主，WorkerRuntime）   │
│  ┌─────────────┐  ┌──────────────┐  ┌─────────────────┐ │
│  │ PluginHost  │  │ WarmupPool   │  │ EpisodeExecutor │ │
│  └──────┬──────┘  └──────┬───────┘  └─────────────────┘ │
│         │                │ 管理「进程级实例」队列            │
└─────────┼────────────────┼───────────────────────────────┘
          │ spawn/kill     │
          ▼                ▼
   ┌──────────────┐  ┌──────────────┐  ┌──────────────┐
   │ 插件进程 #1   │  │ 插件进程 #2   │  │ 插件进程 #N   │  ← MVP：每进程 = 1 env instance
   │ 1 env inst   │  │ 1 env inst   │  │ 1 env inst   │
   │ Proto/UDS    │  │ Proto/UDS    │  │ Proto/UDS    │
   └──────────────┘  └──────────────┘  └──────────────┘
```

> Phase 1+ 可选 `cdylib` 进程内路径；**MVP 不实现** 单进程多 session（见 §3.5）。

### 3.3 接入方式（按阶段）

| 方式 | 阶段 | 协议 | 说明 |
|------|------|------|------|
| **子进程 Sidecar** | **MVP（Phase 0）** | **Protobuf over UDS** | `ProcessBackend` 启动插件进程，aRPC 调用 |
| 进程内动态库 | Phase 1+ | Cap'n Proto / FFI | `crate-type = ["cdylib"]`，由 `PluginHost` 加载 |
| 容器内入口 | 生产 | 同上 + 容器边界 | `PodmanBackend` 管理容器生命周期 |

### 3.4 aRPC 接口契约（插件 ↔ 宿主）

插件需实现的最小 Simulation API（与方案 §4.1 对齐）：

| 方法 | 功能 | 返回值 |
|------|------|--------|
| `reset(seed?)` | 重置环境 | Observation |
| `step(action)` | 执行一步 | (obs, reward, terminated, truncated, info) |
| `close()` | 释放资源 | — |
| `health_check()` | 存活探测 | ok / error |

IDL 与传输（按阶段）：

- **MVP（冻结）**：Protobuf + Unix Domain Socket
- **后续**：Cap'n Proto（低延迟同机 IPC）；loopback TCP（跨网络边界调试）

### 3.5 插件 Runtime 生命周期模型（MVP 冻结）

本节冻结 **PluginHost / ProcessBackend / WarmupPool** 之间的实例语义，避免实现阶段对「一个插件进程里几个 env」产生分歧。

#### 3.5.1 核心等式（MVP）

```text
1 插件子进程 == 1 environment instance（进程级实例）
```

| 概念 | MVP 定义 |
|------|----------|
| **environment instance** | 一个已启动的插件子进程 + 其 UDS 连接 + 内部单一 env 状态机 |
| **WarmupPool 条目** | 一个 **进程级实例**（非进程内的 logical session） |
| **Active Episode** | 某一进程级实例在同一时刻 **最多绑定 1 个** Episode |

**明确不支持（MVP）**：

```text
1 plugin process → N env sessions   （多路复用 / session routing）
```

否则将引入：session ID 路由、插件侧调度、共享内存复用、实例内生命周期与 Worker 侧 Warm/Active 状态同步等问题 — **全部推迟到 Phase 1+**。

#### 3.5.2 生命周期状态（进程级实例）

与 §5.3 环境实例状态机一致；状态迁移作用于 **整个插件进程**：

```
PluginHost.spawn ──► Creating ──► Warm ⇄ Active ──► Idle ──► Cooling ──► Warm
                                      │                              │
                                      └── Evicting ──► kill 进程 ──► Destroyed
```

| 阶段 | Worker 侧行为 | 插件进程 |
|------|---------------|----------|
| 创建 | `ProcessBackend::create` 启动子进程，建立 UDS | 启动，等待 `reset` |
| 分配 Episode | Warm → Active；校验 **no double allocation** | 同一进程执行 `reset` → `step*` → 归还前 `cleanup` |
| 归还 | Active → Idle → Cooling → Warm；`reset`/cleanup 后回池 | 进程 **不退出** |
| 回收 | `health_check` 失败 / 超复用次数 / 空闲超时 | `kill` 进程，WarmupPool 移除条目 |
| 崩溃 | 见 §6.4 | 进程退出；实例标记 **Broken**，不回到 Warm 队列 |

#### 3.5.3 与并发模型的关系

- **Episode 间并发** = 多个 **插件进程** 并行（受 `UENV_MAX_CONCURRENT` 限制），非单进程内多 Episode。
- **Episode 内** = 单进程内串行 `reset → step* → close/cleanup`。
- `PluginHost` 只负责 **工厂 + 进程表**；`WarmupPool` 持有可复用的进程级实例句柄（`instance_id`、PID、UDS fd）。

#### 3.5.4 关键不变量（实现必须断言）

1. 每个 Warm/Active 条目对应 **唯一** 插件 PID。
2. 同一 `instance_id` 在 Active 期间 **不得** 被第二个 Episode 绑定。
3. 插件进程崩溃后，该 `instance_id` 不得再入池，须销毁并可选补池。

### 3.6 插件注册与发现

1. **本地注册表**：Worker 启动时扫描插件目录 / 配置清单，注册 `env_type → PluginFactory`
2. **声明能力**：每个插件上报 `supported_backends`、`resource_requirements`
3. **向 Server 注册**：`RegisterWorker` 时携带 `supported_envs` 列表
4. **UEnvHub 元数据**：Server 从 Hub 拉取 manifest，用于调度侧资源匹配（Worker 可选择性缓存）

初期目录约定：

```
plugins/
└── gsm8k/
    ├── manifest.yaml      # env_type=gsm8k, version, backends
    ├── plugin.toml        # 入口、协议类型（MVP 固定 proto+uds）
    └── (二进制或启动脚本)
```

---

## 4. 核心组件设计

### 4.1 组件一览

| 组件 | 职责 |
|------|------|
| `WorkerRuntime` | 进程入口：配置加载、组件装配、优雅退出 |
| **`WorkerGrpcServer`** | Worker 侧 gRPC Server：`DispatchEpisode`、`HealthCheck`（Server 主动调用） |
| `ControlPlaneClient` | 与 UEnv Server 的 gRPC 客户端：**仅**注册、心跳、`ReportResult`（**不** subscribe 拉任务） |
| **`WorkerPoolRegistry`** | 本进程或侧车维护：worker 清单、capacity、env 能力、预热池元数据（供 Scheduler 查询） |
| **`MockSchedulerGateway`** | 开发期模拟 **UEnv Server/Scheduler** 行为：查询资源目录 + **主动** `DispatchEpisode`（见 §8） |
| `EpisodeExecutor` | 单 Episode 执行引擎：模型回调、step 循环、Reward、流式上报 |
| `ConcurrencyPool` | 多 Episode 并发调度（受 `max_concurrent_episodes` 限制） |
| **`WarmupPool`** | 按 `env_type` 管理预热实例队列（见 §5） |
| `PluginHost` | 插件工厂：按 §3.5 **spawn/kill 进程级实例**；维护 PID / UDS 表 |
| `Backend` | ProcessBackend：启动 **1 子进程 = 1 instance**；崩溃检测与回收 |
| `LocalRegistry` | `env_type → PluginFactory` 映射 |
| `ModelClient` | 异步调用 `model_endpoint`（HTTP/gRPC/Ray） |
| `RewardEngine` | 根据 `RewardConfig` 构建 v4.0 兼容 Reward 链 |
| `WalWriter` | 断连时缓存 `EpisodeResult` |
| `MetricsExporter` | Prometheus `/metrics` |
| `LogSink` | Linux `.log` 文件写入 |

### 4.2 Episode 执行流程（Worker 侧）

与方案 §4.4 对齐，Worker 内完整步骤：

1. **获取实例**：从 `WarmupPool` 取 Warm 实例；池空则经 `Backend` + `PluginHost` 新建
2. **初始化 Reward 链**：按 `RewardConfig` 构建
3. **`env.reset(seed)`**：记录初始观测
4. **主循环**（最多 `max_steps` 步）：
   - 超时检查（`deadline`）
   - 模型回调 → `action` + `logprob`
   - `env.step(action)`（MCP 路由在插件内）
   - Reward 计算
   - 写入 Trajectory
   - `StreamReport` 上报 `STEP_COMPLETE`
   - 终止检查
5. **TrajectoryReward**（若配置）
6. 构建 `EpisodeResult`（含 SHA-256 `trajectory_checksum`）
7. **实例归还**：归还 `WarmupPool`（非销毁），供后续复用

### 4.3 并发模型

| 维度 | 模型 | 说明 |
|------|------|------|
| Episode 间 | 并发（Tokio 任务池） | 共享预热池；上限 `UENV_MAX_CONCURRENT` |
| Episode 内 | 串行 | reset → step 循环 → close 严格顺序 |
| 模型回调 | 异步非阻塞 | 等待推理时可处理其他 Episode |
| 工具调用 | 异步非阻塞 | MCP 工具执行期间不阻塞其他 Episode |
| Reward | 可并行 | `WeightedSum` 多信号源并行（方案 §7.1） |

### 4.4 资源隔离

| 资源 | 隔离方式 |
|------|----------|
| 环境实例（MVP） | **每实例 = 独立插件子进程**；进程间无共享 env 状态 |
| 内存 | OS 进程级隔离（子进程独立地址空间） |
| 网络 | Worker 进程共享；插件进程按策略隔离（Phase 0 通常仅 loopback） |
| GPU | 时间片共享，推理请求排队 |
| 故障域 | **插件 crash ≠ Worker crash**（见 §6.4） |

---

## 5. 预热池（Warmup Pool）

### 5.1 归属与目标

**预热池由 Worker Pool 层实现并持有**（非 Scheduler 进程内对象）。目标是消除环境冷启动（约 100ms–2s），使 Episode 到达时可直接获取 Warm 实例。

### 5.2 机制（方案 §6.4）

- 按 `env_type` 维护实例队列
- LRU 保留最近使用的环境类型
- Episode 完成后实例 **归还池中复用**，而非立即销毁
- 实例状态遵循环境实例状态机（§5.3）

### 5.3 环境实例状态机

```
Creating → Warm ⇄ Active → Idle → Cooling → Warm
                ↓                              ↓
            Evicting ──────────────────► Destroyed
```

| 状态 | 含义 |
|------|------|
| Creating | 正在创建 |
| Warm | 在预热池中待分配 |
| Active | 被某 Episode 占用 |
| Idle | Episode 结束，短暂空闲 |
| Cooling | 冷却期，等待回池 |
| Evicting | 回收中 |
| Destroyed | 已销毁 |

### 5.4 池容量：动态策略（重要）

方案 v7.1 §6.4 给出静态默认值（如 `warmup_pool_size=5`），**仅用于 Phase 0 初期**。本层最终实现要求：

| 阶段 | 策略 |
|------|------|
| **Phase 0 初期** | 使用固定默认参数，保证可跑通 |
| **Phase 1+** | **根据历史请求模式动态计算**每 Worker、每 `env_type` 的预热数量 |

动态计算可参考的信号：

- 滑动窗口内该 `env_type` 的 Episode 到达率（QPS）
- 实例创建 P95 延迟
- 池命中率（命中 Warm / 总获取次数）
- 当前 `max_concurrent_episodes` 与活跃 Episode 数
- 训练 Job 启动事件（方案 §6.5：训练 Job 启动前提前预热）

建议接口：

```rust
trait WarmupSizer {
    fn target_pool_size(&self, env_type: &str, stats: &PoolStats) -> usize;
}
```

### 5.5 配置参数

| 参数 | Phase 0 默认 | 说明 |
|------|-------------|------|
| `UENV_WARMUP_POOL_SIZE` | 2（每 env_type） | **初期固定值**，后期由 `WarmupSizer` 覆盖 |
| `UENV_MAX_IDLE_TIME` | 300s | 空闲超时回收 |
| `UENV_COOL_TIMEOUT` | 60s | 冷却期 |
| `UENV_MAX_EPISODE_COUNT` | 1000 | 单实例最大复用次数（防泄漏） |

### 5.6 复用安全（验收必达）

预热池管理对象为 **§3.5 进程级实例**（每个条目 = 一个插件子进程）。归还复用必须满足以下约束，防止坏实例 / 状态污染 / 双重分配：

| 验收项 | 时机 | 说明 |
|--------|------|------|
| `health_check` before reuse | 出池分配前 | 失败实例销毁，不回到 Warm 队列 |
| `reset` / cleanup before reuse | 归还入池前 | 清除上一 Episode 残留状态 |
| `max_episode_count` | 每次复用后递增 | 达上限强制销毁并重建 |
| `max_idle_time` | 后台巡检 | 防僵尸实例长期占用 |
| **no double allocation** | 分配 / 归还全程 | 同一实例同一时刻仅允许一个 Active Episode |
| `warmup_pool_hit` / `warmup_pool_miss` metrics | 每次获取实例 | 支撑 M6 量化收益与后续 `WarmupSizer` |

---

## 6. 后端引擎（Backend）

### 6.1 Backend Trait

Worker 仅依赖统一 trait，不感知底层是进程还是容器（方案 §7.2）：

| 方法 | 功能 |
|------|------|
| `create(spec)` | 创建环境实例 |
| `destroy(instance_id)` | 销毁实例 |
| `health_check(instance_id)` | 健康检查 |
| `list_instances()` | 列举受管实例 |
| `capabilities()` | 返回 `BackendCapabilities` |

### 6.2 两阶段后端

| 后端 | 启动延迟 | 隔离 | 初期用途 |
|------|----------|------|----------|
| **ProcessBackend** | <10ms | 进程级 | **Phase 0 开发与联调** |
| **PodmanBackend** | ~2s | rootless 容器 | Phase 1+ 生产 |

渐进路径（方案 §7.3）：`ProcessBackend`（开发）→ `PodmanBackend`（生产）。

### 6.3 Phase 0 后端选择

开发初期仅接入 **GSM8K**，推荐使用 **ProcessBackend + Python/Rust 插件子进程**，零容器依赖、最快迭代。

**MVP 冻结（与 §3.5 一致）**：

- 每个环境实例对应 **一个独立插件子进程**。
- `WarmupPool` 管理的是 **进程级实例**（非进程内多 session）。
- 同一插件进程同一时刻 **仅允许一个 Active Episode**。

### 6.4 Plugin Failure Semantics（插件崩溃恢复，冻结）

子进程模型的核心价值：**插件崩溃 ≠ Worker 崩溃**。`WorkerRuntime` 必须存活；插件进程可单独回收，不影响其他实例。

| 场景 | Worker 行为 | Episode 结果 | 实例 / 池 |
|------|-------------|--------------|-----------|
| 插件 `exit code != 0` | 记录 ERROR；标记实例 Broken | 当前 Episode **FAILED** | 进程终止；**不得**回 Warm 池；可选触发补池 |
| 插件 step / RPC **timeout** | `kill` 插件进程 | **FAILED**（不重试 `env.step`） | 实例销毁 |
| 插件 **UDS 断开** | 视为实例损坏 | 若在 step 中：**FAILED** | 实例销毁；WarmupPool 移除 |
| 插件在 **step 执行中** crash | 不重试 step | **FAILED** | 实例销毁 |
| 插件 crash **after step、before ReportResult** | 结果未确认；写 WAL（若已有部分结果则带 `FAILED`） | 由 Scheduler 新 `attempt_id` 重投；Worker 侧 **不重放 step** |
| Worker OOM / panic | 进程管理器重启 Worker | 在途 Episode 由 Scheduler 重分配 | 全部实例丢失，池重建 |

**原则**：

1. **禁止** 因插件 crash 自动重试 `env.step()`（副作用风险，见 §11.1）。
2. 插件失败时，`ReportResult` 应携带明确 `failure_reason=PLUGIN_CRASH`（或等价枚举）。
3. `PluginHost` 订阅子进程退出事件（`waitpid` / 任务 Join），与 `EpisodeExecutor` 解耦，避免阻塞其他 Episode。
4. 控制面 **L1** 只收到 `EpisodeResult` / `StreamReport`；插件 stderr 等仅写入 Worker **L2** 日志（§2.4）。

---

## 7. 与 Scheduler / UEnv Server 的交互

### 7.0 总体结构（冻结）

```
                    ┌─────────────────────┐
                    │   UEnv Server       │
                    │  Scheduler / CP     │
                    └─────────┬───────────┘
                              │
                查询 worker / capacity / env 能力
                              │
                              ▼
               ┌─────────────────────────┐
               │     Worker Pool         │
               │ Resource Registry Layer │
               └──────────┬──────────────┘
                          │ 返回 endpoint / load / warm 状态
                          ▼
               ┌─────────────────────────┐
               │        Worker           │
               │     gRPC Server         │
               │  DispatchEpisode / …    │
               └─────────────────────────┘
```

**调度路径**：Scheduler 查询 Worker Pool 获得候选 Worker 的 `endpoint` 与负载信息 → **直连** `Worker.DispatchEpisode(...)`。**不经过** Worker Pool 转发 Episode 流。

### 7.1 gRPC 接口分工

| 交互 | 方向 | RPC | 承载方 | 说明 |
|------|------|-----|--------|------|
| 注册 | Worker → Server | `RegisterWorker` | ControlPlaneClient | 上报 `worker_id`、`endpoint`、`supported_envs`、`capacity` |
| 心跳 | Worker ↔ Server | `WorkerHeartbeat` | ControlPlaneClient | 保活；含 `server_epoch` fencing（见 §7.4） |
| **下发任务** | **Server → Worker** | **`DispatchEpisode`** | **WorkerGrpcServer** | Server 主动调用；Worker 返回 `stream StreamReport` |
| 上报结果 | Worker → Server | `ReportResult` | ControlPlaneClient | 幂等键见 §7.5；断连时经 WAL 重放 |
| 健康检查 | 探活 → Worker | gRPC Health | WorkerGrpcServer | K8s / 编排探活 |
| 资源查询 | Server → Worker Pool | `ListWorkers` / `GetCapacity`（名称以 proto 为准） | WorkerPoolRegistry | **只读**；不含 Episode 转发 |

> **禁止**：Worker 作为纯 Client 通过 `subscribe_dispatch` 长连接拉取任务；该模型与 v7.1 及本设计 v1.1 不一致。

### 7.2 Worker 生命周期状态机

```
Created → Ready → Busy ⇄ Ready
              ↓
          Draining → Terminated
```

| 状态 | 含义 |
|------|------|
| Created | 进程已启动，未注册 |
| Ready | 已注册，可接收 Episode |
| Busy | 正在执行 Episode |
| Draining | 排空，完成在途 Episode 后退出 |
| Terminated | 已终止 |

### 7.3 断连与 WAL（Worker 侧）

网络分区时（方案 §10.5、§10.7）：

1. 检测控制面 gRPC 断开 → `WorkerGrpcServer` 仍可完成在途 `DispatchEpisode`（本地策略可配置拒绝新 Dispatch）
2. 在途 Episode 继续至完成或超时
3. 完成结果写入本地 WAL（**schema 见 §7.5，M1 冻结**）
4. 指数退避重连 ControlPlaneClient（1s → 2s → … → 30s 上限）
5. 重连后按幂等键重放 WAL；`ReportResult` 重复提交由 Server 去重

**实现顺序**：WAL **持久化实现** 可放在 M8；**record schema 与幂等语义必须在 M1 随 proto 一并冻结**，避免返工。

### 7.4 Heartbeat / fencing / epoch

| 字段 / 行为 | 说明 |
|-------------|------|
| `server_epoch` | Server HA 切换时递增；Worker 发现 epoch 变化则重新 `RegisterWorker` |
| `worker_id` + stale 检测 | Server 侧 fencing：过期 worker 注册不得接收新 Dispatch |
| `heartbeat_timeout` | 超时后 Server 将 Worker 标为不可用；Worker 侧重连并重新注册 |
| `next_heartbeat_interval_ms` | Server 在心跳流中下发，Worker 遵从此间隔 |

### 7.5 WAL record schema（M1 冻结）

每条 WAL 记录最小字段：

| 字段 | 说明 |
|------|------|
| `episode_id` | Episode 标识 |
| `attempt_id` | 重试尝试序号（Scheduler 分配） |
| `worker_id` | 执行 Worker |
| `dispatch_lease_id` | 与 `EpisodeRequest` 中租约一致（见 §7.7） |
| `server_epoch` | 写入时 Server epoch |
| `request_checksum` | 对应 `EpisodeRequest` 摘要 |
| `result_checksum` | 对应 `EpisodeResult` 摘要 |
| `status` | 完成状态 |
| `protobuf_payload` | 序列化 `EpisodeResult` |
| `created_at` | 写入时间 |
| `replay_state` | `pending` / `sent` / `acked` |

**幂等键（`ReportResult`）**：

```
idempotency_key = episode_id + attempt_id + worker_id
```

Server 对相同 `idempotency_key` 的重复 `ReportResult` 必须幂等应答（ACK 不重投 Episode）。

### 7.6 Stream 中断恢复语义（冻结方向）

| 场景 | 行为 |
|------|------|
| `DispatchEpisode` 流中途断开 | Worker 继续执行至完成或超时；进度以 WAL + 最终 `ReportResult` 为准 |
| `StreamReport` 部分丢失 | 允许；最终 `EpisodeResult` 为权威 |
| 重复 `DispatchEpisode`（同 `episode_id` + `attempt_id`） | Worker **必须幂等**：返回已有结果或拒绝并上报 `ALREADY_RUNNING` / `ALREADY_COMPLETED`（proto 枚举待定） |
| 重复 Dispatch **租约冲突** | 见 §7.7：非当前 `dispatch_lease_id` 拒绝执行 |

### 7.7 Dispatch 租约（lease / ownership，M1 冻结）

仅有 `episode_id` + `attempt_id` + 幂等键，在 **Scheduler HA / failover** 场景下仍可能出现「两个 Scheduler 同时认为拥有同一 Episode」的歧义。须在控制面引入 **租约所有权**。

#### 7.7.1 `EpisodeRequest` 最小字段（M1 proto 冻结）

| 字段 | 类型 | 说明 |
|------|------|------|
| `dispatch_lease_id` | string | 全局唯一；每次 Scheduler **权威下发** 时生成（新 UUID） |
| `lease_expire_at` | timestamp | 租约过期时间；过期后 Worker **不得** 为新 step 续执行（在途 step 可配置完成） |
| `scheduler_epoch` | uint64 | 与 `server_epoch` 对齐或细分；标识当前 Scheduler 领导者 |
| `dispatch_token` | bytes（可选） | HMAC/签名，防伪造 Dispatch（Phase 1+ 可强制） |

> `dispatch_lease_id` + `episode_id` + `attempt_id` 共同构成执行期 **ownership**；`idempotency_key` 仍用于 `ReportResult` 去重。

#### 7.7.2 Worker 执行规则

1. **接受 Dispatch**：`dispatch_lease_id` 未过期，且（无在途 lease **或** 与当前持有 lease 相同且 `attempt_id` 一致）。
2. **拒绝 Dispatch**：
   - 租约过期 → `LEASE_EXPIRED`
   - 已有不同 `dispatch_lease_id` 在执行同一 `episode_id`+`attempt_id` → `LEASE_CONFLICT`
   - `scheduler_epoch` / `server_epoch` 落后于 Worker 已注册 epoch → `STALE_SCHEDULER`
3. **执行中**：仅接受与 **当前持有 lease** 匹配的控制操作（MVP：无 `CancelEpisode` RPC 时，以 lease 校验 + 本地 `deadline` 为准；Phase 1+ 增加显式 cancel/preempt）。
4. **Scheduler failover**：新领导者必须使用 **新** `dispatch_lease_id`；若 Worker 仍在旧 lease 下执行，完成或失败后上报，Server 根据 `attempt_id` 与结果决定是否重投。

#### 7.7.3 HA 场景示例

```text
Scheduler A: Dispatch(ep-1, lease=L1)
Worker:      持有 L1，执行中

网络分区 + Scheduler failover

Scheduler B: Dispatch(ep-1, lease=L2)   # 新 lease
Worker:      若 L1 未结束 → 拒绝 L2（LEASE_CONFLICT）或 L1 已过期则接受 L2（策略写死一种）
```

**MVP 推荐策略（冻结）**：

- 同一 `episode_id` + `attempt_id` 下，Worker **仅承认最新**（`lease_expire_at` 最大或 `dispatch_lease_id` 字典序最大）的 lease 为权威；旧 lease 在途执行 **尽快终止** 并 `ReportResult(FAILED, reason=LEASE_SUPERSEDED)`。
- 避免双写：旧 lease 不得再提交成功态 `ReportResult`（仅允许 FAILED / 幂等 ACK）。

#### 7.7.4 与 WAL / 幂等关系

- WAL 记录必须含 `dispatch_lease_id`。
- `ReportResult` 携带 `dispatch_lease_id`；Server 拒绝与当前权威 lease 不一致的成功结果。

---

## 8. Mock Scheduler Gateway（初期协作开发）

### 8.1 背景

Worker Pool 与 Scheduler 分团队并行开发时，真实 UEnv Server 可能尚未就绪。本层需提供 **Mock Scheduler Gateway**，模拟 **UEnv Server / Scheduler 控制面** 行为：接受 Worker 注册与心跳，**作为 gRPC 客户端主动调用** Worker 的 `DispatchEpisode`，并接收 `ReportResult` / `StreamReport`。

> Mock **不是**「Worker 来 subscribe 拉任务」的反向 Server；它扮演的是 **调度方**，与生产 Scheduler 同角色。

### 8.2 职责

| 能力 | 说明 |
|------|------|
| 模拟注册应答 | 接受 Worker `RegisterWorker`（ControlPlane），返回合成 `worker_id` |
| 模拟心跳 | WorkerHeartbeat 双向流；可注入 `next_heartbeat_interval_ms`、`server_epoch` |
| **模拟调度下发** | 从 fixture 取 `EpisodeRequest`，**主动** `DispatchEpisode(worker_endpoint, …)` |
| 模拟资源目录 | 可选：维护内存 worker 表，模拟 `ListWorkers` 查询 |
| 模拟结果收集 | 接收 `ReportResult`；校验 `idempotency_key` 与 WAL 重放 |
| 故障注入 | 延迟、断连、重复 Dispatch、epoch 变化等（见 §8.6） |

### 8.3 部署模式

```
模式 A：独立 Mock Scheduler（推荐，M1）
  Worker（gRPC Server :50052）◄── DispatchEpisode ── Mock Scheduler（:50051）
  Worker ── RegisterWorker / Heartbeat / ReportResult ──► Mock Scheduler

模式 B：进程内 Mock ControlPlane（仅单元测试）
  Worker 内嵌 Mock ControlPlane + 外部测试驱动 Dispatch
```

### 8.4 与真实 Scheduler 切换

```bash
# Mock 模式（仅 Worker 团队本地）
UENV_SCHEDULER_MODE=mock
UENV_MOCK_EPISODE_FIXTURE=./fixtures/gsm8k_episode.pb

# 真实模式
UENV_SCHEDULER_MODE=remote
UENV_SERVER_ENDPOINT=localhost:50051
```

### 8.5 Mock 数据要求（GSM8K）

初期 fixture 至少覆盖：

- `env_type = "gsm8k"`（或 `"math"` + `dataset=gsm8k`）
- 含 `model_endpoint` 占位（可指向 mock LLM）
- `max_steps`、`timeout_seconds`、`reward_config`
- `episode_id`、`attempt_id`、`dispatch_lease_id`、`lease_expire_at`（供租约与 WAL 测试）
- 预期 `EpisodeResult`：status、trajectory 字段、checksum、`dispatch_lease_id`

### 8.6 Contract Chaos Tests（M1.7，协议鲁棒性）

Mock 阶段即应覆盖以下场景（供 M8 容错复用）：

| 场景 | 验证目标 |
|------|----------|
| duplicate `DispatchEpisode` | Worker 幂等 |
| unsupported `env_type` | 能力声明与拒绝语义 |
| capacity full | 背压 / `RESOURCE_EXHAUSTED` |
| stale `worker_id` | fencing |
| `server_epoch` 变化 | HA 后重新注册 |
| heartbeat timeout | 重连与再注册 |
| `ReportResult` retry | WAL + 幂等键 |
| partial stream interruption | `StreamReport` 中断后仍以 `ReportResult` 为准 |
| `lease_expired` / `lease_conflict` | 租约过期与 failover 后新 lease 冲突 |
| superseded lease | 旧 lease 在途时新 Dispatch，Worker 上报 `LEASE_SUPERSEDED` |

---

## 9. Phase 0 范围：仅 GSM8K

### 9.1 环境收敛

| 项 | Phase 0 | 后续 |
|----|---------|------|
| 接入环境 | **GSM8K（MathEnv）仅一种** | MATH、CodeEnv、AgentEnv 等按路线图扩展 |
| env_type 建议 | `gsm8k` | 与 UEnvHub manifest 对齐 |
| 训练场景 | 单轮验证型（问题 → 答案 → 规则奖励） | 多轮 / 工具 / MCP |
| 后端 | ProcessBackend | PodmanBackend |

### 9.2 GSM8K 插件交付物

- `plugins/gsm8k/`：manifest + 插件入口
- 实现：可验证奖励（规则 Reward，对齐方案 MathEnv）
- 单轮执行模式（方案 §4.4「单轮」模式）
- 单元测试 + 与 `MockSchedulerGateway` 的集成测试

### 9.3 验收标准（Worker 层）

1. Mock 模式下完整跑通：Register → **Scheduler 主动** Dispatch → Execute → Report
2. 预热池命中时，Episode 启动延迟显著低于冷创建（metrics / 日志可量化）
3. Episode 完成后实例经 cleanup 归还池中，`episode_count` 递增；无双分配
4. 日志符合 ADR-001（`.log` 文本，含 `trace_id` / `episode_id` / `worker_id`）
5. GSM8K 插件通过 **Protobuf/UDS** 子进程与 Rust 宿主通信正常
6. 无默认 `env.step()` 重试；M5+ metrics 可 scrape  
7. 插件 kill 后 Worker 存活；1 子进程 = 1 instance（§3.5、§6.4）

---

## 10. 可观测性

### 10.1 日志（Worker 组件）

Worker 进程日志写入 `/var/log/uenv/worker.log`（§2.2）；Mock Scheduler 写入 `/var/log/uenv/mock-scheduler.log`。运维使用 `tail -f` 实时跟踪，行内 `grep episode_id=` / `trace_id=` 检索。

| 级别 | 场景示例 |
|------|----------|
| ERROR | 插件加载失败、Episode 崩溃、WAL 写入失败 |
| WARN | 心跳延迟、预热池未命中、模型回调重试 |
| INFO | Episode 开始/完成、实例创建/归还、Worker 注册 |
| DEBUG | 调度评分（若本地 Mock）、step 级耗时 |
| TRACE | aRPC 调用细节（仅开发） |

### 10.2 Prometheus 指标（M5/M6 最小集，非 M9+）

以下指标 **不推迟到 M9+**；M5 起暴露 Episode 路径指标，M6 起暴露预热池指标，否则无法量化 warm pool 收益。

| 指标名（建议） | 阶段 | 说明 |
|----------------|------|------|
| `uenv_episode_total` | M5 | 完成 / 失败计数 |
| `uenv_episode_duration_ms` | M5 | Episode 耗时直方图 |
| `uenv_env_step_duration_ms` | M5 | `env.step` 耗时 |
| `uenv_model_callback_duration_ms` | M5 | 模型回调耗时 |
| `uenv_active_episode_count` | M5 | 当前活跃 Episode |
| `uenv_heartbeat_lag_ms` | M5 | 心跳滞后 |
| `uenv_warmup_pool_hit_total` | M6 | 预热池命中 |
| `uenv_warmup_pool_miss_total` | M6 | 预热池未命中 |
| `uenv_instance_pool_size{env_type,status}` | M6 | warm/active/idle |
| `uenv_wal_pending_records` | M8 | 待重放 WAL 条数 |

### 10.3 分布式追踪

- `trace_id` 从 `EpisodeRequest` 贯穿至 `EpisodeResult`
- gRPC metadata：`x-uenv-trace-id`、`x-uenv-span-id`
- 日志行内携带 `trace_id`，便于与 OTel 关联（日志本身非 JSON）

---

## 11. 容错（Worker 层）

### 11.1 重试边界（冻结）

**禁止默认重试 `env.step()`** — `env.step()` 不天然幂等（Agent 工具、外部 API、文件系统、浏览器、Sandbox、DB 等均可能产生副作用）。

| 操作 | 是否允许 Worker 内默认重试 |
|------|---------------------------|
| `env.step()` | **否** |
| external API（经 env） | **否** |
| model callback | 可以（可配置 `max_retries`） |
| RM 推理 | 可以 |
| read-only MCP tool | 可以（环境显式声明时） |

**Episode 级重试** 由 **Scheduler / UEnv Server** 统一控制（新 `attempt_id`），Worker 不自行对同一 Episode 做局部 step 重试循环。

### 11.2 分层策略

| 层级 | 场景 | 策略 |
|------|------|------|
| L1 | 控制面 gRPC 瞬断 | ControlPlaneClient 重连 + 退避；WAL 重放 `ReportResult` |
| L2 | 模型推理超时 | Worker 内可重试 model callback（非 env.step） |
| L2 | 工具/MCP 失败 | 记录错误，按环境策略跳过或失败；**不重试有副作用的 step** |
| L3 | Episode 失败 | 上报 Server；由 Scheduler 决定是否新 `attempt_id` 重投 |
| L1 | Worker OOM | 进程管理器重启；Server 侧摘除 |
| L2 | **插件子进程 crash** | 当前 Episode FAILED；**Worker 存活**；销毁实例，见 §6.4 |

数据完整性：`EpisodeResult.trajectory_checksum`（SHA-256），`integrity_verified=true`。

### 11.3 插件 vs Worker 故障分离

| | Worker 进程 | 插件子进程 |
|--|-------------|------------|
| 崩溃影响 | 整机 Worker 重启；所有在途 Episode 丢失 | **仅** 绑定该实例的 Episode FAILED |
| 恢复 | 重新 `RegisterWorker`；池重建 | `PluginHost` kill + 补池；**不**重启 Worker |
| step 重试 | — | **禁止**（§11.1） |
| 控制面 | gRPC Server + ControlPlane 中断 | 对 Scheduler **透明**（仅见 EpisodeResult） |

---

## 12. 配置参考（Worker 专用）

配置可通过 **CLI 参数**、**环境变量**、**YAML/JSON 配置文件**（§2.6 ADR-002）或内置默认值提供；优先级：**CLI 参数 > 环境变量 > 配置文件 > 默认值**。

**默认配置文件**：`/etc/uenv/worker.yaml`（或同目录 `worker.json`）；开发环境可用 `./uenv-worker.yaml`。

| 配置项 | YAML 键 | 类型 | 默认值 | 说明 |
|--------|---------|------|--------|------|
| `UENV_SERVER_ENDPOINT` | `server.endpoint` | string | `localhost:50051` | Scheduler / ControlPlane 地址 |
| `UENV_WORKER_LISTEN` | `worker.listen` | string | `0.0.0.0:50052` | Worker gRPC Server 监听（供 Dispatch） |
| `UENV_SCHEDULER_MODE` | `scheduler.mode` | string | `remote` | `remote` / `mock` |
| `UENV_ENV_TYPES` | `env.types` | string | `gsm8k` | **Phase 0 仅 gsm8k**（YAML 为数组） |
| `UENV_MAX_CONCURRENT` | `worker.max_concurrent` | int | 4 | 最大并发 Episode |
| `UENV_WARMUP_POOL_SIZE` | `pool.warmup_size` | int | 2 | 初期固定；后期由动态策略覆盖 |
| `UENV_MAX_IDLE_TIME` | `pool.max_idle_time` | int | 300 | 实例空闲回收（秒） |
| `UENV_COOL_TIMEOUT` | `pool.cool_timeout` | int | 60 | 冷却期（秒） |
| `UENV_MAX_EPISODE_COUNT` | `pool.max_episode_count` | int | 1000 | 单实例最大复用次数 |
| `UENV_BACKEND` | `env.backend` | string | `process` | `process` / `podman` |
| `UENV_PLUGIN_DIR` | `env.plugin_dir` | string | `./plugins` | 插件扫描目录 |
| `UENV_LOG_LEVEL` | `logging.level` | string | `INFO` | 日志级别 |
| `UENV_LOG_FILE` | `logging.file` | string | `/var/log/uenv/worker.log` | **文本 .log 路径** |
| `UENV_WAL_DIR` | `wal.dir` | string | `/tmp/uenv/wal` | 本地 WAL |
| `UENV_WORKER_ID` | `worker.id` | string | `auto` | Worker 标识 |

**CLI 等效示例**：

```bash
uenv-worker serve \
  --config /etc/uenv/worker.yaml \
  --log-level INFO \
  --log-file /var/log/uenv/worker.log
```

完整 schema 与 JSON 示例见 §2.6。

---

## 13. 建议 crate 结构

```
uenv-worker/                    # Rust 2024 主 crate
├── src/
│   ├── main.rs                 # CLI 入口：serve / version / health
│   ├── cli/                    # clap 子命令与全局 flags
│   ├── config/                 # YAML/JSON 加载 + env 映射（§2.6）
│   ├── runtime.rs              # WorkerRuntime
│   ├── control_plane/
│   │   └── client.rs           # RegisterWorker / Heartbeat / ReportResult
│   ├── grpc_server/
│   │   └── worker_service.rs   # DispatchEpisode / HealthCheck
│   ├── registry/
│   │   └── worker_pool.rs      # WorkerPoolRegistry（资源目录）
│   ├── mock/
│   │   └── scheduler_gateway.rs # Mock Scheduler（主动 Dispatch）
│   ├── episode/
│   │   ├── executor.rs
│   │   └── model_client.rs
│   ├── pool/
│   │   ├── warmup_pool.rs
│   │   └── warmup_sizer.rs     # 动态容量（Phase 1+）
│   ├── plugin/
│   │   ├── host.rs             # 进程级实例表；waitpid / 崩溃回收
│   │   ├── instance.rs         # instance_id / PID / UDS；§3.5
│   │   └── arpc/               # L2 MVP: Protobuf over UDS（与 proto/ 控制面分离）
│   ├── backend/
│   │   ├── process.rs
│   │   └── podman.rs
│   ├── wal/
│   ├── metrics/
│   └── logging/                # Linux .log sink
├── proto/                      # 与 Server 共享的 Protobuf
├── capnp/                      # Phase 1+ 插件 IDL（非 MVP）
└── plugins/                    # 内置或示例插件路径

plugins/gsm8k/                  # Phase 0 唯一环境插件
├── manifest.yaml
└── ...
```

---

## 14. 实现阶段与里程碑

| 阶段 | Worker Pool 交付 | 依赖 |
|------|------------------|------|
| **W0** | crate 骨架、CLI（`serve`）、Linux 日志、YAML/JSON 配置加载 | — |
| **W1** | `PluginHost` + GSM8K 插件（ProcessBackend） | GSM8K 环境实现 |
| **W1** | `WorkerGrpcServer` + `ControlPlaneClient` + Mock 主动 Dispatch | proto + WAL schema 冻结 |
| **W2** | `EpisodeExecutor` 单轮模式 + M1.7 Contract Chaos Tests | Protobuf fixture |
| **W3** | `WarmupPool`（固定容量 + §5.6 复用安全）+ M5/M6 最小 metrics | — |
| **W4** | 真实 UEnv Server 联调（Server 直连 Dispatch） | Server 就绪 |
| **W5** | `WalWriter` 持久化 + 断连重连（schema 已在 W1 冻结） | — |
| **W6+** | `WarmupSizer` 动态预热、PodmanBackend、Cap'n/cdylib | 历史指标采集 |

与方案里程碑对齐：Phase 0 验证（ROLL + 环境端到端）不阻塞于预热池（方案注明 Phase 1 再加预热池）；**本仓库 Worker 层仍提前实现预热池**，初期用固定参数，与 Scheduler 侧「预测预热」逻辑在联调后合并。

---

## 15. 与方案 v7.1 的差异汇总

| 主题 | 方案 v7.1 | 本文 v1.0（已废弃方向） | 本文 v1.1（冻结） |
|------|-----------|------------------------|-------------------|
| Worker 角色 | gRPC Server + 主动上报 | Worker 纯 Client subscribe 任务 | **Worker gRPC Server**；Server **主动** `DispatchEpisode` |
| Worker Pool | 实例池元数据 | 易演化为二次调度 | **Resource Registry only**；不转发 Episode |
| 环境插件 IPC | 多语言 | Cap'n + Proto + cdylib 并行 | **MVP：ProcessBackend + Proto/UDS 子进程** |
| 本地 step 重试 | 未强调 | Step 级重试 | **禁止默认 `env.step()` 重试**；Episode 重试归 Scheduler |
| WAL | §10.7 | M8 才设计 schema | **M1 冻结 schema**；M8 实现持久化 |
| 日志 | JSON（§14.1） | `.log` 文本 | **ADR-001 单行 k=v；`/var/log/uenv/<service>.log`；`tail -f` 运维** |
| 服务入口 | 未强调 | 隐含 main | **每服务 CLI 二进制 + `serve` 子命令（§2.5）** |
| 配置文件 | 未强调 | 仅 env | **ADR-002 YAML/JSON + env 映射（§2.6）** |
| Metrics | §14.2 | 推迟 M9+ | **M5/M6 最小集** |
| Phase 0 环境 | 多种 | 仅 GSM8K | **仅 GSM8K** |
| 框架语言 | Python 示例 | Rust 2024 | **Rust 2024** |
| 插件实例模型 | 未明确 | 隐含 1:1 | **1 子进程 = 1 instance**（§3.5） |
| Dispatch 所有权 | 未明确 | 仅幂等键 | **dispatch_lease_id + lease_expire_at**（§7.7） |
| 协议分层 | 未强调 | 隐含 | **L1 控制面 / L2 插件 IPC 严格隔离**（§2.4） |
| 插件 crash | 未明确 | 部分 | **§6.4 Plugin Failure Semantics** |

---

## 16. 参考资料

- [UEnv 方案 v7.1 PDF](./UEnv%20—%20下一代分布式训练环境框架方案-v7.1.pdf) — §4.4、§6.4、§7.1–7.3、§9.2–9.3、§10.5–10.7、§14、§15.3
- 环境开发模板：方案 §16.3（`edition = "2024"`、`cdylib` 插件）
