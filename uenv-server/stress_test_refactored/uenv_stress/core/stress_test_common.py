#!/usr/bin/env python3
"""压测样本和结果格式的公共工具。

这个文件解决一个问题：不同压测脚本不要各自手写 episode payload。
如果 Code 任务、SWE/OpenHands 任务或结果 JSON 字段以后要改，只需要改这里。

本文件故意不依赖 paramiko、grpc 等运行时库。分布式压测会把它复制到
/tmp/uenv-<run_id>/，让临时生成的 load_client.py / smoke_client.py 也能导入。
"""

from __future__ import annotations

import json
import hashlib
import math
from pathlib import Path
import time
import uuid
from typing import Any, Iterable


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


# Both the scale suite and the formal stability suite persist exactly this flat
# row.  It is deliberately friendly to JSONL, CSV and SQLite: scenario-specific
# values belong in extensions_json instead of changing the table per workload.
EPISODE_OBSERVATION_SCHEMA_VERSION = 1
EPISODE_OBSERVATION_FIELDS = (
    "schema_version",
    "observation_type",
    "suite",
    "run_id",
    "phase",
    "task",
    "dataset",
    "environment",
    "parallel_mode",
    "arrival_mode",
    "episode_id",
    "request_id",
    "batch_id",
    "sample_index",
    "sequence",
    "dataset_item_id",
    "instance_id",
    "planned_at",
    "dispatch_started",
    "dispatched_at",
    "deadline",
    "terminal_at",
    "timeout_seconds",
    "end_to_end_ms",
    "batch_rpc_latency_ms",
    "status",
    "done",
    "reward",
    "termination_reason",
    "error_code",
    "error_message",
    "failure_class",
    "terminal_count",
    "worker_id",
    "worker_host",
    "dispatch_lease_id",
    "attempt_id",
    "worker_attribution",
    "replay_strategy",
    "trace_id",
    "trace_slot",
    "trace_corpus_size",
    "actual_steps",
    "response_tokens",
    "training_trace_valid",
    "training_trace_errors_json",
    "rollout_param_version",
    "rollout_policy_version",
    "result_checksum",
    "result_checksum_valid",
    "extensions_json",
)

_EPISODE_OBSERVATION_DEFAULTS = {
    "schema_version": EPISODE_OBSERVATION_SCHEMA_VERSION,
    "observation_type": "episode",
    "suite": "",
    "run_id": "",
    "phase": "",
    "task": "",
    "dataset": "",
    "environment": "",
    "parallel_mode": "",
    "arrival_mode": "",
    "episode_id": "",
    "request_id": "",
    "batch_id": "",
    "sample_index": 0,
    "sequence": -1,
    "dataset_item_id": "",
    "instance_id": "",
    "planned_at": 0.0,
    "dispatch_started": False,
    "dispatched_at": 0.0,
    "deadline": 0.0,
    "terminal_at": 0.0,
    "timeout_seconds": 0.0,
    "end_to_end_ms": 0.0,
    "batch_rpc_latency_ms": 0.0,
    "status": "planned",
    "done": False,
    "reward": 0.0,
    "termination_reason": "",
    "error_code": "",
    "error_message": "",
    "failure_class": "pending",
    "terminal_count": 0,
    "worker_id": "",
    "worker_host": "",
    "dispatch_lease_id": "",
    "attempt_id": "",
    "worker_attribution": "unavailable_in_adapter_result",
    "replay_strategy": "",
    "trace_id": "",
    "trace_slot": -1,
    "trace_corpus_size": 0,
    "actual_steps": 0,
    "response_tokens": 0,
    "training_trace_valid": False,
    "training_trace_errors_json": "[]",
    "rollout_param_version": "",
    "rollout_policy_version": "",
    "result_checksum": "",
    "result_checksum_valid": False,
    "extensions_json": "{}",
}


def _json_object(raw: Any) -> dict[str, Any]:
    if isinstance(raw, dict):
        return dict(raw)
    if isinstance(raw, (bytes, bytearray)) and raw:
        try:
            value = json.loads(bytes(raw).decode("utf-8"))
            return value if isinstance(value, dict) else {}
        except (UnicodeDecodeError, json.JSONDecodeError):
            return {}
    if isinstance(raw, str) and raw:
        try:
            value = json.loads(raw)
            return value if isinstance(value, dict) else {}
        except json.JSONDecodeError:
            return {}
    return {}


def _stable_json(value: Any) -> str:
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))


