#!/usr/bin/env python3
from __future__ import annotations

import argparse
import csv
import math
import re
from pathlib import Path
from typing import Any


STEP_LINE_RE = re.compile(r"(?:^|\s)step:(?P<step>\d+)\s+-\s+(?P<body>.+)")
METRIC_RE = re.compile(r"(?P<key>[A-Za-z0-9_./-]+):(?P<value>.*?)(?=\s+-\s+[A-Za-z0-9_./-]+:|$)")

DEFAULT_METRICS = [
    "critic/score/mean",
    "critic/rewards/mean",
    "critic/advantages/mean",
    "critic/advantages/max",
    "critic/advantages/min",
    "actor/loss",
    "actor/entropy",
    "actor/grad_norm",
    "response_length/mean",
    "timing_s/step",
    "timing_s/gen",
    "timing_s/update_actor",
]

PLOT_COLORS = [
    "#2563eb",
    "#dc2626",
    "#16a34a",
    "#9333ea",
    "#ea580c",
    "#0891b2",
    "#4f46e5",
    "#be123c",
]

METRIC_LABELS = {
    "critic/score/mean": "Score Mean",
    "critic/rewards/mean": "Reward Mean",
    "critic/advantages/mean": "Advantage Mean",
    "critic/advantages/max": "Advantage Max",
    "critic/advantages/min": "Advantage Min",
    "actor/loss": "Actor Loss",
    "actor/entropy": "Actor Entropy",
    "actor/grad_norm": "Actor Grad Norm",
    "response_length/mean": "Response Length Mean",
    "timing_s/step": "Step Time (s)",
    "timing_s/gen": "Generation Time (s)",
    "timing_s/update_actor": "Actor Update Time (s)",
}


def parse_float(text: str) -> float | None:
    text = text.strip().strip("'\"")
    if text in {"None", "nan", "NaN", ""}:
        return None
    try:
        value = float(text)
    except ValueError:
        return None
    return value if math.isfinite(value) else None


def parse_log(path: Path, run_name: str) -> list[dict[str, float | int | str]]:
    rows: list[dict[str, float | int | str]] = []
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        match = STEP_LINE_RE.search(line)
        if not match:
            continue

        row: dict[str, float | int | str] = {
            "run": run_name,
            "source": str(path),
            "step": int(match.group("step")),
        }
        for metric_match in METRIC_RE.finditer(match.group("body")):
            value = parse_float(metric_match.group("value"))
            if value is not None:
                row[metric_match.group("key")] = value
        rows.append(row)
    return rows


