# NexRL 调研与最小样例规划

> 日期：2026-07-26
> 阶段：框架调研与最小训练样例验证
> 目标：先不接入 UEnv，优先跑通 NexRL 自身的最小 GSM8K 1step 训练样例，再判断其架构是否适合作为 UEnv 后续 step 级并行 / 服务化训练方向的候选。

## 1. 背景与目标

当前 UEnv 已经完成 VeRL 链路接入、五类 benchmark 基线评测，以及对 VeRL / ROLL 的 step 级并行初步调研。下一步希望探索 NexRL，是因为 NexRL 的核心设计与我们在 UEnv 中遇到的问题高度相关：

1. UEnv 的环境执行、模型推理、训练更新天然分布在不同组件中，需要稳定的服务化边界。
2. 我们希望后续支持更灵活的 rollout / training 解耦，而不仅限于某一个 RL 框架内部的同步 step。
3. NexRL 明确提出 Training-as-a-Service、Rollout-as-a-Service、Agent Service、TrajectoryPool 和 WeightSyncController，这些组件与 UEnv 的 Server / Worker / Adapter / Gateway 分层有较强对应关系。

本阶段不急于把 UEnv 接入 NexRL。优先目标是回答三个问题：

1. NexRL 官方最小训练链路能否在本地资源上跑通。
2. NexRL 的 rollout、trajectory pool、training service、weight sync 之间的数据契约是什么。
3. 如果未来接入 UEnv，Adapter 需要接在哪一层、需要新增哪些字段、会不会比 VeRL/ROLL 更适合异步训练。

## 2. NexRL 架构要点

NexRL 的 README 将其定位为生产级、分布式 LLM 后训练框架，核心特点是“极致松耦合”和“面向服务”。官方文档把组件划分为：

| 组件 | 职责 | 与 UEnv 的潜在对应 |
|---|---|---|
| `DataLoader` | 提供训练样本 | VeRL dataset / UEnv benchmark dataset |
| `RolloutWorker` | 执行环境交互与采样 | UEnv Adapter 发起 EpisodeRequest 的位置 |
| `TrajectoryPool` | 收集 trajectory，并按 batch 输出给 Trainer | UEnv 后续异步 episode/result pool 的参考 |
| `Trainer` | 执行 GRPO / OPD 等算法逻辑 | VeRL / ROLL trainer 的对应层 |
| `WeightSyncController` | 协调训练权重同步到推理服务 | UEnv 中模型版本和 gateway 权重版本管理的参考 |
| `Inference Service` | 统一 OpenAI-compatible 接口，支持 SGLang / vLLM / TGI 等 | Adapter Model Gateway / vLLM endpoint |
| `Train Service` | 统一 `forward()` / `forward_backward()` API，适配 FSDP / Megatron / Tinker / Weaver | 训练框架后端服务化接口 |
| `Agent Service` | Agent 可直接把 trajectory 推入 TrajectoryPool | UEnv Worker / agentic env 未来接入点 |

从本地代码看，NexRL 的 controller 主流程是：

```text
DataLoader
  -> RolloutWorker 生成 trajectory
  -> TrajectoryPool 聚合 trajectory
  -> Trainer 从 pool 取 batch 并调用 Train Service 更新
  -> WeightSyncController 协调新权重给 Inference Service
```

这和 VeRL 的“一次 main_ppo 进程内组织 rollout + update”不同。NexRL 更强调组件通过服务/API 解耦，因此编排复杂度更高，但对于跨机器、跨服务、多 agent、多 inference backend 的场景更自然。

## 3. 与 VeRL / ROLL 的差异

