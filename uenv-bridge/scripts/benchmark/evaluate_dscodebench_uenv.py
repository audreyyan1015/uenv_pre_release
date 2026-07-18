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
from collections import Counter, defaultdict
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

from evaluate_dscodebench import build_prompt, load_dataset


DEFAULT_DATA = ROOT / "data/benchmarks/dscodebench/DSCodeBench.json"
DEFAULT_OUTPUT = ROOT / "temp/benchmarks/dscodebench/qwen3_6_35b_a3b_uenv_generate"


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


def build_inline_harness_test_code(
    *,
    ground_truth_code: str,
    test_script: str,
    num_tests: int,
    random_seed: int,
) -> str:
    return f"""
import inspect
import json
from dscodebench_harness import evaluate_problem

_candidate_source = inspect.currentframe().f_back.f_locals.get("code", "")
_result = evaluate_problem(
    ground_truth_code={ground_truth_code!r},
    candidate_code=_candidate_source,
    test_script={test_script!r},
    num_tests={int(num_tests)},
    random_seed={int(random_seed)},
)
if not _result.get("passed"):
    raise AssertionError(json.dumps(_result, ensure_ascii=False))
"""


def build_generation_config(
    *,
    max_tokens: int,
    temperature: float,
    top_p: float,
    enable_thinking: bool,
    preserve_thinking: bool,
    thinking_token_budget: int | None,
) -> dict[str, Any]:
    generation_config: dict[str, Any] = {
        "temperature": temperature,
        "top_p": top_p,
        "max_tokens": max_tokens,
        "max_new_tokens": max_tokens,
    }
    if enable_thinking or preserve_thinking:
        chat_template_kwargs: dict[str, Any] = {}
        if enable_thinking:
            chat_template_kwargs["enable_thinking"] = True
        if preserve_thinking:
            chat_template_kwargs["preserve_thinking"] = True
        generation_config["chat_template_kwargs"] = chat_template_kwargs
    if thinking_token_budget is not None:
        generation_config["thinking_token_budget"] = thinking_token_budget
    return generation_config


def build_request(
    *,
    row: dict[str, Any],
    prompt: str,
    sample_index: int,
    batch_id: str,
    model_endpoint: str,
    model_name: str,
    max_tokens: int,
    temperature: float,
    top_p: float,
    enable_thinking: bool,
    preserve_thinking: bool,
    thinking_token_budget: int | None,
    num_tests: int,
    timeout_seconds: int,
    code_timeout_secs: int,
    seed: int,
    evaluation_mode: str,
) -> EpisodeRequest:
    problem_id = str(row["problem_id"])
    request_id = f"dscodebench-{problem_id}-{uuid.uuid4().hex[:8]}"
    env_config: dict[str, Any] = {
        "task_name": "dscodebench",
        "data_source": "dscodebench",
        "dataset": "dscodebench",
        "question": prompt,
        "task_id": problem_id,
        "library": row.get("library", ""),
        "ground_truth_code": row.get("ground_truth_code", ""),
        "num_tests": num_tests,
        "random_seed": seed,
        "timeout_secs": code_timeout_secs,
    }
    if evaluation_mode == "inline_harness":
        env_config["test_code"] = build_inline_harness_test_code(
            ground_truth_code=str(row.get("ground_truth_code", "")),
            test_script=str(row.get("test_script", "")),
            num_tests=num_tests,
            random_seed=seed,
        )
    elif evaluation_mode == "path_harness":
        env_config["test_script_path"] = f"{problem_id}.py"
    else:
        raise ValueError(f"unsupported evaluation_mode: {evaluation_mode}")

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
            "generation_config": build_generation_config(
                max_tokens=max_tokens,
                temperature=temperature,
                top_p=top_p,
                enable_thinking=enable_thinking,
                preserve_thinking=preserve_thinking,
                thinking_token_budget=thinking_token_budget,
            ),
            "max_retries": 3,
        },
        "episode_config": {
            "max_steps": 1,
            "max_turns": 1,
            "seed": seed,
            "stop_conditions": ["done", "max_steps", "timeout"],
        },
        "reward_config": {
            "type": "rule_reward",
            "target": problem_id,
        },
        "metadata": {
            "batch_id": batch_id,
            "sample_index": sample_index,
            "qid": problem_id,
            "task_name": "dscodebench",
            "data_source": "dscodebench",
            "extra_info": {
                "qid": problem_id,
                "dataset": "dscodebench",
                "library": row.get("library", ""),
                "max_steps": 1,
                "num_tests": num_tests,
                "evaluation_mode": evaluation_mode,
            },
        },
        "timeout_seconds": timeout_seconds,
    }
    return EpisodeRequest(
        request_id=request_id,
        env_type="code",
        payload=json.dumps(payload, ensure_ascii=False, separators=(",", ":")).encode("utf-8"),
        mode=MODE_MULTI,
        max_steps=1,
        model_endpoint=model_endpoint,
        seed=seed,
    )


