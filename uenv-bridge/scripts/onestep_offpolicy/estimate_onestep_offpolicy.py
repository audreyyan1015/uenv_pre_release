#!/usr/bin/env python3
from __future__ import annotations

import argparse
import csv
import math
import re
from pathlib import Path
from statistics import mean
from typing import Any


STEP_LINE_RE = re.compile(r"(?:^|\s)step:(?P<step>\d+)\s+-\s+(?P<body>.+)")
METRIC_RE = re.compile(r"(?P<key>[A-Za-z0-9_./-]+):(?P<value>.*?)(?=\s+-\s+[A-Za-z0-9_./-]+:|$)")

TRAIN_SIDE_METRICS = [
    "timing_s/reward",
    "timing_s/old_log_prob",
    "timing_s/ref",
    "timing_s/adv",
    "timing_s/update_actor",
    "timing_s/update_weights",
]

SUMMARY_METRICS = [
    "critic/rewards/mean",
    "critic/advantages/mean",
    "actor/loss",
    "actor/kl_loss",
    "actor/entropy",
    "response_length/mean",
]


def parse_float(text: str) -> float | None:
    text = text.strip().strip("'\"")
    if text in {"", "None", "nan", "NaN"}:
        return None
    try:
        value = float(text)
    except ValueError:
        return None
    return value if math.isfinite(value) else None


def parse_log(path: Path) -> list[dict[str, float | int | str]]:
    rows: list[dict[str, float | int | str]] = []
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        match = STEP_LINE_RE.search(line)
        if not match:
            continue
        row: dict[str, float | int | str] = {"step": int(match.group("step")), "source": str(path)}
        for metric_match in METRIC_RE.finditer(match.group("body")):
            value = parse_float(metric_match.group("value"))
            if value is not None:
                row[metric_match.group("key")] = value
        if "timing_s/step" in row and "timing_s/gen" in row:
            rows.append(row)
    rows.sort(key=lambda item: int(item["step"]))
    return rows


def float_value(row: dict[str, Any], key: str, default: float = 0.0) -> float:
    value = row.get(key, default)
    try:
        return float(value)
    except (TypeError, ValueError):
        return default


def train_side_time(row: dict[str, Any]) -> float:
    explicit = sum(float_value(row, key) for key in TRAIN_SIDE_METRICS)
    if explicit > 0:
        return explicit
    return max(0.0, float_value(row, "timing_s/step") - float_value(row, "timing_s/gen"))


def residual_time(row: dict[str, Any]) -> float:
    return max(0.0, float_value(row, "timing_s/step") - float_value(row, "timing_s/gen") - train_side_time(row))


def percentile(values: list[float], q: float) -> float:
    if not values:
        return 0.0
    ordered = sorted(values)
    index = (len(ordered) - 1) * q
    lower = math.floor(index)
    upper = math.ceil(index)
    if lower == upper:
        return ordered[lower]
    return ordered[lower] * (upper - index) + ordered[upper] * (index - lower)


def estimate_pipeline(rows: list[dict[str, Any]]) -> dict[str, Any]:
    rollout = [float_value(row, "timing_s/gen") for row in rows]
    train = [train_side_time(row) for row in rows]
    step = [float_value(row, "timing_s/step") for row in rows]

    sync_total = sum(step)
    if not rows:
        return {}
    if len(rows) == 1:
        offpolicy_total = rollout[0] + train[0]
    else:
        overlapped_slots = [max(rollout[index + 1], train[index]) for index in range(len(rows) - 1)]
        offpolicy_total = rollout[0] + sum(overlapped_slots) + train[-1]

    return {
        "step_count": len(rows),
        "sync_total_s": sync_total,
        "sync_avg_step_s": sync_total / len(rows),
        "onestep_est_total_s": offpolicy_total,
        "onestep_est_avg_step_s": offpolicy_total / len(rows),
        "saved_s": sync_total - offpolicy_total,
        "speedup": sync_total / offpolicy_total if offpolicy_total > 0 else 0.0,
        "saved_ratio": (sync_total - offpolicy_total) / sync_total if sync_total > 0 else 0.0,
        "rollout_avg_s": mean(rollout),
        "rollout_p50_s": percentile(rollout, 0.50),
        "rollout_p95_s": percentile(rollout, 0.95),
        "train_side_avg_s": mean(train),
        "train_side_p50_s": percentile(train, 0.50),
        "train_side_p95_s": percentile(train, 0.95),
        "sync_step_p50_s": percentile(step, 0.50),
        "sync_step_p95_s": percentile(step, 0.95),
    }


