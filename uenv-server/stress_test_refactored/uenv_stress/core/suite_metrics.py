"""规模压测与稳定性验收的统一指标聚合。

这个文件把底层 EpisodeObservation、Server 日志、replay 记录、worker 资源采样和清理探测结果转换成评审可读的聚合指标。它负责计算成功率、吞吐、时延分布、数据集覆盖、worker 覆盖、资源峰值、清理状态和稳定性阶段指标。

实现逻辑是：先用 percentile、distribution、metric_block 等函数计算通用统计；build_scale_suite_metrics 从规模压测 summary 中提取各场景 Episode、replay、worker、资源和清理记录；parse_worker_load_log 解析 worker 负载日志；build_stability_suite_metrics 从 SQLite ledger、CSV 和资源采样中生成正式验收指标。"""

from __future__ import annotations

import csv
import json
import math
import re
import sqlite3
import statistics
from collections import defaultdict
from pathlib import Path
from typing import Any, Iterable


SUITE_METRICS_SCHEMA_VERSION = 2
DATASETS = ("dscodebench", "swebench_pro", "olymmath", "scitab", "pubmedqa")
PARALLEL_MODES = ("sync", "one_step_off_policy", "fully_async")


def canonical_dataset(value: Any) -> str:
    normalized = str(value or "").strip().lower().replace("-", "_").replace(" ", "_")
    aliases = {
        "dscodebench": "dscodebench",
        "ds_code_bench": "dscodebench",
        "swe_bench_pro": "swebench_pro",
        "swebench_pro": "swebench_pro",
        "olymmath": "olymmath",
        "scitab": "scitab",
        "pubmedqa": "pubmedqa",
    }
    return aliases.get(normalized, normalized)


def truthy(value: Any) -> bool:
    return str(value).strip().lower() in {"1", "true", "yes", "y"}


def percentile(values: Iterable[float], q: float) -> float:
    ordered = sorted(float(value) for value in values)
    if not ordered:
        return 0.0
    index = min(len(ordered) - 1, max(0, math.ceil(len(ordered) * q) - 1))
    return ordered[index]


def distribution(values: Iterable[float], *, population_count: int | None = None) -> dict[str, Any]:
    samples = [float(value) for value in values]
    if population_count is not None and population_count > len(samples):
        samples.extend([0.0] * (population_count - len(samples)))
    if not samples:
        return {
            "available": False,
            "count": 0,
            "minimum": 0.0,
            "mean": 0.0,
            "p95": 0.0,
            "maximum": 0.0,
            "standard_deviation": 0.0,
            "coefficient_of_variation": 0.0,
            "total": 0.0,
            "reason": "no observations",
        }
    mean = statistics.fmean(samples)
    standard_deviation = statistics.pstdev(samples)
    return {
        "available": True,
        "count": len(samples),
        "minimum": min(samples),
        "mean": mean,
        "p95": percentile(samples, 0.95),
        "maximum": max(samples),
        "standard_deviation": standard_deviation,
        "coefficient_of_variation": standard_deviation / mean if mean else 0.0,
        "total": sum(samples),
    }


def _rate_metrics(
    dispatched: int,
    duration_seconds: float,
    planned_rate_eps: float | None,
    *,
    submission_kind: str,
) -> dict[str, Any]:
    actual = dispatched / duration_seconds if duration_seconds > 0 else 0.0
    available = planned_rate_eps is not None and duration_seconds > 0
    absolute = actual - float(planned_rate_eps or 0.0) if available else 0.0
    return {
        "submission_kind": submission_kind,
        "measurement_seconds": float(duration_seconds),
        "planned_rate_eps": float(planned_rate_eps) if planned_rate_eps is not None else None,
        "actual_rate_eps": actual,
        "absolute_deviation_eps": absolute if available else None,
        "relative_deviation": (
            absolute / float(planned_rate_eps)
            if available and planned_rate_eps
            else None
        ),
        "available": available,
        "reason": "" if available else (
            "pressure plan is backlog/episode-count based, not a configured EPS schedule"
            if submission_kind == "backlog"
            else "planned submission rate is unavailable"
        ),
    }


