from __future__ import annotations

import json
from collections import Counter
from dataclasses import dataclass
from pathlib import Path
from typing import Any


LABELS = ("supports", "refutes", "not enough info")
SCITAB_PHRASES = (
    ("not enough info", "not enough info"),
    ("not enough info", "not enough information"),
    ("not enough info", "insufficient information"),
    ("not enough info", "insufficient evidence"),
)
SCITAB_WORDS = (
    ("supports", "supports"),
    ("supports", "support"),
    ("supports", "supported"),
    ("refutes", "refutes"),
    ("refutes", "refute"),
    ("refutes", "refuted"),
    ("not enough info", "nei"),
)


@dataclass(slots=True)
class Example:
    qid: str
    paper: str
    paper_id: str
    table_caption: str
    table_column_names: list[str]
    table_content_values: list[list[str]]
    claim: str
    label: str
    table_id: str


def load_scitab(path: Path, *, limit: int | None = None) -> list[Example]:
    data = json.loads(path.read_text(encoding="utf-8"))
    examples = []
    for item in data:
        label = str(item["label"]).strip().lower()
        if label not in LABELS:
            raise ValueError(f"unsupported label for {item.get('id')}: {label}")
        examples.append(
            Example(
                qid=str(item["id"]),
                paper=str(item.get("paper", "")).strip(),
                paper_id=str(item.get("paper_id", "")).strip(),
                table_caption=str(item.get("table_caption", "")).strip(),
                table_column_names=[str(value).strip() for value in item["table_column_names"]],
                table_content_values=[
                    [str(value).strip() for value in row] for row in item["table_content_values"]
                ],
                claim=str(item["claim"]).strip(),
                label=label,
                table_id=str(item.get("table_id", "")).strip(),
            )
        )
    if limit is not None:
        examples = examples[:limit]
    return examples


def table_to_markdown(example: Example) -> str:
    headers = example.table_column_names
    rows = example.table_content_values
    if not headers:
        return "\n".join("\t".join(row) for row in rows)

    column_count = len(headers)
    normalized_rows = []
    for row in rows:
        clipped = row[:column_count]
        if len(clipped) < column_count:
            clipped = clipped + [""] * (column_count - len(clipped))
        normalized_rows.append(clipped)

    lines = [
        "| " + " | ".join(headers) + " |",
        "| " + " | ".join(["---"] * column_count) + " |",
    ]
    lines.extend("| " + " | ".join(row) + " |" for row in normalized_rows)
    return "\n".join(lines)


def build_prompt(example: Example, *, prompt_style: str = "default") -> str:
    table_text = table_to_markdown(example)
    if prompt_style == "strict_label":
        return (
            "Decide whether the scientific table supports the claim, refutes the claim, or does not provide enough information.\n"
            "Do not explain. Output exactly one lowercase label from this set: supports, refutes, not enough info.\n\n"
            f"Paper: {example.paper}\n"
            f"Table caption: {example.table_caption}\n"
            f"Table:\n{table_text}\n\n"
            f"Claim: {example.claim}\n\n"
            "Answer:"
        )
    if prompt_style in {"default", "official"}:
        return (
            "Given a scientific paper table and a claim, choose exactly one label: supports, refutes, or not enough info.\n\n"
            f"Paper: {example.paper}\n"
            f"Table caption: {example.table_caption}\n"
            f"Table:\n{table_text}\n\n"
            f"Claim: {example.claim}\n\n"
            "Return only one label: supports, refutes, or not enough info."
        )
    raise ValueError(f"unknown prompt_style: {prompt_style}")


def _is_ascii_alnum(value: str) -> bool:
    return value.isascii() and value.isalnum()


def _find_last_phrase(text: str, phrase: str) -> int | None:
    if not phrase:
        return None
    lower = text.lower()
    needle = phrase.lower()
    last = None
    start = 0
    while True:
        pos = lower.find(needle, start)
        if pos < 0:
            return last
        last = pos
        start = pos + 1


def _find_last_word(text: str, word: str) -> int | None:
    if not word:
        return None
    lower = text.lower()
    needle = word.lower()
    last = None
    start = 0
    while True:
        pos = lower.find(needle, start)
        if pos < 0:
            return last
        before_ok = pos == 0 or not _is_ascii_alnum(lower[pos - 1])
        end = pos + len(needle)
        after_ok = end >= len(lower) or not _is_ascii_alnum(lower[end])
        if before_ok and after_ok:
            last = pos
        start = pos + 1


def _extract_canonical_label(text: str) -> str | None:
    best: tuple[int, str] | None = None
    for canonical, phrase in SCITAB_PHRASES:
        pos = _find_last_phrase(text, phrase)
        if pos is not None and (best is None or pos >= best[0]):
            best = (pos, canonical)
    for canonical, word in SCITAB_WORDS:
        pos = _find_last_word(text, word)
        if pos is not None and (best is None or pos >= best[0]):
            best = (pos, canonical)
    return best[1] if best is not None else None


def parse_label(text: str) -> str | None:
    trimmed = text.strip()
    if not trimmed:
        return None
    lower = trimmed.lower()
    if lower in {"supports", "support", "supported", "true"}:
        return "supports"
    if lower in {"refutes", "refute", "refuted", "false"}:
        return "refutes"
    if lower in {
        "not enough info",
        "not enough information",
        "nei",
        "insufficient",
        "insufficient information",
        "unverifiable",
    }:
        return "not enough info"
    return _extract_canonical_label(trimmed)


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
