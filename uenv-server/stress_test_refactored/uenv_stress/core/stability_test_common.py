#!/usr/bin/env python3
"""正式稳定性套件的公共计算与校验工具。

这个文件实现稳定性验收中可独立测试的基础逻辑，包括配置校验、阶段到达率计算、计划时间生成、轨迹文件校验、轨迹延迟估计、Episode ledger、容量需求估算、CSV 写入和可用性区间分类。它不依赖 gRPC，因此这些规则可以脱离真实 Server 做单元测试。

实现逻辑是：load_config/validate_config 先确认验收配置完整；phase_rate、phase_duration、scheduled_offsets 和 iter_planned_times 按阶段生成投放计划；validate_trace_file 与 verify_manifest 校验冻结轨迹和清单；EpisodeLedger 负责把 Episode 的计划、启动、完成、失败和结果路径写成可恢复记录；required_capacity 根据到达率和 P95 时延估算所需容量；classify_availability 根据健康采样判断服务不可用和恢复区间。"""

from __future__ import annotations

import csv
import hashlib
import json
import math
import random
import time
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any, Iterable, Iterator


TASK_NAMES = ("dscodebench", "swebench_pro", "olymmath", "scitab", "pubmedqa")
PAIRED_TASK_NAMES = ("dscodebench", "olymmath", "scitab", "pubmedqa")
ARRIVAL_MODES = {"constant", "poisson", "batch"}
PHASE_MULTIPLIERS = {
    "reference": 1.0,
    "stability": 1.0,
    "pressure": 10.0,
    "capacity": 1.2,
    "burst": 4.0,
    "fault": 1.0,
}
TERMINAL_SUCCESS = {"completed", "success"}
TRACE_ID_KEYS = (
    "instance_id",
    "problem_id",
    "unique_id",
    "pmid",
    "qid",
    "task_id",
    "sample_id",
    "dataset_id",
    "question_id",
    "id",
)
LATENCY_REPLAY_STRATEGY = "frozen_replay_wait_ms"
def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def percentile(values: Iterable[float], q: float) -> float:
    ordered = sorted(float(value) for value in values)
    if not ordered:
        return 0.0
    position = (len(ordered) - 1) * q
    low = math.floor(position)
    high = math.ceil(position)
    if low == high:
        return ordered[low]
    return ordered[low] + (ordered[high] - ordered[low]) * (position - low)


def load_config(path: Path) -> dict[str, Any]:
    document = json.loads(path.read_text(encoding="utf-8"))
    validate_config(document)
    return document