def new_episode_observation(**values: Any) -> dict[str, Any]:
    """Create one strict, versioned Episode observation row."""
    unknown = sorted(set(values) - set(EPISODE_OBSERVATION_FIELDS))
    if unknown:
        raise ValueError(f"unknown EpisodeObservation fields: {unknown}")
    observation = dict(_EPISODE_OBSERVATION_DEFAULTS)
    observation.update(values)
    validate_episode_observation(observation)
    return observation


def validate_episode_observation(observation: dict[str, Any]) -> None:
    """Fail closed when a producer silently changes the shared contract."""
    keys = tuple(observation)
    if set(keys) != set(EPISODE_OBSERVATION_FIELDS):
        missing = sorted(set(EPISODE_OBSERVATION_FIELDS) - set(keys))
        extra = sorted(set(keys) - set(EPISODE_OBSERVATION_FIELDS))
        raise ValueError(f"invalid EpisodeObservation fields missing={missing} extra={extra}")
    if int(observation["schema_version"]) != EPISODE_OBSERVATION_SCHEMA_VERSION:
        raise ValueError(f"unsupported EpisodeObservation schema_version={observation['schema_version']}")
    if observation["observation_type"] != "episode":
        raise ValueError("EpisodeObservation observation_type must be 'episode'")
    request_id = str(observation["request_id"])
    if request_id and str(observation["episode_id"]) != request_id:
        raise ValueError("EpisodeObservation episode_id must equal request_id")
    for name in ("sample_index", "sequence", "terminal_count", "trace_slot", "trace_corpus_size",
                 "actual_steps", "response_tokens"):
        int(observation[name])
    for name in ("planned_at", "dispatched_at", "deadline", "terminal_at",
                 "timeout_seconds", "end_to_end_ms", "batch_rpc_latency_ms", "reward"):
        value = float(observation[name])
        if not math.isfinite(value):
            raise ValueError(f"EpisodeObservation {name} must be finite")
    for name in ("training_trace_errors_json", "extensions_json"):
        json.loads(str(observation[name]))


def episode_observation_from_envelope(
    envelope: Any,
    *,
    suite: str,
    run_id: str = "",
    phase: str = "",
    planned_at: float = 0.0,
    arrival_mode: str = "",
    replay_strategy: str = "round_robin_episode",
) -> dict[str, Any]:
    """Normalize an Adapter SampleEnvelope into the shared observation row."""
    context = _json_object(getattr(envelope, "sample_context_json", b""))
    request_id = str(getattr(envelope, "request_id", ""))
    timeout = float(getattr(envelope, "timeout_seconds", 0.0) or 0.0)
    known_context = {
        "run_id", "stress_run_id", "task", "dataset", "environment", "sequence",
        "episode_ordinal", "arrival_mode", "dataset_item_id", "dataset_problem_id",
        "instance_id", "replay_strategy", "trace_id", "trace_slot", "trace_corpus_size",
    }
    return new_episode_observation(
        suite=suite,
        run_id=str(run_id or context.get("run_id") or context.get("stress_run_id") or ""),
        phase=str(phase),
        task=str(context.get("task") or context.get("dataset") or ""),
        dataset=str(context.get("dataset") or context.get("task") or ""),
        environment=str(context.get("environment") or getattr(envelope, "env_type", "")),
        parallel_mode=str(getattr(envelope, "parallel_mode", "")),
        arrival_mode=str(arrival_mode or context.get("arrival_mode") or ""),
        episode_id=request_id,
        request_id=request_id,
        batch_id=str(getattr(envelope, "batch_id", "")),
        sample_index=int(getattr(envelope, "sample_index", 0)),
        sequence=int(context.get("sequence", context.get("episode_ordinal", -1))),
        dataset_item_id=str(
            context.get("dataset_item_id") or context.get("dataset_problem_id") or ""
        ),
        instance_id=str(context.get("instance_id") or ""),
        planned_at=float(planned_at),
        timeout_seconds=timeout,
        deadline=float(planned_at) + timeout if planned_at else 0.0,
        replay_strategy=str(context.get("replay_strategy") or replay_strategy),
        trace_id=str(context.get("trace_id") or ""),
        trace_slot=int(context.get("trace_slot", -1)),
        trace_corpus_size=int(context.get("trace_corpus_size", 0)),
        extensions_json=_stable_json({
            key: value for key, value in context.items() if key not in known_context
        }),
    )


