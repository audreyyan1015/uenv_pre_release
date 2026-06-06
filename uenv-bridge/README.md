# uenv-bridge

`uenv-bridge` 是 UEnv 面向训练框架的适配层。目前主要接入目标是 VeRL，当前主线是 rollout 前接管：VeRL 在 AgentLoop 阶段把 prompt/sample 交给 UEnv，后续模型生成、环境 step、reward 和 trajectory 由 UEnv Server/Worker 完成，Bridge 再把外部 rollout 结果包装回 VeRL 可训练的 `AgentLoopOutput`。

仓库中仍保留 rollout 后 reward-manager 链路，作为对照基线和过渡方案。当前阶段新增和验证的重点是 pre-rollout Route A。

当前实现已经覆盖：

- `UEnvAgentLoop` 在 VeRL rollout 前构造 `EpisodeRequest`。
- Python 通过本地 gRPC 调 Rust adapter core。
- Rust adapter core 保留 UEnv 返回的 `trajectory`，使 Python 能恢复 `response_ids`、`response_mask` 和 reward。
- 真实 `verl.trainer.main_ppo` 1-step pre-rollout AgentLoop smoke test。

## 当前架构

当前主线：

```text
VeRL trainer
        |
        | AgentLoop before rollout generation
        v
UEnvAgentLoop
        |
        | EpisodeRequest
        v
Rust adapter core
        |
        | Rust trait / function call
        v
UEnv Serve / UEnv Server implementation
        |
        | model generation + env step + reward + trajectory
        v
Rust adapter core
        |
        | trajectory_json + reward
        v
UEnvAgentLoop
        |
        | AgentLoopOutput(response_ids, response_mask, reward_score)
        v
VeRL GRPO trainer
```

保留的 rollout 后对照基线：

```text
VeRL trainer / reward worker
        -> UEnvBridgeRewardManager
        -> VeRLAdapter
        -> Rust adapter core
        -> reward result
        -> VeRL rm_scores
```

当前阶段的重要边界：

- Python 侧只处理 VeRL 对象：AgentLoop kwargs、tokenizer、prompt ids、`AgentLoopOutput`；后置基线中才处理 `DataProto`、`rm_scores`。
- Python 和 Rust adapter core 之间使用本地 gRPC，proto 在 `proto/adapter_core.proto`。
- Rust adapter core 和 Serve/UEnv Server 之间不走 gRPC。Serve 侧应该以 Rust 库/函数/trait 的形式接入 adapter core。

## Bridge 提供的主要接口

### Python: VeRL reward manager

入口文件：

- `src/uenv/bridge/verl_reward_manager.py`

VeRL 配置中通过 importlib 加载：

```bash
reward.reward_manager.source=importlib
reward.reward_manager.name=UEnvBridgeRewardManager
reward.reward_manager.module.path=/tmp/uenv-bridge/src/uenv/bridge/verl_reward_manager.py
```

`UEnvBridgeRewardManager` 接收 VeRL 传入的单条 `DataProto`，解码 rollout response token，写入 `uenv_response_text`，再调用 `VeRLAdapter`。

### Python: VeRL pre-rollout AgentLoop

入口文件：

- `src/uenv/bridge/verl_agent_loop.py`
- `src/uenv/bridge/agent_loop_clients.py`
- `configs/uenv-agent-loop.yaml`

这是 Route A 的 rollout 前接管入口。VeRL 在 `actor_rollout_ref.rollout.agent.default_agent_loop=uenv_agent` 时会加载 `UEnvAgentLoop`，`UEnvAgentLoop.run()` 会把 `raw_prompt`、`reward_model`、`sampling_params` 和 prompt token 组织成 `EpisodeRequest`，交给 UEnv Server/Worker 完成完整 rollout。

