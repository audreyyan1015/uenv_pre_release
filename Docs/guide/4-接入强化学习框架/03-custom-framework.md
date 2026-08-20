# 自定义强化学习框架接入

目标框架不在支持矩阵中时，在框架侧实现接入代码，把训练任务数据直接转换为 UEnv Server 接入的标准数据包。框架特例留在框架接入代码中，UEnv Server 保持通用。接入代码像普通 gRPC 客户端一样直接连接 UEnv Server 即可。UEnv Worker 的注册、心跳、能力和容量由 UEnv Server 管理，见[UEnv Worker 接入与注册](../2-部署UEnv/04-worker-registration.md)。

## 先固定训练语义

实现前写清楚：

1. hook 位于 sample 选择后、环境 rollout 前的哪个生命周期点。
2. 当前策略模型由 UEnv Worker 直接访问，还是由框架主机提供 model gateway。
3. 一条框架 sample 会产生几个 Episode；每个 rollout 如何生成唯一 ID。
4. 框架需要 response text、token/mask、logprob、reward、trajectory 中哪些字段。
5. 同步或异步训练怎样处理 staleness、取消、失败与权重切换。

如果现有 hook 已经完成本地生成，只想向 UEnv 请求 reward，应明确标为后置 reward 模式；不能宣称 UEnv 接管了完整 rollout。

## 最小项目骨架

一个可维护的 Python 接入项目至少包含：

```text
my_uenv_integration/
  integration.py            # sample/result 映射和 client 生命周期
  model_gateway.py          # 仅在训练模型位于框架主机时需要
  tests/
    test_mapping.py
    test_protocol.py
```

优先复用发布包或仓库中固定生成的 `adapter_core_pb2` / `adapter_core_pb2_grpc`。确实需要为其他语言重新生成客户端时，以 `proto/uenv/v1/adapter_core.proto` 为唯一真源，并在接入项目中锁定对应 UEnv release；不要复制后自行修改 proto。

Python 仓库开发环境：

```bash
python3 -m venv .venv
. .venv/bin/activate
python -m pip install grpcio protobuf pytest
export PYTHONPATH="$PWD/uenv-bridge/src"
```

## 建立 client 生命周期

接入代码启动时创建一个 channel/stub，训练结束或取消时关闭；不要为每个 sample 新建连接：

```python
import grpc
from uenv.bridge.gen import adapter_core_pb2 as pb
from uenv.bridge.gen import adapter_core_pb2_grpc as rpc


class UEnvClient:
    def __init__(self, endpoint: str):
        self.channel = grpc.insecure_channel(endpoint)
        self.stub = rpc.AdapterCoreServiceStub(self.channel)

    def health(self) -> dict:
        reply = self.stub.HealthCheck(pb.HealthCheckRequest(), timeout=5)
        if not reply.ok:
            raise RuntimeError(f"UEnv Server unhealthy: {reply.version}")
        return {"ok": reply.ok, "version": reply.version}

    def close(self) -> None:
        self.channel.close()
```

生产环境在受控网络内使用该 endpoint；跨不可信网络时换成组织批准的 TLS channel 或隧道。

## 构造请求并保存 ID 映射

下面骨架明确返回 `request_id`，不会出现“构造时生成 ID、还原时却去 sample 上读取不存在属性”的错配：

```python
import json
import uuid
from dataclasses import dataclass
from typing import Any


def json_bytes(value: dict[str, Any]) -> bytes:
    return json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode()


def rollout_id(run_id: str, batch_id: str, sample_id: str, sample_index: int) -> str:
    key = f"{run_id}:{batch_id}:{sample_id}:{sample_index}"
    return f"rl-{uuid.uuid5(uuid.NAMESPACE_URL, key).hex}"


@dataclass(frozen=True)
class PreparedSample:
    request_id: str
    sample_index: int
    original: dict[str, Any]
    envelope: pb.SampleEnvelope


def prepare_sample(
    sample: dict[str, Any], *, run_id: str, batch_id: str, sample_index: int
) -> PreparedSample:
    request_id = rollout_id(run_id, batch_id, str(sample["id"]), sample_index)
    env_type = str(sample["env_type"])
    dataset = str(sample["dataset"])
    max_steps = int(sample["max_steps"])
    env_config = dict(sample.get("env_config", {}))
    env_config.update(question=sample["question"], dataset=dataset)
    reward_config = sample.get("reward_config")
    if reward_config is None:
        reward_config = {"type": "rule_reward", "target": sample["target"]}

    envelope = pb.SampleEnvelope(
        request_id=request_id,
        batch_id=batch_id,
        sample_index=sample_index,
        framework="my-rl-framework",
        env_type=env_type,
        parallel_mode="sync",
        env_config_json=json_bytes(env_config),
        episode_config_json=json_bytes({
            "max_steps": max_steps,
            "seed": int(sample.get("seed", sample_index)),
        }),
        reward_config_json=json_bytes(reward_config),
        model_endpoint=pb.ModelEndpoint(
            endpoint_type="http",
            url=sample["model_url"],
            model_name=sample["model_name"],
            generation_config_json=json_bytes(sample.get("generation_config", {})),
            max_retries=2,
        ),
        timeout_seconds=int(sample.get("timeout_seconds", 300)),
        correlation_id=f"{run_id}:{batch_id}:{sample_index}",
        sample_context_json=json_bytes({
            "run_id": run_id,
            "sample_id": sample["id"],
        }),
    )
    return PreparedSample(request_id, sample_index, sample, envelope)
```

