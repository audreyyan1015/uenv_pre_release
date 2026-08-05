#!/usr/bin/env python3
"""DSCodeBench Agent 轨道（ToolEnv）报告生成：metrics.json → report.md。

只读输入：`--output-dir` 下由 dscode_toolenv_agent.py 产出的 metrics.json / results.jsonl。
可选 `--baseline-metrics` 传官方单轮轨道的 metrics json，仅做**并列展示**，
报告中显式标注两者不可直接比较（轨道定义、轮次预算、判分入口不同）。

用法：
  python3 report_dscode_agentic.py --output-dir temp/benchmarks/dscodebench-agentic/<run>
  python3 report_dscode_agentic.py --output-dir <dir> --baseline-metrics <official metrics.json>
"""

from __future__ import annotations

import argparse
import json
import time
from pathlib import Path
from typing import Any

TRACK_NOTE = (
    "Agent 轨道（agentic_pass@1）与官方单轮轨道（pass@1）**不可直接比较**："
    "Agent 允许多轮 run_python 自测与自修后再 submit_code，"
    "官方轨为一次生成即判分；两者仅共用同一官方 harness 与测试用例数。"
)


def _load_metrics(output_dir: Path) -> dict[str, Any]:
    path = output_dir / "metrics.json"
    if not path.exists():
        raise SystemExit(f"metrics.json not found: {path}")
    return json.loads(path.read_text(encoding="utf-8"))


def _baseline_summary(path: Path) -> dict[str, Any]:
    """兼容官方轨 metrics 的常见字段名，提取 pass@1 与题量。"""
    data = json.loads(path.read_text(encoding="utf-8"))
    pass_at_1 = None
    for key in ("pass_at_1", "pass@1", "pass_rate", "accuracy"):
        if isinstance(data.get(key), (int, float)):
            pass_at_1 = float(data[key])
            break
    total = data.get("problem_count") or data.get("total") or data.get("count")
    passed = data.get("passed_count") or data.get("passed")
    if pass_at_1 is None and isinstance(total, int) and total and isinstance(passed, int):
        pass_at_1 = passed / total
    return {
        "source": str(path),
        "pass_at_1": pass_at_1,
        "problem_count": total,
        "passed_count": passed,
        "model": data.get("model_name") or data.get("llm_model") or data.get("model") or "",
    }


def _fmt_pct(value: Any) -> str:
    if not isinstance(value, (int, float)):
        return "n/a"
    return f"{value * 100:.2f}%"


