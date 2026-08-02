# Qwen3.6-35B 当前训练配置显存分析

本文只按当前已经跑通的 UEnv + VeRL 同步 GRPO 配置分析显存，不讨论其他并行或 offload 组合。

## 1. 当前运行配置

| 项目 | 当前值 |
|---|---|
| 模型 | `Qwen/Qwen3.6-35B-A3B` |
| 参数量 | 日志显示 `35.11B parameters` |
| GPU | `8 x A100 80GB`，单卡可见约 `79.25GB` |
| 训练框架 | VeRL `main_ppo` + UEnv AgentLoop |
| 并行方式 | actor/ref 使用 FSDP；rollout 使用单个 vLLM endpoint，`ROLLOUT_TP=8` |
| 精度 | BF16 |
| batch | `TRAIN_BATCH_SIZE=4`，`ROLLOUT_N=2`，每 step 8 条 episode |
| micro batch | actor/ref/rollout logprob 均为 `1` |
| optimizer offload | `actor_rollout_ref.actor.fsdp_config.optimizer_offload=True` |
| param offload | actor/ref 均为 `False` |
| rollout cache | `ROLLOUT_GPU_MEMORY_UTILIZATION=0.20` |
| sleep/free-cache | `ROLLOUT_ENABLE_SLEEP_MODE=False`，`ROLLOUT_FREE_CACHE_ENGINE=False` |
| rollout 长度配置 | `DATA_MAX_RESPONSE_LENGTH=6144`，`max_model_len=16384`，`max_num_batched_tokens=16384` |

最近一次可用证据：

```text
/data/ronghao/uenv/uenv-bridge/temp/logs/verl_layer4_agent_loop/swebenchpro_4sample_grpo_35b_1step_sync_resp6144_iter2_clamp6144_20260801_103050.log
```

该日志完成 1 个训练 step，没有 OOM。需要注意：本次 SWE 训练链路中 Worker 没有返回真实 `response_ids`，VeRL 侧实际使用 pad fallback，因此日志中的 `response_length/mean=1.0`。所以这里的显存峰值是“当前链路实测峰值”，不能直接外推为真实 6144 token 输出时的峰值。

## 2. 参数显存账本

按 `35.11B` 参数、BF16 计算：

| 项目 | 总量 | 8 卡分片后单卡 |
|---|---:|---:|
| BF16 权重 | 约 `70.22GB`，即 `65.40GiB` | 约 `8.78GB`，即 `8.17GiB` |
| BF16 梯度 | 约 `70.22GB`，即 `65.40GiB` | 约 `8.78GB`，即 `8.17GiB` |
| Adam m/v FP32 | 约 `280.88GB`，即 `261.59GiB` | 约 `35.11GB`，即 `32.70GiB` |

当前配置开启了 optimizer CPU offload，因此 Adam m/v 的主压力转移到 CPU。最近日志中 `actor/perf/cpu_memory_used_gb=104.46`，符合 optimizer offload 后 CPU 内存显著上升的现象。

## 3. 当前各阶段单卡显存峰值

当前日志没有打开逐 phase 的 torch memory profiler，因此只有 FSDP 初始化点和 step 级最大值是实测值。下面的阶段峰值是按当前配置推导的理论范围，并用最近一次实测峰值校准。

### 3.1 基础常驻项

| 项目 | 单卡显存估算 | 说明 |
|---|---:|---|
| actor BF16 权重 shard | `~8.17GiB` | 35.11B BF16 权重按 8 卡 FSDP 分片。 |
| ref BF16 权重 shard | `~8.17GiB` | 当前 ref `param_offload=False`，常驻 GPU。 |
| vLLM rollout 权重 shard | `~8.17GiB` | rollout 使用 `TP=8`，权重 shard 大小与 `gpu_memory_utilization` 基本无关。 |
| vLLM KV/cache pool | 数 GB 到十余 GB | `gpu_memory_utilization` 主要影响 KV/cache 可分配空间；具体值由 vLLM 启动 profile、剩余显存、block 管理和 Mamba cache 共同决定，不能直接用 `79.25GB * util` 当作 vLLM 总显存。 |
| Ray/CUDA/allocator 基础开销 | `~2-6GB` | 取决于进程、通信库和 PyTorch/vLLM allocator 预留。 |

因此，在不考虑当前阶段临时张量时，当前配置的单卡基础占用约为：

