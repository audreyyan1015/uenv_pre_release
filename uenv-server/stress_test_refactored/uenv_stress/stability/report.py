#!/usr/bin/env python3
"""Generate the five-metric UEnv stability acceptance report from suite CSVs."""

from __future__ import annotations

import argparse
import csv
import json
import math
import sqlite3
from collections import Counter, defaultdict
from pathlib import Path
from typing import Any

from uenv_stress.core.stability_test_common import (
    TASK_NAMES,
    classify_availability,
    percentile,
)


def read_csv(path: Path) -> list[dict[str, str]]:
    if not path.is_file():
        return []
    with path.open("r", encoding="utf-8", newline="") as source:
        return list(csv.DictReader(source))


def truthy(value: Any) -> bool:
    return str(value).strip().lower() in {"1", "true", "yes", "y"}


def episode_metrics(rows: list[dict[str, str]], duration_seconds: float) -> dict[str, Any]:
    dispatched = [row for row in rows if truthy(row.get("dispatch_started"))]
    by_id: dict[str, list[dict[str, str]]] = defaultdict(list)
    for row in dispatched:
        by_id[str(row["request_id"])].append(row)
    duplicates = sum(
        1
        for values in by_id.values()
        if len(values) != 1 or int(values[0].get("terminal_count") or 0) > 1
    )
    system_failures = {
        request_id
        for request_id, values in by_id.items()
        if values[0].get("failure_class")
        in {"uenv_error", "late_result", "no_terminal_result", "duplicate_terminal_result"}
    }
    config_errors = {
        request_id
        for request_id, values in by_id.items()
        if values[0].get("failure_class") == "test_config_error"
    }
    valid = {
        request_id
        for request_id, values in by_id.items()
        if values[0].get("failure_class") == "none" and truthy(values[0].get("result_checksum_valid"))
    }
    denominator = len(by_id)
    return {
        "dispatch_started_unique": denominator,
        "valid_unique": len(valid),
        "system_failure_unique": len(system_failures),
        "test_config_error_unique": len(config_errors),
        "system_failure_rate": len(system_failures) / denominator if denominator else 0.0,
        "allowed_system_failures": math.floor(0.001 * denominator),
        "duplicate_terminal_results": duplicates,
        "throughput_eps": len(valid) / duration_seconds if duration_seconds > 0 else 0.0,
        "failure_classes": dict(Counter(row.get("failure_class", "") for row in dispatched)),
    }


def episode_metrics_sqlite(
    path: Path,
    duration_seconds: float,
    *,
    task: str | None = None,
    dispatched_after: float | None = None,
    dispatched_before: float | None = None,
) -> dict[str, Any]:
    conditions = ["dispatch_started=1"]
    values: list[Any] = []
    if task is not None:
        conditions.append("task=?")
        values.append(task)
    if dispatched_after is not None:
        conditions.append("dispatched_at>=?")
        values.append(dispatched_after)
    if dispatched_before is not None:
        conditions.append("dispatched_at<?")
        values.append(dispatched_before)
    where = " AND ".join(conditions)
    connection = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    try:
        row = connection.execute(
            f"""SELECT
                   count(*),
                   sum(CASE WHEN failure_class='none' AND result_checksum_valid=1 THEN 1 ELSE 0 END),
                   sum(CASE WHEN failure_class IN (
                       'uenv_error','late_result','no_terminal_result','duplicate_terminal_result'
                   ) THEN 1 ELSE 0 END),
                   sum(CASE WHEN failure_class='test_config_error' THEN 1 ELSE 0 END),
                   sum(CASE WHEN terminal_count>1 THEN 1 ELSE 0 END)
                FROM episode WHERE {where}""",
            values,
        ).fetchone()
        classes = dict(connection.execute(
            f"SELECT failure_class,count(*) FROM episode WHERE {where} GROUP BY failure_class",
            values,
        ).fetchall())
    finally:
        connection.close()
    denominator, valid, failures, config_errors, duplicates = (int(value or 0) for value in row)
    return {
        "dispatch_started_unique": denominator,
        "valid_unique": valid,
        "system_failure_unique": failures,
        "test_config_error_unique": config_errors,
        "system_failure_rate": failures / denominator if denominator else 0.0,
        "allowed_system_failures": math.floor(0.001 * denominator),
        "duplicate_terminal_results": duplicates,
        "throughput_eps": valid / duration_seconds if duration_seconds > 0 else 0.0,
        "failure_classes": classes,
    }