这条链路和 reward-manager 主线的区别是：reward-manager 接入点发生在 VeRL/vLLM 已经生成 response 之后；`UEnvAgentLoop` 接入点发生在 VeRL 调用 rollout 生成之前。因此使用 `UEnvAgentLoop` 时，UEnv Server/Worker 需要负责模型生成、环境 step、reward 和 trajectory，并返回 `response_ids`、`response_mask`、`summary.total_reward`。

VeRL 配置方式：

```bash
actor_rollout_ref.rollout.agent.default_agent_loop=uenv_agent
actor_rollout_ref.rollout.agent.agent_loop_config_path=/tmp/uenv-bridge/configs/uenv-agent-loop.yaml
```

当前 bridge 会优先从 `EpisodeResult.trajectory.steps[-1].info["response_ids"]` 和 `response_mask` 读取 token 级结果。如果没有 token 字段，会退回到最后一步 `action` 文本并用 VeRL tokenizer 编码。真实 Serve/Worker 联调时应优先返回 token ids，避免 tokenizer 或 chat template 不一致。

### Python: VeRLAdapter

入口文件：

- `src/uenv/bridge/verl.py`

主要职责：

- `to_episode_requests(batch)`: 将 dict fixture 或真实 `DataProto` 拆成 `EpisodeRequest` 列表。
- `execute_batch(batch)`: 提交 batch 并返回普通 Python dict 结果。
- `results_to_dataproto(batch, results)`: 构造 VeRL 可消费的 `rm_scores` 和 reward extra fields。

pre-rollout Route A 中，`EpisodeRequest.payload` 是 JSON bytes，核心字段包括：

```json
{
  "protocol_version": "1.0",
  "framework": "verl",
  "correlation_id": "verl-batch-xxx-0",
  "env_config": {
    "task_name": "math",
    "data_source": "openai/gsm8k",
    "raw_prompt": "..."
  },
  "episode_config": {
    "max_steps": 10,
    "seed": 42,
    "initial_observation": {
      "raw_prompt": "...",
      "prompt_text": "...",
      "prompt_ids": [1, 2, 3],
      "token_source": "verl_agent_loop"
    }
  },
  "reward_config": {
    "reward_type": "rubric",
    "rubric_config": {
      "ground_truth": "..."
    }
  },
  "metadata": {
    "batch_id": "...",
    "sample_index": 0,
    "data_source": "openai/gsm8k",
    "required_result_fields": ["response_ids", "response_mask", "reward", "trajectory"]
  }
}
```

rollout 后 reward-manager 对照基线中才会把已生成的模型 response 放入请求；当前主线的请求只携带 prompt/sample 信息，response 由 UEnv Server/Worker 生成并随 `EpisodeResult` 返回。

### Python: RustCoreEpisodeClient

入口文件：

- `src/uenv/bridge/clients.py`

这是 Python shim 到 Rust adapter core 的 client。它可以自动启动本地 Rust core：

```python
from uenv.bridge.clients import RustCoreClientConfig, RustCoreEpisodeClient

client = RustCoreEpisodeClient(
    RustCoreClientConfig(
        endpoint="127.0.0.1:50051",
        auto_start=True,
        binary="/tmp/uenv-bridge/core/target/debug/uenv-adapter-core",
        timeout_seconds=300,
        startup_timeout_seconds=60,
    )
)
```

环境变量方式：

```bash
export UENV_BRIDGE_CLIENT=rust_core
export UENV_ADAPTER_CORE_ENDPOINT=127.0.0.1:50051
export UENV_ADAPTER_CORE_AUTO_START=1
export UENV_ADAPTER_CORE_BINARY=/tmp/uenv-bridge/core/target/debug/uenv-adapter-core
```

## Bridge 内部通道

Python reward manager 与 Rust adapter core 的本地通信由 `proto/adapter_core.proto` 定义。这个协议只用于 bridge 内部调试和验证；Serve 接入时只需要关注下一节的 `EpisodeService` 边界。

## Serve 侧应该如何接入

Serve 侧需要向 `core` 提供 Rust 可调用的 batch episode 实现，满足这个 trait：

