from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import sys
import time
import uuid
from typing import Any, Iterable

from .clients import RustCoreClientConfig, RustCoreEpisodeClient
from .protocol import EpisodeRequest, EpisodeResult, MODE_MULTI


def _object(value: Any, *, name: str) -> dict[str, Any]:
    if value is None:
        return {}
    if not isinstance(value, dict):
        raise ValueError(f"{name} must be a JSON object")
    return dict(value)


def load_cases(path: Path, *, limit: int | None = None) -> list[dict[str, Any]]:
    cases: list[dict[str, Any]] = []
    with path.open("r", encoding="utf-8") as handle:
        for line_number, raw in enumerate(handle, start=1):
            line = raw.strip()
            if not line or line.startswith("#"):
                continue
            try:
                value = json.loads(line)
            except json.JSONDecodeError as exc:
                raise ValueError(f"{path}:{line_number}: invalid JSON: {exc}") from exc
            if not isinstance(value, dict):
                raise ValueError(f"{path}:{line_number}: each row must be a JSON object")
            cases.append(value)
            if limit is not None and len(cases) >= limit:
                break
    if not cases:
        raise ValueError(f"{path}: no evaluation cases found")
    return cases


def build_request(
    case: dict[str, Any],
    *,
    index: int,
    batch_id: str,
    default_env_type: str,
    default_dataset: str,
    model_endpoint: str,
    model_name: str,
    max_tokens: int,
    temperature: float,
    top_p: float,
    max_steps: int,
    timeout_seconds: int,
    seed: int,
) -> EpisodeRequest:
    declared_env_type = str(case.get("env_type") or "").strip()
    env_type = str(default_env_type).strip()
    if not env_type:
        raise ValueError(f"case {index}: env_type is required")
    if declared_env_type and declared_env_type != env_type:
        raise ValueError(
            f"case {index}: env_type={declared_env_type!r} does not match "
            f"the run-task --env-type {env_type!r}; run different tasks separately"
        )

    env_config = _object(case.get("env_config"), name=f"case {index}.env_config")
    declared_dataset = str(case.get("dataset") or env_config.get("dataset") or "").strip()
    dataset = str(default_dataset).strip()
    if not dataset:
        raise ValueError(f"case {index}: dataset is required")
    if declared_dataset and declared_dataset != dataset:
        raise ValueError(
            f"case {index}: dataset={declared_dataset!r} does not match "
            f"the run-task --dataset {dataset!r}; run different tasks separately"
        )
    question = case.get("question")
    if question is not None and "question" not in env_config:
        env_config["question"] = str(question)
    if dataset:
        env_config.setdefault("dataset", dataset)
        env_config.setdefault("data_source", dataset)
    env_config.setdefault("task_name", dataset or env_type)

    reward_config = _object(case.get("reward_config"), name=f"case {index}.reward_config")
    if not reward_config and "target" in case:
        reward_config = {"type": "rule_reward", "target": str(case["target"])}
    if not reward_config:
        raise ValueError(
            f"case {index}: provide target or reward_config so the environment can score the result"
        )

    case_id = str(case.get("id") or case.get("request_id") or index)
    request_id = f"eval-{case_id}-{uuid.uuid4().hex[:8]}"
    case_seed = int(case.get("seed", seed + index))
    case_steps = max(1, int(case.get("max_steps", max_steps)))
    if "max_steps" in case and case_steps != max_steps:
        raise ValueError(
            f"case {index}: max_steps={case_steps} does not match the run-task "
            f"--max-steps {max_steps}"
        )
    metadata = _object(case.get("metadata"), name=f"case {index}.metadata")
    metadata.update(
        {
            "batch_id": batch_id,
            "sample_index": index,
            "case_id": case_id,
            "dataset": dataset,
        }
    )

    generation_config = _object(
        case.get("generation_config"), name=f"case {index}.generation_config"
    )
    generation_config.setdefault("temperature", temperature)
    generation_config.setdefault("top_p", top_p)
    generation_config.setdefault("max_tokens", max_tokens)
    generation_config.setdefault("max_new_tokens", max_tokens)

    selected_endpoint = str(case.get("model_endpoint") or model_endpoint).strip()
    selected_model = str(case.get("model_name") or model_name).strip()
    payload = {
        "protocol_version": "1.0",
        "framework": "uenv-evaluate",
        "correlation_id": f"{batch_id}-{index}",
        "env_config": env_config,
        "model_endpoint": {
            "endpoint_type": "http",
            "url": selected_endpoint,
            "model_name": selected_model,
            "generation_config": generation_config,
            "max_retries": int(case.get("model_max_retries", 3)),
        },
        "episode_config": {
            "max_steps": case_steps,
            "max_turns": int(case.get("max_turns", case_steps)),
            "seed": case_seed,
            "stop_conditions": ["done", "max_steps", "timeout"],
        },
        "reward_config": reward_config,
        "metadata": metadata,
        "timeout_seconds": int(case.get("timeout_seconds", timeout_seconds)),
    }
    return EpisodeRequest(
        request_id=request_id,
        env_type=env_type,
        payload=json.dumps(payload, ensure_ascii=False, separators=(",", ":")).encode("utf-8"),
        mode=MODE_MULTI,
        max_steps=case_steps,
        model_endpoint=selected_endpoint,
        seed=case_seed,
        parallel_mode="sync",
    )


def _text(value: bytes) -> str:
    return value.decode("utf-8", errors="replace")