| 维度 | VeRL | ROLL | NexRL |
|---|---|---|---|
| 默认使用方式 | 一个 trainer 入口组织 actor/ref/rollout/reward | 配置化 RL 训练框架，内置 sync / async rollout / async training 等模式 | 多服务组合，需要 controller、rollout worker、train service、inference service 等组件协作 |
| step 级并行 | 提供 one-step off-policy、fully async experimental 路径 | 有 sync、async rollout、async training 等配置路径 | 通过 TrajectoryPool / TrainBatchPool / service 解耦天然支持 rollout 与 training 分离 |
| 推理服务抽象 | 常由框架内部启动 vLLM/SGLang 或由配置指定 | 类似，依赖框架配置和 worker 布局 | Inference Service 明确使用 OpenAI-compatible API，可切换 SGLang/vLLM/TGI |
| 训练后端抽象 | 与 VeRL 自身实现绑定较紧 | 与 ROLL 训练组件绑定 | Train Service 通过标准 API 适配 FSDP、Megatron、Tinker、Weaver |
| Agent 接入 | 需要接 AgentLoop / rollout manager | 依赖 ROLL rollout worker/task abstraction | Agent Service / RolloutWorker 是一等组件，agent 可以直接推 trajectory |
| 上手复杂度 | 低，一个脚本可启动完整训练 | 中等，配置项较多但仍偏单框架 | 高，需要理解并管理多个服务和配置 |
| 对 UEnv 的启发 | 可作为当前主线 | 可作为异步并行对比基线 | 可作为“服务化 UEnv RL 系统”的长期参考 |

结论：NexRL 不一定比 VeRL/ROLL 更适合快速做单机 baseline，但它更适合调研“训练、推理、环境执行解耦后的系统架构”。这也是我们下一阶段应该先跑最小样例，而不是立即接入 UEnv 的原因。

## 4. 本地 NexRL 状态

当前 NexRL 放在：

```text
/data/ronghao/third_party/NexRL
```

本地已经存在最小 GSM8K smoke 目录：

```text
/data/ronghao/third_party/NexRL/local_gsm8k_smoke
```

其中包含：

| 文件 | 作用 |
|---|---|
| `gsm8k_1sample.parquet` | 1 条 GSM8K 样本 |
| `gsm8k_1step_direct_zmq.yaml` | self-hosted GRPO 1step 配置，训练服务走 direct-zmq |
| `run_gsm8k_1step_direct_zmq.sh` | 使用 podman 启动 mock OpenAI server、NexRL train service API server、FSDP worker 和 `nexrl.main` |
| `mock_openai_server.py` | OpenAI-compatible mock endpoint，用于不依赖真实模型的链路自检 |
| `gsm8k_1step_mock_train.yaml` / `run_gsm8k_1step_mock_train.sh` | mock train 方向的备用 smoke |

这说明当前最小复现路线已经具备雏形：先用 mock inference 跑通 NexRL controller -> rollout -> trajectory pool -> train service -> trainer；再把 mock inference 替换为真实 vLLM endpoint。

## 5. 最小复现实验设计

### 5.1 阶段 A：NexRL 自身 mock smoke

目标：证明 NexRL 组件能启动，数据能走完整闭环。

建议配置：

| 项 | 值 |
|---|---|
| 数据集 | GSM8K 1 条样本 |
| rollout repeat | 2 |
| train steps | 1 |
| inference | `mock_openai_server.py` |
| train backend | self-hosted direct-zmq |
| GPU | 1 张即可 |
| weight sync | `skip_weight_sync=true` / `sync_mode=no-sync` |

预期结果：

1. `api_server.log` 有 health 和 worker group 信息。
2. `worker.log` 中 FSDP worker 成功注册。
3. `nexrl.log` 中 controller 完成 1 个训练 step。
4. `TrajectoryPool` 能收到 2 条 rollout 结果并组成训练 batch。

### 5.2 阶段 B：真实 vLLM inference smoke

目标：不接入 UEnv，先让 NexRL 真实调用模型完成 GSM8K 1step。

建议修改：