```text
actor shard 8.17
+ ref shard 8.17
+ vLLM weight shard 8.17
+ vLLM KV/cache pool 数 GB 级
+ runtime overhead 2~6
= 30~40GB
```

这里要把 vLLM 权重和 KV/cache 分开看：权重 shard 是固定项，KV/cache 才是随 `gpu_memory_utilization`、`max_model_len`、`max_num_batched_tokens` 和运行时 profile 变化的部分。

### 3.2 Rollout 阶段

rollout 阶段主要是 vLLM 生成和 UEnv/Worker 环境交互。当前配置没有开启 sleep/free-cache：

```text
ROLLOUT_ENABLE_SLEEP_MODE=False
ROLLOUT_FREE_CACHE_ENGINE=False
```

因此 rollout 后 vLLM 权重/cache 不会主动释放，后续 `old_log_prob`、`ref`、`update_actor` 阶段仍要和 vLLM 的显存预算共存。

| 组成项 | 单卡显存估算 |
|---|---:|
| 基础常驻项 | `~30-40GB` |
| rollout 阶段额外临时量 | `~1-5GB` |
| 理论活跃峰值 | `~31-45GB` |
| allocator reserved 口径 | 可能接近 `45-60GB` |

当前 SWE smoke 中 VeRL 侧实际 `response_length/mean=1.0`，所以 rollout 的真实 token cache 压力偏低。若 Worker 后续返回真实长 trajectory，并且 rollout 侧实际生成接近 `4096/6144` tokens，rollout 阶段会更接近 vLLM 预算上限，step 时间和 reserved 显存都会继续上升。

### 3.3 Actor Update 阶段

`update_actor` 是真正执行 PPO actor 前向、反向和 optimizer step 的阶段。当前 `optimizer_offload=True`，但 offload 的含义不是 optimizer 永远不进 GPU；VeRL 的 FSDP engine 在 train mode 进入时会按需把 optimizer state 加载到 GPU，退出后再 offload 回 CPU。因此这个阶段需要同时考虑：

| 组成项 | 单卡显存估算 |
|---|---:|
| 基础常驻项 | `~30-40GB` |
| actor 梯度 shard | `~8.17GiB` |
| activation / logits / loss 临时张量 | 当前短输出 `~1-5GB`；真实长输出可能显著增加 |
| FSDP all-gather / reduce-scatter 临时 buffer | `~1-4GB` |
| optimizer state 短时 GPU staging | 理论上最高可到 `~16-33GiB`，实际取决于 optimizer state dtype、lazy 初始化和是否与 activation 峰值重叠 |

当前短输出 smoke 的实测 step 级峰值是：

```text
actor/perf/max_memory_allocated_gb = 42.85
actor/perf/max_memory_reserved_gb  = 60.39
```

这说明在这次运行中，`activation`、`optimizer state staging`、`vLLM budget` 和 FSDP 通信临时量没有以最坏情况完全叠加。按当前短输出链路，`update_actor` 的实际活跃峰值约为 `43GB`；按真实长 SWE trajectory 估算，`update_actor` 更合理的风险区间是：

```text
短输出实测：allocated ~= 43GB，reserved ~= 60GB
真实长输出理论风险：allocated ~= 55-75GB
```

如果继续放大 `DATA_MAX_RESPONSE_LENGTH`、`max_model_len`、`TRAIN_BATCH_SIZE` 或 `ROLLOUT_N`，`update_actor` 会成为最主要的 OOM 风险阶段。

### 3.4 Update Weights 阶段

`update_weights` 不是 optimizer step；optimizer step 已经在 `update_actor` 内完成。这个阶段的作用是把更新后的 actor 权重同步给 vLLM rollout engine。

当前日志中 `checkpoint_engine.backend='naive'`，属于 colocated trainer/rollout 的本地同步路径。VeRL 会从 FSDP actor 中取 `state_dict()`，再通过 bucket/IPC 逐批写入 vLLM。当前默认：

```text
actor_rollout_ref.rollout.checkpoint_engine.update_weights_bucket_megabytes = 2048
```

因此这个阶段不是单卡完整物化 35B 权重，而是逐 tensor / bucket 同步。主要峰值来自：

| 组成项 | 单卡显存估算 |
|---|---:|
| 基础常驻项 | `~30-40GB` |
| 同步 bucket | `~2GB` |
| 当前被同步的大 tensor 临时量 | 通常 `~0.5-2GB`，MoE 大 tensor 会更高 |
| 可能残留的梯度 / allocator 缓存 | `~0-8GB` |
| vLLM load_weights / IPC 临时开销 | `~1-4GB` |

