#!/usr/bin/env python3
from __future__ import annotations

import argparse
import ast
import csv
import json
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path
from typing import Any


def read_csv_rows(path: Path) -> tuple[list[str], list[dict[str, str]]]:
    csv.field_size_limit(sys.maxsize)
    with path.open(newline="", encoding="utf-8") as file:
        reader = csv.DictReader(file)
        rows = list(reader)
        if reader.fieldnames is None:
            raise ValueError(f"CSV has no header: {path}")
        return list(reader.fieldnames), rows


def write_csv_rows(path: Path, fieldnames: list[str], rows: list[dict[str, str]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", newline="", encoding="utf-8") as file:
        writer = csv.DictWriter(file, fieldnames=fieldnames)
        writer.writeheader()
        writer.writerows(rows)


def read_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def write_json(path: Path, obj: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(obj, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def write_lines(path: Path, lines: list[str]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("".join(f"{line}\n" for line in lines), encoding="utf-8")


def load_instance_filter(path: Path | None) -> set[str] | None:
    if path is None:
        return None
    ids = {line.strip() for line in path.read_text(encoding="utf-8").splitlines() if line.strip()}
    return ids


def chunked(items: list[dict[str, Any]], size: int) -> list[list[dict[str, Any]]]:
    return [items[idx : idx + size] for idx in range(0, len(items), size)]


def sanitize_batch_name(idx: int) -> str:
    return f"batch_{idx:05d}"


def parse_pull_failed_ids(log_text: str) -> list[str]:
    pattern = re.compile(r"Failed to pull or find image locally for ([^:]+):")
    return sorted(set(pattern.findall(log_text)))


def parse_returned_none_ids(log_text: str) -> list[str]:
    pattern = re.compile(r"Evaluation for ([^ ]+) returned None")
    return sorted(set(pattern.findall(log_text)))


def find_output_json(eval_dir: Path, instance_id: str) -> Path | None:
    instance_dir = eval_dir / instance_id
    if not instance_dir.exists():
        return None
    outputs = sorted(instance_dir.glob("*_output.json"))
    return outputs[-1] if outputs else None


def evaluate_output_json(output_path: Path, raw_row: dict[str, str]) -> bool | None:
    try:
        output = read_json(output_path)
        tests = output.get("tests")
        if not isinstance(tests, list):
            return None
        passed_tests = {item["name"] for item in tests if item.get("status") == "PASSED"}
        fail_to_pass = set(ast.literal_eval(raw_row["fail_to_pass"]))
        pass_to_pass = set(ast.literal_eval(raw_row["pass_to_pass"]))
        return (fail_to_pass | pass_to_pass) <= passed_tests
    except (KeyError, ValueError, SyntaxError, json.JSONDecodeError):
        return None


def collect_output_results(
    eval_dir: Path,
    instance_ids: list[str],
    raw_by_id: dict[str, dict[str, str]],
) -> tuple[dict[str, bool], list[str]]:
    results: dict[str, bool] = {}
    invalid_outputs: list[str] = []
    for instance_id in instance_ids:
        output_path = find_output_json(eval_dir, instance_id)
        if output_path is None:
            continue
        result = evaluate_output_json(output_path, raw_by_id[instance_id])
        if result is None:
            invalid_outputs.append(instance_id)
            continue
        results[instance_id] = result
    return results, invalid_outputs


def docker_rmi(refs: list[str], log_path: Path) -> dict[str, Any]:
    removed: list[str] = []
    failed: list[str] = []
    with log_path.open("a", encoding="utf-8") as log:
        for ref in refs:
            proc = subprocess.run(
                ["docker", "rmi", ref],
                stdout=log,
                stderr=subprocess.STDOUT,
                text=True,
            )
            if proc.returncode == 0:
                removed.append(ref)
            else:
                failed.append(ref)
    return {"removed": removed, "failed": failed}


def docker_system_df() -> str:
    proc = subprocess.run(
        ["docker", "system", "df"],
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    return proc.stdout


def check_official_evaluator(official_eval_dir: Path) -> None:
    evaluator = official_eval_dir / "swe_bench_pro_eval.py"
    if not evaluator.exists():
        raise FileNotFoundError(f"official evaluator not found: {evaluator}")
    text = evaluator.read_text(encoding="utf-8")
    if "dockerhub_tag" not in text:
        print(
            "WARNING: official evaluator does not appear to use dataset dockerhub_tag. "
            "SWE-bench-Pro image tags may be generated incorrectly.",
            file=sys.stderr,
        )


def prepare_batches(args: argparse.Namespace) -> tuple[list[str], list[list[dict[str, Any]]], dict[str, dict[str, str]]]:
    fieldnames, raw_rows = read_csv_rows(args.raw_sample_csv)
    raw_by_id = {row["instance_id"]: row for row in raw_rows}
    patches = read_json(args.patches)
    if not isinstance(patches, list):
        raise ValueError(f"patches must be a JSON list: {args.patches}")

    wanted_ids = load_instance_filter(args.instance_id_file)
    selected: list[dict[str, Any]] = []
    missing_raw: list[str] = []
    for patch in patches:
        instance_id = patch["instance_id"]
        if wanted_ids is not None and instance_id not in wanted_ids:
            continue
        if instance_id not in raw_by_id:
            missing_raw.append(instance_id)
            continue
        selected.append(patch)

    if missing_raw:
        raise ValueError(f"{len(missing_raw)} patch instance ids missing from raw CSV; first={missing_raw[:5]}")
    if args.limit is not None:
        selected = selected[: args.limit]
    if not selected:
        raise ValueError("no selected patches to evaluate")
    return fieldnames, chunked(selected, args.batch_size), raw_by_id


def batch_status_path(batch_dir: Path) -> Path:
    return batch_dir / "status.json"


def is_batch_done(batch_dir: Path) -> bool:
    path = batch_status_path(batch_dir)
    if not path.exists():
        return False
    try:
        status = read_json(path)
    except json.JSONDecodeError:
        return False
    return status.get("status") == "done"


def run_batch(
    args: argparse.Namespace,
    batch_idx: int,
    fieldnames: list[str],
    patches: list[dict[str, Any]],
    raw_by_id: dict[str, dict[str, str]],
) -> dict[str, Any]:
    batch_name = sanitize_batch_name(batch_idx)
    batch_dir = args.batch_root / batch_name
    eval_dir = batch_dir / "official_eval"
    sample_csv = batch_dir / "samples.csv"
    patch_json = batch_dir / "patches.json"
    evaluator_log = batch_dir / "evaluator.log"
    cleanup_log = batch_dir / "cleanup.log"
    status_path = batch_status_path(batch_dir)

    if is_batch_done(batch_dir) and not args.redo:
        status = read_json(status_path)
        print(f"SKIP {batch_name}: done")
        return status

    batch_dir.mkdir(parents=True, exist_ok=True)
    if args.redo and eval_dir.exists():
        shutil.rmtree(eval_dir)
    instance_ids = [patch["instance_id"] for patch in patches]
    existing_results: dict[str, bool] = {}
    invalid_existing_outputs: list[str] = []
    if args.skip_completed_instances and not args.redo:
        existing_results, invalid_existing_outputs = collect_output_results(eval_dir, instance_ids, raw_by_id)

    pending_patches = [patch for patch in patches if patch["instance_id"] not in existing_results]
    pending_instance_ids = [patch["instance_id"] for patch in pending_patches]
    rows = [raw_by_id[instance_id] for instance_id in pending_instance_ids]
    write_csv_rows(sample_csv, fieldnames, rows)
    write_json(patch_json, pending_patches)

    status: dict[str, Any] = {
        "batch": batch_name,
        "batch_idx": batch_idx,
        "status": "running",
        "instance_count": len(instance_ids),
        "instance_ids": instance_ids,
        "pending_instance_count": len(pending_instance_ids),
        "pending_instance_ids": pending_instance_ids,
        "skipped_completed_count": len(existing_results),
        "skipped_completed_ids": sorted(existing_results),
        "invalid_existing_outputs": invalid_existing_outputs,
        "dockerhub_username": args.dockerhub_username,
        "started_at": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
        "sample_csv": str(sample_csv),
        "patch_json": str(patch_json),
        "eval_dir": str(eval_dir),
        "log": str(evaluator_log),
    }
    write_json(status_path, status)

    cmd = [
        str(args.python),
        str(args.official_eval_dir / "swe_bench_pro_eval.py"),
        "--raw_sample_path",
        str(sample_csv),
        "--patch_path",
        str(patch_json),
        "--output_dir",
        str(eval_dir),
        "--scripts_dir",
        str(args.official_eval_dir / "run_scripts"),
        "--dockerhub_username",
        args.dockerhub_username,
        "--num_workers",
        str(args.official_num_workers),
        "--use_local_docker",
        "--redo",
    ]

    print(
        f"RUN {batch_name}: cases={len(instance_ids)} pending={len(pending_instance_ids)} "
        f"skipped_completed={len(existing_results)} dockerhub={args.dockerhub_username}"
    )
    started = time.time()
    if pending_patches:
        with evaluator_log.open("w", encoding="utf-8") as log:
            log.write("$ " + " ".join(cmd) + "\n")
            log.flush()
            try:
                proc = subprocess.run(
                    cmd,
                    cwd=args.official_eval_dir,
                    stdout=log,
                    stderr=subprocess.STDOUT,
                    text=True,
                    timeout=args.batch_timeout_seconds if args.batch_timeout_seconds > 0 else None,
                )
                returncode = proc.returncode
                timed_out = False
            except subprocess.TimeoutExpired:
                returncode = 124
                timed_out = True
                log.write(f"\nBATCH_TIMEOUT seconds={args.batch_timeout_seconds}\n")
    else:
        evaluator_log.write_text("All instances already have output.json; skipped evaluator.\n", encoding="utf-8")
        returncode = 0
        timed_out = False

    elapsed = time.time() - started
    log_text = evaluator_log.read_text(encoding="utf-8", errors="replace")
    pull_failed_ids = parse_pull_failed_ids(log_text)
    returned_none_ids = parse_returned_none_ids(log_text)
    results_path = eval_dir / "eval_results.json"
    eval_results = read_json(results_path) if results_path.exists() else {}
    if not isinstance(eval_results, dict):
        eval_results = {}
    output_results, invalid_output_ids = collect_output_results(eval_dir, instance_ids, raw_by_id)
    merged_eval_results = {**existing_results, **eval_results, **output_results}
    if merged_eval_results:
        write_json(results_path, merged_eval_results)
    write_json(batch_dir / "existing_output_results.json", existing_results)
    output_json_count = len(list(eval_dir.glob("*/**/*_output.json"))) if eval_dir.exists() else 0

    cleanup_result = None
    if args.clean_images_after_batch:
        refs = [
            f"{args.dockerhub_username}/sweap-images:{row.get('dockerhub_tag', '').strip()}"
            for row in rows
            if row.get("dockerhub_tag", "").strip()
        ]
        cleanup_log.write_text("docker system df before cleanup\n" + docker_system_df() + "\n", encoding="utf-8")
        cleanup_result = docker_rmi(refs, cleanup_log)
        with cleanup_log.open("a", encoding="utf-8") as log:
            log.write("\ndocker system df after cleanup\n")
            log.write(docker_system_df())

    status.update(
        {
            "status": "done" if len(merged_eval_results) == len(instance_ids) and returncode == 0 else "failed",
            "finished_at": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
            "elapsed_seconds": elapsed,
            "returncode": returncode,
            "timed_out": timed_out,
            "eval_results": str(results_path) if results_path.exists() else None,
            "evaluated_count": len(merged_eval_results),
            "resolved_count": sum(1 for value in merged_eval_results.values() if bool(value)),
            "pull_failed_count": len(pull_failed_ids),
            "pull_failed_ids": pull_failed_ids,
            "returned_none_count": len(returned_none_ids),
            "returned_none_ids": returned_none_ids,
            "invalid_output_count": len(invalid_output_ids),
            "invalid_output_ids": invalid_output_ids,
            "output_json_count": output_json_count,
            "cleanup": cleanup_result,
        }
    )
    write_json(status_path, status)
    print(
        f"{status['status'].upper()} {batch_name}: "
        f"evaluated={status['evaluated_count']} resolved={status['resolved_count']} "
        f"pull_failed={status['pull_failed_count']} elapsed={elapsed:.1f}s"
    )
    return status


def iter_statuses(batch_roots: list[Path]) -> list[dict[str, Any]]:
    statuses: list[dict[str, Any]] = []
    for batch_root in batch_roots:
        for status_file in sorted(batch_root.glob("batch_*/status.json")):
            try:
                status = read_json(status_file)
            except json.JSONDecodeError:
                continue
            status["_batch_root"] = str(batch_root)
            statuses.append(status)
    return statuses


def merge_results(args: argparse.Namespace) -> dict[str, Any]:
    batch_roots = [*args.extra_merge_root, args.batch_root]
    statuses = iter_statuses(batch_roots)
    merged: dict[str, bool] = {}
    latest_pull_failed: dict[str, bool] = {}
    latest_returned_none: dict[str, bool] = {}

    for status in statuses:
        pull_failed_in_batch = set(status.get("pull_failed_ids") or [])
        returned_none_in_batch = set(status.get("returned_none_ids") or [])
        for instance_id in status.get("instance_ids") or []:
            latest_pull_failed[instance_id] = instance_id in pull_failed_in_batch
            latest_returned_none[instance_id] = instance_id in returned_none_in_batch

        results_path = status.get("eval_results")
        if results_path and Path(results_path).exists():
            results = read_json(Path(results_path))
            if isinstance(results, dict):
                for instance_id, value in results.items():
                    merged[instance_id] = bool(value)

    resolved = sum(1 for value in merged.values() if value)
    metrics = {
        "evaluated_count": len(merged),
        "resolved_count": resolved,
        "resolve_rate": resolved / len(merged) if merged else 0.0,
        "batch_count": len(statuses),
        "done_batch_count": sum(1 for item in statuses if item.get("status") == "done"),
        "failed_batch_count": sum(1 for item in statuses if item.get("status") == "failed"),
        "pull_failed_count": sum(1 for value in latest_pull_failed.values() if value),
        "returned_none_count": sum(1 for value in latest_returned_none.values() if value),
        "batch_root": str(args.batch_root),
        "extra_merge_roots": [str(path) for path in args.extra_merge_root],
        "dockerhub_username": args.dockerhub_username,
    }

    write_json(args.batch_root / "merged_eval_results.json", merged)
    write_json(args.batch_root / "official_metrics.json", metrics)
    write_json(args.batch_root / "batch_statuses.json", statuses)
    write_lines(
        args.batch_root / "pull_failed_instance_ids.txt",
        sorted(instance_id for instance_id, failed in latest_pull_failed.items() if failed),
    )
    write_lines(
        args.batch_root / "returned_none_instance_ids.txt",
        sorted(instance_id for instance_id, failed in latest_returned_none.items() if failed),
    )
    print(json.dumps(metrics, ensure_ascii=False, indent=2))
    return metrics


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Run SWE-bench-Pro official evaluator in disk-bounded batches")
    parser.add_argument("--raw-sample-csv", type=Path, required=True)
    parser.add_argument("--patches", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--batch-root", type=Path, default=None)
    parser.add_argument("--official-eval-dir", type=Path, required=True)
    parser.add_argument("--python", type=Path, required=True)
    parser.add_argument("--dockerhub-username", default="docker.1panel.live/jefzda")
    parser.add_argument("--batch-size", type=int, default=10)
    parser.add_argument("--official-num-workers", type=int, default=1)
    parser.add_argument("--batch-timeout-seconds", type=int, default=0)
    parser.add_argument("--limit", type=int, default=None)
    parser.add_argument("--max-batches", type=int, default=None)
    parser.add_argument("--start-batch", type=int, default=0)
    parser.add_argument("--instance-id-file", type=Path, default=None)
    parser.add_argument("--extra-merge-root", type=Path, action="append", default=[])
    parser.add_argument("--redo", action="store_true")
    parser.add_argument("--no-skip-completed-instances", dest="skip_completed_instances", action="store_false")
    parser.add_argument("--no-clean-images-after-batch", dest="clean_images_after_batch", action="store_false")
    parser.set_defaults(clean_images_after_batch=True, skip_completed_instances=True)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.batch_size <= 0:
        raise ValueError("--batch-size must be positive")
    args.output_dir.mkdir(parents=True, exist_ok=True)
    if args.batch_root is None:
        args.batch_root = args.output_dir / "official_eval_batches"
    args.batch_root.mkdir(parents=True, exist_ok=True)
    check_official_evaluator(args.official_eval_dir)

    fieldnames, batches, raw_by_id = prepare_batches(args)
    write_json(
        args.batch_root / "run_config.json",
        {
            "raw_sample_csv": str(args.raw_sample_csv),
            "patches": str(args.patches),
            "output_dir": str(args.output_dir),
            "batch_root": str(args.batch_root),
            "official_eval_dir": str(args.official_eval_dir),
            "python": str(args.python),
            "dockerhub_username": args.dockerhub_username,
            "batch_size": args.batch_size,
            "official_num_workers": args.official_num_workers,
            "batch_timeout_seconds": args.batch_timeout_seconds,
            "limit": args.limit,
            "max_batches": args.max_batches,
            "start_batch": args.start_batch,
            "instance_id_file": str(args.instance_id_file) if args.instance_id_file else None,
            "clean_images_after_batch": args.clean_images_after_batch,
            "skip_completed_instances": args.skip_completed_instances,
        },
    )

    run_count = 0
    for batch_idx, patches in enumerate(batches):
        if batch_idx < args.start_batch:
            continue
        if args.max_batches is not None and run_count >= args.max_batches:
            break
        batch_dir = args.batch_root / sanitize_batch_name(batch_idx)
        if is_batch_done(batch_dir) and not args.redo:
            run_batch(args, batch_idx, fieldnames, patches, raw_by_id)
            continue
        run_batch(args, batch_idx, fieldnames, patches, raw_by_id)
        run_count += 1

    merge_results(args)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
