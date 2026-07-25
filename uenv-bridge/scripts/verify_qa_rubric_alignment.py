#!/usr/bin/env python3
"""qa 环境判分金标对齐：UEnv 生产判分 ↔ 公开 `verifiers` Rubric。

要证明的事：`plugins/qa`（复用 `plugins/math` 的 Rust `score_action`）给出的 reward，
与公开可复现的判分口径一致；不一致的地方必须能逐条解释。

参照口径（按 benchmark 官方约定选择抽取器，等价性判定统一交给公开库）：

| dataset | 抽取（benchmark 官方约定） | 等价判定（公开库） |
|---|---|---|
| gsm8k | 最后一个 `####` 之后的内容，无标记则取全文 | `math_verify.verify`（`verifiers.MathRubric` 内核） |
| olymmath[-easy/-hard] | `verifiers` 的 `extract_boxed_answer`，无 boxed 回退 `####` | 同上 |
| pubmedqa | 官方三分类标签 yes/no/maybe，取最后出现的整词 | `verifiers.Rubric` 上的 label_match reward func |
| scitab | 官方三分类 supports/refutes/not enough info，取最后出现的整词 | 同上 |

抽取必须按各 benchmark 官方约定，而不是照搬 `MathRubric` 的默认 boxed-only parser：
GSM8K 的官方答案标记是 `####`，用 boxed-only parser 会把所有 GSM8K 样本判 0，
那样的“对齐率”没有意义。等价性判定则完全交给公开库，不自研。

用法（在装有 verifiers/math-verify 的机器上，如 7143 的 /opt/uenv-gold-venv）：
    # 1) 用生产 Rust 判分给语料打分
    cargo run -q -p uenv-math-env --example score_corpus -- \\
        data/alignment/qa_rubric_corpus.jsonl > /tmp/uenv_scores.jsonl
    # 2) 跑参照口径并出对齐报告
    /opt/uenv-gold-venv/bin/python uenv-bridge/scripts/verify_qa_rubric_alignment.py \\
        --uenv-scores /tmp/uenv_scores.jsonl \\
        --output-dir temp/alignment/qa_rubric
"""

from __future__ import annotations

import argparse
import asyncio
import json
import re
import sys
import time
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]

PUBMEDQA_LABELS = ("yes", "no", "maybe")
SCITAB_LABELS = ("supports", "refutes", "not enough info")
# 官方标签的常见同义写法；归一到官方标签后再比较。
SCITAB_ALIASES = {
    "not enough information": "not enough info",
    "notenoughinfo": "not enough info",
    "support": "supports",
    "refute": "refutes",
}


# ---------------------------------------------------------------------------
# 抽取器（benchmark 官方约定）
# ---------------------------------------------------------------------------
def extract_gsm8k(text: str) -> str:
    """GSM8K 官方约定：`#### <answer>`；无标记时退回全文。"""
    if "####" in text:
        return text.rsplit("####", 1)[1].strip()
    return text.strip()


def extract_olymmath(text: str) -> str:
    """OlymMATH 约定：最后一个 `\\boxed{...}`；无 boxed 时回退 `####`，再回退全文。

    boxed 抽取直接用 `verifiers` 的公开实现，避免自研 parser。
    """
    from verifiers import extract_boxed_answer

    boxed = extract_boxed_answer(text, strict=False)
    if boxed:
        return boxed.strip()
    return extract_gsm8k(text)


def _last_label(text: str, labels: tuple[str, ...]) -> str | None:
    """取最后出现的整词标签（多词标签允许内部空白）。"""
    lowered = text.lower()
    best: tuple[int, str] | None = None
    for label in labels:
        pattern = r"\b" + r"\s+".join(re.escape(part) for part in label.split()) + r"\b"
        for match in re.finditer(pattern, lowered):
            if best is None or match.start() > best[0]:
                best = (match.start(), label)
    return best[1] if best else None


def extract_pubmedqa(text: str) -> str | None:
    return _last_label(text, PUBMEDQA_LABELS)