def _throughput_metrics(
    *,
    duration_seconds: float,
    submitted: int | None,
    completed: int | None,
    successful: int | None,
    source: str,
) -> dict[str, Any]:
    def per_second(value: int | None) -> float | None:
        if value is None or duration_seconds <= 0:
            return None
        return value / duration_seconds

    unavailable = [
        name
        for name, value in (
            ("submission_eps", submitted),
            ("completion_eps", completed),
            ("successful_eps", successful),
        )
        if value is None
    ]
    return {
        "measurement_seconds": float(duration_seconds),
        "submission_eps": per_second(submitted),
        "completion_eps": per_second(completed),
        "successful_eps": per_second(successful),
        "source": source,
        "complete": not unavailable and duration_seconds > 0,
        "reason": (
            ""
            if not unavailable and duration_seconds > 0
            else (
                "measurement duration is unavailable"
                if duration_seconds <= 0
                else f"unavailable counters: {','.join(unavailable)}"
            )
        ),
    }


def metric_block(
    rows: list[dict[str, Any]],
    *,
    duration_seconds: float,
    planned_rate_eps: float | None,
    submission_kind: str,
) -> dict[str, Any]:
    dispatched_rows = [row for row in rows if truthy(row.get("dispatch_started"))]
    terminal_rows = [
        row for row in rows
        if float(row.get("terminal_at") or 0.0) > 0
        or str(row.get("status", "")) not in {"", "planned", "dispatched"}
    ]
    succeeded = [row for row in terminal_rows if row.get("failure_class") == "none"]
    failed = [
        row for row in terminal_rows
        if str(row.get("failure_class", "")) not in {"", "none", "pending"}
    ]
    latencies = [
        float(row.get("end_to_end_ms") or 0.0)
        for row in terminal_rows
        if float(row.get("end_to_end_ms") or 0.0) > 0
    ]
    rewards = [float(row.get("reward") or 0.0) for row in terminal_rows]
    steps = [int(row.get("actual_steps") or 0) for row in terminal_rows]
    return {
        "planned_episodes": len(rows),
        "dispatched_episodes": len(dispatched_rows),
        "terminal_episodes": len(terminal_rows),
        "successful_episodes": len(succeeded),
        "failed_episodes": len(failed),
        "success_rate": len(succeeded) / len(dispatched_rows) if dispatched_rows else 0.0,
        "failure_rate": len(failed) / len(dispatched_rows) if dispatched_rows else 0.0,
        "throughput_eps": len(succeeded) / duration_seconds if duration_seconds > 0 else 0.0,
        "throughput": _throughput_metrics(
            duration_seconds=duration_seconds,
            submitted=len(dispatched_rows),
            completed=len(terminal_rows),
            successful=len(succeeded),
            source="episode_observation",
        ),
        "average_reward": statistics.fmean(rewards) if rewards else 0.0,
        "actual_steps": {
            "total": sum(steps),
            "mean": statistics.fmean(steps) if steps else 0.0,
            "maximum": max(steps, default=0),
        },
        "end_to_end_latency_ms": distribution(latencies),
        "failure_classes": dict(sorted(_counts(row.get("failure_class", "") for row in terminal_rows).items())),
        "submission_rate": _rate_metrics(
            len(dispatched_rows),
            duration_seconds,
            planned_rate_eps,
            submission_kind=submission_kind,
        ),
    }


def _counts(values: Iterable[Any]) -> dict[str, int]:
    result: dict[str, int] = {}
    for value in values:
        key = str(value)
        result[key] = result.get(key, 0) + 1
    return result


def _result_documents(value: Any) -> Iterable[dict[str, Any]]:
    if isinstance(value, dict):
        if isinstance(value.get("episode_observations"), dict):
            yield value
            return
        for child in value.values():
            yield from _result_documents(child)
    elif isinstance(value, list):
        for child in value:
            yield from _result_documents(child)


def _named_records(value: Any, names: set[str]) -> Iterable[dict[str, Any]]:
    if isinstance(value, dict):
        for key, child in value.items():
            if key in names and isinstance(child, dict):
                yield {"kind": key, "value": child}
            yield from _named_records(child, names)
    elif isinstance(value, list):
        for child in value:
            yield from _named_records(child, names)


def _load_jsonl(path: Path) -> list[dict[str, Any]]:
    if not path.is_file():
        return []
    return [
        json.loads(line)
        for line in path.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]


def _document_dataset(document: dict[str, Any], rows: list[dict[str, Any]]) -> str:
    dataset = document.get("dataset")
    if isinstance(dataset, dict):
        value = dataset.get("name")
    else:
        value = dataset
    if not value and rows:
        value = rows[0].get("dataset")
    environment = str(document.get("environment", ""))
    if not value and environment.startswith("math:"):
        value = environment.split(":", 1)[1]
    return canonical_dataset(value)


