# 代码变动整理：相较于上一提交 5fb8fdc

基准提交：`5fb8fdc Merge pull request 'Feature/verl bridge adapter' (#22) from feature/verl-bridge-adapter into bridge-alignment`

整理时间：2026-07-14

范围说明：本文整理 `/home/uenv` 当前工作树相对 `HEAD` 的代码变动。当前工作树包含 tracked 修改、删除文件，以及 untracked 新增文件；其中 `trajectory-data/` 是运行数据目录，不按代码变动展开。

## 总览

当前变动是一组围绕 VeRL async 协议清理、typed rollout 字段、server 调度/取消/结果处理、SWE Agent 编排和 server 代码结构拆分的综合改动。

`git diff --stat` 显示 tracked 文件当前约 `47 files changed, 3563 insertions(+), 7178 deletions(-)`。这个统计不包含 untracked 新增目录和文件，例如 `proto-clean/`、`uenv-server/src/service/`、`uenv-server/src/agent_pool/`、`uenv-server/src/trajectory/`、OpenHands 新生成的 `common_pb2.py/episode_pb2.py`。

主要变化可以分为八类：

1. 协议层从 JSON/metadata/info 兜底迁到 typed proto 字段。
2. 新建 `/home/uenv/proto-clean`，并已用其中的 clean proto 覆盖正式 `/home/uenv/proto`。
3. Bridge/adapter-core 停止构造和读取旧 `SampleEnvelope.payload_json/meta_json/model_output_json`。
4. Adapter-core/worker 改为使用 typed `ModelEndpoint` / `EpisodeRequest.model_endpoint_config`。
5. Worker、server、OpenHands Agent 链路接入 typed rollout 结果和 `RolloutTrace`。
6. Worker 注册时上报 `load/max_load`，server 重启后可恢复真实负载视图。
7. Server 拆分大文件，并重构 admission、episode context、result finalizer、agent pool 和 trajectory 存储。
8. 修复调度/容量一致性问题，并补充回归测试。

## 协议层变动

涉及文件：

- `proto/uenv/v1/adapter_core.proto`
- `proto/uenv/v1/episode.proto`
- `proto/uenv/v1/agent.proto`
- `proto/uenv/v1/scheduler.proto`
- `proto/uenv/v1/server.proto`
- `proto-clean/proto/uenv/v1/*.proto`
- `proto-clean/CLEAN_PROTO_FIELDS.md`
- `proto-clean/REMAINING_CLEAN_MIGRATION.md`

当前正式 proto 已是 clean 版：旧字段通过 `reserved` 保留字段号，新 typed 字段是唯一协议入口。当前运行主链路已经继续收口，不再发送或读取已清理的旧入口。

主要协议变化：

- `SampleEnvelope` 新增 typed 字段：`parallel_mode`、`env_config_json`、`episode_config_json`、`reward_config_json`、`model_endpoint`、`timeout_seconds`、`correlation_id`、`sample_context_json`、`env_package_id`、`env_package_version`。
- `SampleEnvelope.payload_json/meta_json/model_output_json` 仍留在正式兼容 proto 中，但 bridge/adapter-core 主链路已停止构造和读取。
- 新增 `ModelEndpoint` message；adapter-core 把它映射到 `EpisodeRequest.model_endpoint_config`。
- `EpisodeRequest.model_endpoint_config = 22` 承载 URL、model_name、generation_config_json、max_retries；旧 `EpisodeRequest.model_endpoint = 8` 仍仅为兼容字段。
- `SampleResult` 增加 `rollout_param_version`、`rollout_policy_version`、`rollout_log_probs`。
- `EpisodeResult` 保留 typed `parallel_mode`、rollout 版本、logprobs、timing latency 字段。
- 新增 `RolloutTrace`，`StepRecord.rollout_trace` 承载 `response_ids/response_mask`，不再通过 `StepRecord.info` 字符串兜底。
- `RegisterWorkerRequest` 增加 `load/max_load`。
- `CancelEpisodeResponse` 增加 `server_cancelled`、`worker_cancel_attempted`、`worker_cancel_accepted`、`worker_cancel_code`、`worker_cancel_message`；server 已停止显式构造旧 `cancelled` 汇总字段。
- `/home/uenv/proto-clean/proto` 已覆盖正式 `/home/uenv/proto`；Python bridge 和 OpenHands runtime protobuf 已重新生成。