```rust
pub trait EpisodeService: Send + Sync {
    fn submit_episode_batch(
        &self,
        requests: Vec<EpisodeRequest>,
    ) -> impl Future<Output = Result<Vec<EpisodeResult>, EpisodeServiceError>> + Send;
}
```

定义位置：

- `core/src/server_api.rs`

相关结构定义位置：

- `core/src/protocol.rs`

### 与当前 uenv-server 的关系

当前 `main` 分支中的 `uenv-server/proto/server.proto` 是 server 对外和 worker 侧的 gRPC 协议，包含 `UEnvService`、`WorkerRegistration`、`WorkerExecution` 和 `AdminService`。Bridge 不要求 Serve 直接暴露给 Python；Bridge 对 Serve 的要求是 Rust adapter core 能通过本地函数调用拿到 episode 结果。

如果 Serve 侧继续保持 gRPC-first 的实现，可以在 Rust adapter core 这一侧提供一个 wrapper：wrapper 对内实现 `EpisodeService`，对外再调用 Serve 已有的 scheduler/env/worker 逻辑。Bridge 只依赖 `EpisodeService` 这个函数边界。

Serve 对接函数签名：

```rust
async fn submit_episode_batch(
    requests: Vec<EpisodeRequest>,
) -> Result<Vec<EpisodeResult>, CoreError>;
```

函数语义：

- 输入数量为 `N`，返回结果数量也必须为 `N`。
- 每个 `EpisodeResult.request_id` 必须原样等于对应的 `EpisodeRequest.request_id`。
- batch 内单个 episode 失败时，推荐返回该 request 的 failed `EpisodeResult`，不要让整个 batch panic。
- `EpisodeResult.summary.total_reward` 是 VeRL 训练最终读取的 reward。
- pre-rollout Route A 中，`trajectory.steps` 至少要能表达最终模型输出。推荐在最后一个 step 的 `info` 中返回 `response_ids` 和 `response_mask`；如果暂时做不到，Bridge 会退回到 `StepRecord.action` 或 `info["response_text"]` 并用 VeRL tokenizer 编码。

### Bridge EpisodeRequest 到 server proto 的建议映射

如果 Serve 侧需要把 `uenv-bridge/core/src/protocol.rs` 里的 `EpisodeRequest` 映射到 `uenv-server/proto/server.proto` 里的 `uenv.v1.EpisodeRequest`，建议按下面规则处理：

```text
bridge EpisodeRequest.request_id
  -> server EpisodeRequest.request_id

bridge EpisodeRequest.env_type
  -> server EpisodeRequest.env_type

bridge EpisodeRequest.payload.protocol_version
  -> server EpisodeRequest.protocol_version

bridge EpisodeRequest.payload.framework
  -> server EpisodeRequest.framework

bridge EpisodeRequest.payload.correlation_id or batch_id
  -> server EpisodeRequest.correlation_id

bridge EpisodeRequest.payload.env_config
  -> server EpisodeRequest.env_config as JSON bytes

bridge EpisodeRequest.model_endpoint
  -> server EpisodeRequest.model_endpoint.url

bridge EpisodeRequest.max_steps / seed
  -> server EpisodeRequest.episode_config.max_steps / seed

bridge EpisodeRequest.payload.episode_config.initial_observation
  -> server EpisodeRequest.episode_config.initial_observation as JSON bytes

bridge EpisodeRequest.payload.reward_config.reward_type
  -> server EpisodeRequest.reward_config.reward_type

bridge EpisodeRequest.payload.reward_config.rubric_config
  -> server EpisodeRequest.reward_config.rubric_config as JSON bytes

bridge EpisodeRequest.payload.metadata
  -> server EpisodeRequest.metadata as JSON bytes
```

server 返回结果时，Bridge 需要的最小字段是：