必须在提交前校验：ID 非空且批次内唯一；JSON 字段是对象；`env_type`、dataset 和 max steps 显式存在；模型 URL 对 UEnv Worker 可达；reward 足以判分；payload 没有密钥。

同一逻辑请求的传输重试复用同一 `request_id`。需要新的 rollout 时，在 ID 输入中加入 rollout 序号，生成新 ID。

## 提交批次并按 ID 还原

最小 unary 批次骨架：

```python
def extract_training_trace(result: pb.SampleResult) -> dict[str, Any]:
    trajectory = json.loads(result.trajectory_json or b"{}")
    steps = trajectory.get("steps", [])
    traces = [s.get("rollout_trace", {}) for s in steps if isinstance(s, dict)]
    response_ids = [token for t in traces for token in t.get("response_ids", [])]
    response_mask = [mask for t in traces for mask in t.get("response_mask", [])]
    if len(response_ids) != len(response_mask):
        raise RuntimeError(f"token/mask length mismatch for {result.request_id}")
    return {
        "response_ids": response_ids,
        "response_mask": response_mask,
        "reward": result.reward,
        "trajectory": trajectory,
        "termination_reason": result.termination_reason,
        "rollout_policy_version": result.rollout_policy_version,
        "rollout_log_probs": list(result.rollout_log_probs),
    }


def run_batch(
    client: UEnvClient,
    samples: list[dict[str, Any]],
    *,
    run_id: str,
    batch_id: str,
) -> list[dict[str, Any]]:
    prepared = [
        prepare_sample(sample, run_id=run_id, batch_id=batch_id, sample_index=index)
        for index, sample in enumerate(samples)
    ]
    by_id = {item.request_id: item for item in prepared}
    if len(by_id) != len(prepared):
        raise RuntimeError("duplicate request_id before submit")

    reply = client.stub.ExecuteBatch(
        pb.ExecuteBatchRequest(
            request_id=f"batch-call-{batch_id}",
            batch_id=batch_id,
            samples=[item.envelope for item in prepared],
        ),
        timeout=900,
    )

    restored: list[dict[str, Any] | None] = [None] * len(prepared)
    seen: set[str] = set()
    for result in reply.results:
        if result.request_id not in by_id or result.request_id in seen:
            raise RuntimeError(f"unknown or duplicate result: {result.request_id}")
        seen.add(result.request_id)
        item = by_id[result.request_id]
        if result.status != "completed":
            raise RuntimeError(
                f"Episode failed: id={result.request_id} status={result.status} "
                f"error={result.error_code}:{result.error_message}"
            )
        restored[item.sample_index] = {
            "sample": item.original,
            **extract_training_trace(result),
        }

    missing = set(by_id) - seen
    if missing:
        raise RuntimeError(f"missing UEnv results: {sorted(missing)}")
    return [item for item in restored if item is not None]
```

不要写 `zip(samples, results)`。生产实现还要验证 mask 是否存在有效 token、reward 类型、logprob 长度、策略版本与允许的 staleness。

需要双向流时保留同一 `PreparedSample` 与 `by_id` 逻辑，仅把发送/接收改成独立协程，并给本地 in-flight 设置上限。

## 模型回调与资源处理

训练模型通常位于 GPU 主机。接入代码可启动 OpenAI-compatible model gateway，并在 envelope 中传入 UEnv Worker 可达 URL：

- URL 不能使用对远程 UEnv Worker 无效的 loopback。
- 模型版本与结果的 rollout policy/version 必须可追溯。
- 权重更新与 in-flight rollout 的一致性由框架策略决定。
- 框架取消时停止新提交、关闭 stream/channel，并停止由接入代码自己创建的 gateway。
- 接入代码不能停止外部 UEnv Server 或 UEnv Worker。

## 失败与背压

- gRPC deadline 与 Episode timeout 分开配置。
- 传输重试保持 `request_id` 不变。
- `RESOURCE_EXHAUSTED` 时降低 in-flight 并有界退避。
- 默认把失败 Episode 抛给框架；使用零分策略时保留原状态与错误字段，并把 response mask 置为不可训练。
- 框架退出时等待或明确取消已提交请求，不能悄悄丢弃未知结果。

## 四层测试与发布

### 映射单测

- sample→envelope 的 prompt、env、reward、model 与 ID 正确。
- result→framework output 的 token/mask/reward/trajectory 正确。
- Unicode、空值、大 JSON 和边界长度明确。

### 协议测试

- 乱序结果恢复原顺序。
- 重复、缺失、未知 ID 均失败。
- timeout、取消、`RESOURCE_EXHAUSTED` 和幂等重放行为稳定。

### 本地闭环

在真实 UEnv Server 与 UEnv Worker 上运行一个目标环境 Episode，确认终态、reward 和 trajectory 能按共同 ID 关联。

### 框架端到端

使用目标框架完成计划的模型更新并写出指标/checkpoint，验证失败没有静默变成普通训练数据。

本地测试入口至少应能执行：

```bash
python -m pytest -q my_uenv_integration/tests
```

四层全部完成后，才在[支持矩阵](./05-support-matrix.md)记录固定版本、用户入口、限制和验收证据；否则保持“实验”或“规划”。
