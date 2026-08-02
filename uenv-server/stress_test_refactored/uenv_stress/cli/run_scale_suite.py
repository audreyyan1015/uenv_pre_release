#!/usr/bin/env python3
"""规模压测套件编排入口。

这个文件把 DSCodeBench、规则任务和 SWE-bench Pro 的压测命令组合成一次可重复执行的规模压测。它的目标不是证明模型质量，而是验证 Server、Worker、调度、Episode 记录、资源采样和清理逻辑在多场景压力下是否符合预期。

实现逻辑是：先读取 scale_suite.json 和命令行参数，统一校验运行目录、端口范围、worker 主机、LLM 类型、数据集路径和生产保护快照；再分别生成 DSCodeBench、Math/Science 规则任务、SWE-bench Pro 压测或轨迹采集的子命令；每个子命令在隔离目录中运行，完成后收集 summary、episode observation、资源记录和清理记录；最后做场景覆盖检查、资源门限检查和生产进程未受影响检查。"""

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

from uenv_stress.core.runtime_config import load_runtime_inventory
from uenv_stress.core import suite_metrics


PACKAGE_ROOT = Path(__file__).resolve().parents[2]
CONFIG_DIR = PACKAGE_ROOT / "uenv_stress" / "config"
DEFAULT_CONFIG = CONFIG_DIR / "scale_suite.json"
DEFAULT_TRACE_CONFIG = CONFIG_DIR / "trace_collection.json"
DEFAULT_RUNTIME_CONFIG = CONFIG_DIR / "runtime_hosts.json"
ALLOWED_EXPOSED_PORTS = {
    5432, 6379, 8000, 8077, 8088, 8099, 8777, 8888, 22000
}
PARALLEL_MODES = {"sync", "one_step_off_policy", "fully_async"}
SCALE_DATASETS = {
    "dscodebench",
    "swebench_pro",
    "olymmath",
    "scitab",
    "pubmedqa",
}
MATH_RULE_TASKS = {"olymmath", "scitab", "pubmedqa"}



