# proto-clean 替换前剩余改动清单

本文档整理从当前“兼容迁移版”切换到 `/home/uenv/proto-clean` 干净 proto 前，还需要在 bridge、server、worker 三层完成的改动。

当前正式 proto 仍是新旧并存：

- 新 typed 字段已经加入并被代码使用。
- 主链路代码已经停止发送或读取已清理的旧兼容字段；旧字段主要还存在于正式兼容 proto 和已生成 protobuf 代码中。
- `/home/uenv/proto-clean/proto` 已正式覆盖 `/home/uenv/proto`；当前正式 proto 已是 clean 版。

## 旧字段扫描结论

本轮扫描范围：

- `/home/uenv/proto`
- `/home/uenv/proto-clean`
- `/home/uenv/uenv-bridge`
- `/home/uenv/uenv-server`
- `/home/uenv/uenv-worker`

扫描关键词：

- `payload_json`
- `meta_json`
- `model_output_json`
- `legacy_parallel_mode_fallback_used`
- `payload.metadata.parallel_mode`
- `parse_payload_model_endpoint`
- `response_logprobs`
- `response_log_probs`
- `cancelled:`

结论：

- `legacy_parallel_mode_fallback_used`、`payload.metadata.parallel_mode`、`parse_payload_model_endpoint`、`cancelled:` 在主代码路径中已无残留。
- `payload_json/meta_json/model_output_json` 仍存在于正式兼容 proto、已生成 Python protobuf 文件和历史联调文档中，等 clean proto 正式替换、重新生成并更新旧文档后消失。
- `uenv-server/stress_test/*.py` 已迁移到 typed `SampleEnvelope` 字段，不再构造旧 `payload_json/meta_json`。
- `uenv-worker/src/episode/rollout_meta.rs` 已停止从 payload、metadata、metadata.extra_info 读取 rollout 旧键。
- `uenv-bridge/src/uenv/bridge/verl_agent_loop.py` 和 `sitecustomize.py` 中的 `response_logprobs` 是 VeRL 自身输出字段名，不等同于 UEnv 协议旧字段。
- `uenv-bridge/src/uenv/bridge/clients.py` 里的 `_payload_json()` 是解析 Python dataclass payload 的 helper，不是旧 `SampleEnvelope.payload_json` 字段发送入口。
- `uenv-worker/src/episode/payload.rs` 和 `model_client.rs` 中的 `payload_json` 是 worker 内部解析 `EpisodeRequest.payload` 的局部变量名，不是旧 `SampleEnvelope.payload_json` 字段。
- 仓库内已扫描 `CancelEpisodeResponse.cancelled`：主代码和脚本没有读取旧字段；仅正式兼容 proto 保留字段号，历史联调文档已改为拆分字段。

## 本次全局收口扫描

这次扫描的重点不是“字段名是否还出现”，而是确认它还会不会作为旧协议入口影响运行链路。结论如下。

### 运行链路已清理

- Bridge 不再发送 `SampleEnvelope.payload_json/meta_json/model_output_json`，adapter-core 也不再从这些旧字段 fallback。
- Bridge 不再从 `StepRecord.info["response_ids"]` / `StepRecord.info["response_mask"]` 读取 token trace。
- Server 和 worker 已把 `EpisodeRequest.parallel_mode` 作为唯一 canonical 来源，不再从 `EpisodeRequest.metadata["parallel_mode"]` 或 `payload.metadata.parallel_mode` fallback。
- Worker 不再从 `EpisodeRequest.payload.model_endpoint/model_name/generation_config` 读取模型调用配置，改为读取 typed `EpisodeRequest.model_endpoint_config`。
- Worker 不再从 payload、metadata、metadata.extra_info 读取 `rollout_param_version/rollout_policy_version/rollout_log_probs/response_ids/response_mask` 等旧 rollout 键。
- Server 构造 `CancelEpisodeResponse` 时只显式设置 `server_cancelled` 和 `worker_cancel_*`，旧 `cancelled` 只因兼容 proto 默认值存在。

### 仍然预期保留的位置

