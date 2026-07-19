#!/usr/bin/env python3
"""压测样本和结果格式的公共工具。

这个文件解决一个问题：不同压测脚本不要各自手写 episode payload。
如果 Code 任务、SWE/OpenHands 任务或结果 JSON 字段以后要改，只需要改这里。

本文件故意不依赖 paramiko、grpc 等运行时库。分布式压测会把它复制到
/tmp/uenv-<run_id>/，让临时生成的 load_client.py / smoke_client.py 也能导入。
"""

from __future__ import annotations

import json
import math
import time
import uuid
from typing import Iterable


# Code 压测使用一个非常小的 Python 题。题目小是为了把主要压力放在
# server/worker 调度链路上，而不是放在复杂题目本身。
CODE_ENTRY_POINT = "add"
CODE_DATASET = "dscodebench"
CODE_QUESTION = "Write a Python function add(a, b) that returns a + b."
CODE_TEST_CODE = (
    "assert add(1, 2) == 3\n"
    "assert add(-5, 2) == -3\n"
    "assert add(0, 0) == 0"
)

# SWE/OpenHands 任务需要通过 agent bridge 把 server 分配的 episode 交给
# OpenHands agent 执行。下面两个字段用于标识这条 agent 通道。
SWE_AGENT_BRIDGE_ID = "uenv-agent-openhands"
SWE_AGENT_BRIDGE_VERSION = "1.0.0"


def json_bytes(value: dict) -> bytes:
    """把 dict 编成 proto 字段需要的紧凑 JSON bytes。"""
    return json.dumps(value, separators=(",", ":")).encode()


def percentile(values: Iterable[float], q: float) -> float:
    """计算延迟分位数。

    q=0.50 表示 p50，q=0.95 表示 p95，q=0.99 表示 p99。
    没有样本时返回 0，避免结果生成阶段再抛异常。
    """
    ordered = sorted(values)
    if not ordered:
        return 0.0
    index = min(len(ordered) - 1, max(0, math.ceil(len(ordered) * q) - 1))
    return ordered[index]


def code_env_payload(task_id: str) -> dict:
    """生成 Code episode 的 env_config_json 内容。

    worker 收到这个 payload 后，会调用 code plugin，并用 test_code 验证
    模型返回的 Python 代码是否实现了 add(a, b)。
    """
    return {
        "question": CODE_QUESTION,
        "dataset": CODE_DATASET,
        "task_id": task_id,
        "library": "python",
        "test_code": CODE_TEST_CODE,
        "entry_point": CODE_ENTRY_POINT,
        "num_tests": 3,
    }


def code_reward_config() -> dict:
    """生成 Code 任务的 reward_config_json 内容。"""
    return {"type": "code_tests", "entry_point": CODE_ENTRY_POINT}


def swe_openhands_env_payload(
    *,
    instance_id: str,
    benchmark_variant: str,
    command_mode: str,
    mode: str,
    agent_pool_id: str,
    driver_entrypoint: str,
    workspace_dir: str,
    max_iterations: int,
    llm_config_path: str = "",
    instances_catalog: str = "",
    pool_selector: str | None = None,
) -> dict:
    """生成 SWE/OpenHands episode 的 env_config_json 内容。

    这些字段会告诉 SWE worker：
    - 使用哪个 SWEBench instance；
    - 使用哪个 OpenHands agent pool；
    - official driver 从哪里加载；
    - OpenHands 的工作目录和最大迭代次数是多少。
    """
    payload = {
        "execution_mode": "agent",
        "instance_id": instance_id,
        "benchmark_variant": benchmark_variant,
        "command_mode": command_mode,
        "mode": mode,
        "agent_bridge_id": SWE_AGENT_BRIDGE_ID,
        "agent_bridge_version": SWE_AGENT_BRIDGE_VERSION,
        "agent_pool_id": agent_pool_id,
        "driver_entrypoint": driver_entrypoint,
        "workspace_dir": workspace_dir,
        "llm_config_path": llm_config_path,
        "max_iterations": max_iterations,
        "pool_selector": pool_selector or agent_pool_id,
    }
    if instances_catalog:
        payload["instances_catalog"] = instances_catalog
    return payload


