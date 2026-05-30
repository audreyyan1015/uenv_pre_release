# uenv-bridge

`uenv-bridge` 是 UEnv 面向训练框架的适配层。目前主要接入目标是VeRL。它负责把 VeRL 的 `DataProto` batch 转成 UEnv 可以理解的 episode
请求，再把 UEnv/Serve 的结果转回 VeRL 训练所需的 reward。

当前实现已经验证过真实 VeRL 训练链路：

- 真实 `verl.trainer.main_ppo` 1-step GRPO。
- 真实 `verl.trainer.main_ppo` 2-step GRPO。
- VeRL reward worker 通过本地 gRPC 调 Rust adapter core。
- Rust adapter core 返回 reward 后，VeRL 指标中能看到 `critic/score/*` 和  `critic/rewards/*`。

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

`UEnvBridgeRewardManager` 接收 VeRL 传入的单条 `DataProto`，解码 rollout response token，写入 `uenv_response_text`，再调用 VeRLAdapter`。

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

## Python 到 Rust core 的 gRPC 协议

协议文件：

- `proto/adapter_core.proto`

服务：

```protobuf
service AdapterCoreService {
  rpc ExecuteBatch(ExecuteBatchRequest) returns (ExecuteBatchResponse);
  rpc ExecuteBatchStream(stream SampleEnvelope) returns (stream SampleResult);
  rpc HealthCheck(HealthCheckRequest) returns (HealthCheckResponse);
}
```

请求核心结构：

```protobuf
message SampleEnvelope {
  string request_id = 1;
  string batch_id = 2;
  uint32 sample_index = 3;
  string framework = 4;
  string env_type = 5;
  bytes payload_json = 6;
  bytes meta_json = 7;
}
```

响应核心结构：

```protobuf
message SampleResult {
  string request_id = 1;
  string batch_id = 2;
  uint32 sample_index = 3;
  string status = 4;
  double reward = 5;
  bool done = 6;
  string termination_reason = 7;
  bytes trajectory_json = 8;
  string error_code = 9;
  string error_message = 10;
}
```

`request_id` 必须原样返回。Bridge 依靠它把异步/批量结果对齐回原始 VeRL sample。

## Serve 侧应该如何接入

Serve 侧不要实现 Python gRPC 服务。当前约定是：

```text
Python bridge -> local gRPC -> Rust adapter core -> Rust function call -> Serve
```

Serve 侧需要向 `core` 提供 Rust 实现，满足这个 trait：

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

当前 VeRL GRPO reward 链路只依赖 `EpisodeResult.summary.total_reward`，Bridge
会把它写入 VeRL 的 `rm_scores`，advantage 仍由 VeRL 原生逻辑计算。因此
Serve 侧 MVP 可以先返回空 `trajectory.steps`，但必须保证：

- `EpisodeResult.request_id` 与输入 `EpisodeRequest.request_id` 一致。
- `EpisodeResult.summary.total_reward` 是本 episode 的最终 reward。
- `EpisodeResult.summary.terminate_reason` 能解释 reward 来源或终止原因。
- `EpisodeResult.trajectory.total_reward` / `total_steps` 与 summary 保持合理一致。

完整 `StepRecord` trajectory 仍然重要，但它属于后续 CodeEnv/AgentEnv 调试、
回放、过程奖励和审计能力。真实 Serve 接入后可以逐步填充
`trajectory.steps`；当前 math 类 GRPO smoke test 不依赖完整 trajectory。

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

跑真实 VeRL 1-step：

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

跑真实 VeRL 2-step，使用 Rust math proxy reward：

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

已在 `localhost/uenv-bridge-verl:latest` 中验证：

- Python unit tests: 13 passed。
- Rust unit tests: 5 passed。
- fixture -> Python -> Rust core gRPC 闭环。
- 真实 VeRL 1-step GRPO，Rust core fixed reward 返回到 `critic/score/mean`。
- 真实 VeRL 2-step GRPO，Rust core math proxy reward 每步返回到 `critic/score/mean`。

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

- `episode_requests.jsonl`: Bridge 发给 Rust core 的 sample payload。
- `episode_results.jsonl`: Rust core 返回给 VeRL 的 reward/result。

## 当前限制

- Serve 真实实现还未接入，当前 Rust core 使用 fake/math proxy service。
- `trajectory_json` 目前只保留占位 JSON，后续应由 Serve 返回真实轨迹。
- Python `GrpcEpisodeClient` 是早期直连 Serve proto 的预留骨架；当前目标链路不使用它。
- 当前 reward 写入 VeRL 的 `rm_scores`，advantage 仍由 VeRL 自己计算。
