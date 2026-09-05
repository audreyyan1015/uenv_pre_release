from __future__ import annotations

import json
import re
from collections import Counter, defaultdict
from dataclasses import dataclass
from pathlib import Path
from typing import Any


PROMPT_EN = "Please reason step by step, and put your final answer within \\boxed{}.\n\n"
PROMPT_ZH = "请逐步推理，并在 \\boxed{} 内给出您的最终答案。\n\n"


@dataclass(slots=True)
class Example:
    qid: str
    problem: str
    answer: str
    subject: str
    language: str
    difficulty: str
    source_file: str


def infer_meta(path: Path, qid: str) -> tuple[str, str]:
    text = f"{path.name} {qid}".upper()
    language = "ZH" if "-ZH-" in text or text.endswith("-ZH") else "EN"
    difficulty = "HARD" if "HARD" in text else "EASY"
    return language, difficulty


def load_olymmath(paths: list[Path], *, limit: int | None = None) -> list[Example]:
    examples: list[Example] = []
    for path in paths:
        with path.open("r", encoding="utf-8") as file:
            for line in file:
                if not line.strip():
                    continue
                item = json.loads(line)
                qid = str(item["unique_id"])
                language, difficulty = infer_meta(path, qid)
                examples.append(
                    Example(
                        qid=qid,
                        problem=str(item["problem"]).strip(),
                        answer=str(item["answer"]).strip(),
                        subject=str(item.get("subject", "")).strip(),
                        language=language,
                        difficulty=difficulty,
                        source_file=path.name,
                    )
                )
    if limit is not None:
        examples = examples[:limit]
    return examples


def build_prompt(example: Example, *, prompt_style: str = "official") -> str:
    prefix = PROMPT_ZH if example.language == "ZH" else PROMPT_EN
    if prompt_style == "official_no_think":
        return prefix + example.problem
    if prompt_style == "boxed_no_think":
        if example.language == "ZH":
            return (
                "请解答下面的数学题。只输出最终答案，不要解释、不要推理过程。"
                "输出格式必须为：\\boxed{答案}\n\n"
                f"{example.problem}"
            )
        return (
            "Solve the following math problem. Output only the final answer, with no explanation."
            " The output format must be: \\boxed{answer}\n\n"
            f"{example.problem}"
        )
    return prefix + example.problem


def extract_boxed(text: str) -> str | None:
    boxed_contents: list[str] = []
    i = 0
    while i < len(text):
        if text.startswith("\\boxed{", i):
            start = i + len("\\boxed{")
            depth = 1
            j = start
            while j < len(text) and depth:
                char = text[j]
                if char == "{" and (j == 0 or text[j - 1] != "\\"):
                    depth += 1
                elif char == "}" and (j == 0 or text[j - 1] != "\\"):
                    depth -= 1
                j += 1
            if depth == 0:
                boxed_contents.append(text[start : j - 1].strip())
                i = j
                continue
        i += 1
    if boxed_contents:
        return boxed_contents[-1]

    matches = re.findall(r"\\boxed\s*{([^{}]*(?:{[^{}]*}[^{}]*)*)}", text)
    if matches:
        return matches[-1].strip()
    return None


def clean_candidate(text: str) -> str:
    text = text.strip()
    text = re.sub(r"^[$\\(\\[\\{\\s]+", "", text)
    text = re.sub(r"[$\\)\\]\\}\\s.。]+$", "", text)
    text = text.replace("**", "").replace("`", "")
    return text.strip()


def extract_answer(text: str) -> tuple[str | None, str]:
    normalized = text.strip()
    if "</think>" in normalized:
        normalized = normalized.split("</think>")[-1].strip()

    boxed = extract_boxed(normalized)
    if boxed:
        return boxed, "boxed"

    patterns = [
        r"(?:final answer|the answer is|answer is|answer)\s*[:：]?\s*(.+)",
        r"(?:最终答案|答案为|答案是|答案)\s*[:：]?\s*(.+)",
    ]
    for pattern in patterns:
        matches = re.findall(pattern, normalized, flags=re.IGNORECASE)
        if matches:
            return clean_candidate(matches[-1]), "answer_phrase"

    return None, "missing"


