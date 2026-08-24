#!/usr/bin/env python3
"""Generate SWE-smith training charts for the quarterly meeting slides.

The script merges the original training run and the resumed run:

- `verl_swesmith_grpo_train_20260812_184238`: keep steps <= 425
- `verl_swesmith_grpo_resume_20260816_101936`: keep steps >= 426

It writes PNG files into the same directory as this script.
"""

from __future__ import annotations

import json
import re
from pathlib import Path

import matplotlib

matplotlib.use("Agg")
from matplotlib.mlab import window_none
import matplotlib.pyplot as plt
import numpy as np
import pandas as pd


REPO_ROOT = Path("/data/ronghao/uenv/uenv-bridge")
OUT_DIR = Path(__file__).resolve().parent

OLD_RUN = "verl_swesmith_grpo_train_20260812_184238"
RESUME_RUN = "verl_swesmith_grpo_resume_20260816_101936"

ANSI_RE = re.compile(r"\x1b\[[0-9;]*m")
NUM_RE = re.compile(r"([-+]?(?:\d+\.\d*|\d*\.\d+|\d+)(?:[eE][-+]?\d+)?)")
STEP_RE = re.compile(r"step-(\d+)-")


def setup_matplotlib() -> None:
    plt.style.use("seaborn-v0_8-whitegrid")
    plt.rcParams.update(
        {
            "figure.dpi": 180,
            "savefig.dpi": 180,
            "axes.titlesize": 14,
            "axes.labelsize": 11,
            "xtick.labelsize": 10,
            "ytick.labelsize": 10,
            "legend.fontsize": 9,
            "font.family": "DejaVu Sans",
            "axes.unicode_minus": False,
        }
    )


def parse_training_log(path: Path) -> list[dict[str, float | str]]:
    rows: list[dict[str, float | str]] = []
    for raw in path.read_text(errors="ignore").splitlines():
        line = ANSI_RE.sub("", raw)
        if "training/global_step" not in line or "step:" not in line:
            continue

        row: dict[str, float | str] = {"source_log": path.name}
        for segment in line.split(" - "):
            if ":" not in segment:
                continue
            key, value = segment.rsplit(":", 1)
            key = key.split()[-1].strip()
            match = NUM_RE.search(value)
            if not match:
                continue
            row[key] = float(match.group(1))

        if "training/global_step" in row and "timing_s/step" in row:
            rows.append(row)
    return rows


def build_step_dataframe() -> pd.DataFrame:
    old_log = REPO_ROOT / f"temp/logs/verl_layer4_agent_loop/{OLD_RUN}.log"
    resume_log = REPO_ROOT / f"temp/logs/verl_layer4_agent_loop/{RESUME_RUN}.log"

    rows_by_step: dict[int, dict[str, float | str]] = {}
    for row in parse_training_log(old_log):
        step = int(row["training/global_step"])
        if step <= 425:
            rows_by_step[step] = row

    for row in parse_training_log(resume_log):
        step = int(row["training/global_step"])
        if step >= 426:
            rows_by_step[step] = row

    rows = [rows_by_step[step] for step in sorted(rows_by_step)]
    df = pd.DataFrame(rows)
    if df.empty:
        raise RuntimeError("no training rows parsed")
    return df


def batch_step(obj: dict) -> int | None:
    batch_id = obj.get("batch_id") or ""
    match = STEP_RE.search(batch_id)
    return int(match.group(1)) if match else None


def load_filtered_jsonl(path: Path, keep_step) -> list[dict]:
    objs: list[dict] = []
    if not path.exists():
        return objs
    for line in path.read_text(errors="ignore").splitlines():
        try:
            obj = json.loads(line)
        except json.JSONDecodeError:
            continue
        step = batch_step(obj)
        if step is not None and keep_step(step):
            objs.append(obj)
    return objs


def load_episode_flow() -> tuple[list[dict], list[dict]]:
    old_dir = REPO_ROOT / f"temp/logs/layer4_distributed/{OLD_RUN}"
    resume_dir = REPO_ROOT / f"temp/logs/layer4_distributed/{RESUME_RUN}"

    requests = []
    results = []
    requests += load_filtered_jsonl(old_dir / "agent-loop-requests.jsonl", lambda step: step <= 425)
    requests += load_filtered_jsonl(resume_dir / "agent-loop-requests.jsonl", lambda step: step >= 426)
    results += load_filtered_jsonl(old_dir / "agent-loop-results.jsonl", lambda step: step <= 425)
    results += load_filtered_jsonl(resume_dir / "agent-loop-results.jsonl", lambda step: step >= 426)
    return requests, results


