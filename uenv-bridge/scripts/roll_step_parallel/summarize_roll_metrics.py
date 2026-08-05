#!/usr/bin/env python3
"""Extract JSON metric records from ROLL driver logs."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from statistics import mean
from typing import Iterable


METRIC_PREFIXES = (
    "time/",
    "system/",
    "critic/",
    "actor/",
    "token/",
    "group/",
    "math_rule/",
    "val_correct/",
)

SUMMARY_KEYS = [
    "time/step_total",
    "time/step_generate",
    "time/step_train",
    "time/step_rollout",
    "time/step_model_update",
    "time/ref_log_probs_values",
    "time/old_log_probs",
    "time/val_step",
    "math_rule/scheduler/math_rule/time/generate/mean",
    "math_rule/scheduler/math_rule/time/reward/mean",
    "math_rule/scheduler/off_policy_ratio",
    "math_rule/actor/samples_used",
    "math_rule/actor/samples_total",
    "critic/score/mean",
    "critic/rewards/mean",
    "critic/advantages/mean",
    "actor/total_loss@sum",
]


def iter_json_objects(text: str):
    decoder = json.JSONDecoder()
    for line in text.splitlines():
        if "metrics_tag:" in line:
            line = line.split("metrics_tag:", 1)[1]
        for idx, char in enumerate(line):
            if char != "{":
                continue
            try:
                value, _ = decoder.raw_decode(line[idx:])
            except json.JSONDecodeError:
                continue
            if not isinstance(value, dict):
                continue

            # ROLL writes metrics as {"step": 0, "metrics": {...}} after a
            # metrics_tag prefix. Flatten it so all modes share one summary path.
            if isinstance(value.get("metrics"), dict):
                metrics = dict(value["metrics"])
                if "system/step" not in metrics and isinstance(value.get("step"), int):
                    metrics["system/step"] = value["step"]
                value = metrics

            if any(key.startswith(METRIC_PREFIXES) for key in value):
                yield value
                break


def numeric_values(records: Iterable[dict], key: str) -> list[float]:
    return [r[key] for r in records if isinstance(r.get(key), (int, float))]


def step_id(record: dict) -> int | None:
    value = record.get("system/step", record.get("system/global_step"))
    return value if isinstance(value, int) else None


def dedupe_step_records(records: Iterable[dict]) -> list[dict]:
    merged: dict[int, dict] = {}
    for record in records:
        sid = step_id(record)
        if sid is None:
            continue
        if sid not in merged:
            merged[sid] = dict(record)
            continue
        # ROLL may print the same step once as metrics_tag and once as a raw
        # JSON metrics line. Merge them so summaries count each training step once.
        merged[sid].update(record)
    return [merged[sid] for sid in sorted(merged)]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("logs", nargs="+", type=Path)
    parser.add_argument(
        "--drop-first",
        action="store_true",
        help="Also report means after dropping the first step record.",
    )
    args = parser.parse_args()

    for log_path in args.logs:
        records = list(iter_json_objects(log_path.read_text(errors="replace")))
        raw_step_records = [r for r in records if step_id(r) is not None]
        step_records = dedupe_step_records(raw_step_records)
        print(f"\n{log_path}")
        print(
            f"metric_records={len(records)} "
            f"raw_step_records={len(raw_step_records)} "
            f"step_records={len(step_records)}"
        )
        if not step_records:
            continue
        for key in SUMMARY_KEYS:
            values = numeric_values(step_records, key)
            if values:
                print(f"{key}: count={len(values)} mean={mean(values):.4f} last={values[-1]:.4f}")
                if args.drop_first and len(values) > 1:
                    print(f"{key} drop_first_mean={mean(values[1:]):.4f}")


if __name__ == "__main__":
    main()
