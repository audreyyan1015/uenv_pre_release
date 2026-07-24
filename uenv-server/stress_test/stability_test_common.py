#!/usr/bin/env python3
"""Shared primitives for the 100-GPU-equivalent UEnv stability suite.

This module intentionally has no grpc dependency so its deterministic scheduling,
admission, ledger and acceptance calculations can be unit-tested in isolation.
"""

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
ARRIVAL_MODES = {"constant", "poisson", "batch"}
PHASE_MULTIPLIERS = {"reference": 1.0, "stability": 1.0, "capacity": 1.2, "burst": 4.0, "fault": 1.0}
TERMINAL_SUCCESS = {"completed", "success"}


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
    for name, profile in document.get("latency_profiles", {}).items():
        if float(profile["mean_seconds"]) <= 0 or float(profile["base_tokens"]) <= 0:
            raise ValueError(f"latency profile {name} has non-positive values")
        if float(profile["std_seconds"]) < 0:
            raise ValueError(f"latency profile {name} std_seconds must be non-negative")


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


def deterministic_lognormal_base_seconds(
    profile: dict[str, Any], *, run_seed: int, episode_id: str
) -> float:
    mean = float(profile["mean_seconds"])
    std = float(profile["std_seconds"])
    sigma2 = math.log1p((std * std) / (mean * mean))
    mu = math.log(mean) - sigma2 / 2
    seed_bytes = hashlib.sha256(f"{run_seed}{episode_id}".encode()).digest()
    rng = random.Random(int.from_bytes(seed_bytes, "big"))
    return rng.lognormvariate(mu, math.sqrt(sigma2))


def target_llm_delay_seconds(
    profile: dict[str, Any], *, run_seed: int, episode_id: str, generated_tokens: int
) -> float:
    if generated_tokens < 0:
        raise ValueError("generated_tokens must be non-negative")
    base = deterministic_lognormal_base_seconds(profile, run_seed=run_seed, episode_id=episode_id)
    return base * generated_tokens / float(profile["base_tokens"])


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


def validate_trace_file(path: Path, *, dataset: str, minimum: int) -> dict[str, Any]:
    required = {
        "trace_id", "dataset", "source_model", "source_version", "collected_at",
        "prompt_hash", "turns", "result_checksum",
    }
    trace_ids: set[str] = set()
    valid = 0
    token_counts: list[int] = []
    byte_counts: list[int] = []
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
            for turn_index, turn in enumerate(turns):
                for field_name in (
                    "turn_index", "assistant_output", "source_completion_tokens",
                    "target_qwen3_tokens", "request_bytes", "response_bytes", "env_latency_ms",
                ):
                    if field_name not in turn:
                        raise ValueError(f"{path}:{line_number} turn {turn_index} missing {field_name}")
                if not str(turn["assistant_output"]):
                    raise ValueError(f"{path}:{line_number} turn {turn_index} has empty output")
                token_counts.append(int(turn["target_qwen3_tokens"]))
                byte_counts.append(int(turn["response_bytes"]))
            trace_ids.add(trace_id)
            valid += 1
    if valid < minimum:
        raise ValueError(f"{path} has {valid} valid traces; requires {minimum}")
    return {
        "valid_traces": valid,
        "token_p50": percentile(token_counts, 0.50),
        "token_p95": percentile(token_counts, 0.95),
        "response_bytes_p50": percentile(byte_counts, 0.50),
        "response_bytes_p95": percentile(byte_counts, 0.95),
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
