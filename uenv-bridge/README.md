# uenv-bridge

`uenv-bridge` 是 UEnv 面向训练框架的适配层。目前主要接入目标是 VeRL。它负责把 VeRL 的 `DataProto` batch 转成 UEnv 可以理解的 episode 请求，再把 UEnv/Serve 的结果转回 VeRL 训练所需的 reward。

当前实现已经验证过真实 VeRL 训练链路：

- 真实 `verl.trainer.main_ppo` 1-step GRPO。
- 真实 `verl.trainer.main_ppo` 2-step GRPO。
- VeRL reward worker 通过本地 gRPC 调 Rust adapter core。
- Rust adapter core 返回 reward 后，VeRL 指标中能看到 `critic/score/*` 和 `critic/rewards/*`。

## 当前架构

```text
VeRL trainer / reward worker
        |
        | Python import
        v
UEnvBridgeRewardManager
        |
        | DataProto -> EpisodeRequest
        v
VeRLAdapter
        |
        | local gRPC, adapter_core.proto
        v
Rust adapter core
        |
        | Rust trait / function call
        v
UEnv Serve / UEnv Server implementation
```

重要边界：

- Python 侧只处理 VeRL 对象：`DataProto`、tokenizer、tensor、`non_tensor_batch`、`rm_scores`。
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

### Python: VeRLAdapter

入口文件：

- `src/uenv/bridge/verl.py`

主要职责：

- `to_episode_requests(batch)`: 将 dict fixture 或真实 `DataProto` 拆成 `EpisodeRequest` 列表。
- `execute_batch(batch)`: 提交 batch 并返回普通 Python dict 结果。
- `results_to_dataproto(batch, results)`: 构造 VeRL 可消费的 `rm_scores` 和 reward extra fields。

`EpisodeRequest.payload` 是 JSON bytes，核心字段包括：

```json
{
  "protocol_version": "1.0",
  "framework": "verl",
  "correlation_id": "verl-batch-xxx-0",
  "env_config": {
    "task_name": "math",
    "data_source": "openai/gsm8k",
    "raw_prompt": "...",
    "response_text": "model rollout text"
  },
  "episode_config": {
    "max_steps": 10,
    "seed": 42,
    "initial_observation": {}
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
    "data_source": "openai/gsm8k"
  }
}
```

### Python: RustCoreEpisodeClient

入口文件：

- `src/uenv/bridge/clients.py`

这是 Python shim 到 Rust adapter core 的 client。它可以自动启动本地 Rust core：

```python
from uenv.bridge.clients import RustCoreClientConfig, RustCoreEpisodeClient

client = RustCoreEpisodeClient(
    RustCoreClientConfig(
        endpoint="127.0.0.1:55101",
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
export UENV_ADAPTER_CORE_ENDPOINT=127.0.0.1:55101
export UENV_ADAPTER_CORE_AUTO_START=1
export UENV_ADAPTER_CORE_BINARY=/tmp/uenv-bridge/core/target/debug/uenv-adapter-core
```

## Bridge 内部通道

Python reward manager 与 Rust adapter core 的本地通信由 `proto/adapter_core.proto` 定义。这个协议只用于 bridge 内部调试和验证；Serve 接入时只需要关注下一节的 `EpisodeService` 边界。

## Serve 侧应该如何接入

Serve 侧需要向 `core` 提供 Rust 可调用的 batch episode 实现，满足这个 trait：

```rust
#[async_trait]
pub trait EpisodeService: Send + Sync {
    async fn submit_episode_batch(
        &self,
        requests: Vec<EpisodeRequest>,
    ) -> Result<Vec<EpisodeResult>, CoreError>;
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
- `trajectory.steps` 在 math 类 MVP 中可以为空，但 `summary` 必须完整。

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
  -> bridge EpisodeResult.trajectory, MVP 可以先为空 steps

server EpisodeResult.error
  -> bridge EpisodeResult.error_code / error_message
```

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

当前 VeRL GRPO reward 链路只依赖 `EpisodeResult.summary.total_reward`，Bridge 会把它写入 VeRL 的 `rm_scores`，advantage 仍由 VeRL 原生逻辑计算。因此 Serve 侧 MVP 可以先返回空 `trajectory.steps`，但必须保证：

- `EpisodeResult.request_id` 与输入 `EpisodeRequest.request_id` 一致。
- `EpisodeResult.summary.total_reward` 是本 episode 的最终 reward。
- `EpisodeResult.summary.terminate_reason` 能解释 reward 来源或终止原因。
- `EpisodeResult.trajectory.total_reward` / `total_steps` 与 summary 保持合理一致。

完整 `StepRecord` trajectory 仍然重要，但它属于后续 CodeEnv/AgentEnv 调试、回放、过程奖励和审计能力。真实 Serve 接入后可以逐步填充 `trajectory.steps`；当前 math 类 GRPO smoke test 不依赖完整 trajectory。

Serve 完成后，应新增真实实现，例如：