def swe_reward_config() -> dict:
    """生成 SWE/OpenHands 任务的 reward_config_json 内容。"""
    return {"type": "swe_bench"}


def rule_reward_config(target: str) -> dict:
    """生成数学/规则类任务的 reward_config_json 内容。"""
    return {"type": "rule_reward", "target": target}


def make_sample_envelope(
    adapter_core_pb2,
    *,
    batch_id: str,
    sample_index: int,
    env_type: str,
    parallel_mode: str,
    env_config: dict,
    reward_config: dict,
    sample_context: dict,
    timeout_seconds: int,
    max_steps: int = 1,
    model_url: str = "",
    model_name: str = "",
):
    """构造 AdapterCore ExecuteBatch 接口需要的 SampleEnvelope。

    SampleEnvelope 是 server 接收 episode 的统一外壳。这里统一设置：
    - request_id: 单个样本的唯一 ID；
    - batch_id/sample_index: 批次和样本序号；
    - env_type: code、swe、math 等环境类型；
    - parallel_mode: sync、one_step_off_policy、fully_async；
    - env_config_json/reward_config_json: 环境和奖励配置；
    - episode_config_json.max_steps: 单个 episode 最多允许执行多少个 step；
    - model_endpoint: Code/Math 任务需要调用模型时才设置。
    """
    if max_steps <= 0:
        raise ValueError(f"max_steps must be positive, got {max_steps}")
    fields = {
        "request_id": str(uuid.uuid4()),
        "batch_id": batch_id,
        "sample_index": sample_index,
        "framework": "verl",
        "env_type": env_type,
        "parallel_mode": parallel_mode,
        "env_config_json": json_bytes(env_config),
        "episode_config_json": json_bytes({"max_steps": max_steps}),
        "reward_config_json": json_bytes(reward_config),
        "sample_context_json": json_bytes(sample_context),
        "timeout_seconds": timeout_seconds,
    }
    if model_url:
        fields["model_endpoint"] = adapter_core_pb2.ModelEndpoint(
            endpoint_type="http", url=model_url, model_name=model_name
        )
    return adapter_core_pb2.SampleEnvelope(**fields)


def sample_result_dict(result) -> dict:
    """把 proto result 转成普通 dict，方便写入 JSON 文件。"""
    return {
        "sample_index": result.sample_index,
        "status": result.status,
        "reward": result.reward,
        "error_code": result.error_code,
        "error_message": result.error_message,
        "trajectory_id": getattr(result, "trajectory_id", ""),
    }


def gate3_result_document(
    *,
    run_id: str,
    mode: str,
    configured_workers: int,
    worker_capacity: int,
    elapsed_seconds: float,
    submitted: int,
    completed: int,
    failed: int,
    rpc_error_episodes: int,
    protocol_errors: int,
    latencies_ms: Iterable[float],
    rewards: Iterable[float],
) -> dict:
    """生成 Gate3 Code 扩容压测的结果 JSON。

    Gate3 关心的是真实 Code worker 在不同 worker/slot 规模下的吞吐、
    延迟和协议错误数量，所以这里输出 completed、failed、throughput_eps、
    batch_latency_ms、protocol_errors 等字段。
    """
    rewards = list(rewards)
    resolved = completed + failed + rpc_error_episodes
    return {
        "schema_version": 1,
        "run_id": run_id,
        "mode": mode,
        "configured_workers": configured_workers,
        "registered_workers": configured_workers,
        "worker_capacity": worker_capacity,
        "worker_slots": configured_workers * worker_capacity,
        "batch_size": configured_workers,
        "concurrent_batches": worker_capacity,
        "requested_episode_concurrency": configured_workers * worker_capacity,
        "elapsed_seconds": elapsed_seconds,
        "submitted": submitted,
        "completed": completed,
        "failed": failed,
        "rpc_error_episodes": rpc_error_episodes,
        "protocol_errors": protocol_errors,
        "completion_rate": completed / submitted if submitted else 0.0,
        "throughput_eps": resolved / elapsed_seconds if elapsed_seconds > 0 else 0.0,
        "batch_latency_ms": {
            "p50": percentile(latencies_ms, 0.50),
            "p95": percentile(latencies_ms, 0.95),
            "p99": percentile(latencies_ms, 0.99),
        },
        "average_reward": sum(rewards) / len(rewards) if rewards else 0.0,
    }


