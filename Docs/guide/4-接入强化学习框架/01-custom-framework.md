# 自定义强化学习框架接入

UEnv 向强化学习框架提供 Episode 级 rollout 接口。框架把一批训练样本和当前策略模型的服务地址交给 UEnv；UEnv 调用模型、与环境交互并计算奖励，然后返回每条样本的执行状态、reward 和 trajectory。数据加载、rollout 结果整理、loss 计算、参数更新和 checkpoint 仍由强化学习框架负责。

与 UEnv 相关的接入代码只需要完成两次数据转换：在 rollout 开始前把框架 sample 转成 UEnv 请求，收到结果后再把 UEnv 结果转成框架所需的 rollout 输出。接入新框架时不需要修改 UEnv Server、UEnv Worker 或环境实现。

## 接入位置

在强化学习框架中，把 UEnv 调用放在“本批 sample 已经确定，本地 rollout 尚未开始”的位置。接入后的主流程是：

`framework sample → UEnv rollout → framework rollout output → loss / update`

框架要保留原 sample 和 prompt token，并为每次 rollout 分配唯一的 `request_id`。这个 ID 用于将 UEnv 结果放回正确的批次位置。

## UEnv 提供的接口

框架通过 gRPC 调用 `uenv.bridge.v1.AdapterCoreService`。下文先按同步训练说明：使用 `ExecuteBatch` 提交一批 `SampleEnvelope`，整批完成后收到 `ExecuteBatchResponse`。异步框架通常使用 `ExecuteBatchStream`，调用方式见[如果框架采用异步训练](#如果框架采用异步训练)。公开协议的真源是 `proto/uenv/v1/adapter_core.proto`。

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

框架适配层需要实现两个转换函数，并在两者之间调用同步或流式接口。`to_uenv_request()` 的输入是框架中的一条 rollout sample 和当前 rollout 配置，输出是 UEnv 的 `SampleEnvelope`；`to_framework_output()` 接收 UEnv 的 `SampleResult` 和提交前保留的原 sample，输出是框架计算 loss 所需的 token、mask 和 reward。

下面是这两个函数的完整实现。示例用 Python 字典表示框架 sample，实际接入时只需把取字段的方式替换为你的框架数据结构。

```python
import json

from uenv.bridge.gen import adapter_core_pb2 as pb


def json_bytes(value):
    return json.dumps(value, ensure_ascii=False).encode("utf-8")


def to_uenv_request(framework_sample, rollout_context):
    """将框架 sample 编码为 SampleEnvelope。"""
    return pb.SampleEnvelope(
        request_id=framework_sample["request_id"],
        batch_id=framework_sample["batch_id"],
        sample_index=int(framework_sample["sample_index"]),
        framework=rollout_context["framework"],
        env_type=framework_sample["env_type"],
        parallel_mode=rollout_context["parallel_mode"],
        env_config_json=json_bytes(framework_sample["env_config"]),
        episode_config_json=json_bytes(framework_sample["episode_config"]),
        reward_config_json=json_bytes(framework_sample["reward_config"]),
        model_endpoint=pb.ModelEndpoint(
            endpoint_type="http",
            url=rollout_context["model_url"],
            model_name=rollout_context["model_name"],
            generation_config_json=json_bytes(
                rollout_context.get("generation_config", {})
            ),
            max_retries=int(rollout_context.get("model_max_retries", 2)),
        ),
        timeout_seconds=int(rollout_context["timeout_seconds"]),
        correlation_id=framework_sample["request_id"],
        sample_context_json=json_bytes({"sample_id": framework_sample["id"]}),
    )


def to_framework_output(uenv_result, original_sample):
    """将 action、token trace 和 reward 整理为框架的 rollout 输出。"""
    if uenv_result.status != "completed":
        raise RuntimeError(
            f"UEnv failed: {uenv_result.error_code}: "
            f"{uenv_result.error_message}"
        )
    if not uenv_result.trajectory_json:
        raise RuntimeError(
            f"UEnv returned an empty trajectory: {uenv_result.request_id}"
        )

    trajectory = json.loads(uenv_result.trajectory_json.decode("utf-8"))
    steps = trajectory.get("steps")
    if not isinstance(steps, list):
        raise RuntimeError("UEnv trajectory does not contain a steps list")

    actions = []
    response_ids = []
    response_mask = []
    for step_index, step in enumerate(steps):
        if not isinstance(step, dict):
            raise RuntimeError(f"trajectory step {step_index} is not an object")
        if "action" in step:
            actions.append(step["action"])

        trace = step.get("rollout_trace") or {}
        step_ids = [int(value) for value in trace.get("response_ids", [])]
        step_mask = [int(value) for value in trace.get("response_mask", [])]
        if len(step_ids) != len(step_mask):
            raise RuntimeError(
                f"token trace length mismatch at step {step_index}: "
                f"response_ids={len(step_ids)}, response_mask={len(step_mask)}"
            )
        response_ids.extend(step_ids)
        response_mask.extend(step_mask)

    if not response_ids:
        raise RuntimeError(
            "UEnv result has no rollout token trace; the model service or "
            "Worker must return the actual response_ids and response_mask"
        )

    rollout_log_probs = [float(value) for value in uenv_result.rollout_log_probs]
    if rollout_log_probs and len(rollout_log_probs) != len(response_ids):
        raise RuntimeError(
            "rollout_log_probs length does not match response_ids: "
            f"{len(rollout_log_probs)} != {len(response_ids)}"
        )

    output = {
        "sample_id": original_sample["id"],
        "prompt_ids": list(original_sample["prompt_ids"]),
        "response_ids": response_ids,
        "response_mask": response_mask,
        "actions": actions,
        "reward": float(uenv_result.reward),
        "trajectory": trajectory,
    }
    if uenv_result.rollout_policy_version or rollout_log_probs:
        output.update({
            "rollout_param_version": int(uenv_result.rollout_param_version),
            "rollout_policy_version": uenv_result.rollout_policy_version,
            "rollout_log_probs": rollout_log_probs,
        })
    return output
```

上面的实现要求每个框架 sample 已经带有本次 rollout 的唯一 `request_id`。同一条业务样本需要生成多条 rollout 时，为每条 rollout 分配不同的 ID；传输重试时则复用原 ID。适配层还要保存 `request_id -> 原 sample / prompt_ids / 批次位置` 的对应关系。`ExecuteBatch` 会按请求顺序返回结果，`ExecuteBatchStream` 则按完成顺序返回，因此两种方式都按 `request_id` 关联最稳妥。

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

将 `MODEL_URL` 改为 UEnv Worker 可访问的当前策略模型地址。如果框架和 UEnv Server 不在同一台主机，同时将 `UENV_ENDPOINT` 改为框架侧可访问的 Server 地址。下面的 sample 就是两个转换函数所期望的输入：

```python
import grpc

from uenv.bridge.gen import adapter_core_pb2_grpc as rpc


UENV_ENDPOINT = "127.0.0.1:50051"
MODEL_URL = "http://10.0.0.20:8000/v1"
MODEL_NAME = "my-policy"

framework_batch = [{
    "id": "example-1",
    "request_id": "batch-1-sample-0-rollout-0",
    "batch_id": "batch-1",
    "sample_index": 0,
    # 示例值；真实接入时由框架 tokenizer 生成并保留。
    "prompt_ids": [101, 102],
    "env_type": "qa",
    "env_config": {
        "dataset": "gsm8k",
        "question": "There are 9 notebooks and 3 are sold. How many remain? End with `#### number`.",
    },
    "episode_config": {"max_steps": 1, "seed": 42},
    "reward_config": {"type": "rule_reward", "target": "6"},
}]