def load_suite_config(
    path: Path,
    trace_config_path: Path = DEFAULT_TRACE_CONFIG,
) -> dict[str, Any]:
    document = json.loads(path.read_text(encoding="utf-8"))
    if document.get("schema_version") != 1:
        raise ValueError("stress suite schema_version must be 1")
    trace_document = json.loads(trace_config_path.read_text(encoding="utf-8"))
    if trace_document.get("schema_version") != 1:
        raise ValueError("trace collection schema_version must be 1")
    document["trace_collection"] = {
        key: value
        for key, value in trace_document.items()
        if key != "schema_version"
    }
    dscodebench_pressure = document.get("dscodebench_pressure")
    swebench_pro_pressure = document.get("swebench_pro_pressure")
    math_rule_pressure = document.get("math_rule_pressure")
    worker_scale = document.get("worker_scale")
    trace_collection = document.get("trace_collection")
    if (
        not isinstance(dscodebench_pressure, dict)
        or not isinstance(swebench_pro_pressure, dict)
        or not isinstance(math_rule_pressure, dict)
        or not isinstance(worker_scale, dict)
    ):
        raise ValueError(
            "stress suite requires dscodebench_pressure, swebench_pro_pressure, "
            "math_rule_pressure and worker_scale objects"
        )
    if not isinstance(trace_collection, dict):
        raise ValueError("stress suite requires a trace_collection object")
    dscodebench_collection = trace_collection.get("dscodebench")
    swe_collection = trace_collection.get("swebench_pro")
    math_collection = trace_collection.get("math_rule_tasks")
    if (
        not isinstance(dscodebench_collection, dict)
        or not isinstance(swe_collection, dict)
        or not isinstance(math_collection, dict)
    ):
        raise ValueError(
            "trace_collection requires dscodebench, swebench_pro and math_rule_tasks objects"
        )
    if int(dscodebench_collection.get("dataset_count", 0)) != 100:
        raise ValueError("DSCodeBench real-LLM trace collection must sample exactly 100 records")
    if int(dscodebench_collection.get("collection_concurrency", 0)) != 100:
        raise ValueError("DSCodeBench real-LLM trace collection concurrency must be 100")
    if bool(dscodebench_collection.get("uses_1024_workers", True)):
        raise ValueError("DSCodeBench trace collection must not use 1024 Workers")
    if int(swe_collection.get("instance_count", 0)) != 50:
        raise ValueError("SWE-bench Pro real-LLM trace collection must sample exactly 50 instances")
    swe_concurrency = int(swe_collection.get("collection_concurrency", 0))
    if not 1 <= swe_concurrency <= 50:
        raise ValueError("SWE-bench Pro real-LLM trace collection concurrency must be in [1, 50]")
    if int(swe_collection.get("target_valid_traces", 50)) != 50:
        raise ValueError("SWE-bench Pro trace collection target_valid_traces must be 50")
    if bool(swe_collection.get("uses_1024_workers", True)):
        raise ValueError("SWE-bench Pro trace collection must not use 1024 Workers")
    if str(swe_collection.get("source_model", "doubao")).lower() != "doubao":
        raise ValueError("SWE-bench Pro trace collection must use Doubao as source_model")
    if not str(swe_collection.get("trace_corpus_path", "")).strip():
        raise ValueError("SWE-bench Pro trace collection requires trace_corpus_path")
    if set(math_collection.get("datasets", [])) != MATH_RULE_TASKS:
        raise ValueError("Math trace collection must cover exactly all three rule tasks")
    if int(math_collection.get("samples_per_dataset", 0)) < 100:
        raise ValueError("Math trace collection requires at least 100 traces per dataset")
    if bool(math_collection.get("uses_1024_workers", True)):
        raise ValueError("Math trace collection must remain separate from 1024-Worker pressure")
    modes = dscodebench_pressure.get("modes")
    if not isinstance(modes, list) or not modes or set(modes) - PARALLEL_MODES:
        raise ValueError(f"invalid DSCodeBench pressure modes: {modes!r}")
    if dscodebench_pressure.get("model_mode") != "simulator":
        raise ValueError("scale stress requires DSCodeBench pressure model_mode=simulator")
    dscodebench_pressure_simulator_mode = str(dscodebench_pressure.get("simulator_mode", "template"))
    if dscodebench_pressure_simulator_mode not in {"template", "trace_replay"}:
        raise ValueError("DSCodeBench pressure simulator_mode must be template or trace_replay")
    if dscodebench_pressure_simulator_mode != "trace_replay":
        raise ValueError("DSCodeBench pressure pressure evidence requires simulator_mode=trace_replay")
    if not str(dscodebench_pressure.get("trace_corpus_path", "")).strip():
        raise ValueError("DSCodeBench pressure trace_replay requires trace_corpus_path")
    dscodebench_pressure_sampling = str(dscodebench_pressure.get("trace_sampling_strategy", ""))
    if dscodebench_pressure_sampling != "round_robin_episode":
        raise ValueError("DSCodeBench pressure requires trace_sampling_strategy=round_robin_episode")
    if int(dscodebench_pressure.get("workers", 0)) < 1024:
        raise ValueError("DSCodeBench pressure pressure evidence requires at least 1024 Workers")
    capacity = int(dscodebench_pressure.get("capacity_per_worker", 0))
    batch_size = int(dscodebench_pressure.get("episode_batch_size", dscodebench_pressure.get("workers", 0)))
    exact_batches = int(dscodebench_pressure.get("exact_batches_per_mode", 0))
    min_waves = int(dscodebench_pressure.get("min_episode_waves", 10))
    if batch_size * exact_batches < int(dscodebench_pressure["workers"]) * capacity * min_waves:
        raise ValueError("DSCodeBench pressure total episodes per mode must be at least workers * capacity * min_episode_waves")
    if swebench_pro_pressure.get("mode") != "llm":
        raise ValueError("the integrated acceptance suite requires SWE-bench Pro pressure mode=llm")
    swebench_pro_pressure_parallel_modes = swebench_pro_pressure.get("parallel_modes")
    # 允许只跑子集（例如验收阶段只跑 sync），但必须非空且取值合法。
    if (
        not isinstance(swebench_pro_pressure_parallel_modes, list)
        or not swebench_pro_pressure_parallel_modes
        or set(swebench_pro_pressure_parallel_modes) - PARALLEL_MODES
    ):
        raise ValueError(f"invalid SWE-bench Pro pressure parallel_modes: {swebench_pro_pressure_parallel_modes!r}")
    llm_kind = str(swebench_pro_pressure.get("llm_kind", "simulator"))
    if llm_kind not in {"simulator", "real"}:
        raise ValueError("SWE-bench Pro pressure llm_kind must be simulator or real")
    if llm_kind == "simulator":
        simulator_mode = str(swebench_pro_pressure.get("simulator_mode", "template"))
        if simulator_mode not in {"template", "trace_replay"}:
            raise ValueError("SWE-bench Pro pressure simulator_mode must be template or trace_replay")
        if simulator_mode == "trace_replay" and not str(swebench_pro_pressure.get("trace_corpus_path", "")).strip():
            raise ValueError("SWE-bench Pro pressure trace_replay requires trace_corpus_path")
        sampling = str(swebench_pro_pressure.get("trace_sampling_strategy", ""))
        if sampling != "round_robin_episode":
            raise ValueError("SWE-bench Pro pressure requires trace_sampling_strategy=round_robin_episode")
    concurrencies = swebench_pro_pressure.get("concurrencies")
    if not isinstance(concurrencies, list) or not concurrencies or any(int(value) <= 0 for value in concurrencies):
        raise ValueError("SWE-bench Pro pressure concurrencies must be positive integers")
    agents_per_node = int(swebench_pro_pressure.get("agents_per_node", 1))
    if agents_per_node <= 0:
        raise ValueError("SWE-bench Pro pressure agents_per_node must be positive")
    if int(swebench_pro_pressure.get("instance_count", 0)) < 50:
        raise ValueError("SWE-bench Pro pressure coverage requires at least 50 sampled instances")
    for field in ("dataset_catalog", "instance_list"):
        if not str(swebench_pro_pressure.get(field, "")).strip():
            raise ValueError(f"SWE-bench Pro pressure requires {field}")
    swebench_pro_pressure_workers = int(swebench_pro_pressure.get("registered_workers", 1))
    swebench_pro_pressure_capacity = int(swebench_pro_pressure.get("worker_capacity", 1))
    swebench_pro_pressure_waves = int(swebench_pro_pressure.get("min_episode_waves", 10))
    swebench_pro_pressure_total_episodes = int(swebench_pro_pressure.get("total_episodes", 0))
    # SWE Worker 下限按验收要求可下调（1024 -> 64）；下限以下的规模不接受。
    if swebench_pro_pressure_workers < 64:
        raise ValueError("SWE-bench Pro pressure scale evidence requires at least 64 registered Workers")
    if swebench_pro_pressure_capacity < 1:
        raise ValueError("SWE-bench Pro pressure worker_capacity must be positive")
    if swebench_pro_pressure_total_episodes < swebench_pro_pressure_workers * swebench_pro_pressure_capacity * swebench_pro_pressure_waves:
        raise ValueError("SWE-bench Pro pressure total_episodes must be at least registered_workers * worker_capacity * min_episode_waves")
    if llm_kind != "simulator" or str(swebench_pro_pressure.get("simulator_mode", "")) != "trace_replay":
        raise ValueError("SWE-bench Pro pressure 1024 Worker scale requires simulator trace_replay")
    math_tasks = math_rule_pressure.get("tasks")
    if not isinstance(math_tasks, dict) or set(math_tasks) != MATH_RULE_TASKS:
        raise ValueError(
            "math_rule_pressure.tasks must contain exactly olymmath, scitab and pubmedqa"
        )
    # 与 dscodebench 一致：允许只跑子集（例如验收阶段只跑 sync）。
    math_modes = math_rule_pressure.get("modes", [])
    if not isinstance(math_modes, list) or not math_modes or set(math_modes) - PARALLEL_MODES:
        raise ValueError(f"invalid math_rule_pressure modes: {math_modes!r}")
    if math_rule_pressure.get("model_mode") != "trace_replay_simulator":
        raise ValueError("math_rule_pressure requires model_mode=trace_replay_simulator")
    if math_rule_pressure.get("trace_sampling_strategy") != "round_robin_episode":
        raise ValueError("math_rule_pressure requires trace_sampling_strategy=round_robin_episode")
    math_workers = int(math_rule_pressure.get("workers", 0))
    math_capacity = int(math_rule_pressure.get("capacity_per_worker", 0))
    math_batch_size = int(math_rule_pressure.get("episode_batch_size", 0))
    math_batches = int(math_rule_pressure.get("exact_batches_per_dataset_mode", 0))
    math_waves = int(math_rule_pressure.get("min_episode_waves", 0))
    if math_workers < 1024:
        raise ValueError("math_rule_pressure scale evidence requires at least 1024 Workers")
    if math_capacity < 1 or math_batch_size < 1 or math_batches < 1:
        raise ValueError("math_rule_pressure capacity and batch values must be positive")
    if math_batch_size * math_batches < math_workers * math_capacity * math_waves:
        raise ValueError(
            "every Math rule dataset/mode must submit at least "
            "workers * capacity_per_worker * min_episode_waves episodes"
        )
    for task, task_config in math_tasks.items():
        if not isinstance(task_config, dict):
            raise ValueError(f"math_rule_pressure.tasks.{task} must be an object")
        for field in ("dataset_path", "trace_corpus_path"):
            if not str(task_config.get(field, "")).strip():
                raise ValueError(f"math_rule_pressure.tasks.{task} requires {field}")
        if int(task_config.get("dataset_limit", 0)) < 100:
            raise ValueError(
                f"math_rule_pressure.tasks.{task}.dataset_limit must be at least 100"
            )
    min_steps = int(dscodebench_pressure.get("min_steps", 0))
    max_steps = int(dscodebench_pressure.get("max_steps", 0))
    if min_steps < 2 or max_steps < min_steps:
        raise ValueError("DSCodeBench pressure requires 2 <= min_steps <= max_steps")
    if not worker_scale.get("enabled", False):
        return document
    if worker_scale.get("model_mode") != "trace_replay_simulator":
        raise ValueError("worker_scale must explicitly use trace_replay_simulator")
    worker_scale_simulator_mode = str(worker_scale.get("simulator_mode", "trace_replay"))
    if worker_scale_simulator_mode != "trace_replay":
        raise ValueError("worker_scale requires simulator_mode=trace_replay")
    if not str(worker_scale.get("trace_corpus_path", "")).strip():
        raise ValueError("worker_scale trace_replay requires trace_corpus_path")
    worker_scale_sampling = str(worker_scale.get("trace_sampling_strategy", ""))
    if worker_scale_sampling != "round_robin_episode":
        raise ValueError("worker_scale requires trace_sampling_strategy=round_robin_episode")
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


