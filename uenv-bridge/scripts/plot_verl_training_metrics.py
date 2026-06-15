#!/usr/bin/env python3
from __future__ import annotations

import argparse
import csv
import math
import re
from pathlib import Path


STEP_LINE_RE = re.compile(r"(?:^|\s)step:(?P<step>\d+)\s+-\s+(?P<body>.+)")
METRIC_RE = re.compile(r"(?P<key>[A-Za-z0-9_./-]+):(?P<value>[^-]+?)(?=\s+-\s+[A-Za-z0-9_./-]+:|$)")

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


def parse_float(text: str) -> float | None:
    text = text.strip().strip("'\"")
    if text in {"None", "nan", "NaN", ""}:
        return None
    try:
        value = float(text)
    except ValueError:
        return None
    if math.isfinite(value):
        return value
    return None


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


def plot_metrics(rows: list[dict[str, float | int | str]], metrics: list[str], output_dir: Path) -> list[Path]:
    try:
        import matplotlib.pyplot as plt
    except Exception:
        return plot_metrics_svg(rows, metrics, output_dir)

    output_dir.mkdir(parents=True, exist_ok=True)
    paths: list[Path] = []
    runs = sorted({str(row["run"]) for row in rows})

    for metric in metrics:
        if not any(metric in row for row in rows):
            continue

        fig, ax = plt.subplots(figsize=(8, 4.5))
        for run in runs:
            run_rows = sorted((row for row in rows if row["run"] == run and metric in row), key=lambda r: int(r["step"]))
            if not run_rows:
                continue
            ax.plot(
                [int(row["step"]) for row in run_rows],
                [float(row[metric]) for row in run_rows],
                marker="o",
                linewidth=1.5,
                label=run,
            )

        ax.set_title(metric)
        ax.set_xlabel("training step")
        ax.set_ylabel(metric)
        ax.grid(True, alpha=0.3)
        ax.legend()
        fig.tight_layout()

        path = output_dir / f"{metric.replace('/', '__')}.png"
        fig.savefig(path, dpi=150)
        plt.close(fig)
        paths.append(path)
    return paths


def plot_metrics_svg(rows: list[dict[str, float | int | str]], metrics: list[str], output_dir: Path) -> list[Path]:
    output_dir.mkdir(parents=True, exist_ok=True)
    paths: list[Path] = []
    runs = sorted({str(row["run"]) for row in rows})
    colors = ["#2563eb", "#dc2626", "#16a34a", "#9333ea", "#ea580c", "#0891b2"]

    for metric in metrics:
        series = []
        for run in runs:
            points = sorted(
                ((int(row["step"]), float(row[metric])) for row in rows if row["run"] == run and metric in row),
                key=lambda item: item[0],
            )
            if points:
                series.append((run, points))
        if not series:
            continue

        all_x = [x for _, points in series for x, _ in points]
        all_y = [y for _, points in series for _, y in points]
        x_min, x_max = min(all_x), max(all_x)
        y_min, y_max = min(all_y), max(all_y)
        if x_min == x_max:
            x_max += 1
        if y_min == y_max:
            pad = max(abs(y_min) * 0.05, 1.0)
            y_min -= pad
            y_max += pad

        width, height = 900, 520
        left, right, top, bottom = 80, 30, 50, 80
        plot_w = width - left - right
        plot_h = height - top - bottom

        def sx(x: int) -> float:
            return left + (x - x_min) / (x_max - x_min) * plot_w

        def sy(y: float) -> float:
            return top + (y_max - y) / (y_max - y_min) * plot_h

        elements = [
            f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}">',
            '<rect width="100%" height="100%" fill="white"/>',
            f'<text x="{left}" y="30" font-family="sans-serif" font-size="20" font-weight="600">{escape_xml(metric)}</text>',
            f'<line x1="{left}" y1="{top + plot_h}" x2="{left + plot_w}" y2="{top + plot_h}" stroke="#333"/>',
            f'<line x1="{left}" y1="{top}" x2="{left}" y2="{top + plot_h}" stroke="#333"/>',
            f'<text x="{left + plot_w / 2}" y="{height - 25}" text-anchor="middle" font-family="sans-serif" font-size="13">training step</text>',
            f'<text x="20" y="{top + plot_h / 2}" text-anchor="middle" font-family="sans-serif" font-size="13" transform="rotate(-90 20 {top + plot_h / 2})">{escape_xml(metric)}</text>',
        ]

        for i in range(5):
            x = left + i / 4 * plot_w
            step = x_min + i / 4 * (x_max - x_min)
            elements.append(f'<line x1="{x:.1f}" y1="{top}" x2="{x:.1f}" y2="{top + plot_h}" stroke="#e5e7eb"/>')
            elements.append(
                f'<text x="{x:.1f}" y="{top + plot_h + 20}" text-anchor="middle" font-family="sans-serif" font-size="11">{step:.0f}</text>'
            )
        for i in range(5):
            y = top + i / 4 * plot_h
            value = y_max - i / 4 * (y_max - y_min)
            elements.append(f'<line x1="{left}" y1="{y:.1f}" x2="{left + plot_w}" y2="{y:.1f}" stroke="#e5e7eb"/>')
            elements.append(
                f'<text x="{left - 8}" y="{y + 4:.1f}" text-anchor="end" font-family="sans-serif" font-size="11">{value:.3g}</text>'
            )

        for idx, (run, points) in enumerate(series):
            color = colors[idx % len(colors)]
            path_points = " ".join(f"{sx(x):.1f},{sy(y):.1f}" for x, y in points)
            elements.append(f'<polyline points="{path_points}" fill="none" stroke="{color}" stroke-width="2"/>')
            for x, y in points:
                elements.append(f'<circle cx="{sx(x):.1f}" cy="{sy(y):.1f}" r="3" fill="{color}"/>')
            legend_y = top + idx * 20
            elements.append(f'<rect x="{left + plot_w - 180}" y="{legend_y - 10}" width="12" height="12" fill="{color}"/>')
            elements.append(
                f'<text x="{left + plot_w - 162}" y="{legend_y}" font-family="sans-serif" font-size="12">{escape_xml(run)}</text>'
            )

        elements.append("</svg>")
        path = output_dir / f"{metric.replace('/', '__')}.svg"
        path.write_text("\n".join(elements), encoding="utf-8")
        paths.append(path)

    return paths


def escape_xml(text: str) -> str:
    return (
        text.replace("&", "&amp;")
        .replace("<", "&lt;")
        .replace(">", "&gt;")
        .replace('"', "&quot;")
        .replace("'", "&apos;")
    )


def main() -> int:
    parser = argparse.ArgumentParser(description="Parse VeRL console logs into CSV and metric plots.")
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
            print("matplotlib is unavailable or no requested metrics were present; skipped plots")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