def validate_config(document: dict[str, Any]) -> None:
    if int(document.get("schema_version", 0)) != 1:
        raise ValueError("stability config schema_version must be 1")
    if document.get("traces", {}).get("selection_strategy") != "paired_alternating_episode":
        raise ValueError(
            "stability traces.selection_strategy must be paired_alternating_episode"
        )
    tasks = document.get("tasks")
    if not isinstance(tasks, dict) or set(tasks) != set(TASK_NAMES):
        raise ValueError(f"tasks must be exactly {TASK_NAMES}")
    share_sum = sum(float(tasks[name]["allocation_share"]) for name in TASK_NAMES)
    expected_sum = float(document.get("load", {}).get("allocation_share_sum", 1.0))
    if not math.isclose(share_sum, expected_sum, rel_tol=0, abs_tol=1e-9):
        raise ValueError(f"allocation_share sum must be {expected_sum}, got {share_sum}")
    normalized_sum = 0.0
    for name in TASK_NAMES:
        task = tasks[name]
        standalone = float(task["standalone_rate_eps"])
        share = float(task["allocation_share"])
        target = float(task["target_rate_eps"])
        if standalone <= 0 or target <= 0 or share <= 0:
            raise ValueError(f"{name}: rates and allocation_share must be positive")
        if not math.isclose(target, standalone * share, rel_tol=0, abs_tol=1e-9):
            raise ValueError(f"{name}: target_rate_eps must equal standalone_rate_eps * allocation_share")
        if int(task["max_steps"]) <= 0 or float(task["episode_p95_seconds"]) <= 0:
            raise ValueError(f"{name}: max_steps and episode_p95_seconds must be positive")
        normalized_sum += target / standalone
    if not math.isclose(normalized_sum, 1.0, rel_tol=0, abs_tol=1e-9):
        raise ValueError(f"sum(target/standalone) must be 1.0, got {normalized_sum}")
    arrival_mode = str(document.get("load", {}).get("arrival_mode", "constant"))
    if arrival_mode not in ARRIVAL_MODES:
        raise ValueError(f"arrival_mode must be one of {sorted(ARRIVAL_MODES)}")
    segments = document.get("load", {}).get("formal_arrival_segments", [])
    if segments:
        fraction_sum = 0.0
        for index, segment in enumerate(segments):
            mode = str(segment.get("mode", ""))
            fraction = float(segment.get("fraction", 0))
            batch_size = int(segment.get("batch_size", 1))
            if mode not in ARRIVAL_MODES or fraction <= 0 or batch_size <= 0:
                raise ValueError(f"invalid formal_arrival_segments[{index}]")
            fraction_sum += fraction
        if not math.isclose(fraction_sum, 1.0, rel_tol=0, abs_tol=1e-9):
            raise ValueError(f"formal arrival segment fractions must sum to 1.0, got {fraction_sum}")
    replay = document.get("latency_replay")
    if not isinstance(replay, dict):
        raise ValueError("latency_replay must be configured")
    if replay.get("strategy") != LATENCY_REPLAY_STRATEGY:
        raise ValueError(
            f"latency_replay.strategy must be {LATENCY_REPLAY_STRATEGY}"
        )
    if replay.get("missing_policy") != "fail_closed":
        raise ValueError("latency_replay.missing_policy must be fail_closed")
    if replay.get("multi_turn_allocation") != "pre_frozen_per_turn":
        raise ValueError(
            "latency_replay.multi_turn_allocation must be pre_frozen_per_turn"
        )
    phases = document.get("phases", {})
    if not math.isclose(
        float(phases.get("pressure_multiplier", 0)),
        PHASE_MULTIPLIERS["pressure"],
        rel_tol=0,
        abs_tol=1e-9,
    ):
        raise ValueError("phases.pressure_multiplier must be 10.0")
    if document.get("load", {}).get("rate_basis") != "100xa100_throughput_estimate":
        raise ValueError("load.rate_basis must be 100xa100_throughput_estimate")
    if replay.get("latency_basis") != LATENCY_REPLAY_STRATEGY:
        raise ValueError(
            f"latency_replay.latency_basis must be {LATENCY_REPLAY_STRATEGY}"
        )
    for name, task in tasks.items():
        expected_policy = (
            "doubao_only_round_robin"
            if name == "swebench_pro"
            else "paired_doubao_qwen_alternating"
        )
        if task.get("sampling_policy") != expected_policy:
            raise ValueError(f"{name}: sampling_policy must be {expected_policy}")


def phase_rate(task: dict[str, Any], phase: str) -> float:
    if phase == "selfcheck":
        return float(task["standalone_rate_eps"])
    if phase not in PHASE_MULTIPLIERS:
        raise ValueError(f"unsupported phase {phase!r}")
    return float(task["target_rate_eps"]) * PHASE_MULTIPLIERS[phase]


def phase_duration(config: dict[str, Any], phase: str, override_seconds: float | None = None) -> float:
    if override_seconds is not None:
        if override_seconds <= 0:
            raise ValueError("duration override must be positive")
        return float(override_seconds)
    field = f"{phase}_seconds"
    if field not in config["phases"]:
        raise ValueError(f"phase {phase!r} has no duration")
    return float(config["phases"][field])