def effective_llm_kind(args: argparse.Namespace, config: dict[str, Any]) -> str:
    if getattr(args, "scenario", "") == "swebench-pro-trace-collection":
        return "real"
    return str(config["swebench_pro_pressure"].get("llm_kind", "simulator"))


def validate_arguments(args: argparse.Namespace, config: dict[str, Any]) -> None:
    for label in (
        "source_repo",
        "server_bin",
        "worker_bin",
        "code_plugin_bin",
        "math_plugin_bin",
    ):
        require_absolute(f"--{label.replace('_', '-')}", str(getattr(args, label)))
    scenario = getattr(args, "scenario", "")
    if (
        config["swebench_pro_pressure"].get("llm_kind") == "real"
        or scenario == "swebench-pro-trace-collection"
    ):
        require_absolute("--llm-config", str(args.llm_config))
    swebench_pro_pressure = config["swebench_pro_pressure"]
    swebench_pro_pressure_model_port = int(swebench_pro_pressure.get("model_port", args.model_port))
    swebench_pro_pressure_gateway_port = int(swebench_pro_pressure.get("gateway_port", args.gateway_port))
    swebench_pro_pressure_agent_api_port = int(swebench_pro_pressure.get("agent_api_port", args.agent_api_port))
    swebench_pro_pressure_agent_health_port = int(swebench_pro_pressure.get("agent_health_port", args.agent_health_port))
    math_rule_model_port = int(
        config["math_rule_pressure"].get("model_port", args.model_port)
    )
    if scenario == "swebench-pro-trace-collection":
        collection = config["trace_collection"]["swebench_pro"]
        collection_gateway_port = int(collection.get("gateway_port", args.gateway_port))
        collection_agent_api_port = int(collection.get("agent_api_port", args.agent_api_port))
        collection_agent_health_port = int(collection.get("agent_health_port", args.agent_health_port))
        exposed_ports = {
            "server": args.server_port,
            "worker": args.worker_port,
            "gateway": collection_gateway_port,
            "model": swebench_pro_pressure_model_port,
        }
        for label, port in exposed_ports.items():
            if port not in ALLOWED_EXPOSED_PORTS:
                raise ValueError(f"{label} port {port} is not in the explicitly allowed exposed-port set")
        overlap = set(args.protected_port) & {args.server_port, swebench_pro_pressure_model_port}
        if overlap:
            raise ValueError(f"isolated trace collection ports overlap protected ports: {sorted(overlap)}")
        return
    swebench_pro_pressure_workers = int(swebench_pro_pressure.get("registered_workers", 1))
    dscodebench_pressure_workers = int(config["dscodebench_pressure"].get("workers", 1))
    math_rule_workers = int(config["math_rule_pressure"].get("workers", 1))
    exposed_ports = {
        "server": args.server_port,
        "model": swebench_pro_pressure_model_port,
        "Math model": math_rule_model_port,
    }
    if (
        dscodebench_pressure_workers == 1
        and swebench_pro_pressure_workers == 1
        and math_rule_workers == 1
    ):
        exposed_ports["worker"] = args.worker_port
        exposed_ports["gateway"] = swebench_pro_pressure_gateway_port
    for label, port in exposed_ports.items():
        if port not in ALLOWED_EXPOSED_PORTS:
            raise ValueError(f"{label} port {port} is not in the explicitly allowed exposed-port set")
    protected = set(args.protected_port)
    requested = {
        args.server_port,
        swebench_pro_pressure_model_port,
        math_rule_model_port,
    }
    if (
        dscodebench_pressure_workers == 1
        and swebench_pro_pressure_workers == 1
        and math_rule_workers == 1
    ):
        requested.update({args.worker_port, swebench_pro_pressure_gateway_port})
    overlap = protected & requested
    if overlap:
        raise ValueError(f"isolated suite ports overlap protected ports: {sorted(overlap)}")
    dscodebench_pressure = config["dscodebench_pressure"]
    workers = int(dscodebench_pressure["workers"])
    worker_scale = config["worker_scale"]
    max_scale_workers = max(
        [workers, math_rule_workers]
        + [int(value) for value in worker_scale.get("tiers", [])]
    )
    scale_model_port = int(worker_scale.get("model_port", args.model_port))
    swebench_agent_api_end = swebench_pro_pressure_agent_api_port + int(swebench_pro_pressure.get("agents_per_node", 1)) - 1
    swebench_agent_health_end = swebench_pro_pressure_agent_health_port + int(swebench_pro_pressure.get("agents_per_node", 1)) - 1
    for label, start, end in (
        ("agent API", swebench_pro_pressure_agent_api_port, swebench_agent_api_end),
        ("agent health", swebench_pro_pressure_agent_health_port, swebench_agent_health_end),
    ):
        if not 1 <= start <= end <= 65535:
            raise ValueError(f"{label} port range {start}-{end} is outside TCP port range")
    if worker_scale.get("enabled", False):
        if scale_model_port not in ALLOWED_EXPOSED_PORTS:
            raise ValueError(
                f"worker-scale model port {scale_model_port} is not in the explicitly allowed exposed-port set"
            )
        if scale_model_port in protected:
            raise ValueError(f"worker-scale model port {scale_model_port} overlaps a protected port")
    if (workers > 1 or config["worker_scale"].get("enabled", True)) and not args.private_worker_port_range:
        raise ValueError("multi-Worker execution requires --private-worker-port-range")
    private_ranges: list[tuple[str, int, int]] = []
    if args.private_worker_port_range:
        start, end = parse_port_range("--private-worker-port-range", args.private_worker_port_range)
        private_ranges.append(("Worker", start, end))
        if start != args.worker_port or end - start + 1 < max_scale_workers or end > 65535:
            raise ValueError(
                f"private Worker range must start at {args.worker_port} and contain at least {max_scale_workers} ports"
            )
        if start <= swebench_pro_pressure_model_port <= end:
            raise ValueError(
                f"model port {swebench_pro_pressure_model_port} overlaps the private Worker port range {start}-{end}"
            )
        if start <= math_rule_model_port <= end:
            raise ValueError(
                f"Math model port {math_rule_model_port} overlaps the private Worker port range {start}-{end}"
            )
        for label, left, right in (
            ("agent API", swebench_pro_pressure_agent_api_port, swebench_agent_api_end),
            ("agent health", swebench_pro_pressure_agent_health_port, swebench_agent_health_end),
        ):
            if max(start, left) <= min(end, right):
                raise ValueError(f"{label} port range {left}-{right} overlaps the private Worker port range {start}-{end}")
        if start <= scale_model_port < start + max_scale_workers:
            raise ValueError(
                f"worker-scale model port {scale_model_port} overlaps the {max_scale_workers}-Worker port range"
            )
    if int(config["swebench_pro_pressure"].get("registered_workers", 1)) > 1 and not getattr(args, "private_gateway_port_range", ""):
        raise ValueError("SWE-bench Pro pressure multi-Worker execution requires --private-gateway-port-range")
    if args.private_gateway_port_range:
        swebench_pro_pressure_workers = int(config["swebench_pro_pressure"].get("registered_workers", 1))
        start, end = parse_port_range("--private-gateway-port-range", args.private_gateway_port_range)
        private_ranges.append(("Gateway", start, end))
        if start != swebench_pro_pressure_gateway_port or end - start + 1 < swebench_pro_pressure_workers:
            raise ValueError(
                f"private Gateway range must start at {swebench_pro_pressure_gateway_port} and contain at least {swebench_pro_pressure_workers} ports"
            )
        for label, port in {
            "server": args.server_port,
            "model": swebench_pro_pressure_model_port,
            "Math model": math_rule_model_port,
            "worker-scale model": scale_model_port,
        }.items():
            if start <= port <= end:
                raise ValueError(f"{label} port {port} overlaps the private Gateway port range {start}-{end}")
        for label, left, right in (
            ("agent API", swebench_pro_pressure_agent_api_port, swebench_agent_api_end),
            ("agent health", swebench_pro_pressure_agent_health_port, swebench_agent_health_end),
        ):
            if max(start, left) <= min(end, right):
                raise ValueError(f"{label} port range {left}-{right} overlaps the private Gateway port range {start}-{end}")
    obs_start = args.obs_port
    obs_end = args.obs_port + max(int(config["swebench_pro_pressure"].get("registered_workers", 1)), max_scale_workers) - 1
    for label, port in {
        "model": swebench_pro_pressure_model_port,
        "Math model": math_rule_model_port,
    }.items():
        if obs_start <= port <= obs_end:
            raise ValueError(f"{label} port {port} overlaps the Observability port range {obs_start}-{obs_end}")
    for label, left, right in (
        ("agent API", swebench_pro_pressure_agent_api_port, swebench_agent_api_end),
        ("agent health", swebench_pro_pressure_agent_health_port, swebench_agent_health_end),
    ):
        if max(obs_start, left) <= min(obs_end, right):
            raise ValueError(f"{label} port range {left}-{right} overlaps the Observability port range {obs_start}-{obs_end}")
    private_ranges.append(("Agent API", swebench_pro_pressure_agent_api_port, swebench_agent_api_end))
    private_ranges.append(("Agent health", swebench_pro_pressure_agent_health_port, swebench_agent_health_end))
    private_ranges.append(("Observability", obs_start, obs_end))
    for left_index, (left_label, left_start, left_end) in enumerate(private_ranges):
        for right_label, right_start, right_end in private_ranges[left_index + 1:]:
            if max(left_start, right_start) <= min(left_end, right_end):
                raise ValueError(
                    f"{left_label} port range {left_start}-{left_end} overlaps "
                    f"{right_label} port range {right_start}-{right_end}"
                )