def resource_p95(rows: list[dict[str, str]]) -> dict[str, float]:
    fields = ("rss_bytes", "open_fds", "threads")
    return {field: percentile([float(row[field]) for row in rows if row.get(field)], 0.95) for field in fields}


def resource_evidence(rows: list[dict[str, str]], duration_seconds: float, sample_seconds: float = 30.0) -> dict[str, Any]:
    required_fields = {
        "timestamp", "rss_bytes", "open_fds", "threads", "oom_events",
        "fd_exhaustions", "thread_exhaustions", "uenv_crashes", "manual_restarts",
    }
    complete = [row for row in rows if required_fields.issubset(row) and all(row.get(field) not in (None, "") for field in required_fields)]
    expected = max(1, math.floor(duration_seconds / sample_seconds))
    return {
        "complete_samples": len(complete),
        "expected_samples": expected,
        "coverage": min(1.0, len(complete) / expected),
        "valid": len(complete) >= expected,
        "oom_events": max((int(float(row["oom_events"])) for row in complete), default=-1),
        "fd_exhaustions": max((int(float(row["fd_exhaustions"])) for row in complete), default=-1),
        "thread_exhaustions": max((int(float(row["thread_exhaustions"])) for row in complete), default=-1),
        "uenv_crashes": max((int(float(row["uenv_crashes"])) for row in complete), default=-1),
        "manual_restarts": max((int(float(row["manual_restarts"])) for row in complete), default=-1),
    }


def resource_growth(reference: dict[str, float], last: dict[str, float]) -> dict[str, float]:
    return {
        key: ((last[key] / reference[key]) - 1.0) if reference.get(key) else 0.0
        for key in reference
    }


def unified_metrics_summary(run_dir: Path) -> dict[str, Any]:
    path = run_dir / "suite-metrics.json"
    if not path.is_file():
        return {
            "available": False,
            "artifact": str(path),
            "reason": "suite-metrics.json missing",
        }
    document = json.loads(path.read_text(encoding="utf-8"))
    resources = dict(document.get("resources") or {})
    resources.pop("records", None)
    return {
        "available": True,
        "artifact": str(path),
        "complete": bool(document.get("complete")),
        "by_dataset": document.get("by_dataset", {}),
        "by_parallel_mode": document.get("by_parallel_mode", {}),
        "worker_load_distribution": document.get("worker_load_distribution", {}),
        "per_worker_records": len(document.get("by_worker", [])),
        "per_worker_dataset_records": len(document.get("by_worker_dataset", [])),
        "replay": document.get("replay", {}),
        "submission_rate": document.get("submission_rate", {}),
        "resources": resources,
        "cleanup": document.get("cleanup", {}),
        "data_quality": document.get("data_quality", {}),
    }


