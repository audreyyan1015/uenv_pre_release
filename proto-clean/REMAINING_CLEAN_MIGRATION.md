# proto-clean 剩余迁移状态

更新时间：2026-07-15
代码基线：`bridge-alignment`，已合并到 `5b8f12f`。

本文档只记录从当前代码状态继续收口 clean proto 迁移还需要做什么。旧版文档里“正式 proto 仍是兼容版、等待 proto-clean 替换”的说法已经过期：当前 `/home/uenv/proto` 已经是 clean 字段集合，`/home/uenv/proto-clean/proto` 现在更像是干净协议模板和说明目录。

## 当前结论

- 正式 proto 已经删除旧兼容字段，旧字段号用 `reserved` 保留，避免后续误复用。
- Rust server、worker、adapter-core 走 `build.rs` 编译期生成 protobuf，当前测试已经验证 clean proto 可编译。
- Python bridge adapter-core 生成文件已经重新生成，当前生成文件不再暴露 `payload_json`、`meta_json`、`model_output_json` 字段名。
- Bridge / adapter-core / server / worker 主链路已经走 typed 字段：`parallel_mode`、`ModelEndpoint`、rollout 版本、logprobs、`RolloutTrace`、cancel 拆分字段。
- 合并 `feature/worker-pool-260622` 后新增了 CodeEnv / DSCodeBench 路径；adapter-core 会把 `env_config.response_text` 转发到 worker payload，但 worker model client 不会把它当模型输出捷径。它现在只能作为 CodeEnv/smoke 业务输入或调试字段存在，不能再代表 UEnv 协议里的模型输出。
- 当前剩余工作不再是“删 proto 字段”，而是上线前确认、外部客户端兼容、真实 rollout producer 补齐，以及少量脚本/历史文档清理。

## 已完成的迁移点

### Bridge / adapter-core

- `SampleEnvelope` 已改成 typed 字段：
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
- 旧 `SampleEnvelope.payload_json`、`meta_json`、`model_output_json` 已从正式 proto 删除，字段号保留为 reserved。
- adapter-core 已用 typed `SampleEnvelope.model_endpoint` 构造 `EpisodeRequest.model_endpoint_config`。
- adapter-core 已过滤上下文 metadata 中的协议 key，不再把 `parallel_mode` 等协议字段塞回 worker payload 的 metadata。
- adapter-core 已把 `StepRecord.rollout_trace.response_ids/response_mask` 序列化进 trajectory JSON。
- Python bridge 已读取 typed rollout trace，不再依赖 `StepRecord.info["response_ids"]` / `StepRecord.info["response_mask"]`。
- Python bridge 单测已覆盖当前路径：`python3 -m unittest uenv-bridge.tests.test_verl_agent_loop`，23 个测试通过。
- adapter-core 单测已覆盖 typed parallel mode、typed model endpoint、env package、rollout trace、metadata 过滤等路径：`cargo test -p uenv-adapter-core`，11 个测试通过。

合并 CodeEnv 后需要特别说明：

- adapter-core 当前会转发 CodeEnv 执行字段：
  - `task_id`
  - `library`
  - `test_code`
  - `test_script_path`
  - `ground_truth_path`
  - `ground_truth_code`
  - `entry_point`
  - `num_tests`
  - `random_seed`
  - `timeout_secs`
  - `benchmark_root`
  - `response_text`
- 其中 `response_text` 不是 clean proto 字段，也不是模型输出协议字段；它只是 worker payload 里的环境/测试输入。真实模型调用仍必须走 typed `ModelEndpoint`。

### Server

- `EpisodeRequest.parallel_mode` 已成为唯一 canonical 来源，server 不再从 request metadata 或 payload metadata fallback。
- server 仍保留 `EpisodeRequest.metadata` / `EpisodeResult.metadata`，但它们只用于上下文信息，不承载协议字段。
- `EpisodeResult` 已保留 typed async 结果字段：
  - `parallel_mode`
  - `rollout_param_version`
  - `rollout_policy_version`
  - `rollout_log_probs`
  - timing latency 字段
- `RolloutTrace` 已接入 agent 完成上报，server 会把 `AgentJobCompleteRequest.rollout_trace` 映射到 `EpisodeResult.trajectory.steps[0].rollout_trace`。
- `RegisterWorkerRequest.load/max_load` 已进入注册逻辑，server 重启后 worker 重新注册时可以恢复真实负载视图。
- `CancelEpisodeResponse` 已走拆分字段：
  - `server_cancelled`
  - `worker_cancel_attempted`
  - `worker_cancel_accepted`
  - `worker_cancel_code`
  - `worker_cancel_message`
- server 测试已通过：`cargo test -p uenv-server`，52 个单元测试和 7 个集成测试通过。

### Worker

- worker 已从 typed `EpisodeRequest.model_endpoint_config` 读取模型端点、模型名、生成配置和重试次数。
- worker model client 已忽略 payload 中的模型配置旧入口：`model_endpoint`、`model_name`、`generation_config`。
- worker model client 已忽略 `payload.response_text` 模型输出捷径。
- 新增 CodeEnv 集成测试已经改成 typed `ModelEndpoint` + 本地 mock LLM，不再靠 payload `response_text` 直接伪造模型输出。
- worker 已把 async rollout 结果写入 typed 字段：
  - `rollout_param_version`
  - `rollout_policy_version`
  - `rollout_log_probs`
  - `StepRecord.rollout_trace.response_ids/response_mask`
