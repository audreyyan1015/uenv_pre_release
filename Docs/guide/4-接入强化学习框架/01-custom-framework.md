# 自定义强化学习框架接入

UEnv 向强化学习框架提供 Episode 级 rollout 接口。框架把一批训练样本和当前策略模型的服务地址交给 UEnv；UEnv 调用模型、与环境交互并计算奖励，然后返回每条样本的执行状态、reward 和 trajectory。数据加载、rollout 结果整理、loss 计算、参数更新和 checkpoint 仍由强化学习框架负责。

接入代码只需要完成两次转换：在 rollout 开始前把框架 sample 转成 UEnv 请求，收到结果后再把 UEnv 结果转成框架所需的 rollout 输出。接入新框架时不需要修改 UEnv Server、UEnv Worker 或环境实现。

## 接入位置

在强化学习框架中，把 UEnv 调用放在“本批 sample 已经确定，本地 rollout 尚未开始”的位置。接入后的主流程是：

`framework sample → UEnv rollout → framework rollout output → loss / update`

框架要保留原 sample 和 prompt token，并为每次 rollout 分配唯一的 `request_id`。这个 ID 用于将 UEnv 结果放回正确的批次位置。

## UEnv 提供的接口

框架通过 gRPC 调用 `uenv.bridge.v1.AdapterCoreService.ExecuteBatch`。一次调用提交一批 `SampleEnvelope`，并收到同批 `SampleResult`。公开协议的真源是 `proto/uenv/v1/adapter_core.proto`。

构造一条请求时，接入代码需要提供以下信息：

- 运行哪个环境：`env_type` 以及该环境定义的 `env_config_json`。
- Episode 如何运行：`episode_config_json`，通常包含 `max_steps` 和可选的 `seed`。
- 如何计分：目标环境需要的 `reward_config_json`。
- Worker 调用哪个当前策略模型：`model_endpoint`，包含 OpenAI-compatible URL、模型名和生成参数。
- 如何关联批次和样本：`request_id`、`batch_id` 和 `sample_index`。

`env_config_json` 的字段由具体环境决定。例如 QA 环境需要问题和数据集名，代码环境还会需要测试代码或任务 ID。通用框架适配器不应猜测这些环境字段。

`model_endpoint.url` 必须从 UEnv Worker 所在的网络访问。Worker 和训练主机不是同一台机器时，不能填写只对训练主机自身有效的 `127.0.0.1`。

UEnv 返回的 `SampleResult` 中，接入代码通常只需关心：

- `request_id`：找回原 sample 及其 prompt token。
- `status`、`error_code` 和 `error_message`：判断 Episode 是否成功。
- `reward`：交给框架计算 advantage 和 loss。
- `trajectory_json`：包含环境每一步的 action 和 reward；当 rollout 结果携带 token trace 时，它位于 `steps[*].rollout_trace.response_ids` 和 `response_mask`，多步结果按 step 顺序拼接。

## 开发者需要实现什么

框架适配层需要实现两个转换函数，并在两者之间调用 `ExecuteBatch`：

```python
def to_uenv_request(framework_sample, rollout_context):
    """将框架 sample 编码为 SampleEnvelope。"""
    ...


def to_framework_output(uenv_result, original_sample):
    """将 action、token trace 和 reward 整理为框架的 rollout 输出。"""
    ...
```

适配层还需要保存 `request_id -> 原 sample / prompt_ids / 批次位置` 的对应关系。UEnv 不承诺结果数组与请求数组的顺序相同，因此不能用 `zip(requests, results)` 关联结果。

如果当前策略模型只存在于框架进程内，还需要在框架侧暴露一个 Worker 可访问的 OpenAI-compatible 模型服务。这个服务如何跟随参数更新，由框架适配层处理。

## 最小可运行示例

下面的代码向已运行的 UEnv 服务提交一条 `qa/gsm8k` sample。使用发布包时，先安装其中的 Python Bridge wheel：

```bash
python -m pip install /opt/uenv/current/wheels/uenv_bridge-*.whl
```