## Bridge / Adapter-Core 变动

涉及文件：

- `uenv-bridge/core/src/core.rs`
- `uenv-bridge/core/src/main.rs`
- `uenv-bridge/core/src/protocol.rs`
- `uenv-bridge/src/uenv/bridge/clients.py`
- `uenv-bridge/src/uenv/bridge/protocol.py`
- `uenv-bridge/src/uenv/bridge/verl_agent_loop.py`
- `uenv-bridge/tests/test_verl_agent_loop.py`
- `uenv-bridge/src/uenv/bridge/gen/...`

主要内容：

- Python bridge 组 `SampleEnvelope` 时写 typed 字段，不再构造旧 `payload_json/meta_json/model_output_json`。
- Rust adapter-core 内部 `SampleEnvelope` 已删除旧 JSON 字段，并忽略兼容 proto 里的旧字段。
- Adapter-core 从 typed `SampleEnvelope` 构造 `EpisodeRequest`，不再从旧 JSON 字段 fallback。
- Adapter-core 会重新组装 `EpisodeRequest.payload`，只保留 worker 业务输入，例如 `question/dataset/metadata` 和 SWE 的 `instance_id/benchmark_variant/agent_bridge_id/agent_bridge_version/agent_pool_id/driver_entrypoint/workspace_dir/llm_config_path/max_iterations`。
- Adapter-core 不再把模型调用协议字段重复放进 worker payload：`model_endpoint`、`model_name`、`generation_config`、`correlation_id`、`env_package_id`、`env_package_version`。
- Adapter-core 把 `SampleEnvelope.model_endpoint` 映射到 `EpisodeRequest.model_endpoint_config`。
- Python bridge 读取 adapter-core 返回的 `rollout_param_version/rollout_policy_version/rollout_log_probs`。
- Python bridge 已删除从 `StepRecord.info["response_ids"]` / `info["response_mask"]` 读取 token trace 的旧 fallback。
- `verl_agent_loop.py` 仍可读取 VeRL 自身输出字段名 `response_logprobs`，这不是 UEnv 协议旧入口。

## Worker 侧变动

涉及文件：

- `uenv-worker/src/control_plane/client.rs`
- `uenv-worker/src/episode/async_context.rs`
- `uenv-worker/src/episode/executor.rs`
- `uenv-worker/src/episode/model_client.rs`
- `uenv-worker/src/episode/rollout_meta.rs`
- `uenv-worker/src/llm.rs`
- `uenv-worker/src/swe/session.rs`
- `uenv-worker/src/swe/trajectory.rs`
- `uenv-worker/tests/m5_episode_executor.rs`
- `uenv-worker/tests/trajectory_upload_e2e.rs`

主要内容：