def load_gateway_rows() -> list[dict]:
    rows: list[dict] = []
    for run_id in (OLD_RUN, RESUME_RUN):
        path = REPO_ROOT / f"temp/logs/layer4_distributed/{run_id}/model-gateway.jsonl"
        if not path.exists():
            continue
        for line in path.read_text(errors="ignore").splitlines():
            try:
                rows.append(json.loads(line))
            except json.JSONDecodeError:
                continue
    return rows


def save_reward_curve(df: pd.DataFrame, full_label: str) -> None:
    fig, ax = plt.subplots(figsize=(10, 4.8))
    ax.plot(
        df["training/global_step"],
        df["critic/rewards/mean"],
        color="#1f77b4",
        alpha=0.28,
        linewidth=1.2,
        label="reward mean",
    )
    window_size = 100
    rolling_reward = df["critic/rewards/mean"].rolling(window=window_size, min_periods=1).mean()
    ax.plot(
        df["training/global_step"],
        rolling_reward,
        color="#d62728",
        linewidth=2.2,
        label=f"{window_size}-step moving avg",
    )
    ax.set_title("SWE-smith training reward curve")
    ax.set_xlabel("global step")
    ax.set_ylabel("reward mean")
    ax.set_ylim(-0.05, 1.05)
    ax.legend(loc="upper left")
    ax.text(0.99, 0.03, full_label, transform=ax.transAxes, ha="right", va="bottom", fontsize=9, color="#444")
    fig.tight_layout()
    fig.savefig(OUT_DIR / "reward_curve.png", bbox_inches="tight")
    plt.close(fig)


