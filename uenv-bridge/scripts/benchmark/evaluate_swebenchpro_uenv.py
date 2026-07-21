#!/usr/bin/env python3
from __future__ import annotations

import argparse
import csv
import json
import os
import socket
import sys
import time
import types
import uuid
from collections import Counter
from collections.abc import Iterable
from pathlib import Path
from typing import Any

try:
    from tqdm import tqdm
except ModuleNotFoundError:
    def tqdm(iterable=None, *args, **kwargs):  # type: ignore[no-redef]
        return iterable if iterable is not None else []

    tqdm_module = types.ModuleType("tqdm")
    tqdm_module.tqdm = tqdm
    sys.modules.setdefault("tqdm", tqdm_module)

ROOT = Path(__file__).resolve().parents[2]
SRC = ROOT / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))

from uenv.bridge.clients import RustCoreClientConfig, RustCoreEpisodeClient
from uenv.bridge.protocol import MODE_MULTI, EpisodeRequest, EpisodeResult


DEFAULT_DATA = ROOT / "data/benchmarks/swebenchpro/test.jsonl"
DEFAULT_OUTPUT = ROOT / "temp/benchmarks/swebenchpro/qwen3_6_35b_a3b_uenv_agent_full"


def read_jsonl(path: Path) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    with path.open("r", encoding="utf-8") as file:
        for line in file:
            if line.strip():
                rows.append(json.loads(line))
    return rows


def wait_for_tcp(endpoint: str, timeout: float) -> None:
    host, port_text = endpoint.rsplit(":", 1)
    deadline = time.time() + timeout
    last_error: OSError | None = None
    while time.time() < deadline:
        sock = socket.socket()
        sock.settimeout(2.0)
        try:
            sock.connect((host, int(port_text)))
            return
        except OSError as exc:
            last_error = exc
            time.sleep(0.5)
        finally:
            sock.close()
    raise TimeoutError(f"adapter core endpoint not reachable: {endpoint}; last_error={last_error}")


def build_request(
    *,
    row: dict[str, Any],
    sample_index: int,
    batch_id: str,
    model_endpoint: str,
    model_name: str,
    temperature: float,
    top_p: float,
    max_tokens: int,
    thinking_token_budget: int | None,
    timeout_seconds: int,
    seed: int,
    benchmark_variant: str,
    command_mode: str,
    env_package_id: str,
    env_package_version: str,
    agent_bridge_id: str,
    agent_bridge_version: str,
    agent_pool_id: str,
    driver_entrypoint: str,
    workspace_dir: str,
    llm_config_path: str,
    max_iterations: int,
    pool_selector: dict[str, str],
) -> EpisodeRequest:
    instance_id = str(row["instance_id"])
    request_id = f"swebenchpro-{instance_id}-{uuid.uuid4().hex[:8]}"
    env_config: dict[str, Any] = {
        "task_name": "swe-bench-pro",
        "data_source": "swe-bench-pro",
        "dataset": "swe-bench-pro",
        "instance_id": instance_id,
        "benchmark_variant": benchmark_variant,
        "command_mode": command_mode,
        "env_package_id": env_package_id,
        "env_package_version": env_package_version,
        "execution_mode": "agent",
        "mode": "llm",
        "agent_bridge_id": agent_bridge_id,
        "agent_bridge_version": agent_bridge_version,
        "agent_pool_id": agent_pool_id,
        "driver_entrypoint": driver_entrypoint,
        "workspace_dir": workspace_dir,
        "llm_config_path": llm_config_path,
        "max_iterations": max_iterations,
        "repo": row.get("repo", ""),
        "repo_language": row.get("repo_language", ""),
        "base_commit": row.get("base_commit", ""),
    }
    if pool_selector:
        env_config["pool_selector"] = pool_selector

    generation_config: dict[str, Any] = {
        "temperature": temperature,
        "top_p": top_p,
        "max_tokens": max_tokens,
        "max_new_tokens": max_tokens,
    }
    if thinking_token_budget is not None:
        generation_config["thinking_token_budget"] = thinking_token_budget

    payload = {
        "protocol_version": "1.0",
        "framework": "uenv-benchmark",
        "correlation_id": f"{batch_id}-{sample_index}",
        "request_ts": time.time(),
        "env_config": env_config,
        "model_endpoint": {
            "endpoint_type": "http",
            "url": model_endpoint,
            "model_name": model_name,
            "generation_config": generation_config,
            "max_retries": 3,
        },
        "episode_config": {
            "max_steps": max_iterations,
            "max_turns": max_iterations,
            "seed": seed,
            "stop_conditions": ["done", "max_steps", "timeout"],
        },
        "reward_config": {
            "type": "swe_resolved",
            "target": str(row.get("instance_id", "")),
        },
        "metadata": {
            "batch_id": batch_id,
            "sample_index": sample_index,
            "instance_id": instance_id,
            "task_name": "swe-bench-pro",
            "data_source": "swe-bench-pro",
            "repo": row.get("repo", ""),
            "repo_language": row.get("repo_language", ""),
            "base_commit": row.get("base_commit", ""),
            "dockerhub_tag": row.get("dockerhub_tag", ""),
            "extra_info": {
                "instance_id": instance_id,
                "dataset": "swe-bench-pro",
                "benchmark_variant": benchmark_variant,
                "max_steps": max_iterations,
            },
        },
        "timeout_seconds": timeout_seconds,
    }
    return EpisodeRequest(
        request_id=request_id,
        env_type="swe",
        payload=json.dumps(payload, ensure_ascii=False, separators=(",", ":")).encode("utf-8"),
        mode=MODE_MULTI,
        max_steps=max_iterations,
        model_endpoint=model_endpoint,
        seed=seed,
    )