- worker 注册时上报当前 `active_episode_count()` 为 `load`，并上报 `max_load`。
- worker 把 `EpisodeRequest.parallel_mode` 作为 canonical typed field，不再从 `EpisodeRequest.metadata["parallel_mode"]` 或 `payload.metadata.parallel_mode` fallback。
- worker model client 改为读取 typed `EpisodeRequest.model_endpoint_config`；`payload.model_endpoint/model_name/generation_config` 不再作为模型调用参数来源。
- 删除旧的 `parse_payload_model_endpoint()` helper。
- worker 不再把 `payload.response_text` 当作第一步预生成模型输出。
- `rollout_meta.rs` 停止从 payload、metadata、metadata.extra_info 读取 `rollout_param_version/rollout_policy_version/rollout_log_probs/response_ids/response_mask` 等旧键。
- worker 把 `response_ids/response_mask` 写入 `StepRecord.rollout_trace`。
- worker 把 async 训练结果写入 `EpisodeResult` typed 字段，并过滤 `EpisodeResult.metadata` 中的协议 key。
- SWE trajectory bundle schema 新增 `steps[*].rollout_trace.response_ids/response_mask` 容器；gateway command/read/write 步骤默认 `rollout_trace: None`，不伪造 token ids。
- 普通模型路径可以从模型响应 body/header 解析 rollout 字段；SWE async 如果没有真实 typed rollout 来源，会按缺少 async 字段失败，不再接受旧 payload 键伪造成成功结果。

## Server 侧功能变动

涉及 tracked 文件：

- `uenv-server/src/admin_http.rs`
- `uenv-server/src/agent_job.rs`
- `uenv-server/src/config.rs`
- `uenv-server/src/control_plane.rs`
- `uenv-server/src/lib.rs`
- `uenv-server/src/proto.rs`
- `uenv-server/src/scheduler/mod.rs`
- `uenv-server/src/scheduler/traits.rs`
- `uenv-server/src/state.rs`
- `uenv-server/tests/swe_agent_orchestration.rs`

涉及新增 untracked 模块：

- `uenv-server/src/admin_query.rs`
- `uenv-server/src/admission.rs`
- `uenv-server/src/episode_context.rs`
- `uenv-server/src/execution_backend.rs`
- `uenv-server/src/ports.rs`
- `uenv-server/src/result_finalizer.rs`
- `uenv-server/src/agent_pool/`
- `uenv-server/src/service/`
- `uenv-server/src/trajectory/`

主要内容：

- 新增 `AdmissionController`，封装 episode semaphore/dynamic queue 逻辑。
- 新增 `EpisodeContext`，在 dispatch 时保存完整 `EpisodeRequest` 和稳定上下文，worker `ReportResult` 回来时不再临时重建缩水版 request。
- 新增 `result_finalizer`，统一处理结果补齐、async protocol 校验、trajectory 持久化、广播和 terminal result 构造。
- 新增 `execution_backend`，把 native worker 路径和 SWE Agent 路径选择从 service 主入口中抽出。
- 新增 `ports`，封装 worker gRPC dispatch/cancel 和 runtime gateway session HTTP 调用。
- 新增 `admin_query`，把 admin HTTP/gRPC 状态查询的数据聚合从 handler 中拆出。
- `control_plane` 的 `ReportResult` 校验 server epoch、idempotency key、dispatch lease、dispatch token，并使用 `pending.ctx.request` 做 finalization。
- `RegisterWorkerRequest.load/max_load` 进入 scheduler；server 重启后 worker 重新注册即可恢复真实负载视图。
- heartbeat 继续用 `load/max_load` 更新 worker 负载和容量。
- dynamic admission 按 scheduler 实际接受的注册结果调整容量，避免 active lease 重注册时错误增加 permit。
- `EpisodeRequest.parallel_mode` 是 server 边界 canonical typed field；server 不再从 metadata 或 payload metadata fallback。
- Agent 完成上报新增 typed `AgentJobCompleteRequest.rollout_trace`，server 映射到 `EpisodeResult.trajectory.steps[0].rollout_trace`。
- `CancelEpisodeResponse` 返回拆分后的 cancel 字段；旧 `cancelled` 只因兼容 proto 默认值存在。
- `config` 改为配置文件存在但非法时 fail fast；文件缺失才 fallback 默认值。

## OpenHands Agent 链路变动

涉及文件：