def render(metrics: dict[str, Any], baseline: dict[str, Any] | None) -> str:
    rows: list[dict[str, Any]] = metrics.get("results", [])
    lines: list[str] = []
    lines.append(f"# DSCodeBench Agent 轨道评测报告 — {metrics.get('run_name', 'unknown')}")
    lines.append("")
    lines.append(f"> 生成时间：{time.strftime('%Y-%m-%d %H:%M:%S')}  ")
    lines.append(f"> 轨道：`{metrics.get('track')}`　指标：`{metrics.get('metric')}`  ")
    lines.append(f"> {TRACK_NOTE}")
    lines.append("")
    lines.append("## 运行配置")
    lines.append("")
    lines.append("| 项 | 值 |")
    lines.append("|----|----|")
    for label, key in (
        ("Adapter Core", "endpoint"),
        ("策略", "policy"),
        ("模型", "llm_model"),
        ("模型端点", "llm_endpoint"),
        ("最大轮次", "max_turns"),
        ("判分模式", "evaluation_mode"),
        ("测试用例数", "num_tests"),
        ("数据集", "dataset"),
        ("库过滤", "library_filter"),
        ("总耗时(s)", "elapsed_seconds"),
    ):
        lines.append(f"| {label} | `{metrics.get(key, '')}` |")
    lines.append("")
    lines.append("## 总体结果")
    lines.append("")
    lines.append("| 指标 | 值 |")
    lines.append("|------|----|")
    lines.append(f"| agentic_pass@1 | **{_fmt_pct(metrics.get('agentic_pass_at_1'))}** |")
    lines.append(f"| 通过 / 总题数 | {metrics.get('passed_count')} / {metrics.get('problem_count')} |")
    lines.append(f"| episode completed | {metrics.get('completed_count')} |")
    lines.append(f"| 平均 ToolEnv 轮次 | {float(metrics.get('avg_toolenv_turns') or 0):.2f} |")
    lines.append(f"| 平均 run_python 次数 | {float(metrics.get('avg_run_python_calls') or 0):.2f} |")
    lines.append("")

    by_library = metrics.get("by_library") or {}
    if by_library:
        lines.append("## 分库结果")
        lines.append("")
        lines.append("| library | 通过 | 总数 | agentic_pass@1 |")
        lines.append("|---------|------|------|----------------|")
        for lib, stats in by_library.items():
            lines.append(
                f"| {lib} | {stats.get('passed')} | {stats.get('total')} | {_fmt_pct(stats.get('agentic_pass_at_1'))} |"
            )
        lines.append("")

    if baseline:
        lines.append("## 与官方单轮轨道并列（非可比对照）")
        lines.append("")
        lines.append("| 轨道 | 指标 | 值 | 通过/总数 | 模型 |")
        lines.append("|------|------|----|-----------|------|")
        lines.append(
            f"| Agent (ToolEnv) | agentic_pass@1 | {_fmt_pct(metrics.get('agentic_pass_at_1'))} | "
            f"{metrics.get('passed_count')}/{metrics.get('problem_count')} | {metrics.get('llm_model')} |"
        )
        lines.append(
            f"| 官方单轮 | pass@1 | {_fmt_pct(baseline.get('pass_at_1'))} | "
            f"{baseline.get('passed_count')}/{baseline.get('problem_count')} | {baseline.get('model')} |"
        )
        lines.append("")
        lines.append(f"> 官方轨来源：`{baseline.get('source')}`。{TRACK_NOTE}")
        lines.append("")

    failed = [r for r in rows if not r.get("passed")]
    if failed:
        lines.append(f"## 未通过明细（{len(failed)}）")
        lines.append("")
        lines.append("| problem_id | library | status | tests_passed/run | 轮次 | error |")
        lines.append("|------------|---------|--------|------------------|------|-------|")
        for r in failed[:200]:
            err = (str(r.get("worker_error") or "") or str(r.get("worker_detail") or ""))[:80].replace("|", "/")
            lines.append(
                f"| {r.get('problem_id')} | {r.get('library','')} | {r.get('status')} | "
                f"{r.get('tests_passed','')}/{r.get('tests_run','')} | {r.get('toolenv_turns')} | {err} |"
            )
        lines.append("")

    lines.append("## 产物布局")
    lines.append("")
    lines.append("```")
    lines.append("<output-dir>/")
    lines.append("  run_config.json     # 运行参数快照")
    lines.append("  results.jsonl       # 每题一行，支持 --resume 续跑")
    lines.append("  metrics.json        # 聚合指标（track=agentic）")
    lines.append("  report.md           # 本报告")
    lines.append("  codes/<pid>.py      # Agent 定稿代码")
    lines.append("  traces/<pid>.history.json  # ToolEnv 多轮轨迹")
    lines.append("  run.log             # 运行日志")
    lines.append("```")
    lines.append("")
    return "\n".join(lines)


def main() -> int:
    p = argparse.ArgumentParser(description="DSCodeBench agentic 轨道报告生成")
    p.add_argument("--output-dir", required=True)
    p.add_argument("--baseline-metrics", default=None, help="官方单轮轨 metrics json（仅并列展示）")
    p.add_argument("--stdout", action="store_true", help="同时打印报告到 stdout")
    args = p.parse_args()

    out_dir = Path(args.output_dir)
    metrics = _load_metrics(out_dir)
    baseline = _baseline_summary(Path(args.baseline_metrics)) if args.baseline_metrics else None
    report = render(metrics, baseline)
    target = out_dir / "report.md"
    target.write_text(report, encoding="utf-8")
    if args.stdout:
        print(report)
    print(f"report: {target}")
    print(
        json.dumps(
            {
                "track": metrics.get("track"),
                "metric": metrics.get("metric"),
                "agentic_pass_at_1": metrics.get("agentic_pass_at_1"),
                "passed_count": metrics.get("passed_count"),
                "problem_count": metrics.get("problem_count"),
            },
            ensure_ascii=False,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