```text
server EpisodeResult.request_id
  -> bridge EpisodeResult.request_id

server EpisodeResult.status
  -> bridge EpisodeResult.status

server EpisodeResult.summary.total_reward
  -> bridge EpisodeResult.summary.total_reward

server EpisodeResult.summary.termination_reason
  -> bridge EpisodeResult.summary.terminate_reason

server EpisodeResult.trajectory
  -> bridge EpisodeResult.trajectory

server EpisodeResult.error
  -> bridge EpisodeResult.error_code / error_message
```

pre-rollout Route A 还要求 `trajectory.steps[-1]` 包含下列信息之一：

```text
trajectory.steps[-1].info["response_ids"] = "[101,102,...]"
trajectory.steps[-1].info["response_mask"] = "[1,1,...]"
```

或者：

```text
trajectory.steps[-1].action = response text bytes
trajectory.steps[-1].info["response_text"] = "..."
```

推荐优先返回 token ids，因为这样可以避免 tokenizer、chat template 或特殊 token 处理不一致。

Serve 侧要做的事情：

1. 接收 `Vec<EpisodeRequest>`。
2. 对每个 request 解析 `payload`。
3. 根据 `env_type`、`max_steps`、`seed`、`model_endpoint`、`payload` 中的 `env_config`、`episode_config`、`reward_config` 创建或调用真实环境。
4. 执行 episode 或 reward 计算。
5. 返回同等数量的 `EpisodeResult`。
6. 每个 `EpisodeResult.request_id` 必须等于对应 `EpisodeRequest.request_id`。
7. 如果单个 request 失败，推荐返回该 request 的 failed result，而不是让整个 batch panic。

Serve 返回成功样例：

```rust
EpisodeResult {
    request_id: request.request_id,
    status: "completed".to_string(),
    trajectory: Trajectory {
        steps: Vec::new(),
        total_reward: 1.0,
        total_steps: 1,
    },
    summary: EpisodeSummary {
        total_reward: 1.0,
        total_steps: 1,
        total_duration_ms: 0,
        terminate_reason: "exact_match".to_string(),
    },
    error_code: None,
    error_message: String::new(),
}
```

Serve 返回失败样例：

```rust
EpisodeResult {
    request_id: request.request_id,
    status: "failed".to_string(),
    trajectory: Trajectory::default(),
    summary: EpisodeSummary {
        total_reward: 0.0,
        total_steps: 0,
        total_duration_ms: 0,
        terminate_reason: "env_error".to_string(),
    },
    error_code: Some(3002), // ERR_ENV_INIT_FAILED
    error_message: "failed to create environment".to_string(),
}
```

当前 `core/src/server_api.rs` 中有两个临时实现：

- `FakeEpisodeService`: 固定 reward，用于链路测试。
- `MathProxyEpisodeService`: 简单 math reward，用于真实 VeRL 多步 smoke test。

### Trajectory 在当前 MVP 中的要求

当前主线是 rollout 前接管，所以 Serve/Worker 需要返回可恢复 VeRL response 的 trajectory。最小要求：

- `EpisodeResult.request_id` 与输入 `EpisodeRequest.request_id` 一致。
- `EpisodeResult.summary.total_reward` 是本 episode 的最终 reward。
- `EpisodeResult.summary.terminate_reason` 能解释 reward 来源或终止原因。
- `EpisodeResult.trajectory.total_reward` / `total_steps` 与 summary 保持合理一致。
- 最后一个 `StepRecord` 能提供 `response_ids` / `response_mask`，或者能通过 `action` / `response_text` 恢复 response 文本。

完整多步 `StepRecord` 仍可逐步完善；但对 rollout 前接管来说，最终 response token 和最终 reward 是训练能继续运行的硬要求。

Serve 完成后，应新增真实实现，例如：

```rust
pub struct UEnvServeEpisodeService {
    // registry, scheduler, env manager, etc.
}

impl EpisodeService for UEnvServeEpisodeService {
    async fn submit_episode_batch(
        &self,
        requests: Vec<EpisodeRequest>,
    ) -> Result<Vec<EpisodeResult>, EpisodeServiceError> {
        // 1. parse EpisodeRequest.payload
        // 2. call Serve/UEnv environment functions
        // 3. map Serve/UEnv output to EpisodeResult
        todo!()
    }
}
```