def batched(items: list[Any], size: int) -> Iterable[list[Any]]:
    for start in range(0, len(items), size):
        yield items[start : start + size]


def append_jsonl(path: Path, payload: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a", encoding="utf-8") as file:
        file.write(json.dumps(payload, ensure_ascii=False) + "\n")


def payload_json(request: EpisodeRequest) -> dict[str, Any]:
    return json.loads(request.payload.decode("utf-8"))


def last_step(result: EpisodeResult):
    if not result.trajectory.steps:
        return None
    return result.trajectory.steps[-1]


def result_to_row(row: dict[str, Any], result: EpisodeResult, elapsed_ms: int) -> dict[str, Any]:
    step = last_step(result)
    info = step.info if step is not None else {}
    reward = float(result.summary.total_reward or 0.0)
    meta = result.metadata or {}
    trajectory_id = str(result.trajectory_id or info.get("trajectory_id", "") or meta.get("trajectory_id", ""))
    tests_passed = meta.get("tests_passed", "")
    tests_total = meta.get("tests_total", "")
    git_diff_nonempty = meta.get("git_diff_nonempty", "")
    git_diff_bytes = meta.get("git_diff_bytes", "")
    return {
        "instance_id": row["instance_id"],
        "repo": row.get("repo", ""),
        "repo_language": row.get("repo_language", ""),
        "base_commit": row.get("base_commit", ""),
        "dockerhub_tag": row.get("dockerhub_tag", ""),
        "resolved": reward > 0.0 and result.status == "completed",
        "uenv_reward": reward,
        "uenv_status": result.status,
        "uenv_request_id": result.request_id,
        "uenv_error_code": result.error_code,
        "uenv_error_message": result.error_message,
        "trajectory_id": trajectory_id,
        "tests_passed": tests_passed,
        "tests_total": tests_total,
        "git_diff_nonempty": git_diff_nonempty,
        "git_diff_bytes": git_diff_bytes,
        "elapsed_ms": elapsed_ms,
        "terminate_reason": result.summary.terminate_reason,
    }


def write_outputs(output_dir: Path, rows: list[dict[str, Any]], metadata: dict[str, Any]) -> None:
    output_dir.mkdir(parents=True, exist_ok=True)
    evaluated = len(rows)
    resolved = sum(1 for row in rows if row["resolved"])
    status_counts = Counter(str(row["uenv_status"]) for row in rows)
    repo_counts = Counter(str(row["repo"]) for row in rows)
    metrics = {
        "sample_count": evaluated,
        "resolved_count": resolved,
        "resolve_rate": resolved / evaluated if evaluated else 0.0,
        "completed_count": status_counts.get("completed", 0),
        "failed_count": evaluated - status_counts.get("completed", 0),
        "status_counts": status_counts,
        "repo_distribution": repo_counts,
        **metadata,
    }
    (output_dir / "metrics.json").write_text(
        json.dumps(metrics, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    (output_dir / "official_like_results.json").write_text(
        json.dumps({row["instance_id"]: bool(row["resolved"]) for row in rows}, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    with (output_dir / "uenv_predictions.jsonl").open("w", encoding="utf-8") as file:
        for row in rows:
            file.write(json.dumps(row, ensure_ascii=False) + "\n")
    with (output_dir / "uenv_predictions.csv").open("w", encoding="utf-8", newline="") as file:
        fieldnames = [
            "instance_id",
            "repo",
            "repo_language",
            "base_commit",
            "dockerhub_tag",
            "resolved",
            "uenv_reward",
            "uenv_status",
            "uenv_request_id",
            "uenv_error_code",
            "uenv_error_message",
            "trajectory_id",
            "tests_passed",
            "tests_total",
            "git_diff_nonempty",
            "git_diff_bytes",
            "elapsed_ms",
            "terminate_reason",
        ]
        writer = csv.DictWriter(file, fieldnames=fieldnames)
        writer.writeheader()
        for row in rows:
            writer.writerow({key: row.get(key, "") for key in fieldnames})


def parse_pool_selector(raw: str) -> dict[str, str]:
    if not raw.strip():
        return {}
    data = json.loads(raw)
    if not isinstance(data, dict):
        raise SystemExit("--pool-selector-json must be a JSON object")
    return {str(key): str(value) for key, value in data.items()}


def completed_instance_ids(path: Path) -> set[str]:
    if not path.exists():
        return set()
    done: set[str] = set()
    with path.open("r", encoding="utf-8") as file:
        for line in file:
            if not line.strip():
                continue
            try:
                row = json.loads(line)
            except json.JSONDecodeError:
                continue
            instance_id = str(row.get("instance_id") or "")
            if instance_id:
                done.add(instance_id)
    return done


def main() -> int:
    parser = argparse.ArgumentParser(description="Evaluate SWE-bench-Pro through UEnv SWE+Agent.")
    parser.add_argument("--data", type=Path, default=DEFAULT_DATA)
    parser.add_argument("--output-dir", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--endpoint", default=os.getenv("UENV_ADAPTER_CORE_ENDPOINT", "8.130.75.157:8088"))
    parser.add_argument("--model-endpoint", default=os.getenv("UENV_ROLLOUT_MODEL_ENDPOINT", ""))
    parser.add_argument("--model-name", default=os.getenv("UENV_ROLLOUT_MODEL_NAME", "Qwen/Qwen3.6-35B-A3B"))
    parser.add_argument("--limit", type=int, default=None)
    parser.add_argument("--instance-id", action="append", default=[])
    parser.add_argument("--batch-size", type=int, default=1)
    parser.add_argument("--max-tokens", type=int, default=8192)
    parser.add_argument("--thinking-token-budget", type=int, default=4096)
    parser.add_argument("--temperature", type=float, default=0.0)
    parser.add_argument("--top-p", type=float, default=1.0)
    parser.add_argument("--timeout-seconds", type=int, default=7200)
    parser.add_argument("--client-timeout-seconds", type=float, default=7600.0)
    parser.add_argument("--connect-timeout-seconds", type=float, default=20.0)
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--benchmark-variant", default="pro")
    parser.add_argument("--command-mode", default="full_shell")
    parser.add_argument("--env-package-id", default="swe-bench-pro")
    parser.add_argument("--env-package-version", default="0.3.4")
    parser.add_argument("--agent-bridge-id", default="uenv-agent-openhands")
    parser.add_argument("--agent-bridge-version", default="1.0.0")
    parser.add_argument("--agent-pool-id", default="openhands-default")
    parser.add_argument("--driver-entrypoint", default="run_swebenchpro_official.py")
    parser.add_argument("--workspace-dir", default="/app")
    parser.add_argument("--llm-config-path", default="/root/UEnv/config/openhands-llm-qwen3-thinking-max-token-8192.json")
    parser.add_argument("--max-iterations", type=int, default=50)
    parser.add_argument("--pool-selector-json", default="")
    parser.add_argument("--requests-log", type=Path, default=None)
    parser.add_argument("--results-log", type=Path, default=None)
    parser.add_argument("--resume", action="store_true")
    args = parser.parse_args()

    if args.batch_size < 1:
        raise SystemExit("--batch-size must be >= 1")

    wait_for_tcp(args.endpoint, args.connect_timeout_seconds)
    examples = read_jsonl(args.data)
    if args.instance_id:
        wanted = set(args.instance_id)
        examples = [row for row in examples if str(row.get("instance_id", "")) in wanted]
        missing = wanted - {str(row.get("instance_id", "")) for row in examples}
        if missing:
            raise SystemExit(f"instance_id not found in data: {', '.join(sorted(missing))}")
    if args.limit is not None:
        examples = examples[: args.limit]
    batch_id = f"swebenchpro-uenv-{time.strftime('%Y%m%d_%H%M%S')}"
    pool_selector = parse_pool_selector(args.pool_selector_json)

    result_log = args.results_log or args.output_dir / "uenv_results.jsonl"
    request_log = args.requests_log or args.output_dir / "uenv_requests.jsonl"
    if not args.resume:
        request_log.unlink(missing_ok=True)
        result_log.unlink(missing_ok=True)

    skip_ids = completed_instance_ids(result_log) if args.resume else set()
    pending_examples = [row for row in examples if str(row["instance_id"]) not in skip_ids]

    requests = [
        build_request(
            row=row,
            sample_index=idx,
            batch_id=batch_id,
            model_endpoint=args.model_endpoint,
            model_name=args.model_name,
            temperature=args.temperature,
            top_p=args.top_p,
            max_tokens=args.max_tokens,
            thinking_token_budget=args.thinking_token_budget,
            timeout_seconds=args.timeout_seconds,
            seed=args.seed + idx,
            benchmark_variant=args.benchmark_variant,
            command_mode=args.command_mode,
            env_package_id=args.env_package_id,
            env_package_version=args.env_package_version,
            agent_bridge_id=args.agent_bridge_id,
            agent_bridge_version=args.agent_bridge_version,
            agent_pool_id=args.agent_pool_id,
            driver_entrypoint=args.driver_entrypoint,
            workspace_dir=args.workspace_dir,
            llm_config_path=args.llm_config_path,
            max_iterations=args.max_iterations,
            pool_selector=pool_selector,
        )
        for idx, row in enumerate(pending_examples)
    ]

    rows: list[dict[str, Any]] = []
    if args.resume and result_log.exists():
        with result_log.open("r", encoding="utf-8") as file:
            for line in file:
                if line.strip():
                    rows.append(json.loads(line))

    client = RustCoreEpisodeClient(
        RustCoreClientConfig(
            endpoint=args.endpoint,
            timeout_seconds=args.client_timeout_seconds,
            auto_start=False,
        )
    )
    try:
        example_by_request_id = {request.request_id: row for request, row in zip(requests, pending_examples, strict=True)}
        for batch in tqdm(list(batched(requests, args.batch_size)), desc="UEnv SWE-bench-Pro"):
            started = time.time()
            for request in batch:
                append_jsonl(
                    request_log,
                    {
                        "request_id": request.request_id,
                        "env_type": request.env_type,
                        "model_endpoint": request.model_endpoint,
                        "payload": payload_json(request),
                    },
                )
            results = list(client.submit_episode_stream(batch))
            elapsed_ms = int((time.time() - started) * 1000)
            for result in results:
                row = result_to_row(example_by_request_id[result.request_id], result, elapsed_ms)
                rows.append(row)
                append_jsonl(result_log, row)
    finally:
        client.close()

    order = {str(row["instance_id"]): idx for idx, row in enumerate(examples)}
    rows.sort(key=lambda row: order.get(str(row["instance_id"]), 10**9))
    write_outputs(
        args.output_dir,
        rows,
        {
            "adapter_core_endpoint": args.endpoint,
            "model_endpoint": args.model_endpoint,
            "model_name": args.model_name,
            "batch_id": batch_id,
            "batch_size": args.batch_size,
            "benchmark_variant": args.benchmark_variant,
            "env_package_id": args.env_package_id,
            "env_package_version": args.env_package_version,
            "agent_pool_id": args.agent_pool_id,
            "agent_bridge_id": args.agent_bridge_id,
            "agent_bridge_version": args.agent_bridge_version,
            "llm_config_path": args.llm_config_path,
            "max_iterations": args.max_iterations,
            "max_tokens": args.max_tokens,
            "thinking_token_budget": args.thinking_token_budget,
            "inference_mode": "uenv_swe_agent",
            "thinking": "enabled_by_llm_config",
            "resumed_skipped_count": len(skip_ids),
        },
    )
    metrics = json.loads((args.output_dir / "metrics.json").read_text(encoding="utf-8"))
    print(json.dumps(metrics, ensure_ascii=False, indent=2))
    print(f"Wrote UEnv SWE-bench-Pro results to {args.output_dir}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