def format_for_math_verify(answer: str) -> str:
    answer = answer.strip().strip("$").strip()
    return f"${answer}$" if answer else "$.$"


def normalize_for_string(text: str | None) -> str:
    if not text:
        return ""
    text = text.strip().lower()
    if "</think>" in text:
        text = text.split("</think>")[-1]
    replacements = {
        "\\dfrac": "\\frac",
        "\\tfrac": "\\frac",
        "\\left": "",
        "\\right": "",
        "\\,": "",
        "\\;": "",
        "\\!": "",
        "\\cdot": "*",
        "\\times": "*",
        "−": "-",
        "，": ",",
        "。": "",
    }
    for old, new in replacements.items():
        text = text.replace(old, new)
    text = re.sub(r"\s+", "", text)
    text = text.strip("$")
    return text


def latex_to_sympy_text(text: str) -> str:
    text = normalize_for_string(text)
    text = text.replace("\\pi", "pi")
    text = text.replace("^", "**")
    text = re.sub(r"(\d+)\\circ", r"\1*pi/180", text)

    frac_pattern = re.compile(r"\\frac{([^{}]+)}{([^{}]+)}")
    while True:
        next_text = frac_pattern.sub(r"((\1)/(\2))", text)
        if next_text == text:
            break
        text = next_text

    sqrt_n_pattern = re.compile(r"\\sqrt\[([^{}\[\]]+)]{([^{}]+)}")
    while True:
        next_text = sqrt_n_pattern.sub(r"((\2)**(1/(\1)))", text)
        if next_text == text:
            break
        text = next_text

    sqrt_pattern = re.compile(r"\\sqrt{([^{}]+)}")
    while True:
        next_text = sqrt_pattern.sub(r"sqrt(\1)", text)
        if next_text == text:
            break
        text = next_text

    text = re.sub(r"([0-9)])(sqrt|pi)", r"\1*\2", text)
    text = re.sub(r"(pi|sqrt\([^)]*\))([0-9(])", r"\1*\2", text)
    text = re.sub(r"\\[a-zA-Z]+", "", text)
    return text


def sympy_equiv(pred: str, gold: str) -> bool:
    import sympy as sp

    pred_expr = latex_to_sympy_text(pred)
    gold_expr = latex_to_sympy_text(gold)
    if any(char in pred_expr + gold_expr for char in "[]{}"):
        raise ValueError("skip set/list/range expression")
    parsed_pred = sp.sympify(pred_expr)
    parsed_gold = sp.sympify(gold_expr)
    diff = sp.simplify(parsed_pred - parsed_gold)
    if diff == 0:
        return True
    return bool(abs(float(sp.N(diff))) < 1e-8)


def judge_answer(pred: str | None, gold: str) -> tuple[bool, str]:
    if not pred:
        return False, "missing"

    try:
        from math_verify import parse, verify

        if verify(parse(format_for_math_verify(gold)), parse(format_for_math_verify(pred))):
            return True, "math_verify"
    except Exception:
        pass

    pred_norm = normalize_for_string(pred)
    gold_norm = normalize_for_string(gold)
    if pred_norm and pred_norm == gold_norm:
        return True, "string"

    try:
        if sympy_equiv(pred, gold):
            return True, "sympy"
    except Exception:
        pass

    return False, "no_match"


def safe_div(num: float, den: float) -> float:
    return num / den if den else 0.0


def group_metrics(rows: list[dict[str, Any]], key: str) -> dict[str, Any]:
    grouped: dict[str, dict[str, Any]] = defaultdict(lambda: {"total": 0, "parsed": 0, "correct": 0})
    for row in rows:
        group = str(row[key])
        grouped[group]["total"] += 1
        grouped[group]["parsed"] += int(bool(row["extracted_answer"]))
        grouped[group]["correct"] += int(bool(row["is_correct"]))
    return {
        group: {
            "sample_count": values["total"],
            "parsed_count": values["parsed"],
            "parse_rate": safe_div(values["parsed"], values["total"]),
            "accuracy": safe_div(values["correct"], values["total"]),
        }
        for group, values in sorted(grouped.items())
    }