def per_step_rows(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
    output = []
    for index, row in enumerate(rows):
        rollout = float_value(row, "timing_s/gen")
        train = train_side_time(row)
        next_rollout = float_value(rows[index + 1], "timing_s/gen") if index + 1 < len(rows) else 0.0
        output.append(
            {
                "step": int(row["step"]),
                "sync_step_s": float_value(row, "timing_s/step"),
                "rollout_s": rollout,
                "train_side_s": train,
                "next_rollout_s": next_rollout,
                "pipeline_slot_s": max(next_rollout, train) if index + 1 < len(rows) else train,
                "hidden_train_s": min(next_rollout, train) if index + 1 < len(rows) else 0.0,
                "rollout_minus_train_s": rollout - train,
                "reward_mean": float_value(row, "critic/rewards/mean", float("nan")),
                "actor_loss": float_value(row, "actor/loss", float("nan")),
                "advantage_mean": float_value(row, "critic/advantages/mean", float("nan")),
                "response_length_mean": float_value(row, "response_length/mean", float("nan")),
            }
        )
    return output


def write_csv(rows: list[dict[str, Any]], path: Path) -> None:
    if not rows:
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    fieldnames = list(rows[0].keys())
    with path.open("w", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(handle, fieldnames=fieldnames)
        writer.writeheader()
        writer.writerows(rows)


def fmt_seconds(value: float) -> str:
    return f"{value:.2f}s"


def fmt_ratio(value: float) -> str:
    return f"{value * 100:.2f}%"


def summary_metric(rows: list[dict[str, Any]], metric: str) -> tuple[float, float, float] | None:
    values = [float_value(row, metric, float("nan")) for row in rows]
    values = [value for value in values if math.isfinite(value)]
    if not values:
        return None
    return min(values), mean(values), max(values)


def write_markdown(rows: list[dict[str, Any]], estimate: dict[str, Any], output: Path, source_log: Path, csv_path: Path) -> None:
    lines = [
        "# One-step Off-policy 耗时估算报告",
        "",
        "## 输入",
        "",
        f"- 同步日志：`{source_log}`",
        f"- 解析 step 数：`{estimate['step_count']}`",
        f"- 明细 CSV：`{csv_path}`",
        "",
        "## 估算模型",
        "",
        "该脚本不实现 one-step off-policy，只用已有同步日志做理论估算。",
        "",
        "同步模式：",
        "",
        "```text",
        "total_sync = sum(timing_s/step)",
        "```",
        "",
        "One-step off-policy 估算：",
        "",
        "```text",
        "total_one_step = rollout_0 + sum(max(rollout_{i+1}, train_side_i)) + train_side_last",
        "```",
        "",
        "`train_side` 默认由以下耗时相加：",
        "",
        "```text",
        ", ".join(TRAIN_SIDE_METRICS),
        "```",
        "",
        "## 总体结果",
        "",
        "| 指标 | 同步 step | One-step off-policy 估算 |",
        "|---|---:|---:|",
        f"| 总耗时 | {fmt_seconds(estimate['sync_total_s'])} | {fmt_seconds(estimate['onestep_est_total_s'])} |",
        f"| 平均每 step | {fmt_seconds(estimate['sync_avg_step_s'])} | {fmt_seconds(estimate['onestep_est_avg_step_s'])} |",
        f"| 吞吐提升倍数 | 1.00x | {estimate['speedup']:.3f}x |",
        f"| 节省比例 | 0.00% | {fmt_ratio(estimate['saved_ratio'])} |",
        "",
        "## 耗时结构",
        "",
        "| 指标 | 平均 | P50 | P95 |",
        "|---|---:|---:|---:|",
        f"| rollout / timing_s/gen | {fmt_seconds(estimate['rollout_avg_s'])} | {fmt_seconds(estimate['rollout_p50_s'])} | {fmt_seconds(estimate['rollout_p95_s'])} |",
        f"| train side | {fmt_seconds(estimate['train_side_avg_s'])} | {fmt_seconds(estimate['train_side_p50_s'])} | {fmt_seconds(estimate['train_side_p95_s'])} |",
        f"| sync step | {fmt_seconds(estimate['sync_avg_step_s'])} | {fmt_seconds(estimate['sync_step_p50_s'])} | {fmt_seconds(estimate['sync_step_p95_s'])} |",
        "",
        "## 训练指标范围",
        "",
        "| 指标 | min | mean | max |",
        "|---|---:|---:|---:|",
    ]
    for metric in SUMMARY_METRICS:
        summary = summary_metric(rows, metric)
        if summary is None:
            continue
        lines.append(f"| `{metric}` | {summary[0]:.6g} | {summary[1]:.6g} | {summary[2]:.6g} |")
    lines.extend(
        [
            "",
            "## 解读",
            "",
            "- 如果 `rollout / timing_s/gen` 远大于 `train side`，one-step off-policy 的主要收益来自把训练侧耗时隐藏在下一步 rollout 后面。",
            "- 如果 `train side` 接近或大于 rollout，one-step off-policy 的收益会下降，甚至需要更多 trainer/rollout 资源才能体现。",
            "- 该估算默认结果最多 stale 一步，实际实现时必须记录 `policy_version`、`rollout_step`、`request_id`，并在 result 回来时检查 `max_staleness <= 1`。",
            "- 该估算是理论上界，不包含异步队列、调度、权重版本检查和失败重试的额外开销。",
            "",
        ]
    )
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text("\n".join(lines), encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Estimate one-step off-policy timing benefit from an existing synchronous VeRL training log."
    )
    parser.add_argument("log", type=Path, help="VeRL console log containing 'step:<n> - metric:value' lines.")
    parser.add_argument("--output-dir", type=Path, default=Path("logs/onestep_offpolicy"), help="Output directory.")
    parser.add_argument("--prefix", default="", help="Output filename prefix. Defaults to input log stem.")
    args = parser.parse_args()

    rows = parse_log(args.log)
    if not rows:
        raise SystemExit(f"No VeRL step timing rows found in {args.log}")

    prefix = args.prefix or args.log.stem
    output_dir = args.output_dir
    csv_path = output_dir / f"{prefix}_onestep_estimate.csv"
    report_path = output_dir / f"{prefix}_onestep_estimate.md"
    detail_rows = per_step_rows(rows)
    estimate = estimate_pipeline(rows)

    write_csv(detail_rows, csv_path)
    write_markdown(rows, estimate, report_path, args.log, csv_path)

    print(f"steps={estimate['step_count']}")
    print(f"sync_total={fmt_seconds(estimate['sync_total_s'])}")
    print(f"onestep_est_total={fmt_seconds(estimate['onestep_est_total_s'])}")
    print(f"speedup={estimate['speedup']:.3f}x")
    print(f"saved_ratio={fmt_ratio(estimate['saved_ratio'])}")
    print(f"wrote_csv={csv_path}")
    print(f"wrote_report={report_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