- 正式兼容 proto 仍保留旧字段号，例如 `SampleEnvelope.payload_json/meta_json/model_output_json`、`EpisodeRequest.model_endpoint`、`CancelEpisodeResponse.cancelled`。
- 已生成 Python protobuf 文件仍包含旧字段描述，这是兼容 proto 尚未被 clean proto 覆盖前的预期结果。
- `/home/uenv/proto-clean/proto` 中对应旧字段已经用 `reserved` 或删除方式表达未来干净协议。

### 不是旧协议入口但名字相似的位置

- `uenv-bridge/src/uenv/bridge/clients.py::_payload_json()` 是解析 Python dataclass `EpisodeRequest.payload` 的 helper，不是旧 `SampleEnvelope.payload_json`。
- `uenv-worker/src/episode/payload.rs` 和 `model_client.rs` 里的 `payload_json` 是 worker 内部解析 `EpisodeRequest.payload` 的局部变量名。
- `uenv-bridge/src/uenv/bridge/verl_agent_loop.py` 和 `sitecustomize.py` 里的 `response_logprobs` 是 VeRL 自身输出字段名，不等同于 UEnv 协议旧字段。
- Bridge 入口层仍可能在 Python payload 中暂存 `model_endpoint/generation_config`，但 adapter-core 会把它提升成 typed `SampleEnvelope.model_endpoint` / `EpisodeRequest.model_endpoint_config`，不会再放进 worker payload 作为模型配置入口。

### 仍需人工收尾的位置

- 如果存在仓库外 admin client，需要确认它们已经从 `CancelEpisodeResponse.cancelled` 迁到 `server_cancelled/worker_cancel_*`。
- 真正的 LLM/token producer 仍要产出 typed rollout 数据：`rollout_param_version`、`rollout_policy_version`、`rollout_log_probs`、`rollout_trace.response_ids/response_mask`。当前 runner/client/worker schema 已能透传，但不会凭空生成这些训练字段。
- 本轮文档只维护 `/home/uenv/proto-clean/REMAINING_CLEAN_MIGRATION.md`、`/home/uenv/proto-clean/CLEAN_PROTO_FIELDS.md`、`/home/uenv/code-change-summary-since-last-commit.md`；其他 README、旧联调文档和脚本说明不作为当前处理范围。

## Bridge / adapter-core

### 当前已经做了

- Python bridge 组 `SampleEnvelope` 时已经填充新 typed 字段：
  - `parallel_mode`
  - `env_config_json`
  - `episode_config_json`
  - `reward_config_json`
  - `model_endpoint`
  - `timeout_seconds`
  - `correlation_id`
  - `sample_context_json`
  - `env_package_id`
  - `env_package_version`
- Python bridge 已停止构造旧 `SampleEnvelope.payload_json/meta_json/model_output_json` 字段。
- Python bridge 已读取 adapter-core 返回的 async 训练结果字段：
  - `rollout_param_version`
  - `rollout_policy_version`
  - `rollout_log_probs`
- Python bridge 已从 typed `rollout_trace` / `StepRecord.response_ids,response_mask` 读取 `response_ids/response_mask`。
- Python bridge 已删除旧 `StepRecord.info["response_ids"]` / `StepRecord.info["response_mask"]` fallback。
  - 文件：`/home/uenv/uenv-bridge/src/uenv/bridge/verl_agent_loop.py`
  - 测试：`/home/uenv/uenv-bridge/tests/test_verl_agent_loop.py`
- Rust adapter-core 内部 `SampleEnvelope` 已删除旧 `payload_json/meta_json/model_output_json` 字段，并忽略 proto 兼容层里的旧字段。
- Rust adapter-core 已只从 typed `SampleEnvelope` 字段构造 `EpisodeRequest`，不再从旧 `payload_json/meta_json/model_output_json` fallback。
- Rust adapter-core 已把 `response_ids/response_mask` 放进 `rollout_trace`，不再只塞进 `info`。
- Rust adapter-core 当前会重新组装 `EpisodeRequest.payload`。代码确认当前 payload 会包含：
  - 通用字段：`request_id`、`question`、`dataset`、`metadata`。
  - SWE 字段：`instance_id`、`benchmark_variant`、`use_gold_patch`、`command_mode`、`execution_mode`、`mode`、`agent_bridge_id`、`agent_bridge_version`、`agent_pool_id`、`driver_entrypoint`、`workspace_dir`、`llm_config_path`、`max_iterations`。
  - adapter-core 已停止把输入侧预生成 rollout 旧键合并到 payload：`response_text`、`response_ids`、`response_mask`、`rollout_log_probs`、`response_logprobs`、`response_log_probs`、`rollout_param_version`、`rollout_policy_version`、`uenv_model_version`。
