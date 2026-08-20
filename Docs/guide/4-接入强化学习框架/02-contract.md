# 强化学习接入契约

本文定义强化学习框架接入 UEnv 时必须保持的运行边界。框架接入代码可以使用不同语言，但不能改变 ID、批次、token、结果或失败语义。

## 术语与运行边界

| 术语 | 本文含义 |
|---|---|
| sample | 强化学习框架选中的一条训练输入 |
| Episode | UEnv 中一次独立环境执行 |
| rollout | 模型在 Episode 中产生 response/action 的过程 |
| response token/mask | 用于框架计算 loss 的 token 与有效位置 |
| staleness | 异步训练中 rollout 相对当前策略版本的陈旧程度 |

接入代码在框架侧运行，把训练任务数据直接转换为 UEnv Server 接入的标准数据包，通过 `AdapterCoreService` gRPC API 提交。协议服务名是兼容标识；接入代码只与 UEnv Server 交互，UEnv Worker 的注册与选择全部由 UEnv Server 完成。

UEnv Server 把 `SampleEnvelope` 转成 `EpisodeRequest`，调度 UEnv Worker，再把 `EpisodeResult` 转成 `SampleResult`。接入点应位于“训练 sample 已确定、环境 rollout 尚未执行”的位置：

```text
sample -> framework preprocessing -> 接入代码 -> UEnv Server/Worker rollout
       -> 接入代码 restores tokens/reward -> loss/update/checkpoint
```

如果框架已经在本地完成生成，再把纯文本交给环境判分，则属于后置 reward 模式：UEnv 只完成判分，token、mask 和 trajectory 的来源必须另行说明。

## 协议入口与最小连通检查

类型化边界位于 `proto/uenv/v1/adapter_core.proto`：

- `HealthCheck`：连接与协议版本检查。
- `ExecuteBatch`：一次请求/响应的有界批次。
- `ExecuteBatchStream`：样本和结果双向流式传输。

Python 开发可以直接使用仓库固定生成的 stub。安装项目依赖后，从仓库根目录执行：

```bash
export PYTHONPATH="$PWD/uenv-bridge/src"
export UENV_SERVER_ENDPOINT='127.0.0.1:50051'

python3 - <<'PY'
import os
import grpc
from uenv.bridge.gen import adapter_core_pb2 as pb
from uenv.bridge.gen import adapter_core_pb2_grpc as rpc

with grpc.insecure_channel(os.environ["UENV_SERVER_ENDPOINT"]) as channel:
    reply = rpc.AdapterCoreServiceStub(channel).HealthCheck(
        pb.HealthCheckRequest(), timeout=5
    )
    assert reply.ok, reply
    print({"ok": reply.ok, "version": reply.version})
PY
```

明文 gRPC 只用于受控内网；跨不可信网络时通过组织批准的 TLS 入口或隧道连接。健康检查通过只说明 RPC 可达；训练接入完成以四层验收为准。

## 请求契约

每条 `SampleEnvelope` 的关键字段：

| 字段 | 约束 |
|---|---|
| `request_id` | 每个 sample/rollout 唯一；重试同一逻辑请求时保持不变 |
| `batch_id` | 非空；同一框架批次一致 |
| `sample_index` | sample 在框架批次中的稳定索引 |
| `framework` | 非空框架标识，例如 `verl` |
| `env_type` | 显式环境类型，不从文件名或默认值猜测 |
| `parallel_mode` | 与框架实际执行模式一致 |
| `env_config_json` | 环境初始化与任务数据；UTF-8 JSON object |
| `episode_config_json` | max steps/turns、seed、初始 observation 等 |
| `reward_config_json` | target、rubric 或 plugin 配置 |
| `model_endpoint` | UEnv Worker 可达的 URL、模型名、生成参数和重试上限 |
| `timeout_seconds` | Episode 业务超时，与 gRPC deadline 分开设置 |
| `correlation_id` | 跨框架日志、UEnv Server 日志与轨迹使用的关联键 |
| `sample_context_json` | 可追溯 metadata，不得放 API Key |
| `env_package_id/version` | 需要固定环境包时显式填写 |

`dataset` 放入 `env_config_json`，`max_steps` 放入 `episode_config_json`；不要让同一含义在多个 JSON 中冲突。一个批次内 `request_id` 不得重复，`batch_id` 与 `framework` 不得为空，所有 JSON bytes 解码后必须是对象。

## 模型、token 与策略版本

接入代码必须明确当前策略模型在哪里：

- UEnv Worker 直接访问模型服务时，request 携带 UEnv Worker 可达 URL。
- 模型位于 GPU 主机时，框架 runner 提供对 UEnv Worker 可达的 model gateway。
- 密钥通过服务配置或受保护文件传入，不进入 sample payload、轨迹或普通日志。

