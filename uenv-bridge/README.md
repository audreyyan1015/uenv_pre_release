# uenv-bridge

`uenv-bridge` 是 UEnv 面向 VeRL 的训练框架适配层。当前主线只保留 pre-rollout 接管：VeRL 在 AgentLoop 阶段把 prompt/sample 交给 UEnv，UEnv Server/Worker 负责模型生成、环境 step、reward 和 trajectory，Bridge 再把结果包装成 VeRL 可继续训练的 `AgentLoopOutput`。

## 当前架构

```text
verl.trainer.main_ppo
  -> AgentLoopManager / AgentLoopWorker
  -> UEnvAgentLoop
  -> EpisodeRequest(prompt only)
  -> RustCoreEpisodeClient
  -> Rust adapter core
  -> EpisodeService function call
  -> UEnv Server / Worker
  -> EpisodeResult(response_ids, response_mask, trajectory, reward)
  -> UEnvAgentLoop
  -> AgentLoopOutput
  -> VeRL logprob / advantage / actor update
```

关键边界：

- VeRL 接入口是 `UEnvAgentLoop`，位置在 rollout 生成之前。
- Python 和 Rust adapter core 之间使用本地 gRPC，proto 位于 `../proto/uenv/v1/adapter_core.proto`。
- Rust adapter core 和 UEnv Server/Worker 之间是 Rust 函数调用，不要求 Serve 侧暴露 Python gRPC。
- 请求中只携带 prompt、采样参数、reward 配置和元数据；response 由 UEnv Server/Worker 生成并返回。

## 代码结构

```text
configs/uenv-agent-loop.yaml
scripts/run_verl_grpo_1step_with_uenv_agent_loop.sh
scripts/run_layer4_smoke_with_services.sh
scripts/verify_pre_rollout_rust_core_loop.py
scripts/dump_verl_pre_rollout_request.py

src/uenv/bridge/
  verl_agent_loop.py      # VeRL AgentLoop pre-rollout 入口
  agent_loop_clients.py   # fake / rust_core client 选择
  clients.py              # Python -> Rust adapter core client
  protocol.py             # Python 内部 EpisodeRequest / EpisodeResult dataclass
  utils.py                # JSON 和 prompt 工具函数

core/
  src/core.rs             # SampleEnvelope -> server EpisodeRequest -> SampleResult
  src/server_api.rs       # EpisodeService 函数边界
  src/service.rs          # adapter core gRPC service
```

## VeRL 接入方式

VeRL 配置中启用自定义 AgentLoop：

```bash
actor_rollout_ref.rollout.agent.default_agent_loop=uenv_agent
actor_rollout_ref.rollout.agent.agent_loop_config_path=/tmp/uenv-bridge/configs/uenv-agent-loop.yaml
```

`configs/uenv-agent-loop.yaml` 会加载：

```yaml
- name: uenv_agent
  _target_: uenv.bridge.verl_agent_loop.UEnvAgentLoop
```

常用环境变量：

```bash
export UENV_AGENT_LOOP_CLIENT=rust_core
export UENV_ADAPTER_CORE_ENDPOINT=127.0.0.1:50053
export UENV_ADAPTER_CORE_AUTO_START=1
export UENV_ADAPTER_CORE_BINARY=/tmp/uenv-bridge/core/target/debug/uenv-adapter-core
export UENV_ROLLOUT_MODEL_ENDPOINT=http://127.0.0.1:18080/v1
export UENV_ROLLOUT_MODEL_NAME=policy-model
```

默认 `UENV_AGENT_LOOP_CLIENT=rust_core` 走 adapter-core `server` backend 与远端 Worker 真实链路；仅 Python 单测可显式注入 `StaticRolloutEpisodeClient`。

## EpisodeRequest

`UEnvAgentLoop.build_episode_request()` 生成的请求是 prompt-only。典型 payload：

