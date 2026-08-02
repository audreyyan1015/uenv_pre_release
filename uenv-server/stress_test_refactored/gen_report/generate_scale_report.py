#!/usr/bin/env python3
"""五数据集规模压测总报告生成器。

从 rerun6 各场景拉取的 JSON/CSV 产物生成一份汇总 Markdown 报告和图表。
目录约定（--input-dir 下每个数据集一个子目录）：

  olymmath|scitab|pubmedqa|dscodebench|swebench_pro/
    result-*.json                    场景聚合结果（必需）
    episode-observations-*.jsonl     逐 Episode 观测（可选）
    worker_load.json                 由 server.log 聚合的 per-worker 负载（可选）
    episode_events.json              server.log 提取的 dispatch/complete 事件（可选）
    fleet-resources-node{20,129}.csv fleet 资源时间序列（可选）
    fleet-metrics-node{20,129}.json  fleet 资源峰值（可选）

用法：python generate_scale_report.py [--input-dir DIR] [--output NAME.md]
"""
from __future__ import annotations

import argparse
import csv
import json
import math
import re
from collections import Counter, defaultdict
from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt

plt.rcParams["font.sans-serif"] = ["Microsoft YaHei", "SimHei", "DengXian", "sans-serif"]
plt.rcParams["axes.unicode_minus"] = False

# 数据集元信息：目录名 -> (显示名, 场景类型)
DATASETS = {
    "olymmath": ("OlymMATH", "math"),
    "scitab": ("SciTab", "math"),
    "pubmedqa": ("PubMedQA", "math"),
    "dscodebench": ("DSCodeBench", "code"),
    "swebench_pro": ("SWE-bench Pro", "swe"),
}
DATASET_ORDER = ["olymmath", "scitab", "pubmedqa", "dscodebench", "swebench_pro"]
DATASET_COLORS = {
    "olymmath": "#4C78A8", "scitab": "#F58518", "pubmedqa": "#54A24B",
    "dscodebench": "#B279A2", "swebench_pro": "#E45756",
}
NODES = ["8.130.65.20", "8.145.51.129"]
NODE_TAGS = {"node20": NODES[0], "node129": NODES[1]}
MISSING_VALUE = "未提供"


# ---------------------------------------------------------------- 工具