- Rust adapter-core 已把模型调用配置从 `SampleEnvelope.model_endpoint` 映射到 `EpisodeRequest.model_endpoint_config`：
  - `url`
  - `model_name`
  - `generation_config_json`
  - `max_retries`
- Rust adapter-core 已停止把这些 typed 协议字段重复放进 `EpisodeRequest.payload`：
  - `model_endpoint`
  - `model_name`
  - `generation_config`
  - `correlation_id`
  - `env_package_id`
  - `env_package_version`
- stress test 脚本已迁移为 typed `SampleEnvelope`：
  - `/home/uenv/uenv-server/stress_test/stress_test.py`
  - `/home/uenv/uenv-server/stress_test/stress_test_real.py`

### 切到 proto-clean 前还需要做

1. 等正式 proto 覆盖为 clean proto 后，重新生成 bridge/adapter-core protobuf 代码。
   - Rust adapter-core 生成代码要重新生成。
   - Python bridge 生成代码要重新生成：
     - `/home/uenv/uenv-bridge/src/uenv/bridge/gen/adapter_core_pb2.py`
     - `/home/uenv/uenv-bridge/src/uenv/bridge/gen/adapter_core_pb2_grpc.py`
     - `/home/uenv/uenv-bridge/src/uenv/bridge/gen/uenv/v1/adapter_core_pb2.py`

## Server

### 当前已经做了

- `RegisterWorkerRequest.load/max_load` 已在注册时进入 scheduler。
  - 文件：`/home/uenv/uenv-server/src/control_plane.rs`
  - server 重启后，worker 重新注册时可以立刻恢复真实负载视图。
- heartbeat 中的 `load/max_load` 继续更新 worker 负载和容量。
- dynamic admission 已按 scheduler 实际接受的注册结果调整容量，避免 active lease 重注册时错误增加 permit。
- `EpisodeRequest.parallel_mode` 已作为 server 边界的 canonical typed field。
  - 文件：`/home/uenv/uenv-server/src/service/prelude_and_guards.rs`
  - server 已停止从 `EpisodeRequest.metadata["parallel_mode"]` 和 `payload.metadata.parallel_mode` fallback。
  - request metadata 中的协议 key 仍会被清理，metadata 只保留上下文信息。
- `EpisodeResult` finalizer 已把协议字段保留在 typed 字段中，并过滤 request metadata 里的协议 key。
  - 文件：`/home/uenv/uenv-server/src/result_finalizer.rs`
- async 训练结果已校验 typed 字段：
  - `parallel_mode`
  - `rollout_param_version`
  - `rollout_policy_version`
  - `rollout_log_probs`
- Agent 完成上报已新增 typed `AgentJobCompleteRequest.rollout_trace`。
  - 字段：`rollout_trace = 18`
  - 文件：`/home/uenv/proto/uenv/v1/agent.proto`
  - server 已把它映射到 `EpisodeResult.trajectory.steps[0].rollout_trace`，用于承载 `response_ids/response_mask`。
  - 文件：`/home/uenv/uenv-server/src/service/episode.rs`
- OpenHands agent producer 已接入 typed `rollout_trace`。
  - runner 会从 `submit_result.json` 的 typed `rollout_trace` 或 `trajectory.steps[*].rollout_trace` 读取 `response_ids/response_mask`。
  - 如果 `trajectory_bundle.json` 存在，也会从其中的 typed `rollout_trace` 读取。
  - runner 不从旧 `info.response_ids/response_mask` 字符串 fallback。
  - AgentControlClient 会把读取到的数组写入 `AgentJobCompleteRequest.rollout_trace`。
  - runner/client 也已支持透传 typed async rollout 结果字段：
    - `parallel_mode`
    - `rollout_param_version`
    - `rollout_policy_version`
    - `rollout_log_probs`
    - `worker_start_ts`
    - `worker_finish_ts`
    - `result_ready_ts`
    - `worker_latency_ms`
    - `model_latency_ms`
  - 文件：
    - `/home/uenv/scripts/openhands/openhands_runner.py`
    - `/home/uenv/integrations/openhands/uenv_runtime/agent_client.py`
