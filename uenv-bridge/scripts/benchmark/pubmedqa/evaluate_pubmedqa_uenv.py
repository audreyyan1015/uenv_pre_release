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

ROOT = Path(__file__).resolve().parents[3]
SRC = ROOT / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))
COMMON = ROOT / "scripts/benchmark/common"
if str(COMMON) not in sys.path:
    sys.path.insert(0, str(COMMON))

from uenv.bridge.clients import RustCoreClientConfig, RustCoreEpisodeClient
from uenv.bridge.protocol import EpisodeRequest, EpisodeResult, MODE_MULTI

from pubmedqa_utils import LABELS, build_prompt, compute_metrics, load_pubmedqa, parse_label
from obs_run import (
    add_obs_args,
    attach_training_run_id,
    emit_run_closed,
    emit_run_started,
    emit_episode_result,
    resolve_run_id,
)


DEFAULT_DATA = ROOT / "data/benchmarks/pubmedqa/ori_pqal.json"
DEFAULT_OUTPUT = ROOT / "temp/benchmarks/pubmedqa/qwen3_6_35b_a3b_uenv_generate"


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
    qid: str,
    prompt: str,
    answer: str,
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
    timeout_seconds: int,
    seed: int,
    training_run_id: str,
) -> EpisodeRequest:
    request_id = f"pubmedqa-{qid}-{uuid.uuid4().hex[:8]}"
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
    payload = {
        "protocol_version": "1.0",
        "framework": "uenv-benchmark",
        "correlation_id": f"{batch_id}-{sample_index}",
        "request_ts": time.time(),
        "env_config": {
            "task_name": "pubmedqa",
            "data_source": "pubmedqa",
            "dataset": "pubmedqa",
            "question": prompt,
        },
        "model_endpoint": {
            "endpoint_type": "http",
            "url": model_endpoint,
            "model_name": model_name,
            "generation_config": generation_config,
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
            "target": answer,
        },
        "metadata": {
            "batch_id": batch_id,
            "sample_index": sample_index,
            "qid": qid,
            "task_name": "pubmedqa",
            "data_source": "pubmedqa",
            "extra_info": {
                "qid": qid,
                "dataset": "pubmedqa",
                "max_steps": 1,
            },
        },
        "timeout_seconds": timeout_seconds,
    }
    attach_training_run_id(payload, training_run_id)
    return EpisodeRequest(
        request_id=request_id,
        env_type="qa",
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


def result_to_row(example: Any, result: EpisodeResult, elapsed_ms: int) -> dict[str, Any]:
    text = response_text_from_result(result)
    pred = parse_label(text)
    step = last_step(result)
    info = step.info if step is not None else {}
    return {
        "qid": example.qid,
        "gold": example.answer,
        "pred": pred or "unparsed",
        "correct": pred == example.answer,
        "uenv_reward": result.summary.total_reward,
        "uenv_status": result.status,
        "uenv_request_id": result.request_id,
        "uenv_error_code": result.error_code,
        "uenv_error_message": result.error_message,
        "elapsed_ms": elapsed_ms,
        "response_text": text,
        "worker_dataset": info.get("dataset", ""),
        "worker_expected": info.get("expected", ""),
    }


def write_outputs(output_dir: Path, rows: list[dict[str, Any]], metadata: dict[str, Any]) -> None:
    output_dir.mkdir(parents=True, exist_ok=True)
    metrics = compute_metrics(rows)
    metrics["uenv"] = {
        "completed_count": sum(1 for row in rows if row["uenv_status"] == "completed"),
        "failed_count": sum(1 for row in rows if row["uenv_status"] != "completed"),
        "reward_accuracy": (
            sum(float(row["uenv_reward"] or 0.0) for row in rows) / len(rows) if rows else 0.0
        ),
        **metadata,
    }
    (output_dir / "metrics.json").write_text(
        json.dumps(metrics, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    (output_dir / "predictions_official.json").write_text(
        json.dumps(
            {row["qid"]: row["pred"] for row in rows if row["pred"] in LABELS},
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
            "qid",
            "gold",
            "pred",
            "correct",
            "uenv_reward",
            "uenv_status",
            "uenv_request_id",
            "uenv_error_code",
            "uenv_error_message",
            "elapsed_ms",
            "worker_dataset",
            "worker_expected",
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


def payload_json(request: EpisodeRequest) -> dict[str, Any]:
    return json.loads(request.payload.decode("utf-8"))


def main() -> int:
    parser = argparse.ArgumentParser(description="Evaluate PubMedQA through UEnv AdapterCore/Server/Worker.")
    parser.add_argument("--data", type=Path, default=DEFAULT_DATA)
    parser.add_argument("--output-dir", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--endpoint", default=os.getenv("UENV_ADAPTER_CORE_ENDPOINT", "8.130.75.157:8088"))
    parser.add_argument("--model-endpoint", default=os.getenv("UENV_ROLLOUT_MODEL_ENDPOINT", ""))
    parser.add_argument("--model-name", default=os.getenv("UENV_ROLLOUT_MODEL_NAME", "policy-model"))
    parser.add_argument("--limit", type=int, default=None)
    parser.add_argument("--batch-size", type=int, default=1)
    parser.add_argument("--prompt-style", default="official", choices=["default", "official", "strict_label", "thinking_label"])
    parser.add_argument("--max-tokens", type=int, default=512)
    parser.add_argument("--temperature", type=float, default=0.0)
    parser.add_argument("--top-p", type=float, default=1.0)
    parser.add_argument("--enable-thinking", action="store_true")
    parser.add_argument("--preserve-thinking", action="store_true")
    parser.add_argument("--thinking-token-budget", type=int, default=None)
    parser.add_argument("--timeout-seconds", type=int, default=900)
    parser.add_argument("--client-timeout-seconds", type=float, default=1200.0)
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--connect-timeout-seconds", type=float, default=20.0)
    parser.add_argument("--requests-log", type=Path, default=None)
    parser.add_argument("--results-log", type=Path, default=None)
    add_obs_args(parser)
    args = parser.parse_args()

    if not args.model_endpoint:
        raise SystemExit("--model-endpoint is required, or set UENV_ROLLOUT_MODEL_ENDPOINT")
    if args.batch_size < 1:
        raise SystemExit("--batch-size must be >= 1")

    wait_for_tcp(args.endpoint, args.connect_timeout_seconds)
    examples = load_pubmedqa(args.data, limit=args.limit)
    batch_id = f"pubmedqa-uenv-{time.strftime('%Y%m%d_%H%M%S')}"
    training_run_id = resolve_run_id(args.run_id, batch_id)
    print(
        json.dumps(
            {
                "benchmark": "pubmedqa",
                "training_run_id": training_run_id,
                "obs_url": args.obs_url,
                "batch_id": batch_id,
                "total_examples": len(examples),
            },
            ensure_ascii=False,
        ),
        flush=True,
    )
    requests = [
        build_request(
            qid=example.qid,
            prompt=build_prompt(example, prompt_style=args.prompt_style),
            answer=example.answer,
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
            timeout_seconds=args.timeout_seconds,
            seed=args.seed + idx,
            training_run_id=training_run_id,
        )
        for idx, example in enumerate(examples)
    ]

    request_log = args.requests_log or args.output_dir / "uenv_requests.jsonl"
    result_log = args.results_log or args.output_dir / "uenv_results.jsonl"
    request_log.unlink(missing_ok=True)
    result_log.unlink(missing_ok=True)

    rows: list[dict[str, Any]] = []
    emit_run_started(
        obs_url=args.obs_url,
        obs_token=args.obs_token,
        training_run_id=training_run_id,
        benchmark="pubmedqa",
        batch_id=batch_id,
        output_dir=args.output_dir,
        total_examples=len(examples),
        payload={"limit": args.limit},
    )
    client = RustCoreEpisodeClient(
        RustCoreClientConfig(
            endpoint=args.endpoint,
            timeout_seconds=args.client_timeout_seconds,
            auto_start=False,
        )
    )
    try:
        request_payload_by_id = {request.request_id: payload_json(request) for request in requests}
        example_by_request_id = {request.request_id: example for request, example in zip(requests, examples, strict=True)}
        for batch in tqdm(list(batched(requests, args.batch_size)), desc="UEnv PubMedQA"):
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
                example = example_by_request_id[result.request_id]
                row = result_to_row(example, result, elapsed_ms)
                rows.append(row)
                append_jsonl(result_log, row)
                request_payload = request_payload_by_id.get(result.request_id, {})
                emit_episode_result(
                    obs_url=args.obs_url,
                    obs_token=args.obs_token,
                    training_run_id=training_run_id,
                    benchmark="pubmedqa",
                    batch_id=batch_id,
                    request_id=result.request_id,
                    status=result.status,
                    reward=float(result.summary.total_reward or 0.0),
                    correlation_id=str(request_payload.get("correlation_id") or ""),
                    env_type="qa",
                    trajectory_id=result.trajectory_id,
                    error_code=result.error_code,
                    error_message=result.error_message,
                )
    finally:
        client.close()

    rows.sort(key=lambda row: next(i for i, example in enumerate(examples) if example.qid == row["qid"]))
    write_outputs(
        args.output_dir,
        rows,
        {
            "adapter_core_endpoint": args.endpoint,
            "model_endpoint": args.model_endpoint,
            "model_name": args.model_name,
            "training_run_id": training_run_id,
            "obs_url": args.obs_url,
            "batch_id": batch_id,
            "batch_size": args.batch_size,
            "prompt_style": args.prompt_style,
            "inference_mode": "uenv_generate",
            "enable_thinking": args.enable_thinking,
            "preserve_thinking": args.preserve_thinking,
            "thinking_token_budget": args.thinking_token_budget,
            "max_tokens": args.max_tokens,
        },
    )
    emit_run_closed(
        obs_url=args.obs_url,
        obs_token=args.obs_token,
        training_run_id=training_run_id,
        benchmark="pubmedqa",
        batch_id=batch_id,
        output_dir=args.output_dir,
        ok=all(row["uenv_status"] == "completed" for row in rows),
        result_count=len(rows),
        payload={
            "completed_count": sum(1 for row in rows if row["uenv_status"] == "completed"),
            "failed_count": sum(1 for row in rows if row["uenv_status"] != "completed"),
        },
    )
    metrics = json.loads((args.output_dir / "metrics.json").read_text(encoding="utf-8"))
    print(json.dumps(metrics, ensure_ascii=False, indent=2))
    print(f"Wrote UEnv PubMedQA results to {args.output_dir}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