- `scripts/openhands/openhands_runner.py`
- `integrations/openhands/uenv_runtime/agent_client.py`
- `integrations/openhands/uenv_runtime/gen/uenv/v1/agent_pb2.py`
- `integrations/openhands/uenv_runtime/gen/uenv/v1/common_pb2.py`
- `integrations/openhands/uenv_runtime/gen/uenv/v1/episode_pb2.py`

主要内容：

- `agent_client.complete_agent_job()` 支持 typed async rollout 字段：`parallel_mode`、`rollout_param_version`、`rollout_policy_version`、`rollout_log_probs`、worker/model timing 字段。
- `agent_client.complete_agent_job()` 支持发送 `AgentJobCompleteRequest.rollout_trace`。
- `openhands_runner.py` 从 `submit_result.json`、`trajectory.steps[*].rollout_trace` 或 `trajectory_bundle.json` 的 typed `rollout_trace` 读取 `response_ids/response_mask`。
- runner 不再从旧 `info.response_ids/response_mask` 字符串 fallback。
- OpenHands runtime Python protobuf 已重新生成 `agent_pb2.py`，并新增 `common_pb2.py`、`episode_pb2.py`，以支持 `RolloutTrace` 类型引用。
- 当前 runner/client 只透传已有 typed 字段；真正 token ids、logprobs 和 rollout 版本仍需要真实 LLM/token producer 产出。

## Server 代码结构拆分

原来的三个大文件被拆为目录模块：

- `uenv-server/src/agent_pool.rs` 删除，拆到 `uenv-server/src/agent_pool/`
- `uenv-server/src/service.rs` 删除，拆到 `uenv-server/src/service/`
- `uenv-server/src/trajectory.rs` 删除，拆到 `uenv-server/src/trajectory/`

主要拆分如下：

- `agent_pool/mod.rs`：模块入口。
- `agent_pool/types.rs`：AgentInfo、bridge info、路由配置、错误类型等。
- `agent_pool/registry.rs`：AgentRegistry、pool admission、选池和容量逻辑。
- `agent_pool/tests.rs`：Agent pool 路由和容量测试。
- `service/mod.rs`：service 模块入口。
- `service/prelude_and_guards.rs`：imports、guard、cancel helpers、基础结构。
- `service/episode.rs`：submit_episode、native worker、SWE agent、batch、async result 主流程。
- `service/support.rs`：SWE spec 解析、gateway helper、metadata 辅助函数。
- `service/admin.rs`：AdminService gRPC 实现。
- `service/rpc.rs`：EpisodeService gRPC wrapper。
- `service/tests.rs`：service 层测试。
- `trajectory/mod.rs`：trajectory 模块入口。
- `trajectory/config.rs`：环境变量配置。
- `trajectory/store.rs`：SQLite/body 文件存储。
- `trajectory/http.rs`：Axum HTTP router 和 handlers。
- `trajectory/prelude.rs`：共享类型、metrics、工具函数。
- `trajectory/tests.rs`：trajectory 存储测试。

同时，`uenv-server/src` Rust 文件开头补充了中文文件职责注释：文件职责、主要功能、大致工作流。

## 已修复的问题

### 1. Worker 重注册响应语义错误

文件：`uenv-server/src/control_plane.rs`

问题：scheduler 在同 `worker_id` 仍有 active lease 时会拒绝替换旧 worker，但 `RegisterWorkerResponse.accepted` 之前写死为 `true`。

修复：响应中的 `accepted` 改为使用 `registration.accepted`；拒绝时 message 说明已有 active lease，旧 worker 已标记 draining。

测试：增强 `active_lease_reregister_does_not_increase_admission_permits`，断言重注册响应 `accepted=false`。

### 2. Agent 换池重新注册时旧 pool admission 容量可能泄漏

文件：`uenv-server/src/agent_pool/registry.rs`

问题：同一 agent 从 `pool-a` 移到 `pool-b` 时，旧 pool admission permit 可能不会收缩。

修复：注册前记录旧 pool；如果旧 pool 和新 pool 不同，同时同步旧 pool 和新 pool 的 admission 容量。

