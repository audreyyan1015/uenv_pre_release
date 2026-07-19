#!/usr/bin/env python3
"""Run Gate3 and Gate4 as one protected, reproducible real-LLM suite.

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


def load_suite_config(path: Path) -> dict[str, Any]:
    document = json.loads(path.read_text(encoding="utf-8"))
    if document.get("schema_version") != 1:
        raise ValueError("stress suite schema_version must be 1")
    gate3 = document.get("gate3")
    gate4 = document.get("gate4")
    worker_scale = document.get("worker_scale")
    if not isinstance(gate3, dict) or not isinstance(gate4, dict) or not isinstance(worker_scale, dict):
        raise ValueError("stress suite requires gate3, gate4 and worker_scale objects")
    modes = gate3.get("modes")
    if not isinstance(modes, list) or not modes or set(modes) - GATE3_MODES:
        raise ValueError(f"invalid Gate3 modes: {modes!r}")
    if gate3.get("model_mode") != "real":
        raise ValueError("the integrated acceptance suite requires Gate3 model_mode=real")
    if gate4.get("mode") != "llm":
        raise ValueError("the integrated acceptance suite requires Gate4 mode=llm")
    concurrencies = gate4.get("concurrencies")
    if concurrencies != [1, 2]:
        raise ValueError("the integrated acceptance suite requires Gate4 concurrencies [1, 2]")
    min_steps = int(gate3.get("min_steps", 0))
    max_steps = int(gate3.get("max_steps", 0))
    if min_steps < 2 or max_steps < min_steps:
        raise ValueError("Gate3 requires 2 <= min_steps <= max_steps")
    if worker_scale.get("model_mode") != "deterministic_dataset_oracle":
        raise ValueError("worker_scale must explicitly use deterministic_dataset_oracle")
    tiers = worker_scale.get("tiers")
    if tiers != [32, 512, 1024]:
        raise ValueError("worker_scale tiers must be exactly [32, 512, 1024]")
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


def validate_arguments(args: argparse.Namespace, config: dict[str, Any]) -> None:
    for label in ("source_repo", "server_bin", "worker_bin", "code_plugin_bin", "llm_config"):
        require_absolute(f"--{label.replace('_', '-')}", str(getattr(args, label)))
    for label, port in {
        "server": args.server_port,
        "worker": args.worker_port,
        "model": args.model_port,
        "gateway": args.gateway_port,
    }.items():
        if port not in ALLOWED_EXPOSED_PORTS:
            raise ValueError(f"{label} port {port} is not in the explicitly allowed exposed-port set")
    protected = set(args.protected_port)
    requested = {args.server_port, args.worker_port, args.model_port, args.gateway_port}
    overlap = protected & requested
    if overlap:
        raise ValueError(f"isolated suite ports overlap protected ports: {sorted(overlap)}")
    gate3 = config["gate3"]
    workers = int(gate3["workers"])
    max_scale_workers = max(int(value) for value in config["worker_scale"]["tiers"])
    if (workers > 1 or config["worker_scale"].get("enabled", True)) and not args.private_worker_port_range:
        raise ValueError("multi-Worker execution requires --private-worker-port-range")
    if args.private_worker_port_range:
        try:
            start_text, end_text = args.private_worker_port_range.split("-", 1)
            start, end = int(start_text), int(end_text)
        except (ValueError, AttributeError) as exc:
            raise ValueError("--private-worker-port-range must be START-END") from exc
        if start != args.worker_port or end - start + 1 < max_scale_workers or end > 65535:
            raise ValueError(
                f"private Worker range must start at {args.worker_port} and contain at least {max_scale_workers} ports"
            )


def common_child_args(args: argparse.Namespace) -> list[str]:
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
        "--model-port", str(args.model_port),
        "--obs-port", str(args.obs_port),
    ]
    for port in args.protected_port:
        command.extend(["--protected-port", str(port)])
    return command


def gate3_command(args: argparse.Namespace, config: dict[str, Any], artifacts: Path) -> list[str]:
    gate = config["gate3"]
    command = [
        sys.executable,
        str(HERE / "run_distributed_gate3_code.py"),
        "--duration", str(gate["duration_seconds_per_mode"]),
        "--workers", str(gate["workers"]),
        "--capacity", str(gate["capacity_per_worker"]),
        "--min-steps", str(gate["min_steps"]),
        "--max-steps", str(gate["max_steps"]),
        "--model-mode", "real",
        "--llm-config", args.llm_config,
        "--dataset-jsonl", str(gate["dataset_jsonl"]),
        "--dataset-limit", str(gate["dataset_limit"]),
        "--dataset-offset", str(gate["dataset_offset"]),
        "--exact-batches", str(gate["exact_batches_per_mode"]),
        "--acceptance-purpose", "gate3-real-llm",
        "--artifacts", str(artifacts),
    ]
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
    return [
        sys.executable,
        str(HERE / "run_distributed_gate3_code.py"),
        "--duration", "1",
        "--workers", str(workers),
        "--capacity", str(gate["capacity_per_worker"]),
        "--mode", "sync",
        "--min-steps", "1",
        "--max-steps", str(gate["max_steps"]),
        "--model-mode", "simulator",
        "--code-wrong-steps", "0",
        "--dataset-jsonl", str(gate["dataset_jsonl"]),
        "--dataset-limit", str(gate["dataset_limit"]),
        "--dataset-offset", str(gate["dataset_offset"]),
        "--exact-batches", str(gate["exact_batches"]),
        "--registration-timeout", str(gate["registration_timeout_seconds"]),
        "--batch-timeout", str(gate["batch_timeout_seconds"]),
        "--simulator-latency-ms", str(gate["simulator_latency_ms"]),
        "--plugin-ready-timeout-seconds", str(gate["plugin_ready_timeout_seconds"]),
        "--worker-register-max-attempts", str(gate["worker_register_max_attempts"]),
        "--worker-register-retry-backoff-ms", str(gate["worker_register_retry_backoff_ms"]),
        "--acceptance-purpose", "worker-scale",
        "--private-worker-port-range", args.private_worker_port_range,
        "--artifacts", str(artifacts),
    ] + common_child_args(args)


def gate4_command(args: argparse.Namespace, config: dict[str, Any], artifacts: Path) -> list[str]:
    gate = config["gate4"]
    return [
        sys.executable,
        str(HERE / "run_distributed_gate4_swe.py"),
        "--mode", "llm",
        "--max-steps", str(gate["max_steps"]),
        "--openhands-max-iterations", str(gate["openhands_max_iterations"]),
        "--llm-config", args.llm_config,
        "--gateway-port", str(args.gateway_port),
        "--agent-api-port", str(args.agent_api_port),
        "--agent-health-port", str(args.agent_health_port),
        "--artifacts", str(artifacts),
    ] + common_child_args(args)


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
    }
    print(f"[suite] {name} {status} returncode={returncode}", flush=True)
    return result


def scale_resource_gate(
    scenario: dict[str, Any],
    current_workers: int,
    next_workers: int | None,
    config: dict[str, Any],
) -> dict[str, Any]:
    """Refuse unsafe escalation using measurements from the just-finished real fleet."""
    candidates = [
        value for value in collect_records(scenario.get("result"), "fleet_resource_metrics")
        if isinstance(value, dict) and value
    ]
    if len(candidates) != 1:
        return {"passed": False, "reason": f"expected one fleet metric record, found {len(candidates)}"}
    metrics = candidates[0]
    required = {
        "mem_total_bytes", "initial_mem_available_bytes", "min_mem_available_bytes", "peak_rss_bytes",
        "peak_processes", "peak_open_fds", "sample_count",
    }
    missing = sorted(required - metrics.keys())
    if missing:
        return {"passed": False, "reason": f"fleet metrics missing fields: {missing}", "metrics": metrics}
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
    passed = available_ok and projected_ok and int(metrics["sample_count"]) > 0
    return {
        "passed": passed,
        "current_workers": current_workers,
        "next_workers": next_workers,
        "metrics": metrics,
        "minimum_mem_available_bytes": minimum_available,
        "measured_mem_available_drop_bytes": measured_available_drop,
        "projected_next_fleet_memory_bytes": projected_bytes,
        "projected_next_mem_available_bytes": projected_available_bytes,
        "maximum_projected_host_memory_fraction": config["maximum_projected_host_memory_fraction"],
        "available_memory_gate_passed": available_ok,
        "projected_memory_gate_passed": projected_ok,
        "reason": "safe to continue" if passed else "measured Worker-host memory gate refused escalation",
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
        dataset_paths = sorted({
            str(config["gate3"]["dataset_jsonl"]),
            str(config["worker_scale"]["dataset_jsonl"]),
        })
        datasets = {}
        for path in dataset_paths:
            base.run(server, f"test -f {base.q(path)}")
            _, dataset_hash, _ = base.run(server, f"sha256sum {base.q(path)}")
            datasets[path] = dataset_hash.split()[0]
        _, mode_text, _ = base.run(worker, f"stat -c %a {base.q(args.llm_config)}")
        if mode_text.strip() != "600":
            raise RuntimeError("real OpenHands LLM config must have mode 0600")
        _, hash_text, _ = base.run(worker, f"sha256sum {base.q(args.llm_config)}")
        llm_config_sha256 = hash_text.split()[0]
        return {
            "protected_server": protected,
            "source_and_binaries": source_and_binaries,
            "llm_config_path": args.llm_config,
            "llm_config_sha256": llm_config_sha256,
            "llm_config_mode": "0600",
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
    parser.add_argument("--llm-config", required=True)
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
    parser.add_argument("--agent-api-port", type=int, default=18004)
    parser.add_argument("--agent-health-port", type=int, default=18005)
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
    suite_id = f"real-training-suite-{time.strftime('%Y%m%d-%H%M%S')}"
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
        document["planned_commands"] = {
            "gate3": gate3_command(args, config, suite_root / "gate3"),
            "gate4": gate4_command(args, config, suite_root / "gate4"),
            "worker_scale": [
                worker_scale_command(args, config, workers, suite_root / f"worker-scale-{workers:04d}")
                for workers in config["worker_scale"]["tiers"]
            ],
        }
        summary_path.write_text(json.dumps(document, indent=2, sort_keys=True), encoding="utf-8")
        print(f"[suite] preflight PASS summary={summary_path}")
        return 0

    try:
        if config["gate3"].get("enabled", True):
            document["scenarios"].append(run_child(
                "gate3-real-llm",
                gate3_command(args, config, suite_root / "gate3"),
                suite_root / "gate3",
                "gate3-summary-*.json",
            ))
        if config["gate4"].get("enabled", True):
            document["scenarios"].append(run_child(
                "gate4-real-llm",
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
                resource_gate = scale_resource_gate(
                    scenario, workers, next_workers, config["worker_scale"]
                )
                scenario["resource_gate"] = resource_gate
                if not resource_gate["passed"]:
                    scenario["status"] = "failed"
                    scenario["error"] = resource_gate["reason"]
                    print(f"[suite] Worker scale resource gate FAILED: {resource_gate}", flush=True)
                    break
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
    summary_path.write_text(json.dumps(document, indent=2, sort_keys=True), encoding="utf-8")
    print(f"[suite] status={document['status']} summary={summary_path}")
    return 0 if document["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