def _document_mode(document: dict[str, Any], rows: list[dict[str, Any]]) -> str:
    value = (
        document.get("parallel_mode")
        or document.get("mode")
        or (document.get("scale") or {}).get("parallel_mode")
    )
    if not value and rows:
        value = rows[0].get("parallel_mode")
    return str(value or "")


def _replay_record(document: dict[str, Any], dataset: str, mode: str) -> dict[str, Any] | None:
    replay = document.get("trace_replay")
    if not isinstance(replay, dict):
        return None
    hits = int(replay.get("hits", replay.get("trace_replay_hits", 0)) or 0)
    misses = int(replay.get("misses", replay.get("trace_replay_misses", 0)) or 0)
    calls = int(replay.get("calls", hits + misses) or 0)
    return {
        "dataset": dataset,
        "parallel_mode": mode,
        "calls": calls,
        "hits": hits,
        "misses": misses,
        "assigned_episodes": int(replay.get("assigned_episodes", 0) or 0),
        "sampling_strategy": str(
            replay.get("sampling_strategy")
            or replay.get("selection_strategy")
            or ""
        ),
        "raw": replay,
    }


def replay_section(records: list[dict[str, Any]]) -> dict[str, Any]:
    def summarize(values: list[dict[str, Any]]) -> dict[str, Any]:
        hits = sum(int(value.get("hits", 0)) for value in values)
        misses = sum(int(value.get("misses", 0)) for value in values)
        calls = sum(int(value.get("calls", 0)) for value in values)
        denominator = hits + misses
        return {
            "available": bool(values) and denominator > 0,
            "calls": calls,
            "hits": hits,
            "misses": misses,
            "hit_rate": hits / denominator if denominator else 0.0,
            "assigned_episodes": sum(int(value.get("assigned_episodes", 0)) for value in values),
            "sampling_strategies": sorted({
                str(value.get("sampling_strategy", "")) for value in values
                if value.get("sampling_strategy")
            }),
            "reason": "" if values and denominator else "no replay hit/miss counters",
        }

    by_dataset = {
        dataset: summarize([value for value in records if value["dataset"] == dataset])
        for dataset in DATASETS
    }
    load_profiles = [
        {
            "dataset": value["dataset"],
            "parallel_mode": value["parallel_mode"],
            "source_model_usage": value["raw"].get("source_model_usage", {}),
            "source_family_usage": value["raw"].get("source_family_usage", {}),
            "source_model_replay_stats": value["raw"].get(
                "source_model_replay_stats", {}
            ),
            "latency_source_usage": value["raw"].get(
                "latency_source_usage", {}
            ),
            "wait_seconds_p50": value["raw"].get("wait_seconds_p50", 0.0),
            "wait_seconds_p95": value["raw"].get("wait_seconds_p95", 0.0),
            "completion_tokens_p50": value["raw"].get(
                "completion_tokens_p50", 0.0
            ),
            "completion_tokens_p95": value["raw"].get(
                "completion_tokens_p95", 0.0
            ),
        }
        for value in records
    ]
    return {
        "overall": summarize(records),
        "by_dataset": by_dataset,
        "load_profiles": load_profiles,
        "records": records,
    }


