from __future__ import annotations

import json
import re
from collections import Counter
from dataclasses import dataclass
from pathlib import Path
from typing import Any


LABELS = ("yes", "no", "maybe")


@dataclass(slots=True)
class Example:
    qid: str
    question: str
    contexts: list[str]
    answer: str


def load_pubmedqa(path: Path, *, limit: int | None = None) -> list[Example]:
    data = json.loads(path.read_text(encoding="utf-8"))
    examples = []
    for qid, item in data.items():
        answer = str(item["final_decision"]).strip().lower()
        if answer not in LABELS:
            raise ValueError(f"unsupported label for {qid}: {answer}")
        examples.append(
            Example(
                qid=str(qid),
                question=str(item["QUESTION"]).strip(),
                contexts=[str(text).strip() for text in item["CONTEXTS"]],
                answer=answer,
            )
        )
    if limit is not None:
        examples = examples[:limit]
    return examples


def build_prompt(example: Example, *, prompt_style: str = "default") -> str:
    context = "\n".join(f"[{i + 1}] {text}" for i, text in enumerate(example.contexts))
    if prompt_style == "strict_label":
        return (
            "Read the abstract context and answer the biomedical question.\n"
            "Do not explain. Do not provide reasoning. Output exactly one lowercase word from this set: yes, no, maybe.\n\n"
            f"Context:\n{context}\n\n"
            f"Question: {example.question}\n\n"
            "Answer:"
        )
    if prompt_style == "thinking_label":
        return (
            "Read the abstract context and answer the biomedical question.\n"
            "First write a concise reasoning process inside <think>...</think>.\n"
            "After </think>, output only the final answer as exactly one lowercase word from this set: yes, no, maybe.\n"
            "Do not write any other text outside the think block.\n\n"
            f"Context:\n{context}\n\n"
            f"Question: {example.question}\n\n"
            "Answer:"
        )
    if prompt_style in {"default", "official"}:
        return (
            "Read the abstract context and answer the biomedical question with exactly one label: yes, no, or maybe.\n\n"
            f"Context:\n{context}\n\n"
            f"Question: {example.question}\n\n"
            "Return only one word: yes, no, or maybe."
        )
    raise ValueError(f"unknown prompt_style: {prompt_style}")


def parse_label(text: str) -> str | None:
    normalized = text.strip().lower()
    normalized = normalized.replace("**", "").replace("`", "")
    if "</think>" in normalized:
        normalized = normalized.split("</think>")[-1]
    matches = re.findall(r"\b(yes|no|maybe)\b", normalized)
    return matches[-1] if matches else None


def safe_div(num: float, den: float) -> float:
    return num / den if den else 0.0


def compute_metrics(rows: list[dict[str, Any]]) -> dict[str, Any]:
    total = len(rows)
    parsed_rows = [row for row in rows if row["pred"] in LABELS]
    correct = sum(1 for row in rows if row["pred"] == row["gold"])
    parsed_correct = sum(1 for row in parsed_rows if row["pred"] == row["gold"])

    confusion = {gold: {pred: 0 for pred in (*LABELS, "unparsed")} for gold in LABELS}
    for row in rows:
        pred = row["pred"] if row["pred"] in LABELS else "unparsed"
        confusion[row["gold"]][pred] += 1

    per_class = {}
    f1_values = []
    for label in LABELS:
        tp = confusion[label][label]
        fp = sum(confusion[gold][label] for gold in LABELS if gold != label)
        fn = sum(confusion[label][pred] for pred in (*LABELS, "unparsed") if pred != label)
        precision = safe_div(tp, tp + fp)
        recall = safe_div(tp, tp + fn)
        f1 = safe_div(2 * precision * recall, precision + recall)
        support = sum(confusion[label].values())
        per_class[label] = {
            "precision": precision,
            "recall": recall,
            "f1": f1,
            "support": support,
        }
        f1_values.append(f1)

    return {
        "sample_count": total,
        "parsed_count": len(parsed_rows),
        "unparsed_count": total - len(parsed_rows),
        "parse_rate": safe_div(len(parsed_rows), total),
        "accuracy": safe_div(correct, total),
        "parsed_accuracy": safe_div(parsed_correct, len(parsed_rows)),
        "macro_f1": sum(f1_values) / len(f1_values),
        "label_distribution": dict(Counter(row["gold"] for row in rows)),
        "prediction_distribution": dict(Counter(row["pred"] if row["pred"] in LABELS else "unparsed" for row in rows)),
        "per_class": per_class,
        "confusion": confusion,
    }