def scheduled_offsets(
    rate_eps: float,
    count: int,
    *,
    mode: str,
    batch_size: int = 1,
    seed: int = 0,
) -> list[float]:
    """Return monotonic planned offsets without consulting wall clock."""
    if rate_eps <= 0 or count < 0 or batch_size <= 0:
        raise ValueError("rate_eps and batch_size must be positive and count non-negative")
    if mode not in ARRIVAL_MODES:
        raise ValueError(f"unsupported arrival mode {mode!r}")
    if mode == "constant":
        return [index / rate_eps for index in range(count)]
    if mode == "batch":
        return [(index // batch_size) * batch_size / rate_eps for index in range(count)]
    rng = random.Random(seed)
    offsets: list[float] = []
    current = 0.0
    for index in range(count):
        if index:
            current += rng.expovariate(rate_eps)
        offsets.append(current)
    return offsets


def iter_planned_times(
    rate_eps: float,
    *,
    mode: str,
    batch_size: int,
    seed: int,
    start_ns: int | None = None,
) -> Iterator[tuple[int, int]]:
    """Yield ``(sequence, monotonic_ns deadline)`` indefinitely."""
    origin = time.monotonic_ns() if start_ns is None else start_ns
    rng = random.Random(seed)
    sequence = 0
    elapsed = 0.0
    while True:
        if mode == "constant":
            elapsed = sequence / rate_eps
        elif mode == "batch":
            elapsed = (sequence // batch_size) * batch_size / rate_eps
        elif mode == "poisson":
            if sequence:
                elapsed += rng.expovariate(rate_eps)
        else:
            raise ValueError(f"unsupported arrival mode {mode!r}")
        yield sequence, origin + int(elapsed * 1_000_000_000)
        sequence += 1


def trace_dataset_id(trace: dict[str, Any]) -> str:
    """Return the stable dataset item identity used to pair model trajectories."""
    for key in TRACE_ID_KEYS:
        value = trace.get(key)
        if value not in (None, ""):
            return str(value)
    metadata = trace.get("metadata")
    if isinstance(metadata, dict):
        for key in TRACE_ID_KEYS:
            value = metadata.get(key)
            if value not in (None, ""):
                return str(value)
    raise ValueError(f"trace {trace.get('trace_id', '<unknown>')} has no dataset identity")


def source_model_family(source_model: Any) -> str:
    value = str(source_model or "").strip().lower()
    if value.startswith("doubao"):
        return "doubao"
    if value.startswith("qwen"):
        return "qwen"
    return value


def trace_pair_id(trace: dict[str, Any]) -> str:
    pair_id = str(trace.get("pair_id") or "").strip()
    return pair_id or trace_dataset_id(trace)


def _positive_number(value: Any) -> float:
    if isinstance(value, bool):
        return 0.0
    try:
        number = float(value)
    except (TypeError, ValueError):
        return 0.0
    return number if number > 0 else 0.0


def frozen_replay_wait_ms(turn: dict[str, Any]) -> tuple[float, str, float]:
    """Read the already-frozen per-turn replay wait from a trace corpus row."""
    wait_ms = _positive_number(turn.get("replay_wait_ms"))
    latency_source = str(turn.get("latency_source", "")).strip()
    episode_elapsed_proxy_ms = _positive_number(turn.get("episode_elapsed_proxy_ms"))
    if wait_ms <= 0 or not latency_source:
        raise ValueError("trace turn requires positive replay_wait_ms and latency_source")
    return wait_ms, latency_source, episode_elapsed_proxy_ms


def validate_paired_trace_order(
    traces: list[dict[str, Any]],
    *,
    expected_pairs: int | None = None,
) -> dict[str, Any]:
    if len(traces) % 2:
        raise ValueError("paired trace corpus must contain an even number of rows")
    pair_ids: list[str] = []
    prompt_hash_matches = 0
    prompt_hash_mismatches = 0
    for index in range(0, len(traces), 2):
        doubao, qwen = traces[index : index + 2]
        pair_id = trace_pair_id(doubao)
        if pair_id != trace_pair_id(qwen):
            raise ValueError(f"trace rows {index}/{index + 1} do not share pair_id")
        if source_model_family(doubao.get("source_model")) != "doubao":
            raise ValueError(f"trace row {index} must be Doubao")
        if source_model_family(qwen.get("source_model")) != "qwen":
            raise ValueError(f"trace row {index + 1} must be Qwen")
        if str(doubao.get("prompt_hash")) == str(qwen.get("prompt_hash")):
            prompt_hash_matches += 1
        else:
            prompt_hash_mismatches += 1
        pair_ids.append(pair_id)
    if len(set(pair_ids)) != len(pair_ids):
        raise ValueError("paired trace corpus contains duplicate pair IDs")
    if expected_pairs is not None and len(pair_ids) != expected_pairs:
        raise ValueError(
            f"paired trace corpus has {len(pair_ids)} pairs; expected {expected_pairs}"
        )
    return {
        "pair_count": len(pair_ids),
        "paired_trace_count": len(traces),
        "pair_coverage": 1.0,
        "source_family_counts": {"doubao": len(pair_ids), "qwen": len(pair_ids)},
        "strict_alternation": True,
        "unique_pair_ids": True,
        "dataset_id_match": True,
        "prompt_hash_matches": prompt_hash_matches,
        "prompt_hash_mismatches": prompt_hash_mismatches,
    }


def select_trace_for_sequence(
    traces: list[dict[str, Any]],
    *,
    sequence: int,
    sampling_policy: str,
) -> tuple[dict[str, Any], int]:
    if sequence < 0:
        raise ValueError("trace sequence must be non-negative")
    if sampling_policy == "paired_doubao_qwen_alternating":
        if len(traces) % 2:
            raise ValueError("paired corpus size must be even")
        pair_count = len(traces) // 2
        slot = ((sequence // 2) % pair_count) * 2 + sequence % 2
    elif sampling_policy == "doubao_only_round_robin":
        slot = sequence % len(traces)
    else:
        raise ValueError(f"unsupported sampling_policy {sampling_policy!r}")
    return traces[slot], slot


def required_slots(rate_eps: float, episode_p95_seconds: float) -> int:
    return math.ceil(1.2 * rate_eps * episode_p95_seconds)


def required_capacity(config: dict[str, Any], phase: str) -> dict[str, Any]:
    by_task = {
        name: required_slots(phase_rate(task, phase), float(task["episode_p95_seconds"]))
        for name, task in config["tasks"].items()
    }
    maximum = int(config["run"]["max_samples_per_stream"])
    return {
        "by_task": by_task,
        "total_slots": sum(by_task.values()),
        "required_streams": math.ceil(sum(by_task.values()) / maximum),
    }


@dataclass
class EpisodeRecord:
    request_id: str
    task: str
    batch_id: str
    planned_at: float
    timeout_seconds: float
    dispatch_started: bool = False
    dispatched_at: float | None = None
    terminal_at: float | None = None
    status: str = "planned"
    error_code: str = ""
    error_message: str = ""
    result_checksum_valid: bool = False
    terminal_count: int = 0
    failure_class: str = "pending"

    @property
    def deadline(self) -> float:
        origin = self.dispatched_at if self.dispatched_at is not None else self.planned_at
        return origin + self.timeout_seconds


class EpisodeLedger:
    """Keep every dispatched ID in the denominator, including after stream loss."""

    def __init__(self) -> None:
        self.records: dict[str, EpisodeRecord] = {}

    def plan(self, record: EpisodeRecord) -> None:
        if record.request_id in self.records:
            raise ValueError(f"duplicate planned request_id {record.request_id}")
        self.records[record.request_id] = record

    def mark_dispatched(self, request_id: str, now: float) -> None:
        record = self.records[request_id]
        record.dispatch_started = True
        record.dispatched_at = now
        record.status = "dispatched"

    def record_terminal(
        self,
        request_id: str,
        *,
        now: float,
        status: str,
        error_code: str = "",
        error_message: str = "",
        checksum_valid: bool = True,
    ) -> None:
        record = self.records[request_id]
        record.terminal_count += 1
        if record.terminal_count > 1:
            record.failure_class = "duplicate_terminal_result"
            return
        record.terminal_at = now
        record.status = status
        record.error_code = error_code
        record.error_message = error_message
        record.result_checksum_valid = checksum_valid
        if now > record.deadline:
            record.failure_class = "late_result"
        elif status.lower() in TERMINAL_SUCCESS and checksum_valid:
            record.failure_class = "none"
        else:
            record.failure_class = "uenv_error"

    def reconcile(self, now: float, grace_seconds: float) -> None:
        for record in self.records.values():
            if not record.dispatch_started or record.terminal_count:
                continue
            if now >= record.deadline + grace_seconds:
                record.status = "timeout"
                record.error_code = "NO_TERMINAL_RESULT"
                record.error_message = "no terminal result before reconciliation grace expired"
                record.failure_class = "no_terminal_result"

    def dispatched(self) -> list[EpisodeRecord]:
        return [record for record in self.records.values() if record.dispatch_started]

    def assert_reconciled(self) -> None:
        pending = [record.request_id for record in self.dispatched() if record.failure_class == "pending"]
        if pending:
            raise RuntimeError(f"unreconciled dispatched episodes: {pending[:20]}")

    def write_csv(self, path: Path) -> None:
        rows = [asdict(record) for record in self.records.values()]
        write_csv(path, rows, list(asdict(EpisodeRecord("", "", "", 0, 0)).keys()))


def write_csv(path: Path, rows: Iterable[dict[str, Any]], fieldnames: list[str]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    with temporary.open("w", encoding="utf-8", newline="") as target:
        writer = csv.DictWriter(target, fieldnames=fieldnames, extrasaction="ignore")
        writer.writeheader()
        writer.writerows(rows)
    temporary.replace(path)


def append_csv(path: Path, row: dict[str, Any], fieldnames: list[str]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    exists = path.exists() and path.stat().st_size > 0
    with path.open("a", encoding="utf-8", newline="") as target:
        writer = csv.DictWriter(target, fieldnames=fieldnames, extrasaction="ignore")
        if not exists:
            writer.writeheader()
        writer.writerow(row)
        target.flush()


def verify_manifest(manifest_path: Path, *, root: Path | None = None) -> dict[str, Any]:
    document = json.loads(manifest_path.read_text(encoding="utf-8"))
    base = root or manifest_path.parent
    files = document.get("files")
    if not isinstance(files, list) or not files:
        raise ValueError(f"manifest has no files: {manifest_path}")
    for item in files:
        path = base / str(item["path"])
        if not path.is_file():
            raise ValueError(f"manifest file missing: {path}")
        if path.stat().st_size != int(item["size_bytes"]):
            raise ValueError(f"manifest size mismatch: {path}")
        if sha256_file(path) != str(item["sha256"]):
            raise ValueError(f"manifest sha256 mismatch: {path}")
    return document


def validate_trace_file(
    path: Path,
    *,
    dataset: str,
    minimum: int,
    max_latency_missing_ratio: float | None = None,
) -> dict[str, Any]:
    required = {
        "trace_id", "dataset", "source_model", "source_version", "collected_at",
        "prompt_hash", "turns", "result_checksum",
    }
    trace_ids: set[str] = set()
    valid = 0
    token_counts: list[int] = []
    episode_token_counts: list[int] = []
    request_byte_counts: list[int] = []
    response_byte_counts: list[int] = []
    turn_counts: list[int] = []
    latency_proxy_ms: list[float] = []
    latency_sources: dict[str, int] = {}
    effective_replay_seconds: list[float] = []
    model_stats: dict[str, dict[str, Any]] = {}
    with path.open("r", encoding="utf-8") as source:
        for line_number, line in enumerate(source, 1):
            if not line.strip():
                continue
            item = json.loads(line)
            missing = required - set(item)
            if missing:
                raise ValueError(f"{path}:{line_number} missing {sorted(missing)}")
            if str(item["dataset"]) != dataset:
                raise ValueError(f"{path}:{line_number} dataset mismatch")
            trace_id = str(item["trace_id"])
            if trace_id in trace_ids:
                raise ValueError(f"{path}:{line_number} duplicate trace_id {trace_id}")
            turns = item["turns"]
            if not isinstance(turns, list) or not turns:
                raise ValueError(f"{path}:{line_number} has no turns")
            episode_tokens = 0
            episode_request_bytes = 0
            episode_response_bytes = 0
            episode_replay_wait_ms = 0.0
            episode_elapsed_proxy_ms = 0.0
            for turn_index, turn in enumerate(turns):
                for field_name in (
                    "turn_index", "assistant_output", "source_completion_tokens",
                    "target_qwen3_tokens", "request_bytes", "response_bytes", "env_latency_ms",
                    "replay_wait_ms", "latency_source",
                ):
                    if field_name not in turn:
                        raise ValueError(f"{path}:{line_number} turn {turn_index} missing {field_name}")
                if not str(turn["assistant_output"]):
                    raise ValueError(f"{path}:{line_number} turn {turn_index} has empty output")
                replay_wait_ms, latency_source, turn_episode_proxy_ms = frozen_replay_wait_ms(turn)
                tokens = int(turn["target_qwen3_tokens"])
                request_bytes = int(turn["request_bytes"])
                response_bytes = int(turn["response_bytes"])
                if tokens < 0 or request_bytes < 0 or response_bytes < 0:
                    raise ValueError(
                        f"{path}:{line_number} turn {turn_index} has negative metrics"
                    )
                token_counts.append(tokens)
                episode_tokens += tokens
                episode_request_bytes += request_bytes
                episode_response_bytes += response_bytes
                episode_replay_wait_ms += replay_wait_ms
                episode_elapsed_proxy_ms = max(episode_elapsed_proxy_ms, turn_episode_proxy_ms)
                latency_sources[latency_source] = latency_sources.get(latency_source, 0) + 1
            latency_proxy_ms.append(episode_elapsed_proxy_ms or episode_replay_wait_ms)
            effective_replay_seconds.append(episode_replay_wait_ms / 1000.0)
            model = str(item["source_model"])
            family = source_model_family(model)
            model_record = model_stats.setdefault(
                model,
                {
                    "source_family": family,
                    "episodes": 0,
                    "turns": 0,
                    "episode_tokens": [],
                    "request_bytes": [],
                    "response_bytes": [],
                    "latency_proxy_ms": [],
                    "latency_missing": 0,
                },
            )
            model_record["episodes"] += 1
            model_record["turns"] += len(turns)
            model_record["episode_tokens"].append(episode_tokens)
            model_record["request_bytes"].append(episode_request_bytes)
            model_record["response_bytes"].append(episode_response_bytes)
            model_record["latency_proxy_ms"].append(
                episode_elapsed_proxy_ms or episode_replay_wait_ms
            )
            episode_token_counts.append(episode_tokens)
            request_byte_counts.append(episode_request_bytes)
            response_byte_counts.append(episode_response_bytes)
            turn_counts.append(len(turns))
            trace_ids.add(trace_id)
            valid += 1
    if valid < minimum:
        raise ValueError(f"{path} has {valid} valid traces; requires {minimum}")
    effective_latency_sources = dict(latency_sources)
    summarized_models: dict[str, Any] = {}
    for model, values in sorted(model_stats.items()):
        episodes = int(values["episodes"])
        summarized_models[model] = {
            "source_family": values["source_family"],
            "episodes": episodes,
            "turns": int(values["turns"]),
            "episode_token_p50": percentile(values["episode_tokens"], 0.50),
            "episode_token_p95": percentile(values["episode_tokens"], 0.95),
            "request_bytes_p50": percentile(values["request_bytes"], 0.50),
            "request_bytes_p95": percentile(values["request_bytes"], 0.95),
            "response_bytes_p50": percentile(values["response_bytes"], 0.50),
            "response_bytes_p95": percentile(values["response_bytes"], 0.95),
            "latency_proxy_seconds_p50": percentile(
                (value / 1000.0 for value in values["latency_proxy_ms"]), 0.50
            ),
            "latency_proxy_seconds_p95": percentile(
                (value / 1000.0 for value in values["latency_proxy_ms"]), 0.95
            ),
            "latency_missing": int(values["latency_missing"]),
            "latency_missing_ratio": int(values["latency_missing"]) / episodes,
        }
    return {
        "valid_traces": valid,
        "episode_count": valid,
        "turn_p50": percentile(turn_counts, 0.50),
        "turn_p95": percentile(turn_counts, 0.95),
        "token_p50": percentile(token_counts, 0.50),
        "token_p95": percentile(token_counts, 0.95),
        "episode_token_p50": percentile(episode_token_counts, 0.50),
        "episode_token_p95": percentile(episode_token_counts, 0.95),
        "request_bytes_p50": percentile(request_byte_counts, 0.50),
        "request_bytes_p95": percentile(request_byte_counts, 0.95),
        "response_bytes_p50": percentile(response_byte_counts, 0.50),
        "response_bytes_p95": percentile(response_byte_counts, 0.95),
        "latency_proxy_seconds_p50": percentile(
            (value / 1000.0 for value in latency_proxy_ms), 0.50
        ),
        "latency_proxy_seconds_p95": percentile(
            (value / 1000.0 for value in latency_proxy_ms), 0.95
        ),
        "latency_sources": dict(sorted(latency_sources.items())),
        "effective_replay_seconds_p50": percentile(
            effective_replay_seconds, 0.50
        ),
        "effective_replay_seconds_p95": percentile(
            effective_replay_seconds, 0.95
        ),
        "effective_latency_sources": dict(
            sorted(effective_latency_sources.items())
        ),
        "source_models": summarized_models,
    }


def classify_availability(samples: list[dict[str, Any]], *, fail_threshold: int = 3, recover_threshold: int = 3) -> list[tuple[float, float]]:
    """Convert one-second samples into confirmed outage intervals."""
    outages: list[tuple[float, float]] = []
    failed: list[float] = []
    recovered: list[float] = []
    outage_start: float | None = None
    for sample in sorted(samples, key=lambda item: float(item["timestamp"])):
        timestamp = float(sample["timestamp"])
        if bool(sample["ok"]):
            failed.clear()
            if outage_start is not None:
                recovered.append(timestamp)
                if len(recovered) >= recover_threshold:
                    outages.append((outage_start, recovered[0]))
                    outage_start = None
                    recovered.clear()
        else:
            recovered.clear()
            if outage_start is None:
                failed.append(timestamp)
                if len(failed) >= fail_threshold:
                    outage_start = failed[0]
                    failed.clear()
    if outage_start is not None and samples:
        outages.append((outage_start, float(samples[-1]["timestamp"]) + 1.0))
    return outages