def write_csv(rows: list[dict[str, float | int | str]], output: Path) -> None:
    keys = {"run", "source", "step"}
    for row in rows:
        keys.update(row.keys())
    fieldnames = ["run", "source", "step"] + sorted(keys - {"run", "source", "step"})
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("w", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(handle, fieldnames=fieldnames)
        writer.writeheader()
        writer.writerows(rows)


def metric_label(metric: str) -> str:
    if metric in METRIC_LABELS:
        return METRIC_LABELS[metric]
    return metric.replace("/", " ").replace("_", " ").replace("-", " ").title()


def metric_filename(metric: str) -> str:
    return metric.replace("/", "__") + ".png"


def format_metric_tick(value: float, span: float | None = None) -> str:
    if not math.isfinite(value):
        return ""
    if value == 0:
        return "0"
    if abs(value) >= 10000 or abs(value) < 1e-5:
        return f"{value:.2e}"
    digits = 4 if span is None or span >= 1e-3 else 6
    return f"{value:.{digits}f}".rstrip("0").rstrip(".")


def y_limits(values: list[float]) -> tuple[float, float, float]:
    y_min = min(values)
    y_max = max(values)
    span = y_max - y_min
    scale = max(abs(y_min), abs(y_max), 1.0)

    if span == 0 or span < scale * 1e-4:
        center = (y_min + y_max) / 2
        pad = max(scale * 0.05, 1e-3)
        return center - pad, center + pad, 2 * pad

    pad = max(span * 0.10, scale * 0.01)
    return y_min - pad, y_max + pad, span + 2 * pad


def apply_plot_style() -> Any:
    try:
        import matplotlib

        matplotlib.use("Agg")
        import matplotlib.pyplot as plt
        from cycler import cycler
    except Exception as exc:  # pragma: no cover - depends on runtime env
        raise SystemExit("matplotlib is required. Install it with: pip install matplotlib") from exc

    plt.rcParams.update(
        {
            "figure.dpi": 130,
            "savefig.dpi": 180,
            "figure.facecolor": "white",
            "axes.facecolor": "#f8fafc",
            "axes.edgecolor": "#cbd5e1",
            "axes.labelcolor": "#334155",
            "axes.titlecolor": "#0f172a",
            "axes.prop_cycle": cycler(color=PLOT_COLORS),
            "axes.spines.top": False,
            "axes.spines.right": False,
            "xtick.color": "#475569",
            "ytick.color": "#475569",
            "grid.color": "#cbd5e1",
            "grid.alpha": 0.55,
            "grid.linewidth": 0.8,
            "legend.frameon": False,
            "font.size": 10,
            "axes.titlesize": 15,
            "axes.titleweight": "bold",
            "axes.labelsize": 10,
        }
    )
    return plt


def plot_metrics(rows: list[dict[str, float | int | str]], metrics: list[str], output_dir: Path) -> list[Path]:
    plt = apply_plot_style()
    from matplotlib.ticker import FuncFormatter, MaxNLocator

    output_dir.mkdir(parents=True, exist_ok=True)
    paths: list[Path] = []
    runs = sorted({str(row["run"]) for row in rows})

    for metric in metrics:
        series = build_series(rows, runs, metric)
        if not series:
            continue

        metric_values = [value for _, _, y_values in series for value in y_values]
        y_min, y_max, y_span = y_limits(metric_values)
        fig_height = 5.0 if len(series) <= 2 else 5.8
        fig, ax = plt.subplots(figsize=(9.6, fig_height), constrained_layout=True)

        for index, (run, x_values, y_values) in enumerate(series):
            color = PLOT_COLORS[index % len(PLOT_COLORS)]
            markevery = max(1, len(x_values) // 18)
            ax.plot(
                x_values,
                y_values,
                color=color,
                linewidth=2.2,
                marker="o",
                markersize=4.2,
                markeredgewidth=0,
                markevery=markevery,
                label=short_run_name(run),
            )
            annotate_last_point(ax, x_values, y_values, color, y_span, len(series))

        if y_min < 0 < y_max:
            ax.axhline(0, color="#64748b", linewidth=1.0, alpha=0.8)

        ax.set_ylim(y_min, y_max)
        ax.set_title(metric_label(metric), loc="left", pad=16)
        ax.set_xlabel("Training Step")
        ax.set_ylabel(metric_label(metric))
        ax.yaxis.set_major_locator(MaxNLocator(nbins=6))
        ax.yaxis.set_major_formatter(FuncFormatter(lambda value, _: format_metric_tick(value, y_span)))
        ax.xaxis.set_major_locator(MaxNLocator(integer=True, nbins=8))
        ax.grid(True, axis="y")
        ax.grid(True, axis="x", alpha=0.20)
        ax.text(
            1.0,
            1.02,
            summary_text(series),
            transform=ax.transAxes,
            ha="right",
            va="bottom",
            color="#64748b",
            fontsize=9,
        )

        if len(series) == 1:
            ax.text(
                0.01,
                0.02,
                short_run_name(series[0][0]),
                transform=ax.transAxes,
                ha="left",
                va="bottom",
                color="#64748b",
                fontsize=9,
            )
        else:
            columns = min(len(series), 3)
            ax.legend(loc="upper center", bbox_to_anchor=(0.5, -0.16), ncol=columns, fontsize=9)

        path = output_dir / metric_filename(metric)
        fig.savefig(path, bbox_inches="tight", facecolor="white")
        plt.close(fig)
        paths.append(path)
    return paths


def build_series(
    rows: list[dict[str, float | int | str]],
    runs: list[str],
    metric: str,
) -> list[tuple[str, list[int], list[float]]]:
    series = []
    for run in runs:
        run_rows = sorted((row for row in rows if row["run"] == run and metric in row), key=lambda row: int(row["step"]))
        if not run_rows:
            continue
        x_values = [int(row["step"]) for row in run_rows]
        y_values = [float(row[metric]) for row in run_rows]
        series.append((run, x_values, y_values))
    return series


def annotate_last_point(
    ax: Any,
    x_values: list[int],
    y_values: list[float],
    color: str,
    y_span: float,
    series_count: int,
) -> None:
    if not x_values or series_count > 4:
        return
    ax.annotate(
        format_metric_tick(y_values[-1], y_span),
        xy=(x_values[-1], y_values[-1]),
        xytext=(7, 0),
        textcoords="offset points",
        va="center",
        color=color,
        fontsize=8,
        fontweight="bold",
    )


def short_run_name(run: str) -> str:
    if len(run) <= 34:
        return run
    return run[:15] + "..." + run[-15:]


def summary_text(series: list[tuple[str, list[int], list[float]]]) -> str:
    step_count = max((len(x_values) for _, x_values, _ in series), default=0)
    run_count = len(series)
    if run_count == 1:
        return f"{step_count} steps"
    return f"{run_count} runs, up to {step_count} steps"


def main() -> int:
    parser = argparse.ArgumentParser(description="Parse VeRL console logs into CSV and matplotlib metric plots.")
    parser.add_argument("logs", nargs="+", type=Path, help="VeRL log files to parse.")
    parser.add_argument("--output-csv", type=Path, required=True, help="Output CSV path.")
    parser.add_argument("--plot-dir", type=Path, help="Directory for PNG plots.")
    parser.add_argument("--metric", action="append", dest="metrics", help="Metric key to plot. Repeatable.")
    args = parser.parse_args()

    rows: list[dict[str, float | int | str]] = []
    for log_path in args.logs:
        run_name = log_path.stem
        rows.extend(parse_log(log_path, run_name))

    if not rows:
        raise SystemExit("No VeRL step metrics found in input logs.")

    write_csv(rows, args.output_csv)
    print(f"wrote {len(rows)} rows to {args.output_csv}")

    if args.plot_dir:
        plotted = plot_metrics(rows, args.metrics or DEFAULT_METRICS, args.plot_dir)
        if plotted:
            print(f"wrote {len(plotted)} plots to {args.plot_dir}")
        else:
            print("no requested metrics were present; skipped plots")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