def common_child_args(
    args: argparse.Namespace,
    *,
    model_port: int | None = None,
    plugin_bin: str | None = None,
) -> list[str]:
    command = [
        "--source-repo", args.source_repo,
        "--server-bin", args.server_bin,
        "--worker-bin", args.worker_bin,
        "--code-plugin-bin", args.code_plugin_bin if plugin_bin is None else plugin_bin,
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
    # --worker-node 可重复传入多台 worker（HOST:PRIVATE_IP），透传给子脚本；
    # 子脚本收到后以它为准，覆盖上面的 --worker-host/--worker-private-ip。
    for worker_node in getattr(args, "worker_node", None) or []:
        command.extend(["--worker-node", worker_node])
    return command


def dscodebench_pressure_command(args: argparse.Namespace, config: dict[str, Any], artifacts: Path) -> list[str]:
    gate = config["dscodebench_pressure"]
    command = [
        sys.executable,
        "-m",
        "uenv_stress.scale.dscodebench_pressure",
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
        "--simulator-seed", str(gate["simulator_seed"]),
        "--simulator-mode", str(gate.get("simulator_mode", "trace_replay")),
        "--trace-corpus-path", str(gate.get("trace_corpus_path", "")),
        "--trace-sampling-strategy", str(gate["trace_sampling_strategy"]),
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


def math_rule_pressure_command(
    args: argparse.Namespace,
    config: dict[str, Any],
    artifacts: Path,
) -> list[str]:
    gate = config["math_rule_pressure"]
    command = [
        sys.executable,
        "-m",
        "uenv_stress.scale.rule_task_pressure",
        "--workers", str(gate["workers"]),
        "--capacity", str(gate["capacity_per_worker"]),
        "--episode-batch-size", str(gate["episode_batch_size"]),
        "--concurrent-batches", str(gate["concurrent_batches"]),
        "--exact-batches", str(gate["exact_batches_per_dataset_mode"]),
        "--min-scale-episode-waves", str(gate["min_episode_waves"]),
        "--registration-timeout", str(gate["registration_timeout_seconds"]),
        "--batch-timeout", str(gate["batch_timeout_seconds"]),
        "--plugin-ready-timeout-seconds", str(gate["plugin_ready_timeout_seconds"]),
        "--worker-register-max-attempts", str(gate["worker_register_max_attempts"]),
        "--worker-register-retry-backoff-ms", str(gate["worker_register_retry_backoff_ms"]),
        "--simulator-seed", str(gate["simulator_seed"]),
        "--trace-sampling-strategy", str(gate["trace_sampling_strategy"]),
        "--evidence-boundary", str(gate["evidence_boundary"]),
        "--private-worker-port-range", args.private_worker_port_range,
        "--artifacts", str(artifacts),
    ]
    for mode in gate["modes"]:
        command.extend(["--mode", mode])
    for task in sorted(MATH_RULE_TASKS):
        task_config = gate["tasks"][task]
        command.extend([
            f"--{task}-dataset", str(task_config["dataset_path"]),
            f"--{task}-dataset-limit", str(task_config["dataset_limit"]),
            f"--{task}-trace", str(task_config["trace_corpus_path"]),
        ])
    return command + common_child_args(
        args,
        model_port=int(gate.get("model_port", args.model_port)),
        plugin_bin=args.math_plugin_bin,
    )


def worker_scale_command(
    args: argparse.Namespace,
    config: dict[str, Any],
    workers: int,
    artifacts: Path,
) -> list[str]:
    gate = config["worker_scale"]
    episode_batch_size = int(gate["episode_batch_size"])
    exact_batches = workers * int(gate["episodes_per_worker"]) // episode_batch_size
    # 所有计划 batch 立即交给 server；server 的 admission/scheduler 负责排队，
    # 不能按一轮 worker 容量截断 backlog。
    concurrent_batches = exact_batches
    command = [
        sys.executable,
        "-m",
        "uenv_stress.scale.dscodebench_pressure",
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
        "--simulator-seed", str(gate["simulator_seed"]),
        "--simulator-mode", str(gate.get("simulator_mode", "trace_replay")),
        "--trace-corpus-path", str(gate.get("trace_corpus_path", "")),
        "--trace-sampling-strategy", str(gate["trace_sampling_strategy"]),
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


def swebench_pro_pressure_command(args: argparse.Namespace, config: dict[str, Any], artifacts: Path) -> list[str]:
    gate = config["swebench_pro_pressure"]
    model_port = int(gate.get("model_port", args.model_port))
    gateway_port = int(gate.get("gateway_port", args.gateway_port))
    agent_api_port = int(gate.get("agent_api_port", args.agent_api_port))
    agent_health_port = int(gate.get("agent_health_port", args.agent_health_port))
    command = [
        sys.executable,
        "-m",
        "uenv_stress.scale.swebench_pro_pressure",
        "--mode", "llm",
        "--llm-kind", str(gate["llm_kind"]),
        "--max-steps", str(gate["max_steps"]),
        "--openhands-max-iterations", str(gate["openhands_max_iterations"]),
        "--instance-count", str(gate["instance_count"]),
        "--instance-seed", str(gate["instance_seed"]),
        "--dataset-catalog", str(gate["dataset_catalog"]),
        "--instance-list", str(gate["instance_list"]),
        "--registered-workers", str(gate.get("registered_workers", 1)),
        "--worker-capacity", str(gate.get("worker_capacity", 1)),
        "--total-episodes", str(gate.get("total_episodes", 0)),
        "--episode-batch-size", str(gate.get("episode_batch_size", 0)),
        "--agents-per-node", str(gate.get("agents_per_node", 1)),
        "--min-scale-episode-waves", str(gate.get("min_episode_waves", 10)),
        "--fleet-supervisor-threshold", str(gate.get("fleet_supervisor_threshold", 16)),
        "--registration-timeout", str(gate.get("registration_timeout_seconds", 900)),
        "--batch-timeout", str(gate.get("batch_timeout_seconds", 1800)),
        "--simulator-seed", str(gate["simulator_seed"]),
        "--simulator-mode", str(gate.get("simulator_mode", "template")),
        "--trace-corpus-path", str(gate.get("trace_corpus_path", "")),
        "--trace-sampling-strategy", str(gate["trace_sampling_strategy"]),
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


def swebench_pro_trace_collection_command(args: argparse.Namespace, config: dict[str, Any], artifacts: Path) -> list[str]:
    gate = config["swebench_pro_pressure"]
    collection = config["trace_collection"]["swebench_pro"]
    concurrency = int(collection["collection_concurrency"])
    trace_corpus_path = str(collection["trace_corpus_path"])
    source_model = str(collection.get("source_model", "doubao"))
    source_version = str(collection.get("source_version", ""))
    model_port = int(gate.get("model_port", args.model_port))
    gateway_port = int(collection.get("gateway_port", args.gateway_port))
    agent_api_port = int(collection.get("agent_api_port", args.agent_api_port))
    agent_health_port = int(collection.get("agent_health_port", args.agent_health_port))
    command = [
        sys.executable,
        "-m",
        "uenv_stress.scale.swebench_pro_pressure",
        "--mode", "llm",
        "--llm-kind", "real",
        "--llm-config", args.llm_config,
        "--max-steps", str(gate["max_steps"]),
        "--openhands-max-iterations", str(gate["openhands_max_iterations"]),
        "--instance-count", str(collection["instance_count"]),
        "--instance-seed", str(gate["instance_seed"]),
        "--dataset-catalog", str(gate["dataset_catalog"]),
        "--instance-list", str(gate["instance_list"]),
        "--registered-workers", "1",
        "--worker-capacity", str(concurrency),
        "--total-episodes", str(collection["instance_count"]),
        "--episode-batch-size", str(concurrency),
        "--min-scale-episode-waves", "1",
        "--fleet-supervisor-threshold", str(gate.get("fleet_supervisor_threshold", 16)),
        "--registration-timeout", str(gate.get("registration_timeout_seconds", 900)),
        "--batch-timeout", str(gate.get("batch_timeout_seconds", 1800)),
        "--simulator-seed", str(gate["simulator_seed"]),
        "--simulator-mode", "template",
        "--trace-source-model", source_model,
        "--trace-source-version", source_version,
        "--freeze-trace-corpus-path", trace_corpus_path,
        "--freeze-require-complete",
        "--gateway-port", str(gateway_port),
        "--agent-api-port", str(agent_api_port),
        "--agent-health-port", str(agent_health_port),
        "--parallel-mode", "fully_async",
        "--concurrency", str(concurrency),
        "--artifacts", str(artifacts),
    ]
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


def scenario_dataset_coverage(scenarios: list[dict[str, Any]]) -> set[str]:
    covered: set[str] = set()
    for scenario in scenarios:
        if scenario.get("status") != "passed":
            continue
        name = str(scenario.get("name", ""))
        if name.startswith("dscodebench-pressure"):
            covered.add("dscodebench")
        if name.startswith("swebench-pro-pressure"):
            covered.add("swebench_pro")
        if name.startswith("math-rule-pressure"):
            for value in collect_records(scenario.get("result"), "datasets"):
                if isinstance(value, list):
                    covered.update(str(item) for item in value)
    return covered


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
                cwd=PACKAGE_ROOT,
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


def scale_resource_gate(
    scenario: dict[str, Any],
    current_workers: int,
    next_workers: int | None,
    config: dict[str, Any],
) -> dict[str, Any]:
    """Compatibility entrypoint used by preflight tests and staged scale runs."""
    result = scale_resource_observation(scenario, current_workers, next_workers, config)
    result["passed"] = bool(
        result.get("available")
        and result.get("available_memory_above_reference")
        and result.get("projected_memory_above_reference")
    )
    return result


def preflight(args: argparse.Namespace, config: dict[str, Any]) -> dict[str, Any]:
    if "UENV_PASS" not in os.environ:
        raise RuntimeError("UENV_PASS is required in the environment")
    from uenv_stress.core import distributed_runtime as base

    base.configure_from_args(args)
    server = base.connect(base.SERVER_HOST, os.environ["UENV_PASS"])
    # 每个 worker 节点一条连接；单节点时行为与改造前一致。
    worker_clients = base.connect_worker_nodes(os.environ["UENV_PASS"])
    try:
        protected = base.protected_snapshot(server)
        base.assert_protected_unchanged(server, protected)
        scenario = getattr(args, "scenario", "suite")
        include_code_plugin = scenario in {"suite", "dscodebench-pressure"}
        source_and_binaries = base.source_and_binary_manifest(
            server,
            include_code_plugin=include_code_plugin,
        )
        if scenario in {"suite", "math-rule-pressure"}:
            base.run(server, f"test -x {base.q(args.math_plugin_bin)}")
            _, math_hash, _ = base.run(
                server, f"sha256sum {base.q(args.math_plugin_bin)}"
            )
            source_and_binaries["binaries"]["math_plugin"] = {
                "path": args.math_plugin_bin,
                "sha256": math_hash.split()[0],
            }
        if scenario == "swebench-pro-trace-collection":
            dataset_paths = {
                str(config["swebench_pro_pressure"]["dataset_catalog"]),
                str(config["swebench_pro_pressure"]["instance_list"]),
            }
        else:
            dataset_paths = set()
            if scenario in {"suite", "dscodebench-pressure"}:
                dataset_paths.add(str(config["dscodebench_pressure"]["dataset_jsonl"]))
            if scenario in {"suite", "swebench-pro-pressure"}:
                dataset_paths.update({
                    str(config["swebench_pro_pressure"]["dataset_catalog"]),
                    str(config["swebench_pro_pressure"]["instance_list"]),
                })
            if scenario in {"suite", "math-rule-pressure"}:
                dataset_paths.update(
                    str(value["dataset_path"])
                    for value in config["math_rule_pressure"]["tasks"].values()
                )
            if (
                scenario in {"suite", "dscodebench-pressure"}
                and config["worker_scale"].get("enabled", False)
            ):
                dataset_paths.add(str(config["worker_scale"]["dataset_jsonl"]))
        dataset_paths = sorted(dataset_paths)
        datasets = {}
        for path in dataset_paths:
            base.run(server, f"test -e {base.q(path)}")
            status, _, _ = base.run(server, f"test -d {base.q(path)}", check=False)
            if status == 0:
                _, listing, _ = base.run(
                    server,
                    f"find {base.q(path)} -type f -print0 | sort -z | xargs -0 sha256sum",
                )
                datasets[path] = hashlib.sha256(listing.encode()).hexdigest()
            else:
                _, dataset_hash, _ = base.run(server, f"sha256sum {base.q(path)}")
                datasets[path] = dataset_hash.split()[0]
        trace_corpora = {}
        if scenario in {"suite", "math-rule-pressure"}:
            for host, worker in worker_clients.items():
                trace_corpora[host] = {}
                for task, task_config in config["math_rule_pressure"]["tasks"].items():
                    path = str(task_config["trace_corpus_path"])
                    base.run(worker, f"test -f {base.q(path)}")
                    _, trace_hash, _ = base.run(worker, f"sha256sum {base.q(path)}")
                    trace_corpora[host][task] = {
                        "path": path,
                        "sha256": trace_hash.split()[0],
                    }
        llm_config_sha256 = ""
        llm_config_mode = ""
        llm_kind = effective_llm_kind(args, config)
        if llm_kind == "real":
            # OpenHands runner 落在每个 worker 节点上，逐节点校验 real LLM 配置，
            # 且各节点内容必须一致。
            for host, worker in worker_clients.items():
                _, mode_text, _ = base.run(worker, f"stat -c %a {base.q(args.llm_config)}")
                if mode_text.strip() != "600":
                    raise RuntimeError(f"{host}: real OpenHands LLM config must have mode 0600")
                _, hash_text, _ = base.run(worker, f"sha256sum {base.q(args.llm_config)}")
                node_sha256 = hash_text.split()[0]
                if llm_config_sha256 and node_sha256 != llm_config_sha256:
                    raise RuntimeError(f"{host}: real OpenHands LLM config hash diverged across nodes")
                llm_config_sha256 = node_sha256
            llm_config_mode = "0600"
        return {
            "protected_server": protected,
            "source_and_binaries": source_and_binaries,
            "worker_nodes": [
                {"host": node.host, "private_ip": node.private_ip} for node in base.WORKER_NODES
            ],
            "llm_config_path": args.llm_config,
            "llm_config_sha256": llm_config_sha256,
            "llm_config_mode": llm_config_mode,
            "llm_kind": llm_kind,
            "datasets": datasets,
            "math_trace_corpora": trace_corpora,
        }
    finally:
        for worker in worker_clients.values():
            worker.close()
        server.close()


def assert_protected_after(args: argparse.Namespace, before: dict[str, Any]) -> dict[str, Any]:
    from uenv_stress.core import distributed_runtime as base

    base.configure_from_args(args)
    server = base.connect(base.SERVER_HOST, os.environ["UENV_PASS"])
    try:
        base.assert_protected_unchanged(server, before)
        return base.protected_snapshot(server)
    finally:
        server.close()


def parse_args(
    argv: list[str] | None = None,
    *,
    prog: str | None = None,
) -> argparse.Namespace:
    parser = argparse.ArgumentParser(prog=prog)
    parser.add_argument(
        "scenario",
        nargs="?",
        default="suite",
        choices=(
            "suite",
            "dscodebench-pressure",
            "swebench-pro-pressure",
            "math-rule-pressure",
            "swebench-pro-trace-collection",
        ),
        help=(
            "Run the five-dataset scale suite, one pressure scenario, or the "
            "SWE-bench Pro Doubao trace collection scenario."
        ),
    )
    parser.add_argument("--config", type=Path, default=DEFAULT_CONFIG)
    parser.add_argument("--trace-config", type=Path, default=DEFAULT_TRACE_CONFIG)
    parser.add_argument("--runtime-config", type=Path, default=DEFAULT_RUNTIME_CONFIG)
    parser.add_argument(
        "--artifacts",
        type=Path,
        default=Path("/opt/uenv-stress/runs"),
    )
    parser.add_argument(
        "--execute",
        action="store_true",
        help="Actually run all selected pressure scenarios; omit for protected preflight only.",
    )
    parser.add_argument("--private-worker-port-range", default="")
    parser.add_argument("--private-gateway-port-range", default="")
    parser.add_argument("--llm-config", default="")
    parser.add_argument("--source-repo", required=True)
    parser.add_argument("--server-bin", required=True)
    parser.add_argument("--worker-bin", required=True)
    parser.add_argument("--code-plugin-bin", required=True)
    parser.add_argument("--math-plugin-bin", required=True)
    parser.add_argument("--protected-pid", type=int, required=True)
    parser.add_argument("--protected-port", type=int, action="append", default=[])
    parser.add_argument("--server-host", default="")
    parser.add_argument("--worker-host", default="")
    parser.add_argument("--server-private-ip", default="")
    parser.add_argument("--worker-private-ip", default="")
    parser.add_argument(
        "--worker-node",
        action="append",
        default=[],
        metavar="HOST:PRIVATE_IP",
        help=(
            "Worker node as SSH host plus private IP. Repeat for each node; "
            "when present it overrides --worker-host/--worker-private-ip."
        ),
    )
    parser.add_argument("--server-port", type=int, default=8099)
    parser.add_argument("--worker-port", type=int, default=8000)
    parser.add_argument("--model-port", type=int, default=8888)
    parser.add_argument("--obs-port", type=int, default=18002)
    parser.add_argument("--gateway-port", type=int, default=8777)
    parser.add_argument("--agent-api-port", type=int, default=24000)
    parser.add_argument("--agent-health-port", type=int, default=24100)
    args = parser.parse_args(argv)
    inventory = load_runtime_inventory(args.runtime_config)
    args.server_host = args.server_host or inventory.server.ssh_host
    args.server_private_ip = args.server_private_ip or inventory.server.private_ip
    args.worker_host = args.worker_host or inventory.workers[0].ssh_host
    args.worker_private_ip = (
        args.worker_private_ip or inventory.workers[0].private_ip
    )
    if not args.worker_node:
        args.worker_node = inventory.worker_node_arguments()
    configured_worker_hosts = {
        value.split(":", 1)[0].strip() for value in args.worker_node
    }
    banned = sorted(configured_worker_hosts & inventory.banned_worker_hosts)
    if banned:
        raise ValueError(f"banned worker hosts requested: {banned}")
    if not args.protected_port:
        args.protected_port = list(inventory.protected_ports)
    return args


def main(
    argv: list[str] | None = None,
    *,
    prog: str | None = None,
) -> int:
    args = parse_args(argv, prog=prog)
    # Child runners use HERE as cwd, so relative artifact roots would otherwise
    # be written under the source tree and become invisible to this collector.
    args.artifacts = args.artifacts.resolve()
    config = load_suite_config(args.config, args.trace_config)
    validate_arguments(args, config)
    args.artifacts.mkdir(parents=True, exist_ok=True)
    suite_id = f"scale-stress-suite-{time.strftime('%Y%m%d-%H%M%S')}"
    suite_root = args.artifacts / suite_id
    suite_root.mkdir(parents=True, exist_ok=False)
    before = preflight(args, config)
    document: dict[str, Any] = {
        "schema_version": 1,
        "suite_id": suite_id,
        "scenario": args.scenario,
        "status": "preflight_passed",
        "executed": args.execute,
        "config_path": str(args.config),
        "config_sha256": sha256_file(args.config),
        "config": config,
        "trace_config_path": str(args.trace_config),
        "trace_config_sha256": sha256_file(args.trace_config),
        "runtime_config_path": str(args.runtime_config),
        "runtime_config_sha256": sha256_file(args.runtime_config),
        "preflight": before,
        "required_dataset_coverage": (
            sorted(SCALE_DATASETS) if args.scenario == "suite" else []
        ),
        "suite_metrics_contract": {
            "schema_version": suite_metrics.SUITE_METRICS_SCHEMA_VERSION,
            "artifact": "suite-metrics.json",
            "status": "planned" if not args.execute else "pending",
        },
        "scenarios": [],
    }
    summary_path = suite_root / "summary.json"
    if not args.execute:
        planned_commands: dict[str, Any] = {}
        if args.scenario in {"suite", "dscodebench-pressure"} and config["dscodebench_pressure"].get("enabled", True):
            planned_commands["dscodebench_pressure"] = dscodebench_pressure_command(args, config, suite_root / "dscodebench_pressure")
        if args.scenario in {"suite", "swebench-pro-pressure"} and config["swebench_pro_pressure"].get("enabled", True):
            planned_commands["swebench_pro_pressure"] = swebench_pro_pressure_command(args, config, suite_root / "swebench_pro_pressure")
        if args.scenario in {"suite", "math-rule-pressure"} and config["math_rule_pressure"].get("enabled", True):
            planned_commands["math_rule_pressure"] = math_rule_pressure_command(
                args, config, suite_root / "math_rule_pressure"
            )
        if args.scenario == "swebench-pro-trace-collection":
            planned_commands["swebench_pro_trace_collection"] = swebench_pro_trace_collection_command(
                args, config, suite_root / "swebench_pro_trace_collection"
            )
        if args.scenario in {"suite", "dscodebench-pressure"} and config["worker_scale"].get("enabled", False):
            planned_commands["worker_scale"] = [
                worker_scale_command(args, config, workers, suite_root / f"worker-scale-{workers:04d}")
                for workers in config["worker_scale"]["tiers"]
            ]
        document["planned_commands"] = planned_commands
        summary_path.write_text(json.dumps(document, indent=2, sort_keys=True), encoding="utf-8")
        print(f"[suite] preflight PASS summary={summary_path}")
        return 0

    try:
        # suite 执行顺序按验收要求固定为 math-rule -> dscodebench -> swebench-pro。
        if args.scenario in {"suite", "math-rule-pressure"} and config["math_rule_pressure"].get("enabled", True):
            document["scenarios"].append(run_child(
                f"math-rule-pressure-{config['math_rule_pressure']['workers']}workers-three-datasets",
                math_rule_pressure_command(
                    args, config, suite_root / "math_rule_pressure"
                ),
                suite_root / "math_rule_pressure",
                "math-rule-pressure-summary-*.json",
            ))
        if args.scenario in {"suite", "dscodebench-pressure"} and config["dscodebench_pressure"].get("enabled", True):
            document["scenarios"].append(run_child(
                "dscodebench-pressure-1024-simulator",
                dscodebench_pressure_command(args, config, suite_root / "dscodebench_pressure"),
                suite_root / "dscodebench_pressure",
                "dscodebench-pressure-summary-*.json",
            ))
        if args.scenario in {"suite", "swebench-pro-pressure"} and config["swebench_pro_pressure"].get("enabled", True):
            document["scenarios"].append(run_child(
                f"swebench-pro-pressure-openhands-{config['swebench_pro_pressure'].get('registered_workers', 1)}workers-trace-replay",
                swebench_pro_pressure_command(args, config, suite_root / "swebench_pro_pressure"),
                suite_root / "swebench_pro_pressure",
                "swebench-pro-pressure-summary-*.json",
            ))
        if args.scenario == "swebench-pro-trace-collection":
            document["scenarios"].append(run_child(
                "swebench-pro-trace-collection-doubao",
                swebench_pro_trace_collection_command(args, config, suite_root / "swebench_pro_trace_collection"),
                suite_root / "swebench_pro_trace_collection",
                "swebench-pro-pressure-summary-*.json",
            ))
        if args.scenario in {"suite", "dscodebench-pressure"} and config["worker_scale"].get("enabled", True):
            tiers = [int(value) for value in config["worker_scale"]["tiers"]]
            for index, workers in enumerate(tiers):
                scale_artifacts = suite_root / f"worker-scale-{workers:04d}"
                scenario = run_child(
                    f"worker-scale-{workers}",
                    worker_scale_command(args, config, workers, scale_artifacts),
                    scale_artifacts,
                    "dscodebench-pressure-summary-*.json",
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

    covered_datasets = scenario_dataset_coverage(document["scenarios"])
    coverage_passed = (
        args.scenario != "suite" or covered_datasets == SCALE_DATASETS
    )
    document["dataset_coverage"] = {
        "required": sorted(SCALE_DATASETS) if args.scenario == "suite" else [],
        "covered": sorted(covered_datasets),
        "missing": (
            sorted(SCALE_DATASETS - covered_datasets)
            if args.scenario == "suite"
            else []
        ),
        "passed": coverage_passed,
    }
    document["status"] = (
        "passed"
        if (
            document["scenarios"]
            and all(item["status"] == "passed" for item in document["scenarios"])
            and coverage_passed
        )
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
    document["suite_metrics"] = suite_metrics.build_scale_suite_metrics(document)
    metrics_path = suite_root / "suite-metrics.json"
    metrics_path.write_text(
        json.dumps(document["suite_metrics"], indent=2, sort_keys=True),
        encoding="utf-8",
    )
    document["suite_metrics_contract"].update({
        "status": "recorded",
        "artifact": str(metrics_path),
    })
    if args.scenario == "suite" and not document["suite_metrics"]["complete"]:
        document["status"] = "failed"
        document["infrastructure"]["passed"] = False
        document["infrastructure"]["suite_metrics_complete"] = False
    else:
        document["infrastructure"]["suite_metrics_complete"] = bool(
            document["suite_metrics"]["complete"]
        )
    summary_path.write_text(json.dumps(document, indent=2, sort_keys=True), encoding="utf-8")
    print(f"[suite] status={document['status']} summary={summary_path}")
    return 0 if document["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