def _worker_sections(
    coverage_records: list[dict[str, Any]],
) -> tuple[dict[str, Any], list[dict[str, Any]], list[dict[str, Any]]]:
    workers: dict[tuple[str, str], dict[str, Any]] = {}
    worker_datasets: dict[tuple[str, str, str], dict[str, Any]] = {}
    expected_by_run: dict[str, int] = {}
    for coverage in coverage_records:
        run_id = str(coverage.get("run_id") or "")
        dataset = canonical_dataset(coverage.get("dataset"))
        mode = str(coverage.get("parallel_mode") or "")
        expected_by_run[run_id] = max(
            expected_by_run.get(run_id, 0),
            int(coverage.get("expected_workers", 0) or 0),
        )
        timeline = coverage.get("load_timeline") or {}
        for row in timeline.get("per_worker", []):
            worker_id = str(row.get("worker_id") or "")
            worker_key = (run_id, worker_id)
            target = workers.setdefault(worker_key, {
                "run_id": run_id,
                "worker_id": worker_id,
                "started_episodes": 0,
                "completed_episodes": 0,
                "datasets": set(),
                "parallel_modes": set(),
                "first_completion_timestamp": "",
                "last_completion_timestamp": "",
                "measurement_seconds": 0.0,
                "source": "server_log",
            })
            target["started_episodes"] += int(row.get("started_episodes_observed", 0) or 0)
            target["completed_episodes"] += int(row.get("completed_episodes", 0) or 0)
            target["measurement_seconds"] += float(coverage.get("duration_seconds", 0.0) or 0.0)
            if dataset:
                target["datasets"].add(dataset)
            if mode:
                target["parallel_modes"].add(mode)
            first = str(row.get("first_completion_timestamp") or "")
            last = str(row.get("last_completion_timestamp") or "")
            if first and (
                not target["first_completion_timestamp"]
                or first < target["first_completion_timestamp"]
            ):
                target["first_completion_timestamp"] = first
            if last and last > target["last_completion_timestamp"]:
                target["last_completion_timestamp"] = last
            dataset_key = (run_id, worker_id, dataset)
            cross = worker_datasets.setdefault(dataset_key, {
                "run_id": run_id,
                "worker_id": worker_id,
                "dataset": dataset,
                "started_episodes": 0,
                "completed_episodes": 0,
                "parallel_modes": set(),
                "measurement_seconds": 0.0,
                "source": "server_log",
            })
            cross["started_episodes"] += int(row.get("started_episodes_observed", 0) or 0)
            cross["completed_episodes"] += int(row.get("completed_episodes", 0) or 0)
            cross["measurement_seconds"] += float(coverage.get("duration_seconds", 0.0) or 0.0)
            if mode:
                cross["parallel_modes"].add(mode)
    total_completed = sum(row["completed_episodes"] for row in workers.values())
    per_worker = []
    for value in sorted(workers.values(), key=lambda row: (row["run_id"], row["worker_id"])):
        value = dict(value)
        value["datasets"] = sorted(value["datasets"])
        value["parallel_modes"] = sorted(value["parallel_modes"])
        value["completion_share"] = (
            value["completed_episodes"] / total_completed if total_completed else 0.0
        )
        value["throughput"] = _throughput_metrics(
            duration_seconds=float(value.pop("measurement_seconds")),
            submitted=None,
            completed=int(value["completed_episodes"]),
            successful=None,
            source="server_log",
        )
        per_worker.append(value)
    expected_population = sum(expected_by_run.values())
    load_distribution = distribution(
        [row["completed_episodes"] for row in per_worker],
        population_count=expected_population,
    )
    load_distribution.update({
        "metric": "completed_episodes",
        "population": "configured Worker instances across run IDs",
        "configured_workers": expected_population,
        "observed_workers": len(per_worker),
        "source": "server_log",
    })
    per_worker_dataset = []
    dataset_totals = {
        dataset: sum(
            int(value["completed_episodes"])
            for value in worker_datasets.values()
            if value["dataset"] == dataset
        )
        for dataset in DATASETS
    }
    for value in sorted(
        worker_datasets.values(),
        key=lambda row: (row["run_id"], row["worker_id"], row["dataset"]),
    ):
        value = dict(value)
        value["parallel_modes"] = sorted(value["parallel_modes"])
        value["completion_share_within_dataset"] = (
            value["completed_episodes"] / dataset_totals.get(value["dataset"], 0)
            if dataset_totals.get(value["dataset"], 0)
            else 0.0
        )
        value["throughput"] = _throughput_metrics(
            duration_seconds=float(value.pop("measurement_seconds")),
            submitted=None,
            completed=int(value["completed_episodes"]),
            successful=None,
            source="server_log",
        )
        per_worker_dataset.append(value)
    return load_distribution, per_worker, per_worker_dataset


def _sample_count(value: Any) -> int:
    if not isinstance(value, dict):
        return 0
    if isinstance(value.get("value"), dict):
        return _sample_count(value["value"])
    for key in ("sample_count", "samples", "complete_samples"):
        if key in value and isinstance(value[key], (int, float)):
            return int(value[key])
    if isinstance(value.get("per_node"), dict):
        return sum(_sample_count(child) for child in value["per_node"].values())
    return 0


def resource_section(records: list[Any], *, expected_samples: int | None = None) -> dict[str, Any]:
    samples = sum(_sample_count(value) for value in records)
    return {
        "available": bool(records) and samples > 0,
        "source_count": len(records),
        "sample_count": samples,
        "expected_samples": expected_samples,
        "coverage": (
            min(1.0, samples / expected_samples)
            if expected_samples and expected_samples > 0
            else None
        ),
        "records": records,
        "reason": "" if records and samples > 0 else "no resource samples",
    }


