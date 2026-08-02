#!/usr/bin/env python3
"""将单个压测场景的统一产物原样归档为报告输入。

数据集可使用不同的执行器，但都必须提供同一组资源证据。该工具只复制
已完成 run 的原始产物，不计算或补写监控数值。若缺少 fleet 资源文件或其
字段不完整，立即失败，避免报告用 host watchdog 的简化字段替代进程级指标。
"""

from __future__ import annotations

import argparse
import csv
import json
from pathlib import Path
import shutil
import tempfile


REQUIRED_RESOURCE_COLUMNS = {
    "timestamp", "run_id", "processes", "rss_bytes", "open_fds", "threads",
    "mem_total_bytes", "available_bytes", "load1", "load5", "load15",
    "worker_exits", "oom_events",
}

# 报告输入长期使用这两个稳定节点标签。保留未知主机原始标签，避免静默丢失数据。
HOST_FILE_TAGS = {
    "8.130.65.20": "node20",
    "8.145.51.129": "node129",
}


def read_json(path: Path) -> dict:
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise SystemExit(f"cannot read {path}: {exc}") from exc
    if not isinstance(document, dict):
        raise SystemExit(f"{path} must contain a JSON object")
    return document


def copy_atomic(source: Path, destination: Path) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(
        prefix=destination.name + ".", suffix=".tmp", dir=destination.parent,
        delete=False,
    ) as handle:
        temporary = Path(handle.name)
    try:
        shutil.copyfile(source, temporary)
        temporary.replace(destination)
    finally:
        temporary.unlink(missing_ok=True)


def validate_resource_csv(path: Path, expected_run_id: str, allow_empty_run_id: bool) -> None:
    with path.open(encoding="utf-8", newline="") as source:
        reader = csv.DictReader(source)
        fields = set(reader.fieldnames or [])
        missing = sorted(REQUIRED_RESOURCE_COLUMNS - fields)
        if missing:
            raise SystemExit(f"{path} lacks required resource columns: {', '.join(missing)}")
        rows = list(reader)
    if not rows:
        raise SystemExit(f"{path} has no resource samples")
    observed = {row.get("run_id", "") for row in rows if row.get("run_id", "")}
    if not observed and not allow_empty_run_id:
        raise SystemExit(
            f"{path} has no run_id values; rerun with a current fleet supervisor or "
            "use --allow-empty-run-id only for an already completed legacy run"
        )
    if observed and observed != {expected_run_id}:
        raise SystemExit(
            f"{path} run_id mismatch: expected {expected_run_id}, got {sorted(observed)}"
        )


def output_name(path: Path) -> str:
    prefix, separator, host = path.stem.rpartition("-")
    tag = HOST_FILE_TAGS.get(host, host)
    return f"{prefix}{separator}{tag}{path.suffix}"


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--source", required=True, type=Path, help="completed scenario artifact directory")
    parser.add_argument("--output", required=True, type=Path, help="dataset directory below report input")
    parser.add_argument("--mode", default="sync", help="result/observation suffix")
    parser.add_argument(
        "--resources-only", action="store_true",
        help="copy only the common fleet resource evidence after verifying the staged result run_id",
    )
    parser.add_argument(
        "--allow-empty-run-id", action="store_true",
        help="accept legacy completed fleet CSVs that predate the run_id column population",
    )
    args = parser.parse_args()

    source = args.source.resolve()
    output = args.output.resolve()
    result_path = source / "result.json"
    observations_path = source / "episode-observations.jsonl"
    if not result_path.is_file() or not observations_path.is_file():
        raise SystemExit("source must contain result.json and episode-observations.jsonl")
    run_id = str(read_json(result_path).get("run_id") or "")
    if not run_id:
        raise SystemExit("result.json lacks run_id")

    resources = sorted(source.glob("fleet-resources-*.csv"))
    metrics = sorted(source.glob("fleet-metrics-*.json"))
    if not resources or not metrics or len(resources) != len(metrics):
        raise SystemExit("source must contain matching fleet-resources-*.csv and fleet-metrics-*.json files")
    for resource in resources:
        validate_resource_csv(resource, run_id, args.allow_empty_run_id)

    if args.resources_only:
        staged_result = output / f"result-{args.mode}.json"
        if not staged_result.is_file():
            raise SystemExit(f"resources-only requires staged result: {staged_result}")
        staged_run_id = str(read_json(staged_result).get("run_id") or "")
        if staged_run_id != run_id:
            raise SystemExit(
                f"staged result run_id mismatch: expected {run_id}, got {staged_run_id or '<missing>'}"
            )
    else:
        copy_atomic(result_path, output / f"result-{args.mode}.json")
        copy_atomic(observations_path, output / f"episode-observations-{args.mode}.jsonl")
        for optional_name in ("host-watchdog.jsonl", "resource-summary.json", "manifest.json"):
            optional = source / optional_name
            if optional.is_file():
                copy_atomic(optional, output / optional_name)
    for artifact in resources + metrics:
        copy_atomic(artifact, output / output_name(artifact))

    print(json.dumps({
        "run_id": run_id,
        "output": str(output),
        "resource_nodes": len(resources),
        "resource_samples": {
            item.stem.replace("fleet-resources-", ""): sum(1 for _ in item.open(encoding="utf-8")) - 1
            for item in resources
        },
    }, ensure_ascii=False, sort_keys=True))


if __name__ == "__main__":
    main()