- SWE async 旧 payload/metadata rollout 入口已经删除；没有真实 typed rollout 来源时，不再接受旧字段伪造成功。
- worker 全量测试已通过：`cargo test -p uenv-worker`。其中 2 个 Docker/SWE 重型测试仍按原设计 ignored。

### Hub / CodeEnv 合并后的补充验证

- `feature/worker-pool-260622` 合入后引入 CodeEnv plugin、DSCodeBench fixture、hub seed 和相关测试。
- `uenv-hub` 是独立 Rust workspace，已在 `/home/uenv/uenv-hub` 跑过 `cargo test`，全量通过。
- CodeEnv worker 测试 `m5_single_round_code_dscodebench_smoke` 已通过，证明新增 CodeEnv 路径能在 clean proto 下使用 typed model endpoint。

## 当前仍然允许存在的字段/名字

这些名字出现不等于 clean proto 迁移失败：

- `EpisodeRequest.payload`：仍然保留，承载环境业务输入，例如 math 的 `question/dataset`、SWE 的 `instance_id/benchmark_variant/command_mode`、CodeEnv 的 `task_id/test_code/entry_point` 等。
- `EpisodeRequest.metadata` / `EpisodeResult.metadata`：仍然保留，只承载上下文元数据，不承载协议字段。
- worker/adapter 代码里的局部变量名 `payload_json`：只是解析 `EpisodeRequest.payload` 的局部变量，不是旧 `SampleEnvelope.payload_json`。
- VeRL 侧的 `response_logprobs`：属于 VeRL 自身 tensor/result 命名，不等同于 UEnv proto 旧字段。
- CodeEnv payload 中的 `response_text`：当前只作为环境/测试输入存在；worker model client 不会把它当模型输出 shortcut。

## 还需要做的事

### P0：上线前必须确认

1. 正式部署前重新 build/install/restart，并验证真实服务：
   - 不在本次文档更新中重启服务。
   - 上线时需要验证 `uenv-server.service` 的 PID、监听端口、日志路径和健康检查。
2. 再跑一轮部署后 smoke：
   - adapter-core ExecuteBatch。
   - native math。
   - CodeEnv / DSCodeBench。
   - SWE agent orchestration。
   - async `one_step_off_policy` / `fully_async` 字段 dump。
3. 检查真实运行日志中是否还有旧协议字段被写入：
   - `payload_json`
   - `meta_json`
   - `model_output_json`
   - `payload.metadata.parallel_mode`
   - `info.response_ids`
   - `info.response_mask`
   - `CancelEpisodeResponse.cancelled`

### P1：外部客户端和真实 rollout producer

1. 如果有仓库外 admin client，需要确认它们已经从旧 `CancelEpisodeResponse.cancelled` 迁移到拆分字段。
2. 真实 SWE/agent runner 需要继续确认是否能产出 typed rollout 数据：
   - `rollout_param_version`
   - `rollout_policy_version`
   - `rollout_log_probs`
   - `rollout_trace.response_ids`
   - `rollout_trace.response_mask`
3. OpenHands runner/client 目前能透传已有 typed trace，但不会凭空生成 token ids/logprobs。若某个真实 driver 没有产出这些字段，async 训练结果仍会缺少训练所需信息。
4. CodeEnv / DSCodeBench 如果用于正式 rollout，应确认它走真实 typed `ModelEndpoint`；脚本里的 `response_text` smoke 用法只能算测试快捷输入，不应作为生产模型输出链路。

### P2：文档和脚本清理

1. 旧联调文档里仍可能保留 `payload_json/meta_json/info.response_ids` 等历史描述；这些不影响运行，但会误导后续读代码的人。
2. 只维护当前指定的三份迁移文档：
   - `/home/uenv/proto-clean/REMAINING_CLEAN_MIGRATION.md`
   - `/home/uenv/proto-clean/CLEAN_PROTO_FIELDS.md`
   - `/home/uenv/code-change-summary-since-last-commit.md`
3. 后续如果继续改 proto，需要同步更新：
   - Rust 编译期生成：server、worker、adapter-core 的 `build.rs` 输出。
   - Python bridge 生成文件：`uenv-bridge/src/uenv/bridge/gen/*adapter_core_pb2*.py`。
   - OpenHands runtime Python 生成文件：`integrations/openhands/uenv_runtime/gen/uenv/v1/{common,episode,agent}_pb2.py`。

## 本轮合并后的验证记录

最近一次合并后已执行：

- `git diff --check`：通过。
- `cargo test -p uenv-adapter-core`：11 个测试通过。
- `cargo test -p uenv-server`：52 个单元测试 + 7 个集成测试通过。
- `cargo test -p uenv-worker`：通过；2 个 Docker/SWE 重型测试 ignored。
- `PYTHONPATH=/home/uenv/uenv-bridge/src python3 -m unittest uenv-bridge.tests.test_verl_agent_loop`：23 个测试通过。
- `cd /home/uenv/uenv-hub && cargo test`：hub 独立 workspace 全量通过。

没有执行：

- 没有重启线上服务。
- 没有重新部署 `/usr/local/bin/uenv-adapter-core`。
- 没有在本轮文档更新里跑真实长链路 SWE Docker ignored 测试。