def extract_scitab(text: str) -> str | None:
    candidates = SCITAB_LABELS + tuple(SCITAB_ALIASES)
    label = _last_label(text, candidates)
    if label is None:
        return None
    return SCITAB_ALIASES.get(label, label)


# ---------------------------------------------------------------------------
# 参照判分：数学走 verifiers.MathRubric，分类走 verifiers.Rubric 上的 label_match
# ---------------------------------------------------------------------------
class ReferenceScorer:
    def __init__(self) -> None:
        from verifiers import Rubric
        from verifiers.rubrics.math_rubric import MathRubric

        self.math_rubric = MathRubric()
        # 分类任务：verifiers 没有内置三分类 Rubric，这里在公开 Rubric 上挂
        # 官方标签匹配 reward func，保持“判分函数注册在公开框架里”的形态。
        self.label_rubric = Rubric()
        self.label_rubric.add_reward_func(self._label_match)
        self._label_ctx: dict[str, Any] = {}

    async def _label_match(self, completion: Any, answer: str, **_kwargs: Any) -> float:
        dataset = self._label_ctx.get("dataset", "")
        text = self._label_ctx.get("text", "")
        extract = extract_pubmedqa if dataset == "pubmedqa" else extract_scitab
        predicted = extract(text)
        expected = extract(answer) or answer.strip().lower()
        return 1.0 if predicted is not None and predicted == expected else 0.0

    def score_math(self, action: str, target: str, dataset: str) -> float:
        extract = extract_gsm8k if dataset == "gsm8k" else extract_olymmath
        extracted = extract(action)
        if not extracted:
            return 0.0
        # 把抽取结果包成 boxed，交给 MathRubric 默认 parser + math_verify 判等价，
        # 这样等价性判定完全由公开库负责，我们只负责按官方约定抽取。
        completion = [{"role": "assistant", "content": f"\\boxed{{{extracted}}}"}]
        return float(
            asyncio.run(
                self.math_rubric.correct_answer(
                    parser=self.math_rubric.parser,
                    completion=completion,
                    answer=target,
                )
            )
        )

    def score_label(self, action: str, target: str, dataset: str) -> float:
        self._label_ctx = {"dataset": dataset, "text": action}
        completion = [{"role": "assistant", "content": action}]
        return float(asyncio.run(self._label_match(completion=completion, answer=target)))

    def score(self, dataset: str, action: str, target: str) -> float:
        if dataset in ("gsm8k",) or dataset.startswith("olymmath"):
            return self.score_math(action, target, "gsm8k" if dataset == "gsm8k" else "olymmath")
        if dataset in ("pubmedqa", "scitab"):
            return self.score_label(action, target, dataset)
        raise ValueError(f"unsupported dataset for reference scoring: {dataset}")