rollout_context = {
    "framework": "my-framework",
    "parallel_mode": "sync",
    "model_url": MODEL_URL,
    "model_name": MODEL_NAME,
    "generation_config": {"temperature": 0.8, "max_tokens": 512},
    "model_max_retries": 2,
    "timeout_seconds": 300,
}

prepared = [
    to_uenv_request(sample, rollout_context)
    for sample in framework_batch
]
original_by_id = {
    request.request_id: sample
    for request, sample in zip(prepared, framework_batch)
}

channel = grpc.insecure_channel(UENV_ENDPOINT)
try:
    client = rpc.AdapterCoreServiceStub(channel)
    reply = client.ExecuteBatch(
        pb.ExecuteBatchRequest(
            request_id="batch-1-call-1",
            batch_id="batch-1",
            samples=prepared,
        ),
        timeout=330,
    )

    result_by_id = {item.request_id: item for item in reply.results}
    missing_ids = set(original_by_id) - set(result_by_id)
    if missing_ids:
        raise RuntimeError(f"UEnv did not return results for: {sorted(missing_ids)}")

    framework_outputs = [
        to_framework_output(result_by_id[request.request_id], original_by_id[request.request_id])
        for request in prepared
    ]
finally:
    channel.close()

print(framework_outputs[0])
```

这段代码的调用链只有 `to_uenv_request() → ExecuteBatch → to_framework_output()`。`prompt_ids` 始终从原 sample 保留，不需要从 UEnv 结果恢复。示例中的输出转换会要求 Worker 返回真实的 token trace；如果当前模型服务只返回 action 文本，应先让模型服务或 Worker 回传实际生成的 `response_ids` 和 `response_mask`，不要在训练适配层中使用可能不同的 tokenizer 静默重新编码。

## 如果框架采用异步训练

异步训练中，rollout 生产端持续产生结果，模型更新端（learner）同时消费已经完成的 rollout 并更新模型。UEnv 可以承担 rollout 执行，但不会替代强化学习框架的结果队列和训练调度。

`ExecuteBatch` 会等待本批所有样本完成后再返回。异步框架可以改用双向流 `ExecuteBatchStream`，在同一连接上持续发送 `SampleEnvelope`，并在每条 rollout 完成时收到对应的 `SampleResult`。返回顺序可能与提交顺序不同，因此仍要使用 `request_id` 关联结果。

`parallel_mode` 要与框架的调度方式一致：如果框架只允许 rollout 落后当前模型一个更新步，使用 `one_step_off_policy`；rollout 生产与模型更新持续解耦时，使用 `fully_async`。其他环境和 Episode 字段与同步请求相同。框架提供的模型服务还必须在每次生成响应中给出实际使用的策略版本和 token 级 logprob；UEnv 会将它们作为 `rollout_param_version`、`rollout_policy_version` 和 `rollout_log_probs` 返回。

框架侧需要维护待执行 sample、在途 `request_id` 和 learner 输入队列；模型更新后，将新权重发布到 `model_endpoint` 对应的服务；收到结果时，根据 rollout 版本决定接受、修正还是丢弃过旧结果。下面的完整生成器复用前文的两个转换函数：

```python
import threading


