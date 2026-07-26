#!/usr/bin/env python3
"""Collect frozen OlymMATH/SciTab/PubMedQA traces through the real UEnv path."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import random
import shutil
import subprocess
import sys
import tempfile
import time
from collections import Counter, defaultdict
from pathlib import Path
from typing import Any

from uenv_stress.core.stability_test_common import sha256_file, validate_trace_file


TASKS = ("olymmath", "scitab", "pubmedqa")


def read_jsonl(path: Path) -> list[dict[str, Any]]:
    return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line.strip()]


def write_jsonl(path: Path, rows: list[dict[str, Any]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as target:
        for row in rows:
            target.write(json.dumps(row, ensure_ascii=False, sort_keys=True) + "\n")


def stratified_sample(rows: list[dict[str, Any]], label_key: str, count: int, rng: random.Random) -> list[dict[str, Any]]:
    groups: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for row in rows:
        groups[str(row[label_key]).strip().lower().replace("_", " ")].append(row)
    if not groups:
        raise ValueError("cannot stratify an empty dataset")
    selected: list[dict[str, Any]] = []
    labels = sorted(groups)
    base, remainder = divmod(count, len(labels))
    for index, label in enumerate(labels):
        candidates = list(groups[label])
        rng.shuffle(candidates)
        take = base + (1 if index < remainder else 0)
        if len(candidates) < take:
            raise ValueError(f"label {label!r} has {len(candidates)} rows; needs {take}")
        selected.extend(candidates[:take])
    rng.shuffle(selected)
    return selected


def prepare_sample_inputs(dataset_root: Path, temporary: Path, count: int, seed: int) -> dict[str, Path]:
    rng = random.Random(seed)
    olym_target = temporary / "olymmath"
    olym_target.mkdir(parents=True)
    for name in ("OlymMATH-EN-EASY.jsonl", "OlymMATH-EN-HARD.jsonl", "OlymMATH-ZH-EASY.jsonl", "OlymMATH-ZH-HARD.jsonl"):
        rows = read_jsonl(dataset_root / "olymmath" / name)
        rng.shuffle(rows)
        write_jsonl(olym_target / name, rows[: count // 4])

    scitab_value = json.loads((dataset_root / "scitab" / "sci_tab.json").read_text(encoding="utf-8"))
    scitab_rows = scitab_value if isinstance(scitab_value, list) else list(scitab_value.values())
    scitab_selected = stratified_sample(scitab_rows, "label", count, rng)
    scitab_path = temporary / "sci_tab.json"
    scitab_path.write_text(json.dumps(scitab_selected, ensure_ascii=False), encoding="utf-8")

    pubmed_value = json.loads((dataset_root / "pubmedqa" / "ori_pqal.json").read_text(encoding="utf-8"))
    keyed = [{"__pmid": key, **value} for key, value in pubmed_value.items()]
    pubmed_selected = stratified_sample(keyed, "final_decision", count, rng)
    pubmed_path = temporary / "ori_pqal.json"
    pubmed_path.write_text(
        json.dumps({row.pop("__pmid"): row for row in pubmed_selected}, ensure_ascii=False), encoding="utf-8"
    )
    return {"olymmath": olym_target, "scitab": scitab_path, "pubmedqa": pubmed_path}


def run_evaluators(
    repo_root: Path,
    inputs: dict[str, Path],
    output: Path,
    *,
    endpoint: str,
    model_endpoint: str,
    model_name: str,
    batch_size: int,
    seed: int,
) -> None:
    benchmark = repo_root / "uenv-bridge" / "scripts" / "benchmark"
    commands = {
        "olymmath": [
            sys.executable, str(benchmark / "evaluate_olymmath_uenv.py"),
            "--data-dir", str(inputs["olymmath"]), "--datasets", "EN-EASY,EN-HARD,ZH-EASY,ZH-HARD",
        ],
        "scitab": [sys.executable, str(benchmark / "evaluate_scitab_uenv.py"), "--data", str(inputs["scitab"])],
        "pubmedqa": [sys.executable, str(benchmark / "evaluate_pubmedqa_uenv.py"), "--data", str(inputs["pubmedqa"])],
    }
    for task, command in commands.items():
        task_output = output / "raw" / task
        command.extend([
            "--output-dir", str(task_output), "--endpoint", endpoint,
            "--model-endpoint", model_endpoint, "--model-name", model_name,
            "--batch-size", str(batch_size), "--seed", str(seed),
        ])
        subprocess.run(command, check=True, cwd=repo_root, env=os.environ.copy())


def import_raw_sources(source_root: Path, output: Path) -> None:
    """Freeze previously collected real-UEnv evaluator logs for conversion."""
    for task in TASKS:
        source = source_root / task
        target = output / "raw" / task
        target.mkdir(parents=True, exist_ok=True)
        for name in ("uenv_requests.jsonl", "uenv_results.jsonl"):
            path = source / name
            if not path.is_file():
                raise ValueError(f"raw trace source missing {path}")
            shutil.copy2(path, target / name)


def load_tokenizer(path: str):
    try:
        from transformers import AutoTokenizer  # type: ignore
    except ImportError as exc:
        raise RuntimeError("trace collection requires transformers for frozen Qwen3 token counts") from exc
    return AutoTokenizer.from_pretrained(path, local_files_only=True, trust_remote_code=False)


def canonical_checksum(value: Any) -> str:
    encoded = json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(encoded).hexdigest()


def convert_task(task: str, raw_dir: Path, target: Path, *, model: dict[str, Any], tokenizer: Any) -> int:
    requests = read_jsonl(raw_dir / "uenv_requests.jsonl")
    results = read_jsonl(raw_dir / "uenv_results.jsonl")
    request_by_id = {str(row.get("episode_id") or row.get("request_id")): row for row in requests}
    traces: list[dict[str, Any]] = []
    dataset_name = {"olymmath": "olymmath", "scitab": "scitab", "pubmedqa": "pubmedqa"}[task]
    for result in results:
        request_id = str(
            result.get("episode_id") or result.get("request_id") or result.get("uenv_request_id") or ""
        )
        request = request_by_id.get(request_id, {})
        status = str(result.get("status") or result.get("uenv_status") or "").lower()
        output = str(result.get("raw_output") or result.get("response_text") or result.get("response") or "")
        if status not in {"completed", "success"} or not output:
            continue
        serialized_request = json.dumps(request, ensure_ascii=False, sort_keys=True).encode()
        serialized_response = json.dumps(result, ensure_ascii=False, sort_keys=True).encode()
        target_tokens = len(tokenizer.encode(output, add_special_tokens=False))
        source_tokens = int(result.get("output_tokens") or target_tokens)
        qid = str(result.get("qid") or result.get("id") or request_id)
        trace = {
            "trace_id": f"{task}-{hashlib.sha256((request_id + qid).encode()).hexdigest()[:20]}",
            "dataset": dataset_name,
            "dataset_id": qid,
            "label": result.get("answer") or result.get("target") or "",
            "source_model": str(model["model"]),
            "source_version": str(model["source_version"]),
            "collected_at": model["collected_at"],
            "prompt_hash": hashlib.sha256(serialized_request).hexdigest(),
            "turns": [{
                "turn_index": 0,
                "assistant_output": output,
                "source_completion_tokens": source_tokens,
                "target_qwen3_tokens": target_tokens,
                "request_bytes": len(serialized_request),
                "response_bytes": len(serialized_response),
                "source_api_latency_ms": int(result.get("model_latency_ms") or 0),
                "env_latency_ms": int(result.get("elapsed_ms") or 0),
                "environment_calls": result.get("environment_calls") or [],
            }],
            "episode_total_ms": int(result.get("elapsed_ms") or 0),
            "final_status": status,
            "result_checksum": canonical_checksum(result),
        }
        traces.append(trace)
    write_jsonl(target, traces)
    return len(traces)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--datasets", default="olymmath,scitab,pubmedqa")
    parser.add_argument("--samples-per-dataset", type=int, default=100)
    parser.add_argument("--model-config", type=Path, required=True)
    parser.add_argument("--dataset-root", type=Path, default=Path("/opt/uenv-stress/datasets"))
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--repo-root", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--endpoint", default=os.getenv("UENV_ADAPTER_CORE_ENDPOINT", "127.0.0.1:8088"))
    parser.add_argument("--concurrency", type=int, default=8)
    parser.add_argument("--seed", type=int, default=20260722)
    parser.add_argument("--tokenizer", required=True, help="Frozen local Qwen3-8B tokenizer directory")
    parser.add_argument(
        "--raw-source-root", type=Path,
        help="Reuse task subdirectories containing prior real-UEnv uenv_requests/results JSONL instead of calling the model",
    )
    args = parser.parse_args()
    selected = tuple(item.strip() for item in args.datasets.split(",") if item.strip())
    if set(selected) != set(TASKS):
        raise ValueError(f"formal collection requires exactly {TASKS}")
    if args.samples_per_dataset < 100 or args.samples_per_dataset % 4:
        raise ValueError("samples-per-dataset must be at least 100 and divisible by four")
    model = json.loads(args.model_config.read_text(encoding="utf-8"))
    for key in ("base_url", "model", "source_version"):
        if not str(model.get(key, "")).strip():
            raise ValueError(f"model config missing {key}")
    model["collected_at"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    tokenizer = load_tokenizer(args.tokenizer)
    args.output_dir.mkdir(parents=True, exist_ok=False)
    with tempfile.TemporaryDirectory(prefix="uenv-math-traces-") as temp:
        inputs = prepare_sample_inputs(args.dataset_root, Path(temp), args.samples_per_dataset, args.seed)
        if args.raw_source_root:
            import_raw_sources(args.raw_source_root.resolve(), args.output_dir)
        else:
            run_evaluators(
                args.repo_root, inputs, args.output_dir, endpoint=args.endpoint,
                model_endpoint=str(model["base_url"]), model_name=str(model["model"]),
                batch_size=args.concurrency, seed=args.seed,
            )
    summaries: dict[str, Any] = {}
    files = []
    for task in TASKS:
        trace_path = args.output_dir / f"{task}.jsonl"
        count = convert_task(task, args.output_dir / "raw" / task, trace_path, model=model, tokenizer=tokenizer)
        if count < args.samples_per_dataset:
            raise RuntimeError(f"{task}: only {count} valid real traces; required {args.samples_per_dataset}")
        summaries[task] = validate_trace_file(trace_path, dataset=task, minimum=args.samples_per_dataset)
        files.append({"path": trace_path.name, "size_bytes": trace_path.stat().st_size, "sha256": sha256_file(trace_path)})
    manifest = {
        "schema_version": 1, "seed": args.seed, "source_model": model["model"],
        "source_version": model["source_version"], "collected_at": model["collected_at"],
        "tokenizer": args.tokenizer, "datasets": summaries, "files": files,
    }
    (args.output_dir / "trace_manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True), encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
