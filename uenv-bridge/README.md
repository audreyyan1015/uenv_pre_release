# UEnv Bridge

UEnv Bridge 是评测程序或训练框架一侧的适配层。当前主线实现把 VeRL v0.7.1 的 sample 在 pre-rollout 阶段交给 UEnv，并把 response token、mask、reward 和 trajectory 还原成 `AgentLoopOutput`。

## 组件边界

```text
VeRL → UEnv Bridge → UEnv Server → UEnv Worker
```

- Bridge：本目录的 Python 包 `src/uenv/bridge/`。
- Server：中心服务，接收 Bridge 请求并选择 Worker。
- Worker：执行模型生成、环境 step 和 reward。

本页面向 Bridge 维护者。`core/`、`AdapterCoreService` 和 `UENV_ADAPTER_CORE_ENDPOINT` 是现有源码与协议名称；普通用户只需把它们理解为 UEnv Server 的实现入口或连接地址。

## 主流程

```text
VeRL AgentLoop sample
  -> UEnvAgentLoop
  -> SampleEnvelope / EpisodeRequest
  -> Server 调度 Worker
  -> EpisodeResult / SampleResult
  -> AgentLoopOutput
  -> VeRL logprob / advantage / update
```

Bridge 在 Server 乱序返回时按 `request_id` 对齐结果。未知、重复或缺失 ID 都是协议错误。

## 代码结构

| 路径 | 作用 |
|---|---|
| `src/uenv/bridge/verl_agent_loop.py` | VeRL pre-rollout hook 与字段映射 |
| `src/uenv/bridge/clients.py` | Bridge 到 Server 的 gRPC 客户端 |
| `src/uenv/bridge/protocol.py` | Python 内部 Episode 类型 |
| `src/uenv/bridge/model_gateway.py` | 向 Worker 暴露当前训练模型 |
| `configs/uenv-agent-loop.yaml` | VeRL AgentLoop 配置 |
| `core/` | Server 可执行程序的 Rust 源码（现有代码名 adapter-core） |

## VeRL 配置

VeRL 配置中启用自定义 AgentLoop：

```bash
actor_rollout_ref.rollout.agent.default_agent_loop=uenv_agent
actor_rollout_ref.rollout.agent.agent_loop_config_path=/path/uenv-bridge/configs/uenv-agent-loop.yaml
```

正式部署的最小连接配置：

```bash
export UENV_AGENT_LOOP_CLIENT=rust_core
export UENV_ADAPTER_CORE_ENDPOINT='SERVER_HOST:50051'
export UENV_ADAPTER_CORE_AUTO_START=false
```

每条训练数据都应显式提供 `env_type`。不要用全局默认值掩盖数据缺失。

GPU 主机和 Worker 分开时，还需要向 Worker 暴露当前策略模型：

```bash
export UENV_MODEL_GATEWAY_ENABLED=true
export UENV_MODEL_GATEWAY_BIND_HOST='GPU_HOST'
export UENV_MODEL_GATEWAY_PORT=18080
export UENV_MODEL_GATEWAY_PUBLIC_URL='http://GPU_HOST:18080/v1'
```

`PUBLIC_URL` 必须从 Worker 可达。密钥不进入 sample payload、请求记录或轨迹。

## 结果要求

训练结果至少提供：

| UEnv 字段 | VeRL 用途 |
|---|---|
| `request_id` | 回填原 sample |
| terminal `status` | 判断是否可进入训练 |
| response IDs / mask | 构造训练 token 与 loss mask |
| reward | `AgentLoopOutput.reward_score` |
| trajectory / trajectory ID | 调试与可追溯 |
| policy version / logprobs（异步时） | 新鲜度和 off-policy 处理 |

Bridge 优先使用 Worker 返回的原始 token。只有通用兼容路径可以从 response text 重新编码；SWE 训练默认要求完整 response trace。

## 用户入口

用户通过 release 命令运行，不直接调用仓库内部脚本：

```text
uenv train run-task ...
uenv train run-swe ...
```

完整文档：