def problem_group_metrics(rows: list[dict[str, Any]], key: str) -> dict[str, Any]:
    grouped: dict[str, dict[str, Any]] = defaultdict(
        lambda: {"qids": set(), "parsed_qids": set(), "correct_qids": set()}
    )
    for row in rows:
        group = str(row[key])
        qid = str(row["qid"])
        grouped[group]["qids"].add(qid)
        if row["extracted_answer"]:
            grouped[group]["parsed_qids"].add(qid)
        if row["is_correct"]:
            grouped[group]["correct_qids"].add(qid)
    return {
        group: {
            "problem_count": len(values["qids"]),
            "problem_parse_rate": safe_div(len(values["parsed_qids"]), len(values["qids"])),
            "pass_at_sample": safe_div(len(values["correct_qids"]), len(values["qids"])),
        }
        for group, values in sorted(grouped.items())
    }


def consensus_answer(rows: list[dict[str, Any]]) -> str | None:
    answers = [
        normalize_for_string(row["extracted_answer"])
        for row in rows
        if row["extracted_answer"]
    ]
    if not answers:
        return None
    return Counter(answers).most_common(1)[0][0]


def compute_metrics(rows: list[dict[str, Any]]) -> dict[str, Any]:
    total = len(rows)
    parsed = sum(1 for row in rows if row["extracted_answer"])
    correct = sum(1 for row in rows if row["is_correct"])
    token_counts = [row["output_tokens"] for row in rows if row["output_tokens"] is not None]
    by_qid: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for row in rows:
        by_qid[str(row["qid"])].append(row)

    problem_count = len(by_qid)
    parsed_problem_count = sum(
        1 for problem_rows in by_qid.values() if any(row["extracted_answer"] for row in problem_rows)
    )
    passed_problem_count = sum(
        1 for problem_rows in by_qid.values() if any(row["is_correct"] for row in problem_rows)
    )
    consensus_correct = 0
    unique_answer_counts = []
    for problem_rows in by_qid.values():
        answer = consensus_answer(problem_rows)
        unique_answer_counts.append(
            len(
                {
                    normalize_for_string(row["extracted_answer"])
                    for row in problem_rows
                    if row["extracted_answer"]
                }
            )
        )
        if answer:
            is_correct, _ = judge_answer(answer, str(problem_rows[0]["gold"]))
            consensus_correct += int(is_correct)

    samples_per_problem = safe_div(total, problem_count)
    return {
        "sample_count": total,
        "problem_count": problem_count,
        "samples_per_problem": samples_per_problem,
        "parsed_count": parsed,
        "unparsed_count": total - parsed,
        "parse_rate": safe_div(parsed, total),
        "accuracy": safe_div(correct, total),
        "parsed_accuracy": safe_div(correct, parsed),
        "problem_parsed_count": parsed_problem_count,
        "problem_parse_rate": safe_div(parsed_problem_count, problem_count),
        "pass_at_sample": safe_div(passed_problem_count, problem_count),
        "consensus_accuracy": safe_div(consensus_correct, problem_count),
        "avg_unique_extracted_answers_per_problem": safe_div(
            sum(unique_answer_counts), len(unique_answer_counts)
        ),
        "avg_output_tokens": safe_div(sum(token_counts), len(token_counts)),
        "max_output_tokens": max(token_counts) if token_counts else 0,
        "difficulty_distribution": dict(Counter(row["difficulty"] for row in rows)),
        "subject_distribution": dict(Counter(row["subject"] for row in rows)),
        "extraction_method_distribution": dict(Counter(row["extraction_method"] for row in rows)),
        "judge_method_distribution": dict(Counter(row["judge_method"] for row in rows)),
        "by_difficulty": group_metrics(rows, "difficulty"),
        "problem_by_difficulty": problem_group_metrics(rows, "difficulty"),
        "by_language": group_metrics(rows, "language"),
        "problem_by_language": problem_group_metrics(rows, "language"),
        "by_subject": group_metrics(rows, "subject"),
        "problem_by_subject": problem_group_metrics(rows, "subject"),
    }