然后在 `core/src/main.rs` 中把 `AdapterCore::new(...)` 的 service 换成真实 Serve implementation。

## Rust core 运行参数

Rust binary:

```bash
core/target/debug/uenv-adapter-core
```

环境变量：

```bash
UENV_ADDR=127.0.0.1:50051
UENV_ADAPTER_CORE_BACKEND=static_rollout | server
UENV_ADAPTER_CORE_STATIC_REWARD=0.73
UENV_ADAPTER_CORE_STATIC_RESPONSE_IDS=201,202,203
UENV_ADAPTER_CORE_STATIC_RESPONSE_TEXT="static external rollout"
```

`static_rollout` 是 rollout 前接管的 bridge-only 调试后端，会返回固定 token、mask 和 reward。真实 Serve/Worker 接入时应使用 `server` 后端，由 `UEnvEpisodeService` 调度真实 worker。

## VeRL image 环境准备

如果协作者也想本地跑真实 VeRL smoke test，可以直接构建包含 `uenv-bridge`、Rust、Cargo 和 `protoc` 的镜像。构建只需要容器运行时和网络；GPU 只在后续运行真实 GRPO 时需要。

前置条件：

- Linux host。
- `podman` 或 `docker`。
- 能访问 `docker.io/verlai/verl:vllm011.latest`，或通过 `BASE_IMAGE` 指向已有 VeRL base image。
- 如果要跑真实训练，需要 NVIDIA GPU 和可用的 container GPU runtime。

构建默认镜像：

```bash
cd uenv-bridge
./scripts/build_verl_bridge_image.sh
```

默认会生成：

```text
localhost/uenv-bridge-verl:latest
```

如果使用 Docker 或自定义镜像名：

```bash
cd uenv-bridge
CONTAINER_TOOL=docker IMAGE=uenv-bridge-verl:latest ./scripts/build_verl_bridge_image.sh
```

构建脚本会做一个轻量验证：确认镜像内可以 import `verl` 和 `uenv.bridge`，并检查 `rustc`、`cargo`、`protoc` 是否可用。如果只想构建不验证：

```bash
cd uenv-bridge
./scripts/build_verl_bridge_image.sh --no-verify
```

跑真实 GRPO 前还需要准备模型和 GSM8K parquet 数据。现有训练脚本默认从 `MODEL_CACHE` 查找或下载模型，并要求 `VERL_WORKSPACE` 下存在 `data/gsm8k/train.parquet` 和 `data/gsm8k/test.parquet`：

```bash
cd uenv-bridge
IMAGE=localhost/uenv-bridge-verl:latest \
VERL_WORKSPACE=/path/to/verl/workspace \
MODEL_CACHE=/path/to/models \
TRAINING_STEPS=1 \
SAMPLE_COUNT=2 \
TRAIN_BATCH_SIZE=2 \
ROLLOUT_N=2 \
UENV_AGENT_LOOP_CLIENT=rust_core \
UENV_ADAPTER_CORE_BACKEND=static_rollout \
UENV_ADAPTER_CORE_STATIC_REWARD=0.73 \
UENV_ADAPTER_CORE_STATIC_RESPONSE_IDS=201,202,203 \
./scripts/run_verl_grpo_1step_with_uenv_agent_loop.sh
```

没有 VeRL image 或 GPU 的协作者仍然可以做 Layer 1 到 Layer 3 的轻量验证；只有 Layer 4 的真实 GRPO smoke test 需要这个镜像和 GPU。

## 四层验证测试

Bridge 当前按四层验证。越靠前越轻量，越靠后越接近真实 VeRL + Serve/Worker 联动。当前主线是 rollout 前接管，Serve 侧协作者主要关注 Layer 4；Layer 1 到 Layer 3 是 bridge 侧在联调前确认自身链路可用的基线测试。