def result_record(case: dict[str, Any], request: EpisodeRequest, result: EpisodeResult) -> dict[str, Any]:
    steps = []
    for step in result.trajectory.steps:
        steps.append(
            {
                "step_index": step.step_index,
                "observation": _text(step.observation),
                "action": _text(step.action),
                "reward": step.reward,
                "terminated": step.terminated,
                "truncated": step.truncated,
                "info": step.info,
                "duration_ms": step.duration_ms,
            }
        )
    return {
        "case_id": str(case.get("id") or case.get("request_id") or ""),
        "request_id": request.request_id,
        "env_type": request.env_type,
        "status": result.status,
        "reward": result.summary.total_reward,
        "total_steps": result.summary.total_steps,
        "terminate_reason": result.summary.terminate_reason,
        "trajectory_id": result.trajectory_id,
        "error_code": result.error_code,
        "error_message": result.error_message,
        "steps": steps,
    }


def _batches(items: list[Any], size: int) -> Iterable[list[Any]]:
    for start in range(0, len(items), size):
        yield items[start : start + size]


def run(args: argparse.Namespace) -> int:
    cases = load_cases(args.input, limit=args.limit)
    batch_id = args.batch_id or f"eval-{time.strftime('%Y%m%d-%H%M%S')}"
    requests = [
        build_request(
            case,
            index=index,
            batch_id=batch_id,
            default_env_type=args.env_type,
            default_dataset=args.dataset,
            model_endpoint=args.model_endpoint,
            model_name=args.model_name,
            max_tokens=args.max_tokens,
            temperature=args.temperature,
            top_p=args.top_p,
            max_steps=args.max_steps,
            timeout_seconds=args.timeout_seconds,
            seed=args.seed,
        )
        for index, case in enumerate(cases)
    ]
    by_id = {request.request_id: (case, request) for case, request in zip(cases, requests)}
    records: list[dict[str, Any]] = []
    config = RustCoreClientConfig(
        endpoint=args.endpoint,
        timeout_seconds=float(args.client_timeout_seconds),
        auto_start=False,
        streaming=args.streaming,
    )
    with RustCoreEpisodeClient(config) as client:
        for request_batch in _batches(requests, args.batch_size):
            for result in client.submit_episode_stream(request_batch):
                pair = by_id.get(result.request_id)
                if pair is None:
                    raise RuntimeError(f"UEnv Server returned unknown request_id={result.request_id}")
                case, request = pair
                records.append(result_record(case, request, result))

    returned = {record["request_id"] for record in records}
    missing = [request.request_id for request in requests if request.request_id not in returned]
    if missing:
        raise RuntimeError(f"UEnv Server did not return {len(missing)} request(s): {missing[:3]}")

    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open("w", encoding="utf-8") as handle:
        for record in records:
            handle.write(json.dumps(record, ensure_ascii=False) + "\n")

    completed = sum(record["status"] == "completed" for record in records)
    reward_mean = sum(float(record["reward"] or 0.0) for record in records) / len(records)
    summary = {
        "batch_id": batch_id,
        "cases": len(records),
        "completed": completed,
        "failed": len(records) - completed,
        "mean_reward": reward_mean,
        "output": str(args.output),
    }
    print(json.dumps(summary, ensure_ascii=False, indent=2))
    return 0 if completed == len(records) else 1


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="uenv-evaluate",
        description="Run JSONL evaluation cases through UEnv Bridge, Server, and Worker.",
    )
    parser.add_argument("--input", type=Path, required=True, help="JSONL evaluation cases")
    parser.add_argument("--output", type=Path, required=True, help="JSONL result path")
    parser.add_argument(
        "--endpoint",
        required=True,
        help="UEnv Server endpoint",
    )
    parser.add_argument(
        "--env-type",
        required=True,
        help="authoritative env_type for this batch; repeated row values must match",
    )
    parser.add_argument(
        "--dataset",
        required=True,
        help="authoritative dataset route; repeated row values must match",
    )
    parser.add_argument(
        "--model-endpoint",
        default=os.getenv("UENV_ROLLOUT_MODEL_ENDPOINT", ""),
        help="OpenAI-compatible API base URL; empty uses Worker LLM configuration",
    )
    parser.add_argument(
        "--model-name",
        default=os.getenv("UENV_ROLLOUT_MODEL_NAME", ""),
        help="model identifier expected by the model service",
    )
    parser.add_argument("--max-tokens", type=int, default=512)
    parser.add_argument("--temperature", type=float, default=0.0)
    parser.add_argument("--top-p", type=float, default=1.0)
    parser.add_argument("--max-steps", type=int, required=True)
    parser.add_argument("--timeout-seconds", type=int, default=900)
    parser.add_argument("--client-timeout-seconds", type=float, default=1200.0)
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--batch-size", type=int, default=1)
    parser.add_argument("--limit", type=int)
    parser.add_argument("--batch-id")
    parser.add_argument("--streaming", action="store_true")
    return parser


def main() -> None:
    parser = build_parser()
    args = parser.parse_args()
    if args.batch_size < 1:
        parser.error("--batch-size must be at least 1")
    if args.max_steps < 1:
        parser.error("--max-steps must be at least 1")
    try:
        raise SystemExit(run(args))
    except (OSError, RuntimeError, ValueError) as exc:
        print(f"uenv-evaluate: {exc}", file=sys.stderr)
        raise SystemExit(2) from exc


if __name__ == "__main__":
    main()