def stream_uenv_rollouts(client, framework_samples, rollout_context):
    """边提交 sample，边按完成顺序产出框架 rollout 结果。"""
    if rollout_context["parallel_mode"] not in {
        "one_step_off_policy",
        "fully_async",
    }:
        raise ValueError("streaming rollout requires an asynchronous parallel_mode")

    pending = {}
    pending_lock = threading.Lock()

    def request_stream():
        for original_sample in framework_samples:
            request = to_uenv_request(original_sample, rollout_context)
            with pending_lock:
                if request.request_id in pending:
                    raise ValueError(f"duplicate request_id: {request.request_id}")
                pending[request.request_id] = original_sample
            yield request

    missing = object()
    for result in client.ExecuteBatchStream(request_stream()):
        with pending_lock:
            original_sample = pending.pop(result.request_id, missing)
        if original_sample is missing:
            raise RuntimeError(f"unknown request_id: {result.request_id}")
        yield to_framework_output(result, original_sample)

    with pending_lock:
        unfinished_ids = sorted(pending)
    if unfinished_ids:
        raise RuntimeError(f"UEnv did not return results for: {unfinished_ids}")


async_rollout_context = dict(rollout_context)
async_rollout_context["parallel_mode"] = "fully_async"
```

接入时，框架把 rollout producer 作为 `framework_samples` 传入，并将 `stream_uenv_rollouts()` 逐条产出的结果放入自己的 learner 队列。返回值已包含 `rollout_param_version`、`rollout_policy_version` 和 `rollout_log_probs`，框架可以在入队前执行自己的过旧判定。这些都是框架本来就要提供的调度能力，因此示例不再伪造未定义的队列或 learner API。

`ExecuteBatchStream` 只是流式传输通道，不是持久化任务队列。断线恢复、在途数量限制、模型权重同步和过旧 rollout 的处理策略都由强化学习框架负责。仅修改 `parallel_mode`，不会自动把同步训练改造成异步训练。

## 完成接入前检查

将新框架用于真实训练前，确认：

- 每次 rollout 的 `request_id` 唯一，传输重试时复用原 ID。
- 结果按 `request_id` 关联，不依赖返回顺序。
- `model_endpoint` 可从 UEnv Worker 实际访问。
- 每个环境需要的 `env_config` 和 `reward_config` 已完整提供。
- 只有 `status="completed"` 的结果进入训练；失败或超时不能静默转成普通零分样本。
- 交给框架的 `response_ids` 与 `response_mask` 非空且长度相同；异步结果中的 `rollout_log_probs` 长度也与它们一致。

如果你使用 VeRL，UEnv 已经实现了上述适配层，请直接阅读[以 VeRL 为例接入 UEnv](./02-verl.md)。
