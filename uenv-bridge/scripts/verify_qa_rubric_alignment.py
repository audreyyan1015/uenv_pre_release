#!/usr/bin/env python3
"""qa 环境判分金标对齐：UEnv 生产判分 ↔ 公开 `verifiers` Rubric。

要证明的事：`plugins/qa`（复用 `plugins/math` 的 Rust `score_action`）给出的 reward，
与公开可复现的判分口径一致；不一致的地方必须能逐条解释。

**参照口径本体在 `qa_rubric.py`**，本脚本只负责「拿它去量」并出报告。二者分开的
原因是分发：Hub 把 `qa_rubric.py` 作为 `rubric_scorer` 制品托管、用 sha256 钉住
（`RubricSpec.reference_scorer`），因此「报告里那个对齐率是用哪套规则量出来的」
可以被字节校验。如果规则内嵌在本脚本里，Hub 能托管的就只有报告，而报告本身
无法证明它依据的规则是什么。同理，本脚本必须 import 规则包而不是复制一份，
否则两处会各自演化，报告声明的口径与 Hub 分发的口径就会悄悄分叉。

判分口径见 `qa_rubric.py` 模块文档（抽取按 benchmark 官方约定，等价性判定交给公开库）。

用法（在装有 verifiers/math-verify 的机器上，如 7143 的 /opt/uenv-gold-venv）：
    # 1) 用生产 Rust 判分给语料打分
    cargo run -q -p uenv-math-env --example score_corpus -- \\
        data/alignment/qa_rubric_corpus.jsonl > /tmp/uenv_scores.jsonl
    # 2) 跑参照口径并出对齐报告
    /opt/uenv-gold-venv/bin/python uenv-bridge/scripts/verify_qa_rubric_alignment.py \\
        --uenv-scores /tmp/uenv_scores.jsonl \\
        --output-dir temp/alignment/qa_rubric

规则包若从 Hub 同步到别处，用 `--rubric-dir` 指向它所在目录：
    uenv env rubric fetch-scorer qa --version latest --target-dir /opt/uenv/rubric
    … verify_qa_rubric_alignment.py --rubric-dir /opt/uenv/rubric …
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
import time
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]

#: 金标规则包文件名。同步自 Hub (`uenv env rubric fetch-scorer`) 时目录不同，文件名不变。
RUBRIC_MODULE_FILE = "qa_rubric.py"


def load_rubric_module(rubric_dir: Path):
    """加载金标规则包，并返回 (module, 源码 sha256)。

    返回 digest 而不只是 module：报告里要写明「这次是用哪份规则字节量出来的」，
    否则对齐率无法与 Hub 上 `RubricSpec.reference_scorer.digest` 对上。
    """
    path = rubric_dir / RUBRIC_MODULE_FILE
    if not path.is_file():
        raise SystemExit(
            f"gold-standard rubric package not found: {path}\n"
            f"  fetch it from the Hub: uenv env rubric fetch-scorer qa --version latest "
            f"--target-dir <dir>, then pass --rubric-dir <dir>"
        )
    digest = "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest()
    if str(rubric_dir) not in sys.path:
        sys.path.insert(0, str(rubric_dir))
    import importlib

    module = importlib.import_module(RUBRIC_MODULE_FILE[: -len(".py")])
    return module, digest


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
    lines.append(
        f"> 金标规则包：`{RUBRIC_MODULE_FILE}` `{metrics['rubric_module_digest']}`  "
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
    p.add_argument(
        "--rubric-dir",
        default=str(Path(__file__).resolve().parent),
        help=f"金标规则包 {RUBRIC_MODULE_FILE} 所在目录（默认与本脚本同目录；"
        f"从 Hub 同步后指向 <target-dir>/rubric）",
    )
    p.add_argument("--min-agreement", type=float, default=0.95, help="低于该对齐率则返回非 0")
    p.add_argument(
        "--max-over-credit",
        type=int,
        default=0,
        help="允许的『UEnv 判对而参照判错』条数；默认 0（过宽判分是 reward hacking 入口）",
    )
    args = p.parse_args()

    import importlib.metadata as md

    rubric_module, rubric_digest = load_rubric_module(Path(args.rubric_dir))
    print(f"gold-standard rubric package: {args.rubric_dir}/{RUBRIC_MODULE_FILE} {rubric_digest}")

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

    scorer = rubric_module.ReferenceScorer()
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
        # 报告自带规则包 digest：`uenv env rubric import` 会把它写进
        # `RubricSpec.reference_scorer.digest`，于是「声明的规则」与「量出这个
        # 对齐率的规则」不可能互相打架。
        "rubric_module": RUBRIC_MODULE_FILE,
        "rubric_module_digest": rubric_digest,
        "rubric_entrypoint": f"{RUBRIC_MODULE_FILE[: -len('.py')]}:score",
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