| 项 | mock smoke | 真实 vLLM smoke |
|---|---|---|
| `service.inference_service.base_url` | mock OpenAI server | 本机 vLLM / Adapter Gateway |
| `service.inference_service.model` | `nexrl-mock-math` | 实际 served model name |
| `service.inference_service.max_tokens` | 128 | 512 或 1024 |
| `rollout_worker.temperature` | 0.0 | 0.0 |
| `data.batch_size` | 1 | 1 |
| `trajectory_pool.batch_size` | 2 | 2 |

需要重点检查：

1. NexRL 的 OpenAI client 默认会请求 `logprobs=True`。
2. vLLM 需要返回 token ids / logprobs，否则 `nexrl_train.response_tokens` 或 `response_logprobs` 可能为空。
3. Qwen chat template、reasoning parser、tool parser 是否匹配。

### 5.3 阶段 C：扩大到小批量 1step / 10step

目标：观察 NexRL 在真实模型下的吞吐、显存占用和训练时间。

建议矩阵：

| 实验 | 数据量 | train steps | GPU | 关注指标 |
|---|---:|---:|---:|---|
| C1 | 8 条 | 1 | 1 | 是否稳定完成 |
| C2 | 32 条 | 5 | 1-2 | rollout 与 train 时间占比 |
| C3 | 128 条 | 10 | 4-8 | trajectory pool 是否堆积、weight sync 是否成为瓶颈 |

指标建议：

1. end-to-end step time
2. rollout latency / throughput
3. train batch wait time
4. train update time
5. trajectory pool size
6. weight sync time
7. GPU utilization

## 6. 未来接入 UEnv 的候选方案

### 6.1 方案一：UEnv 替换 RolloutWorker 内的环境执行

NexRL 保留 controller、trajectory pool、trainer 和 train service。我们实现一个 UEnv RolloutWorker：

```text
NexRL RolloutWorker
  -> UEnv Adapter/Core
  -> UEnv Server/Worker
  -> Model Gateway/vLLM
  -> EpisodeResult
  -> NexRL Trajectory
  -> TrajectoryPool
```

优点：

1. 对 NexRL 架构改动最小。
2. UEnv 仍然作为环境执行层，不接管训练。
3. 可以复用 NexRL 的 TrajectoryPool 和 TrainBatchPool。

挑战：

1. 需要把 `EpisodeResult` 转成 NexRL `Trajectory`，字段包括 prompt token、response token、response logprob、reward、finish_reason、metadata。
2. UEnv 当前需要保证 worker 返回 token-level logprob，否则 NexRL 的训练样本不完整。
3. 模型版本需要和 NexRL 的 WeightSyncController 对齐。

### 6.2 方案二：UEnv Worker/Agent 直接推 trajectory 到 NexRL Agent Service

Worker 不再只返回给 Adapter，而是直接把 trajectory 推给 NexRL 的 Agent Service / TrajectoryPool。

优点：

1. 更符合 NexRL 的 Agent Service 设计。
2. 对异步、多 agent、多环境比较自然。
3. Adapter 可以变薄，主要负责协议转换和元数据对齐。

挑战：

1. UEnv Server/Worker 需要感知 NexRL Agent Service。
2. request/result 生命周期会从“同步返回”变成“提交任务 + 异步推送 trajectory”。
3. 需要设计去重、重试、失败回收和 run_id / episode_id / trajectory_id 对齐。

### 6.3 方案三：NexRL 只作为长期架构参考

短期仍用 VeRL / ROLL 完成训练与异步并行实验，只把 NexRL 的设计吸收到 UEnv：

1. 引入更明确的 trajectory pool。
2. 把 model endpoint / train backend / rollout worker 的服务边界文档化。
3. 参考 WeightSyncController 处理模型版本和推理端冻结。

这是最稳妥的路线。它不会打断当前 VeRL 主线，也能为未来跨框架统一提供结构参考。

## 7. 关键挑战