def batched(items: list[Any], size: int) -> Iterable[list[Any]]:
    for start in range(0, len(items), size):
        yield items[start : start + size]


def last_step(result: EpisodeResult):
    if not result.trajectory.steps:
        return None
    return result.trajectory.steps[-1]


def response_text_from_result(result: EpisodeResult) -> str:
    step = last_step(result)
    if step is None:
        return ""
    if step.info.get("response_text"):
        return step.info["response_text"]
    if step.action:
        return step.action.decode("utf-8", errors="replace")
    return ""


def bool_from_info(value: Any) -> bool | None:
    if isinstance(value, bool):
        return value
    if isinstance(value, str):
        lower = value.strip().lower()
        if lower in {"true", "1", "yes"}:
            return True
        if lower in {"false", "0", "no"}:
            return False
    return None


def int_from_info(value: Any) -> int | None:
    if value is None or value == "":
        return None
    try:
        return int(value)
    except (TypeError, ValueError):
        return None


def output_token_count(result: EpisodeResult) -> int | None:
    step = last_step(result)
    if step is None:
        return None
    for key in ("response_token_ids", "token_ids", "output_token_ids"):
        raw = step.info.get(key)
        if not raw:
            continue
        try:
            value = json.loads(raw)
        except Exception:
            value = raw
        if isinstance(value, list):
            return len(value)
    return None


def result_to_row(row: dict[str, Any], result: EpisodeResult, elapsed_ms: int) -> dict[str, Any]:
    text = response_text_from_result(result)
    reward = float(result.summary.total_reward or 0.0)
    step = last_step(result)
    info = step.info if step is not None else {}
    passed = bool_from_info(info.get("passed"))
    tests_run = int_from_info(info.get("tests_run"))
    tests_passed = int_from_info(info.get("tests_passed"))
    execution_time_ms = int_from_info(info.get("execution_time_ms"))
    return {
        "problem_id": row["problem_id"],
        "library": row.get("library", ""),
        "passed": passed if passed is not None else reward > 0.0,
        "tests_run": tests_run,
        "tests_passed": tests_passed,
        "execution_time_ms": execution_time_ms,
        "output_tokens": output_token_count(result),
        "uenv_reward": reward,
        "uenv_status": result.status,
        "uenv_request_id": result.request_id,
        "uenv_error_code": result.error_code,
        "uenv_error_message": result.error_message,
        "elapsed_ms": elapsed_ms,
        "worker_dataset": info.get("dataset", ""),
        "worker_task_id": info.get("task_id", ""),
        "worker_library": info.get("library", ""),
        "worker_error": info.get("error", ""),
        "worker_detail": info.get("detail", ""),
        "response_text": text,
    }


def safe_div(num: float, den: float) -> float:
    return num / den if den else 0.0


def compute_metrics(rows: list[dict[str, Any]]) -> dict[str, Any]:
    total = len(rows)
    completed = sum(1 for row in rows if row["uenv_status"] == "completed")
    failed = total - completed
    passed = sum(1 for row in rows if row["passed"])
    executed = sum(1 for row in rows if (row.get("tests_run") or 0) > 0)
    errors = sum(1 for row in rows if row.get("worker_error") or row.get("uenv_error_message"))
    by_library: dict[str, dict[str, int]] = defaultdict(
        lambda: {"total": 0, "completed": 0, "executed": 0, "passed": 0, "errors": 0}
    )
    for row in rows:
        stats = by_library[str(row.get("library") or "")]
        stats["total"] += 1
        stats["completed"] += int(row["uenv_status"] == "completed")
        stats["executed"] += int((row.get("tests_run") or 0) > 0)
        stats["passed"] += int(bool(row["passed"]))
        stats["errors"] += int(bool(row.get("worker_error") or row.get("uenv_error_message")))
    return {
        "problem_count": total,
        "completed_count": completed,
        "failed_count": failed,
        "executed_count": executed,
        "passed_count": passed,
        "error_count": errors,
        "completion_rate": safe_div(completed, total),
        "execution_rate": safe_div(executed, total),
        "pass_at_1": safe_div(passed, total),
        "reward_accuracy": safe_div(sum(float(row["uenv_reward"] or 0.0) for row in rows), total),
        "library_distribution": dict(Counter(str(row.get("library") or "") for row in rows)),
        "by_library": {
            library: {
                "problem_count": values["total"],
                "completion_rate": safe_div(values["completed"], values["total"]),
                "execution_rate": safe_div(values["executed"], values["total"]),
                "pass_at_1": safe_div(values["passed"], values["total"]),
                "error_count": values["errors"],
            }
            for library, values in sorted(by_library.items())
        },
    }