# ---------------------------------------------------------------------------
# 报告
# ---------------------------------------------------------------------------
def render_report(metrics: dict[str, Any]) -> str:
    lines: list[str] = []
    lines.append("# qa 环境判分金标对齐报告")
    lines.append("")
    lines.append(f"> 生成时间：{time.strftime('%Y-%m-%d %H:%M:%S')}  ")
    lines.append(f"> 语料：`{metrics['corpus']}`（{metrics['total']} 例）  ")
    lines.append(
        f"> 参照实现：`verifiers {metrics['verifiers_version']}` + `math-verify {metrics['math_verify_version']}`  "
    )
    lines.append("> UEnv 侧：`plugins/math` 的 `score_action`（`plugins/qa` 通过 run.sh 复用同一二进制）")
    lines.append("")
    lines.append("## 对齐口径")
    lines.append("")
    lines.append("| dataset | 抽取（benchmark 官方约定） | 等价判定（公开库） |")
    lines.append("|---|---|---|")
    lines.append("| gsm8k | 最后一个 `####` 之后；无标记取全文 | `MathRubric.correct_answer` → `math_verify` |")
    lines.append("| olymmath[-easy/-hard] | `verifiers.extract_boxed_answer`，回退 `####` | 同上 |")
    lines.append("| pubmedqa | 官方标签 yes/no/maybe，取最后整词 | `verifiers.Rubric` + label_match |")
    lines.append("| scitab | 官方标签 supports/refutes/not enough info，取最后整词 | 同上 |")
    lines.append("")
    lines.append("## 总体对齐率")
    lines.append("")
    lines.append("| 指标 | 值 |")
    lines.append("|---|---|")
    lines.append(f"| 一致 / 总数 | {metrics['agreed']} / {metrics['total']} |")
    lines.append(f"| **对齐率** | **{metrics['agreement_rate'] * 100:.2f}%** |")
    lines.append(f"| 过宽（UEnv 判对、参照判错） | **{metrics['over_credit_count']}** |")
    lines.append(f"| 过严（UEnv 判错、参照判对） | {metrics['under_credit_count']} |")
    lines.append("")
    lines.append(
        "两类不一致的性质不同：**过宽**意味着策略可以用不正确的输出骗到 reward，"
        "属于必须修的缺陷（门槛为 0）；**过严**只会漏判正确答案，损失召回但不会被策略利用，"
        "按 benchmark 官方约定取舍即可。"
    )
    lines.append("")
    lines.append("## 分 dataset")
    lines.append("")
    lines.append("| dataset | 一致 | 总数 | 对齐率 |")
    lines.append("|---|---|---|---|")
    for ds, stats in metrics["by_dataset"].items():
        lines.append(
            f"| {ds} | {stats['agreed']} | {stats['total']} | {stats['agreement_rate'] * 100:.2f}% |"
        )
    lines.append("")

    disagreements = metrics["disagreements"]
    lines.append(f"## 不一致明细（{len(disagreements)}）")
    lines.append("")
    if not disagreements:
        lines.append("无。")
    else:
        lines.append("| case_id | dataset | action | target | UEnv | 参照 | 类型 | 说明 |")
        lines.append("|---|---|---|---|---|---|---|---|")
        kind_label = {"over_credit": "过宽 ⚠️", "under_credit": "过严"}
        for row in disagreements:
            action = (row["action"] or "").replace("|", "/").replace("\n", "\\n")[:60]
            lines.append(
                f"| {row['case_id']} | {row['dataset']} | `{action}` | `{row['target']}` | "
                f"{row['uenv_reward']} | {row['reference_reward']} | "
                f"{kind_label.get(row.get('kind'), row.get('kind', ''))} | {row.get('note', '')} |"
            )
    lines.append("")
    lines.append("## 复现")
    lines.append("")
    lines.append("```bash")
    lines.append("cargo run -q -p uenv-math-env --example score_corpus -- \\")
    lines.append("  data/alignment/qa_rubric_corpus.jsonl > /tmp/uenv_scores.jsonl")
    lines.append("/opt/uenv-gold-venv/bin/python uenv-bridge/scripts/verify_qa_rubric_alignment.py \\")
    lines.append("  --uenv-scores /tmp/uenv_scores.jsonl --output-dir temp/alignment/qa_rubric")
    lines.append("```")
    lines.append("")
    return "\n".join(lines)