训练输出优先使用 UEnv Worker 返回的原始 `response_ids` 与 `response_mask`。仅有 `response_text` 时，用框架 tokenizer 重新编码只能作为显式兼容模式，并记录 tokenizer、chat template 与 token 来源；否则可能改变训练语义。

异步训练还要携带并检查 `rollout_param_version`、`rollout_policy_version` 和 token 级 logprob。框架必须定义最大 staleness、过期结果处理及权重切换时对 in-flight rollout 的策略。

## 结果契约

接入代码至少处理以下 `SampleResult` 字段：

| 字段 | 接入代码行为 |
|---|---|
| `request_id` | 映射输入；未知或重复 ID 是协议错误 |
| `batch_id`、`sample_index` | 校验批次归属，恢复框架原顺序 |
| `status`、`done` | 决定结果能否进入训练更新 |
| `reward` | 回填 reward，不用默认 0 掩盖错误 |
| `termination_reason` | 写入框架 extra fields 与日志 |
| `trajectory_json` | 还原 step、token/mask、轨迹引用和调试信息 |
| `error_code/message` | 进入失败策略与可观测性 |
| rollout version/logprobs | 验证策略新鲜度和 token 对齐 |

UEnv Server 可以乱序返回，接入代码必须按 `request_id` 重排，不能用响应数组位置或 `zip(samples, results)` 回填。

状态语义：

| 状态 | 默认训练行为 |
|---|---|
| `completed` | 仍需校验 token/mask/reward 后才能用于更新 |
| `failed` / `timeout` | 不进入更新；按失败策略终止或显式屏蔽 |
| `recorded` | 旧接入实现的兼容状态；只有完整 trace 已校验时才按 completed 处理，并记录兼容来源 |
| 未知值 | 协议错误，禁止猜测 |

新接入不能为了兼容未知状态而把 `done=true` 作为唯一成功依据。

## 批次、流和背压

- 有界批次使用 `ExecuteBatch`；高吞吐或逐条返回使用 `ExecuteBatchStream`。
- 接入代码限制本地 in-flight 数；收到 `RESOURCE_EXHAUSTED` 时降低并发并有界退避。
- 框架 batch、UEnv Server pending batch、UEnv Worker slot、Agent job 和 Runtime Gateway session 是不同层级的上限，最终并发取最小值。
- 调度 hint 不能绕过 UEnv Worker 上报的硬容量。

流式调用必须在取消或框架退出时关闭发送端与 channel，不能留下继续提交的后台任务。

## 超时、取消、重试与幂等

| 场景 | 必须行为 |
|---|---|
| Episode 超时 | 返回终态超时/失败，记录终止原因 |
| gRPC deadline | 调用方停止等待；只在请求可幂等重放时重试 |
| 框架取消 | 停止产生新请求并传播取消 |
| 容量不足 | 有界退避或拆小批次，保持原 `request_id` |
| 业务低分 | 不做传输级重试 |
| 未知结果 ID | 立即报协议错误 |

同一 `request_id` 的重复提交必须表示同一个逻辑 Episode。需要新的 rollout 时生成新 ID，并用 `batch_id` / `correlation_id` 建立关联。

## 失败策略

默认 fail fast：非可训练终态不进入参数更新。只有任务定义明确允许时，才可把失败转成 `zero_reward`；此时同时保留零长度或全 0 mask 的占位、原始状态与错误字段，以及明确的 `failed_episode_policy` 标记。

模型不可达、UEnv Worker 崩溃、容器失败和协议错配不应成为模型能力低下的训练信号。

## 可观测性与数据安全

框架、接入代码、UEnv Server 和 UEnv Worker 都应记录 `run_id`、`batch_id`、`request_id`、`episode_id` 与 `trajectory_id`。不得记录 API Key；prompt、response、源码和轨迹使用同一数据分级与保留策略。

## 四层验收

| 层级 | 验证内容 | 通过标准 |
|---|---|---|
| 映射单测 | sample→envelope、result→framework output | 必填字段和 token/reward 映射一致 |
| 协议测试 | ID、乱序/重复/缺失、timeout、backpressure | 错误稳定识别，没有按位置误配 |
| 本地闭环 | 接入代码 → UEnv Server → UEnv Worker | 真实 Episode 有终态、reward 与 trajectory |
| 框架端到端 | 真实强化学习作业 | 完成预定更新并写出可追溯 checkpoint |

验收使用目标训练流程本身，不另加一套与真实接入语义不同的简化任务。实现步骤见[自定义强化学习框架接入](./03-custom-framework.md)。