def build_report(reference_dir: Path, stability_dir: Path, fault_dir: Path | None) -> dict[str, Any]:
    reference_manifest = json.loads((reference_dir / "manifest.json").read_text(encoding="utf-8"))
    stability_manifest = json.loads((stability_dir / "manifest.json").read_text(encoding="utf-8"))
    reference_mentor_metrics = unified_metrics_summary(reference_dir)
    stability_mentor_metrics = unified_metrics_summary(stability_dir)
    reference_duration = float(reference_manifest["duration_seconds"])
    stability_duration = float(stability_manifest["duration_seconds"])
    reference_started = float(reference_manifest["started_unix"])
    stability_started = float(stability_manifest["started_unix"])
    reference_db = reference_dir / "episode.sqlite"
    stability_db = stability_dir / "episode.sqlite"
    use_sqlite = reference_db.is_file() and stability_db.is_file()
    reference_rows = [] if use_sqlite else read_csv(reference_dir / "episode.csv")
    stability_rows = [] if use_sqlite else read_csv(stability_dir / "episode.csv")
    if not use_sqlite:
        reference_rows = [
            row for row in reference_rows
            if reference_started <= float(row.get("dispatched_at") or 0) < reference_started + reference_duration
        ]
        stability_rows = [
            row for row in stability_rows
            if stability_started <= float(row.get("dispatched_at") or 0) < stability_started + stability_duration
        ]
    stability_window_start = float(stability_manifest["started_unix"]) + stability_duration - 14400
    stability_last_rows = [
        row for row in stability_rows
        if float(row.get("dispatched_at") or 0) >= stability_window_start
    ]
    task_reports: dict[str, Any] = {}
    for task in TASK_NAMES:
        if use_sqlite:
            ref = episode_metrics_sqlite(
                reference_db,
                reference_duration,
                task=task,
                dispatched_after=reference_started,
                dispatched_before=reference_started + reference_duration,
            )
            stable = episode_metrics_sqlite(
                stability_db,
                14400,
                task=task,
                dispatched_after=stability_window_start,
                dispatched_before=stability_started + stability_duration,
            )
        else:
            ref = episode_metrics([row for row in reference_rows if row.get("task") == task], reference_duration)
            stable = episode_metrics([row for row in stability_last_rows if row.get("task") == task], 14400)
        ratio = stable["throughput_eps"] / ref["throughput_eps"] if ref["throughput_eps"] else 0.0
        task_reports[task] = {"reference": ref, "stability": stable, "throughput_retention": ratio}

    all_episode = (
        episode_metrics_sqlite(
            stability_db,
            stability_duration,
            dispatched_after=stability_started,
            dispatched_before=stability_started + stability_duration,
        )
        if use_sqlite else episode_metrics(stability_rows, stability_duration)
    )
    availability_start_second = int(stability_started)
    availability_end_second = availability_start_second + math.ceil(stability_duration)
    availability_rows = [
        row
        for row in read_csv(stability_dir / "availability.csv")
        if availability_start_second
        <= float(row.get("timestamp") or 0)
        < availability_end_second
    ]
    availability_samples = [
        {
            "timestamp": float(row["timestamp"]),
            "ok": truthy(row.get("reachable", row.get("ok"))),
        }
        for row in availability_rows
    ]
    outages = classify_availability(availability_samples)
    outage_durations = [end - start for start, end in outages]
    raw_observed_seconds = float(stability_manifest.get("observed_seconds", 0))
    observed_seconds = stability_duration if raw_observed_seconds >= stability_duration else raw_observed_seconds
    downtime = sum(outage_durations)
    availability = 1.0 - downtime / observed_seconds if observed_seconds else 0.0
    covered_seconds = {
        int(float(row["timestamp"])) for row in availability_rows if row.get("timestamp")
    }
    expected_seconds = max(1, math.floor(observed_seconds))
    observation_coverage = min(1.0, len(covered_seconds) / expected_seconds)

    reference_resource_rows = read_csv(reference_dir / "resource.csv")
    reference_resource = resource_p95(reference_resource_rows)
    stability_resource_rows = read_csv(stability_dir / "resource.csv")
    last_window_start = float(stability_manifest["started_unix"]) + stability_duration - 14400
    last_resource = resource_p95(
        [row for row in stability_resource_rows if float(row.get("timestamp") or 0) >= last_window_start]
    )
    growth = resource_growth(reference_resource, last_resource)
    cleanup = stability_manifest.get("cleanup", {})
    reference_resource_evidence = resource_evidence(reference_resource_rows, reference_duration)
    stability_resource_evidence = resource_evidence(stability_resource_rows, stability_duration)

    faults = read_csv(fault_dir / "fault.csv") if fault_dir else []
    recovered = [row for row in faults if truthy(row.get("automatic_recovery"))]
    fault_counts = Counter(row.get("fault_type", "") for row in faults)
    recovery_deadlines_ok = all(
        float(row.get("recovery_seconds") or math.inf)
        <= (300.0 if row.get("fault_type") == "node_isolation" else 120.0)
        for row in faults
    )
    fault_pass = (
        len(faults) == 15
        and all(fault_counts[name] == 5 for name in ("worker_exit", "worker_network", "node_isolation"))
        and len(recovered) == 15
        and recovery_deadlines_ok
        and all(
            row.get("first_healthy_at")
            and row.get("registered_at")
            and row.get("three_episode_successes_at")
            for row in faults
        )
        and all(int(row.get("lost_results") or 0) == 0 for row in faults)
        and all(int(row.get("duplicate_results") or 0) == 0 for row in faults)
        and all(int(row.get("checksum_mismatches") or 0) == 0 for row in faults)
        and all(int(row.get("state_regressions") or 0) == 0 for row in faults)
    )
    fingerprint_match = (
        bool(reference_manifest.get("acceptance_fingerprint", {}).get("sha256"))
        and reference_manifest.get("acceptance_fingerprint", {}).get("sha256")
        == stability_manifest.get("acceptance_fingerprint", {}).get("sha256")
    )
    no_config_errors = (
        all_episode["test_config_error_unique"] == 0
        and all(
            item["reference"]["test_config_error_unique"] == 0
            and item["stability"]["test_config_error_unique"] == 0
            for item in task_reports.values()
        )
    )
    resource_baseline_valid = all(value > 0 for value in reference_resource.values())

    metric_pass = {
        "continuous_availability": (
            observed_seconds >= 259200
            and observation_coverage >= 1.0
            and stability_resource_evidence["uenv_crashes"] == 0
            and stability_resource_evidence["manual_restarts"] == 0
            and downtime <= 259
            and max(outage_durations, default=0.0) <= 120
            and availability >= 0.999
        ),
        "resource_stability": (
            fingerprint_match
            and reference_resource_evidence["valid"]
            and stability_resource_evidence["valid"]
            and resource_baseline_valid
            and all(float(value) <= 0.10 for value in growth.values())
            and stability_resource_evidence["oom_events"] == 0
            and stability_resource_evidence["fd_exhaustions"] == 0
            and stability_resource_evidence["thread_exhaustions"] == 0
            and all(int(cleanup.get(key, 0)) == 0 for key in ("remaining_workers", "remaining_containers", "remaining_processes"))
        ),
        "system_failure_rate": (
            no_config_errors
            and all_episode["system_failure_unique"] <= all_episode["allowed_system_failures"]
            and all_episode["system_failure_rate"] <= 0.001
        ),
        "throughput_retention": (
            fingerprint_match
            and all(item["throughput_retention"] >= 0.90 for item in task_reports.values())
        ),
        "fault_recovery_consistency": fault_pass,
    }
    development_only = bool(reference_manifest.get("development_only") or stability_manifest.get("development_only"))
    evidence_complete = (
        fingerprint_match
        and no_config_errors
        and reference_resource_evidence["valid"]
        and stability_resource_evidence["valid"]
        and resource_baseline_valid
        and bool(reference_mentor_metrics.get("complete"))
        and bool(stability_mentor_metrics.get("complete"))
    )
    return {
        "schema_version": 1,
        "development_only": development_only,
        "formal_acceptance_eligible": not development_only and evidence_complete,
        "overall_pass": not development_only and evidence_complete and all(metric_pass.values()),
        "acceptance_fingerprint_match": fingerprint_match,
        "evidence_complete": evidence_complete,
        "metrics_pass": metric_pass,
        "availability": {
            "observed_seconds": observed_seconds,
            "downtime_seconds": downtime,
            "availability": availability,
            "observation_coverage": observation_coverage,
            "outage_count": len(outages),
            "longest_outage_seconds": max(outage_durations, default=0.0),
        },
        "resources": {
            "reference_p95": reference_resource,
            "last_4h_p95": last_resource,
            "growth": growth,
            "cleanup": cleanup,
            "reference_evidence": reference_resource_evidence,
            "stability_evidence": stability_resource_evidence,
        },
        "mentor_metrics": {
            "reference": reference_mentor_metrics,
            "stability": stability_mentor_metrics,
        },
        "episodes": all_episode,
        "tasks": task_reports,
        "faults": {"attempts": len(faults), "automatic_recoveries": len(recovered)},
    }