def finalize_episode_observation(
    observation: dict[str, Any],
    *,
    result: Any | None = None,
    terminal_at: float,
    dispatched_at: float = 0.0,
    batch_rpc_latency_ms: float = 0.0,
    terminal_count: int = 1,
    failure_class: str = "",
    error_code: str = "",
    error_message: str = "",
) -> dict[str, Any]:
    """Attach one terminal result (or a missing/RPC terminal condition)."""
    finalized = dict(observation)
    finalized["dispatch_started"] = bool(dispatched_at)
    finalized["dispatched_at"] = float(dispatched_at)
    finalized["deadline"] = (
        float(dispatched_at) + float(finalized["timeout_seconds"])
        if dispatched_at else float(finalized["deadline"])
    )
    finalized["terminal_at"] = float(terminal_at)
    finalized["terminal_count"] = int(terminal_count)
    finalized["batch_rpc_latency_ms"] = float(batch_rpc_latency_ms)
    origin = float(dispatched_at or finalized["planned_at"] or 0.0)
    finalized["end_to_end_ms"] = max(0.0, (float(terminal_at) - origin) * 1000.0) if origin else 0.0
    if result is None:
        finalized.update({
            "status": "rpc_error" if error_code else "missing_result",
            "error_code": str(error_code or "NO_TERMINAL_RESULT"),
            "error_message": str(error_message),
            "failure_class": str(failure_class or ("rpc_error" if error_code else "no_terminal_result")),
            "training_trace_errors_json": _stable_json(["no SampleResult"]),
        })
    else:
        converted = result if isinstance(result, dict) else sample_result_dict(result)
        checksum_payload = {
            "status": str(converted.get("status", "")),
            "error_code": str(converted.get("error_code", "")),
            "trajectory": converted.get("trajectory") or {},
        }
        checksum = hashlib.sha256(
            _stable_json(checksum_payload).encode("utf-8")
        ).hexdigest()
        trajectory = converted.get("trajectory") or {}
        status = str(converted.get("status", ""))
        finalized.update({
            "status": status,
            "done": bool(converted.get("done", False)),
            "reward": float(converted.get("reward", 0.0) or 0.0),
            "termination_reason": str(converted.get("termination_reason", "")),
            "error_code": str(converted.get("error_code", "")),
            "error_message": str(converted.get("error_message", "")),
            "failure_class": str(
                failure_class or ("none" if status.lower() in {"completed", "success"} else "uenv_error")
            ),
            "actual_steps": int(converted.get("actual_steps", 0) or 0),
            "response_tokens": len(converted.get("response_ids") or []),
            "training_trace_valid": bool(converted.get("training_trace_valid", False)),
            "training_trace_errors_json": _stable_json(converted.get("training_trace_errors") or []),
            "rollout_param_version": str(converted.get("rollout_param_version", "")),
            "rollout_policy_version": str(converted.get("rollout_policy_version", "")),
            "result_checksum": checksum,
            "result_checksum_valid": bool(trajectory) and len(checksum) == 64,
        })
    validate_episode_observation(finalized)
    return finalized


def observe_episode_batch(
    envelopes: Iterable[Any],
    results: Iterable[Any],
    *,
    suite: str,
    run_id: str,
    phase: str,
    planned_at: float,
    dispatched_at: float,
    terminal_at: float,
    batch_rpc_latency_ms: float,
    arrival_mode: str = "",
    replay_strategy: str = "round_robin_episode",
    rpc_error_code: str = "",
    rpc_error_message: str = "",
) -> list[dict[str, Any]]:
    """Return exactly one observation for every submitted Episode."""
    result_groups: dict[str, list[Any]] = {}
    for result in results:
        request_id = str(
            result.get("request_id", "") if isinstance(result, dict)
            else getattr(result, "request_id", "")
        )
        result_groups.setdefault(request_id, []).append(result)
    observations = []
    for envelope in envelopes:
        observation = episode_observation_from_envelope(
            envelope,
            suite=suite,
            run_id=run_id,
            phase=phase,
            planned_at=planned_at,
            arrival_mode=arrival_mode,
            replay_strategy=replay_strategy,
        )
        matches = result_groups.get(observation["request_id"], [])
        if matches:
            observations.append(finalize_episode_observation(
                observation,
                result=matches[0],
                terminal_at=terminal_at,
                dispatched_at=dispatched_at,
                batch_rpc_latency_ms=batch_rpc_latency_ms,
                terminal_count=len(matches),
                failure_class="duplicate_terminal_result" if len(matches) > 1 else "",
            ))
        else:
            observations.append(finalize_episode_observation(
                observation,
                terminal_at=terminal_at,
                dispatched_at=dispatched_at,
                batch_rpc_latency_ms=batch_rpc_latency_ms,
                error_code=rpc_error_code,
                error_message=rpc_error_message,
            ))
    return observations