- `CancelEpisodeResponse` 已返回拆分后的 cancel 字段：
  - `server_cancelled`
  - `worker_cancel_attempted`
  - `worker_cancel_accepted`
  - `worker_cancel_code`
  - `worker_cancel_message`
- server 已停止显式构造旧汇总字段 `cancelled`。
- 仓库内 admin client / 脚本已扫描完成，没有发现读取 `CancelEpisodeResponse.cancelled`。

### 切到 proto-clean 前还需要做

1. 如存在仓库外 admin client，人工确认它们没有继续读取旧 `CancelEpisodeResponse.cancelled`。
   - 仓库内已经没有旧字段读取点。
   - 仓库外客户端必须读取：
     - `server_cancelled`
     - `worker_cancel_attempted`
     - `worker_cancel_accepted`
     - `worker_cancel_code`
     - `worker_cancel_message`

2. 保留 metadata 过滤逻辑。
   - `EpisodeRequest.metadata` 和 `EpisodeResult.metadata` 字段本身不删除。
   - 但应继续断言 metadata 不承载这些协议 key：
     - `parallel_mode`
     - `rollout_param_version`
     - `rollout_policy_version`
     - `rollout_log_probs`
     - timing latency 字段

3. 重新生成 server protobuf 代码并跑测试。
   - 覆盖正式 proto 后重新生成 Rust pb。
   - 至少跑：
     - `cargo test -p uenv-server`
     - cancel response 相关测试
     - parallel_mode typed-only 测试
     - async rollout result typed 字段测试
     - Agent `rollout_trace` 映射测试

## Worker

### 当前已经做了

- worker 注册时已上报当前负载和真实容量。
  - 文件：`/home/uenv/uenv-worker/src/control_plane/client.rs`
  - 字段：
    - `load`
    - `max_load`
- worker 已把 `EpisodeRequest.parallel_mode` 作为 canonical typed field。
  - 文件：`/home/uenv/uenv-worker/src/episode/async_context.rs`
  - 已停止从旧位置 fallback：
    - `EpisodeRequest.metadata["parallel_mode"]`
    - `payload.metadata.parallel_mode`
- worker model client 已改为从 typed `EpisodeRequest.model_endpoint_config` 读取模型调用配置。
  - 文件：
    - `/home/uenv/uenv-worker/src/episode/model_client.rs`
    - `/home/uenv/uenv-worker/src/episode/executor.rs`
  - `payload.model_endpoint` 已不再作为模型 endpoint 覆盖入口。
  - `payload.model_name`、`payload.generation_config` 已不再作为模型调用参数来源。
  - URL、model_name、generation_config_json、max_retries 都从 typed `ModelEndpoint` 读取；缺省时回退到 worker 本地 LLM 配置。
- worker 已删除旧的 `parse_payload_model_endpoint()` helper。
  - 文件：`/home/uenv/uenv-worker/src/llm.rs`
- worker model client 已停止把 `payload.response_text` 当成第一步预生成模型输出。
  - 异步 rollout 的模型结果应来自真实模型响应 body/header，而不是输入 payload 里的旧兼容键。
- worker 集成测试 `m5_episode_executor` 已迁移到 typed `EpisodeRequest.model_endpoint_config` + mock LLM。
- worker 已把 `response_ids/response_mask` 写入 `StepRecord.rollout_trace`。
  - 文件：`/home/uenv/uenv-worker/src/episode/rollout_meta.rs`
- worker 已把 async 训练结果写入 `EpisodeResult` typed 字段：
  - `rollout_param_version`
  - `rollout_policy_version`
  - `rollout_log_probs`
- worker 已过滤 `EpisodeResult.metadata` 中的协议 key。
- 普通 executor 路径已显式设置 `rollout_trace: None`，适配新 proto 字段。
- worker 已删除 SWE async 从 `EpisodeRequest.payload` / `metadata` / `metadata.extra_info` 提取 rollout 旧键的兼容入口。
  - 文件：`/home/uenv/uenv-worker/src/episode/rollout_meta.rs`
  - 删除入口：`extract_rollout_from_payload()`
  - SWE async 在没有真正 typed rollout 来源时会按缺少 async 字段失败，不再接受 payload 旧键伪造成功结果。