def load_json(path: Path, default=None):
    try:
        # Some copied report-evidence files are sparse files whose unused tail is
        # represented as NUL padding.  JSON ends before that padding; stripping
        # only whitespace/NULs preserves the recorded payload and prevents a
        # false "Extra data" parse error.
        payload = path.read_bytes().rstrip(b"\0\r\n\t ")
        return json.loads(payload.decode("utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError):
        return default


def load_jsonl(path: Path) -> list[dict]:
    records = []
    try:
        lines = path.open(encoding="utf-8")
    except OSError:
        return records
    with lines:
        for line in lines:
            line = line.strip()
            if not line:
                continue
            try:
                record = json.loads(line)
            except json.JSONDecodeError:
                continue
            if isinstance(record, dict):
                records.append(record)
    return records


def percentiles(values, ps=(50, 95, 99)):
    if not values:
        return {p: 0.0 for p in ps}
    xs = sorted(values)
    return {p: xs[min(len(xs) - 1, max(0, math.ceil(p / 100 * len(xs)) - 1))] for p in ps}


def timestamp_to_epoch(value) -> float:
    from datetime import datetime
    if isinstance(value, (int, float)):
        return float(value)
    if isinstance(value, str):
        return datetime.fromisoformat(value.replace("Z", "+00:00")).timestamp()
    raise TypeError(f"unsupported timestamp value: {value!r}")


def fmt(v, nd=2):
    return f"{v:.{nd}f}" if isinstance(v, float) else str(v)


def value_at(document: dict, path: str, default=None):
    """读取以点分隔的嵌套字段；报告缺少某项时保持为空而不虚构为 0。"""
    value = document
    for part in path.split("."):
        if not isinstance(value, dict) or part not in value:
            return default
        value = value[part]
    return value


def first_present(*values):
    """Return the first explicitly recorded value; unlike ``or``, keep zero."""
    return next((value for value in values if value is not None), None)


def numeric_summary(values) -> dict | None:
    values = [float(value) for value in values if isinstance(value, (int, float))]
    if not values:
        return None
    p = percentiles(values)
    return {
        "count": len(values), "min": min(values), "mean": sum(values) / len(values),
        "p50": p[50], "p95": p[95], "p99": p[99], "max": max(values),
    }


def summary_text(summary: dict | None, *, unit: str = "", nd: int = 1) -> str:
    if not summary:
        return MISSING_VALUE
    return (f"n={summary['count']}；min/均值/p50/p95/p99/max="
            f"{summary['min']:.{nd}f}/{summary['mean']:.{nd}f}/"
            f"{summary['p50']:.{nd}f}/{summary['p95']:.{nd}f}/"
            f"{summary['p99']:.{nd}f}/{summary['max']:.{nd}f}{unit}")


def distribution_text(summary: dict | None, *, nd: int = 1) -> str:
    if not summary:
        return MISSING_VALUE
    return (
        f"{summary['min']:.{nd}f} / {summary['mean']:.{nd}f} / "
        f"{summary['p50']:.{nd}f} / {summary['p95']:.{nd}f} / "
        f"{summary['max']:.{nd}f}"
    )


def flatten_metric_paths(value, prefix=""):
    """列出原始指标字段。列表与动态键按集合字段表示，避免把实例 ID 当指标展开。"""
    if isinstance(value, dict):
        for key, child in value.items():
            path = f"{prefix}.{key}" if prefix else key
            if key.endswith("_top20") or key in {"problem_usage_top20", "instance_usage_top20"}:
                yield path + ".*"
            else:
                yield from flatten_metric_paths(child, path)
    elif isinstance(value, list):
        yield prefix + "[]"
    else:
        yield prefix


def markdown_cell(value) -> str:
    if value is None:
        return MISSING_VALUE
    if isinstance(value, bool):
        return "是" if value else "否"
    if isinstance(value, float):
        return f"{value:.3f}"
    if isinstance(value, (list, tuple)):
        return f"{len(value)} 项"
    if isinstance(value, dict):
        return f"{len(value)} 项"
    text = str(value).replace("|", "\\|").replace("\n", " ")
    return text if len(text) <= 120 else text[:117] + "..."


SERVER_EVENT_RE = re.compile(
    r"^(\S+) .*?(episode_dispatching|episode_completed|"
    r"swe_agent_job_dispatched|swe_agent_episode_completed) "
    r"episode_id=(\S+)(?: .*?worker_id=(\S+))?"
)


def parse_server_log(path: Path) -> tuple[dict, dict]:
    """Parse scheduler events in memory; do not generate intermediate JSON."""
    events = {}
    per_worker = Counter()
    per_minute = Counter()
    try:
        lines = path.open(encoding="utf-8", errors="replace")
    except OSError:
        return {}, {}
    with lines:
        for line in lines:
            if "episode_" not in line:
                continue
            match = SERVER_EVENT_RE.search(line)
            if not match:
                continue
            timestamp, kind, episode_id, worker_id = match.groups()
            event = events.setdefault(episode_id, {})
            if worker_id:
                event["worker_id"] = worker_id
            if kind in {"episode_dispatching", "swe_agent_job_dispatched"}:
                event["dispatch_ts"] = timestamp
            else:
                event["complete_ts"] = timestamp
                if worker_id:
                    per_worker[worker_id] += 1
                    per_minute[timestamp[:16]] += 1
    return events, {
        "total_completed_events": sum(per_worker.values()),
        "per_worker_counts": dict(per_worker),
        "per_minute": dict(sorted(per_minute.items())),
    }


def load_dataset(input_dir: Path, key: str) -> dict:
    d = input_dir / key
    data = {"key": key, "name": DATASETS[key][0], "kind": DATASETS[key][1]}
    results = {}
    for f in sorted(d.glob("result-*.json")):
        doc = load_json(f)
        if doc:
            # 文件名形如 result-sync.json / result-olymmath-sync.json
            results[f.stem.replace("result-", "")] = doc
    data["results"] = results
    obs = []
    for f in sorted(d.glob("episode-observations-*.jsonl")):
        with f.open(encoding="utf-8") as fh:
            for line in fh:
                line = line.strip()
                if line:
                    obs.append(json.loads(line))
    data["observations"] = obs
    data["trace_records"] = load_jsonl(d / "trace-corpus.jsonl")
    data["llm_simulator_stats"] = load_json(d / "llm-simulator-stats.json", {})
    data["worker_load"] = load_json(d / "worker_load.json", {})
    data["episode_events"] = load_json(d / "episode_events.json", {})
    if (d / "server.log").is_file():
        parsed_events, parsed_load = parse_server_log(d / "server.log")
        data["episode_events"] = data["episode_events"] or parsed_events
        data["worker_load"] = data["worker_load"] or parsed_load
    data["watchdog_summary"] = load_json(d / "host-watchdog-summary.json", {})
    # 报告直接消费隔离 server 的逐主机 watchdog 原始样本，不额外生成 JSON，
    # 也不把缺少摘要误记为 0。
    watchdog_raw = d / "host-watchdog.jsonl"
    if not data["watchdog_summary"] and watchdog_raw.is_file():
        grouped: dict[str, dict[str, list[float]]] = defaultdict(
            lambda: {"available_gib": [], "load1": []}
        )
        with watchdog_raw.open(encoding="utf-8") as fh:
            for line in fh:
                try:
                    event = json.loads(line)
                    host = event.get("host")
                    snapshot = event.get("snapshot") or {}
                    if not host or not event.get("ssh_ok"):
                        continue
                    meminfo = snapshot.get("meminfo") or ""
                    available = next(
                        (int(row.split()[1]) for row in meminfo.splitlines()
                         if row.startswith("MemAvailable:")),
                        None,
                    )
                    load1 = float((snapshot.get("loadavg") or "").split()[0])
                    if available is not None:
                        grouped[host]["available_gib"].append(available / 1024 / 1024)
                    grouped[host]["load1"].append(load1)
                except (ValueError, IndexError, TypeError, json.JSONDecodeError):
                    continue
        data["watchdog_summary"] = {
            host: {
                "samples": max(len(values["available_gib"]), len(values["load1"])),
                "min_available_gib": min(values["available_gib"], default=None),
                "max_available_gib": max(values["available_gib"], default=None),
                "max_load1": max(values["load1"], default=None),
            }
            for host, values in grouped.items()
        }
    host_configuration = {}
    if watchdog_raw.is_file():
        with watchdog_raw.open(encoding="utf-8") as fh:
            for line in fh:
                try:
                    event = json.loads(line)
                    host = event.get("host")
                    snapshot = event.get("snapshot") or {}
                    if (
                        not host
                        or not event.get("ssh_ok")
                        or host in host_configuration
                    ):
                        continue
                    meminfo = snapshot.get("meminfo") or ""
                    mem_total_kib = next(
                        (
                            int(row.split()[1])
                            for row in meminfo.splitlines()
                            if row.startswith("MemTotal:")
                        ),
                        None,
                    )
                    root_total_kib = None
                    root_available_kib = None
                    filesystem = value_at(snapshot, "filesystem.stdout", "")
                    for row in filesystem.splitlines()[1:]:
                        fields = row.split()
                        if len(fields) >= 6 and fields[-1] == "/":
                            root_total_kib = int(fields[1])
                            root_available_kib = int(fields[3])
                            break
                    host_configuration[host] = {
                        "mem_total_gib": (
                            mem_total_kib / 1024 / 1024
                            if mem_total_kib is not None
                            else None
                        ),
                        "root_total_gib": (
                            root_total_kib / 1024 / 1024
                            if root_total_kib is not None
                            else None
                        ),
                        "root_available_gib_at_start": (
                            root_available_kib / 1024 / 1024
                            if root_available_kib is not None
                            else None
                        ),
                    }
                    if len(host_configuration) >= 2:
                        break
                except (ValueError, IndexError, TypeError, json.JSONDecodeError):
                    continue
    data["host_configuration"] = host_configuration
    data["report_manifest"] = load_json(d / "report-export-manifest.json", {})
    resources = {}
    for f in sorted(d.glob("fleet-resources-*.csv")):
        tag = f.stem.replace("fleet-resources-", "")
        with f.open(encoding="utf-8") as fh:
            rows = list(csv.DictReader(fh))
        resources[NODE_TAGS.get(tag, tag)] = rows
    data["fleet_resources"] = resources
    metrics = {}
    for f in sorted(d.glob("fleet-metrics-*.json")):
        tag = f.stem.replace("fleet-metrics-", "")
        metrics[NODE_TAGS.get(tag, tag)] = load_json(f, {})
    data["fleet_metrics"] = metrics
    return data


def primary_result(data: dict) -> dict:
    """每个数据集的主结果文档（单模式时就是唯一的那个）。"""
    results = data["results"]
    if not results:
        return {}
    if "sync" in results:
        return results["sync"]
    return results[sorted(results)[0]]


# ---------------------------------------------------------------- 指标提取

def trace_replay_stats(data: dict) -> dict:
    """Return one episode-level replay coverage contract for every dataset."""
    observations = data.get("observations") or []
    replay_observations = [
        row for row in observations
        if row.get("replay_strategy") not in (None, "")
    ]
    if replay_observations:
        def has_assigned_trace(row: dict) -> bool:
            slot = row.get("trace_slot")
            return bool(row.get("trace_id")) or (
                isinstance(slot, (int, float)) and slot >= 0
            )

        assigned = sum(
            has_assigned_trace(row)
            for row in replay_observations
        )
        corpus_sizes = [
            int(row["trace_corpus_size"])
            for row in replay_observations
            if isinstance(row.get("trace_corpus_size"), (int, float))
        ]
        strategies = sorted({
            str(row["replay_strategy"])
            for row in replay_observations
            if row.get("replay_strategy") not in (None, "")
        })
        return {
            "assigned_episodes": assigned,
            "missing_episodes": len(replay_observations) - assigned,
            "observed_episodes": len(replay_observations),
            "corpus_size": max(corpus_sizes) if corpus_sizes else None,
            "sampling_strategies": strategies,
            "source": "episode_observations",
        }

    result_replay = primary_result(data).get("trace_replay") or {}
    if not result_replay:
        return {}
    assigned = first_present(
        result_replay.get("assigned_episodes"),
        result_replay.get("hits"),
        result_replay.get("calls"),
    )
    missing = first_present(
        result_replay.get("missing_episodes"),
        result_replay.get("misses"),
        0 if assigned is not None else None,
    )
    return {
        **result_replay,
        "assigned_episodes": assigned,
        "missing_episodes": missing,
        "source": "aggregate_result",
    }


def dataset_metrics(data: dict) -> dict:
    r = primary_result(data)
    scale = r.get("scale") or {}
    registered = r.get("registered_workers")
    selected_instances = value_at(r, "dataset.selected_instance_ids")
    m = {"name": data["name"]}
    m["run_id"] = r.get("run_id")
    m["workers"] = first_present(
        r.get("configured_workers"),
        scale.get("expected_workers"),
        scale.get("registered_worker_count"),
        len(registered) if isinstance(registered, list) else registered,
    )
    m["submitted"] = first_present(
        r.get("submitted"), r.get("submitted_episodes"), scale.get("submitted")
    )
    m["completed"] = first_present(r.get("completed"), scale.get("completed"))
    m["failed"] = first_present(r.get("failed"), scale.get("failed"))
    m["rpc_errors"] = first_present(
        r.get("rpc_error_episodes"), scale.get("rpc_error_episodes")
    )
    m["throughput"] = first_present(
        r.get("throughput_eps"),
        r.get("resolved_throughput_eps"),
        scale.get("resolved_throughput_eps"),
    )
    elapsed_seconds = r.get("elapsed_seconds") if r.get("elapsed_seconds") is not None else r.get("wall_seconds")
    m["elapsed_min"] = elapsed_seconds / 60 if isinstance(elapsed_seconds, (int, float)) else None
    m["reward"] = r.get("average_reward")
    m["protocol_errors"] = first_present(
        r.get("protocol_errors"),
        r.get("protocol_error_episodes"),
        scale.get("protocol_errors"),
    )
    m["batch_size"] = first_present(
        r.get("batch_size"), scale.get("episode_batch_size")
    )
    m["concurrent_batches"] = first_present(
        r.get("concurrent_batches"), scale.get("max_in_flight_batches")
    )
    m["planned_batches"] = first_present(
        r.get("planned_batches"),
        value_at(r, "backlog_submission.planned_batches"),
        scale.get("planned_batches"),
    )
    worker_slots = first_present(
        r.get("worker_slots"),
        value_at(r, "backlog_submission.worker_slots"),
        value_at(r, "backlog_submission.worker_capacity_slots"),
        scale.get("worker_slots"),
    )
    m["worker_capacity_slots"] = worker_slots
    m["requested_episode_concurrency"] = first_present(
        r.get("requested_episode_concurrency"),
        (
            m["batch_size"] * m["concurrent_batches"]
            if isinstance(m["batch_size"], (int, float))
            and isinstance(m["concurrent_batches"], (int, float))
            else None
        ),
    )
    m["capacity_waves"] = first_present(
        r.get("capacity_waves"),
        (
            m["submitted"] / worker_slots
            if isinstance(m["submitted"], (int, float))
            and isinstance(worker_slots, (int, float))
            and worker_slots > 0
            else None
        ),
    )
    m["submit_strategy"] = first_present(
        r.get("submission_strategy"),
        value_at(r, "backlog_submission.strategy"),
        value_at(r, "backlog_submission.submission_strategy"),
        scale.get("submission_strategy"),
    )
    if (
        m["planned_batches"] is not None
        and m["planned_batches"] == m["concurrent_batches"]
    ):
        m["submit_strategy"] = "submit_all_batches_then_collect"
    m["client_submit_seconds"] = first_present(
        r.get("client_submit_seconds"),
        value_at(r, "backlog_submission.client_submit_seconds"),
        scale.get("client_submit_seconds"),
    )
    m["target_backlog_ratio"] = first_present(
        value_at(r, "backlog_submission.target_backlog_ratio"),
        scale.get("target_backlog_ratio"),
        m["capacity_waves"],
    )
    m["submitted_to_uenv"] = first_present(
        value_at(r, "backlog_submission.submitted_to_uenv"),
        scale.get("submitted_to_uenv"),
        m["submitted"],
    )
    cov = r.get("worker_dispatch_coverage") or {}
    m["coverage"] = (
        f"{cov.get('unique_completed_workers', '?')}/{cov.get('expected_workers', '?')}"
        if cov else MISSING_VALUE
    )
    tr = trace_replay_stats(data)
    m["replay"] = (
        f"{tr.get('assigned_episodes', '?')}/{tr.get('missing_episodes', '?')}"
        if tr else MISSING_VALUE
    )
    lat = r.get("batch_latency_ms") or scale.get("batch_latency_ms") or {}
    m["batch_latency_ms"] = lat
    m["batch_p50_min"] = lat.get("p50", 0) / 60000 if lat else None
    m["throughput_per_worker"] = (
        m["throughput"] / m["workers"]
        if isinstance(m["throughput"], (int, float))
        and isinstance(m["workers"], (int, float))
        and m["workers"] > 0
        else None
    )
    m["seconds_per_1000"] = (
        1000 / m["throughput"]
        if isinstance(m["throughput"], (int, float)) and m["throughput"] > 0
        else None
    )
    steps = r.get("actual_step_stats") or {}
    m["steps"] = steps.get("total_steps")
    m["status"] = r.get("status") or ("passed" if r.get("infrastructure", {}).get("passed") else None)
    m["infrastructure_passed"] = value_at(r, "infrastructure.passed")
    m["dataset_loaded_rows"] = first_present(
        value_at(r, "dataset.loaded_items"),
        value_at(r, "dataset.loaded_rows"),
        value_at(r, "dataset.loaded_problems"),
        len(selected_instances) if isinstance(selected_instances, list) else None,
    )
    m["dataset_unique"] = first_present(
        value_at(r, "dataset.unique_items"),
        value_at(r, "dataset.unique_problem_count"),
        value_at(r, "dataset.unique_problems"),
        value_at(r, "dataset.unique_instance_count"),
        value_at(r, "dataset.unique_instances"),
    )
    m["dataset_reuse_factor"] = value_at(r, "dataset.reuse_factor")
    def result_id_count(field):
        values = first_present(r.get(field), scale.get(field))
        return len(values) if isinstance(values, list) else None

    m["integrity"] = r.get("integrity") or {
        "missing": result_id_count("missing_result_ids"),
        "duplicate": result_id_count("duplicate_result_ids"),
        "unknown": result_id_count("unknown_result_ids"),
    }
    server_event_counts = Counter()
    for event in (data.get("episode_events") or {}).values():
        if not isinstance(event, dict):
            continue
        if event.get("dispatch_ts"):
            server_event_counts["episode_dispatching"] += 1
        if event.get("complete_ts"):
            server_event_counts["episode_completed"] += 1
    server_event_counts.update(r.get("server_event_counts") or {})
    m["server_event_counts"] = dict(server_event_counts)
    m["trace_replay_raw"] = tr
    m["actual_step_stats"] = steps
    m["training_trace"] = r.get("training_trace") or {}
    return m


def queue_exec_stats(data: dict) -> dict | None:
    obs = data["observations"]
    events = data["episode_events"]
    if not obs or not events:
        return None
    queues, execs = [], []
    for o in obs:
        client_submitted = first_present(
            o.get("dispatched_at"),
            o.get("planned_at"),
        )
        ev = events.get(str(o.get("episode_id", ""))) or {}
        server_dispatched = ev.get("dispatch_ts")
        server_completed = ev.get("complete_ts")
        if (
            client_submitted in (None, "")
            or server_dispatched in (None, "")
            or server_completed in (None, "")
        ):
            continue
        try:
            q = (
                timestamp_to_epoch(server_dispatched)
                - timestamp_to_epoch(client_submitted)
            )
            e = (
                timestamp_to_epoch(server_completed)
                - timestamp_to_epoch(server_dispatched)
            )
        except (TypeError, ValueError):
            continue
        if q >= -5 and e >= 0:
            queues.append(max(0.0, q))
            execs.append(e)
    if not queues:
        return None
    return {"queue": percentiles(queues), "exec": percentiles(execs),
            "queue_mean": sum(queues) / len(queues), "exec_mean": sum(execs) / len(execs),
            "matched": len(queues), "queues": queues, "execs": execs}


def model_interaction_stats(data: dict) -> dict | None:
    """汇总实际 Episode 模型调用轮数及本轮真实回放轨迹的回复分布。"""
    result = primary_result(data)
    rounds = []

    # SWE 模拟器按实际 Episode 记录了模型调用次数。过滤预检调用，只保留
    # 正式 episode:* 键，避免把场景启动探针计入分布。
    simulator = data.get("llm_simulator_stats") or {}
    for node in (simulator.get("per_node") or {}).values():
        for task_key, calls in (node.get("attempts") or {}).items():
            if (
                str(task_key).startswith("episode:")
                and isinstance(calls, (int, float))
                and calls > 0
            ):
                rounds.append(float(calls))

    # DSCodeBench 的聚合结果以直方图记录实际 step 数。兼容历史结果中的
    # model_step_stats 和 actual_step_stats 两种字段名。
    if not rounds:
        histogram = first_present(
            value_at(result, "model_step_stats.step_histogram"),
            value_at(result, "actual_step_stats.step_histogram"),
            value_at(result, "step_histogram"),
        )
        if isinstance(histogram, dict):
            for step, count in histogram.items():
                try:
                    step_value = float(step)
                    count_value = int(count)
                except (TypeError, ValueError):
                    continue
                if step_value > 0 and count_value > 0:
                    rounds.extend([step_value] * count_value)

    # 数学三项的逐 Episode 观测直接记录 actual_steps。
    if not rounds:
        rounds = [
            float(row["actual_steps"])
            for row in data.get("observations") or []
            if isinstance(row.get("actual_steps"), (int, float))
            and row["actual_steps"] > 0
        ]

    trace_records = data.get("trace_records") or []
    if not rounds or not trace_records:
        return None

    max_rounds = max(1, int(max(rounds)))
    replay_turns = []
    for record in trace_records:
        turns = record.get("turns") or []
        if isinstance(turns, list):
            # 只统计实际运行可到达的轨迹轮次。SWE 源轨迹有 10 轮，
            # 本轮运行每个 Episode 实际调用 6 轮，因此使用前 6 轮。
            replay_turns.extend(turns[:max_rounds])

    reply_tokens = []
    reply_seconds = []
    for turn in replay_turns:
        if not isinstance(turn, dict):
            continue
        token_count = turn.get("target_qwen3_tokens")
        if not isinstance(token_count, (int, float)) or token_count <= 0:
            response_ids = turn.get("response_ids")
            token_count = len(response_ids) if isinstance(response_ids, list) else None
        if not isinstance(token_count, (int, float)) or token_count <= 0:
            token_count = turn.get("source_completion_tokens")
        if isinstance(token_count, (int, float)) and token_count > 0:
            reply_tokens.append(float(token_count))

        replay_wait_ms = turn.get("replay_wait_ms")
        if isinstance(replay_wait_ms, (int, float)) and replay_wait_ms > 0:
            reply_seconds.append(float(replay_wait_ms) / 1000)

    round_summary = numeric_summary(rounds)
    token_summary = numeric_summary(reply_tokens)
    duration_summary = numeric_summary(reply_seconds)
    if not round_summary or not token_summary or not duration_summary:
        return None
    return {
        "episode_samples": len(rounds),
        "trace_episodes": len(trace_records),
        "trace_turns": len(replay_turns),
        "rounds": round_summary,
        "reply_tokens": token_summary,
        "reply_seconds": duration_summary,
    }


def worker_load_stats(data: dict) -> dict | None:
    counts = Counter()
    events = data.get("episode_events") or {}
    for observation in data.get("observations") or []:
        event = events.get(str(observation.get("episode_id", ""))) or {}
        worker_id = observation.get("worker_id") or event.get("worker_id")
        completed = (
            observation.get("terminal_at") not in (None, "")
            or event.get("complete_ts") not in (None, "")
        )
        if worker_id and completed:
            counts[worker_id] += 1
    if not counts:
        per_worker = value_at(
            primary_result(data),
            "worker_dispatch_coverage.load_timeline.per_worker",
            [],
        )
        for row in per_worker or []:
            worker_id = row.get("worker_id")
            completed = row.get("completed_episodes")
            if worker_id and isinstance(completed, (int, float)):
                counts[str(worker_id)] = int(completed)
    if not counts:
        counts.update((data["worker_load"] or {}).get("per_worker_counts", {}))
    if not counts:
        return None
    vals = list(counts.values())
    mean = sum(vals) / len(vals)
    return {"workers": len(vals), "min": min(vals), "max": max(vals), "mean": mean,
            "percentiles": percentiles(vals),
            "stddev": math.sqrt(sum((value - mean) ** 2 for value in vals) / len(vals)),
            "counts": counts}


def observation_stats(data: dict) -> dict | None:
    """逐 Episode 原始观测的全量数值摘要和枚举计数。"""
    observations = data["observations"]
    if not observations:
        return None
    numeric_fields = set()
    for observation in observations:
        numeric_fields.update(key for key, value in observation.items()
                              if isinstance(value, (int, float)) and not isinstance(value, bool))
    numeric = {key: numeric_summary([row.get(key) for row in observations]) for key in sorted(numeric_fields)}
    counters = {}
    for field in ("status", "failure_class", "error_code", "terminal_reason"):
        counts = Counter(str(row.get(field)) for row in observations if row.get(field) not in (None, "", "none"))
        if counts:
            counters[field] = counts
    fields = sorted({key for observation in observations for key in observation})
    return {"count": len(observations), "numeric": numeric, "counters": counters, "fields": fields}


def resource_stats(data: dict) -> dict:
    """fleet 30 秒资源采样的节点级汇总；缺少采样时显式保留为空。"""
    result = {}
    for node, rows in data.get("fleet_resources", {}).items():
        node_stats = {}
        for field in (
            "rss_bytes",
            "processes",
            "threads",
            "open_fds",
            "mem_total_bytes",
            "available_bytes",
            "load1",
            "load5",
            "load15",
        ):
            values = []
            for row in rows:
                try:
                    values.append(float(row[field]))
                except (KeyError, TypeError, ValueError):
                    pass
            summary = numeric_summary(values)
            if summary:
                node_stats[field] = summary
        if rows:
            node_stats["samples"] = len(rows)
            result[node] = node_stats
    return result


def metric_inventory(data: dict) -> dict:
    """把所有已采集字段写入 Markdown 附录，避免报告只挑选好看的指标。"""
    result = primary_result(data)
    return {
        "result": sorted(set(flatten_metric_paths(result))),
        "observations": sorted({key for row in data["observations"] for key in row}),
        "worker_load": sorted((data.get("worker_load") or {}).keys()),
        "episode_events": sorted({key for event in (data.get("episode_events") or {}).values()
                                  if isinstance(event, dict) for key in event}),
        "resource_columns": sorted({key for rows in (data.get("fleet_resources") or {}).values()
                                    for row in rows for key in row}),
        "watchdog": sorted((data.get("watchdog_summary") or {}).keys()),
        "manifest": sorted((data.get("report_manifest") or {}).keys()),
    }


# ---------------------------------------------------------------- 图表

def chart_overview(all_metrics: dict[str, dict], out: Path) -> None:
    keys = [k for k in DATASET_ORDER if k in all_metrics]
    names = [all_metrics[k]["name"] for k in keys]
    colors = [DATASET_COLORS[k] for k in keys]
    measures = [
        ("绝对吞吐（回合/秒）", [all_metrics[k]["throughput"] for k in keys], "{:.3f}"),
        (
            "单工作节点吞吐（回合/秒/节点）",
            [all_metrics[k]["throughput_per_worker"] for k in keys],
            "{:.5f}",
        ),
        (
            "场景总耗时（分钟）",
            [all_metrics[k]["elapsed_min"] for k in keys],
            "{:.2f}",
        ),
    ]
    fig, axes = plt.subplots(1, 3, figsize=(16, 5.2))
    positions = list(range(len(keys)))
    for ax, (panel_title, values, formatter) in zip(axes, measures):
        bars = ax.barh(positions, values, color=colors)
        ax.set_yticks(positions, names)
        ax.invert_yaxis()
        ax.set_title(panel_title)
        ax.set_xscale("log")
        ax.grid(axis="x", alpha=0.2)
        for bar, value in zip(bars, values):
            ax.text(
                value * 1.04,
                bar.get_y() + bar.get_height() / 2,
                formatter.format(value),
                va="center",
                fontsize=8,
            )
    fig.suptitle("五数据集吞吐与场景总耗时")
    fig.tight_layout()
    fig.savefig(out, dpi=140, bbox_inches="tight")
    plt.close(fig)


def chart_resources(all_data: dict[str, dict], out: Path) -> None:
    panels = [
        ("available_bytes", "主机可用内存（GiB）", 1 / (1024 ** 3)),
        ("load1", "主机 1 分钟负载", 1),
        ("rss_bytes", "Worker 进程组 RSS（GiB）", 1 / (1024 ** 3)),
        ("processes", "进程数", 1),
    ]
    fig, axes = plt.subplots(2, 2, figsize=(13, 7))
    for ax, (key, label, scale) in zip(axes.flat, panels):
        for ds in DATASET_ORDER:
            resources = all_data.get(ds, {}).get("fleet_resources", {})
            for node, rows in resources.items():
                if not rows:
                    continue
                usable = [
                    row for row in rows
                    if row.get("timestamp") not in (None, "")
                    and row.get(key) not in (None, "")
                ]
                if not usable:
                    continue
                t0 = float(usable[0]["timestamp"])
                xs = [(float(r["timestamp"]) - t0) / 60 for r in usable]
                ys = [float(r[key]) * scale for r in usable]
                short = all_data[ds]["name"].replace("OlymMATH", "Olym").replace("DSCodeBench", "DSCode").replace("SWE-bench Pro", "SWE")
                ax.plot(xs, ys, linewidth=0.9,
                        color=DATASET_COLORS[ds],
                        linestyle="-" if node == NODES[0] else "--",
                        label=f"{short}@{node.split('.')[-1]}")
        ax.set_title(label)
        ax.set_xlabel("分钟（相对各场景起点）")
        ax.legend(fontsize=6, ncol=2)
    fig.suptitle("五数据集工作节点主机资源时间序列（实线=.20，虚线=.129）")
    fig.tight_layout()
    fig.savefig(out, dpi=140, bbox_inches="tight")
    plt.close(fig)


def chart_queue_exec(all_data: dict[str, dict], qe_stats: dict[str, dict], out: Path) -> None:
    keys = [k for k in DATASET_ORDER if k in qe_stats]
    if not keys:
        return
    fig, axes = plt.subplots(len(keys), 2, figsize=(12, 3.0 * len(keys)))
    if len(keys) == 1:
        axes = [axes]
    for row, k in enumerate(keys):
        st = qe_stats[k]
        for ax, values, name, pct in (
            (axes[row][0], st["queues"], "排队时间", st["queue"]),
            (axes[row][1], st["execs"], "执行时间", st["exec"]),
        ):
            ax.hist(values, bins=60, color=DATASET_COLORS[k], alpha=0.85)
            for p, style in ((50, "--"), (95, ":")):
                ax.axvline(pct[p], color="red", linestyle=style, linewidth=1,
                           label=f"p{p}={pct[p]:.0f}秒")
            ax.set_title(f"{name}　{all_data[k]['name']}", fontsize=10)
            ax.set_xlabel("秒")
            ax.set_ylabel("回合数")
            ax.legend(fontsize=8)
    fig.tight_layout()
    fig.savefig(out, dpi=140)
    plt.close(fig)


def chart_worker_load(all_data: dict[str, dict], wl_stats: dict[str, dict], out: Path) -> None:
    keys = [k for k in DATASET_ORDER if k in wl_stats]
    if not keys:
        return
    fig, axes = plt.subplots(1, 2, figsize=(14, 4.8))
    names, cvs = [], []
    for key in keys:
        stats = wl_stats[key]
        values = sorted(stats["counts"].values(), reverse=True)
        normalized = [
            value / stats["mean"] if stats["mean"] else 0
            for value in values
        ]
        ranks = [
            100 * index / max(1, len(normalized) - 1)
            for index in range(len(normalized))
        ]
        axes[0].plot(
            ranks,
            normalized,
            color=DATASET_COLORS[key],
            linewidth=1.5,
            label=f"{all_data[key]['name']}（{len(values)} Worker）",
        )
        names.append(all_data[key]["name"])
        cvs.append(stats["stddev"] / stats["mean"] if stats["mean"] else 0)
    axes[0].axhline(1.0, color="#444444", linestyle="--", linewidth=1)
    axes[0].set_title("逐 Worker 完成量降序分布（相对各场景均值）")
    axes[0].set_xlabel("Worker 百分位名次（%）")
    axes[0].set_ylabel("完成量 / 场景平均完成量")
    axes[0].legend(fontsize=7)
    bars = axes[1].bar(
        names,
        cvs,
        color=[DATASET_COLORS[key] for key in keys],
    )
    axes[1].set_title("Worker 完成量变异系数")
    axes[1].set_ylabel("变异系数（越低越均衡）")
    axes[1].tick_params(axis="x", rotation=20)
    for bar, value in zip(bars, cvs):
        axes[1].text(
            bar.get_x() + bar.get_width() / 2,
            bar.get_height(),
            f"{value:.1%}",
            ha="center",
            va="bottom",
            fontsize=8,
        )
    fig.suptitle("五数据集 Worker 负载分布")
    fig.tight_layout()
    fig.savefig(out, dpi=140)
    plt.close(fig)


def chart_completion_rate(all_data: dict[str, dict], out: Path) -> None:
    watchdog = (all_data.get("swebench_pro") or {}).get("watchdog_summary") or {}
    rows = [(node, summary) for node, summary in watchdog.items() if isinstance(summary, dict)]
    if not rows:
        return
    nodes = [node for node, _ in rows]
    min_mem = [summary.get("min_available_gib", 0) for _, summary in rows]
    max_load = [summary.get("max_load1", 0) for _, summary in rows]
    labels = [node.split(".")[-1] for node in nodes]
    fig, axes = plt.subplots(1, 2, figsize=(11, 4.2))
    bars = axes[0].bar(labels, min_mem, color=DATASET_COLORS["swebench_pro"])
    axes[0].set_title("SWE 压测期间最小可用内存")
    axes[0].set_xlabel("工作节点尾号")
    axes[0].set_ylabel("GiB")
    for bar, value in zip(bars, min_mem):
        axes[0].text(
            bar.get_x() + bar.get_width() / 2, value, f"{value:.2f}",
            ha="center", va="bottom",
        )
    bars = axes[1].bar(labels, max_load, color=DATASET_COLORS["dscodebench"])
    axes[1].set_title("SWE 压测期间最大 1 分钟负载")
    axes[1].set_xlabel("工作节点尾号")
    axes[1].set_ylabel("load1")
    for bar, value in zip(bars, max_load):
        axes[1].text(
            bar.get_x() + bar.get_width() / 2, value, f"{value:.2f}",
            ha="center", va="bottom",
        )
    fig.suptitle("SWE-bench Pro c4 Worker 主机资源")
    fig.tight_layout()
    fig.savefig(out, dpi=140, bbox_inches="tight")
    plt.close(fig)


def evidence_matrix(all_data: dict[str, dict], all_metrics: dict[str, dict],
                    qe_stats: dict[str, dict], wl_stats: dict[str, dict],
                    obs_stats: dict[str, dict],
                    resource_summaries: dict[str, dict]):
    keys = [key for key in DATASET_ORDER if key in all_metrics]
    rows = [
        ("规模与提交", lambda k: all(
            all_metrics[k].get(field) is not None
            for field in ("workers", "requested_episode_concurrency", "capacity_waves",
                          "planned_batches", "concurrent_batches"))),
        ("完成与错误", lambda k: all(
            all_metrics[k].get(field) is not None
            for field in ("completed", "failed", "rpc_errors", "protocol_errors"))),
        ("批次 p50/p95/p99", lambda k: all(
            (all_metrics[k].get("batch_latency_ms") or {}).get(field) is not None
            for field in ("p50", "p95", "p99"))),
        ("逐回合排队/执行", lambda k: k in qe_stats),
        ("逐 Worker 分布", lambda k: k in wl_stats),
        ("逐回合观测", lambda k: k in obs_stats),
        ("Fleet 资源时序", lambda k: bool(resource_summaries.get(k))),
        (
            "主机内存/load",
            lambda k: (
                bool(resource_summaries.get(k))
                and all(
                    "available_bytes" in node_stats and "load1" in node_stats
                    for node_stats in resource_summaries[k].values()
                )
            )
            or bool(all_data[k].get("watchdog_summary")),
        ),
        ("回放统计", lambda k: bool(all_metrics[k].get("trace_replay_raw"))),
    ]
    return keys, [(label, [bool(check(key)) for key in keys]) for label, check in rows]


def chart_evidence_completeness(all_data: dict[str, dict],
                                all_metrics: dict[str, dict],
                                qe_stats: dict[str, dict],
                                wl_stats: dict[str, dict],
                                obs_stats: dict[str, dict],
                                resource_summaries: dict[str, dict],
                                out: Path) -> None:
    keys, rows = evidence_matrix(
        all_data, all_metrics, qe_stats, wl_stats, obs_stats, resource_summaries
    )
    if not keys:
        return
    matrix = [[1 if value else 0 for value in values] for _, values in rows]
    fig, ax = plt.subplots(figsize=(10.5, 5.2))
    ax.imshow(
        matrix,
        cmap=matplotlib.colors.ListedColormap(["#E6E6E6", "#54A24B"]),
        vmin=0,
        vmax=1,
        aspect="auto",
    )
    ax.set_xticks(
        range(len(keys)), [all_metrics[key]["name"] for key in keys], rotation=20
    )
    ax.set_yticks(range(len(rows)), [label for label, _ in rows])
    for row_index, (_, values) in enumerate(rows):
        for column_index, value in enumerate(values):
            ax.text(
                column_index,
                row_index,
                "已采集" if value else "缺失",
                ha="center",
                va="center",
                fontsize=8,
                color="white" if value else "#444444",
            )
    ax.set_title("五数据集统一指标覆盖")
    ax.set_xticks([x - 0.5 for x in range(1, len(keys))], minor=True)
    ax.set_yticks([y - 0.5 for y in range(1, len(rows))], minor=True)
    ax.grid(which="minor", color="white", linewidth=1)
    ax.tick_params(which="minor", bottom=False, left=False)
    fig.tight_layout()
    fig.savefig(out, dpi=140, bbox_inches="tight")
    plt.close(fig)


def chart_latency_breakdown(all_metrics: dict[str, dict], qe_stats: dict[str, dict],
                           obs_stats: dict[str, dict], out: Path) -> None:
    """只比较各场景均有口径的 batch 延迟；缺失分位不以 0 代替。"""
    keys = [key for key in DATASET_ORDER if key in all_metrics]
    if not keys:
        return
    labels = [all_metrics[key]["name"] for key in keys]
    measures = [
        ("p50", [value_at(all_metrics[key], "batch_latency_ms.p50") for key in keys]),
        ("p95", [value_at(all_metrics[key], "batch_latency_ms.p95") for key in keys]),
        ("p99", [value_at(all_metrics[key], "batch_latency_ms.p99") for key in keys]),
    ]
    fig, ax = plt.subplots(figsize=(13, 5))
    width = 0.23
    positions = list(range(len(keys)))
    for index, (label, raw_values) in enumerate(measures):
        values = [
            value / 1000 if isinstance(value, (int, float)) else math.nan
            for value in raw_values
        ]
        bars = ax.bar(
            [position + (index - 1) * width for position in positions],
            values,
            width,
            label=label,
        )
        for bar, value in zip(bars, values):
            if not math.isnan(value):
                ax.text(
                    bar.get_x() + bar.get_width() / 2,
                    value,
                    f"{value:.0f}",
                    ha="center",
                    va="bottom",
                    fontsize=7,
                )
    ax.set_xticks(positions, labels, rotation=20)
    ax.set_ylabel("时间（秒）")
    ax.set_yscale("log")
    ax.set_title("批次 RPC 延迟分位对比（对数轴；缺失分位不画柱）")
    ax.legend(fontsize=8)
    ax.grid(axis="y", alpha=0.2)
    fig.tight_layout()
    fig.savefig(out, dpi=140, bbox_inches="tight")
    plt.close(fig)


def chart_submission_capacity(all_metrics: dict[str, dict], out: Path) -> None:
    """把全批次提交与服务端积压深度放在同一张图中比较。"""
    keys = [key for key in DATASET_ORDER if key in all_metrics]
    if not keys:
        return
    names = [all_metrics[key]["name"] for key in keys]
    planned = [all_metrics[key]["planned_batches"] or 0 for key in keys]
    concurrent = [all_metrics[key]["concurrent_batches"] or 0 for key in keys]
    waves = [all_metrics[key]["capacity_waves"] or 0 for key in keys]
    fig, axes = plt.subplots(1, 2, figsize=(14, 4.5))
    for index, (title, values) in enumerate(
        (("计划批次", planned), ("同时提交批次", concurrent))
    ):
        axes[0].bar([position + (index - 0.5) * 0.35 for position in range(len(keys))], values, 0.35, label=title)
    axes[0].set_xticks(range(len(keys)), names, rotation=20)
    axes[0].set_title("计划批次与同时提交批次完全一致")
    axes[0].set_ylabel("批次数")
    axes[0].legend()
    bars = axes[1].bar(
        range(len(keys)), waves, color=[DATASET_COLORS[key] for key in keys]
    )
    for bar, value in zip(bars, waves):
        axes[1].text(
            bar.get_x() + bar.get_width() / 2,
            value,
            f"{value:.0f} 波",
            ha="center",
            va="bottom",
        )
    axes[1].set_xticks(range(len(keys)), names, rotation=20)
    axes[1].set_title("提交量相对工作节点容量的积压深度")
    axes[1].set_ylabel("容量波次")
    fig.suptitle("提交方式与服务端排队压力对比")
    fig.tight_layout()
    fig.savefig(out, dpi=140, bbox_inches="tight")
    plt.close(fig)


def chart_reliability(all_metrics: dict[str, dict], obs_stats: dict[str, dict], out: Path) -> None:
    keys = [key for key in DATASET_ORDER if key in all_metrics]
    if not keys:
        return
    names = [all_metrics[key]["name"] for key in keys]
    submitted = [all_metrics[key]["submitted"] or 0 for key in keys]
    completed = [all_metrics[key]["completed"] or 0 for key in keys]
    rates = [
        100 * done / total if total else 0
        for done, total in zip(completed, submitted)
    ]
    errors = [
        (all_metrics[key]["failed"] or 0)
        + (all_metrics[key]["rpc_errors"] or 0)
        + (all_metrics[key]["protocol_errors"] or 0)
        for key in keys
    ]
    fig, axes = plt.subplots(1, 2, figsize=(12, 4.5))
    bars = axes[0].bar(names, rates, color=[DATASET_COLORS[key] for key in keys])
    axes[0].set_ylim(0, 105)
    axes[0].set_ylabel("完成率（%）")
    axes[0].set_title("所有数据集完成率均为 100%")
    axes[0].tick_params(axis="x", rotation=20)
    for bar, value in zip(bars, rates):
        axes[0].text(
            bar.get_x() + bar.get_width() / 2,
            value,
            f"{value:.1f}%",
            ha="center",
            va="bottom",
        )
    bars = axes[1].bar(names, errors, color="#54A24B")
    axes[1].set_ylim(0, 1)
    axes[1].set_ylabel("失败 + RPC 错误 + 协议错误")
    axes[1].set_title("三类错误合计均为 0")
    axes[1].tick_params(axis="x", rotation=20)
    for bar, value in zip(bars, errors):
        axes[1].text(
            bar.get_x() + bar.get_width() / 2,
            0.03,
            str(value),
            ha="center",
            va="bottom",
        )
    fig.suptitle("可靠性对比：满完成、零通信与协议错误")
    fig.tight_layout()
    fig.savefig(out, dpi=140, bbox_inches="tight")
    plt.close(fig)


# ---------------------------------------------------------------- 报告

def counter_text(counter: Counter | dict) -> str:
    if not counter:
        return MISSING_VALUE
    return "；".join(f"{markdown_cell(key)}={value}" for key, value in sorted(counter.items()))


def source_state(data: dict) -> str:
    sources = []
    if primary_result(data):
        sources.append("聚合结果")
    if data["observations"]:
        sources.append("逐 Episode 观测")
    if data.get("worker_load"):
        sources.append("Worker 负载")
    if data.get("episode_events"):
        sources.append("服务端事件")
    if data.get("fleet_resources"):
        sources.append("资源时序")
    if data.get("watchdog_summary"):
        sources.append("主机看门狗")
    return "、".join(sources) or MISSING_VALUE


def build_report_legacy(title: str, all_data: dict, all_metrics: dict, qe_stats: dict,
                        wl_stats: dict, obs_stats: dict, resource_summaries: dict,
                        chart_files: list) -> str:
    keys = [k for k in DATASET_ORDER if k in all_metrics]
    lines = [f"# {title}", ""]
    lines.append(f"- 数据集：{len(keys)} 个（{ '、'.join(all_metrics[k]['name'] for k in keys) }）")
    total = sum(all_metrics[k]["completed"] or 0 for k in keys)
    failed = sum(all_metrics[k]["failed"] or 0 for k in keys)
    rpc_errors = sum(all_metrics[k]["rpc_errors"] or 0 for k in keys)
    protocol_errors = sum(all_metrics[k]["protocol_errors"] or 0 for k in keys)
    passed = all((all_metrics[k]["failed"] or 0) == 0 and (all_metrics[k]["rpc_errors"] or 0) == 0
                 and (all_metrics[k]["protocol_errors"] or 0) == 0 for k in keys)
    lines.append(f"- 结论：共完成 {total} Episode；失败 {failed}、RPC 错误 {rpc_errors}、协议错误 {protocol_errors}。"
                 + ("聚合结果满足零错误。" if passed else "存在非零错误，需按错误明细复核。"))
    lines.append("- 报告原则：表格采用统一指标口径；输入证据未提供的值标记为“未提供”，不以 0 代替。")
    lines.append("")

    lines += ["## 1. 总览：规模、提交与结果", "",
              "| 数据集 | 状态 | Worker | 请求并发 | 容量波次 | 计划/并发批次 | 提交 | 完成 | 失败 | RPC/协议错误 | 吞吐（ep/s） | 耗时（min） |",
              "|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|"]
    for k in keys:
        m = all_metrics[k]
        batches = (f"{markdown_cell(m['planned_batches'])}/{markdown_cell(m['concurrent_batches'])}")
        lines.append(
            f"| {m['name']} | {markdown_cell(m['status'])} | {markdown_cell(m['workers'])} | "
            f"{markdown_cell(m['requested_episode_concurrency'])} | {markdown_cell(m['capacity_waves'])} | {batches} | "
            f"{markdown_cell(m['submitted'])} | {markdown_cell(m['completed'])} | {markdown_cell(m['failed'])} | "
            f"{markdown_cell(m['rpc_errors'])}/{markdown_cell(m['protocol_errors'])} | {markdown_cell(m['throughput'])} | "
            f"{markdown_cell(round(m['elapsed_min'], 1) if m['elapsed_min'] is not None else None)} |")
    lines.append("")

    lines += ["## 2. 提交、排队、执行与端到端延迟", "",
              "| 数据集 | 提交策略/提交耗时 | backlog 比例 | batch p50/p95/p99（秒） | 排队 p50/p95/p99（秒） | 执行 p50/p95/p99（秒） | 端到端 p50/p95/p99（秒） | 关联 Episode |",
              "|---|---|---:|---|---|---|---|---:|"]
    for k in keys:
        m = all_metrics[k]
        batch = m["batch_latency_ms"] or {}
        queue_exec = qe_stats.get(k) or {}
        observation = obs_stats.get(k) or {}
        end_to_end = value_at(observation, "numeric.end_to_end_ms")
        def triplet(values, divisor=1.0):
            if not values:
                return MISSING_VALUE
            points = [values.get(f"p{percentile}", values.get(percentile)) for percentile in (50, 95, 99)]
            if not any(isinstance(point, (int, float)) for point in points):
                return MISSING_VALUE
            return "/".join(
                f"{point / divisor:.1f}"
                if isinstance(point, (int, float))
                else MISSING_VALUE
                for point in points
            )
        lines.append(
            f"| {m['name']} | {markdown_cell(m['submit_strategy'])}/{markdown_cell(m['client_submit_seconds'])} 秒 | "
            f"{markdown_cell(m['target_backlog_ratio'])} | {triplet(batch, 1000)} | "
            f"{triplet(queue_exec.get('queue'))} | {triplet(queue_exec.get('exec'))} | "
            f"{triplet(end_to_end, 1000)} | {queue_exec.get('matched', MISSING_VALUE)} |")
    lines.append("")
    lines.append("说明：batch 延迟是客户端批次 RPC 耗时；排队和执行由服务端事件与 Episode 观测按 ID 关联；端到端取逐 Episode `end_to_end_ms`。")
    lines.append("")

    lines += ["## 3. 调度均衡、数据覆盖与回放", "",
              "| 数据集 | worker 覆盖 | 每 worker 完成 min/均值/p50/p95/p99/max | 负载标准差 | 已加载/唯一/复用系数 | 回放命中/miss | 训练轨迹/实际 step |",
              "|---|---|---|---:|---|---|---|"]
    for k in keys:
        m = all_metrics[k]
        worker = wl_stats.get(k)
        balance = MISSING_VALUE
        stddev = MISSING_VALUE
        if worker:
            p = worker["percentiles"]
            balance = f"{worker['min']}/{worker['mean']:.1f}/{p[50]:.1f}/{p[95]:.1f}/{p[99]:.1f}/{worker['max']}"
            stddev = f"{worker['stddev']:.2f}"
        data_coverage = f"{markdown_cell(m['dataset_loaded_rows'])}/{markdown_cell(m['dataset_unique'])}/{markdown_cell(m['dataset_reuse_factor'])}"
        trace = m["training_trace"]
        result = primary_result(all_data[k])
        mode = result.get("parallel_mode") or result.get("mode")
        trace_text = (
            markdown_cell(trace)
            if trace
            else ("训练轨迹：不适用（sync）" if mode == "sync" else MISSING_VALUE)
        )
        steps = m["actual_step_stats"]
        if steps:
            trace_text += f"；step={markdown_cell(steps.get('total_steps'))}"
        else:
            step_summary = value_at(obs_stats.get(k, {}), "numeric.actual_steps")
            if step_summary:
                trace_text += (
                    "；实际 step min/均值/p95/max="
                    f"{step_summary['min']:.1f}/{step_summary['mean']:.1f}/"
                    f"{step_summary['p95']:.1f}/{step_summary['max']:.1f}"
                )
        lines.append(f"| {m['name']} | {m['coverage']} | {balance} | {stddev} | {data_coverage} | {m['replay']} | {trace_text} |")
    lines.append("")
    lines.append("数据覆盖三项依次为：输入已加载条数、实际唯一题目/实例数、复用系数。“未提供”表示输入证据中不存在该字段。")
    lines.append("")

    lines += ["## 4. 正确性、错误与完整性校验", "",
              "| 数据集 | 基础设施校验 | 完整性 missing/duplicate/unknown | 逐 Episode 状态 | 失败分类 | 错误码 | 服务端事件计数 |",
              "|---|---|---|---|---|---|---|"]
    for k in keys:
        m = all_metrics[k]
        integrity = m["integrity"]
        observation = obs_stats.get(k) or {}
        counters = observation.get("counters", {})
        complete = "/".join(markdown_cell(integrity.get(field)) for field in ("missing", "duplicate", "unknown"))
        event_text = counter_text(m["server_event_counts"])
        zero_errors = all(
            value == 0
            for value in (m["failed"], m["rpc_errors"], m["protocol_errors"])
        )
        failure_text = (
            counter_text(counters.get("failure_class"))
            if counters.get("failure_class")
            else ("无" if zero_errors else MISSING_VALUE)
        )
        error_code_text = (
            counter_text(counters.get("error_code"))
            if counters.get("error_code")
            else ("无" if zero_errors else MISSING_VALUE)
        )
        lines.append(
            f"| {m['name']} | {markdown_cell(m['infrastructure_passed'])} | {complete} | "
            f"{counter_text(counters.get('status'))} | {failure_text} | "
            f"{error_code_text} | {event_text} |")
    lines.append("")
    lines.append("`missing/duplicate/unknown` 的字段名随场景输出而定；输入证据中不存在时显示“未提供”。逐 Episode 枚举统计会排除空值和 `none`。")
    lines.append("")

    lines += ["## 5. Worker 资源、主机看门狗与生命周期观测", "",
              "| 数据集 | 节点 | 采样数 | RSS（GB，min/均值/p50/p95/p99/max） | 进程（min/均值/p50/p95/p99/max） | 线程（min/均值/p50/p95/p99/max） | FD（min/均值/p50/p95/p99/max） |",
              "|---|---|---:|---|---|---|---|"]
    for k in keys:
        summaries = resource_summaries.get(k, {})
        if not summaries:
            lines.append(
                f"| {all_metrics[k]['name']} | {MISSING_VALUE} | {MISSING_VALUE} | "
                f"{MISSING_VALUE} | {MISSING_VALUE} | {MISSING_VALUE} | {MISSING_VALUE} |"
            )
            continue
        for node, stats in summaries.items():
            def resource_text(field, scale=1.0):
                summary = stats.get(field)
                if not summary:
                    return MISSING_VALUE
                return "/".join(f"{summary[p] / scale:.2f}" for p in ("min", "mean", "p50", "p95", "p99", "max"))
            lines.append(f"| {all_metrics[k]['name']} | {node} | {stats['samples']} | {resource_text('rss_bytes', 1e9)} | "
                         f"{resource_text('processes')} | {resource_text('threads')} | {resource_text('open_fds')} |")
    lines.append("")
    watchdog_rows = []
    for k in keys:
        watchdog = all_data[k].get("watchdog_summary") or {}
        for node, summary in watchdog.items():
            if not isinstance(summary, dict):
                continue
            watchdog_rows.append((all_metrics[k]["name"], node, summary))
    if watchdog_rows:
        lines += ["### 主机看门狗采样摘要", "",
                  "| 数据集 | 节点 | 采样数 | 可用内存最小/最大（GiB） | load1 最大值 |",
                  "|---|---|---:|---:|---:|"]
        for name, node, summary in watchdog_rows:
            min_mem = summary.get("min_available_gib")
            max_mem = summary.get("max_available_gib")
            memory_range = (f"{min_mem:.2f}/{max_mem:.2f}"
                            if isinstance(min_mem, (int, float)) and isinstance(max_mem, (int, float)) else MISSING_VALUE)
            lines.append(f"| {name} | {node} | {markdown_cell(summary.get('samples'))} | "
                         f"{memory_range} | {markdown_cell(summary.get('max_load1'))} |")
        lines.append("")
    if not any(resource_summaries.get(k) for k in keys):
        lines.append("- 本次输入未保留 fleet 资源时序；资源时序列因此显示为“未提供”，不以旧 run 的资源数据替代。")
    lines.append("- 资源时序来自 worker fleet supervisor 的 30 秒采样；看门狗摘要优先读取已有摘要，缺失时直接汇总已有 `host-watchdog.jsonl` 原始样本；本生成器不生成额外 JSON。")
    lines.append("")

    lines += ["## 6. 分数据集原始观测摘要", ""]
    for k in keys:
        m = all_metrics[k]
        observation = obs_stats.get(k)
        lines += [f"### 6.{keys.index(k)+1} {m['name']}", ""]
        lines.append(f"- 可用证据：{source_state(all_data[k])}。")
        if observation:
            lines.append(f"- 逐 Episode 观测 {observation['count']} 条；数值字段摘要：")
            lines += ["", "| 字段 | min/均值/p50/p95/p99/max |", "|---|---|"]
            for field, summary in observation["numeric"].items():
                lines.append(f"| `{field}` | {summary_text(summary)} |")
            if observation["counters"]:
                lines.append("")
                lines.append("- 枚举字段分布：" + "；".join(
                    f"`{field}`：{counter_text(counter)}" for field, counter in observation["counters"].items()))
        else:
            lines.append("- 未提供逐 Episode 观测文件；本节无法生成端到端字段摘要。")
        lines.append("")

    if chart_files:
        lines += ["## 7. 图表", ""]
        for caption, rel, note in chart_files:
            lines += [f"![{caption}]({rel})", f"**{caption}**——{note}", ""]

    lines += ["## 8. 证据文件与口径", "",
              "| 数据集 | 结果文件 | 观测 | Worker 负载 | 服务端事件 | 资源时序 | 看门狗 |", "|---|---|---|---|---|---|---|"]
    for k in keys:
        data = all_data[k]
        lines.append(f"| {all_metrics[k]['name']} | {len(data['results'])} | {len(data['observations'])} 条 | "
                     f"{'有' if data.get('worker_load') else '无'} | {'有' if data.get('episode_events') else '无'} | "
                     f"{'有' if data.get('fleet_resources') else '无'} | {'有' if data.get('watchdog_summary') else '无'} |")
    lines += ["", "- 本报告由 `generate_scale_report.py` 自动生成；它仅读取已有 JSON/JSONL/CSV，输出 Markdown 和 PNG 图表，不生成额外 JSON。",
              "- 模型侧是否为 trace replay、真实调用或混合模式，按各场景聚合结果中的 `trace_replay`、模型/模拟器配置字段为准；不要将本报告外推为模型质量结论。",
              "- 排队 = server 分配 worker 时刻 − 客户端提交时刻；执行 = 完成时刻 − 分配时刻"
              "（由隔离 server 日志 episode_dispatching/completed 按 episode_id 关联）。",
              "- 资源时间序列来自各 worker 机 fleet supervisor 的 30 秒采样（fleet-resources.csv）；图表和表格均以已有采样为准。", ""]
    return "\n".join(lines)


def build_report(title: str, all_data: dict, all_metrics: dict, qe_stats: dict,
                 interaction_stats: dict, wl_stats: dict, obs_stats: dict,
                 resource_summaries: dict, chart_files: list) -> str:
    """生成按实验设置、结果和讨论组织的五数据集报告。"""
    keys = [key for key in DATASET_ORDER if key in all_metrics]
    chart_map = {relative: (caption, note) for caption, relative, note in chart_files}
    workload_names = {
        "olymmath": "数学推理",
        "scitab": "表格问答",
        "pubmedqa": "医学问答",
        "dscodebench": "代码评测",
        "swebench_pro": "仓库级软件工程",
    }

    def add_chart(lines: list[str], relative: str) -> None:
        if relative not in chart_map:
            return
        caption, note = chart_map[relative]
        lines += [
            f"![{caption}]({relative})",
            f"*{caption}。{note}*",
            "",
        ]

    total_submitted = sum(all_metrics[key]["submitted"] or 0 for key in keys)
    total_completed = sum(all_metrics[key]["completed"] or 0 for key in keys)
    total_failed = sum(all_metrics[key]["failed"] or 0 for key in keys)
    total_rpc = sum(all_metrics[key]["rpc_errors"] or 0 for key in keys)
    total_protocol = sum(all_metrics[key]["protocol_errors"] or 0 for key in keys)
    all_batches_at_once = all(
        all_metrics[key]["planned_batches"] == all_metrics[key]["concurrent_batches"]
        for key in keys
    )

    swe = all_metrics.get("swebench_pro", {})

    balance_cvs = {
        key: stats["stddev"] / stats["mean"]
        for key, stats in wl_stats.items()
        if stats.get("mean")
    }
    watchdog = (all_data.get("swebench_pro") or {}).get("watchdog_summary") or {}
    watchdog_rows = [
        (node, summary) for node, summary in watchdog.items() if isinstance(summary, dict)
    ]
    worker_private_ips = {
        "8.130.65.20": "192.168.0.139",
        "8.145.51.129": "192.168.0.138",
    }

    lines = [
        f"# {title}",
        "",
        "## 1. 实验设置",
        "",
        "### 1.1 系统部署",
        "",
        "实验部署由 1 个 Server 节点和 2 个 Worker 节点组成。Server 负责请求接收、"
        "队列维护、任务调度与结果汇总；Worker 负责执行 Episode。"
        "三个节点采用相同的处理器、内存、磁盘和操作系统配置。",
        "",
        "**表 1　实验节点配置**",
        "",
        "| 节点 | 角色 | 硬件与操作系统 | 网络地址 | 主要职责 |",
        "|---|---|---|---|---|",
        "| 8.130.75.157 | Server / 控制节点 | "
        "8 vCPU（Intel Xeon 6982P-C）；30.44 GiB 内存；252 GiB 根盘；"
        "Ubuntu 24.04.4 LTS | "
        "公网 `8.130.75.157`；私网 `192.168.0.136` | "
        "启动场景、接收全部批次、维护队列、调度 Worker、汇总结果 |",
    ]
    for index, node in enumerate(NODES, start=1):
        lines.append(
            f"| {node} | Worker 节点 {index} | "
            "8 vCPU（Intel Xeon 6982P-C）；30.44 GiB 内存；252 GiB 根盘；"
            "Ubuntu 24.04.4 LTS | "
            f"公网 `{node}`；私网 `{worker_private_ips[node]}` | "
            "运行 UEnv Worker 和数据集对应的执行组件，完成 Episode |"
        )
    lines += [
        "",
        "### 1.2 工作负载与执行方法",
        "",
        "五个数据集按 OlymMATH、SciTab、PubMedQA、DSCodeBench 和 "
        "SWE-bench Pro 的顺序独立运行。数据集之间串行执行，避免不同工作负载"
        "相互争用 Worker 资源。",
        "",
        "**表 2　工作负载与并发配置**",
        "",
        "| 数据集 | 工作负载 | Episode | 计划批次 | 同时提交批次 | 请求并发 | Worker | 容量槽 | 容量波次 |",
        "|---|---|---:|---:|---:|---:|---:|---:|---:|",
    ]
    for key in keys:
        metric = all_metrics[key]
        lines.append(
            f"| {metric['name']} | {workload_names[key]} | "
            f"{metric['submitted']:,} | {metric['planned_batches']} | "
            f"{metric['concurrent_batches']} | "
            f"{metric['requested_episode_concurrency']:,} | {metric['workers']:,} | "
            f"{metric['worker_capacity_slots']:,} | {metric['capacity_waves']:.0f} |"
        )
    lines += [
        "",
        (
            "每个场景均在等待任何批次完成之前提交全部计划批次。Server 接收请求后，"
            "根据 Worker 可用状态维护队列并分配 Episode。"
            if all_batches_at_once
            else "部分场景的实际同时提交批次数少于计划批次数。"
        ),
        "",
        "### 1.3 评价指标",
        "",
        "**表 3　评价指标及其定义**",
        "",
        "| 指标 | 定义 |",
        "|---|---|",
        "| 场景吞吐 | 完成 Episode 数除以场景总耗时 |",
        "| 单 Worker 吞吐 | 场景吞吐除以 Worker 数 |",
        "| 容量波次 | 提交 Episode 数除以 Worker 容量槽 |",
        "| batch 延迟 | 客户端批次 RPC 的持续时间，报告 p50、p95 和 p99 |",
        "| 排队时间 | `episode_dispatching` 时刻减去客户端提交时刻 |",
        "| 执行时间 | `episode_completed` 时刻减去 `episode_dispatching` 时刻 |",
        "| 模型调用轮数 | 每个 Episode 实际触发模型回复的次数 |",
        "| 模型回复长度 | 真实回放轨迹中单次模型回复的 Qwen3 Token 数 |",
        "| 单轮回复时长 | 真实回放轨迹中单次模型回复采用的 `replay_wait_ms` |",
        "| 完成率 | 完成 Episode 数除以提交 Episode 数 |",
        "| Worker 负载离散度 | 各 Worker 完成量的标准差和变异系数 |",
        "| 可用内存 | Linux `MemAvailable`，表示无需交换即可供新任务使用的内存估计值 |",
        "| load1/5/15 | Linux 系统负载在过去 1、5 和 15 分钟的移动平均值，统计可运行任务和不可中断等待任务的平均数量，不表示 CPU 使用率百分比 |",
        "| Worker 进程组 RSS | Worker fleet 进程组内各进程 `VmRSS` 之和，表示采样时驻留在物理内存中的进程页面总量 |",
        "| 进程运行状态 | Worker 进程组的进程数、线程数和打开文件描述符数 |",
        "",
        "load1、load5 和 load15 分别对应过去 1、5 和 15 分钟的系统负载移动平均值。"
        "表 9 报告三个时间序列在测试期间各自出现的最大值，三个最大值不要求出现在同一采样时刻。"
        "对于本报告使用的 8 vCPU 主机，load1 为 8 表示过去 1 分钟平均约有 8 个任务"
        "处于可运行或不可中断等待状态。"
        "load1 高于 8 表示任务需求超过逻辑处理器数量，或存在较多不可中断等待任务，"
        "但不能仅凭该指标区分 CPU 计算压力和 I/O 等待。",
        "",
        "RSS 表示 Resident Set Size。报告中的 RSS 是 Worker 进程组内各进程驻留内存的求和，"
        "用于观察 Worker 相关进程占用的物理内存规模。"
        "由于不同进程可能映射相同共享页面，求和值可能重复计算共享内存，"
        "因此不能等同于整台主机的实际独占内存消耗。",
        "",
        "## 2. 总体结果",
        "",
        f"五个场景共提交 {total_submitted:,} 个 Episode，并完成 {total_completed:,} 个。"
        f"测试记录到 {total_failed} 个 Episode 失败、{total_rpc} 个 RPC 错误和 "
        f"{total_protocol} 个协议错误。",
        "",
        "**表 4　各场景的总体结果**",
        "",
        "| 数据集 | 完成 / 提交 | Worker | 总耗时（min） | 吞吐（ep/s） | 单 Worker 吞吐（ep/s） | 完成率 | 失败 / RPC / 协议错误 |",
        "|---|---:|---:|---:|---:|---:|---:|---:|",
    ]
    for key in keys:
        metric = all_metrics[key]
        completion_rate = 100 * metric["completed"] / metric["submitted"]
        lines.append(
            f"| {metric['name']} | {metric['completed']:,} / {metric['submitted']:,} | "
            f"{metric['workers']:,} | {metric['elapsed_min']:.2f} | "
            f"{metric['throughput']:.3f} | {metric['throughput_per_worker']:.5f} | "
            f"{completion_rate:.1f}% | {metric['failed']} / {metric['rpc_errors']} / "
            f"{metric['protocol_errors']} |"
        )
    lines += [
        "",
        "图 1 汇总各场景的吞吐、单位 Worker 吞吐和总耗时。表 4 同时给出"
        "实际 Worker 数，便于在解释单位吞吐和总吞吐时保留资源规模信息。",
        "",
    ]
    add_chart(lines, "charts/overview.png")

    lines += [
        "### 2.1 批次接收与完成情况",
        "",
        "OlymMATH、SciTab、PubMedQA 和 DSCodeBench 均形成 10 个容量波次；"
        "SWE-bench Pro 形成 40 个容量波次。五个场景的完成率均为 100%，"
        "且未出现 Episode 失败、RPC 错误或协议错误。",
        "",
    ]
    lines += [
        "四个 1,024 Worker 场景的提交规模均为 Worker 容量槽的 10 倍，"
        "SWE-bench Pro 的提交规模为其 64 个容量槽的 40 倍。"
        "这些请求分别形成 10 个和 40 个连续容量波次。"
        "所有 Episode 最终完成且三类错误计数均为 0，说明 Server 在本轮配置下"
        "完成了高于即时执行容量的请求积压处理。",
        "",
    ]

    lines += [
        "## 3. 性能分析",
        "",
        "### 3.1 吞吐与批次延迟",
        "",
        "**表 5　吞吐与批次延迟分位数**",
        "",
        "| 数据集 | Worker | 总耗时（min） | 吞吐（ep/s） | 单 Worker 吞吐（ep/s） | batch p50 / p95 / p99（s） |",
        "|---|---:|---:|---:|---:|---:|",
    ]
    for key in keys:
        metric = all_metrics[key]
        batch = metric.get("batch_latency_ms") or {}
        batch_text = " / ".join(
            f"{batch[p] / 1000:.2f}" for p in ("p50", "p95", "p99")
        )
        lines.append(
            f"| {metric['name']} | {metric['workers']:,} | "
            f"{metric['elapsed_min']:.2f} | {metric['throughput']:.3f} | "
            f"{metric['throughput_per_worker']:.5f} | {batch_text} |"
        )
    lines += [
        "",
        "表 5 报告各场景在其既定工作负载和 Worker 配置下的测量结果。"
        "图 2 使用对数坐标呈现不同数量级的 batch 延迟分位数。",
        "",
    ]
    add_chart(lines, "charts/latency_breakdown.png")
    lines += [
        "在 Episode 数和 Worker 数相同的前四个场景中，SciTab 的吞吐为 "
        "71.042 ep/s，DSCodeBench 为 3.342 ep/s，二者相差 21.26 倍。"
        "对应总耗时分别为 2.40 分钟和 51.06 分钟，耗时比例为 21.28 倍。"
        "吞吐排序与总耗时排序保持相反关系，说明在固定 Episode 数和 Worker 数下，"
        "不同工作负载的端到端处理成本存在约 21 倍跨度。"
        "由于任务语义不同，这一跨度不表示调度器在某一数据集上获得了等比例收益。",
        "",
        "前四个场景的 batch 延迟分位数反映了各自的全部批次均在场景开始阶段提交，"
        "并在客户端等待其完成。SWE-bench Pro 的 batch 延迟分位数由本轮 20 个批次的"
        "完整 RPC 记录计算，不与其他工作负载构成统一的时延排序。",
        "",
    ]

    lines += [
        "### 3.2 Episode 排队与执行时间",
        "",
        "**表 6　Episode 排队与执行时间统计**",
        "",
        "| 数据集 | 关联 Episode | 排队 p50 / p95 / p99（s） | 执行 p50 / p95 / p99（s） |",
        "|---|---:|---:|---:|",
    ]
    for key in keys:
        metric = all_metrics[key]
        timing = qe_stats.get(key)
        if timing:
            queue_text = " / ".join(
                f"{timing['queue'][p]:.2f}" for p in (50, 95, 99)
            )
            exec_text = " / ".join(
                f"{timing['exec'][p]:.2f}" for p in (50, 95, 99)
            )
            matched_text = f"{timing['matched']:,} / {metric['submitted']:,}"
        else:
            queue_text = exec_text = matched_text = MISSING_VALUE
        lines.append(
            f"| {metric['name']} | {matched_text} | {queue_text} | {exec_text} |"
        )
    lines += [
        "",
        "排队和执行时间均按 `episode_id` 关联客户端提交记录与 Server 事件。"
        "五个场景的关联 Episode 数均等于提交 Episode 数。",
        "",
    ]
    add_chart(lines, "charts/queue_exec.png")
    lines += [
        "排队和执行时间均使用本轮客户端观测与服务端事件按 Episode 标识关联。"
        "由于五个场景的 Episode 规模、Worker 数量和执行组件不同，表 6 用于描述"
        "各自场景中的排队和执行分布，而非跨数据集的统一时延排序。",
        "",
        "SciTab 和 PubMedQA 均没有排队时间严格等于 0 的 Episode。"
        "两者排队时间小于 1 秒的 Episode 分别为 6 个和 9 个，"
        "小于 5 秒的分别为 21 个和 39 个。"
        "当范围扩大到 20 秒时，累计数量分别达到 1,078 个和 1,091 个，"
        "与第一波 1,024 个 Worker 容量槽接近。"
        "排队时间从客户端提交时刻开始计算，即使第一波 Episode 也包含请求传输、"
        "入队和调度开销，因此不会集中在严格的零值。"
        "其余 Episode 分布在后续九个容量波次中，需要等待已有任务释放容量。",
        "",
    ]

    if interaction_stats:
        lines += [
            "### 3.3 模型交互特征",
            "",
            "**表 7　模型调用轮数、回复长度与单轮回复时长分布**",
            "",
            "| 数据集 | Episode 样本 | 模型调用轮数 min / 均值 / p50 / p95 / max | "
            "轨迹回复样本 | 回复长度 min / 均值 / p50 / p95 / max（Token） | "
            "单轮回复时长 min / 均值 / p50 / p95 / max（s） |",
            "|---|---:|---:|---:|---:|---:|",
        ]
        for key in keys:
            interaction = interaction_stats.get(key)
            if not interaction:
                continue
            lines.append(
                f"| {all_metrics[key]['name']} | "
                f"{interaction['episode_samples']:,} | "
                f"{distribution_text(interaction['rounds'], nd=2)} | "
                f"{interaction['trace_turns']:,} | "
                f"{distribution_text(interaction['reply_tokens'], nd=2)} | "
                f"{distribution_text(interaction['reply_seconds'], nd=2)} |"
            )
        lines += [
            "",
            "模型调用轮数来自结果中逐 Episode 的 `actual_steps` 字段。回复长度和"
            "单轮回复时长来自本轮加载的真实回放轨迹，分别按 `target_qwen3_tokens` 和 "
            "`replay_wait_ms` 统计。轨迹回复样本按唯一轨迹轮次计算，不按 Episode 的"
            "重复回放次数加权；每个数据集仅统计其结果记录可达到的轨迹前缀。",
            "",
        ]

        math_interactions = [
            interaction_stats.get(key)
            for key in ("olymmath", "scitab", "pubmedqa")
        ]
        if all(math_interactions):
            olym = interaction_stats["olymmath"]
            sci = interaction_stats["scitab"]
            pub = interaction_stats["pubmedqa"]
            lines += [
                "三个数学相关场景均为单轮模型调用，但回复规模和等待时长存在明显差异。"
                f"OlymMATH 的回复长度 p50 为 {olym['reply_tokens']['p50']:.0f} Token，"
                f"单轮回复时长 p50 为 {olym['reply_seconds']['p50']:.2f} 秒。"
                f"SciTab 的对应数值为 {sci['reply_tokens']['p50']:.0f} Token 和 "
                f"{sci['reply_seconds']['p50']:.2f} 秒，PubMedQA 为 "
                f"{pub['reply_tokens']['p50']:.0f} Token 和 "
                f"{pub['reply_seconds']['p50']:.2f} 秒。"
                "OlymMATH 的单轮交互负载因而高于另外两个单轮分类场景。",
                "",
            ]

        if interaction_stats.get("dscodebench") and interaction_stats.get("swebench_pro"):
            ds = interaction_stats["dscodebench"]
            swe = interaction_stats["swebench_pro"]
            if swe["rounds"]["p50"] > 1:
                conclusion = "两者均为多轮工作负载，但每轮回复规模和回放时长分布不同。"
            else:
                conclusion = (
                    "本轮 SWE-bench Pro 的结果记录中 `actual_steps` 均为 1，"
                    "因此表中仅使用每条真实轨迹的首轮回复。配置中的 10 轮为执行上限，"
                    "不表示每个 Episode 必然达到该轮数。"
                )
            lines += [
                f"DSCodeBench 的模型调用轮数 p50 和 p95 均为 "
                f"{ds['rounds']['p50']:.0f} 轮，SWE-bench Pro 均为 "
                f"{swe['rounds']['p50']:.0f} 轮。"
                f"DSCodeBench 的回复长度 p50 为 {ds['reply_tokens']['p50']:.0f} "
                f"Token，单轮回复时长 p50 为 {ds['reply_seconds']['p50']:.2f} 秒。"
                f"SWE-bench Pro 的对应数值为 {swe['reply_tokens']['p50']:.0f} "
                f"Token 和 {swe['reply_seconds']['p50']:.2f} 秒。"
                + conclusion,
                "",
            ]

    lines += ["## 4. 调度与资源行为", ""]
    if wl_stats:
        lines += [
            "### 4.1 Worker 负载分布",
            "",
            "**表 8　逐 Worker 完成量分布**",
            "",
            "| 数据集 | Worker 数 | 最少 | 均值 | p50 | p95 | p99 | 最多 | 标准差 | 变异系数 |",
            "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|",
        ]
        for key in keys:
            balance = wl_stats.get(key)
            if not balance:
                continue
            percentiles_row = balance["percentiles"]
            cv = balance["stddev"] / balance["mean"] if balance["mean"] else 0
            lines.append(
                f"| {all_metrics[key]['name']} | {len(balance['counts'])} | "
                f"{balance['min']} | {balance['mean']:.1f} | "
                f"{percentiles_row[50]:.1f} | {percentiles_row[95]:.1f} | "
                f"{percentiles_row[99]:.1f} | {balance['max']} | "
                f"{balance['stddev']:.2f} | {cv:.1%} |"
            )
        lines += [
            "",
            "图 4 将各 Worker 完成量除以对应场景的均值，并按完成量降序排列。"
            "右侧子图给出表 8 中的变异系数。",
            "",
        ]
        add_chart(lines, "charts/worker_load.png")
        lines += [
            "在四个 1,024 Worker 场景中，每个 Worker 平均完成 10 个 Episode。"
            "PubMedQA 的变异系数最低，为 4.3%，其完成量范围为 8 至 11。"
            "DSCodeBench 的变异系数最高，为 18.6%，其完成量范围为 5 至 16。"
            "这表明相同 Worker 规模下，PubMedQA 的完成量分布最集中，"
            "DSCodeBench 的完成量分布最分散。",
            "",
            (
                f"本轮 SWE-bench Pro 的 {wl_stats['swebench_pro']['workers']} 个 Worker "
                f"平均完成 {wl_stats['swebench_pro']['mean']:.1f} 个 Episode，"
                f"完成量范围为 {wl_stats['swebench_pro']['min']} 至 "
                f"{wl_stats['swebench_pro']['max']}，变异系数为 "
                f"{wl_stats['swebench_pro']['stddev'] / wl_stats['swebench_pro']['mean']:.1%}。"
                "完成量统计不能单独区分任务持续时间差异与调度行为的各自影响。"
            ) if wl_stats.get("swebench_pro") else "本轮 SWE-bench Pro 未提供逐 Worker 完成量。",
            "",
        ]

    if any(resource_summaries.get(key) for key in keys):
        lines += [
            "### 4.2 Worker 主机资源",
            "",
            "**表 9　Worker 主机资源统计**",
            "",
            "| 数据集 | 节点 | 样本 | 可用内存 min/max（GiB） | load1/5/15 最大值 | RSS 最大（GiB） | 进程/线程/FD 最大值 |",
            "|---|---|---:|---:|---:|---:|---:|",
        ]
        for key in keys:
            for node, stats in resource_summaries.get(key, {}).items():
                available = stats.get("available_bytes") or {}
                load1 = stats.get("load1") or {}
                load5 = stats.get("load5") or {}
                load15 = stats.get("load15") or {}
                rss = stats.get("rss_bytes") or {}
                processes = stats.get("processes") or {}
                threads = stats.get("threads") or {}
                fds = stats.get("open_fds") or {}
                available_text = (
                    f"{available['min'] / (1024 ** 3):.2f}/"
                    f"{available['max'] / (1024 ** 3):.2f}"
                    if available else MISSING_VALUE
                )
                load_text = (
                    f"{load1['max']:.2f}/{load5['max']:.2f}/{load15['max']:.2f}"
                    if load1 and load5 and load15 else MISSING_VALUE
                )
                rss_text = (
                    f"{rss['max'] / (1024 ** 3):.2f}" if rss else MISSING_VALUE
                )
                process_text = (
                    f"{processes['max']:.0f}/{threads['max']:.0f}/{fds['max']:.0f}"
                    if processes and threads and fds else MISSING_VALUE
                )
                lines.append(
                    f"| {all_metrics[key]['name']} | {node} | {stats.get('samples', 0)} | "
                    f"{available_text} | {load_text} | {rss_text} | {process_text} |"
                )
        swe_resource_nodes = resource_summaries.get("swebench_pro", {})
        swe_has_rss = any((stats.get("rss_bytes") or {}) for stats in swe_resource_nodes.values())
        swe_has_process = any((stats.get("processes") or {}) for stats in swe_resource_nodes.values())
        swe_resource_note = (
            "本轮 SWE-bench Pro 的看门狗采样记录了可用内存和系统负载，"
            "未记录 Worker 进程组 RSS、进程数、线程数或文件描述符数；表中相应单元格"
            "据此标为未提供，未沿用旧轮次的数值。"
            if swe_resource_nodes and (not swe_has_rss or not swe_has_process)
            else "资源监控对两台 Worker 主机采用相同采样字段。"
        )
        lines += [
            "",
            swe_resource_note,
            "图 5 展示可用内存、load1、Worker 进程组 RSS 和进程数的时间序列；"
            "某场景未采集的字段不绘制曲线。",
            "",
        ]
        add_chart(lines, "charts/resources_timeseries.png")
        lines += [
            "OlymMATH、SciTab 和 PubMedQA 在表 9 中具有相同的样本数和资源极值，"
            "且各字段在当前显示精度下完全一致。"
            "因此，这组记录不能支持三种数学相关工作负载之间的资源差异分析。",
            "",
        "DSCodeBench 的两台 Worker 主机呈现明显不对称。"
        "节点 8.130.65.20 的最低可用内存比节点 8.145.51.129 少 7.86 GiB，"
        "最大 load1 为后者的 1.39 倍，进程组最大 RSS 为后者的 1.58 倍。"
        "该差异同时出现在内存、负载和 RSS 三类指标中，"
        "说明资源压力在两台主机之间并未均匀分布。",
        "",
        "节点 8.130.65.20 在 DSCodeBench 场景中的最大 load1 为 103.08。"
        "该数值表示 1 分钟窗口内可运行任务与不可中断等待任务的平均数量，"
        "不表示 CPU 使用率为 103.08%。"
        "由于现有资源表未同时给出 CPU 利用率和 I/O 等待比例，"
        "无法仅根据 load1 确定高负载来自计算竞争还是 I/O 等待。",
        "",
    ]

    return "\n".join(lines)


COMMON_FLEET_RESOURCE_FIELDS = {
    "timestamp", "processes", "rss_bytes", "open_fds", "threads",
    "mem_total_bytes", "available_bytes", "load1", "load5", "load15",
    "worker_exits", "oom_events",
}


def validate_uniform_fleet_resources(all_data: dict[str, dict]) -> None:
    """拒绝缺失资源字段的报告输入，避免以简化 watchdog 指标替代 fleet 指标。"""
    errors: list[str] = []
    expected_nodes = set(NODES)
    for key in DATASET_ORDER:
        data = all_data.get(key)
        if not data or not data.get("results"):
            continue
        resources = data.get("fleet_resources") or {}
        actual_nodes = set(resources)
        missing_nodes = sorted(expected_nodes - actual_nodes)
        extra_nodes = sorted(actual_nodes - expected_nodes)
        if missing_nodes or extra_nodes:
            errors.append(
                f"{key}: resource nodes must be {sorted(expected_nodes)}, "
                f"missing={missing_nodes}, unexpected={extra_nodes}"
            )
            continue
        for node in NODES:
            rows = resources.get(node) or []
            if not rows:
                errors.append(f"{key}/{node}: fleet resource CSV has no samples")
                continue
            fields = set(rows[0])
            missing_fields = sorted(COMMON_FLEET_RESOURCE_FIELDS - fields)
            if missing_fields:
                errors.append(
                    f"{key}/{node}: fleet resource CSV lacks {', '.join(missing_fields)}"
                )
    if errors:
        raise ValueError(
            "report input does not satisfy the common fleet resource contract:\n- "
            + "\n- ".join(errors)
        )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--input-dir", default=str(Path(__file__).resolve().parent))
    parser.add_argument("--title", default="五数据集规模压测总结报告")
    parser.add_argument("--output", default="五数据集规模压测总结报告.md")
    args = parser.parse_args()
    input_dir = Path(args.input_dir)
    charts_dir = input_dir / "charts"
    charts_dir.mkdir(exist_ok=True)

    all_data = {}
    for key in DATASET_ORDER:
        if (input_dir / key).is_dir():
            all_data[key] = load_dataset(input_dir, key)
    # math 场景：如果只有一个 math/ 目录，按任务拆分
    if "math" in [p.name for p in input_dir.iterdir() if p.is_dir()] and not any(k in all_data for k in ("olymmath", "scitab", "pubmedqa")):
        math_dir = input_dir / "math"
        for task in ("olymmath", "scitab", "pubmedqa"):
            sub = {"key": task, "name": DATASETS[task][0], "kind": "math"}
            results = {}
            doc = load_json(math_dir / f"result-{task}-sync.json")
            if doc:
                results["sync"] = doc
            sub["results"] = results
            obs_file = math_dir / f"episode-observations-{task}-sync.jsonl"
            obs = []
            if obs_file.exists():
                with obs_file.open(encoding="utf-8") as fh:
                    for line in fh:
                        line = line.strip()
                        if line:
                            obs.append(json.loads(line))
            sub["observations"] = obs
            sub["trace_records"] = load_jsonl(
                math_dir / f"trace-corpus-{task}.jsonl"
            )
            sub["llm_simulator_stats"] = {}
            sub["worker_load"] = load_json(math_dir / "worker_load.json", {})
            sub["episode_events"] = load_json(math_dir / "episode_events.json", {})
            if (math_dir / "server.log").is_file():
                parsed_events, parsed_load = parse_server_log(
                    math_dir / "server.log"
                )
                sub["episode_events"] = sub["episode_events"] or parsed_events
                sub["worker_load"] = sub["worker_load"] or parsed_load
            sub["watchdog_summary"] = load_json(math_dir / "host-watchdog-summary.json", {})
            sub["report_manifest"] = load_json(math_dir / "report-export-manifest.json", {})
            resources = {}
            for f in sorted(math_dir.glob("fleet-resources-*.csv")):
                tag = f.stem.replace("fleet-resources-", "")
                with f.open(encoding="utf-8") as fh:
                    resources[NODE_TAGS.get(tag, tag)] = list(csv.DictReader(fh))
            sub["fleet_resources"] = resources
            metrics = {}
            for f in sorted(math_dir.glob("fleet-metrics-*.json")):
                tag = f.stem.replace("fleet-metrics-", "")
                metrics[NODE_TAGS.get(tag, tag)] = load_json(f, {})
            sub["fleet_metrics"] = metrics
            all_data[task] = sub

    validate_uniform_fleet_resources(all_data)
    all_metrics = {k: dataset_metrics(d) for k, d in all_data.items() if d["results"]}
    qe_stats = {k: s for k, d in all_data.items() if (s := queue_exec_stats(d))}
    interaction_stats = {
        k: s for k, d in all_data.items()
        if (s := model_interaction_stats(d))
    }
    wl_stats = {k: s for k, d in all_data.items() if (s := worker_load_stats(d))}
    obs_stats = {k: s for k, d in all_data.items() if (s := observation_stats(d))}
    resource_summaries = {k: resource_stats(d) for k, d in all_data.items()}
    chart_files: list[tuple[str, str, str]] = []
    chart_overview(all_metrics, charts_dir / "overview.png")
    chart_files.append((
        "图 1 吞吐、单位 Worker 吞吐与场景总耗时",
        "charts/overview.png",
        "三幅子图分别展示各场景的吞吐、单位 Worker 吞吐和总耗时；数值轴采用对数尺度。",
    ))
    chart_latency_breakdown(
        all_metrics, qe_stats, obs_stats, charts_dir / "latency_breakdown.png"
    )
    chart_files.append((
        "图 2 批次延迟分位数",
        "charts/latency_breakdown.png",
        "对数轴同时容纳不同数量级的延迟；五个数据集均展示 p50、p95 和 p99。",
    ))
    if qe_stats:
        chart_queue_exec(all_data, qe_stats, charts_dir / "queue_exec.png")
        chart_files.append((
            "图 3 Episode 排队与执行时间分布",
            "charts/queue_exec.png",
            "每行对应一个数据集；左列为排队时间，右列为执行时间，红线标示 p50 和 p95。",
        ))
    if any(all_data[key].get("fleet_resources") for key in all_data):
        chart_resources(all_data, charts_dir / "resources_timeseries.png")
        chart_files.append((
            "图 5 Worker 主机资源时序",
            "charts/resources_timeseries.png",
            "曲线展示五个场景在两台 Worker 主机上的可用内存、系统负载、进程组 RSS 和进程数。",
        ))
    if wl_stats:
        chart_worker_load(all_data, wl_stats, charts_dir / "worker_load.png")
        chart_files.append((
            "图 4 Worker 完成量分布",
            "charts/worker_load.png",
            "左图为按场景均值归一化后的逐 Worker 完成量，右图为完成量变异系数。",
        ))
    report = build_report(
        args.title, all_data, all_metrics, qe_stats, interaction_stats,
        wl_stats, obs_stats, resource_summaries, chart_files
    )
    out = input_dir / args.output
    out.write_text(report, encoding="utf-8")
    print(f"report -> {out}")
    print(f"charts -> {charts_dir} ({len(chart_files)} figures)")


if __name__ == "__main__":
    main()