def write_episode_observations_jsonl(
    path: str | Path, observations: Iterable[dict[str, Any]]
) -> int:
    """Write observations in canonical field order and return the row count."""
    count = 0
    with Path(path).open("w", encoding="utf-8") as target:
        for observation in observations:
            validate_episode_observation(observation)
            ordered = {field: observation[field] for field in EPISODE_OBSERVATION_FIELDS}
            target.write(
                json.dumps(ordered, ensure_ascii=False, separators=(",", ":")) + "\n"
            )
            count += 1
    return count


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


def load_dscodebench_jsonl(path: str, *, limit: int = 0, offset: int = 0) -> list[dict]:
    """Load real DSCodeBench JSONL rows without loading the entire corpus."""
    rows = []
    with Path(path).open("r", encoding="utf-8") as source:
        for line_index, line in enumerate(source):
            if line_index < offset or not line.strip():
                continue
            row = json.loads(line)
            required = {"problem_id", "library", "code_problem", "ground_truth_code", "test_script"}
            missing = sorted(required - set(row))
            if missing:
                raise ValueError(f"DSCodeBench row {line_index} missing fields: {missing}")
            rows.append(row)
            if limit > 0 and len(rows) >= limit:
                break
    if not rows:
        raise ValueError(f"no DSCodeBench rows loaded from {path!r} offset={offset} limit={limit}")
    return rows


def dscodebench_prompt(row: dict) -> str:
    return (
        "You are a careful Python data science coding assistant.\n"
        "Please generate a Python3 solution for the following code problem description:\n\n"
        "# Code problem description #\n"
        f"{row['code_problem']}\n\n"
        "# Response #\n"
        "Return only one Python markdown code block containing the solution. "
        "Do not add a __main__ block."
    )


def dscodebench_inline_test_code(row: dict, *, num_tests: int, random_seed: int) -> str:
    """Build the same inline harness used by the DSCodeBench UEnv evaluator."""
    return f"""
import inspect
from dscodebench_harness import evaluate_problem

_candidate_source = inspect.currentframe().f_back.f_locals.get("code", "")
_result = evaluate_problem(
    ground_truth_code={str(row['ground_truth_code'])!r},
    candidate_code=_candidate_source,
    test_script={str(row['test_script'])!r},
    num_tests={int(num_tests)},
    random_seed={int(random_seed)},
)
"""


def dscodebench_env_payload(
    row: dict,
    *,
    task_id: str,
    min_steps_before_terminate: int,
    num_tests: int = 20,
    random_seed: int = 42,
    timeout_secs: int = 120,
) -> dict:
    """Map one real DSCodeBench row to the Code Worker contract."""
    problem_id = str(row["problem_id"])
    question = (
        f"{dscodebench_prompt(row)}\n"
        f"Dataset Problem ID: {problem_id}\n"
        f"Task ID: {task_id}"
    )
    return {
        "question": question,
        "dataset": "dscodebench",
        "task_id": task_id,
        "library": str(row.get("library", "")),
        "ground_truth_code": str(row["ground_truth_code"]),
        "test_code": dscodebench_inline_test_code(
            row,
            num_tests=num_tests,
            random_seed=random_seed,
        ),
        "num_tests": num_tests,
        "random_seed": random_seed,
        "timeout_secs": timeout_secs,
        "min_steps_before_terminate": min_steps_before_terminate,
        "dataset_problem_id": problem_id,
    }