- SWE trajectory bundle schema 已支持 typed `rollout_trace`。
  - 文件：`/home/uenv/uenv-worker/src/swe/trajectory.rs`
  - 字段位置：`TrajectoryBundle.steps[*].rollout_trace.response_ids/response_mask`
  - 当前 gateway command/read/write 步骤默认 `rollout_trace: None`，不会伪造 token ids。
  - OpenHands runner 已能从这个 typed 字段透传到 `AgentJobCompleteRequest.rollout_trace`。
- worker 当前仍从 `EpisodeRequest.payload` 读取部分兼容字段：
  - math/reset：`question`、`dataset`。
  - SWE：`instance_id`、`use_gold_patch`、`command_mode`、`benchmark_variant`。

### 切到 proto-clean 前还需要做

1. 补齐 SWE async 结果 producer 的真实 typed rollout 来源。
   - Agent 协议、server 映射和 OpenHands runner/client 已经接好：`AgentJobCompleteRequest.rollout_trace` -> `EpisodeResult.trajectory.steps[0].rollout_trace`。
   - OpenHands runner 当前只透传已有 typed trace；worker trajectory bundle 也已经有 typed `steps[*].rollout_trace` 容器。
   - 如果 driver/gateway 产出的 `submit_result.json` / `trajectory_bundle.json` 没有真实 `rollout_trace`、rollout 版本、logprobs 或 timing 字段，完成上报仍不会凭空生成这些训练字段。
   - 普通模型路径已经从模型响应 body/header 解析 rollout 字段。
   - SWE async 旧 payload 入口已经删除；如果未来仍要支持原生 SWE harness async 成功结果，需要新增 typed 插件结果通道，把 rollout 版本、logprobs、response_ids/response_mask 写入 `EpisodeResult` / `RolloutTrace`。

2. 继续收紧 worker 对 `EpisodeRequest.payload` 的读取。
   - 文件：
     - `/home/uenv/uenv-worker/src/episode/model_client.rs`
     - `/home/uenv/uenv-worker/src/episode/payload.rs`
     - `/home/uenv/uenv-worker/src/episode/executor.rs`
   - `payload` 继续承载 worker 业务输入：
     - math：`question`、`dataset`
     - SWE：`instance_id`、`use_gold_patch`、`command_mode`、`benchmark_variant`
   - 模型调用参数不再从 payload 读取；统一来自 typed `EpisodeRequest.model_endpoint_config`。

3. 继续补充 worker 测试。
   - 保留 typed `EpisodeRequest.parallel_mode` 测试。
   - 增加断言：
     - `EpisodeResult.rollout_*` 有值。
     - `StepRecord.rollout_trace.response_ids/response_mask` 有值。
     - `EpisodeResult.metadata` 不含协议 key。
     - `StepRecord.info` 不含 `response_ids/response_mask`。

4. 重新生成 worker protobuf 代码并跑测试。
   - 覆盖正式 proto 后重新生成 Rust pb。
   - 至少跑：
     - `cargo test -p uenv-worker parallel_mode`
     - worker async rollout 相关测试
     - worker registration load/max_load 测试

## Protobuf 生成链路检查

仓库内生成链路当前分成 Rust 和 Python 两类：

- Rust server：由 `/home/uenv/uenv-server/build.rs` 在 `cargo build/test` 时通过 `tonic_prost_build` 编译到 `OUT_DIR`，不提交 Rust 生成文件。
  - 直接编译：`server.proto`、`scheduler.proto`、`agent.proto`、`uenv-worker/proto/worker_service.proto`
  - 通过 import 影响：`episode.proto`、`common.proto` 等
- Rust worker：由 `/home/uenv/uenv-worker/build.rs` 在 `cargo build/test` 时通过 `tonic_prost_build` 编译到 `OUT_DIR`，不提交 Rust 生成文件。
  - 直接编译：`adapter_core.proto`、`agent.proto`、`common.proto`、`episode.proto`、`scheduler.proto`、`server.proto`、`wal.proto`、`worker_service.proto`、`plugin.proto`
