#!/usr/bin/env python3
from __future__ import annotations

import argparse
import concurrent.futures
import json
import re
import time
import urllib.error
import urllib.request
from collections import Counter
from pathlib import Path
from typing import Any

DEFAULT_DATA_DIR = Path("/data/ronghao/uenv/uenv-bridge/data/benchmarks/swebenchpro")


def _read_jsonl(path: Path) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    with path.open("r", encoding="utf-8") as file:
        for line in file:
            if line.strip():
                rows.append(json.loads(line))
    return rows


def _write_json(path: Path, obj: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(obj, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def _write_jsonl(path: Path, rows: list[dict[str, Any]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as file:
        for row in rows:
            file.write(json.dumps(row, ensure_ascii=False) + "\n")


def _normalise_row(row: dict[str, Any], idx: int) -> dict[str, Any]:
    normalised = dict(row)
    normalised["id"] = idx
    for key in (
        "fail_to_pass",
        "pass_to_pass",
        "selected_test_files_to_run",
        "requirements",
        "interface",
        "issue_specificity",
        "issue_categories",
    ):
        value = normalised.get(key)
        if isinstance(value, (list, dict)):
            normalised[key] = json.dumps(value, ensure_ascii=False)
        elif value is None:
            normalised[key] = ""
    return normalised


def prepare(args: argparse.Namespace) -> None:
    from datasets import load_dataset
    import pandas as pd

    dataset = load_dataset(args.dataset, split=args.split)
    rows = [_normalise_row(dict(row), idx) for idx, row in enumerate(dataset)]
    if args.limit is not None:
        rows = rows[: args.limit]

    args.output_dir.mkdir(parents=True, exist_ok=True)
    jsonl_path = args.output_dir / "test.jsonl"
    csv_path = args.output_dir / "swe_bench_pro_full.csv"
    _write_jsonl(jsonl_path, rows)
    pd.DataFrame(rows).to_csv(csv_path, index=False)

    summary = {
        "dataset": args.dataset,
        "split": args.split,
        "sample_count": len(rows),
        "repo_distribution": Counter(row["repo"] for row in rows),
        "language_distribution": Counter(row.get("repo_language", "") for row in rows),
        "jsonl": str(jsonl_path),
        "csv": str(csv_path),
    }
    _write_json(args.output_dir / "dataset_summary.json", summary)
    print(json.dumps(summary, ensure_ascii=False, indent=2))


def build_prompt(row: dict[str, Any]) -> str:
    return (
        "You are an expert software engineer. Generate a minimal git unified diff patch "
        "that fixes the issue in the repository.\n\n"
        "Return only the patch. The response must start with `diff --git` and must not "
        "include markdown fences or explanation.\n\n"
        f"Repository: {row.get('repo', '')}\n"
        f"Language: {row.get('repo_language', '')}\n"
        f"Base commit: {row.get('base_commit', '')}\n\n"
        "<issue>\n"
        f"{row.get('problem_statement', '')}\n"
        "</issue>\n\n"
        "<requirements>\n"
        f"{row.get('requirements', '')}\n"
        "</requirements>\n\n"
        "<interface>\n"
        f"{row.get('interface', '')}\n"
        "</interface>\n"
    )


def build_messages(row: dict[str, Any]) -> list[dict[str, str]]:
    return [
        {
            "role": "system",
            "content": "You generate correct, minimal source-code patches in git unified diff format.",
        },
        {"role": "user", "content": build_prompt(row)},
    ]


def parse_patch(response: str) -> str:
    text = response.strip()
    fence = re.search(r"```(?:diff|patch)?\s*(.*?)```", text, flags=re.DOTALL | re.IGNORECASE)
    if fence:
        text = fence.group(1).strip()

    start = text.find("diff --git ")
    if start >= 0:
        return text[start:].strip() + "\n"

    # Some models omit the `diff --git` header but still emit a unified diff.
    start = re.search(r"(?m)^---\s+[ab]/", text)
    if start:
        return text[start.start() :].strip() + "\n"

    return ""


def _patch_format(parsed_patch: str) -> dict[str, bool]:
    return {
        "nonempty_patch": bool(parsed_patch.strip()),
        "has_diff_git": bool(re.search(r"(?m)^diff --git ", parsed_patch)),
        "has_hunk": bool(re.search(r"(?m)^@@ ", parsed_patch)),
    }


def generate(args: argparse.Namespace) -> None:
    from transformers import AutoTokenizer
    from vllm import LLM, SamplingParams

    rows = _read_jsonl(args.data)
    if args.limit is not None:
        rows = rows[: args.limit]

    tokenizer = AutoTokenizer.from_pretrained(args.model, trust_remote_code=True)
    prompts = [
        tokenizer.apply_chat_template(
            build_messages(row),
            tokenize=False,
            add_generation_prompt=True,
            **({"enable_thinking": False} if args.disable_thinking else {}),
        )
        for row in rows
    ]

    llm = LLM(
        model=args.model,
        tensor_parallel_size=args.tensor_parallel_size,
        trust_remote_code=True,
        dtype=args.dtype,
        gpu_memory_utilization=args.gpu_memory_utilization,
        max_model_len=args.max_model_len,
        enforce_eager=args.enforce_eager,
    )
    sampling = SamplingParams(
        temperature=args.temperature,
        top_p=args.top_p,
        max_tokens=args.max_tokens,
        stop=args.stop,
    )
    outputs = llm.generate(prompts, sampling)

    args.output_dir.mkdir(parents=True, exist_ok=True)
    generations: list[dict[str, Any]] = []
    patches: list[dict[str, str]] = []
    for idx, (row, output) in enumerate(zip(rows, outputs, strict=True)):
        response = output.outputs[0].text
        patch = parse_patch(response)
        fmt = _patch_format(patch)
        generations.append(
            {
                "id": idx,
                "instance_id": row["instance_id"],
                "repo": row.get("repo", ""),
                "repo_language": row.get("repo_language", ""),
                "response": response,
                "parsed_patch": patch,
                "output_tokens": len(output.outputs[0].token_ids),
                **fmt,
            }
        )
        patches.append(
            {
                "instance_id": row["instance_id"],
                "patch": patch,
                "prefix": args.prefix,
            }
        )

    _write_json(args.output_dir / "generations.json", generations)
    _write_json(args.output_dir / "patches.json", patches)
    summarize_generation(args.output_dir, rows, generations)
    print(json.dumps({"generated": len(generations), "output": str(args.output_dir)}, indent=2))


def summarize_generation(
    output_dir: Path,
    rows: list[dict[str, Any]],
    generations: list[dict[str, Any]],
) -> dict[str, Any]:
    output_tokens = [int(row.get("output_tokens", 0)) for row in generations]
    nonempty = sum(1 for row in generations if row.get("nonempty_patch"))
    has_diff_git = sum(1 for row in generations if row.get("has_diff_git"))
    has_hunk = sum(1 for row in generations if row.get("has_hunk"))
    summary: dict[str, Any] = {
        "sample_count": len(generations),
        "dataset_count": len(rows),
        "nonempty_patch_count": nonempty,
        "diff_git_patch_count": has_diff_git,
        "hunk_patch_count": has_hunk,
        "nonempty_patch_rate": nonempty / len(generations) if generations else 0.0,
        "diff_git_patch_rate": has_diff_git / len(generations) if generations else 0.0,
        "hunk_patch_rate": has_hunk / len(generations) if generations else 0.0,
        "repo_distribution": Counter(row.get("repo", "") for row in rows),
        "language_distribution": Counter(row.get("repo_language", "") for row in rows),
        "output_tokens_min": min(output_tokens) if output_tokens else 0,
        "output_tokens_max": max(output_tokens) if output_tokens else 0,
        "output_tokens_avg": sum(output_tokens) / len(output_tokens) if output_tokens else 0.0,
    }
    _write_json(output_dir / "generation_metrics.json", summary)
    return summary


def summarize_official_eval(output_dir: Path, official_results: Path | None) -> dict[str, Any] | None:
    if official_results is None or not official_results.exists():
        return None
    results = json.loads(official_results.read_text(encoding="utf-8"))
    if not isinstance(results, dict):
        return None
    resolved = sum(1 for value in results.values() if bool(value))
    total = len(results)
    metrics = {
        "evaluated_count": total,
        "resolved_count": resolved,
        "resolve_rate": resolved / total if total else 0.0,
    }
    _write_json(output_dir / "official_metrics.json", metrics)
    return metrics


def summarize(args: argparse.Namespace) -> None:
    rows = _read_jsonl(args.data)
    generations_path = args.output_dir / "generations.json"
    if generations_path.exists():
        generations = json.loads(generations_path.read_text(encoding="utf-8"))
        gen_metrics = summarize_generation(args.output_dir, rows, generations)
    else:
        gen_metrics = None
    official_metrics = summarize_official_eval(args.output_dir, args.official_results)
    print(json.dumps({"generation": gen_metrics, "official": official_metrics}, ensure_ascii=False, indent=2))


def export_gold_patches(args: argparse.Namespace) -> None:
    rows = _read_jsonl(args.data)
    patches = [
        {"instance_id": row["instance_id"], "patch": row.get("patch", ""), "prefix": args.prefix}
        for row in rows
    ]
    _write_json(args.output, patches)
    print(json.dumps({"exported": len(patches), "output": str(args.output)}, indent=2))


def download_official_assets(args: argparse.Namespace) -> None:
    rows = _read_jsonl(args.data)
    if args.limit is not None:
        rows = rows[: args.limit]

    required_paths: list[str] = []
    for row in rows:
        instance_id = row["instance_id"]
        required_paths.extend(
            [
                f"run_scripts/{instance_id}/run_script.sh",
                f"run_scripts/{instance_id}/parser.py",
                f"dockerfiles/base_dockerfile/{instance_id}/Dockerfile",
                f"dockerfiles/instance_dockerfile/{instance_id}/Dockerfile",
            ]
        )

    skipped_paths: list[str] = []
    paths_to_download: list[str] = []
    for rel_path in required_paths:
        dst = args.official_eval_dir / rel_path
        if dst.exists() and dst.stat().st_size > 0 and not args.redownload:
            skipped_paths.append(rel_path)
            continue
        paths_to_download.append(rel_path)

    def _download_one(rel_path: str) -> dict[str, str]:
        dst = args.official_eval_dir / rel_path
        dst.parent.mkdir(parents=True, exist_ok=True)
        url = args.base_url.rstrip("/") + "/" + rel_path
        last_error = ""
        for attempt in range(1, args.retries + 1):
            try:
                with urllib.request.urlopen(url, timeout=args.timeout_seconds) as response:
                    data = response.read()
                dst.write_bytes(data)
                return {"path": rel_path, "status": "downloaded", "size": str(len(data))}
            except (OSError, urllib.error.URLError, urllib.error.HTTPError) as exc:
                last_error = f"{type(exc).__name__}: {exc}"
                if attempt < args.retries:
                    time.sleep(args.retry_delay_seconds)
        return {"path": rel_path, "status": "failed", "url": url, "error": last_error}

    results: list[dict[str, str]] = []
    if paths_to_download:
        with concurrent.futures.ThreadPoolExecutor(max_workers=args.workers) as executor:
            futures = [executor.submit(_download_one, rel_path) for rel_path in paths_to_download]
            for future in concurrent.futures.as_completed(futures):
                results.append(future.result())

    downloaded = sum(1 for item in results if item["status"] == "downloaded")
    failed = [item for item in results if item["status"] == "failed"]

    summary = {
        "instance_count": len(rows),
        "required_file_count": len(required_paths),
        "downloaded_count": downloaded,
        "skipped_count": len(skipped_paths),
        "failed_count": len(failed),
        "failed": failed[:20],
        "official_eval_dir": str(args.official_eval_dir),
        "base_url": args.base_url,
        "workers": args.workers,
    }
    _write_json(args.official_eval_dir / "official_assets_summary.json", summary)
    print(json.dumps(summary, ensure_ascii=False, indent=2))


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="SWE-bench-Pro baseline helper")
    sub = parser.add_subparsers(dest="command", required=True)

    prep = sub.add_parser("prepare")
    prep.add_argument("--dataset", default="ScaleAI/SWE-bench_Pro")
    prep.add_argument("--split", default="test")
    prep.add_argument("--output-dir", type=Path, default=DEFAULT_DATA_DIR)
    prep.add_argument("--limit", type=int, default=None)
    prep.set_defaults(func=prepare)

    gen = sub.add_parser("generate")
    gen.add_argument("--data", type=Path, default=DEFAULT_DATA_DIR / "test.jsonl")
    gen.add_argument("--model", required=True)
    gen.add_argument("--output-dir", type=Path, required=True)
    gen.add_argument("--prefix", default="qwen3_6_35b_a3b")
    gen.add_argument("--limit", type=int, default=None)
    gen.add_argument("--tensor-parallel-size", type=int, default=8)
    gen.add_argument("--max-model-len", type=int, default=16384)
    gen.add_argument("--max-tokens", type=int, default=4096)
    gen.add_argument("--gpu-memory-utilization", type=float, default=0.9)
    gen.add_argument("--temperature", type=float, default=0.2)
    gen.add_argument("--top-p", type=float, default=1.0)
    gen.add_argument("--dtype", default="bfloat16")
    gen.add_argument("--enforce-eager", action="store_true")
    gen.add_argument("--disable-thinking", action="store_true")
    gen.add_argument("--stop", action="append", default=[])
    gen.set_defaults(func=generate)

    summ = sub.add_parser("summarize")
    summ.add_argument("--data", type=Path, default=DEFAULT_DATA_DIR / "test.jsonl")
    summ.add_argument("--output-dir", type=Path, required=True)
    summ.add_argument("--official-results", type=Path, default=None)
    summ.set_defaults(func=summarize)

    gold = sub.add_parser("export-gold-patches")
    gold.add_argument("--data", type=Path, default=DEFAULT_DATA_DIR / "test.jsonl")
    gold.add_argument("--output", type=Path, required=True)
    gold.add_argument("--prefix", default="gold")
    gold.set_defaults(func=export_gold_patches)

    assets = sub.add_parser("download-official-assets")
    assets.add_argument("--data", type=Path, default=DEFAULT_DATA_DIR / "test.jsonl")
    assets.add_argument("--official-eval-dir", type=Path, required=True)
    assets.add_argument(
        "--base-url",
        default="https://cdn.jsdelivr.net/gh/scaleapi/SWE-bench_Pro-os@main",
    )
    assets.add_argument("--limit", type=int, default=None)
    assets.add_argument("--timeout-seconds", type=float, default=60.0)
    assets.add_argument("--retries", type=int, default=3)
    assets.add_argument("--retry-delay-seconds", type=float, default=2.0)
    assets.add_argument("--workers", type=int, default=16)
    assets.add_argument("--redownload", action="store_true")
    assets.set_defaults(func=download_official_assets)

    return parser.parse_args()


def main() -> int:
    args = parse_args()
    args.func(args)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