def cleanup_section(records: list[dict[str, Any]]) -> dict[str, Any]:
    attempted = bool(records)
    passed = attempted and all(bool(record.get("passed")) for record in records)
    return {
        "available": attempted,
        "attempted": attempted,
        "passed": passed,
        "records": records,
        "reason": "" if attempted else "no cleanup result",
    }


def build_scale_suite_metrics(document: dict[str, Any]) -> dict[str, Any]:
    rows: list[dict[str, Any]] = []
    windows: list[dict[str, Any]] = []
    coverage_records: list[dict[str, Any]] = []
    replay_records: list[dict[str, Any]] = []
    resource_records: list[Any] = []
    observation_sources: list[dict[str, Any]] = []
    seen_paths: set[str] = set()
    for result in _result_documents(document.get("scenarios", [])):
        metadata = result["episode_observations"]
        path_text = str(metadata.get("local_artifact") or "")
        if not path_text or path_text in seen_paths:
            continue
        seen_paths.add(path_text)
        source_rows = _load_jsonl(Path(path_text))
        dataset = _document_dataset(result, source_rows)
        mode = _document_mode(result, source_rows)
        for row in source_rows:
            row = dict(row)
            row["dataset"] = canonical_dataset(row.get("dataset") or dataset)
            rows.append(row)
        duration = float(result.get("elapsed_seconds", 0.0) or 0.0)
        windows.append({
            "dataset": dataset,
            "parallel_mode": mode,
            "duration_seconds": duration,
            "submitted": int(metadata.get("submitted_count", len(source_rows)) or 0),
        })
        observation_sources.append({
            "path": path_text,
            "dataset": dataset,
            "parallel_mode": mode,
            "row_count": len(source_rows),
            "declared_row_count": int(metadata.get("row_count", 0) or 0),
            "complete": bool(metadata.get("complete")) and len(source_rows) == int(metadata.get("row_count", 0) or 0),
        })
        coverage = result.get("worker_dispatch_coverage")
        if isinstance(coverage, dict):
            coverage_records.append({
                **coverage,
                "run_id": str(result.get("run_id") or ""),
                "dataset": dataset,
                "parallel_mode": mode,
                "duration_seconds": duration,
            })
        replay = _replay_record(result, dataset, mode)
        if replay:
            replay_records.append(replay)
    resource_records.extend(_named_records(
        document.get("scenarios", []),
        {"resource_observations", "host_resource_metrics", "fleet_resource_metrics"},
    ))

    def duration_for(*, dataset: str | None = None, mode: str | None = None) -> float:
        return sum(
            float(window["duration_seconds"])
            for window in windows
            if (dataset is None or window["dataset"] == dataset)
            and (mode is None or window["parallel_mode"] == mode)
        )

    by_dataset = {
        dataset: metric_block(
            [row for row in rows if canonical_dataset(row.get("dataset")) == dataset],
            duration_seconds=duration_for(dataset=dataset),
            planned_rate_eps=None,
            submission_kind="backlog",
        )
        for dataset in DATASETS
    }
    by_mode = {
        mode: metric_block(
            [row for row in rows if row.get("parallel_mode") == mode],
            duration_seconds=duration_for(mode=mode),
            planned_rate_eps=None,
            submission_kind="backlog",
        )
        for mode in PARALLEL_MODES
    }
    by_dataset_mode = {
        f"{dataset}|{mode}": metric_block(
            [
                row for row in rows
                if canonical_dataset(row.get("dataset")) == dataset
                and row.get("parallel_mode") == mode
            ],
            duration_seconds=duration_for(dataset=dataset, mode=mode),
            planned_rate_eps=None,
            submission_kind="backlog",
        )
        for dataset in DATASETS
        for mode in PARALLEL_MODES
    }
    load_distribution, per_worker, per_worker_dataset = _worker_sections(coverage_records)
    explicit_cleanup = [
        value["value"]
        for value in _named_records(document.get("scenarios", []), {"cleanup"})
    ]
    cleanup_records = explicit_cleanup or [
        {
            "scenario": str(scenario.get("name") or ""),
            "attempted": bool(document.get("executed")),
            "passed": scenario.get("status") == "passed",
            "returncode": int(scenario.get("returncode", -1)),
            "error": str(scenario.get("error") or ""),
            "protected_process_unchanged": bool(
                (document.get("infrastructure") or {}).get("protected_process_unchanged")
            ),
        }
        for scenario in document.get("scenarios", [])
    ]
    overall = metric_block(
        rows,
        duration_seconds=duration_for(),
        planned_rate_eps=None,
        submission_kind="backlog",
    )
    observation_complete = bool(observation_sources) and all(
        source["complete"] for source in observation_sources
    )
    worker_section = {
        "available": bool(coverage_records),
        "source": "isolated Server logs",
        "coverage_records": len(coverage_records),
        "reason": "" if coverage_records else "no worker dispatch coverage records",
    }
    replay = replay_section(replay_records)
    resources = resource_section(resource_records)
    cleanup = cleanup_section(cleanup_records)
    complete = bool(
        observation_complete
        and worker_section["available"]
        and replay["overall"]["available"]
        and resources["available"]
        and cleanup["passed"]
    )
    return {
        "schema_version": SUITE_METRICS_SCHEMA_VERSION,
        "metrics_type": "suite_metrics",
        "suite": "scale",
        "run_id": str(document.get("suite_id") or ""),
        "overall": overall,
        "by_dataset": by_dataset,
        "by_parallel_mode": by_mode,
        "by_dataset_parallel_mode": by_dataset_mode,
        "by_worker": per_worker,
        "by_worker_dataset": per_worker_dataset,
        "worker_load_distribution": load_distribution,
        "replay": replay,
        "submission_rate": overall["submission_rate"],
        "resources": resources,
        "cleanup": cleanup,
        "complete": complete,
        "data_quality": {
            "episode_observations": {
                "available": bool(observation_sources),
                "complete": observation_complete,
                "row_count": len(rows),
                "sources": observation_sources,
            },
            "worker_attribution": worker_section,
        },
    }