测试：新增 `pool_semaphore_shrinks_when_agent_moves_pool`。

### 3. Server 重启后 worker 负载视图丢失

文件：

- `proto/uenv/v1/scheduler.proto`
- `uenv-worker/src/control_plane/client.rs`
- `uenv-server/src/control_plane.rs`

问题：server 重启后 worker 重新注册时，如果只靠后续 heartbeat 更新负载，注册瞬间的调度视图可能低估真实 load。

修复：`RegisterWorkerRequest` 增加并使用 `load/max_load`；worker 注册时发送当前 active episode 数和容量，server 注册时初始化 scheduler 负载视图。

测试：`register_worker_initializes_reported_load`。

## 当前验证状态

已在远端 `/home/uenv` 验证过的项目：

- clean proto dry-run：在临时副本中用 `/home/uenv/proto-clean/proto` 覆盖 `proto/` 后，`cargo check -p uenv-server`、`cargo check -p uenv-worker`、`cargo check -p uenv-adapter-core` 均通过。
- clean proto 已正式覆盖 `/home/uenv/proto`，Python bridge adapter-core protobuf 和 OpenHands runtime `common/episode/agent` protobuf 已重新生成。
- `cargo test -p uenv-adapter-core`：通过。
- `cargo test -p uenv-server`：通过。
- `cargo test -p uenv-worker`：通过。
- `PYTHONPATH=/home/uenv/uenv-bridge/src python3 -m unittest uenv-bridge/tests/test_verl_agent_loop.py`：21 tests OK。
- OpenHands typed rollout trace/client 构造临时验证脚本：通过。
- `git diff --check` 针对本轮改动过的 proto/文档/关键代码路径：通过。

环境限制：

- 没有重启服务。
- `cargo fmt --check` / `cargo clippy` 仍受远端 toolchain 是否安装 `rustfmt`、`clippy` 影响；此前远端 stable toolchain 缺少这些组件。

## 当前工作树注意事项

- 当前工作树仍有大量 uncommitted 修改和 untracked 文件。
- `proto-clean/` 是新建的 clean proto 文档和替换来源，目前显示为 untracked。
- `code-change-summary-since-last-commit.md` 当前也是 untracked 文档。
- `trajectory-data/` 是运行数据目录，建议不要混入代码提交，除非明确要提交测试数据。
- `uenv-server/src/agent_pool.rs`、`uenv-server/src/service.rs`、`uenv-server/src/trajectory.rs` 已被目录模块取代；提交时需要确认删除旧文件并 add 新目录。
- `uenv-server/src/trajectory.rs.bak_feat` 当前显示为删除。若它只是历史备份，删除可以接受；若还有保留价值，需要单独确认。
- 生成的 protobuf Python 文件已经变动，应和 proto 变动一起提交，避免 Python bridge、OpenHands runtime 与 proto schema 不一致。

## 建议提交拆分

如果要做成易 review 的 commits，建议拆成：

1. Protocol typed migration：正式 proto、clean proto、字段说明文档。
2. Bridge/adapter-core typed envelope：删除旧 JSON 字段运行入口、typed ModelEndpoint、typed rollout result。
3. Worker typed rollout/model endpoint：parallel_mode、model_endpoint_config、rollout_meta、SWE trajectory bundle。
4. Server lifecycle refactor：admission、episode_context、result_finalizer、ports、execution_backend。
5. Agent/OpenHands path：AgentJobCompleteRequest.rollout_trace、runner/client 透传、OpenHands pb 生成文件。
6. Server module split and comments：agent_pool/service/trajectory 拆分、文件职责注释。
7. Scheduler/capacity fixes：worker load/max_load、active lease 重注册、agent pool move capacity shrink。
8. Hygiene：确认 `trajectory-data/`、`__pycache__/`、`.bak_feat` 是否纳入提交或清理。
