#!/usr/bin/env python3
"""Freeze prior real-model/real-UEnv evaluator logs into stability trace JSONL."""

from __future__ import annotations

import argparse
import hashlib
import json
import time
from pathlib import Path
from typing import Any

from uenv_stress.core.stability_test_common import sha256_file, validate_trace_file


def read_jsonl(path: Path) -> list[dict[str, Any]]:
    return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line.strip()]


def checksum(value: Any) -> str:
    encoded = json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(encoded).hexdigest()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--dataset", required=True)
    parser.add_argument("--requests", type=Path, required=True)
    parser.add_argument("--results", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--source-model", required=True)
    parser.add_argument("--source-version", required=True)
    parser.add_argument("--tokenizer", required=True)
    parser.add_argument("--minimum", type=int, default=100)
    args = parser.parse_args()
    from transformers import AutoTokenizer  # type: ignore

    tokenizer = AutoTokenizer.from_pretrained(args.tokenizer, local_files_only=True, trust_remote_code=False)
    requests = read_jsonl(args.requests)
    request_by_id = {str(row.get("episode_id") or row.get("request_id")): row for row in requests}
    traces = []
    collected_at = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    for index, result in enumerate(read_jsonl(args.results)):
        status = str(result.get("status") or result.get("uenv_status") or "").lower()
        output = str(result.get("raw_output") or result.get("response_text") or result.get("response") or "")
        if status not in {"completed", "success"} or not output:
            continue
        request_id = str(result.get("episode_id") or result.get("request_id") or result.get("uenv_request_id") or "")
        request = request_by_id.get(request_id, {})
        request_bytes = json.dumps(request, ensure_ascii=False, sort_keys=True).encode()
        response_bytes = json.dumps(result, ensure_ascii=False, sort_keys=True).encode()
        target_tokens = len(tokenizer.encode(output, add_special_tokens=False))
        dataset_id = str(
            result.get("instance_id") or result.get("problem_id") or result.get("qid") or request_id or index
        )
        traces.append({
            "trace_id": f"{args.dataset}-{hashlib.sha256((request_id + dataset_id).encode()).hexdigest()[:20]}",
            "dataset": args.dataset,
            "dataset_id": dataset_id,
            "source_model": args.source_model,
            "source_version": args.source_version,
            "collected_at": collected_at,
            "prompt_hash": hashlib.sha256(request_bytes).hexdigest(),
            "turns": [{
                "turn_index": 0,
                "assistant_output": output,
                "source_completion_tokens": int(result.get("output_tokens") or target_tokens),
                "target_qwen3_tokens": target_tokens,
                "request_bytes": len(request_bytes),
                "response_bytes": len(response_bytes),
                "source_api_latency_ms": int(result.get("model_latency_ms") or 0),
                "env_latency_ms": int(result.get("elapsed_ms") or result.get("execution_time_ms") or 0),
                "environment_calls": result.get("environment_calls") or [],
            }],
            "final_status": status,
            "result_checksum": checksum(result),
        })
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open("x", encoding="utf-8") as target:
        for trace in traces:
            target.write(json.dumps(trace, ensure_ascii=False, sort_keys=True) + "\n")
    stats = validate_trace_file(args.output, dataset=args.dataset, minimum=args.minimum)
    manifest = {
        "schema_version": 1, "dataset": args.dataset, "source_model": args.source_model,
        "source_version": args.source_version, "collected_at": collected_at, "stats": stats,
        "file": {"path": args.output.name, "size_bytes": args.output.stat().st_size, "sha256": sha256_file(args.output)},
    }
    args.output.with_name("trace_manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True), encoding="utf-8")
    print(json.dumps(manifest, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