```rust
pub struct UEnvServeEpisodeService {
    // registry, scheduler, env manager, etc.
}

#[async_trait]
impl EpisodeService for UEnvServeEpisodeService {
    async fn submit_episode_batch(
        &self,
        requests: Vec<EpisodeRequest>,
    ) -> Result<Vec<EpisodeResult>, CoreError> {
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
UENV_ADAPTER_CORE_ADDR=127.0.0.1:55101
UENV_ADAPTER_CORE_REWARD_MODE=fixed | math_proxy
UENV_ADAPTER_CORE_FAKE_REWARD=0.37
UENV_ADAPTER_CORE_FORMAT_REWARD=0.2
UENV_ADAPTER_CORE_NONEMPTY_REWARD=0.05
UENV_ADAPTER_CORE_DEFAULT_REWARD=0.0
```

`fixed` 和 `math_proxy` 都是临时调试模式。真实 Serve 接入后，reward mode 应替换为 Serve backed implementation。

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
ADAPTER_CORE_REWARD_MODE=fixed \
ADAPTER_CORE_FAKE_REWARD=0.37 \
./scripts/run_verl_grpo_1step_with_bridge_reward.sh
```

没有 VeRL image 或 GPU 的协作者仍然可以做 Layer 1 到 Layer 3 的轻量验证；只有 Layer 4 的真实 GRPO smoke test 需要这个镜像和 GPU。

## 四层验证测试

Bridge 当前按四层验证。越靠前越轻量，越靠后越接近真实 VeRL + Serve 联动。Serve 侧协作者主要关注 Layer 4；Layer 1 到 Layer 3 是 bridge 侧在联调前确认自身链路可用的基线测试。

### Layer 1: Python adapter 转换与单测

前置条件：

- Python >= 3.10。
- 已安装 Python 最小依赖：`grpcio`、`grpcio-tools`、`protobuf`、`pyyaml`。
- 不需要 VeRL image、GPU、真实 Serve 或 Rust core。

流程：

```text
dict fixture / fake DataProto-like batch
  -> VeRLAdapter.to_episode_requests()
  -> FakeEpisodeClient / DryRunEpisodeClient
  -> VeRLAdapter.results_to_dataproto()
  -> Python dict result / rm_scores-like fields
```

运行方式：

```bash
cd uenv-bridge
PYTHONPATH=src python3 -m unittest discover -s tests -v
```

预期结果：

- 当前应看到 `13` 个 Python tests 通过。
- request 中的 `request_id`、`batch_id`、`sample_index` 和 metadata 能被保留。
- fake/dry-run client 返回的结果能按原 sample 顺序写回 reward 字段。

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
  -> EpisodeService(fake/math_proxy)
  -> EpisodeResult
  -> normalized reward result
```

运行方式：

```bash
cd uenv-bridge/core
cargo test
```

预期结果：

- 当前应看到 `5` 个 Rust tests 通过。
- Rust core 能校验 batch result 数量和 `request_id` 对齐。
- `FakeEpisodeService` 和 `MathProxyEpisodeService` 能产出可回传的 `EpisodeResult.summary.total_reward`。

### Layer 3: Python 到 Rust core 的本地 gRPC 闭环

前置条件：

- Layer 1 的 Python 依赖。
- Layer 2 的 Rust/Cargo 和 `protoc`。
- 已构建或可自动构建 `core/target/debug/uenv-adapter-core`。
- 不需要 VeRL image、GPU 或真实 Serve。

流程：

```text
fixture batch
  -> Python VeRLAdapter
  -> RustCoreEpisodeClient(auto_start=True)
  -> Rust adapter core
  -> EpisodeService(fake)
  -> Python EpisodeResult
```

运行方式：

```bash
cd uenv-bridge
./scripts/generate_adapter_core_proto.sh
PYTHONPATH=src ./scripts/verify_rust_core_grpc_loop.py --reward 0.37
```

预期结果：

- Python client 能自动启动本地 Rust core。
- gRPC `HealthCheck` 通过。
- fixture batch 能得到类似下面的 reward 输出：

```json
{"rewards": [0.37, 0.37]}
```

### Layer 4: 真实 VeRL + Serve 联动 smoke test

前置条件：

- 可运行的 VeRL image，例如 `localhost/uenv-bridge-verl:latest`。
- GPU、模型缓存/模型路径、GSM8K sample 数据。
- Layer 3 的 Rust core 本地 gRPC 链路可用。
- Serve 侧已经实现 [core/src/server_api.rs](core/src/server_api.rs) 中的 `EpisodeService`，并在 Rust adapter core binary 中替换 `FakeEpisodeService` / `MathProxyEpisodeService`。
- Serve 返回的 `EpisodeResult.request_id` 必须和输入 `EpisodeRequest.request_id` 一致，`summary.total_reward` 必须是该 sample 的最终 reward。

流程：

```text
verl.trainer.main_ppo
  -> UEnvBridgeRewardManager
  -> VeRLAdapter
  -> Rust adapter core
  -> Serve EpisodeService implementation
  -> EpisodeResult
  -> rm_scores
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
ADAPTER_CORE_REWARD_MODE=serve \
./scripts/run_verl_grpo_1step_with_bridge_reward.sh
```

多步联动可以把 `TRAINING_STEPS` 调大，例如：