def parse_worker_load_log(
    path: Path,
    *,
    run_id: str,
    expected_worker_ids: Iterable[str],
    datasets: Iterable[str] = DATASETS,
) -> dict[str, Any]:
    worker_ids = [str(value) for value in expected_worker_ids]
    by_worker = {worker_id: 0 for worker_id in worker_ids}
    by_worker_dataset = {
        (worker_id, dataset): 0
        for worker_id in worker_ids
        for dataset in datasets
    }
    if not path.is_file():
        return {
            "available": False,
            "source": str(path),
            "reason": "server log does not exist",
            "by_worker": by_worker,
            "by_worker_dataset": by_worker_dataset,
            "matched_lines": 0,
        }
    matched = 0
    worker_pattern = re.compile(r"worker_id=([^\s,]+)")
    dataset_names = tuple(str(value) for value in datasets)
    with path.open("r", encoding="utf-8", errors="replace") as source:
        for line in source:
            if "episode_completed" not in line or run_id not in line:
                continue
            worker_match = worker_pattern.search(line)
            if not worker_match:
                continue
            worker_id = worker_match.group(1).strip('",')
            if worker_id not in by_worker:
                continue
            dataset = next(
                (name for name in dataset_names if f"-{name}-" in line),
                "",
            )
            if not dataset:
                continue
            by_worker[worker_id] += 1
            by_worker_dataset[(worker_id, dataset)] += 1
            matched += 1
    return {
        "available": matched > 0,
        "source": str(path),
        "reason": "" if matched else "no run-owned episode_completed lines matched",
        "by_worker": by_worker,
        "by_worker_dataset": by_worker_dataset,
        "matched_lines": matched,
    }