```json
{
  "protocol_version": "1.0",
  "framework": "verl",
  "correlation_id": "batch-a-0",
  "env_config": {
    "task_name": "math",
    "data_source": "openai/gsm8k",
    "raw_prompt": "user: What is 2 + 2?"
  },
  "model_endpoint": {
    "endpoint_type": "http",
    "url": "http://127.0.0.1:18080/v1",
    "model_name": "policy-model",
    "generation_config": {
      "temperature": 1.0,
      "max_new_tokens": 32
    }
  },
  "episode_config": {
    "max_steps": 10,
    "max_turns": 1,
    "seed": 42,
    "initial_observation": {
      "raw_prompt": [{"role": "user", "content": "What is 2 + 2?"}],
      "prompt_text": "user: What is 2 + 2?",
      "prompt_ids": [10, 11, 12],
      "token_source": "verl_agent_loop"
    },
    "stop_conditions": ["done", "max_steps", "timeout"]
  },
  "reward_config": {
    "reward_type": "rubric",
    "rubric_config": {
      "ground_truth": "4"
    }
  },
  "metadata": {
    "batch_id": "batch-a",
    "sample_index": 0,
    "data_source": "openai/gsm8k",
    "required_result_fields": [
      "response_ids",
      "response_mask",
      "response_text",
      "reward",
      "trajectory",
      "finish_reason"
    ]
  }
}
```

## EpisodeResult

UEnv Server/Worker 返回的 `EpisodeResult` 至少需要满足：

| 字段 | 要求 |
|---|---|
| `episode_id` / `request_id` | 必须等于输入请求 id，用于回填原 sample |
| `status` | `completed` 表示可用于训练 |
| `summary.total_reward` | 写入 `AgentLoopOutput.reward_score` |
| `summary.terminate_reason` | 记录结束原因 |
| `trajectory.steps` | 保留环境执行轨迹 |
| 最后一步 `info.response_ids` | 推荐返回 JSON 数组字符串，例如 `[201,202]` |
| 最后一步 `info.response_mask` | 推荐返回 JSON 数组字符串，例如 `[1,1]` |
| 最后一步 `info.response_text` | 调试和 fallback 编码使用 |

如果没有 `response_ids`，`UEnvAgentLoop` 会把最后一步 `action` 或 `response_text` 用 VeRL tokenizer 重新编码。真实训练联调建议直接返回 token ids，避免 tokenizer 或 chat template 不一致。

## Serve 侧对接

Serve 侧只需要关注 Rust adapter core 暴露的函数边界：

```rust
pub trait EpisodeService: Send + Sync {
    fn submit_episode_batch(
        &self,
        requests: Vec<EpisodeRequest>,
    ) -> impl Future<Output = Result<Vec<EpisodeResult>, EpisodeServiceError>> + Send;
}
```

定义位置：

```text
core/src/server_api.rs
```

数据结构来自 `uenv-server` 的 proto 生成类型。Adapter core 会把 Python 传入的 sample payload 转成 server `EpisodeRequest`，Serve 实现负责调度 worker、调用模型、执行环境和计算 reward，然后返回同数量的 `EpisodeResult`。

Serve 返回顺序可以不同，但每个结果的 `episode_id` 必须能对应输入请求。缺失、重复或未知 id 会被 adapter core 视为错误。

## 四层验证

以下命令统一使用 `localhost/uenv-bridge-verl:layer4-build`。首次运行 Layer 2 会下载 Rust 依赖，建议保留 `tmp/cargo-home` 和 `tmp/cargo-target` 作为本地缓存。

```bash
cd /data/ronghao/uenv/uenv-bridge
export IMAGE=localhost/uenv-bridge-verl:layer4-build
mkdir -p tmp/cargo-home tmp/cargo-target
```

| Layer | 内容 | 前置条件 | 期望结果 |
|---|---|---|---|
| 1 | Python AgentLoop 单测 | bridge image | `test_verl_agent_loop` 通过 |
| 2 | Rust adapter core 单测 | bridge image | core request/result 映射通过 |
| 3 | Python 自动启动 Rust core | Layer 2 已构建 Rust binary | 输出 `reward_score` 和 `response_ids` |
| 4 | 真实 VeRL + Serve/Worker pre-rollout 联动 smoke test | bridge image、GPU、Rust binaries | VeRL `Training Progress: 100%`，`critic/score/mean` 可见 |

Layer 1：Python AgentLoop 单测。

```bash
podman run --rm --entrypoint bash --network host \
  -v /data/ronghao/uenv:/data/ronghao/uenv \
  -w /data/ronghao/uenv/uenv-bridge \
  "$IMAGE" \
  -lc 'set -euo pipefail
export PYTHONPATH=src
python3 -m unittest discover -s tests -v'
```

Layer 2：Rust adapter core 单测。