def markdown(report: dict[str, Any]) -> str:
    lines = [
        "# UEnv 百卡稳定性验收报告",
        "",
        f"- 正式验收资格：{'是' if report['formal_acceptance_eligible'] else '否（development_only）'}",
        f"- 总体结论：{'通过' if report['overall_pass'] else '不通过'}",
        "",
        "| 指标 | 结论 |",
        "| --- | --- |",
    ]
    labels = {
        "continuous_availability": "百卡72小时连续可用性",
        "resource_stability": "资源使用稳定性",
        "system_failure_rate": "系统原因Episode失败率",
        "throughput_retention": "有效处理吞吐保持率",
        "fault_recovery_consistency": "故障恢复与结果一致性",
    }
    for key, passed in report["metrics_pass"].items():
        lines.append(f"| {labels[key]} | {'通过' if passed else '不通过'} |")
    lines.extend(["", "## 分任务吞吐保持率", "", "| 数据集 | 保持率 |", "| --- | ---: |"])
    for task, item in report["tasks"].items():
        lines.append(f"| {task} | {item['throughput_retention']:.2%} |")
    return "\n".join(lines) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--reference-dir", type=Path, required=True)
    parser.add_argument("--stability-dir", type=Path, required=True)
    parser.add_argument("--fault-dir", type=Path)
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()
    report = build_report(args.reference_dir, args.stability_dir, args.fault_dir)
    args.output_dir.mkdir(parents=True, exist_ok=True)
    (args.output_dir / "report.json").write_text(json.dumps(report, indent=2, sort_keys=True), encoding="utf-8")
    (args.output_dir / "report.md").write_text(markdown(report), encoding="utf-8")
    return 0 if report["overall_pass"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