### Layer 1: Python adapter 转换与单测

前置条件：

- Python >= 3.10。
- 已安装 Python 最小依赖：`grpcio`、`grpcio-tools`、`protobuf`、`pyyaml`。
- 不需要 VeRL image、GPU、真实 Serve 或 Rust core。

流程：

```text
pre-rollout dict fixture / AgentLoop fake input
  -> VeRLAdapter.to_episode_requests()
  -> UEnvAgentLoop.build_episode_request()
  -> FakeEpisodeClient / DryRunEpisodeClient
  -> Python dict result / AgentLoopOutput
```

运行方式：

```bash
cd uenv-bridge
PYTHONPATH=src python3 -m unittest discover -s tests -v
```

预期结果：

- 当前应看到 `17` 个 Python tests 通过。
- request 中的 `request_id`、`batch_id`、`sample_index`、prompt token 和 metadata 能被保留。
- fake/dry-run client 返回的结果能恢复 `response_ids`、`response_mask` 和 reward。

### Layer 2: Rust adapter core 单测

前置条件：

- Rust/Cargo。
- `protoc`，通常由系统包 `protobuf-compiler` 提供。
- 不需要 VeRL image、GPU、真实 Serve 或 Python bridge。

流程：

```text
normalized sample
  -> Rust adapter core
  -> EpisodeRequest
  -> EpisodeService(static_rollout/test service)
  -> EpisodeResult
  -> SampleResult(reward + trajectory_json)
```

运行方式：

```bash
cd uenv-bridge/core
cargo test
```

预期结果：

- 当前应看到 `6` 个 Rust tests 通过。
- Rust core 能校验 batch result 数量和 `request_id` 对齐。
- Rust core 能把 server/worker 返回的 `trajectory` 保留为 `SampleResult.trajectory_json`，供 Python AgentLoop 恢复 token 级 rollout 结果。

### Layer 3: rollout 前 AgentLoop 到 Rust core 的本地 gRPC 闭环

前置条件：

- Layer 1 的 Python 依赖。
- Layer 2 的 Rust/Cargo 和 `protoc`。
- 已构建或可自动构建 `core/target/debug/uenv-adapter-core`。
- 不需要 VeRL image、GPU 或真实 Serve。

流程：

```text
AgentLoop sample
  -> UEnvAgentLoop.run()
  -> RustCoreEpisodeClient(auto_start=True)
  -> Rust adapter core
  -> EpisodeService(static_rollout)
  -> EpisodeResult(trajectory + reward)
  -> AgentLoopOutput(response_ids, response_mask, reward_score)
```

运行方式：

```bash
cd uenv-bridge
./scripts/generate_adapter_core_proto.sh
PYTHONPATH=src ./scripts/verify_pre_rollout_rust_core_loop.py --reward 0.73 --response-ids 201,202,203
```

预期结果：

- Python client 能自动启动本地 Rust core。
- gRPC `HealthCheck` 通过。
- Rust core 的 `static_rollout` 后端会返回含 `response_ids`、`response_mask` 和 reward 的 trajectory。
- `UEnvAgentLoop` 能得到类似下面的输出：

```json
{"response_ids": [201, 202, 203], "response_mask": [1, 1, 1], "reward_score": 0.73}
```

### Layer 4: 真实 VeRL + Serve/Worker pre-rollout 联动 smoke test

前置条件：

- 可运行的 VeRL image，例如 `localhost/uenv-bridge-verl:latest`。
- GPU、模型缓存/模型路径、GSM8K sample 数据。
- Layer 3 的 Rust core 本地 gRPC 链路可用。
- Serve/Worker 侧已经实现 [core/src/server_api.rs](core/src/server_api.rs) 中的 `EpisodeService`，并能调模型生成 action、执行环境 step、计算 reward。
- Serve 返回的 `EpisodeResult.request_id` 必须和输入 `EpisodeRequest.request_id` 一致，`summary.total_reward` 必须是该 sample 的最终 reward，trajectory 中必须能恢复 `response_ids` / `response_mask`。