def save_reward_curve_review(df: pd.DataFrame, full_label: str, results: list[dict]) -> None:
    """Save a review-facing reward plot without overstating trend strength."""

    steps = df["training/global_step"]
    rewards = df["critic/rewards/mean"]
    rolling50 = rewards.rolling(window=50, min_periods=10).mean()
    rolling100 = rewards.rolling(window=100, min_periods=20).mean()

    bins = ((steps - 1) // 25) * 25 + 1
    binned = (
        pd.DataFrame({"bin_start": bins, "reward": rewards})
        .groupby("bin_start", as_index=False)["reward"]
        .mean()
    )
    binned["bin_center"] = binned["bin_start"] + 12

    first150 = rewards.head(150).mean()
    last150 = rewards.tail(150).mean()
    first100 = rewards.head(100).mean()
    last100 = rewards.tail(100).mean()
    overall = rewards.mean()
    reward1 = sum(1 for item in results if item.get("reward") in (1, 1.0))
    result_count = len(results)
    reward1_rate = reward1 / result_count * 100 if result_count else 0.0

    fig, ax = plt.subplots(figsize=(10.5, 5.2))
    ax.scatter(steps, rewards, s=10, color="#9ecae9", alpha=0.22, linewidths=0, label="step reward")
    ax.plot(binned["bin_center"], binned["reward"], color="#4c78a8", linewidth=1.8, marker="o", markersize=3.2, label="25-step bin mean")
    ax.plot(steps, rolling50, color="#f58518", linewidth=2.0, label="50-step moving avg")
    ax.plot(steps, rolling100, color="#d62728", linewidth=2.4, label="100-step moving avg")
    ax.axhline(overall, color="#444", linestyle="--", linewidth=1.2, label=f"overall mean {overall:.3f}")

    ax.set_title("SWE-smith GRPO reward signal")
    ax.set_xlabel("global step")
    ax.set_ylabel("reward mean")
    ax.set_ylim(-0.05, 1.05)
    ax.set_xlim(0, steps.max() + 12)
    ax.legend(loc="upper left", ncol=2, frameon=True)

    summary = (
        f"{full_label}\n"
        f"first 100 avg {first100:.3f} -> last 100 avg {last100:.3f}\n"
        f"first 150 avg {first150:.3f} -> last 150 avg {last150:.3f}\n"
        f"reward=1 episodes {reward1:,}/{result_count:,} ({reward1_rate:.1f}%)"
    )
    ax.text(
        0.985,
        0.035,
        summary,
        transform=ax.transAxes,
        ha="right",
        va="bottom",
        fontsize=9,
        color="#333",
        bbox=dict(boxstyle="round,pad=0.38", facecolor="white", edgecolor="#ddd", alpha=0.92),
    )

    fig.tight_layout()
    fig.savefig(OUT_DIR / "reward_curve_review.png", bbox_inches="tight")
    plt.close(fig)


def save_step_breakdown(df: pd.DataFrame, full_label: str) -> None:
    components = {
        "gen": df["timing_s/gen"].mean(),
        "old_log_prob": df["timing_s/old_log_prob"].mean(),
        "ref": df["timing_s/ref"].mean(),
        "update_actor": df["timing_s/update_actor"].mean(),
        "update_weights": df["timing_s/update_weights"].mean(),
    }
    step_mean = df["timing_s/step"].mean()
    components["other"] = max(0.0, step_mean - sum(components.values()))

    order = ["gen", "old_log_prob", "ref", "update_actor", "update_weights", "other"]
    colors = ["#4c78a8", "#9ecae9", "#72b7b2", "#f58518", "#e45756", "#bab0ab"]

    fig, ax = plt.subplots(figsize=(10, 3.6))
    left = 0.0
    for name, color in zip(order, colors):
        value = components[name]
        ax.barh([0], [value], left=left, color=color, edgecolor="white", height=0.55, label=name)
        if value >= 8:
            text_color = "white" if name != "other" else "#333"
            ax.text(left + value / 2, 0, f"{value:.1f}s", ha="center", va="center", fontsize=9, color=text_color)
        left += value

    ax.axvline(step_mean, color="#222", linestyle="--", linewidth=1.2, label=f"step mean {step_mean:.1f}s")
    ax.set_yticks([])
    ax.set_xlabel("seconds")
    ax.set_title(f"Mean time per training step ({len(df)} steps)")
    ax.legend(loc="upper right", ncol=2, frameon=False)
    ax.text(0.01, -0.28, f"{full_label}; gen is the dominant component.", transform=ax.transAxes, ha="left", va="top", fontsize=9, color="#444")
    fig.tight_layout()
    fig.savefig(OUT_DIR / "step_breakdown.png", bbox_inches="tight")
    plt.close(fig)


def save_episode_flow(requests: list[dict], results: list[dict], full_label: str) -> None:
    status_counts: dict[str, int] = {}
    reward_counts: dict[float, int] = {}
    for obj in results:
        status = str(obj.get("status", "unknown"))
        status_counts[status] = status_counts.get(status, 0) + 1
        reward = obj.get("reward")
        if reward is not None:
            reward_counts[float(reward)] = reward_counts.get(float(reward), 0) + 1

    completed = status_counts.get("completed", 0)
    failed = status_counts.get("failed", 0)
    reward1 = reward_counts.get(1.0, 0)

    labels = ["requests", "results", "completed", "failed", "reward=1"]
    values = [len(requests), len(results), completed, failed, reward1]

    fig, ax = plt.subplots(figsize=(8.8, 4.8))
    y = np.arange(len(labels))
    colors = ["#4c78a8", "#72b7b2", "#54a24b", "#e45756", "#f58518"]
    ax.barh(y, values, color=colors, height=0.6)
    for yi, value in zip(y, values):
        ax.text(value + max(values) * 0.015, yi, f"{value:,}", va="center", ha="left", fontsize=10)

    completion_rate = completed / len(results) * 100 if results else 0.0
    reward_rate = reward1 / len(results) * 100 if results else 0.0
    ax.set_yticks(y, labels)
    ax.invert_yaxis()
    ax.set_xlabel("count")
    ax.set_title("Episode flow summary")
    ax.set_xlim(0, max(values) * 1.18)
    ax.text(
        0.99,
        0.03,
        f"{full_label}; completed/result {completion_rate:.1f}% | reward=1/result {reward_rate:.1f}%",
        transform=ax.transAxes,
        ha="right",
        va="bottom",
        fontsize=9,
        color="#444",
    )
    fig.tight_layout()
    fig.savefig(OUT_DIR / "episode_flow.png", bbox_inches="tight")
    plt.close(fig)


def save_cumulative_training_progress(df: pd.DataFrame, results: list[dict]) -> None:
    """Save a monotonic progress chart with throughput on a twin axis."""

    step_re = re.compile(r"step-(\d+)-")
    rows = []
    for item in results:
        batch_id = item.get("batch_id") or ""
        match = step_re.search(batch_id)
        if not match:
            continue
        rows.append(
            {
                "step": int(match.group(1)),
                "completed": 1 if item.get("status") == "completed" else 0,
                "failed": 1 if item.get("status") == "failed" else 0,
                "reward1": 1 if item.get("reward") in (1, 1.0) else 0,
            }
        )

    result_df = pd.DataFrame(rows)
    if result_df.empty:
        raise RuntimeError("no episode results parsed")

    grouped = (
        result_df.groupby("step", as_index=False)
        .agg(completed=("completed", "sum"), failed=("failed", "sum"), reward1=("reward1", "sum"))
        .sort_values("step")
    )
    grouped["episodes_returned"] = grouped["completed"] + grouped["failed"]
    grouped["cum_returned"] = grouped["episodes_returned"].cumsum()
    grouped["cum_completed"] = grouped["completed"].cumsum()
    grouped["cum_reward1"] = grouped["reward1"].cumsum()

    throughput = (
        df[["training/global_step", "perf/throughput"]]
        .dropna()
        .assign(step=lambda frame: frame["training/global_step"].astype(int))
        .groupby("step", as_index=False)["perf/throughput"]
        .mean()
        .rename(columns={"perf/throughput": "throughput"})
        .sort_values("step")
    )
    grouped = grouped.merge(throughput, on="step", how="left")
    grouped["throughput"] = grouped["throughput"].interpolate(limit_direction="both")
    grouped["throughput_ma20"] = grouped["throughput"].rolling(window=20, min_periods=5).mean()

    total_returned = int(grouped["cum_returned"].iloc[-1])
    total_completed = int(grouped["cum_completed"].iloc[-1])
    total_reward1 = int(grouped["cum_reward1"].iloc[-1])
    complete_rate = total_completed / total_returned * 100 if total_returned else 0
    reward1_rate = total_reward1 / total_returned * 100 if total_returned else 0
    throughput_mean = float(throughput["throughput"].mean())
    throughput_first100 = float(throughput["throughput"].head(100).mean())
    throughput_last100 = float(throughput["throughput"].tail(100).mean())
    throughput_ma20_max = float(grouped["throughput_ma20"].max())
    throughput_max = float(grouped["throughput"].max())

    fig, ax = plt.subplots(figsize=(10.5, 5.2))
    # ax.plot(grouped["step"], grouped["cum_returned"], color="#64748B", linewidth=2.0, label="returned episodes")
    ax.plot(grouped["step"], grouped["cum_completed"], color="#059669", linewidth=2.5, label="completed episodes")
    ax.plot(grouped["step"], grouped["cum_reward1"], color="#F59E0B", linewidth=2.5, label="reward=1 episodes")
    ax.fill_between(grouped["step"], grouped["cum_reward1"], color="#F59E0B", alpha=0.10)
    ax.fill_between(grouped["step"], grouped["cum_completed"], color="#059669", alpha=0.08)

    ax2 = ax.twinx()
    ax2.plot(
        grouped["step"],
        grouped["throughput_ma20"],
        color="#2563EB",
        linewidth=2.1,
        label="throughput (20-step avg)",
    )
    ax2.set_ylabel("throughput (tokens/s/GPU)")
    ax2.set_ylim(0, max(throughput_ma20_max, throughput_max) * 1.12)

    ax.set_title("SWE training progress")
    ax.set_xlabel("global step")
    ax.set_ylabel("cumulative episode count")
    ax.set_xlim(0, grouped["step"].max() + 8)
    ax.set_ylim(0, max(total_returned, total_completed) * 1.08)
    handles1, labels1 = ax.get_legend_handles_labels()
    handles2, labels2 = ax2.get_legend_handles_labels()
    ax.legend(handles1 + handles2, labels1 + labels2, loc="upper left", ncol=2, frameon=True)

    # summary = (
    #     f"step 1-{int(grouped['step'].max())}\n"
    #     f"completed {total_completed:,}/{total_returned:,} ({complete_rate:.1f}%)\n"
    #     f"reward=1 {total_reward1:,}/{total_returned:,} ({reward1_rate:.1f}%)\n"
    #     f"throughput {throughput_mean:.2f} step/s\n"
    #     f"first 100 {throughput_first100:.2f} -> last 100 {throughput_last100:.2f} step/s"
    # )
    # ax.text(
    #     0.97,
    #     0.08,
    #     summary,
    #     transform=ax.transAxes,
    #     ha="right",
    #     va="bottom",
    #     fontsize=10,
    #     color="#111827",
    #     bbox=dict(boxstyle="round,pad=0.42", facecolor="white", edgecolor="#CBD5E1", alpha=0.95),
    # )
    fig.tight_layout()
    fig.savefig(OUT_DIR / "training_progress_cumulative.png", bbox_inches="tight")
    plt.close(fig)


def save_gateway_stability(gateway_rows: list[dict]) -> None:
    latencies = []
    status_codes = []
    for obj in gateway_rows:
        if "latency_ms" in obj:
            latencies.append(float(obj["latency_ms"]))
        if "status_code" in obj:
            status_codes.append(str(obj["status_code"]))

    status_series = pd.Series(status_codes).value_counts().sort_index()
    latency_array = np.array(latencies, dtype=float)

    fig, (ax1, ax2) = plt.subplots(1, 2, figsize=(11.5, 4.4))
    ax1.bar(status_series.index.astype(str), status_series.values, color="#4c78a8")
    ax1.set_yscale("log")
    for x_value, value in zip(status_series.index.astype(str), status_series.values):
        ax1.text(x_value, value * 1.15, f"{value:,}", ha="center", va="bottom", fontsize=9)
    ax1.set_title("Gateway status codes")
    ax1.set_xlabel("status code")
    ax1.set_ylabel("count, log scale")

    ax2.hist(latency_array, bins=50, color="#72b7b2", alpha=0.85, edgecolor="white")
    p50, p90, p99 = np.percentile(latency_array, [50, 90, 99])
    for percentile, color in [(p50, "#d62728"), (p90, "#f58518"), (p99, "#54a24b")]:
        ax2.axvline(percentile, color=color, linestyle="--", linewidth=1.4)
    ax2.set_title("Gateway latency distribution")
    ax2.set_xlabel("latency (ms)")
    ax2.set_ylabel("count")
    ax2.text(
        0.98,
        0.98,
        f"p50 {p50:.0f} ms\np90 {p90:.0f} ms\np99 {p99:.0f} ms",
        transform=ax2.transAxes,
        ha="right",
        va="top",
        fontsize=9,
        bbox=dict(boxstyle="round,pad=0.35", facecolor="white", edgecolor="#ddd"),
    )
    fig.suptitle("Gateway stability", y=1.02, fontsize=14)
    fig.tight_layout()
    fig.savefig(OUT_DIR / "gateway_stability.png", bbox_inches="tight")
    plt.close(fig)


def save_response_length_clip(df: pd.DataFrame, full_label: str) -> None:
    fig, ax1 = plt.subplots(figsize=(10, 4.8))
    ax1.plot(df["training/global_step"], df["response_length/mean"], color="#4c78a8", linewidth=1.6, label="response length mean")
    ax1.set_xlabel("global step")
    ax1.set_ylabel("response length")
    ax1.set_title("Response length and clip ratio")

    ax2 = ax1.twinx()
    ax2.plot(df["training/global_step"], df["response_length/clip_ratio"], color="#e45756", linewidth=1.5, alpha=0.95, label="clip ratio")
    ax2.set_ylabel("clip ratio")
    ax2.set_ylim(-0.02, 1.02)

    ax1.text(
        0.01,
        0.03,
        f"{full_label}; response length mean {df['response_length/mean'].mean():.0f}; p90 {df['response_length/mean'].quantile(0.9):.0f}; clip ratio mean {df['response_length/clip_ratio'].mean():.3f}",
        transform=ax1.transAxes,
        ha="left",
        va="bottom",
        fontsize=9,
        color="#444",
    )
    lines1, labels1 = ax1.get_legend_handles_labels()
    lines2, labels2 = ax2.get_legend_handles_labels()
    ax1.legend(lines1 + lines2, labels1 + labels2, loc="upper left")
    fig.tight_layout()
    fig.savefig(OUT_DIR / "response_length_clip.png", bbox_inches="tight")
    plt.close(fig)


def main() -> None:
    setup_matplotlib()
    df = build_step_dataframe()
    requests, results = load_episode_flow()
    gateway_rows = load_gateway_rows()

    full_label = f"full training so far: step {int(df['training/global_step'].min())}-{int(df['training/global_step'].max())}"

    save_reward_curve(df, full_label)
    save_reward_curve_review(df, full_label, results)
    save_step_breakdown(df, full_label)
    save_episode_flow(requests, results, full_label)
    save_cumulative_training_progress(df, results)
    save_gateway_stability(gateway_rows)
    save_response_length_clip(df, full_label)

    completed = sum(1 for item in results if item.get("status") == "completed")
    failed = sum(1 for item in results if item.get("status") == "failed")
    reward1 = sum(1 for item in results if item.get("reward") in (1, 1.0))
    status_200 = sum(1 for item in gateway_rows if str(item.get("status_code")) == "200")

    print(f"generated charts in {OUT_DIR}")
    print(f"steps: {int(df['training/global_step'].min())}-{int(df['training/global_step'].max())} ({len(df)})")
    print(f"episode: requests={len(requests)}, results={len(results)}, completed={completed}, failed={failed}, reward1={reward1}")
    print(f"gateway: total={len(gateway_rows)}, status_200={status_200}")


if __name__ == "__main__":
    main()