从源码开发时，在仓库根目录改用：

```bash
python -m pip install ./uenv-bridge
```

将 `MODEL_URL` 改为 UEnv Worker 可访问的当前策略模型地址。如果框架和 UEnv Server 不在同一台主机，同时将 `UENV_ENDPOINT` 改为框架侧可访问的 Server 地址，再运行：

```python
import json

import grpc

from uenv.bridge.gen import adapter_core_pb2 as pb
from uenv.bridge.gen import adapter_core_pb2_grpc as rpc


UENV_ENDPOINT = "127.0.0.1:50051"
MODEL_URL = "http://10.0.0.20:8000/v1"
MODEL_NAME = "my-policy"


def json_bytes(value):
    return json.dumps(value, ensure_ascii=False).encode("utf-8")


sample = pb.SampleEnvelope(
    request_id="rollout-1",
    batch_id="batch-1",
    sample_index=0,
    framework="my-framework",
    env_type="qa",
    env_config_json=json_bytes({
        "dataset": "gsm8k",
        "question": "There are 9 notebooks and 3 are sold. How many remain? End with `#### number`.",
    }),
    episode_config_json=json_bytes({"max_steps": 1, "seed": 42}),
    reward_config_json=json_bytes({"type": "rule_reward", "target": "6"}),
    model_endpoint=pb.ModelEndpoint(
        url=MODEL_URL,
        model_name=MODEL_NAME,
    ),
    timeout_seconds=300,
)

channel = grpc.insecure_channel(UENV_ENDPOINT)
try:
    client = rpc.AdapterCoreServiceStub(channel)
    reply = client.ExecuteBatch(
        pb.ExecuteBatchRequest(
            request_id="call-1",
            batch_id="batch-1",
            samples=[sample],
        ),
        timeout=330,
    )
finally:
    channel.close()

result = next(item for item in reply.results if item.request_id == sample.request_id)
if result.status != "completed":
    raise RuntimeError(
        f"UEnv failed: {result.error_code}: {result.error_message}"
    )

trajectory = json.loads(result.trajectory_json.decode("utf-8"))
print({"reward": result.reward, "trajectory": trajectory})
```

这个请求返回的 trajectory 已包含模型 action 和每步 reward。`to_framework_output()` 应优先按 step 顺序拼接 `rollout_trace` 中的 `response_ids` 和 `response_mask`；如果当前同步运行只返回 action 文本，则使用框架当前模型的同一 tokenizer 编码该文本，并为生成 token 构造 mask。prompt token 始终从原 sample 保留，不需要从 UEnv 结果恢复。

真正接入框架批次时，把单条 sample 构造改成 `to_uenv_request()`，并始终按 ID 取回结果：

```python
prepared = [to_uenv_request(sample, context) for sample in framework_batch]
original_by_id = {item.request_id: sample for item, sample in zip(prepared, framework_batch)}

reply = client.ExecuteBatch(
    pb.ExecuteBatchRequest(
        request_id=batch_call_id,
        batch_id=batch_id,
        samples=prepared,
    ),
    timeout=grpc_timeout,
)
result_by_id = {item.request_id: item for item in reply.results}

framework_outputs = [
    to_framework_output(result_by_id[item.request_id], original_by_id[item.request_id])
    for item in prepared
]
```

## 完成接入前检查

将新框架用于真实训练前，确认：

- 每次 rollout 的 `request_id` 唯一，传输重试时复用原 ID。
- 结果按 `request_id` 关联，不依赖返回顺序。
- `model_endpoint` 可从 UEnv Worker 实际访问。
- 每个环境需要的 `env_config` 和 `reward_config` 已完整提供。
- 只有 `status="completed"` 的结果进入训练；失败或超时不能静默转成普通零分样本。
- 交给框架的 `response_ids` 与 `response_mask` 非空且长度相同，并记录它们来自 token trace 还是文本编码。

如果你使用 VeRL，UEnv 已经实现了上述适配层，请直接阅读[以 VeRL 为例接入 UEnv](./02-verl.md)。
