# UENV 仿真环境系统联调测试方案v0\.01

总体说明：

描述各个板块本次实现及测试的主要内容，描述清楚各个部分提供的相关接口及数据结构。

# 第一部分：UENV Adapter（荣皓）

## 模块定位

UENV Adapter 是 UEnv 面向 VeRL 的训练框架适配层。当前主链路为：

主链路图为：

![Image](https://internal-api-drive-stream.feishu.cn/space/api/box/stream/download/authcode/?code=NjZiZDJhMWQ5Nzg1MDY5MDQ5YjlmZGEzYTM0YjEwZmJfNWE2M2UyZjY2ZWEzMWNkZTQ0MzdmMTQyNjQyNWYwZmFfSUQ6NzY0OTk3NTQ3NDQ1Nzg0MDg0NF8xNzgxNjY4NDAyOjE3ODE3NTQ4MDJfVjM)

这里有三层请求/结果对象，名称相近但边界不同：

因此，gRPC 只出现在 Python shim 和 Rust adapter core 之间。Rust adapter core 与 UEnv Server/Worker 之间是 Rust 函数调用边界。

## 本次实现内容

## 对外接口

### 3\.1 VeRL \-\> Python shim

VeRL 通过自定义 AgentLoop 接入：

```Python
actor_rollout_ref.rollout.agent.default_agent_loop=uenv_agent
actor_rollout_ref.rollout.agent.agent_loop_config_path=/tmp/uenv-bridge/configs/uenv-agent-loop.yaml
```

核心配置项包括：

### 3\.2 Python shim \-\> Rust adapter core

Python shim 与 Rust adapter core 通过 gRPC 通信，Python shim 发送的核心中间结构是 `SampleEnvelope`;Rust adapter core 返回 `SampleResult`。详细的数据结构信息见第4节主要数据结构。

### 3\.3 Rust Adapter Core \-\> UEnv Server/Worker

Rust adapter core 与 UEnv Server/Worker 不通过 Python gRPC 通信，而是通过 Rust trait 函数边界：

```Rust
pub trait EpisodeService: Send + Sync {
    fn submit_episode_batch(
        &self,
        requests: Vec<EpisodeRequest>,
    ) -> impl Future<Output = Result<Vec<EpisodeResult>, EpisodeServiceError>> + Send;
}
```

Serve 侧需要实现 `EpisodeService`，负责调度 Worker、调用模型、执行环境、计算 reward，并返回同一批次的 `EpisodeResult`。返回顺序可以不同，但每个 `episode_id` 必须能对应输入请求；缺失、重复或未知 id 会被 adapter core 判为错误。

## 主要数据结构

本章只介绍主链路上的 6 个核心数据结构，按数据流顺序排列：

|顺序|数据结构|所属边界|作用|
|---|---|---|---|
|1<br>|Python `EpisodeRequest`|`UEnvAgentLoop` \-\> Python shim|Python 内部请求对象，保存从 VeRL sample 解析出的 pre\-rollout episode 输入。|
|2|`SampleEnvelope`|Python shim \-\> Rust adapter core|gRPC 传输信封，承载 Python `EpisodeRequest` 的核心内容。|
|3|server `EpisodeRequest`|Rust adapter core \-\> Server/Worker|Server/Worker 侧业务请求，由 Rust adapter core 转换生成。|
|4|server `EpisodeResult`|Server/Worker \-\> Rust adapter core|Server/Worker 返回的 episode 执行结果。|
|5|`SampleResult`|Rust adapter core \-\> Python shim|gRPC 传输结果，由 server `EpisodeResult` 转换生成。|
|6|Python `EpisodeResult`|Python shim \-\> `UEnvAgentLoop`|Python 内部结果对象，后续转成 VeRL `AgentLoopOutput`。|

这里需要特别区分三类名称相近的对象：Python `EpisodeRequest` 是 Python dataclass；`SampleEnvelope` 是 Python 与 Rust adapter core 之间的传输对象；server `EpisodeRequest` 是 Serve/Worker 侧 proto 业务对象。`SampleEnvelope.payload_json` 的内容来自 Python `EpisodeRequest.payload`，但它不是 server `EpisodeRequest.payload`。

### **4\.1 Python EpisodeRequest**

定义位置：`src/uenv/bridge/protocol.py`。这是 Python shim 内部请求对象，由 `UEnvAgentLoop` 构造，并交给 Python shim 包装成 `SampleEnvelope`。

|字段|类型|说明|示例|
|---|---|---|---|
|`request_id`|`str`|单个 VeRL sample 对应的 episode id。后续会传到 `SampleEnvelope.request_id`，再变成 server `EpisodeRequest.episode_id`。|`verl-agent-loop-0001-0`|
|`env_type`|`str`|环境类型。数学任务使用 `math`。|`math`|
|`payload`|`bytes`|UTF\-8 JSON bytes，是 Python shim 内部保存的 PRD 风格 episode payload。pre\-rollout 阶段这里包含 prompt、prompt token、模型端点、生成配置、reward 配置和元信息。|见下方完整示例|
|`mode`|`int`|Python 内部执行模式。当前 pre\-rollout 主线使用 `MODE_MULTI=2`。|`2`|
|`max_steps`|`int`|episode 最大 step 数。该值同时也会出现在 `payload.episode_config.max_steps` 中，Rust adapter core 最终从 `payload_json` 解析并写入 server `EpisodeRequest.max_steps`。|`10`|
|`resource_spec`|`ResourceSpec`|Python 内部资源需求描述。当前 adapter 不直接调度资源，通常使用默认值。|`ResourceSpec(cpu_cores=0, memory_mb=0, gpu_count=0, gpu_type="")`|
|`model_endpoint`|`str`|Server/Worker 调用模型时使用的 OpenAI\-compatible endpoint。|`http://10.10.20.142:18080/v1`|
|`seed`|`int`|`None`|episode 随机种子。|

Python dataclass 示例：

```Python
EpisodeRequest(
    request_id="verl-agent-loop-0001-0",
    env_type="math",
    payload=b'{"protocol_version":"1.0","framework":"verl",...}',
    mode=MODE_MULTI,
    max_steps=10,
    resource_spec=ResourceSpec(cpu_cores=0, memory_mb=0, gpu_count=0, gpu_type=""),
    model_endpoint="http://10.10.20.142:18080/v1",
    seed=42,
)
```

其中 `payload` 解码后是完整 JSON。下面为了可读性写成 JSON 对象，实际在 Python `EpisodeRequest.payload` 中是 `bytes`：

```JSON
{
  "protocol_version": "1.0",
  "framework": "verl",
  "correlation_id": "verl-step-0001-sample-0",
  "env_config": {
    "task_name": "math",
    "env_type": "math",
    "data_source": "openai/gsm8k",
    "dataset": "openai/gsm8k",
    "question": "What is 2+2?",
    "raw_prompt": "What is 2+2?"
  },
  "model_endpoint": {
    "endpoint_type": "openai_compatible",
    "url": "http://10.10.20.142:18080/v1",
    "model_name": "mock-policy",
    "generation_config": {
      "temperature": 1.0,
      "top_p": 1.0,
      "max_new_tokens": 32,
      "do_sample": true
    }
  },
  "episode_config": {
    "max_steps": 10,
    "max_turns": 1,
    "seed": 42,
    "initial_observation": {
      "raw_prompt": [
        {"role": "user", "content": "What is 2+2?"}
      ],
      "prompt_text": "What is 2+2?",
      "prompt_ids": [151644, 872, 3923, 374, 220, 17, 10, 17, 30, 151645],
      "attention_mask": [1, 1, 1, 1, 1, 1, 1, 1, 1, 1],
      "token_source": "verl_agent_loop"
    }
  },
  "reward_config": {
    "reward_type": "rubric",
    "rubric_config": {
      "ground_truth": "4"
    }
  },
  "metadata": {
    "batch_id": "verl-step-0001",
    "sample_index": 0,
    "data_source": "openai/gsm8k",
    "extra_info": {
      "question": "What is 2+2?",
      "answer": "4",
      "split": "train"
    },
    "required_result_fields": [
      "response_ids",
      "response_mask",
      "response_text",
      "reward",
      "trajectory",
      "finish_reason"
    ]
  },
  "timeout_seconds": 300
}
```

### **4\.2 SampleEnvelope**

定义位置：`proto/uenv/v1/adapter_core.proto`。这是 Python shim 通过 gRPC 发给 Rust adapter core 的传输信封。

|字段|类型|说明|示例|
|---|---|---|---|
|`request_id`|`string`|对应 Python `EpisodeRequest.request_id`。Rust adapter core 会把它转成 server `EpisodeRequest.episode_id`。|`verl-agent-loop-0001-0`|
|`batch_id`|`string`|VeRL batch id，用于批量关联和结果回填。通常来自 Python payload 的 `metadata.batch_id`。|`verl-step-0001`|
|`sample_index`|`uint32`|sample 在 batch 内的位置。|`0`|
|`framework`|`string`|来源训练框架。当前为 `verl`。|`verl`|
|`env_type`|`string`|环境类型。Rust adapter core 会把它转成 server `EpisodeRequest.env_type`。|`math`|
|`payload_json`|`bytes`|与 Python `EpisodeRequest.payload` 内容相同的 JSON bytes。这里仍是 pre\-rollout prompt\-only payload；`max_steps`、`seed`、`model_endpoint`、`reward_config` 等业务字段都在这个 JSON 内部。|见下方完整示例|
|`meta_json`|`bytes`|gRPC 传输层额外元数据，当前主要用于保留调试信息，可以为空 JSON。它不替代 `payload_json.metadata`。|`b'{"transport":"python-shim"}'`|

`SampleEnvelope` 外层只保存跨语言传输和 batch 回填所需字段。server `EpisodeRequest.max_steps` 的来源是 `payload_json.episode_config.max_steps`，Rust adapter core 在 `sample_to_episode_request()` 中解析该字段并写入 server `EpisodeRequest.max_steps`。

`SampleEnvelope.payload_json` 与 Python `EpisodeRequest.payload` 是同一份 JSON 内容在 gRPC 传输对象中的承载形式。下面为了可读性把 bytes 展开为 JSON：

```JSON
{
  "request_id": "verl-agent-loop-0001-0",
  "batch_id": "verl-step-0001",
  "sample_index": 0,
  "framework": "verl",
  "env_type": "math",
  "payload_json": {
    "protocol_version": "1.0",
    "framework": "verl",
    "correlation_id": "verl-step-0001-sample-0",
    "env_config": {
      "task_name": "math",
      "env_type": "math",
      "data_source": "openai/gsm8k",
      "dataset": "openai/gsm8k",
      "question": "What is 2+2?",
      "raw_prompt": "What is 2+2?"
    },
    "model_endpoint": {
      "endpoint_type": "openai_compatible",
      "url": "http://10.10.20.142:18080/v1",
      "model_name": "mock-policy",
      "generation_config": {
        "temperature": 1.0,
        "top_p": 1.0,
        "max_new_tokens": 32,
        "do_sample": true
      }
    },
    "episode_config": {
      "max_steps": 10,
      "max_turns": 1,
      "seed": 42,
      "initial_observation": {
        "raw_prompt": [
          {"role": "user", "content": "What is 2+2?"}
        ],
        "prompt_text": "What is 2+2?",
        "prompt_ids": [151644, 872, 3923, 374, 220, 17, 10, 17, 30, 151645],
        "attention_mask": [1, 1, 1, 1, 1, 1, 1, 1, 1, 1],
        "token_source": "verl_agent_loop"
      }
    },
    "reward_config": {
      "reward_type": "rubric",
      "rubric_config": {"ground_truth": "4"}
    },
    "metadata": {
      "batch_id": "verl-step-0001",
      "sample_index": 0,
      "data_source": "openai/gsm8k",
      "extra_info": {
        "question": "What is 2+2?",
        "answer": "4",
        "split": "train"
      },
      "required_result_fields": [
        "response_ids",
        "response_mask",
        "response_text",
        "reward",
        "trajectory",
        "finish_reason"
      ]
    },
    "timeout_seconds": 300
  },
  "meta_json": {
    "transport": "python-shim",
    "note": "debug metadata only"
  }
}
```

转换关系说明：

|来源|目标|说明|
|---|---|---|
|`EpisodeRequest.request_id`|`SampleEnvelope.request_id`|保持不变。|
|`EpisodeRequest.env_type`|`SampleEnvelope.env_type`|保持不变。|
|`EpisodeRequest.payload`|`SampleEnvelope.payload_json`|内容保持不变，都是 JSON bytes。|
|`payload_json.metadata.batch_id`|`SampleEnvelope.batch_id`|用于 batch 级结果回填。|
|`payload_json.metadata.sample_index`|`SampleEnvelope.sample_index`|用于 sample 级结果回填。|

### **4\.3 server EpisodeRequest**

定义位置：`proto/uenv/v1/episode.proto`。这是 Rust adapter core 根据 `SampleEnvelope.payload_json` 转换出的 Server/Worker 侧业务请求。

|字段|类型|说明|示例|
|---|---|---|---|
|`episode_id`|`string`|Server/Worker 侧 episode id；来自 `SampleEnvelope.request_id`。|`verl-agent-loop-0001-0`|
|`attempt_id`|`uint32`<br>|第几次执行尝试；MVP 通常为默认值。|`0`|
|`env_type`|`string`|环境类型。|`math`|
|`payload`|`bytes`|Worker 消费的 payload，通常包含 `question`、`dataset`、`model_endpoint`、`model_name`、`generation_config`。|`b'{"question":"What is 2+2?",...}'`|
|`mode`|`ExecutionMode`|Server/Worker 执行模式。|`EXECUTION_MODE_MULTI`|
|`max_steps`|`int32`|最大环境 step 数。|`10`|
|`resource_spec`|`ResourceSpec`|Server/Worker 侧资源需求。|`{}`|
|`model_endpoint`|`string`|模型服务地址。|`http://10.10.20.142:18080/v1`|
|`seed`|`optional int32`|随机种子。|`42`|
|`correlation_id`|`string`|跨组件排障关联 id。|`verl-step-0001-0`|
|`timeout_seconds`|`int32`|episode 超时时间。|`300`|
|`reward_config`|`bytes`|Worker/reward engine 使用的 reward 配置。|`b'{"type":"rule_reward","target":"4"}'`|
|`dispatch_lease_id`|`string`|Server 调度 Worker 时填充的租约 id。|`lease-001`|
|`lease_expire_at`|`google.protobuf.Timestamp`|租约过期时间。|`2026-06-12T10:00:00Z`|
|`scheduler_epoch`|`uint64`|调度轮次或调度器 epoch。|`1`|
|`dispatch_token`|`bytes`|调度鉴权或校验 token。|`b'...'`|

示例：

```JSON
{
  "episode_id": "verl-agent-loop-0001-0",
  "attempt_id": 0,
  "env_type": "math",
  "payload": {
    "request_id": "verl-agent-loop-0001-0",
    "question": "What is 2+2?",
    "dataset": "openai/gsm8k",
    "model_endpoint": "http://10.10.20.142:18080/v1",
    "model_name": "mock-policy",
    "generation_config": {
      "temperature": 1.0,
      "top_p": 1.0,
      "max_new_tokens": 32,
      "do_sample": true
    }
  },
  "mode": "EXECUTION_MODE_MULTI",
  "max_steps": 10,
  "model_endpoint": "http://10.10.20.142:18080/v1",
  "seed": 42,
  "correlation_id": "verl-step-0001-0",
  "timeout_seconds": 300,
  "reward_config": {"type": "rule_reward", "target": "4"}
}
```

注意：server `EpisodeRequest.payload` 不是 `SampleEnvelope.payload_json` 的原样拷贝，而是 Rust adapter core 从 `payload_json.env_config`、`payload_json.metadata.extra_info`、`payload_json.model_endpoint` 中提取后构造出的 Worker payload。

### **4\.4 server EpisodeResult**

定义位置：`proto/uenv/v1/episode.proto`。这是 Server/Worker 执行 episode 后返回给 Rust adapter core 的业务结果。

|字段|类型|说明|示例|
|---|---|---|---|
|`episode_id`|`string`|必须对应输入 `episode_id`，用于结果匹配。|`verl-agent-loop-0001-0`|
|`attempt_id`|`uint32`|对应输入尝试次数。|`0`|
|`status`|`string`|episode 状态。|`completed`|
|`trajectory`|`Trajectory`|执行轨迹，包含 step 列表、总 reward、总 step 数。|`{steps:[...], total_reward:1.0, total_steps:1}`|
|`summary`|`EpisodeResult.Summary`|汇总信息。|`{total_reward:1.0, total_steps:1, terminate_reason:"completed"}`|
|`error_code`|`optional ErrorCode`|失败时的错误码。|`null`|
|`error_message`|`string`|失败时的错误信息。|`""`|
|`trajectory_checksum`|`string`|trajectory 完整性校验值。|`sha256:...`|
|`integrity_verified`|`bool`|trajectory 校验是否通过。|`true`|

`Trajectory.steps` 中的 `StepRecord` 常用字段：

|字段|类型|说明|示例|
|---|---|---|---|
|`step_index`|`int32`|第几个环境 step。|`1`|
|`observation`|`bytes`|环境观测。|`b'What is 2+2?'`|
|`action`|`bytes`|模型生成的 action / response。|`b'4'`|
|`reward`|`double`|当前 step reward。|`1.0`|
|`terminated`|`bool`|episode 是否正常结束。|`true`|
|`truncated`|`bool`|是否因上限等原因截断。|`false`|
|`info`|`map<string,string>`|附加信息；建议包含 `response_ids`、`response_mask`、`response_text`。|`{"response_ids":"[19]","response_mask":"[1]","response_text":"4"}`|
|`duration_ms`|`int64`|当前 step 耗时。|`120`|

示例：

```JSON
{
  "episode_id": "verl-agent-loop-0001-0",
  "attempt_id": 0,
  "status": "completed",
  "trajectory": {
    "steps": [
      {
        "step_index": 1,
        "observation": "What is 2+2?",
        "action": "4",
        "reward": 1.0,
        "terminated": true,
        "truncated": false,
        "info": {
          "response_ids": "[19]",
          "response_mask": "[1]",
          "response_text": "4"
        },
        "duration_ms": 120
      }
    ],
    "total_reward": 1.0,
    "total_steps": 1
  },
  "summary": {
    "total_reward": 1.0,
    "total_steps": 1,
    "total_duration_ms": 120,
    "terminate_reason": "completed"
  },
  "error_message": "",
  "trajectory_checksum": "sha256:...",
  "integrity_verified": true
}
```

### **4\.5 SampleResult**

定义位置：`proto/uenv/v1/adapter_core.proto`。这是 Rust adapter core 将 server `EpisodeResult` 转换后，通过 gRPC 返回给 Python shim 的传输结果。

|字段|类型|说明|示例|
|---|---|---|---|
|`request_id`|`string`|对应输入 `SampleEnvelope.request_id`。|`verl-agent-loop-0001-0`|
|`batch_id`|`string`|对应输入 batch id。|`verl-step-0001`|
|`sample_index`|`uint32`|对应输入 sample index。|`0`|
|`status`|`string`|执行状态。|`completed`|
|`reward`|`double`|episode 总 reward。|`1.0`|
|`done`|`bool`|episode 是否结束。|`true`|
|`termination_reason`|`string`|结束原因。|`completed`|
|`trajectory_json`|`bytes`|JSON 编码后的 trajectory，供 Python shim 还原。|`b'{"steps":[...]}'`|
|`error_code`|`string`|失败错误码。|`""`|
|`error_message`|`string`|失败错误信息。|`""`|

示例：

```JSON
{
  "request_id": "verl-agent-loop-0001-0",
  "batch_id": "verl-step-0001",
  "sample_index": 0,
  "status": "completed",
  "reward": 1.0,
  "done": true,
  "termination_reason": "completed",
  "trajectory_json": {
    "steps": [
      {
        "step_index": 1,
        "observation": "What is 2+2?",
        "action": "4",
        "reward": 1.0,
        "terminated": true,
        "truncated": false,
        "info": {"response_ids": "[19]", "response_mask": "[1]", "response_text": "4"},
        "duration_ms": 120
      }
    ],
    "total_reward": 1.0,
    "total_steps": 1
  },
  "error_code": "",
  "error_message": ""
}
```

### **4\.6 Python EpisodeResult**

定义位置：`src/uenv/bridge/protocol.py`。这是 Python shim 将 `SampleResult` 还原后的内部结果对象，随后会被 `UEnvAgentLoop` 转成 VeRL `AgentLoopOutput`。

|字段|类型|说明|示例|
|---|---|---|---|
|`request_id`|`str`|对应 Python `EpisodeRequest.request_id`。|`verl-agent-loop-0001-0`|
|`status`|`str`|执行状态。|`completed`|
|`trajectory`|`Trajectory`|Python 内部 trajectory 对象。|`Trajectory(steps=[...], total_reward=1.0, total_steps=1)`|
|`summary`|`EpisodeSummary`|Python 内部汇总对象。|`EpisodeSummary(total_reward=1.0, total_steps=1, total_duration_ms=120, terminate_reason="completed")`|
|`error_code`|`int`|`None`|失败错误码；无错误时为空。|
|`error_message`|`str`|失败错误信息。|`""`|

示例：

```Python
EpisodeResult(
    request_id="verl-agent-loop-0001-0",
    status="completed",
    trajectory=Trajectory(
        steps=[
            StepRecord(
                step_index=1,
                observation=b"What is 2+2?",
                action=b"4",
                reward=1.0,
                terminated=True,
                truncated=False,
                info={"response_ids": "[19]", "response_mask": "[1]", "response_text": "4"},
                duration_ms=120,
            )
        ],
        total_reward=1.0,
        total_steps=1,
    ),
    summary=EpisodeSummary(
        total_reward=1.0,
        total_steps=1,
        total_duration_ms=120,
        terminate_reason="completed",
    ),
    error_code=None,
    error_message="",
)
```

## 测试与验证

Layer 4 使用真实 VeRL 入口：

```Bash
python3 -m verl.trainer.main_ppo \
  algorithm.adv_estimator=grpo \
  actor_rollout_ref.rollout.agent.default_agent_loop=uenv_agent
```

## 当前联调注意事项

1. 当前主线是 pre\-rollout 接管，不再使用 post\-rollout RewardManager 路线。

2. Bridge 目前会把 `model_endpoint` 传给 Server/Worker，作为一个mock model，后续模型的管理责任还未明确。

3. VeRL 训练主体已经能完成 1\-step 和 2\-step；部分运行在结束时可能仍出现 vLLM/Ray 资源释放阶段的 teardown 日志噪声。该问题发生在训练完成之后，不影响 Server 返回结果、reward、trajectory 和训练指标生成。

# 第二部分：UENV Server（子翼）

本节介绍 UEnv 系统里**控制平面**这部分程序的设计。控制平面做四件事：接收训练程序提交的任务（一个任务叫一个 Episode）、记录有哪些 Worker 可用、决定每个任务交给哪个 Worker、把任务发给 Worker 并等它返回结果，最后把结果交回给提交方。

控制平面的代码写在库 `uenv-server` 里（它本身不能单独运行，只是一堆被别人调用的代码）。真正能运行起来、监听端口的程序是 `uenv-adapter-core`（源码在 `uenv-bridge/core/src/main.rs`）。这个程序把 `uenv-server` 当作库引入，并对外提供三个 gRPC 接口：给 Python 训练框架用的 `AdapterCoreService`、给 Worker 用的 `ControlPlaneService`、给运维用的 `AdminService`。

## 1\. 对外接口

```Plaintext
Python VeRL shim ────AdapterCoreService──▶ uenv-adapter-core ┐
Worker ──────────────ControlPlaneService─▶   (内嵌            │ ServerState
运维工具 ────────────AdminService────────▶    uenv-server 库) ┘  共享状态
```

`uenv-adapter-core` 进程在一个端口（默认 `[::]:50051`）上同时提供上面三个 gRPC 接口。这三个接口操作的是同一份内存数据 `ServerState`（见 §2\.2）。

除了「别人调用我」的三个接口，这个进程还会**反过来主动调用 Worker**：当需要把任务发给某个 Worker 时，它作为客户端去连接 Worker 提供的 `WorkerGrpcService`（见 §1\.4）。

下面每个接口表里的「类型」一列含义：

- **unary**：普通调用，发一个请求、收一个响应。

- **server stream**：发一个请求，服务端会陆续返回多条响应，直到结束。

- **bidi stream**：双向流，请求和响应都可以是连续多条。

### 1\.1 AdapterCoreService — 给 Python 训练框架用（主要入口）

> 接口定义：`proto/uenv/v1/adapter_core.proto`，包名 `uenv.bridge.v1`

VeRL 是训练框架。它的 Python 代码把一批待执行的样本（包含 prompt、采样参数、打分配置等信息）打包成 `SampleEnvelope` 发过来；Rust 这边把它们转成统一的内部格式再去执行。

|RPC|类型|说明|
|---|---|---|
|`ExecuteBatch`<br>|unary<br>|提交一批 `SampleEnvelope`，返回每个样本的结果 `SampleResult`（包含 reward、轨迹 JSON、状态）|
|`ExecuteBatchStream`|bidi stream|流式提交样本，收齐后整批执行，再逐条把结果流式返回|
|`HealthCheck`|unary|健康检查，返回是否正常 \+ 版本号|

实现代码在 `uenv-bridge/core`（crate `uenv-adapter-core`）：它把 `SampleEnvelope` 里的 JSON 字段（`episode_config`、`model_endpoint`、`reward_config` 等）提取出来，组装成内部的 `EpisodeRequest`，调用 `EpisodeService::submit_episode_batch` 执行，拿到 `EpisodeResult` 后再转回 `SampleResult`。

> **注意**：这条路径不经过额外的 gRPC。`AdapterCore` 内部直接持有一个 `UEnvEpisodeService` 对象，`ExecuteBatch` 就是在本进程里**直接调用它的方法**（普通的 Rust 函数调用），不需要再走一次网络序列化。

### 1\.2 ControlPlaneService — 给 Worker 用

> 接口定义：`proto/uenv/v1/scheduler.proto`

|RPC|类型|说明|
|---|---|---|
|`RegisterWorker`|unary<br>|Worker 启动后调用，上报自己的地址 `endpoint`、能处理的环境类型 `supported_env_types`、最大并发数 `max_concurrent`|
|`WorkerHeartbeat`|bidi stream<br>|Worker 周期性上报自己当前的负载；控制平面回复建议的下次心跳间隔|
|`ReportResult`<br>|unary|Worker 执行完一个任务后，调用它把结果送回来（带一个 `idempotency_key` 防止重复处理）|
|`ListWorkers`|unary|按环境类型查询当前有哪些 Worker|

### 1\.3 AdminService — 给运维用

> 接口定义：`proto/uenv/v1/server.proto`

- `ListWorkers`：列出所有 Worker。

- `DrainWorker`：把某个 Worker 下线（不再给它派任务）。

- `CancelEpisode`：取消某个正在进行的任务。

- `GetServerStatus`：查询整体状态（epoch、Worker 数量、正在执行的任务数、正在等待结果的任务数）。

### 1\.4 WorkerGrpcService — 控制平面反过来调用 Worker

> 接口定义：`uenv-worker/proto/worker_service.proto`

**这个接口是 Worker 提供的，不是控制平面提供的。** 控制平面在这里扮演客户端：

- `DispatchEpisode(EpisodeRequest) → stream StreamReport`：控制平面连接到 Worker 上报的 `endpoint`，把任务发过去；Worker 在执行过程中通过这个流不断返回进度。

- `HealthCheck`：探测 Worker 是否存活。

也就是说，Worker 既是 `ControlPlaneService` 的客户端（主动注册、上报），又是 `WorkerGrpcService` 的服务端（等控制平面来派活）。

---

## 2\. 内部结构

### 2\.1 代码组织

**uenv\-bridge/core（crate ****`uenv-adapter-core`****，可运行的程序）**

|文件|职责|
|---|---|
|`core/src/main.rs`|程序入口：读环境变量 `UENV_ADDR` / `UENV_ADAPTER_CORE_BACKEND`，注册 3 个 gRPC 接口并启动服务|
|`core/src/core.rs`|`AdapterCore`：负责 `SampleEnvelope` 和 `EpisodeRequest` / `EpisodeResult` 之间的相互转换，以及批次合法性检查|
|`core/src/service.rs`|`AdapterCoreService` 的 gRPC 实现（`ExecuteBatch` / `ExecuteBatchStream` / `HealthCheck`）|
|`core/src/server_api.rs`|从 `uenv-server` 重新导出 `EpisodeService` 接口（两个 crate 之间的连接点）|

**uenv\-server（纯库 crate，控制平面的核心逻辑）**

|文件|职责|
|---|---|
|`src/lib.rs`|导出各模块，并提供 `create_default_state()`（创建带默认调度器的 `ServerState`）|
|`src/state.rs`|定义共享数据结构 `ServerState`|
|`src/service.rs`|`UEnvEpisodeService`（提交任务的核心逻辑，见 §2\.4）\+ `AdminService` 实现 \+ `EpisodeService` 接口适配|
|`src/control_plane.rs`|`ControlPlaneService` 实现（注册、心跳、结果上报）|
|`src/scheduler/traits.rs`|`Scheduler` 接口定义，以及 `WorkerInfo` / `ScheduleError` 类型|
|`src/scheduler/mod.rs`|`RoundRobinScheduler`：轮询调度的具体实现|
|`src/proto.rs`|引入 `build.rs` 由 `.proto` 生成的 Rust 代码|

### 2\.2 ServerState：所有接口共享的数据

> 定义在 `uenv-server/src/state.rs`

这是一份在整个进程里共享的数据。它用 `Arc`（引用计数智能指针）包起来，分别传给每个 gRPC 接口，这样它们读写的是同一份数据。各字段含义：

- `scheduler: Arc<RwLock<RoundRobinScheduler>>` — 调度器，保存所有 Worker 的信息。`RwLock` 是读写锁：允许多个线程同时读，但写的时候独占。

- `active_episodes: DashMap<episode_id, ActiveEpisode>` — 当前正在调度或执行中的任务。`DashMap` 是一个可以被多个线程同时安全读写的哈希表。

- `pending_results: DashMap<(episode_id, attempt_id), PendingResult>` — 正在等待结果的任务表。每一项里存着一个 `oneshot` 发送端（`oneshot` 是只能用一次的单向通道：一个发送端、一个接收端，用来把一个值从一处传到另一处）。提交任务的代码把发送端存进这里并在接收端上等待；Worker 上报结果时，`ReportResult` 从这里取出发送端、把结果发出去，等待方就被唤醒。这就是「结果从上报路径回到提交路径」的具体做法。

- `completed_async: DashMap<episode_id, EpisodeResult>` — 异步提交的任务完成后，结果先存这里，供之后用 `get_result` 查询。

- `episode_broadcast: broadcast::Sender<EpisodeResult>` — 一个广播通道（一个发送端、多个接收端，每个接收端都能收到一份拷贝）。每完成一个任务就往这里发一份，订阅者（`subscribe`）能收到。

- `seen_idempotency: HashSet` — 记录已经处理过的 `idempotency_key`，用来识别重复的结果上报。

- `server_epoch` / `next_lease_seq` — 两个原子计数器（可以被多线程安全自增，不用加锁）。分别用于生成 epoch 和租约编号。

### 2\.3 调度器

> 定义在 `uenv-server/src/scheduler/`

`Scheduler` 是一个 Rust trait（接口），把「怎么选 Worker」抽象出来，方便以后换算法。当前实现是 `RoundRobinScheduler`（轮询）：

- `register_worker` / `unregister_worker`：增删 Worker。如果注册时 `worker_id` 已存在，会先删掉旧的再加新的，保证不会重复。

- `schedule(&req)`：选一个 Worker。先筛出满足两个条件的候选：\(1\) 它声明支持这个任务的环境类型 `env_type`；\(2\) 它当前负载没满（`current_load < capacity`）。然后用一个原子计数器对候选数量取模，轮流选中其中一个。如果没有候选，按原因返回不同错误：一个 Worker 都没有、没有支持该类型的、或全部满载。

- 负载数字有两个来源：控制平面自己在派发前后做 `+1` / `-1`（`increment_load` / `decrement_load`），以及 Worker 心跳上报的真实负载（`update_worker_load`，更准确，会覆盖前者）。

### 2\.4 UEnvEpisodeService：提交任务的核心代码

> 定义在 `uenv-server/src/service.rs`

这是控制平面里真正执行「提交一个任务」的代码。它和具体的网络协议无关——`AdapterCore`、批量入口、测试都直接当成普通对象调用它的方法。

|方法|说明|
|---|---|
|`submit_episode(req) -> Result<EpisodeResult>`|提交单个任务，走完整个流程（见 §3），一直阻塞到结果返回|
|`submit_episode_batch(reqs) -> Vec<Result<..>>`|用 `join_all` 同时执行多个 `submit_episode`；`AdapterCore` 的 `ExecuteBatch` 就调用它|
|`submit_episode_async(req) -> String`|在后台启动一个任务去执行，立即返回任务 id；结果完成后放进 `completed_async`，失败时额外广播一次|
|`get_result(&id) -> Option<EpisodeResult>`|按 id 查 `completed_async` 里的结果|
|`subscribe() -> broadcast::Receiver<EpisodeResult>`|订阅「任务完成」事件流|

> 说明：`submit_episode_async` / `get_result` / `subscribe` 目前没有调用方，是预留的接口，留给以后可能新增的异步提交或监控功能。

---

## 3\. 核心流程：提交一个 Episode 会发生什么

`service.rs` 里的 `submit_episode` 是整个流程的核心。一次调用按顺序做这些事：

1. **补默认值**：如果请求里没填 `episode_id`，生成一个 UUID；`attempt_id` 没填就用 1；超时时间没填就用 300 秒。

2. **选 Worker（失败会重试）**：调用 `scheduler.schedule()` 选一个 Worker。如果当前没有可用 Worker，就每 500 毫秒重试一次，直到超过截止时间才报错。

3. **登记等待**：创建一个 `oneshot` 通道。把发送端存进 `pending_results`，键是 `(episode_id, attempt_id)`。同时把任务记进 `active_episodes`，并给选中的 Worker 负载 `+1`。

4. **填充派发信息**：在请求里写上 `dispatch_lease_id`（这次派发的编号）、`scheduler_epoch`、`lease_expire_at`（这次派发的有效截止时间）。

5. **派发**：调用 `dispatch_to_worker()`，连接到 Worker 的地址，调它的 `DispatchEpisode`，并读取 Worker 返回的进度流 `StreamReport`。这一步返回后，把该 Worker 负载 `-1`，并从 `active_episodes` 移除这条记录。

6. **等结果**：在截止时间内，在第 3 步那个 `oneshot` 的接收端上等待。

7. **被唤醒**：Worker 执行完后调用 `ReportResult`，请求进到 `control_plane.rs`；它先用 `idempotency_key` 检查是不是重复上报，不是重复就从 `pending_results` 取出对应的发送端，把结果发出去——第 6 步的等待于是被唤醒，拿到结果。

8. **广播并返回**：把结果往 `episode_broadcast` 发一份（让订阅者收到），然后返回给调用方。如果是超时或通道异常关闭，就从 `pending_results` 删掉这条记录并返回错误。

```Plaintext
submit_episode ──┐                              control_plane.report_result
   选 Worker      │  pending_results[(id,att)]          │
   派发 ─────────┼──DispatchEpisode──▶ Worker          │
   等待接收端 ◀──┘◀────── oneshot 发送结果 ◀───────────┘
                                                  (取出发送端)
```

**各种入口最终都走这里**：生产路径是 `AdapterCoreService.ExecuteBatch` → `AdapterCore` 做格式转换 → `UEnvEpisodeService::submit_episode_batch`（用 `join_all` 同时跑多个 `submit_episode`）。后台异步入口 `submit_episode_async` 也是在新任务里跑同一个 `submit_episode`。所以不管从哪个入口进来，最后执行的都是上面这 8 步。

---

## 4\. 构建与运行

`uenv-server` 现在是纯库，不能单独运行；唯一能运行的程序是 `uenv-adapter-core`：

```Bash
cargo build -p uenv-adapter-core
UENV_ADDR="[::]:50051" ./target/debug/uenv-adapter-core
```

可用的环境变量：

|变量|默认值|说明|
|---|---|---|
|`UENV_ADDR`|`[::]:50051`|监听地址|
|`UENV_ADAPTER_CORE_BACKEND`|`server`|`server`=正常调度并派发给 Worker；`static_rollout`=不连 Worker、直接返回固定 reward（用于本地联调）|
|`UENV_ADAPTER_CORE_STATIC_*`|无|`static_rollout` 模式下返回的固定 reward / 文本 / token id|
|`RUST_LOG`|`info`|日志级别|

---

## 5\. 术语表

|术语|含义|
|---|---|
|**Episode**|一个待执行的任务，是调度和执行的基本单位|
|**控制平面（Control Plane）**|负责 Worker 注册、调度、派发任务、收集结果的服务，也就是本文档讲的程序|
|**Worker**|真正执行任务的进程；它向控制平面注册，由控制平面把任务发给它|
|**SampleEnvelope**|VeRL 侧打包好的样本数据，经 `AdapterCore` 转换成内部的 `EpisodeRequest`|
|**gRPC**|一种远程过程调用框架，让一个程序通过网络调用另一个程序的函数|
|**oneshot / broadcast / mpsc**|三种线程间通信通道：oneshot 是一发一收用一次；broadcast 是一发多收；mpsc 是多发一收的队列|
|**Lease（租约）**|一次派发携带的编号 `dispatch_lease_id` 和有效期 `lease_expire_at`|
|**Epoch**|控制平面的版本号（世代计数），用来判断 Worker / 调度状态是否还是同一代|



# 三、UENV Worker

### 1\.1 已实现功能

|模块|实现要点|代码位置|
|---|---|---|
|运行时|读 YAML 配置、加载插件目录、启动 gRPC \+ 可观测性 HTTP|runtime\.rs, main\.rs|
|控制面 Client|向 Server 注册、双向流心跳、上报 Episode 结果|control\_plane/client\.rs|
|数据面 Server|接收 DispatchEpisode，流式返回 StreamReport|grpc\_server/worker\_service\.rs|
|Episode 执行|单轮：acquire → reset → infer\_action → step → release → 判分|episode/executor\.rs|
|预热池|按 env\_type 维护 Warm 实例；命中/未命中指标|pool/warmup\_pool\.rs|
|插件宿主|ProcessBackend 子进程 \+ Proto/UDS；plugins/math/|plugin/host\.rs, backend/process\.rs|
|Hub 元数据|启动 pull manifest；EnvResolver 缺实例前校验|hub/mod\.rs, hub/env\_resolver\.rs|
|WAL|结果持久化 \+ 断连重放 ReportResult|wal/mod\.rs|
|可观测性|Prometheus 文本指标 \+ /health|metrics\.rs, runtime\.rs|
|Lease 校验|dispatch\_lease\_id 必填、过期/冲突拒绝|worker\_service\.rs|
|并发控制|Semaphore\(max\_concurrent\)|worker\_service\.rs|



7143 实机关键环境变量（见日志 03\-worker\-7143\.log）：

UENV\_MATH\_PLUGIN\_BIN=/root/UEnv/target/release/uenv\-math\-plugin

UENV\_PLUGIN\_DIR=/root/UEnv/plugins

UENV\_HUB\_TOKEN=\<Bearer token\>

UENV\_PREWARM\_ON\_STARTUP=true



### 1\.2 仍为 Mock / Stub / 占位

|位置|现状|影响|
|---|---|---|
|uenv\-math\-plugin|reset 写死固定数学题，答案恒为 "20"；不读 Episode payload|E2E 得 reward=1\.0 依赖 fixture 与 stub 对齐，非真实 GSM8K|
|ModelClient|若 reward\_config\.type=rule\_reward 且有 target，直接把 target 当 action，不调 LLM|联调捷径；VeRL 路径应改用 response\_text|
|RewardEngine|仅识别 rule\_reward；Bridge 来的 rubric\_config 未映射时 fallback 插件 step reward|与 Bridge payload 格式未完全打通|
|心跳 load|恒为 0（未上报真实活跃 Episode 数）|Server 调度看不到 Worker 负载|
|RegisterWorker\.resource|发送 None|ResourceSpec 未参与注册|
|StreamReport|主要填 phase；report\_type 等 PRD 扩展字段多为默认|流式进度语义不完整|
|Hub 集成|仅 HTTP 拉 manifest 元数据；不下载镜像/插件包|仍依赖本地 plugins/ \+ UENV\_MATH\_PLUGIN\_BIN|
|Episode 步数|仅 execute\_single\_round（1 step）|多轮 Agent 未实现|
|Podman 后端|代码存在，7143 使用 process|容器化插件未验收|
|registry/worker\_pool\.rs|占位注释|内存 Registry 未用于热路径|



### 1\.3 注意事项：本次 Worker 规模

本次全链路联调只拉起并注册了一个 Worker 进程（uenv\-worker 实例 1 个，Worker ID 5e96910f\-6dac\-4700\-bc58\-80de28cbb7a7，部署在 A100 7143 主机上）。Server 调度清单中仅一条 RegisterWorker 记录。

因此本次测试验证的是：单 Worker 上「Server → DispatchEpisode → 预热池 → math 插件 → ReportResult」链路可达；不能据此推断多 Worker 并行训练、跨节点负载均衡或 PRD §8\.5 大规模并行场景已验收。



## 测试内容与 Worker 内通信流程

### 2\.1 本次测试验证了什么

范围说明：以下均在 单 Worker 进程 前提下验证（7143 主机上仅 1 个 uenv\-worker）；Server 无第二候选 Worker，调度等价于「唯一进程接单」。见 §1\.3。

1. 7143 Worker 存活：/health 返回 ok，进程与日志正常

2. Hub 连通：启动时 hub\_manifest\_pulled（math 1\.0\.0）

3. Server 控制面：register \+ 持续 heartbeat（server\_epoch=1）

4. 全链路 Episode：Python → adapter\-core → Server 调度 → 唯一 Worker DispatchEpisode → 返回 reward=1\.0 → report\_result

### 2\.2 请求进入 Worker 后的完整链路

┌─────────────────────────────────────────┐

│  Server（Scheduler）主动 gRPC 调用       │

│  WorkerGrpcService\.DispatchEpisode      │

└──────────────────┬──────────────────────┘

│

▼

┌──────────────────────────────────────────────────────────────────┐

│ 1\. 准入：lease 校验 / 并发 Semaphore / 控制面连接策略              │

│ 2\. 预热池 acquire\(env\_type=math\) → 命中 math\-2 或 spawn 新实例    │

│ 3\. 插件 reset\(seed\) → UDS 调 uenv\-math\-plugin（返回 observation） │

│ 4\. ModelClient 得 action（rule\_reward 捷径或 HTTP LLM）            │

│ 5\. 插件 step\(action\) → reward / terminated                       │

│ 6\. RewardEngine 规则判分 → 最终 reward                             │

│ 7\. 预热池 release → 实例归还 Warm 队列                             │

│ 8\. 同步返回 StreamReport（step\_complete）                         │

│ 9\. 异步：WAL 持久化 → ControlPlane ReportResult → Server          │

└──────────────────────────────────────────────────────────────────┘



与 Server 的双通道关系：

- Server → Worker（数据面）：DispatchEpisode 下发任务，Worker 流式回 StreamReport

- Worker → Server（控制面）：Worker 主动 RegisterWorker / WorkerHeartbeat / ReportResult

Hub 不参与 Episode 热路径；仅在 Worker 启动或 spawn 前拉 manifest 做元数据对齐。



## 协议与接口结构

本节汇总 Worker 对外暴露与主动调用的全部接口及共享数据结构：与 Server 的 gRPC 数据面/控制面、Hub HTTP manifest、进程内 L2 插件 IPC，以及可观测性 HTTP 端点。

### 3\.1 Worker 为 Server 提供的 gRPC 接口（Worker 作 Server）

Proto：uenv\-worker/proto/worker\_service\.proto

Package：uenv\.worker\.v1

调用方向：UEnv Server / Scheduler 作为 Client，Worker 作为 Server

#### *3\.1\.1 Service：WorkerGrpcService*

|RPC|类型|说明|
|---|---|---|
|DispatchEpisode|Unary → Server stream|下发单个 Episode，执行中/完成后推送 StreamReport|
|HealthCheck|Unary|Worker 探活|



#### *DispatchEpisode*

Request：DispatchEpisodeRequest

|字段|类型|必填|说明|
|---|---|---|---|
|episode|uenv\.v1\.EpisodeRequest|是|完整 Episode 规格（见 §3\.3）|



Response：stream uenv\.v1\.StreamReport

Worker MVP 行为：执行完单轮后发送 一条 StreamReport（phase=step\_complete），然后关闭流；ReportResult 在后台异步上报。

#### *HealthCheck*

Request：HealthCheckRequest — 空消息

Response：HealthCheckResponse

|字段|类型|说明|
|---|---|---|
|ok|bool|恒 true（MVP）|
|status|string|如 "ok"|



HTTP 等价：GET http://\<worker\>:28777/health → 文本 ok



### 3\.2 Worker 作为 Client 连接 Server 的控制面

Proto：proto/uenv/v1/scheduler\.proto

Package：uenv\.scheduler\.v1

调用方向：Worker 作为 Client，Server / adapter\-core 内嵌 ControlPlaneService 作为 Server

#### *Service：ControlPlaneService*

|RPC|类型|Worker 是否实现 Client|
|---|---|---|
|RegisterWorker|Unary|√ 启动时一次|
|WorkerHeartbeat|Client stream → Server stream|√ 后台循环|
|ReportResult|Unary|√ 每个 Episode 完成后|
|ListWorkers|Unary|× Worker 不调用（Admin/Server 侧）|





#### *3\.2\.1 RegisterWorker*

Request：RegisterWorkerRequest

|字段|类型|必填|Worker 实填示例|
|---|---|---|---|
|worker\_id|string|是|配置 auto 则 Server 分配；实机 5e96910f\-\.\.\.|
|supported\_env\_types|repeated string|是|\["math"\]|
|resource|uenv\.v1\.ResourceSpec|否|MVP 发 None|
|endpoint|string|是|advertise\_endpoint，如 219\.147\.100\.43:28888|
|max\_concurrent|uint32|是|如 4|



Response：RegisterWorkerResponse

|字段|类型|说明|
|---|---|---|
|accepted|bool|是否接受注册|
|worker\_id|string|确认/分配的 Worker ID|
|message|string|人类可读信息|
|server\_epoch|uint64|Server 纪元，后续心跳/上报需携带|





#### *3\.2\.2 WorkerHeartbeat*

Request（Client stream）：HeartbeatRequest

|字段|类型|Worker MVP 行为|
|---|---|---|
|worker\_id|string|当前 Worker ID|
|load|int32|固定 0（待改进）|
|max\_load|int32|max\_concurrent|
|timestamp\_ms|int64|当前 Unix 毫秒|
|server\_epoch|uint64|本地缓存的 Server epoch|



Response（Server stream）：HeartbeatResponse

|字段|类型|说明|
|---|---|---|
|ok|bool|心跳是否接受|
|drain|DrainCommand|可选 drain 指令|
|server\_epoch|uint64|更新后的 epoch|
|next\_heartbeat\_interval\_ms|int32|建议下次心跳间隔|



DrainCommand

|字段|类型|说明|
|---|---|---|
|drain|bool|是否进入 drain|
|grace\_period\_sec|int32|优雅退出宽限秒数|



Worker MVP：每 \~5s 发一次心跳；日志 msg=heartbeat。



#### *3\.2\.3 ReportResult*

Request：ReportResultRequest

|字段|类型|说明|
|---|---|---|
|idempotency\_key|string|\{episode\_id\}:\{attempt\_id\}:\{worker\_id\}|
|worker\_id|string|Worker ID|
|server\_epoch|uint64|注册/心跳同步的 epoch|
|result|uenv\.v1\.EpisodeResult|完整结果（见 §3\.4）|



Response：ReportResultResponse

|字段|类型|说明|
|---|---|---|
|ack|bool|Server 是否确认|
|duplicate|bool|是否重复上报|



失败时写入 WAL，后台 spawn\_replay\_loop 重试。



### 3\.3 共享 Episode 数据结构（Server ↔ Worker）

Proto：proto/uenv/v1/episode\.proto、proto/uenv/v1/common\.proto

Package：uenv\.v1

#### *EpisodeRequest（Server 填入后经 DispatchEpisode 下发）*

|字段|类型|说明|
|---|---|---|
|episode\_id|string|Episode 唯一 ID|
|attempt\_id|uint32|重试序号，从 1 起|
|env\_type|string|Phase 0："math"|
|payload|bytes|环境配置 JSON（MVP 多为 env\_config 子集）|
|mode|ExecutionMode|如 MODE\_MULTI|
|max\_steps|int32|最大步数|
|resource\_spec|ResourceSpec|资源需求|
|model\_endpoint|string|模型回调 URL（可选）|
|seed|optional int32|随机种子|
|correlation\_id|string|全链路 trace，如 e2e\-chain\-smoke\-0|
|timeout\_seconds|int32|超时|
|reward\_config|bytes|判分配置 JSON|
|dispatch\_lease\_id|string|必填，调度租约 ID|
|lease\_expire\_at|google\.protobuf\.Timestamp|租约过期时间|
|scheduler\_epoch|uint64|调度器 epoch|
|dispatch\_token|bytes|可选 dispatch 令牌|



#### *ExecutionMode（enum）*

|值|名称|
|---|---|
|0|MODE\_UNSPECIFIED|
|1|MODE\_SINGLE|
|2|MODE\_MULTI|
|3|MODE\_MODEL\_CALLBACK|
|4|MODE\_CUSTOM|



#### *ResourceSpec*

|字段|类型|
|---|---|
|cpu\_cores|int32|
|memory\_mb|int32|
|gpu\_count|int32|
|gpu\_type|string|



#### *StepRecord*

|字段|类型|
|---|---|
|step\_index|int32|
|observation|bytes|
|action|bytes|
|reward|double|
|terminated|bool|
|truncated|bool|
|info|map\<string,string\>|
|duration\_ms|int64|



#### *Trajectory*

|字段|类型|
|---|---|
|steps|repeated StepRecord|
|total\_reward|double|
|total\_steps|int32|



#### *EpisodeResult（Worker 经 ReportResult 上报）*

|字段|类型|说明|
|---|---|---|
|episode\_id|string|与 Request 一致|
|attempt\_id|uint32|与 Request 一致|
|status|string|"completed" / "failed" / "timeout"|
|trajectory|Trajectory|完整轨迹|
|summary|Summary|汇总|
|error\_code|optional ErrorCode|失败时|
|error\_message|string|错误描述|
|trajectory\_checksum|string|SHA256\(hex\)|
|integrity\_verified|bool|MVP 为 true|



EpisodeResult\.Summary

|字段|类型||
|---|---|---|
|total\_reward|double||
|total\_steps|int32||
|total\_duration\_ms|int64||
|terminate\_reason|string|MVP：single\_round\_done|



#### *StreamReport（DispatchEpisode 流式响应）*

|字段|类型|MVP 填充情况|
|---|---|---|
|episode\_id|string|√|
|attempt\_id|uint32|√|
|current\_step|int32|√（单轮为 1）|
|total\_steps|int32|√|
|current\_reward|double|√|
|phase|string|√ step\_complete|
|last\_step|optional StepRecord|√|
|report\_type|ReportType enum|× 默认 UNSPECIFIED|
|step\_latency\_ms|int64|未填|
|model\_latency\_ms|int64|未填|
|estimated\_remaining\_seconds|double|未填|
|worker\_active\_episodes|int32|未填|
|worker\_capacity|int32|未填|
|correlation\_id|string|未填|
|worker\_id|string|未填|



ReportType enum：UNSPECIFIED \| PROGRESS \| STEP\_COMPLETE \| REWARD\_SIGNAL \| LOG \| PACING

#### *ErrorCode（enum，节选）*

|值|名称|场景|
|---|---|---|
|1001|ERR\_INVALID\_REQUEST|请求非法|
|1002|ERR\_UNKNOWN\_ENV\_TYPE|不支持 env\_type|
|2001|ERR\_NO\_AVAILABLE\_WORKER|Server 侧|
|3002|ERR\_ENV\_INIT\_FAILED|插件 reset 失败|
|3003|ERR\_ENV\_STEP\_FAILED|插件 step 失败|
|3004|ERR\_MODEL\_CALL\_FAILED|ModelClient 失败|
|3007|ERR\_LEASE\_EXPIRED|租约过期|





### 3\.4 WAL 记录结构（Worker 内部，供 Server 重放语义）

Proto：proto/uenv/v1/wal\.proto

|字段|类型|说明|
|---|---|---|
|episode\_id|string||
|attempt\_id|uint32||
|worker\_id|string||
|dispatch\_lease\_id|string||
|server\_epoch|uint64||
|request\_checksum|string||
|result\_checksum|string||
|status|string||
|protobuf\_payload|bytes|序列化 EpisodeResult|
|created\_at|Timestamp||
|replay\_state|ReplayState|PENDING / SENT / ACKED|



幂等键：idempotency\_key = episode\_id \+ attempt\_id \+ worker\_id



### 3\.5 Worker 与 Hub 的 HTTP 接口（Worker 作 Client）

Worker 仅消费 Hub Registry 的只读 manifest API；不调用 Publish/Admin。

权威文档：uenv\-hub/docs/api\.md

#### *3\.5\.1 Worker 实际调用的接口*

#### *GET /api/v1/envs/\{env\_type\}/versions/latest*

|项|值|
|---|---|
|方法|GET|
|路径参数|env\_type — 如 math|
|认证|Authorization: Bearer \<UENV\_HUB\_TOKEN\>（reader 角色）|
|超时|10s（Worker 硬编码）|



Worker 解析的 JSON 子集（HubEnvManifest）

|字段|类型|必填|说明|
|---|---|---|---|
|env\_type|string|是|须与请求路径一致|
|version|string|是|如 1\.0\.0|
|entrypoint|string|否|Hub 元数据；Worker 优先本地 plugins/\{env\_type\}/manifest\.yaml 的 \./run\.sh|
|supported\_backends|string\[\]|否|默认 \["process"\]|



Hub 返回的完整 FullManifest 还包含（Worker 当前忽略，不下载）：

|字段|说明|
|---|---|
|changelog|变更说明|
|dependencies|Python 依赖等|
|min\_uenv\_version|最低 UEnv 版本|
|base\_image / image|OCI 镜像 URL/digest|
|health\_check\_path|容器健康检查路径|
|interface|action/observation/state JSON Schema|
|examples|示例请求|
|config\_schema / default\_config|环境配置 Schema|
|resources|CPU/内存/GPU|
|is\_yanked / published\_at|发布元数据|



成功响应示例（Hub 完整体，节选）

\{

"env\_type": "math",

"version": "1\.0\.0",

"entrypoint": "uenv\-worker math",

"supported\_backends": \["process", "podman"\],

"interface": \{

"action": \{ "type": "object", "properties": \{ "answer": \{ "type": "string" \} \} \},

"observation": \{ "type": "object", "properties": \{ "question": \{ "type": "string" \} \} \}

\},

"resources": \{ "cpu": 2\.0, "memory\_mb": 4096, "gpu": 0 \}

\}



Worker 处理逻辑

5. 启动时 sync\_env\_types\_from\_hub 对每个 env\.types pull

6. EnvResolver\.apply\_hub\_summary 合并版本/backend 信息

7. spawn 前 ensure\_env\_ready：本地 plugins/math/ 必须存在

8. 不拉取 image\.url 或替换二进制

失败降级：Hub 不可达时 hub\_pull\_failed\_using\_local\_manifest，继续用本地插件。



### 3\.6 Worker 内部 L2 插件 IPC（Execution 必读）

Proto：plugin\_proto/uenv/plugin/v1/plugin\.proto

传输：Protobuf over Unix Domain Socket（仅 Worker 进程内）

#### *Service：PluginService*

|RPC|Request|Response 要点|
|---|---|---|
|Reset|optional int32 seed|observation bytes, info map|
|Step|action bytes|observation, reward, terminated, truncated, info|
|Close|空|ok|
|HealthCheck|空|ok, message|



math 插件启动：plugins/math/run\.sh → exec $UENV\_MATH\_PLUGIN\_BIN \-\-uds\-path \<path\>



### 3\.7 Worker 可观测性端点（非 gRPC）

|端点|端口（7143）|说明|
|---|---|---|
|GET /health|28777|文本 ok|
|GET /metrics|28777|Prometheus 文本格式|



主要指标名：uenv\_episode\_total、uenv\_episode\_duration\_ms\_sum、uenv\_warmup\_pool\_hit\_total、uenv\_warmup\_pool\_miss\_total、uenv\_active\_episode\_count、uenv\_wal\_pending\_records、uenv\_instance\_pool\_size\_\*

## 第四部分：UENV Hub（锐昕）

UEnv\-Hub 是 UEnv 体系中的 L1 环境元数据注册中心，定位类比 Docker Hub / npm / Hugging Face Hub。它是一个离线目录服务，不参与运行时调度，只负责持久化保存环境的元数据、版本、镜像引用、资源需求与接口 Schema。本次工作完成了它的正式部署、公网对外开放、Token 鉴权加固、命令行工具落地，并对全部接口做了实测与安全审计。下面按板块说明各部分本次实现与测试的主要内容，以及各部分对外提供的接口与数据结构。



**一、整体架构与交付物**

本次交付的 UEnv\-Hub 由四个 Rust crate 组成，分层清晰：

· uenv\-hub\-types：各端共享的 API 数据结构（DTO），是服务端、客户端、CLI 的统一契约。

· uenv\-hub\-core：数据与领域层，负责模型定义、SQLite 仓储、版本/manifest/接口校验、种子数据与模板。

· uenv\-hub\-server：基于 axum 的 HTTP 服务，负责路由、鉴权与 RBAC、服务编排、错误处理、可观测性、限流与 CORS。

· uenv\-hub\-client：客户端 SDK（HTTP \+ 重试 \+ ETag 缓存）以及 uenv 命令行工具。

调用链路：uenv CLI 或 Worker 通过 HTTP 调用 uenv\-hub\-server，服务端经 uenv\-hub\-core 读写 SQLite（WAL 模式）。

本次新增/产出的文件：

· config/hub\.prod\.toml：生产/联调配置（端口 8088、开启鉴权、SQLite 持久化）。

· scripts/start\-hub\.sh：启动脚本，首次启动时从密钥文件注入共享 Token。

· docs/uenv\-hub\-service\-integration\.md：HTTP 接口对接文档。

· docs/uenv\-cli\-guide\.md：CLI 操作文档。

· docs/uenv\-hub\-feishu\-summary\.md：本说明文档。





**二、部署与运行**

本次实现内容：

· 在服务器上安装了 Rust 1\.96 工具链（因 crate 使用 edition 2024，需较新版本，已用国内镜像完成），编译出两个发布二进制：uenv\-hub\-server（服务端）与 uenv（CLI）。

· 服务以 nohup 后台方式常驻运行，绑定 0\.0\.0\.0:8088，对外通过公网 EIP 可达。

· 数据持久化在 SQLite（WAL）文件 data/hub\.db，重启不丢数据。



对外访问信息：

· 协议：HTTP REST，请求体与响应体均为 application/json。

· 公网 Base URL（其他服务器对接用）：http://8\.130\.95\.176:8088

· 同 VPC 内网 Base URL（可选）：http://192\.168\.0\.133:8088

· 监听地址：0\.0\.0\.0:8088（绑定全部网卡，外部可达）。

· 端口说明：四机联调文档里 Hub 默认写的是 8080，但本机只开放 8000 / 8077 / 8088 / 8099，因此实际使用 8088，对接时请将文档中的 8080 替换为 8088。



服务管理常用操作：

· 启动：在 uenv\-hub 目录执行 nohup \./scripts/start\-hub\.sh 大于号 logs/uenv\-hub\.log 2 大于 1 后台运行。

· 看日志：tail \-f logs/uenv\-hub\.log，首次启动会打印 bootstrapped admin token from config。

· 查进程：ss \-tlnp 过滤 8088 拿到 PID，再用 kill 停止（不要用会误杀自身的 pkill 模式匹配）。

· 备份：bash scripts/backup\.sh（VACUUM INTO 一致性备份）。



测试内容：已确认服务监听 0\.0\.0\.0:8088、公网 EIP 探活 /healthz 返回 status=ok、db=up，进程稳定，日志无 error/warn。





**三、鉴权模块（单一共享 Token）**

本次实现内容：服务已公网暴露，为阻断匿名越权，开启了 Token 鉴权（require\_token=true）。为尽量简化，采用单一共享 Bearer Token 方案：所有层（Worker / Server / CLI / 运维）使用同一个 token，无需按角色分发多个 token。该方案零代码改动，复用了 Hub 内置鉴权能力，对现有功能无影响。



规则说明：

· 公开端点不需要 token：GET /healthz、GET /metrics、GET /version（探活、监控、版本照常可用）。

· /api/v1 下的全部接口（读和写）都需要 token；无 token 或 token 无效返回 401 UNAUTHORIZED。

· token 携带方式二选一：请求头 Authorization: Bearer uenvh\_xxx，或请求头 X\-Api\-Token: uenvh\_xxx。



Token 管理：

· token 保存在 Hub 主机的 data/\.admin\_token 文件（权限 600），运维向各层下发同一字符串即可。

· token 不写入仓库内的配置文件，而是通过环境变量在首次启动时注入，创建后已持久化进 SQLite，后续重启即使不带该变量也仍生效。

· 角色分级（本次统一用 admin）：reader 只读，publisher 可创建/发布/下架，admin 可删除/管理 Token/查审计。共享 token 为 admin，覆盖全部操作。

· token 轮换：用当前 token 调 POST /api/v1/admin/tokens 创建新 token（明文仅返回一次），再用 DELETE /api/v1/admin/tokens/\{id\} 吊销旧 token。



测试内容：实测匿名访问 /api/v1 返回 401；带 token（Bearer 与 X\-Api\-Token 两种方式）均返回 200；公开探活端点不受鉴权影响仍为 200。审计中还实测确认了「关闭鉴权时匿名会被当作 admin」这一风险，故最终选择开启鉴权。





**四、HTTP REST 接口模块**

服务端共提供 23 个接口，按角色与用途分组如下。

公开类（无需 token）：

· GET /healthz：探活，含 DB 状态。

· GET /metrics：Prometheus 文本指标。

· GET /version：版本信息。



只读查询类（需 reader 及以上）：

· GET /api/v1/envs：环境列表，支持分页与过滤；带 since 参数时切换为增量同步（供 Server 使用）。

· GET /api/v1/envs/\{env\_type\}：环境详情。

· GET /api/v1/envs/\{env\_type\}/versions：版本列表。

· GET /api/v1/envs/\{env\_type\}/versions/\{version\}：指定版本 manifest。

· GET /api/v1/envs/\{env\_type\}/versions/latest：取最新版本 manifest，是 Worker 的主路径。

· GET /api/v1/envs/\{env\_type\}/resolve?constraint=：语义化版本解析，如 ^1\.0、1\.0\.0。

· GET /api/v1/envs/\{env\_type\}/versions/\{version\}/interface：取该版本的 interface JSON Schema。

· GET /api/v1/envs/\{env\_type\}/versions/\{version\}/examples：取示例。

· GET /api/v1/search?q=：按关键词/标签/作者搜索。

· GET /api/v1/templates：模板列表。

· GET /api/v1/templates/\{name\}/archive：下载模板 gzip 包。



写入类（需 publisher 及以上）：

· POST /api/v1/envs：创建环境。

· POST /api/v1/envs/\{env\_type\}/versions：发布版本。

· PATCH /api/v1/envs/\{env\_type\}：更新环境元数据。

· POST /api/v1/envs/\{env\_type\}/versions/\{version\}/yank：下架版本。



管理类（需 admin）：

· DELETE /api/v1/envs/\{env\_type\}：删除环境。

· POST /api/v1/admin/tokens：创建 Token。

· DELETE /api/v1/admin/tokens/\{id\}：吊销 Token。

· GET /api/v1/admin/audit\-log：查审计日志。



常用查询参数：

· GET /api/v1/envs 支持 page、per\_page、namespace、author、tag、since。

· resolve 接口支持 constraint，如 ^1\.0、1\.0\.0。

· search 接口支持 q、tag、author、namespace、page、per\_page。



测试内容：上述只读、写入、管理三类接口均已带 token 实测通过（列表、详情、版本、latest、interface、resolve、search、templates 返回 200；发布版本返回 201，下架返回 204，删除返回 204）。





**五、数据结构（请求与响应 DTO）**

健康与版本：

· GET /healthz 返回 status、db 两个字段，例如 status=ok、db=up。

· GET /version 返回 name、version、git\_sha 三个字段，例如 name=uenv\-hub、version=0\.1\.0、git\_sha 为空。



环境概要 EnvSummary（列表项）：env\_type、namespace、description（可空）、author（可空）、latest\_version（可空）、tags（字符串数组）、created\_at、updated\_at（均为 Unix 秒）。



环境详情 EnvDetail：在 EnvSummary 基础上增加 homepage、repository、license（均可空）以及 latest\_manifest（可空）。



完整 manifest（FullManifest，是 latest 接口的响应体，也是 Worker 消费的核心结构）：

· env\_type：字符串，环境类型，是调度键。

· version：字符串，版本号。

· changelog：可空字符串，版本说明。

· entrypoint：可空字符串，启动入口（参考用，Worker spawn 时优先本地 run\.sh）。

· supported\_backends：字符串数组，如 process、podman。

· dependencies：可空对象，依赖信息（requirements\_path、install\_script、requires）。

· min\_uenv\_version：可空字符串。

· base\_image：可空字符串。

· health\_check\_path：可空字符串。

· image：可空对象 ImageSpec，含 url、digest、size\_bytes、arch、base\_image\_ref。

· config\_schema：可空 JSON Schema，对 payload 的约束。

· default\_config：可空 JSON，默认配置。

· resources：资源需求对象，含 cpu、memory\_mb、gpu、gpu\_type、disk\_mb。

· interface：接口定义对象，含 action、observation、state，三者均为 JSON Schema。

· examples：示例数组。

· is\_yanked：布尔，是否下架；yank\_reason 为可空字符串，下架原因。

· published\_at：Unix 秒，发布时间。



版本概要 VersionSummary（版本列表项）：version、changelog（可空）、is\_yanked、published\_at。



搜索响应 SearchResponse：results（EnvSummary 数组）、total、page、per\_page。



发布请求 PublishVersionRequest：version、changelog、image、base\_image、health\_check\_path、entrypoint、supported\_backends、config\_schema、default\_config、resources、interface、examples、dependencies、min\_uenv\_version。



发布响应 PublishVersionResponse：env\_type、version、published\_at、manifest\_url。



创建环境请求 CreateEnvRequest：env\_type、namespace（可空）、description（可空）、author（可空）、homepage（可空）、repository（可空）、license（可空）、tags（字符串数组）。





**六、Worker 对接（核心热路径）**

本次约定与实现：Worker 在启动或 spawn 插件前，从 Hub 拉取对应环境的 manifest；若拉取失败可降级使用本地 plugins/环境名/manifest\.yaml，不会阻塞 Episode 热路径。



Worker 侧需要的环境变量：

· UENV\_HUB\_ENDPOINT 设为 http://8\.130\.95\.176:8088

· UENV\_HUB\_ENABLED 设为 true

· UENV\_HUB\_TOKEN 设为共享 token（与 data/\.admin\_token 内容一致）



主路径请求：带上 Authorization: Bearer 头，GET http://8\.130\.95\.176:8088/api/v1/envs/math/versions/latest，返回上面第五节描述的 FullManifest。Worker 主要消费其中的 env\_type、version、supported\_backends、image、config\_schema、default\_config、resources、interface 等字段。拉取失败时 Worker 记录 hub\_pull\_failed\_using\_local\_manifest 并改用本地 manifest。



与全链路的关系：Hub 不参与 Episode 运行时调度，也不下载镜像，Worker 仍需本地插件目录；Server 运行时不依赖 Hub 的 HTTP，Hub 仅在 Worker 启动/spawn 前提供 manifest。





**七、初始数据与模板**

seed 是服务首次启动时自动写入数据库的一批初始（种子）数据，让 Hub 开箱即用，且具备幂等性——环境只在不存在时创建，重启不会覆盖后续发布的改动；模板每次启动做 upsert 以随版本更新。



本次预置的环境：

· math：最新版本 1\.0\.0，标签 math、reasoning，含完整 interface、config schema 与 image。

· code：最新版本 1\.0\.0，标签 code、execution，代码执行奖励环境。

· agent：最新版本 0\.1\.0，标签 agent、multi\-turn，多轮工具调用环境。



本次预置的脚手架模板：math、code、agent、echo，供 CLI 的 uenv env init 使用。



说明：这三个环境是开箱演示数据，实际联调/生产中的真实环境应由各业务方通过发布接口或 CLI 自行发布。





**八、错误处理**

所有非 2xx 响应使用统一信封，包含 error 对象（其中 code 为稳定机读标识、message 为可读信息、details 为可选结构化上下文）以及 request\_id；同时响应头带 x\-request\-id，便于和服务端日志对账。对接方应以 error\.code 为准进行判断，message 文案可能变化。



主要错误码与 HTTP 状态对应：

· UNAUTHORIZED 对应 401，token 缺失或无效。

· FORBIDDEN 对应 403，角色或命名空间不允许。

· NOT\_FOUND 对应 404，环境/版本/Token 不存在。

· VERSION\_ALREADY\_EXISTS 对应 409，版本已发布且不可覆盖。

· ENV\_ALREADY\_EXISTS 对应 409，环境已存在。

· CONFLICT 对应 409，其他唯一性或状态冲突。

· INVALID\_MANIFEST 对应 422，manifest 结构非法。

· INVALID\_VERSION 对应 422，非合法 semver。

· INVALID\_CONSTRAINT 对应 422，版本约束无法解析。

· SCHEMA\_VALIDATION\_FAILED 对应 422，config 或 interface 的 JSON Schema 校验失败。

· RATE\_LIMITED 对应 429，超过限流。

· INTERNAL\_ERROR 对应 500，内部错误。





**九、命令行工具（CLI）**

本次实现内容：编译出 uenv 命令行二进制，分 env 和 hub 两组子命令，通过 HTTP 调用 Hub，鉴权方式与服务端一致（Authorization: Bearer）。配置文件位于 \~/\.config/uenv/hub\.toml，支持命令行参数、环境变量、配置文件三级覆盖。



env 组子命令：

· list：列出已注册环境。

· info 环境名：显示环境详情（JSON）。

· versions 环境名：列出版本，下架版本会标记 yanked。

· search 关键词，可加 \-\-tag、\-\-author：搜索环境。

· init 名称，可加 \-\-template、\-\-dir：用模板脚手架新建项目（纯本地）。

· validate，可加 \-\-manifest：本地校验 manifest 与 schema（纯本地，不连网、不需 token）。

· build，可加 \-\-manifest、\-\-engine：构建容器镜像（需 docker 或 podman）。

· push：构建并推送镜像后发布 manifest（需引擎）。

· publish，可加 \-\-manifest：镜像已在仓库时仅发布元数据。

· yank 环境名 \-\-version \-\-reason：下架版本。



hub 组子命令：

· login \-\-token，可加 \-\-endpoint：保存凭据到配置文件。

· status：显示 endpoint 与连通状态。

· sync，可加 \-\-since、\-\-dry\-run：增量同步元数据。

· config set 键 值 / config show：设置或查看配置（键为 endpoint 或 token）。



测试内容：list、info、versions、search、status、config show、sync、login、init、validate、publish、yank 等命令均实测通过；无 token 调用正确返回 Unauthorized。build 与 push 逻辑正确，但依赖容器引擎，本机当前未安装 docker/podman，若镜像已在仓库直接用 publish 即可，无需引擎。





**十、本次测试与安全审计小结**

· 接口层面：公开端点、只读、写入、管理四类接口全部带 token 实测通过（读 200、发布 201、下架/删除 204、匿名 401）。

· 安全层面：发现并修复了「公网暴露且免鉴权时匿名被当作 admin」的高危问题，已开启共享 Token 鉴权，匿名访问 /api/v1 被正确拦截为 401；密钥文件以 600 权限保存，且不进入仓库配置。

· 数据层面：审计过程中产生的测试环境已清理，数据库已重置回纯净 seed（agent、code、math），latest 主路径不受下架版本影响。

· 当前状态：服务在 http://8\.130\.95\.176:8088 正常运行，鉴权生效，数据纯净，CLI 与服务端均已编译可用。

· 已知约束：CLI 的 build/push 需要本机安装 docker 或 podman，目前未安装，不影响其余功能。