```bash
cd uenv-bridge
IMAGE=localhost/uenv-bridge-verl:latest \
TRAINING_STEPS=2 \
SAMPLE_COUNT=4 \
TRAIN_BATCH_SIZE=2 \
ROLLOUT_N=2 \
ADAPTER_CORE_REWARD_MODE=serve \
./scripts/run_verl_grpo_1step_with_bridge_reward.sh
```

这里假设 Serve 接入 PR 暴露 `serve` 作为真实启动开关；如果实际使用其他 mode，需要替换 `ADAPTER_CORE_REWARD_MODE`。当前仓库尚未实现 `serve` mode，因此在 Serve 接入前直接运行会失败；`fixed` 和 `math_proxy` 只是 bridge-only 基线，不代表真实 Serve 联动通过。

预期结果：

- Serve 日志能看到 Rust adapter core 调用了真实 `EpisodeService`。
- `episode_requests.jsonl` 中的每个 `request_id` 都能在 Serve 侧和 `episode_results.jsonl` 中对应起来。
- VeRL 日志中的 `critic/score/mean` / `critic/rewards/mean` 等于 Serve 返回 reward 的 batch 平均值，而不是固定 fake reward。
- 1-step 和 2-step GRPO 都能正常结束，`Training Progress` 达到 `100%`。
- `tmp/verl_bridge_reward_records/<RUN_ID>/episode_requests.jsonl` 和 `episode_results.jsonl` 行数应匹配 `TRAINING_STEPS * TRAIN_BATCH_SIZE * ROLLOUT_N`。

在真实 Serve implementation 合入前，可以先用下面两个命令作为 bridge-only 基线，确认 VeRL -> Python bridge -> Rust core -> reward 回写链路没有问题：

```bash
cd uenv-bridge
IMAGE=localhost/uenv-bridge-verl:latest \
TRAINING_STEPS=1 \
SAMPLE_COUNT=2 \
TRAIN_BATCH_SIZE=2 \
ROLLOUT_N=2 \
ADAPTER_CORE_REWARD_MODE=fixed \
ADAPTER_CORE_FAKE_REWARD=0.37 \
./scripts/run_verl_grpo_1step_with_bridge_reward.sh
```

```bash
cd uenv-bridge
IMAGE=localhost/uenv-bridge-verl:latest \
TRAINING_STEPS=2 \
SAMPLE_COUNT=4 \
TRAIN_BATCH_SIZE=2 \
ROLLOUT_N=2 \
ADAPTER_CORE_REWARD_MODE=math_proxy \
./scripts/run_verl_grpo_1step_with_bridge_reward.sh
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

验证 Python 自动启动 Rust core 的本地 gRPC 闭环：

```bash
cd uenv-bridge
./scripts/verify_rust_core_grpc_loop.py --reward 0.37
```

跑真实 VeRL 1-step bridge-only 基线：

```bash
IMAGE=localhost/uenv-bridge-verl:latest \
TRAINING_STEPS=1 \
SAMPLE_COUNT=2 \
TRAIN_BATCH_SIZE=2 \
ROLLOUT_N=2 \
ADAPTER_CORE_REWARD_MODE=fixed \
ADAPTER_CORE_FAKE_REWARD=0.37 \
./scripts/run_verl_grpo_1step_with_bridge_reward.sh
```

跑真实 VeRL 2-step bridge-only 基线，使用 Rust math proxy reward：

```bash
IMAGE=localhost/uenv-bridge-verl:latest \
TRAINING_STEPS=2 \
SAMPLE_COUNT=4 \
TRAIN_BATCH_SIZE=2 \
ROLLOUT_N=2 \
ADAPTER_CORE_REWARD_MODE=math_proxy \
./scripts/run_verl_grpo_1step_with_bridge_reward.sh
```

## 已验证结果

已在 `localhost/uenv-bridge-verl:latest` 中验证以下 bridge-only 基线：

- Python unit tests: 13 passed。
- Rust unit tests: 5 passed。
- fixture -> Python -> Rust core gRPC 闭环。
- 真实 VeRL 1-step GRPO，Rust core fixed reward 返回到 `critic/score/mean`。
- 真实 VeRL 2-step GRPO，Rust core math proxy reward 每步返回到 `critic/score/mean`。

真实 Serve 联动需要 Serve 侧 `EpisodeService` 接入 Rust core 后再验收。

多步验证记录示例：

```text
step 1: critic/score/mean = 0.20000000298023224
step 2: critic/score/mean = 0.20000000298023224
```

对应记录目录：

```text
tmp/verl_bridge_reward_records/<RUN_ID>/
```

其中：

- `episode_requests.jsonl`: 本次训练提交给 adapter core 的请求记录。
- `episode_results.jsonl`: adapter core 返回给 VeRL 的 reward/result 记录。

## 当前限制

- Serve 真实实现还未接入，当前 Rust core 使用 fake/math proxy service。
- trajectory 目前可以为空 steps，后续应由 Serve 返回真实轨迹。
- 当前 reward 写入 VeRL 的 `rm_scores`，advantage 仍由 VeRL 自己计算。