def _sqlite_metric_block(
    connection: sqlite3.Connection,
    where: str,
    parameters: list[Any],
    *,
    duration_seconds: float,
    planned_rate_eps: float | None,
) -> dict[str, Any]:
    row = connection.execute(
        f"""SELECT
              count(*),
              sum(CASE WHEN dispatch_started=1 THEN 1 ELSE 0 END),
              sum(CASE WHEN terminal_at>0 OR status NOT IN ('planned','dispatched') THEN 1 ELSE 0 END),
              sum(CASE WHEN failure_class='none' THEN 1 ELSE 0 END),
              sum(CASE WHEN failure_class NOT IN ('','none','pending') THEN 1 ELSE 0 END),
              avg(reward),
              sum(actual_steps),
              max(actual_steps),
              avg(end_to_end_ms),
              min(end_to_end_ms),
              max(end_to_end_ms)
            FROM episode WHERE {where}""",
        parameters,
    ).fetchone()
    (
        planned, dispatched, terminal, succeeded, failed, reward, steps,
        maximum_steps, latency_mean, latency_min, latency_max,
    ) = row
    classes = dict(connection.execute(
        f"SELECT failure_class,count(*) FROM episode WHERE {where} GROUP BY failure_class",
        parameters,
    ).fetchall())
    dispatched = int(dispatched or 0)
    terminal = int(terminal or 0)
    succeeded = int(succeeded or 0)
    failed = int(failed or 0)
    return {
        "planned_episodes": int(planned or 0),
        "dispatched_episodes": dispatched,
        "terminal_episodes": terminal,
        "successful_episodes": succeeded,
        "failed_episodes": failed,
        "success_rate": succeeded / dispatched if dispatched else 0.0,
        "failure_rate": failed / dispatched if dispatched else 0.0,
        "throughput_eps": succeeded / duration_seconds if duration_seconds > 0 else 0.0,
        "throughput": _throughput_metrics(
            duration_seconds=duration_seconds,
            submitted=dispatched,
            completed=terminal,
            successful=succeeded,
            source="episode_observation_sqlite",
        ),
        "average_reward": float(reward or 0.0),
        "actual_steps": {
            "total": int(steps or 0),
            "mean": (int(steps or 0) / terminal if terminal else 0.0),
            "maximum": int(maximum_steps or 0),
        },
        "end_to_end_latency_ms": {
            "available": terminal > 0,
            "count": terminal,
            "minimum": float(latency_min or 0.0),
            "mean": float(latency_mean or 0.0),
            "p95": None,
            "maximum": float(latency_max or 0.0),
            "standard_deviation": None,
            "coefficient_of_variation": None,
            "total": None,
            "reason": "p95 remains available from EpisodeObservation; omitted here to avoid sorting a 72h ledger",
        },
        "failure_classes": classes,
        "submission_rate": _rate_metrics(
            dispatched,
            duration_seconds,
            planned_rate_eps,
            submission_kind="configured_rate",
        ),
    }