流程：

```text
verl.trainer.main_ppo
  -> UEnvAgentLoop
  -> Rust adapter core
  -> Serve EpisodeService implementation
  -> Worker model generation + env step + reward
  -> EpisodeResult(trajectory + reward)
  -> AgentLoopOutput
  -> VeRL GRPO advantage/update
```

运行方式：

```bash
cd uenv-bridge
IMAGE=localhost/uenv-bridge-verl:latest \
TRAINING_STEPS=1 \
SAMPLE_COUNT=2 \
TRAIN_BATCH_SIZE=2 \
ROLLOUT_N=2 \
UENV_AGENT_LOOP_CLIENT=rust_core \
UENV_ADAPTER_CORE_BACKEND=server \
./scripts/run_verl_grpo_1step_with_uenv_agent_loop.sh
```

多步联动可以把 `TRAINING_STEPS` 调大，例如：

```bash
cd uenv-bridge
IMAGE=localhost/uenv-bridge-verl:latest \
TRAINING_STEPS=2 \
SAMPLE_COUNT=4 \
TRAIN_BATCH_SIZE=2 \
ROLLOUT_N=2 \
UENV_AGENT_LOOP_CLIENT=rust_core \
UENV_ADAPTER_CORE_BACKEND=server \
./scripts/run_verl_grpo_1step_with_uenv_agent_loop.sh
```

这里假设 bridge core binary 通过 `UENV_ADAPTER_CORE_BACKEND=server` 使用真实 Server/Worker 后端。当前仓库中的 `static_rollout` 只是 bridge-only 基线，不代表真实 Serve 联动通过。

预期结果：

- Serve/Worker 日志能看到 Rust adapter core 调用了真实 `EpisodeService`。
- 每个输入 `request_id` 都能在 Serve/Worker 返回的 `EpisodeResult` 中对应起来。
- VeRL 日志中的 `critic/score/mean` / `critic/rewards/mean` 等于 Serve/Worker 返回 reward 的 batch 平均值，而不是固定 fake reward。
- VeRL rollout 结果来自 UEnv 返回的 `response_ids` / `response_mask`，不是 VeRL 本地 vLLM 生成。
- 1-step 和 2-step GRPO 都能正常结束，`Training Progress` 达到 `100%`。

在真实 Serve implementation 合入前，可以先用下面命令作为 bridge-only 基线，确认真实 VeRL 能通过 AgentLoop 走到 Rust core，并从 Rust core 的 `static_rollout` 后端拿回 token 和 reward：

```bash
cd uenv-bridge
IMAGE=localhost/uenv-bridge-verl:latest \
TRAINING_STEPS=1 \
SAMPLE_COUNT=2 \
TRAIN_BATCH_SIZE=2 \
ROLLOUT_N=2 \
UENV_AGENT_LOOP_CLIENT=rust_core \
UENV_ADAPTER_CORE_BACKEND=static_rollout \
UENV_ADAPTER_CORE_STATIC_REWARD=0.73 \
UENV_ADAPTER_CORE_STATIC_RESPONSE_IDS=201,202,203 \
./scripts/run_verl_grpo_1step_with_uenv_agent_loop.sh
```

```bash
cd uenv-bridge
IMAGE=localhost/uenv-bridge-verl:latest \
TRAINING_STEPS=2 \
SAMPLE_COUNT=4 \
TRAIN_BATCH_SIZE=2 \
ROLLOUT_N=2 \
UENV_AGENT_LOOP_CLIENT=rust_core \
UENV_ADAPTER_CORE_BACKEND=static_rollout \
UENV_ADAPTER_CORE_STATIC_REWARD=0.73 \
UENV_ADAPTER_CORE_STATIC_RESPONSE_IDS=201,202,203 \
./scripts/run_verl_grpo_1step_with_uenv_agent_loop.sh
```

## 常用开发命令

生成 Python gRPC stub：