- [自定义强化学习框架接入](../Docs/guide/4-接入强化学习框架/01-custom-framework.md)
- [以 VeRL 为例接入 UEnv](../Docs/guide/4-接入强化学习框架/02-verl.md)
- [强化学习训练指南](../Docs/guide/3-运行任务/07-post-training.md)
- [强化学习训练案例](../Docs/guide/3-运行任务/02-cases.md#强化学习训练)

## 开发验证

修改 Bridge 或 Server 映射后，验证以下边界：

1. `tests/test_verl_agent_loop.py` 的 sample/request 和 result/output 映射。
2. `cargo test -p uenv-adapter-core` 的批次、ID 与 Episode 转换。
3. Bridge 连接真实 Server/Worker 后的 response、reward、trajectory 闭环。
4. 目标 VeRL 作业完成计划的模型更新，并写出指标和 checkpoint。

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

Layer 4：真实 VeRL + Server/Worker pre-rollout 联动训练入口。

Layer 4 当前主入口是 pre-rollout AgentLoop wrapper。正式训练和分布式联调统一使用 `scripts/train/launchers/common/run_verl_uenv_grpo.sh`，由真实 `verl.trainer.main_ppo` 通过 `UEnvAgentLoop` 在 rollout 前把 sample 交给 UEnv：

```bash
cd /data/ronghao/uenv/uenv-bridge
./scripts/train/launchers/common/run_verl_uenv_grpo.sh
```

分布式联调时该入口只运行 VeRL/adapter 侧逻辑，并连接 server 侧已经启动的 Rust adapter core。此时 Worker 可以使用自己的模型服务，adapter 不负责启动或替代 Worker 的模型。

每次 Layer 4 运行都会在对应服务日志目录写入 adapter 侧结果记录：

```text
logs/layer4_distributed/<RUN_ID>/agent-loop-results.jsonl
tmp/layer4_smoke/<RUN_ID>/agent-loop-results.jsonl
```

可以直接汇总 reward、response 和 trajectory 摘要：

```bash
python3 scripts/summarize_agent_loop_results.py \
  logs/layer4_distributed/<RUN_ID>/agent-loop-results.jsonl
```

结果记录中重点看 `reward`、`trajectory`、`response_ids` 和 `verl_response_ids`。前者表示 Server/Worker 是否直接返回 token ids，后者表示最终回填给 VeRL 的 token ids。

如果需要验证多步训练，优先使用当前训练入口并调整 `TRAINING_STEPS` 等环境变量。Layer 4 脚本默认设置 `ROLLOUT_FREE_CACHE_ENGINE=False` 和 `ROLLOUT_ENABLE_SLEEP_MODE=False`，用于避开 vLLM 在多步 smoke test 中每步 sleep/free cache 时可能触发的 Python `multiprocessing.resource_tracker` shared-memory 清理异常。

```bash
IMAGE=localhost/uenv-bridge-verl:layer4-build \
TRAINING_STEPS=2 \
SAMPLE_COUNT=4 \
TRAIN_BATCH_SIZE=2 \
ROLLOUT_N=2 \
CUDA_VISIBLE_DEVICES_IN_CONTAINER=0 \
ROLLOUT_GPU_MEMORY_UTILIZATION=0.2 \
./scripts/train/launchers/common/run_verl_uenv_grpo.sh
```

`CUDA_VISIBLE_DEVICES_IN_CONTAINER` 用于选择容器内训练 GPU；`ROLLOUT_GPU_MEMORY_UTILIZATION` 会传给 VeRL 的 vLLM rollout server。显存紧张时可以降低该值，避免 vLLM 启动时报 free memory 不足。

如需接入真实模型服务，设置：

```bash
START_MOCK_MODEL=0 \
UENV_ROLLOUT_MODEL_ENDPOINT=http://<model-host>:<port>/v1 \
UENV_ROLLOUT_MODEL_NAME=<model-name> \
IMAGE=localhost/uenv-bridge-verl:layer4-build \
./scripts/train/launchers/common/run_verl_uenv_grpo.sh
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
- 不要使用 `fake` client 作为真实接入验收；它只用于孤立的 Python 单元测试。
