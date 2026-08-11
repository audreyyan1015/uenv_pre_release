# SWE-smith 昇腾 910C 集群并行实例容量估算报告

> 日期：2026-08-11
> 范围：16 台主机、共 128 张华为昇腾 910C（A3）卡；每台机器仅允许在 Docker 内运行、无 root 权限
> 结论性质：部署前容量估算，不替代目标机器上的实测压测

## 1. 结论摘要

SWE-smith 的测试容器本身不使用昇腾 NPU。NPU 只会影响同机 LLM 推理、模型服务或其他 AI 进程；SWE session 的主要瓶颈是 CPU、内存、磁盘 I/O、Docker daemon/containerd、PID/FD 和网络。

因此不能直接按“128 张卡 = 128 个 SWE 并发实例”估算。若每台主机有 8 张 910C，建议先按每主机以下安全档位规划：

| 档位 | 每实例资源预算 | 每主机建议并发 | 16 台总并发 |
|---|---:|---:|---:|
| 保守生产档 | 2 vCPU、4 GiB RAM、20 GiB writable disk | 8 | 128 |
| 稳态推荐档 | 2 vCPU、3 GiB RAM、15 GiB writable disk | 12 | 192 |
| 激进压测档 | 1 vCPU、2 GiB RAM、10 GiB writable disk | 16 | 256 |

在没有目标机器规格、Docker quota 和真实 pydicom/长尾仓库压测数据前，不建议把生产 admission 直接设到 256。第一阶段建议使用 **每主机 8 个、全局 128 个并发 SWE session**，通过资源采样再向 12/主机扩展。

## 2. 代码中的当前并发语义

当前 Worker 的 SWE session pool 容量为：

```text
swe_capacity = max(runtime_gateway.capacity, worker.max_concurrent)
```

当前生产配置：

```yaml
worker:
  max_concurrent: 4
runtime_gateway:
  capacity: 1
```

所以当前单 Worker 的 SWE pool 实际容量是 **4**，不是 Gateway 配置中的 1。`runtime_gateway.capacity=1` 主要影响 Gateway admission 上报和运行时配置，但池容量取两者较大值。扩容时必须同时检查：

- Worker `max_concurrent`
- Runtime Gateway `capacity`
- SWE instance pool capacity
- Agent pool 的 `max_concurrent_jobs`
- Server 注册到的 Worker `capacity`
- 每台主机的 Docker/CPU/RAM/磁盘上限

否则可能出现 Server 认为可接收的 Episode 数大于 Gateway 或 Docker 实际能承受的数量。

## 3. 单个 SWE-smith 实例资源预算

### 3.1 已有现场证据

当前 7143 A100 主机上，空闲或低负载 SWE 容器的 `docker stats` 观测约为：

```text
RSS 约 40 MiB - 175 MiB
CPU 接近 0%（采样时多数处于等待）
```

但这个数字不能作为测试执行峰值。它只反映容器空闲、等待 pytest 或已接近完成时的水位。测试启动、Python import、编译扩展、pytest 参数化、git 操作和日志输出都可能显著放大 CPU、RSS 和磁盘写入。

### 3.2 建议预算

对一个 SWE-smith session，建议先采用以下 admission 预算，而不是用空闲容器 RSS：

```text
CPU：      2 vCPU 硬预算
内存：     4 GiB 硬预算，3 GiB 为稳态目标
可写层：   15-20 GiB
临时目录： 2 GiB
PID：      256-512 个
打开文件： 512-1024 个
```

这里的可写层不是 Docker 镜像大小。镜像层应预先分发并复用；每个 session 需要额外为仓库修改、pytest cache（即使当前禁用 cache）、日志、编译产物和临时文件预留空间。若多个实例共享同一宿主机，Docker storage driver 的 copy-on-write 放大会使磁盘使用高于容器内部看到的文件大小。

### 3.3 资源构成

| 资源 | 主要消耗来源 | 风险 |
|---|---|---|
| CPU | pytest、编译、压缩、git、多个子进程 | CPU oversubscription 后吞吐和尾延迟急剧恶化 |
| RAM | Python/pytest、编译器、依赖导入、git、Docker daemon page cache | OOM 或宿主机 reclaim |
| 磁盘 | image layer、workspace diff、编译产物、日志、临时文件 | 容器创建失败、Docker daemon 变慢 |
| PID/FD | pytest 子进程、shell、docker exec、文件扫描 | 无法启动新任务或 SSH/daemon 异常 |
| NPU | 默认不由 SWE 测试容器使用 | 若同机部署 LLM 服务，需要单独切分资源 |

## 4. 集群容量计算

若每台主机有 `C` 个可分配 CPU 核、`M` GiB 可用内存、`D` GiB 可用 Docker writable disk，单机安全实例数为：

```text
N_host = min(
  floor((C - C_reserve) / cpu_per_instance),
  floor((M - M_reserve) / ram_per_instance),
  floor((D - D_reserve) / disk_per_instance),
  pid_limit / pid_per_instance,
  fd_limit / fd_per_instance,
  docker_limit,
  gateway_limit,
  agent_limit
)
```

建议预留：

```text
CPU：至少 25%，若同机有 LLM/数据服务则至少 35%
内存：至少 20%，若 Docker daemon 和编译任务较多则 25%
磁盘：至少 20%，并设置硬上限和清理策略
```

以每台 8 卡主机、SWE 不占 NPU、每实例按 2 vCPU/4 GiB/20 GiB 预算为例：

