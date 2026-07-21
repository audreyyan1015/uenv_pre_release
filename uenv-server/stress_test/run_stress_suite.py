#!/usr/bin/env python3
"""Run Gate3 and Gate4 as one protected, reproducible scale-stress suite.

The suite is an orchestrator.  It must run on a control machine that can SSH to
the Server and Worker hosts.  It never installs or restarts the protected
production adapter core; each child gate owns and cleans up its isolated
processes and ports.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time
from typing import Any


HERE = Path(__file__).resolve().parent
DEFAULT_CONFIG = HERE / "stress_suite.json"
ALLOWED_EXPOSED_PORTS = {5432, 6379, 8000, 8077, 8088, 8099, 8777, 8888}
GATE3_MODES = {"sync", "one_step_off_policy", "fully_async"}


def latency_config(section: dict[str, Any], field: str = "simulator_latency_ms") -> dict[str, float]:
    value = section.get(field, {})
    if isinstance(value, dict):
        result = {
            "mean": float(value.get("mean", 0.0)),
            "std": float(value.get("std", 0.0)),
            "min": float(value.get("min", 0.0)),
            "max": float(value.get("max", 0.0)),
        }
    else:
        scalar = float(value)
        result = {"mean": scalar, "std": 0.0, "min": scalar, "max": scalar}
    if not 0 <= result["min"] <= result["mean"] <= result["max"]:
        raise ValueError(f"{field} must satisfy 0 <= min <= mean <= max")
    if result["std"] < 0:
        raise ValueError(f"{field} std must be non-negative")
    return result


def load_suite_config(path: Path) -> dict[str, Any]:
    document = json.loads(path.read_text(encoding="utf-8"))
    if document.get("schema_version") != 1:
        raise ValueError("stress suite schema_version must be 1")
    gate3 = document.get("gate3")
    gate4 = document.get("gate4")
    worker_scale = document.get("worker_scale")
    trace_collection = document.get("trace_collection")
    if not isinstance(gate3, dict) or not isinstance(gate4, dict) or not isinstance(worker_scale, dict):
        raise ValueError("stress suite requires gate3, gate4 and worker_scale objects")
    if not isinstance(trace_collection, dict):
        raise ValueError("stress suite requires a trace_collection object")
    dscodebench_collection = trace_collection.get("dscodebench")
    swe_collection = trace_collection.get("swebench_verified")
    if not isinstance(dscodebench_collection, dict) or not isinstance(swe_collection, dict):
        raise ValueError("trace_collection requires dscodebench and swebench_verified objects")
    if int(dscodebench_collection.get("dataset_count", 0)) != 100:
        raise ValueError("DSCodeBench real-LLM trace collection must sample exactly 100 records")
    if int(dscodebench_collection.get("collection_concurrency", 0)) != 100:
        raise ValueError("DSCodeBench real-LLM trace collection concurrency must be 100")
    if bool(dscodebench_collection.get("uses_1024_workers", True)):
        raise ValueError("DSCodeBench trace collection must not use 1024 Workers")
    if int(swe_collection.get("instance_count", 0)) != 50:
        raise ValueError("SWE-bench Verified real-LLM trace collection must sample exactly 50 instances")
    swe_concurrency = int(swe_collection.get("collection_concurrency", 0))
    if not 1 <= swe_concurrency <= 50:
        raise ValueError("SWE-bench Verified real-LLM trace collection concurrency must be in [1, 50]")
    if int(swe_collection.get("target_valid_traces", 50)) != 50:
        raise ValueError("SWE-bench Verified trace collection target_valid_traces must be 50")
    if bool(swe_collection.get("uses_1024_workers", True)):
        raise ValueError("SWE-bench Verified trace collection must not use 1024 Workers")
    modes = gate3.get("modes")
    if not isinstance(modes, list) or not modes or set(modes) - GATE3_MODES:
        raise ValueError(f"invalid Gate3 modes: {modes!r}")
    if gate3.get("model_mode") != "simulator":
        raise ValueError("scale stress requires Gate3 model_mode=simulator")
    gate3_simulator_mode = str(gate3.get("simulator_mode", "template"))
    if gate3_simulator_mode not in {"template", "trace_replay"}:
        raise ValueError("Gate3 simulator_mode must be template or trace_replay")
    if gate3_simulator_mode != "trace_replay":
        raise ValueError("Gate3 pressure evidence requires simulator_mode=trace_replay")
    latency_config(gate3)
    if not str(gate3.get("trace_corpus_path", "")).strip():
        raise ValueError("Gate3 trace_replay requires trace_corpus_path")
    gate3_sampling = str(gate3.get("trace_sampling_strategy", "problem_then_turn"))
    if gate3_sampling not in {"problem_then_turn", "turn_only"}:
        raise ValueError("Gate3 trace_sampling_strategy is invalid")
    if int(gate3.get("workers", 0)) < 1024:
        raise ValueError("Gate3 pressure evidence requires at least 1024 Workers")
    capacity = int(gate3.get("capacity_per_worker", 0))
    batch_size = int(gate3.get("episode_batch_size", gate3.get("workers", 0)))
    exact_batches = int(gate3.get("exact_batches_per_mode", 0))
    min_waves = int(gate3.get("min_episode_waves", 10))
    if batch_size * exact_batches < int(gate3["workers"]) * capacity * min_waves:
        raise ValueError("Gate3 total episodes per mode must be at least workers * capacity * min_episode_waves")
    if gate4.get("mode") != "llm":
        raise ValueError("the integrated acceptance suite requires Gate4 mode=llm")
    gate4_parallel_modes = gate4.get("parallel_modes")
    if (
        not isinstance(gate4_parallel_modes, list)
        or set(gate4_parallel_modes) != GATE3_MODES
    ):
        raise ValueError("Gate4 must cover sync, one_step_off_policy and fully_async parallel_modes")
    llm_kind = str(gate4.get("llm_kind", "simulator"))
    if llm_kind not in {"simulator", "real"}:
        raise ValueError("Gate4 llm_kind must be simulator or real")
    if llm_kind == "simulator":
        latency_config(gate4)
        gate4_wrong_steps = gate4.get("simulator_wrong_steps", {})
        if not (
            0
            <= int(gate4_wrong_steps.get("min", 0))
            <= float(gate4_wrong_steps.get("mean", 0))
            <= int(gate4_wrong_steps.get("max", 0))
        ):
            raise ValueError("Gate4 simulator wrong_steps must satisfy 0 <= min <= mean <= max")
        if float(gate4_wrong_steps.get("std", 0)) < 0:
            raise ValueError("Gate4 simulator wrong_steps std must be non-negative")
        if not 0 <= float(gate4.get("simulator_repair_success_rate", 0)) <= 1:
            raise ValueError("Gate4 simulator_repair_success_rate must be in [0, 1]")
        simulator_mode = str(gate4.get("simulator_mode", "template"))
        if simulator_mode not in {"template", "trace_replay"}:
            raise ValueError("Gate4 simulator_mode must be template or trace_replay")
        if simulator_mode == "trace_replay" and not str(gate4.get("trace_corpus_path", "")).strip():
            raise ValueError("Gate4 trace_replay requires trace_corpus_path")
        sampling = str(gate4.get("trace_sampling_strategy", "instance_then_turn"))
        if sampling not in {"instance_then_turn", "turn_only"}:
            raise ValueError("Gate4 trace_sampling_strategy is invalid")
    concurrencies = gate4.get("concurrencies")
    if not isinstance(concurrencies, list) or not concurrencies or any(int(value) <= 0 for value in concurrencies):
        raise ValueError("Gate4 concurrencies must be positive integers")
    if int(gate4.get("instance_count", 0)) < 50:
        raise ValueError("Gate4 SWE-bench Verified coverage requires at least 50 sampled instances")
    gate4_workers = int(gate4.get("registered_workers", 1))
    gate4_capacity = int(gate4.get("worker_capacity", 1))
    gate4_waves = int(gate4.get("min_episode_waves", 10))
    gate4_total_episodes = int(gate4.get("total_episodes", 0))
    if gate4_workers < 1024:
        raise ValueError("Gate4 scale evidence requires at least 1024 registered Workers")
    if gate4_capacity < 1:
        raise ValueError("Gate4 worker_capacity must be positive")
    if gate4_total_episodes < gate4_workers * gate4_capacity * gate4_waves:
        raise ValueError("Gate4 total_episodes must be at least registered_workers * worker_capacity * min_episode_waves")
    if llm_kind != "simulator" or str(gate4.get("simulator_mode", "")) != "trace_replay":
        raise ValueError("Gate4 1024 Worker scale requires simulator trace_replay")
    min_steps = int(gate3.get("min_steps", 0))
    max_steps = int(gate3.get("max_steps", 0))
    if min_steps < 2 or max_steps < min_steps:
        raise ValueError("Gate3 requires 2 <= min_steps <= max_steps")
    if not worker_scale.get("enabled", False):
        return document
    if worker_scale.get("model_mode") != "trace_replay_simulator":
        raise ValueError("worker_scale must explicitly use trace_replay_simulator")
    worker_scale_simulator_mode = str(worker_scale.get("simulator_mode", "trace_replay"))
    if worker_scale_simulator_mode != "trace_replay":
        raise ValueError("worker_scale requires simulator_mode=trace_replay")
    if not str(worker_scale.get("trace_corpus_path", "")).strip():
        raise ValueError("worker_scale trace_replay requires trace_corpus_path")
    worker_scale_sampling = str(worker_scale.get("trace_sampling_strategy", "problem_then_turn"))
    if worker_scale_sampling not in {"problem_then_turn", "turn_only"}:
        raise ValueError("worker_scale trace_sampling_strategy is invalid")
    latency_config(worker_scale)
    tiers = worker_scale.get("tiers")
    if not isinstance(tiers, list) or not tiers or min(int(value) for value in tiers) < 1024:
        raise ValueError("worker_scale tiers must all be at least 1024")
    episode_batch_size = int(worker_scale.get("episode_batch_size", 0))
    episodes_per_worker = int(worker_scale.get("episodes_per_worker", 0))
    if episode_batch_size < 1 or episodes_per_worker < 1:
        raise ValueError("worker_scale episode_batch_size and episodes_per_worker must be positive")
    for workers in tiers:
        if int(workers) * episodes_per_worker % episode_batch_size:
            raise ValueError(
                f"worker_scale tier {workers} does not divide evenly into episode batches"
            )
        if int(workers) * int(worker_scale["capacity_per_worker"]) % episode_batch_size:
            raise ValueError(
                f"worker_scale tier {workers} slots do not divide evenly into concurrent batches"
            )
    if int(worker_scale.get("minimum_mem_available_bytes", 0)) < 1024 * 1024 * 1024:
        raise ValueError("worker_scale minimum_mem_available_bytes must be at least 1 GiB")
    fraction = float(worker_scale.get("maximum_projected_host_memory_fraction", 0))
    if not 0.5 <= fraction <= 0.9:
        raise ValueError("worker_scale maximum_projected_host_memory_fraction must be between 0.5 and 0.9")
    return document


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def require_absolute(label: str, value: str) -> None:
    if not value.startswith("/"):
        raise ValueError(f"{label} must be an absolute remote path: {value!r}")


def parse_port_range(label: str, value: str) -> tuple[int, int]:
    try:
        start_text, end_text = value.split("-", 1)
        start, end = int(start_text), int(end_text)
    except (ValueError, AttributeError) as exc:
        raise ValueError(f"{label} must be START-END") from exc
    if start <= 0 or end < start or end > 65535:
        raise ValueError(f"{label} must be a valid TCP port range")
    return start, end


def validate_arguments(args: argparse.Namespace, config: dict[str, Any]) -> None:
    for label in ("source_repo", "server_bin", "worker_bin", "code_plugin_bin"):
        require_absolute(f"--{label.replace('_', '-')}", str(getattr(args, label)))
    if config["gate4"].get("llm_kind") == "real":
        require_absolute("--llm-config", str(args.llm_config))
    gate4 = config["gate4"]
    gate4_model_port = int(gate4.get("model_port", args.model_port))
    gate4_gateway_port = int(gate4.get("gateway_port", args.gateway_port))
    gate4_agent_api_port = int(gate4.get("agent_api_port", args.agent_api_port))
    gate4_agent_health_port = int(gate4.get("agent_health_port", args.agent_health_port))
    gate4_workers = int(gate4.get("registered_workers", 1))
    gate3_workers = int(config["gate3"].get("workers", 1))
    exposed_ports = {
        "server": args.server_port,
        "model": gate4_model_port,
        "agent API": gate4_agent_api_port,
        "agent health": gate4_agent_health_port,
    }
    if gate3_workers == 1 and gate4_workers == 1:
        exposed_ports["worker"] = args.worker_port
        exposed_ports["gateway"] = gate4_gateway_port
    for label, port in exposed_ports.items():
        if port not in ALLOWED_EXPOSED_PORTS:
            raise ValueError(f"{label} port {port} is not in the explicitly allowed exposed-port set")
    protected = set(args.protected_port)
    requested = {args.server_port, gate4_model_port}
    if gate3_workers == 1 and gate4_workers == 1:
        requested.update({args.worker_port, gate4_gateway_port})
    overlap = protected & requested
    if overlap:
        raise ValueError(f"isolated suite ports overlap protected ports: {sorted(overlap)}")
    gate3 = config["gate3"]
    workers = int(gate3["workers"])
    worker_scale = config["worker_scale"]
    max_scale_workers = max(
        [workers] + [int(value) for value in worker_scale.get("tiers", [])]
    )
    scale_model_port = int(worker_scale.get("model_port", args.model_port))
    if worker_scale.get("enabled", False):
        if scale_model_port not in ALLOWED_EXPOSED_PORTS:
            raise ValueError(
                f"worker-scale model port {scale_model_port} is not in the explicitly allowed exposed-port set"
            )
        if scale_model_port in protected:
            raise ValueError(f"worker-scale model port {scale_model_port} overlaps a protected port")
    if (workers > 1 or config["worker_scale"].get("enabled", True)) and not args.private_worker_port_range:
        raise ValueError("multi-Worker execution requires --private-worker-port-range")
    if int(config["gate4"].get("registered_workers", 1)) > 1 and not args.private_gateway_port_range:
        raise ValueError("Gate4 multi-Worker execution requires --private-gateway-port-range")
    private_ranges: list[tuple[str, int, int]] = []
    if args.private_worker_port_range:
        start, end = parse_port_range("--private-worker-port-range", args.private_worker_port_range)
        private_ranges.append(("Worker", start, end))
        if start != args.worker_port or end - start + 1 < max_scale_workers or end > 65535:
            raise ValueError(
                f"private Worker range must start at {args.worker_port} and contain at least {max_scale_workers} ports"
            )
        if start <= gate4_model_port <= end:
            raise ValueError(
                f"model port {gate4_model_port} overlaps the private Worker port range {start}-{end}"
            )
        for label, port in {
            "agent API": gate4_agent_api_port,
            "agent health": gate4_agent_health_port,
        }.items():
            if start <= port <= end:
                raise ValueError(f"{label} port {port} overlaps the private Worker port range {start}-{end}")
        if start <= scale_model_port < start + max_scale_workers:
            raise ValueError(
                f"worker-scale model port {scale_model_port} overlaps the {max_scale_workers}-Worker port range"
            )
    if args.private_gateway_port_range:
        gate4_workers = int(config["gate4"].get("registered_workers", 1))
        start, end = parse_port_range("--private-gateway-port-range", args.private_gateway_port_range)
        private_ranges.append(("Gateway", start, end))
        if start != gate4_gateway_port or end - start + 1 < gate4_workers:
            raise ValueError(
                f"private Gateway range must start at {gate4_gateway_port} and contain at least {gate4_workers} ports"
            )
        for label, port in {
            "server": args.server_port,
            "model": gate4_model_port,
            "worker-scale model": scale_model_port,
            "agent API": gate4_agent_api_port,
            "agent health": gate4_agent_health_port,
        }.items():
            if start <= port <= end:
                raise ValueError(f"{label} port {port} overlaps the private Gateway port range {start}-{end}")
    obs_start = args.obs_port
    obs_end = args.obs_port + max(int(config["gate4"].get("registered_workers", 1)), max_scale_workers) - 1
    for label, port in {
        "model": gate4_model_port,
        "agent API": gate4_agent_api_port,
        "agent health": gate4_agent_health_port,
    }.items():
        if obs_start <= port <= obs_end:
            raise ValueError(f"{label} port {port} overlaps the Observability port range {obs_start}-{obs_end}")
    private_ranges.append(("Observability", obs_start, obs_end))
    for left_index, (left_label, left_start, left_end) in enumerate(private_ranges):
        for right_label, right_start, right_end in private_ranges[left_index + 1:]:
            if max(left_start, right_start) <= min(left_end, right_end):
                raise ValueError(
                    f"{left_label} port range {left_start}-{left_end} overlaps "
                    f"{right_label} port range {right_start}-{right_end}"
                )


def common_child_args(args: argparse.Namespace, *, model_port: int | None = None) -> list[str]:
    command = [
        "--source-repo", args.source_repo,
        "--server-bin", args.server_bin,
        "--worker-bin", args.worker_bin,
        "--code-plugin-bin", args.code_plugin_bin,
        "--protected-pid", str(args.protected_pid),
        "--server-host", args.server_host,
        "--worker-host", args.worker_host,
        "--server-private-ip", args.server_private_ip,
        "--worker-private-ip", args.worker_private_ip,
        "--server-port", str(args.server_port),
        "--worker-port", str(args.worker_port),
        "--model-port", str(args.model_port if model_port is None else model_port),
        "--obs-port", str(args.obs_port),
    ]
    for port in args.protected_port:
        command.extend(["--protected-port", str(port)])
    return command


def gate3_command(args: argparse.Namespace, config: dict[str, Any], artifacts: Path) -> list[str]:
    gate = config["gate3"]
    latency = latency_config(gate)
    command = [
        sys.executable,
        str(HERE / "run_distributed_gate3_code.py"),
        "--duration", str(gate["duration_seconds_per_mode"]),
        "--workers", str(gate["workers"]),
        "--capacity", str(gate["capacity_per_worker"]),
        "--min-steps", str(gate["min_steps"]),
        "--max-steps", str(gate["max_steps"]),
        "--model-mode", str(gate["model_mode"]),
        "--dataset-jsonl", str(gate["dataset_jsonl"]),
        "--dataset-limit", str(gate["dataset_limit"]),
        "--dataset-offset", str(gate["dataset_offset"]),
        "--exact-batches", str(gate["exact_batches_per_mode"]),
        "--episode-batch-size", str(gate["episode_batch_size"]),
        "--concurrent-batches", str(gate["concurrent_batches"]),
        "--registration-timeout", str(gate["registration_timeout_seconds"]),
        "--batch-timeout", str(gate["batch_timeout_seconds"]),
        "--plugin-ready-timeout-seconds", str(gate["plugin_ready_timeout_seconds"]),
        "--worker-register-max-attempts", str(gate["worker_register_max_attempts"]),
        "--worker-register-retry-backoff-ms", str(gate["worker_register_retry_backoff_ms"]),
        "--simulator-latency-mean-ms", str(latency["mean"]),
        "--simulator-latency-std-ms", str(latency["std"]),
        "--simulator-latency-min-ms", str(latency["min"]),
        "--simulator-latency-max-ms", str(latency["max"]),
        "--simulator-wrong-steps-mean", str(gate["simulator_wrong_steps"]["mean"]),
        "--simulator-wrong-steps-std", str(gate["simulator_wrong_steps"]["std"]),
        "--simulator-wrong-steps-min", str(gate["simulator_wrong_steps"]["min"]),
        "--simulator-wrong-steps-max", str(gate["simulator_wrong_steps"]["max"]),
        "--simulator-seed", str(gate["simulator_seed"]),
        "--simulator-mode", str(gate.get("simulator_mode", "trace_replay")),
        "--trace-corpus-path", str(gate.get("trace_corpus_path", "")),
        "--trace-sampling-strategy", str(gate.get("trace_sampling_strategy", "problem_then_turn")),
        "--min-scale-episode-waves", str(gate["min_episode_waves"]),
        "--acceptance-purpose", "worker-scale",
        "--artifacts", str(artifacts),
    ]
    if gate.get("code_python"):
        command.extend(["--code-python", str(gate["code_python"])])
    if gate.get("simulator_zero_latency", False):
        command.append("--simulator-zero-latency")
    if gate["model_mode"] == "real":
        command.extend(["--llm-config", args.llm_config])
    for mode in gate["modes"]:
        command.extend(["--mode", mode])
    if int(gate["workers"]) > 1 and args.private_worker_port_range:
        command.extend(["--private-worker-port-range", args.private_worker_port_range])
    return command + common_child_args(args)


def worker_scale_command(
    args: argparse.Namespace,
    config: dict[str, Any],
    workers: int,
    artifacts: Path,
) -> list[str]:
    gate = config["worker_scale"]
    episode_batch_size = int(gate["episode_batch_size"])
    exact_batches = workers * int(gate["episodes_per_worker"]) // episode_batch_size
    concurrent_batches = exact_batches
    latency = latency_config(gate)
    command = [
        sys.executable,
        str(HERE / "run_distributed_gate3_code.py"),
        "--duration", "1",
        "--workers", str(workers),
        "--capacity", str(gate["capacity_per_worker"]),
        "--min-steps", str(gate["min_steps"]),
        "--max-steps", str(gate["max_steps"]),
        "--model-mode", "simulator",
        "--dataset-jsonl", str(gate["dataset_jsonl"]),
        "--dataset-limit", str(gate["dataset_limit"]),
        "--dataset-offset", str(gate["dataset_offset"]),
        "--exact-batches", str(exact_batches),
        "--episode-batch-size", str(episode_batch_size),
        "--concurrent-batches", str(concurrent_batches),
        "--registration-timeout", str(gate["registration_timeout_seconds"]),
        "--batch-timeout", str(gate["batch_timeout_seconds"]),
        "--simulator-latency-mean-ms", str(latency["mean"]),
        "--simulator-latency-std-ms", str(latency["std"]),
        "--simulator-latency-min-ms", str(latency["min"]),
        "--simulator-latency-max-ms", str(latency["max"]),
        "--simulator-wrong-steps-mean", str(gate["simulator_wrong_steps"]["mean"]),
        "--simulator-wrong-steps-std", str(gate["simulator_wrong_steps"]["std"]),
        "--simulator-wrong-steps-min", str(gate["simulator_wrong_steps"]["min"]),
        "--simulator-wrong-steps-max", str(gate["simulator_wrong_steps"]["max"]),
        "--simulator-seed", str(gate["simulator_seed"]),
        "--simulator-mode", str(gate.get("simulator_mode", "trace_replay")),
        "--trace-corpus-path", str(gate.get("trace_corpus_path", "")),
        "--trace-sampling-strategy", str(gate.get("trace_sampling_strategy", "problem_then_turn")),
        "--min-scale-episode-waves", str(gate.get("min_episode_waves", 10)),
        "--plugin-ready-timeout-seconds", str(gate["plugin_ready_timeout_seconds"]),
        "--worker-register-max-attempts", str(gate["worker_register_max_attempts"]),
        "--worker-register-retry-backoff-ms", str(gate["worker_register_retry_backoff_ms"]),
        "--acceptance-purpose", "worker-scale",
        "--private-worker-port-range", args.private_worker_port_range,
        "--artifacts", str(artifacts),
    ]
    if gate.get("code_python"):
        command.extend(["--code-python", str(gate["code_python"])])
    if gate.get("simulator_zero_latency", False):
        command.append("--simulator-zero-latency")
    for mode in gate.get("modes", ["sync", "one_step_off_policy", "fully_async"]):
        command.extend(["--mode", mode])
    return command + common_child_args(args, model_port=int(gate["model_port"]))


def gate4_command(args: argparse.Namespace, config: dict[str, Any], artifacts: Path) -> list[str]:
    gate = config["gate4"]
    latency = latency_config(gate)
    model_port = int(gate.get("model_port", args.model_port))
    gateway_port = int(gate.get("gateway_port", args.gateway_port))
    agent_api_port = int(gate.get("agent_api_port", args.agent_api_port))
    agent_health_port = int(gate.get("agent_health_port", args.agent_health_port))
    command = [
        sys.executable,
        str(HERE / "run_distributed_gate4_swe.py"),
        "--mode", "llm",
        "--llm-kind", str(gate["llm_kind"]),
        "--max-steps", str(gate["max_steps"]),
        "--openhands-max-iterations", str(gate["openhands_max_iterations"]),
        "--instance-count", str(gate["instance_count"]),
        "--instance-seed", str(gate["instance_seed"]),
        "--registered-workers", str(gate.get("registered_workers", 1)),
        "--worker-capacity", str(gate.get("worker_capacity", 1)),
        "--total-episodes", str(gate.get("total_episodes", 0)),
        "--episode-batch-size", str(gate.get("episode_batch_size", 0)),
        "--min-scale-episode-waves", str(gate.get("min_episode_waves", 10)),
        "--fleet-supervisor-threshold", str(gate.get("fleet_supervisor_threshold", 16)),
        "--registration-timeout", str(gate.get("registration_timeout_seconds", 900)),
        "--batch-timeout", str(gate.get("batch_timeout_seconds", 1800)),
        "--simulator-latency-ms", str(latency["mean"]),
        "--simulator-latency-mean-ms", str(latency["mean"]),
        "--simulator-latency-std-ms", str(latency["std"]),
        "--simulator-latency-min-ms", str(latency["min"]),
        "--simulator-latency-max-ms", str(latency["max"]),
        "--simulator-wrong-steps-mean", str(gate["simulator_wrong_steps"]["mean"]),
        "--simulator-wrong-steps-std", str(gate["simulator_wrong_steps"]["std"]),
        "--simulator-wrong-steps-min", str(gate["simulator_wrong_steps"]["min"]),
        "--simulator-wrong-steps-max", str(gate["simulator_wrong_steps"]["max"]),
        "--simulator-repair-success-rate", str(gate["simulator_repair_success_rate"]),
        "--simulator-repair-style", str(gate["simulator_repair_style"]),
        "--simulator-seed", str(gate["simulator_seed"]),
        "--simulator-mode", str(gate.get("simulator_mode", "template")),
        "--trace-corpus-path", str(gate.get("trace_corpus_path", "")),
        "--trace-sampling-strategy", str(gate.get("trace_sampling_strategy", "instance_then_turn")),
        "--gateway-port", str(gateway_port),
        "--agent-api-port", str(agent_api_port),
        "--agent-health-port", str(agent_health_port),
        "--artifacts", str(artifacts),
    ]
    if gate["llm_kind"] == "real":
        command.extend(["--llm-config", args.llm_config])
    if gate.get("simulator_zero_latency", False):
        command.append("--simulator-zero-latency")
    if int(gate.get("registered_workers", 1)) > 1:
        command.extend(["--private-worker-port-range", args.private_worker_port_range])
        command.extend(["--private-gateway-port-range", args.private_gateway_port_range])
    for parallel_mode in gate.get("parallel_modes", ["sync", "one_step_off_policy", "fully_async"]):
        command.extend(["--parallel-mode", str(parallel_mode)])
    for concurrency in gate["concurrencies"]:
        command.extend(["--concurrency", str(concurrency)])
    return command + common_child_args(args, model_port=model_port)


def newest_summary(root: Path, pattern: str) -> Path | None:
    candidates = list(root.rglob(pattern))
    return max(candidates, key=lambda item: item.stat().st_mtime_ns) if candidates else None


def collect_records(value: Any, key: str) -> list[Any]:
    records: list[Any] = []
    if isinstance(value, dict):
        if key in value:
            records.append(value[key])
        for child in value.values():
            records.extend(collect_records(child, key))
    elif isinstance(value, list):
        for child in value:
            records.extend(collect_records(child, key))
    return records


def run_child(name: str, command: list[str], artifacts: Path, summary_pattern: str) -> dict[str, Any]:
    artifacts.mkdir(parents=True, exist_ok=True)
    log_path = artifacts / f"{name}.log"
    started = time.time()
    print(f"[suite] {name} start", flush=True)
    error = ""
    returncode = -1
    parsed = None
    summary_path = None
    try:
        with log_path.open("w", encoding="utf-8") as log:
            process = subprocess.Popen(
                command,
                cwd=HERE,
                env=os.environ.copy(),
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                encoding="utf-8",
                errors="replace",
            )
            assert process.stdout is not None
            for line in process.stdout:
                print(line, end="", flush=True)
                log.write(line)
            returncode = process.wait()
        summary_path = newest_summary(artifacts, summary_pattern)
        if summary_path is not None:
            parsed = json.loads(summary_path.read_text(encoding="utf-8"))
    except Exception as exc:
        error = f"{type(exc).__name__}: {exc}"
        with log_path.open("a", encoding="utf-8") as log:
            log.write(f"\n[suite-error] {error}\n")
    status = "passed" if returncode == 0 and summary_path is not None else "failed"
    result = {
        "name": name,
        "status": status,
        "returncode": returncode,
        "elapsed_seconds": time.time() - started,
        "command": command,
        "log": str(log_path),
        "summary": str(summary_path) if summary_path else "",
        "result": parsed,
        "error": error,
        "infrastructure_records": collect_records(parsed, "infrastructure"),
        "model_quality_records": collect_records(parsed, "model_quality"),
        "resource_observation_records": collect_records(parsed, "resource_observations"),
        "host_resource_metric_records": collect_records(parsed, "host_resource_metrics"),
        "fleet_resource_metric_records": collect_records(parsed, "fleet_resource_metrics"),
    }
    print(f"[suite] {name} {status} returncode={returncode}", flush=True)
    return result


def scale_resource_observation(
    scenario: dict[str, Any],
    current_workers: int,
    next_workers: int | None,
    config: dict[str, Any],
) -> dict[str, Any]:
    """Summarize measured fleet resource usage without turning it into a pass/fail gate."""
    candidates = [
        value for value in collect_records(scenario.get("result"), "fleet_resource_metrics")
        if isinstance(value, dict) and value
    ]
    if len(candidates) != 1:
        return {
            "observation_only": True,
            "available": False,
            "reason": f"expected one fleet metric record, found {len(candidates)}",
        }
    metrics = candidates[0]
    required = {
        "mem_total_bytes", "initial_mem_available_bytes", "min_mem_available_bytes", "peak_rss_bytes",
        "peak_processes", "peak_open_fds", "sample_count",
    }
    missing = sorted(required - metrics.keys())
    if missing:
        return {
            "observation_only": True,
            "available": False,
            "reason": f"fleet metrics missing fields: {missing}",
            "metrics": metrics,
        }
    minimum_available = int(config["minimum_mem_available_bytes"])
    available_ok = int(metrics["min_mem_available_bytes"]) >= minimum_available
    measured_available_drop = max(
        0,
        int(metrics["initial_mem_available_bytes"]) - int(metrics["min_mem_available_bytes"]),
    )
    projected_bytes = None
    projected_available_bytes = None
    projected_ok = True
    if next_workers is not None:
        # Summed RSS double-counts shared executable/library pages across the
        # fleet. Use the host-level MemAvailable drop for the safety decision;
        # retain peak_rss_bytes only as an observational metric.
        projected_bytes = int(measured_available_drop / current_workers * next_workers)
        projected_available_bytes = int(metrics["initial_mem_available_bytes"]) - projected_bytes
        projected_ok = (
            projected_bytes <= int(
                int(metrics["mem_total_bytes"]) * float(config["maximum_projected_host_memory_fraction"])
            )
            and projected_available_bytes >= minimum_available
        )
    return {
        "observation_only": True,
        "available": int(metrics["sample_count"]) > 0,
        "current_workers": current_workers,
        "next_workers": next_workers,
        "metrics": metrics,
        "minimum_mem_available_bytes": minimum_available,
        "measured_mem_available_drop_bytes": measured_available_drop,
        "projected_next_fleet_memory_bytes": projected_bytes,
        "projected_next_mem_available_bytes": projected_available_bytes,
        "maximum_projected_host_memory_fraction": config["maximum_projected_host_memory_fraction"],
        "available_memory_above_reference": available_ok,
        "projected_memory_above_reference": projected_ok,
        "reason": "resource metrics recorded for report only",
    }


def preflight(args: argparse.Namespace, config: dict[str, Any]) -> dict[str, Any]:
    if "UENV_PASS" not in os.environ:
        raise RuntimeError("UENV_PASS is required in the environment")
    import distributed_stress_runtime as base

    base.configure_from_args(args)
    server = base.connect(base.SERVER_HOST, os.environ["UENV_PASS"])
    worker = base.connect(base.WORKER_HOST, os.environ["UENV_PASS"])
    try:
        protected = base.protected_snapshot(server)
        base.assert_protected_unchanged(server, protected)
        source_and_binaries = base.source_and_binary_manifest(server, include_code_plugin=True)
        dataset_paths = {str(config["gate3"]["dataset_jsonl"])}
        if config["worker_scale"].get("enabled", False):
            dataset_paths.add(str(config["worker_scale"]["dataset_jsonl"]))
        dataset_paths = sorted(dataset_paths)
        datasets = {}
        for path in dataset_paths:
            base.run(server, f"test -f {base.q(path)}")
            _, dataset_hash, _ = base.run(server, f"sha256sum {base.q(path)}")
            datasets[path] = dataset_hash.split()[0]
        llm_config_sha256 = ""
        llm_config_mode = ""
        if config["gate4"].get("llm_kind") == "real":
            _, mode_text, _ = base.run(worker, f"stat -c %a {base.q(args.llm_config)}")
            if mode_text.strip() != "600":
                raise RuntimeError("real OpenHands LLM config must have mode 0600")
            _, hash_text, _ = base.run(worker, f"sha256sum {base.q(args.llm_config)}")
            llm_config_sha256 = hash_text.split()[0]
            llm_config_mode = "0600"
        return {
            "protected_server": protected,
            "source_and_binaries": source_and_binaries,
            "llm_config_path": args.llm_config,
            "llm_config_sha256": llm_config_sha256,
            "llm_config_mode": llm_config_mode,
            "llm_kind": config["gate4"].get("llm_kind"),
            "datasets": datasets,
        }
    finally:
        worker.close()
        server.close()


def assert_protected_after(args: argparse.Namespace, before: dict[str, Any]) -> dict[str, Any]:
    import distributed_stress_runtime as base

    base.configure_from_args(args)
    server = base.connect(base.SERVER_HOST, os.environ["UENV_PASS"])
    try:
        base.assert_protected_unchanged(server, before)
        return base.protected_snapshot(server)
    finally:
        server.close()


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--config", type=Path, default=DEFAULT_CONFIG)
    parser.add_argument("--artifacts", type=Path, default=Path.cwd() / "distributed-suite-artifacts")
    parser.add_argument("--execute", action="store_true", help="Actually run Gate3 then Gate4; omit for protected preflight only.")
    parser.add_argument("--private-worker-port-range", default="")
    parser.add_argument("--private-gateway-port-range", default="")
    parser.add_argument("--llm-config", default="")
    parser.add_argument("--source-repo", required=True)
    parser.add_argument("--server-bin", required=True)
    parser.add_argument("--worker-bin", required=True)
    parser.add_argument("--code-plugin-bin", required=True)
    parser.add_argument("--protected-pid", type=int, required=True)
    parser.add_argument("--protected-port", type=int, action="append", default=[])
    parser.add_argument("--server-host", default="8.130.75.157")
    parser.add_argument("--worker-host", default="8.130.86.71")
    parser.add_argument("--server-private-ip", default="192.168.0.136")
    parser.add_argument("--worker-private-ip", default="192.168.0.132")
    parser.add_argument("--server-port", type=int, default=8099)
    parser.add_argument("--worker-port", type=int, default=8000)
    parser.add_argument("--model-port", type=int, default=8888)
    parser.add_argument("--obs-port", type=int, default=18002)
    parser.add_argument("--gateway-port", type=int, default=8777)
    parser.add_argument("--agent-api-port", type=int, default=8077)
    parser.add_argument("--agent-health-port", type=int, default=8088)
    args = parser.parse_args()
    if not args.protected_port:
        args.protected_port = [50052, 8077, 8088]
    return args


def main() -> int:
    args = parse_args()
    # Child runners use HERE as cwd, so relative artifact roots would otherwise
    # be written under the source tree and become invisible to this collector.
    args.artifacts = args.artifacts.resolve()
    config = load_suite_config(args.config)
    validate_arguments(args, config)
    args.artifacts.mkdir(parents=True, exist_ok=True)
    suite_id = f"scale-stress-suite-{time.strftime('%Y%m%d-%H%M%S')}"
    suite_root = args.artifacts / suite_id
    suite_root.mkdir(parents=True, exist_ok=False)
    before = preflight(args, config)
    document: dict[str, Any] = {
        "schema_version": 1,
        "suite_id": suite_id,
        "status": "preflight_passed",
        "executed": args.execute,
        "config_path": str(args.config),
        "config_sha256": sha256_file(args.config),
        "config": config,
        "preflight": before,
        "scenarios": [],
    }
    summary_path = suite_root / "summary.json"
    if not args.execute:
        planned_commands: dict[str, Any] = {}
        if config["gate3"].get("enabled", True):
            planned_commands["gate3"] = gate3_command(args, config, suite_root / "gate3")
        if config["gate4"].get("enabled", True):
            planned_commands["gate4"] = gate4_command(args, config, suite_root / "gate4")
        if config["worker_scale"].get("enabled", False):
            planned_commands["worker_scale"] = [
                worker_scale_command(args, config, workers, suite_root / f"worker-scale-{workers:04d}")
                for workers in config["worker_scale"]["tiers"]
            ]
        document["planned_commands"] = planned_commands
        summary_path.write_text(json.dumps(document, indent=2, sort_keys=True), encoding="utf-8")
        print(f"[suite] preflight PASS summary={summary_path}")
        return 0

    try:
        if config["gate3"].get("enabled", True):
            document["scenarios"].append(run_child(
                "gate3-1024-simulator",
                gate3_command(args, config, suite_root / "gate3"),
                suite_root / "gate3",
                "gate3-summary-*.json",
            ))
        if config["gate4"].get("enabled", True):
            document["scenarios"].append(run_child(
                f"gate4-openhands-{config['gate4'].get('registered_workers', 1)}workers-trace-replay",
                gate4_command(args, config, suite_root / "gate4"),
                suite_root / "gate4",
                "gate4-summary-*.json",
            ))
        if config["worker_scale"].get("enabled", True):
            tiers = [int(value) for value in config["worker_scale"]["tiers"]]
            for index, workers in enumerate(tiers):
                scale_artifacts = suite_root / f"worker-scale-{workers:04d}"
                scenario = run_child(
                    f"worker-scale-{workers}",
                    worker_scale_command(args, config, workers, scale_artifacts),
                    scale_artifacts,
                    "gate3-summary-*.json",
                )
                document["scenarios"].append(scenario)
                if scenario["status"] != "passed":
                    break
                next_workers = tiers[index + 1] if index + 1 < len(tiers) else None
                resource_observation = scale_resource_observation(
                    scenario, workers, next_workers, config["worker_scale"]
                )
                scenario["resource_observation"] = resource_observation
    finally:
        document["protected_after"] = assert_protected_after(args, before["protected_server"])

    document["status"] = (
        "passed" if document["scenarios"] and all(item["status"] == "passed" for item in document["scenarios"])
        else "failed"
    )
    document["infrastructure"] = {
        "passed": document["status"] == "passed",
        "scenario_statuses": {item["name"]: item["status"] for item in document["scenarios"]},
        "protected_process_unchanged": True,
    }
    document["model_quality"] = {
        item["name"]: item["model_quality_records"] for item in document["scenarios"]
    }
    document["resource_observations"] = {
        item["name"]: {
            "resource_observations": item["resource_observation_records"],
            "host_resource_metrics": item["host_resource_metric_records"],
            "fleet_resource_metrics": item["fleet_resource_metric_records"],
            "observation_only": True,
        }
        for item in document["scenarios"]
    }
    summary_path.write_text(json.dumps(document, indent=2, sort_keys=True), encoding="utf-8")
    print(f"[suite] status={document['status']} summary={summary_path}")
    return 0 if document["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