- Rust adapter-core：由 `/home/uenv/uenv-bridge/core/build.rs` 在 cargo 构建时生成，只直接编译 `adapter_core.proto`，会受 `adapter_core.proto` import 的 message 影响。
- Python bridge：仓库内提交的 Python protobuf 文件主要是 adapter-core 这一组，生成脚本是：
  - `/home/uenv/uenv-bridge/scripts/utils/generate_adapter_core_proto.sh`
  - 输出：
    - `/home/uenv/uenv-bridge/src/uenv/bridge/gen/adapter_core_pb2.py`
    - `/home/uenv/uenv-bridge/src/uenv/bridge/gen/adapter_core_pb2_grpc.py`
    - `/home/uenv/uenv-bridge/src/uenv/bridge/gen/uenv/v1/adapter_core_pb2.py`
    - `/home/uenv/uenv-bridge/src/uenv/bridge/gen/uenv/v1/adapter_core_pb2_grpc.py`
- OpenHands runtime Python：当前已生成并提交/准备提交：
  - `/home/uenv/integrations/openhands/uenv_runtime/gen/uenv/v1/agent_pb2.py`
  - `/home/uenv/integrations/openhands/uenv_runtime/gen/uenv/v1/common_pb2.py`
  - `/home/uenv/integrations/openhands/uenv_runtime/gen/uenv/v1/episode_pb2.py`
  - 重新生成时需要同时包含 `common.proto`、`episode.proto`、`agent.proto`，否则 `AgentJobCompleteRequest.rollout_trace` 依赖的 `RolloutTrace` 类型会缺 import。
- `/home/uenv/scripts/proto-gen.sh` 和 Makefile 中的 `proto-*` target 是较早的全量/半全量生成入口，会写到 `uenv-bridge/src/gen`、`uenv-server/src/gen`、`uenv-worker/src/gen` 等路径；当前 Rust 主链路走 `build.rs`，Python bridge 主代码导入 `uenv.bridge.gen.adapter_core_pb2`，因此 clean proto 替换时不建议优先用这些旧全量入口。
- Makefile 的 `proto-agent-python` 当前只显式生成 `agent.proto`，clean proto 替换后如果继续用于 OpenHands runtime，需要补齐 `common.proto` 和 `episode.proto` 或改用明确的 `protoc -I proto --python_out=integrations/openhands/uenv_runtime/gen proto/uenv/v1/common.proto proto/uenv/v1/episode.proto proto/uenv/v1/agent.proto`。

clean proto 替换会影响的生成/检查对象：

- Rust 编译期生成：`uenv-server`、`uenv-worker`、`uenv-adapter-core` 的 `OUT_DIR` protobuf。
- Python bridge adapter-core 生成文件：`uenv-bridge/src/uenv/bridge/gen/*adapter_core_pb2*.py`。
- OpenHands runtime 生成文件：`integrations/openhands/uenv_runtime/gen/uenv/v1/{common,episode,agent}_pb2.py`。
- 不应作为提交目标的旧生成目录：`uenv-server/src/gen`、`uenv-worker/src/gen`、`uenv-bridge/src/gen`，除非明确决定恢复旧全量生成模式。

## clean proto dry-run 预检结果

本轮 dry-run 只在临时副本中执行，没有修改正式 `/home/uenv/proto`，也没有重启服务。

- 临时目录：`/tmp/uenv-clean-proto-dryrun.EUGO7a`
- 操作方式：复制 `/home/uenv` 到临时目录，删除临时目录里的 `proto/`，再用 `proto-clean/proto` 覆盖临时目录里的 `proto/`。
- 目的：提前发现 clean proto 删除旧字段后，当前代码还会在哪些地方编译不过。

已发现的阻塞点都集中在旧 `EpisodeRequest.model_endpoint`。这些阻塞点已在正式代码中按 clean 方向修复：