```bash
cd uenv-bridge
./scripts/generate_adapter_core_proto.sh
```

跑 Python 单测：

```bash
cd uenv-bridge
PYTHONPATH=src python3 -m unittest discover -s tests -v
```

跑 Rust 单测：

```bash
cd uenv-bridge/core
cargo test
```

验证 rollout 前 AgentLoop 自动启动 Rust core 的本地 gRPC 闭环：

```bash
cd uenv-bridge
PYTHONPATH=src ./scripts/verify_pre_rollout_rust_core_loop.py --reward 0.73 --response-ids 201,202,203
```

跑真实 VeRL 1-step pre-rollout AgentLoop + Rust core 基线：

```bash
IMAGE=localhost/uenv-bridge-verl:latest \
TRAINING_STEPS=1 \
SAMPLE_COUNT=2 \
TRAIN_BATCH_SIZE=2 \
ROLLOUT_N=2 \
UENV_AGENT_LOOP_CLIENT=rust_core \
UENV_ADAPTER_CORE_BACKEND=static_rollout \
UENV_ADAPTER_CORE_STATIC_REWARD=0.73 \
UENV_ADAPTER_CORE_STATIC_RESPONSE_IDS=201,202,203 \
./scripts/run_verl_grpo_1step_with_uenv_agent_loop.sh
```

跑真实 VeRL 2-step pre-rollout AgentLoop + Rust core 基线：

```bash
IMAGE=localhost/uenv-bridge-verl:latest \
TRAINING_STEPS=2 \
SAMPLE_COUNT=4 \
TRAIN_BATCH_SIZE=2 \
ROLLOUT_N=2 \
UENV_AGENT_LOOP_CLIENT=rust_core \
UENV_ADAPTER_CORE_BACKEND=static_rollout \
UENV_ADAPTER_CORE_STATIC_REWARD=0.73 \
UENV_ADAPTER_CORE_STATIC_RESPONSE_IDS=201,202,203 \
./scripts/run_verl_grpo_1step_with_uenv_agent_loop.sh
```

## 已验证结果

已在 `localhost/uenv-bridge-verl:latest` 中验证以下 bridge-only 基线：

- Python unit tests: 17 passed。
- Rust unit tests: 6 passed。
- `UEnvAgentLoop -> RustCoreEpisodeClient -> Rust core static_rollout -> AgentLoopOutput` 本地 gRPC 闭环。
- 真实 VeRL 1-step GRPO，`UEnvAgentLoop` pre-rollout fake client 返回 `reward_score=1.0`，VeRL 日志中 `critic/score/mean=1.0`、`training/global_step=1`。

下列 rollout 后 reward-manager 基线也保留为历史验证结果，但不是当前主线：

- 真实 VeRL 1-step GRPO，Rust core fixed reward 返回到 `critic/score/mean`。
- 真实 VeRL 2-step GRPO，Rust core math proxy reward 每步返回到 `critic/score/mean`。

真实 Serve 联动需要 Serve 侧 `EpisodeService` 接入 Rust core 后再验收。

多步验证记录示例：

```text
step 1: critic/score/mean = 0.20000000298023224
step 2: critic/score/mean = 0.20000000298023224
```

rollout 后 reward-manager 基线的记录目录：

```text
tmp/verl_bridge_reward_records/<RUN_ID>/
```

其中：

- `episode_requests.jsonl`: 本次训练提交给 adapter core 的请求记录。
- `episode_results.jsonl`: adapter core 返回给 VeRL 的 reward/result 记录。

## 当前限制

- Serve 真实实现还未接入，当前 pre-rollout Rust core 基线使用 `static_rollout` service。
- 当前 pre-rollout smoke test 还没有真实 Worker 生成 action；真实 Serve/Worker 接入后必须返回 token 级 response 和 reward。
- 旧 reward-manager 基线仍写入 VeRL 的 `rm_scores`；pre-rollout 主线通过 `AgentLoopOutput.reward_score` 进入 VeRL。