def build_stability_suite_metrics(
    *,
    ledger_path: Path,
    run_id: str,
    phase: str,
    duration_seconds: float,
    parallel_mode: str,
    planned_rates: dict[str, float],
    expected_worker_ids: list[str],
    worker_load: dict[str, Any],
    replay_health: dict[str, Any],
    resource_csv: Path,
    resource_sample_seconds: float,
    cleanup: dict[str, Any],
) -> dict[str, Any]:
    connection = sqlite3.connect(f"file:{ledger_path}?mode=ro", uri=True)
    try:
        overall = _sqlite_metric_block(
            connection,
            "1=1",
            [],
            duration_seconds=duration_seconds,
            planned_rate_eps=sum(planned_rates.values()),
        )
        by_dataset = {
            dataset: _sqlite_metric_block(
                connection,
                "task=?",
                [dataset],
                duration_seconds=duration_seconds,
                planned_rate_eps=planned_rates.get(dataset),
            )
            for dataset in DATASETS
        }
        by_mode = {
            mode: (
                json.loads(json.dumps(overall))
                if mode == parallel_mode
                else metric_block(
                    [],
                    duration_seconds=duration_seconds,
                    planned_rate_eps=0.0,
                    submission_kind="configured_rate",
                )
            )
            for mode in PARALLEL_MODES
        }
    finally:
        connection.close()

    per_worker = [
        {
            "run_id": run_id,
            "worker_id": worker_id,
            "started_episodes": 0,
            "completed_episodes": int(worker_load.get("by_worker", {}).get(worker_id, 0)),
            "completion_share": (
                int(worker_load.get("by_worker", {}).get(worker_id, 0))
                / max(1, int(worker_load.get("matched_lines", 0) or 0))
            ),
            "datasets": [
                dataset for dataset in DATASETS
                if int(worker_load.get("by_worker_dataset", {}).get((worker_id, dataset), 0)) > 0
            ],
            "parallel_modes": [parallel_mode],
            "first_completion_timestamp": "",
            "last_completion_timestamp": "",
            "throughput": _throughput_metrics(
                duration_seconds=duration_seconds,
                submitted=None,
                completed=int(worker_load.get("by_worker", {}).get(worker_id, 0)),
                successful=None,
                source="server_log",
            ),
            "source": "server_log",
        }
        for worker_id in expected_worker_ids
    ]
    per_worker_dataset = [
        {
            "run_id": run_id,
            "worker_id": worker_id,
            "dataset": dataset,
            "started_episodes": 0,
            "completed_episodes": int(
                worker_load.get("by_worker_dataset", {}).get((worker_id, dataset), 0)
            ),
            "completion_share_within_dataset": (
                int(worker_load.get("by_worker_dataset", {}).get((worker_id, dataset), 0))
                / max(
                    1,
                    sum(
                        int(worker_load.get("by_worker_dataset", {}).get((candidate, dataset), 0))
                        for candidate in expected_worker_ids
                    ),
                )
            ),
            "parallel_modes": [parallel_mode],
            "throughput": _throughput_metrics(
                duration_seconds=duration_seconds,
                submitted=None,
                completed=int(
                    worker_load.get("by_worker_dataset", {}).get((worker_id, dataset), 0)
                ),
                successful=None,
                source="server_log",
            ),
            "source": "server_log",
        }
        for worker_id in expected_worker_ids
        for dataset in DATASETS
    ]
    load_distribution = distribution(
        [row["completed_episodes"] for row in per_worker],
        population_count=len(expected_worker_ids),
    )
    load_distribution.update({
        "metric": "completed_episodes",
        "population": "configured Workers",
        "configured_workers": len(expected_worker_ids),
        "observed_workers": sum(1 for row in per_worker if row["completed_episodes"] > 0),
        "source": "server_log",
    })

    replay_records = []
    replay_root = replay_health.get("replay") if isinstance(replay_health, dict) else {}
    if isinstance(replay_root, dict):
        for dataset, value in replay_root.items():
            if not isinstance(value, dict):
                continue
            replay_records.append({
                "dataset": canonical_dataset(dataset),
                "parallel_mode": parallel_mode,
                "calls": int(value.get("calls", 0) or 0),
                "hits": int(value.get("hits", 0) or 0),
                "misses": int(value.get("misses", 0) or 0),
                "assigned_episodes": int(value.get("assigned_episodes", 0) or 0),
                "sampling_strategy": str(
                    replay_health.get("selection_strategy", "")
                ),
                "raw": value,
            })

    resource_rows = []
    if resource_csv.is_file():
        with resource_csv.open("r", encoding="utf-8", newline="") as source:
            resource_rows = list(csv.DictReader(source))
    expected_samples = max(1, math.floor(duration_seconds / resource_sample_seconds))
    resource_summary = {
        "available": bool(resource_rows),
        "sample_count": len(resource_rows),
        "expected_samples": expected_samples,
        "coverage": min(1.0, len(resource_rows) / expected_samples),
        "p95": {
            field: percentile(
                [float(row[field]) for row in resource_rows if row.get(field)],
                0.95,
            )
            for field in ("rss_bytes", "open_fds", "threads", "running_containers")
        },
        "events": {
            field: max(
                (int(float(row[field])) for row in resource_rows if row.get(field)),
                default=-1,
            )
            for field in (
                "worker_exits", "oom_events", "fd_exhaustions",
                "thread_exhaustions", "uenv_crashes", "manual_restarts",
            )
        },
        "artifact": str(resource_csv),
        "reason": "" if resource_rows else "resource.csv is missing or empty",
    }
    cleanup_record = {
        "attempted": True,
        "passed": all(
            int(cleanup.get(key, -1)) == 0
            for key in ("remaining_workers", "remaining_containers", "remaining_processes")
        ) and not cleanup.get("error"),
        **cleanup,
    }
    replay = replay_section(replay_records)
    cleanup_metrics = cleanup_section([cleanup_record])
    worker_quality = {
        "available": bool(worker_load.get("available")),
        "source": worker_load.get("source", ""),
        "matched_lines": int(worker_load.get("matched_lines", 0) or 0),
        "reason": worker_load.get("reason", ""),
    }
    complete = bool(
        ledger_path.is_file()
        and worker_quality["available"]
        and replay["overall"]["available"]
        and resource_summary["available"]
        and resource_summary["coverage"] >= 1.0
        and cleanup_metrics["passed"]
    )
    return {
        "schema_version": SUITE_METRICS_SCHEMA_VERSION,
        "metrics_type": "suite_metrics",
        "suite": "stability",
        "run_id": run_id,
        "phase": phase,
        "overall": overall,
        "by_dataset": by_dataset,
        "by_parallel_mode": by_mode,
        "by_dataset_parallel_mode": {
            f"{dataset}|{parallel_mode}": value for dataset, value in by_dataset.items()
        },
        "by_worker": per_worker,
        "by_worker_dataset": per_worker_dataset,
        "worker_load_distribution": load_distribution,
        "replay": replay,
        "submission_rate": overall["submission_rate"],
        "resources": resource_summary,
        "cleanup": cleanup_metrics,
        "complete": complete,
        "data_quality": {
            "episode_observations": {
                "available": ledger_path.is_file(),
                "source": f"{ledger_path}#episode",
                "row_count": overall["planned_episodes"],
            },
            "worker_attribution": worker_quality,
        },
    }