```bash
podman run --rm --entrypoint bash --network host \
  -v /data/ronghao/uenv:/data/ronghao/uenv \
  -w /data/ronghao/uenv/uenv-bridge \
  "$IMAGE" \
  -lc 'set -euo pipefail
export CARGO_HOME=/data/ronghao/uenv/uenv-bridge/tmp/cargo-home
export CARGO_TARGET_DIR=/data/ronghao/uenv/uenv-bridge/tmp/cargo-target
cargo build --manifest-path ../Cargo.toml -p uenv-adapter-core --bin uenv-adapter-core
cargo test --manifest-path ../Cargo.toml -p uenv-adapter-core'
```

Layer 3：Python AgentLoop 自动连接 Rust core 的本地 gRPC 闭环。

```bash
podman run --rm --entrypoint bash --network host \
  -v /data/ronghao/uenv:/data/ronghao/uenv \
  -w /data/ronghao/uenv/uenv-bridge \
  "$IMAGE" \
  -lc 'set -euo pipefail
export PYTHONPATH=src
export UENV_ADAPTER_CORE_BINARY=/data/ronghao/uenv/uenv-bridge/tmp/cargo-target/debug/uenv-adapter-core
python3 scripts/verify_pre_rollout_rust_core_loop.py --skip-build'
```

Layer 4：真实 VeRL + Serve/Worker pre-rollout 联动 smoke test。

Layer 4 当前主入口是 pre-rollout AgentLoop wrapper。脚本会启动本地 mock OpenAI-compatible model endpoint、Rust adapter core 和 worker，然后让真实 `verl.trainer.main_ppo` 通过 `UEnvAgentLoop` 在 rollout 前把 sample 交给 UEnv：

```bash
cd /data/ronghao/uenv/uenv-bridge
IMAGE=localhost/uenv-bridge-verl:layer4-build \
TRAINING_STEPS=1 \
SAMPLE_COUNT=2 \
TRAIN_BATCH_SIZE=2 \
ROLLOUT_N=2 \
./scripts/run_layer4_smoke_with_services.sh
```

如果需要验证多步训练，把 `TRAINING_STEPS` 改为 2。Layer 4 脚本默认设置 `ROLLOUT_FREE_CACHE_ENGINE=False` 和 `ROLLOUT_ENABLE_SLEEP_MODE=False`，用于避开 vLLM 在多步 smoke test 中每步 sleep/free cache 时可能触发的 Python `multiprocessing.resource_tracker` shared-memory 清理异常。

```bash
IMAGE=localhost/uenv-bridge-verl:layer4-build \
TRAINING_STEPS=2 \
SAMPLE_COUNT=4 \
TRAIN_BATCH_SIZE=2 \
ROLLOUT_N=2 \
./scripts/run_layer4_smoke_with_services.sh
```

如需接入真实模型服务，设置：

```bash
START_MOCK_MODEL=0 \
UENV_ROLLOUT_MODEL_ENDPOINT=http://<model-host>:<port>/v1 \
UENV_ROLLOUT_MODEL_NAME=<model-name> \
IMAGE=localhost/uenv-bridge-verl:layer4-build \
./scripts/run_layer4_smoke_with_services.sh
```

## 构建 VeRL image

如果本地还没有 `localhost/uenv-bridge-verl:layer4-build`，使用下面的命令构建：

```bash
cd /data/ronghao/uenv/uenv-bridge
IMAGE=localhost/uenv-bridge-verl:layer4-build ./scripts/build_verl_bridge_image.sh
```

该脚本默认基于 `docker.io/verlai/verl:vllm011.latest`，安装 Rust、Cargo 和 `protoc`，并把当前 `uenv-bridge` 代码放入镜像，供 Layer 1-4 验证使用。如需指定基础镜像，可以设置 `BASE_IMAGE=<verl-image>`。

dump VeRL prompt batch 的 pre-rollout 请求形状：

```bash
PYTHONPATH=/workspace/verl:src \
python3 scripts/dump_verl_pre_rollout_request.py \
  --data-file tmp/verl_grpo_1step_agent_loop_data/train.parquet \
  --out-dir /tmp/uenv-verl-pre-rollout-dump \
  --batch-size 2 \
  --rollout-n 2
```

输出重点文件：

```text
episode_request_batch.json
episode_request_0.json
mock_episode_result_batch.json
combined_gen_batch_summary.json
```

## 说明

- 当前 Bridge 只维护 VeRL pre-rollout AgentLoop 路线。
- 多卡、权重同步和高并发吞吐还没有作为验收目标。
- Worker 调用的模型服务必须与 VeRL 当前 actor 权重保持一致，否则训练信号不可信。
- `SampleEnvelope` / `SampleResult` 只属于 Python 到 Rust adapter core 的本地协议，Serve 协作者通常不需要直接处理。