| 假设每台主机规格 | CPU 可给 SWE | 可用 RAM | 磁盘可给 SWE | 资源推导并发 | 建议并发 |
|---|---:|---:|---:|---:|---:|
| 16C / 64 GiB / 500 GiB | 12C | 48 GiB | 400 GiB | min(6, 12, 20)=6 | 4-6 |
| 32C / 128 GiB / 1 TiB | 24C | 96 GiB | 800 GiB | min(12, 24, 40)=12 | 8-12 |
| 64C / 256 GiB / 2 TiB | 48C | 192 GiB | 1.6 TiB | min(24, 48, 80)=24 | 16-24 |

因此，在推荐的 **8/主机** 起步档下，16 台机器全局为 **128 个并发实例**。如果实际机器至少是 32C/128GiB，并且 Docker/磁盘/FD 实测稳定，可以提高到 **12/主机、192 个全局并发**。只有在 64C/256GiB 级别并经过长尾压测后，才考虑 **16/主机、256 个全局并发**。

## 5. 910C 和 Docker 约束

### 5.1 NPU 不是 SWE session 的直接容量单位

SWE-smith 流程主要是：

```text
OpenHands/Agent -> Runtime Gateway -> Docker container -> shell/pytest -> reward
```

该链路不需要把 910C 映射进每个测试容器。若 Agent 的 LLM 服务部署在同一批机器上，则需要把 NPU、LLM CPU/RAM 和 SWE CPU/RAM 做显式资源切分；否则 LLM 推理抖动会污染 SWE 容量测量。

### 5.2 无 root 权限的影响

主机用户不能直接依赖裸机 Docker 管理命令或安装驱动。建议由平台侧预先提供：

- 具备 Docker socket 或受控 Docker API 的运行容器
- 映射好的 `/dev/davinci*`、驱动库和 CANN/torch-npu 运行时（仅 LLM 容器需要）
- 每个 Worker 容器的 CPU、内存、PID、FD、磁盘 quota
- 可访问的镜像仓库或预分发镜像 tar
- 统一的日志、artifact 和 Docker 清理 sidecar

如果 Worker 自己还要在 Docker 内再启动 SWE Docker，必须确认 Docker-in-Docker 或受控 Docker socket 方案；普通无 root 容器无法凭空创建同级宿主 Docker 容器。这个约束可能比 CPU/RAM 更早成为实际并发上限。

### 5.3 昇腾 runtime 不应默认注入 SWE 容器

SWE 测试容器应默认不挂载 NPU 设备、不加载 CANN、不启动 torch-npu。这样可以降低容器启动开销、设备锁竞争和驱动异常对测试的影响。只有需要 NPU LLM 推理的独立服务才应通过平台编排层分配 910C。

## 6. 推荐扩容步骤

### 阶段 A：8/主机

```text
16 台 × 8 = 128 个全局并发 session
```

每台只注册一个 Worker 或一个明确的 Worker shard，`max_concurrent=8`，Gateway capacity 与之对齐。先跑 200-500 个跨仓库样本，记录每 30 秒：CPU、RSS、available memory、Docker 容器数、磁盘、PID、FD、pytest timeout 和完成率。

### 阶段 B：12/主机

仅当以下条件连续满足至少 30 分钟才扩展：

- CPU pressure P95 < 70%
- available memory > 25%
- Docker writable disk 使用率 < 70%
- 无 OOM、PID/FD exhaustion、Docker daemon timeout
- episode timeout/error rate 无明显上升
- p95/p99 episode duration 未因并发翻倍而失控

目标：

```text
16 台 × 12 = 192 个全局并发 session
```

### 阶段 C：16/主机

仅作为压测上限，不建议直接作为生产默认：

```text
16 台 × 16 = 256 个全局并发 session
```

必须先完成单机长稳、随机仓库混合、pydicom/pyparsing 等长测试样本和异常清理验证。

## 7. 需要在目标集群补齐的实测项

目前缺少目标 16 台机器的以下数据，因此本报告不能给出唯一精确数字：

- 每台主机 CPU 核数和可给 Docker 的 quota
- 每台可用内存、swap/cgroup memory.max
- Docker daemon 是否允许 socket 复用或 DinD
- Docker storage driver、writable layer quota 和实际剩余磁盘
- PID/FD/conntrack/端口限制
- 每个容器是否有 CPU/memory/pids quota
- CANN/910C 是否与 SWE 容器同机部署
- OpenHands Agent 是每卡一个、每机一个，还是独立 Agent 池
- pydicom、pyparsing、django、numpy/scipy 等长尾仓库的 CPU/RSS 峰值

建议先对每台机器执行并保存：

```bash
nproc
free -h
df -h /
cat /sys/fs/cgroup/cpu.max 2>/dev/null || true
cat /sys/fs/cgroup/memory.max 2>/dev/null || true
cat /sys/fs/cgroup/pids.max 2>/dev/null || true
docker info
docker system df
ulimit -n
ulimit -u
```

## 8. 最终建议

在现有信息下，建议把 **128 个并发 SWE-smith 实例**作为第一版集群容量目标，而不是 128 张卡对应 128 个 NPU 任务。实际生产 admission 先限制为每台 8 个；在目标机完成资源采样后，再决定是否提升到 192。256 只能作为经过长稳压测的理论上限。

本估算的最大不确定项不是 NPU 显存，而是无 root Docker 运行模式、CPU/内存 quota、Docker writable layer 和长尾 pytest 的峰值资源消耗。

7143：
  初始 12-16 个
  清理磁盘并压测后再到 24 个

121.89.82.128：
  初始 6 个
  压测稳定后再到 8 个

两机第一阶段总并发：
  18-22 个

两机稳定扩展目标：
  32 个