def dscodebench_reward_config() -> dict:
    return {"type": "code_tests"}


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
    request_id: str = "",
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
        "request_id": request_id or str(uuid.uuid4()),
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
    """Convert SampleResult and expose the complete typed training trace."""
    trajectory = {}
    raw_trajectory = bytes(getattr(result, "trajectory_json", b"") or b"")
    if raw_trajectory:
        try:
            value = json.loads(raw_trajectory.decode("utf-8"))
            if isinstance(value, dict):
                trajectory = value
        except (UnicodeDecodeError, json.JSONDecodeError):
            trajectory = {}
    response_ids: list[int] = []
    response_mask: list[int] = []
    steps = trajectory.get("steps") if isinstance(trajectory, dict) else None
    if isinstance(steps, list):
        for step in steps:
            trace = step.get("rollout_trace") if isinstance(step, dict) else None
            if not isinstance(trace, dict):
                continue
            response_ids.extend(int(item) for item in (trace.get("response_ids") or []))
            response_mask.extend(int(item) for item in (trace.get("response_mask") or []))
    log_probs = [float(item) for item in getattr(result, "rollout_log_probs", [])]
    trace_errors = []
    if not trajectory:
        trace_errors.append("missing trajectory_json")
    if not response_ids:
        trace_errors.append("missing response_ids")
    if not response_mask:
        trace_errors.append("missing response_mask")
    if len(response_ids) != len(response_mask):
        trace_errors.append("response_ids/response_mask length mismatch")
    if not log_probs:
        trace_errors.append("missing rollout_log_probs")
    if response_ids and len(log_probs) != len(response_ids):
        trace_errors.append("rollout_log_probs/response_ids length mismatch")
    return {
        "request_id": result.request_id,
        "batch_id": result.batch_id,
        "sample_index": result.sample_index,
        "status": result.status,
        "reward": result.reward,
        "done": result.done,
        "termination_reason": result.termination_reason,
        "error_code": result.error_code,
        "error_message": result.error_message,
        "trajectory": trajectory,
        "actual_steps": int(trajectory.get("total_steps", 0) or 0),
        "response_ids": response_ids,
        "response_mask": response_mask,
        "rollout_param_version": result.rollout_param_version,
        "rollout_policy_version": result.rollout_policy_version,
        "rollout_log_probs": log_probs,
        "training_trace_valid": not trace_errors,
        "training_trace_errors": trace_errors,
    }


def dscodebench_pressure_result_document(
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
    """生成 DSCodeBench pressure Code 扩容压测的结果 JSON。

    DSCodeBench pressure 关心的是真实 Code worker 在不同 worker/slot 规模下的吞吐、
    延迟和协议错误数量，所以这里输出 completed、failed、throughput_eps、
    batch_latency_ms、protocol_errors 等字段。
    """
    rewards = list(rewards)
    resolved = completed + failed + rpc_error_episodes
    infrastructure_passed = bool(
        submitted
        and completed == submitted
        and not failed
        and not rpc_error_episodes
        and not protocol_errors
    )
    average_reward = sum(rewards) / len(rewards) if rewards else 0.0
    return {
        "schema_version": 2,
        "test_name": "DSCodeBench pressure",
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
        "infrastructure": {
            "status": "passed" if infrastructure_passed else "failed",
            "passed": infrastructure_passed,
        },
        "model_quality": {
            "average_reward": average_reward,
            "successful_rewards": sum(1 for reward in rewards if reward >= 0.999),
            "sample_count": len(rewards),
        },
        "average_reward": average_reward,
    }


def swebench_pro_pressure_result_document(
    *,
    run_id: str,
    server: str,
    worker_id: str,
    registered_workers: list[str],
    instance_id: str,
    mode: str,
    parallel_mode: str,
    concurrency: int,
    max_steps: int,
    openhands_max_iterations: int,
    elapsed_seconds: float,
    results: list[dict],
) -> dict:
    """生成 SWE-bench Pro pressure SWE/OpenHands 容器压测的结果 JSON。"""
    average_reward = sum(item["reward"] for item in results) / len(results) if results else 0.0
    statuses_ok = bool(
        results and all(item["status"] in {"completed", "success"} for item in results)
    )
    traces_ok = bool(results and all(item["training_trace_valid"] for item in results))
    infrastructure_passed = statuses_ok and traces_ok
    return {
        "schema_version": 2,
        "test_name": "SWE-bench Pro pressure",
        "dataset": "swe-bench-pro",
        "benchmark_variant": "pro",
        "run_id": run_id,
        "server": server,
        "worker_id": worker_id,
        "registered_workers": registered_workers,
        "instance_id": instance_id,
        "mode": mode,
        "parallel_mode": parallel_mode,
        "concurrency": concurrency,
        "max_steps": max_steps,
        "openhands_max_iterations": openhands_max_iterations,
        "elapsed_seconds": elapsed_seconds,
        "status": "completed" if infrastructure_passed else "failed",
        "infrastructure": {
            "status": "passed" if infrastructure_passed else "failed",
            "passed": infrastructure_passed,
            "statuses_valid": statuses_ok,
            "training_traces_valid": traces_ok,
        },
        "model_quality": {
            "average_reward": average_reward,
            "successful_rewards": sum(1 for item in results if item["reward"] >= 0.999),
            "sample_count": len(results),
        },
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

    单机 runner 支持 math/code/swe_openhands，所以字段比 DSCodeBench pressure/SWE-bench Pro pressure 更通用。
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