def main() -> int:
    p = argparse.ArgumentParser(description="qa 判分与公开 verifiers Rubric 对齐核对")
    p.add_argument("--corpus", default=str(ROOT / "data/alignment/qa_rubric_corpus.jsonl"))
    p.add_argument(
        "--uenv-scores",
        required=True,
        help="cargo example score_corpus 的输出 JSONL（含 uenv_reward）",
    )
    p.add_argument("--output-dir", default=str(ROOT / "temp/alignment/qa_rubric"))
    p.add_argument("--min-agreement", type=float, default=0.95, help="低于该对齐率则返回非 0")
    p.add_argument(
        "--max-over-credit",
        type=int,
        default=0,
        help="允许的『UEnv 判对而参照判错』条数；默认 0（过宽判分是 reward hacking 入口）",
    )
    args = p.parse_args()

    import importlib.metadata as md

    corpus = {}
    for line in Path(args.corpus).read_text(encoding="utf-8").splitlines():
        if line.strip():
            row = json.loads(line)
            corpus[row["case_id"]] = row

    scored = []
    for line in Path(args.uenv_scores).read_text(encoding="utf-8").splitlines():
        if line.strip():
            scored.append(json.loads(line))
    if len(scored) != len(corpus):
        print(f"WARN: uenv scores {len(scored)} != corpus {len(corpus)}", file=sys.stderr)

    scorer = ReferenceScorer()
    rows: list[dict[str, Any]] = []
    for row in scored:
        case_id = row["case_id"]
        dataset = row["dataset"]
        action = row.get("action", "")
        target = row.get("target", "")
        reference = scorer.score(dataset, action, target)
        uenv = float(row.get("uenv_reward", 0.0))
        if uenv == reference:
            kind = "agree"
        elif uenv > reference:
            # UEnv 判对、参照判错：过宽，是 reward hacking 入口，必须修
            kind = "over_credit"
        else:
            # UEnv 判错、参照判对：过严，只损失召回，不会被策略利用
            kind = "under_credit"
        rows.append(
            {
                "case_id": case_id,
                "dataset": dataset,
                "action": action,
                "target": target,
                "uenv_reward": uenv,
                "reference_reward": reference,
                "agree": uenv == reference,
                "kind": kind,
                "note": corpus.get(case_id, {}).get("note", ""),
            }
        )

    by_dataset: dict[str, dict[str, Any]] = {}
    for row in rows:
        stats = by_dataset.setdefault(row["dataset"], {"agreed": 0, "total": 0})
        stats["total"] += 1
        stats["agreed"] += 1 if row["agree"] else 0
    for stats in by_dataset.values():
        stats["agreement_rate"] = stats["agreed"] / stats["total"] if stats["total"] else 0.0

    agreed = sum(1 for row in rows if row["agree"])
    over_credit = [row for row in rows if row["kind"] == "over_credit"]
    under_credit = [row for row in rows if row["kind"] == "under_credit"]
    metrics = {
        "corpus": args.corpus,
        "verifiers_version": md.version("verifiers"),
        "math_verify_version": md.version("math-verify"),
        "total": len(rows),
        "agreed": agreed,
        "agreement_rate": agreed / len(rows) if rows else 0.0,
        "over_credit_count": len(over_credit),
        "under_credit_count": len(under_credit),
        "by_dataset": {ds: by_dataset[ds] for ds in sorted(by_dataset)},
        "disagreements": [row for row in rows if not row["agree"]],
        "results": rows,
    }

    out_dir = Path(args.output_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    (out_dir / "metrics.json").write_text(json.dumps(metrics, ensure_ascii=False, indent=2), encoding="utf-8")
    (out_dir / "report.md").write_text(render_report(metrics), encoding="utf-8")

    print(json.dumps({k: v for k, v in metrics.items() if k not in ("results", "disagreements")}, ensure_ascii=False, indent=2))
    for row in metrics["disagreements"]:
        print(
            f"DISAGREE {row['case_id']} [{row['dataset']}] uenv={row['uenv_reward']} ref={row['reference_reward']} "
            f"action={row['action']!r} target={row['target']!r} note={row['note']}"
        )
    print(f"report: {out_dir / 'report.md'}")
    # 门槛：过宽（UEnv 判对而参照判错）必须为 0；过严只记账不阻断。
    ok = metrics["agreement_rate"] >= args.min_agreement and metrics["over_credit_count"] <= args.max_over_credit
    print(
        f"agreement_rate={metrics['agreement_rate']:.4f} (threshold={args.min_agreement}) "
        f"over_credit={metrics['over_credit_count']} (max={args.max_over_credit}) "
        f"under_credit={metrics['under_credit_count']} ok={ok}"
    )
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