def gate4_swe_result_document(
    *,
    run_id: str,
    server: str,
    worker_id: str,
    registered_workers: list[str],
    instance_id: str,
    mode: str,
    concurrency: int,
    max_steps: int,
    openhands_max_iterations: int,
    elapsed_seconds: float,
    results: list[dict],
) -> dict:
    """生成 Gate4 SWE/OpenHands 容器压测的结果 JSON。"""
    average_reward = sum(item["reward"] for item in results) / len(results) if results else 0.0
    return {
        "schema_version": 1,
        "run_id": run_id,
        "server": server,
        "worker_id": worker_id,
        "registered_workers": registered_workers,
        "instance_id": instance_id,
        "mode": mode,
        "concurrency": concurrency,
        "max_steps": max_steps,
        "openhands_max_iterations": openhands_max_iterations,
        "elapsed_seconds": elapsed_seconds,
        "status": (
            "completed"
            if results and all(item["status"] in {"completed", "success"} for item in results)
            else "failed"
        ),
        "average_reward": average_reward,
        "results": results,
    }


def stress_result_document(
    *,
    run_id: str,
    environment: str,
    parallel_mode: str,
    elapsed_seconds: float,
    registered_workers: int,
    configured_workers: int,
    worker_capacity: int,
    batch_size: int,
    concurrent_batches: int,
    openhands_agents: int,
    openhands_agent_capacity: int,
    submitted: int,
    completed: int,
    failed: int,
    rpc_error_episodes: int,
    rpc_error_batches: int,
    protocol_errors: int,
    latencies_ms: Iterable[float],
    rewards: Iterable[float],
) -> dict:
    """生成 stress_test_real.py 单机 runner 的结果 JSON。

    单机 runner 支持 math/code/swe_openhands，所以字段比 Gate3/Gate4 更通用。
    openhands_agent_slots 只有 SWE/OpenHands 场景才有实际意义。
    """
    rewards = list(rewards)
    resolved = completed + failed + rpc_error_episodes
    return {
        "schema_version": 1,
        "run_id": run_id,
        "environment": environment,
        "parallel_mode": parallel_mode,
        "elapsed_seconds": elapsed_seconds,
        "registered_workers": registered_workers,
        "configured_workers": configured_workers,
        "worker_slots": registered_workers * worker_capacity,
        "openhands_agent_slots": openhands_agents * openhands_agent_capacity,
        "requested_episode_concurrency": batch_size * concurrent_batches,
        "submitted": submitted,
        "completed": completed,
        "failed": failed,
        "rpc_error_episodes": rpc_error_episodes,
        "rpc_error_batches": rpc_error_batches,
        "protocol_errors": protocol_errors,
        "resolved_throughput_eps": resolved / elapsed_seconds if elapsed_seconds > 0 else 0.0,
        "batch_latency_ms": {
            "p50": percentile(latencies_ms, 0.50),
            "p95": percentile(latencies_ms, 0.95),
            "p99": percentile(latencies_ms, 0.99),
        },
        "average_reward": sum(rewards) / len(rewards) if rewards else 0.0,
        "completed_at": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
    }