所以在当前 no-sleep、`gpu_memory_utilization=0.20` 配置下，`update_weights` 的理论活跃峰值大致为：

```text
30~40
+ 2
+ 0.5~2
+ 0~8
+ 1~4
= 34~56GB
```

这与当前 step 级 `reserved ~= 60.39GB` 是一致的。

`update_weights` 的危险配置是开启 sleep/free-cache 后再提高 vLLM KV/cache 配额。这里的关键不是 `resume weights`，也不是 actor weight sync 本身；这两步和 `gpu_memory_utilization` 的关系不大。真正随 `gpu_memory_utilization` 变化的是最后的 `resume kv_cache`。

开启 sleep/free-cache 后，`update_weights` 附近的流程可以理解为：

```text
resume weights
-> sync actor weights to vLLM
-> resume kv_cache
```

其中前两步的权重显存基本固定，第三步会按更高的 KV/cache 配额重新申请 cache。如果 `gpu_memory_utilization` 从 `0.20` 提高到 `0.30`，变化主要体现在 KV/cache pool 变大，而不是 vLLM 权重变大。由于 `resume kv_cache` 发生在权重同步之后，PyTorch/vLLM allocator 中可能还残留同步 bucket、FSDP state_dict 临时 tensor、梯度缓存和碎片化预留，此时重新申请更大的 KV/cache pool 就更容易触发 OOM。

因此历史上 `sleep/free-cache + util=0.30` 的 OOM 更准确地说是：

```text
高 util 带来的更大 KV/cache 申请
+ update_weights 后尚未完全回落的临时显存和 allocator reserved
+ 碎片化
=> resume kv_cache / wake_up 附近 OOM
```

### 3.5 当前阶段汇总

| 阶段 | 理论活跃峰值 | 当前实测校准 |
|---|---:|---|
| rollout | `~31-45GB` | 未单独打点，受 step 总峰值约束 |
| actor update | 短输出 `~43GB`；长输出风险 `~55-75GB` | step 实测 `allocated ~= 42.85GB` |
| update weights | `~34-56GB` | 未超过 step 实测 `reserved ~= 60.39GB` |

本次最终 step 级显存指标为：

```text
actor/perf/max_memory_allocated_gb = 42.85
actor/perf/max_memory_reserved_gb  = 60.39
actor/perf/cpu_memory_used_gb      = 104.46
```

因此，按当前已经跑通的配置，单卡 GPU 显存安全余量大约为：

```text
79.25GB - 60.39GB = 18.86GB
```

这里使用 `reserved` 计算更保守，因为 PyTorch/vLLM allocator 会预留未立即释放的显存。

## 4. 当前配置下的结论

| 结论 | 说明 |
|---|---|
| 当前配置不是 optimizer step 必然 OOM | 已完成 1-step smoke，step 级 `reserved` 约 `60.39GB`，低于 A100 80GB 上限。 |
| optimizer offload 是必要余量来源 | 若 Adam m/v 留在 GPU，单卡会多出约 `32.70GiB` 压力，容易超过 80GB。 |
| 当前最大风险不是“完整 35B 权重单卡物化” | FSDP 不会在 optimizer step 长期把完整 35B 权重放到单卡；风险主要来自 actor/ref/rollout 共存、长序列 activation、vLLM cache、权重同步临时 buffer 叠加。 |
| 真实长 response 还需要重新测 | 这次实际 `response_length=1`，若 Worker 返回真实 4096/6144 token trajectory，activation、logprob 和 rollout cache 压力都会明显增加。 |

## 5. 后续测量建议

正式 SWE 训练前，建议在 Worker 能返回真实 `response_ids` 后，用同一配置重新做 1-step smoke，并记录：

| 观测项 | 用途 |
|---|---|
| `actor/perf/max_memory_allocated_gb` | 判断真实训练计算峰值 |
| `actor/perf/max_memory_reserved_gb` | 判断 allocator 保守显存占用 |
| `response_length/mean/max/clip_ratio` | 判断这次是否真的覆盖长输出 |
| `timing_s/gen/ref/old_log_prob/update_actor/update_weights` | 判断瓶颈阶段 |
| 是否出现 `CUDA out of memory`、`cumem_allocator`、`wake_up` | 判断是否是 vLLM cache/权重同步相关 OOM |