| 挑战 | 说明 | 优先级 |
|---|---|---:|
| 组件启动复杂 | NexRL self-hosted 不是单一 trainer 脚本，涉及 API server、GPU worker、inference service、controller | P0 |
| logprob 契约 | NexRL 训练样本需要 response token / logprob，UEnv Worker 和 Gateway 必须稳定透传 | P0 |
| 权重同步 | NexRL 有 WeightSyncController，UEnv 的 gateway/model version 需要与之对齐 | P0 |
| 资源编排 | 官方生产路径依赖 Kubernetes、Volcano、高性能共享存储；本地 podman 只能做 smoke | P1 |
| 多模型/多任务 | NexRL 用 `model_tag/identifier` 区分模型，UEnv 需要把 run_id、episode_id、model version 映射清楚 | P1 |
| 和 VeRL/ROLL 对比口径 | 需要统一模型、数据、batch、GPU 切分、max tokens、thinking 设置，否则时间指标不可比 | P1 |

## 8. 建议推进计划

### 第一步：确认本地 smoke 可复现

运行目标：

```bash
cd /data/ronghao/third_party/NexRL
./local_gsm8k_smoke/run_gsm8k_1step_direct_zmq.sh
```

产物位置：

```text
/data/ronghao/third_party/NexRL/logs/gsm8k_1step_direct_zmq/<RUN_ID>/
```

成功标准：

1. `nexrl.log` 出现 training completed。
2. `worker_group.json` 能看到 worker endpoint。
3. 没有 controller / worker / api_server 异常退出。

### 第二步：替换为真实 vLLM endpoint

在最小配置中替换 inference endpoint，不接 UEnv：

```text
service.inference_service.base_url = http://127.0.0.1:<vllm_port>
service.inference_service.model = <served_model_name>
```

成功标准：

1. 真实模型完成 GSM8K 1step。
2. 返回中有 `response_tokens` 和 `response_logprobs`。
3. trainer 能完成一次 update。

### 第三步：对齐 VeRL baseline 的小规模配置

把 NexRL 配置调成与 VeRL smoke 接近：

| 参数 | 建议 |
|---|---|
| 模型 | 当前 UEnv 基准模型或更小 Qwen 模型 |
| 数据 | GSM8K 小批量 |
| train steps | 10 |
| train batch | 与 VeRL smoke 对齐 |
| max response length | 512 / 1024 |
| temperature | 0.0 或当前训练口径 |
| GPU | 1、2、4、8 分别测试 |

### 第四步：再讨论 UEnv 接入

只有在 NexRL 自身的真实模型 1step/10step 都稳定后，再选择接入点：

1. 如果目标是最小改造，优先做 UEnv RolloutWorker。
2. 如果目标是长期架构，评估 Agent Service / TrajectoryPool 直连。
3. 如果目标只是吸收设计，不必接入 NexRL，可把 pool/queue/weight sync 思路迁移到现有 Adapter。

## 9. 当前结论

NexRL 相比 VeRL / ROLL 的主要优势不是“启动更简单”，而是服务化边界更清楚：推理服务、训练服务、agent/rollout、trajectory pool、weight sync 都是显式模块。因此它适合我们研究 UEnv 长期的分布式 RL 系统形态，尤其是多任务、多模型、多 worker 和异步 trajectory 管理。

但短期风险也明显：NexRL 的最小真实训练比 VeRL/ROLL 更复杂，需要先确认 API server、GPU worker、inference endpoint、logprob 返回和权重同步都能稳定工作。因此建议下一步只做“GSM8K 1step 真实模型 smoke”，不要直接进入 UEnv 接入。

## 10. 参考资料

- NexRL README：<https://github.com/nex-agi/NexRL>
- NexRL 中文 README：`/data/ronghao/third_party/NexRL/docs/README-CN.md`
- NexRL User Guide：`/data/ronghao/third_party/NexRL/docs/user-guide.md`
- NexRL CLI Reference：`/data/ronghao/third_party/NexRL/cli/README.md`
- 本地最小 smoke：`/data/ronghao/third_party/NexRL/local_gsm8k_smoke/`
- NexRL blog：<https://dawning-road.github.io/blog/nexrl>