| 文件 | 位置 | 问题 | 当前修复 |
|---|---:|---|---|
| `/home/uenv/uenv-server/src/service/episode.rs` | 约 `658` 行 | 构造 `AgentJob` 时仍读取 `req.model_endpoint`。 | 已改为从 `req.model_endpoint_config.as_ref().map(|endpoint| endpoint.url.clone()).unwrap_or_default()` 取 URL。 |
| `/home/uenv/uenv-worker/src/main.rs` | 约 `121` 行 | 本地 dispatch 示例构造 `EpisodeRequest` 时仍写 `model_endpoint: String::new()`。 | 已删除旧字段赋值。 |
| `/home/uenv/uenv-bridge/core/src/core.rs` | 约 `204` 行 | adapter-core 构造 `EpisodeRequest` 时仍填旧 `model_endpoint` 字段。 | 已删除旧 `model_endpoint` 赋值，保留已有 `model_endpoint_config`。 |
| `/home/uenv/uenv-worker/src/wal/mod.rs` | 约 `195` 行 | WAL 测试 fixture 构造 `EpisodeRequest` 时仍写 `model_endpoint: String::new()`。 | 已删除旧字段赋值。 |
| `/home/uenv/uenv-worker/tests/m5_episode_executor.rs` | 约 `33` 行 | 测试中调用 `request.model_endpoint.clear()`。 | 已删除该行；测试后面继续设置 typed `request.model_endpoint_config`。 |

验证进展：

- 临时修补前，`cargo check -p uenv-server`、`cargo check -p uenv-worker`、`cargo check -p uenv-adapter-core` 会分别暴露旧 `model_endpoint` 编译错误。
- 在临时副本中修掉 server、worker main、adapter-core 三处旧字段后，三项 `cargo check` 已能通过。
- 继续执行 `cargo test --no-run` 后，又发现 worker WAL 测试 fixture 和 `m5_episode_executor` 测试里的两处旧字段引用。
- 正式代码已处理上表 5 处后，`cargo check -p uenv-server`、`cargo check -p uenv-worker`、`cargo check -p uenv-adapter-core` 均通过。
- `cargo test -p uenv-adapter-core` 通过，11 个测试通过。
- worker 针对性测试本轮未继续跑：远端审批在执行 `cargo test -p uenv-worker m5_single_round_math_matches_expected_reward_and_status` 前超时。后续可单独补跑该测试和 WAL 相关测试。

clean proto 正式替换后的生成动作已完成：

1. 已用 `/home/uenv/proto-clean/proto` 覆盖正式 `/home/uenv/proto`。
2. 已跑 Rust 三个包测试，让 `build.rs` 重新生成 Rust protobuf 并暴露 schema 编译错误：
   - `cargo test -p uenv-server`
   - `cargo test -p uenv-worker`
   - `cargo test -p uenv-adapter-core`
3. 已重新生成 Python bridge adapter-core protobuf：
   - 顶层：`uenv-bridge/src/uenv/bridge/gen/adapter_core_pb2.py`
   - 嵌套：`uenv-bridge/src/uenv/bridge/gen/uenv/v1/adapter_core_pb2.py`
   - `adapter_core_pb2_grpc.py` 已恢复相对 import。
4. 已重新生成 OpenHands runtime Python protobuf：
   - `cd /home/uenv && protoc -I proto --python_out=integrations/openhands/uenv_runtime/gen proto/uenv/v1/common.proto proto/uenv/v1/episode.proto proto/uenv/v1/agent.proto`
5. 已跑 Python bridge 单测：
   - `python3 -m unittest uenv-bridge/tests/test_verl_agent_loop.py`
6. 已跑 OpenHands typed trace/client 临时验证，确认 `AgentJobCompleteRequest.rollout_trace` 仍可构造。
7. 已跑 `git diff --check` 覆盖正式 proto、生成文件和关键代码路径。

## 后续剩余事项

1. 补齐真实 rollout 来源：OpenHands runner/client 已能透传 typed `rollout_trace`；如仍需要原生 SWE harness async，再定义插件返回 rollout 元数据的 typed 通道。
2. 如存在仓库外 admin client，确认它们已迁到 `server_cancelled/worker_cancel_*`。
3. 提交前确认没有把 `uenv-server/src/gen`、`uenv-worker/src/gen`、`uenv-bridge/src/gen` 这类旧生成目录误加入提交。
4. 部署和重启服务尚未执行；如需上线，需要单独走 build/install/restart 和 PID/listener 验证。