def write_outputs(output_dir: Path, rows: list[dict[str, Any]], metadata: dict[str, Any]) -> None:
    output_dir.mkdir(parents=True, exist_ok=True)
    metrics = compute_metrics(rows)
    metrics["uenv"] = metadata
    (output_dir / "metrics.json").write_text(
        json.dumps(metrics, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    (output_dir / "predictions_official.json").write_text(
        json.dumps(
            {row["problem_id"]: row["response_text"] for row in rows},
            ensure_ascii=False,
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )
    with (output_dir / "predictions.jsonl").open("w", encoding="utf-8") as file:
        for row in rows:
            file.write(json.dumps(row, ensure_ascii=False) + "\n")
    with (output_dir / "predictions.csv").open("w", encoding="utf-8", newline="") as file:
        fieldnames = [
            "problem_id",
            "library",
            "passed",
            "tests_run",
            "tests_passed",
            "execution_time_ms",
            "output_tokens",
            "uenv_reward",
            "uenv_status",
            "uenv_request_id",
            "uenv_error_code",
            "uenv_error_message",
            "elapsed_ms",
            "worker_dataset",
            "worker_task_id",
            "worker_library",
            "worker_error",
            "response_text",
        ]
        writer = csv.DictWriter(file, fieldnames=fieldnames)
        writer.writeheader()
        for row in rows:
            writer.writerow({key: row.get(key, "") for key in fieldnames})


def append_jsonl(path: Path, payload: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a", encoding="utf-8") as file:
        file.write(json.dumps(payload, ensure_ascii=False) + "\n")


def load_existing_result_rows(path: Path) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    if not path.exists():
        return rows
    with path.open(encoding="utf-8") as file:
        for line_no, line in enumerate(file, start=1):
            text = line.strip()
            if not text:
                continue
            try:
                row = json.loads(text)
            except json.JSONDecodeError as exc:
                raise SystemExit(f"invalid json in {path}:{line_no}: {exc}") from exc
            if not isinstance(row, dict):
                raise SystemExit(f"invalid row in {path}:{line_no}: expected object")
            if not row.get("problem_id"):
                raise SystemExit(f"invalid row in {path}:{line_no}: missing problem_id")
            rows.append(row)
    return rows


def dedupe_rows_by_problem_id(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
    deduped: dict[str, dict[str, Any]] = {}
    for row in rows:
        deduped[str(row["problem_id"])] = row
    return list(deduped.values())


def payload_json(request: EpisodeRequest) -> dict[str, Any]:
    return json.loads(request.payload.decode("utf-8"))


def main() -> int:
    parser = argparse.ArgumentParser(description="Evaluate DSCodeBench through UEnv AdapterCore/Server/Worker.")
    parser.add_argument("--data", type=Path, default=DEFAULT_DATA)
    parser.add_argument("--output-dir", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--endpoint", default=os.getenv("UENV_ADAPTER_CORE_ENDPOINT", "8.130.75.157:8088"))
    parser.add_argument("--model-endpoint", default=os.getenv("UENV_ROLLOUT_MODEL_ENDPOINT", ""))
    parser.add_argument("--model-name", default=os.getenv("UENV_ROLLOUT_MODEL_NAME", "policy-model"))
    parser.add_argument("--limit", type=int, default=None)
    parser.add_argument("--library")
    parser.add_argument("--max-per-library", type=int)
    parser.add_argument("--batch-size", type=int, default=1)
    parser.add_argument("--prompt-style", default="official_fenced", choices=["official", "official_fenced"])
    parser.add_argument("--max-tokens", type=int, default=32768)
    parser.add_argument("--temperature", type=float, default=0.2)
    parser.add_argument("--top-p", type=float, default=1.0)
    parser.add_argument("--enable-thinking", action="store_true")
    parser.add_argument("--preserve-thinking", action="store_true")
    parser.add_argument("--thinking-token-budget", type=int, default=None)
    parser.add_argument("--test-case-number", type=int, default=200)
    parser.add_argument("--timeout-seconds", type=int, default=7200)
    parser.add_argument("--code-timeout-secs", type=int, default=300)
    parser.add_argument("--client-timeout-seconds", type=float, default=7800.0)
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--connect-timeout-seconds", type=float, default=20.0)
    parser.add_argument("--evaluation-mode", default="inline_harness", choices=["inline_harness", "path_harness"])
    parser.add_argument("--requests-log", type=Path, default=None)
    parser.add_argument("--results-log", type=Path, default=None)
    parser.add_argument(
        "--resume",
        action="store_true",
        help="Append to existing logs and skip problem_ids already present as completed in the results log.",
    )
    args = parser.parse_args()

    if not args.model_endpoint:
        raise SystemExit("--model-endpoint is required, or set UENV_ROLLOUT_MODEL_ENDPOINT")
    if args.batch_size < 1:
        raise SystemExit("--batch-size must be >= 1")

    wait_for_tcp(args.endpoint, args.connect_timeout_seconds)
    examples = load_dataset(
        args.data,
        limit=args.limit,
        library=args.library,
        max_per_library=args.max_per_library,
    )
    batch_id = f"dscodebench-uenv-{time.strftime('%Y%m%d_%H%M%S')}"
    requests = [
        build_request(
            row=row,
            prompt=build_prompt(str(row["code_problem"]), prompt_style=args.prompt_style),
            sample_index=idx,
            batch_id=batch_id,
            model_endpoint=args.model_endpoint,
            model_name=args.model_name,
            max_tokens=args.max_tokens,
            temperature=args.temperature,
            top_p=args.top_p,
            enable_thinking=args.enable_thinking,
            preserve_thinking=args.preserve_thinking,
            thinking_token_budget=args.thinking_token_budget,
            num_tests=args.test_case_number,
            timeout_seconds=args.timeout_seconds,
            code_timeout_secs=args.code_timeout_secs,
            seed=args.seed + idx,
            evaluation_mode=args.evaluation_mode,
        )
        for idx, row in enumerate(examples)
    ]

    request_log = args.requests_log or args.output_dir / "uenv_requests.jsonl"
    result_log = args.results_log or args.output_dir / "uenv_results.jsonl"
    if args.resume:
        current_ids = {str(row["problem_id"]) for row in examples}
        existing_rows = [
            row
            for row in dedupe_rows_by_problem_id(load_existing_result_rows(result_log))
            if str(row["problem_id"]) in current_ids
        ]
        rows = [
            row
            for row in existing_rows
            if row.get("uenv_status") == "completed"
        ]
        completed_ids = {str(row["problem_id"]) for row in rows}
        requests = [
            request
            for request, row in zip(requests, examples, strict=True)
            if str(row["problem_id"]) not in completed_ids
        ]
        print(
            json.dumps(
                {
                    "resume": True,
                    "existing_results": len(existing_rows),
                    "existing_completed_results": len(rows),
                    "remaining_requests": len(requests),
                    "request_log": str(request_log),
                    "result_log": str(result_log),
                },
                ensure_ascii=False,
            ),
            flush=True,
        )
    else:
        request_log.unlink(missing_ok=True)
        result_log.unlink(missing_ok=True)
        rows: list[dict[str, Any]] = []

    client = RustCoreEpisodeClient(
        RustCoreClientConfig(
            endpoint=args.endpoint,
            timeout_seconds=args.client_timeout_seconds,
            auto_start=False,
        )
    )
    try:
        example_by_id = {str(row["problem_id"]): row for row in examples}
        example_by_request_id = {
            request.request_id: example_by_id[str(payload_json(request)["metadata"]["qid"])]
            for request in requests
        }
        for batch in tqdm(list(batched(requests, args.batch_size)), desc="UEnv DSCodeBench"):
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

    rows.sort(key=lambda row: next(i for i, example in enumerate(examples) if example["problem_id"] == row["problem_id"]))
    write_outputs(
        args.output_dir,
        rows,
        {
            "adapter_core_endpoint": args.endpoint,
            "model_endpoint": args.model_endpoint,
            "model_name": args.model_name,
            "batch_id": batch_id,
            "batch_size": args.batch_size,
            "prompt_style": args.prompt_style,
            "inference_mode": "uenv_generate",
            "evaluation_mode": args.evaluation_mode,
            "enable_thinking": args.enable_thinking,
            "preserve_thinking": args.preserve_thinking,
            "thinking_token_budget": args.thinking_token_budget,
            "max_tokens": args.max_tokens,
            "test_case_number": args.test_case_number,
            "code_timeout_secs": args.code_timeout_secs,
            "resume": args.resume,
            "remaining_requests_at_start": len(requests),
        },
    )
    metrics = json.loads((args.output_dir / "metrics.json").read_text(encoding="utf-8"))
    print(json.dumps(metrics, ensure_ascii=False, indent=2))
    print(f"Wrote UEnv DSCodeBench results to {args.output_dir}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
