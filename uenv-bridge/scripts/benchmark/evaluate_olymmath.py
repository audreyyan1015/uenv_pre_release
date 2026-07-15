#!/usr/bin/env python3
from __future__ import annotations

import argparse
import csv
import json
import re
from collections import Counter, defaultdict
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from tqdm import tqdm


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


def build_messages(example: Example, *, prompt_style: str = "official") -> list[dict[str, str]]:
    system_content = "You are a careful mathematical problem solver."
    if example.language == "ZH":
        system_content = "你是一个严谨的数学题求解助手。"
    if prompt_style == "boxed_no_think":
        system_content = "Output only the final answer in \\boxed{}."
        if example.language == "ZH":
            system_content = "只输出 \\boxed{} 内的最终答案。"
    return [
        {"role": "system", "content": system_content},
        {"role": "user", "content": build_prompt(example, prompt_style=prompt_style)},
    ]


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


def clean_candidate(text: str) -> str:
    text = text.strip()
    text = re.sub(r"^[$\\(\\[\\{\\s]+", "", text)
    text = re.sub(r"[$\\)\\]\\}\\s.。]+$", "", text)
    text = text.replace("**", "").replace("`", "")
    return text.strip()


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


def generate_with_vllm(
    args: argparse.Namespace,
    examples: list[Example],
) -> list[list[tuple[str, int | None]]]:
    from transformers import AutoTokenizer
    from vllm import LLM, SamplingParams

    llm = LLM(
        model=args.model,
        tensor_parallel_size=args.tensor_parallel_size,
        trust_remote_code=True,
        dtype=args.dtype,
        gpu_memory_utilization=args.gpu_memory_utilization,
        max_model_len=args.max_model_len,
        enforce_eager=args.enforce_eager,
    )
    sampling_kwargs: dict[str, Any] = {
        "temperature": args.temperature,
        "top_p": args.top_p,
        "max_tokens": args.max_tokens,
        "n": args.sample,
    }
    if args.min_p is not None:
        sampling_kwargs["min_p"] = args.min_p
    sampling = SamplingParams(**sampling_kwargs)
    if args.no_chat_template:
        prompts = [build_prompt(example, prompt_style=args.prompt_style) for example in examples]
    else:
        tokenizer = AutoTokenizer.from_pretrained(args.model, trust_remote_code=True)
        prompts = [
            tokenizer.apply_chat_template(
                build_messages(example, prompt_style=args.prompt_style),
                tokenize=False,
                add_generation_prompt=True,
                **(
                    {"enable_thinking": False}
                    if args.prompt_style in {"boxed_no_think", "official_no_think"}
                    else {}
                ),
            )
            for example in examples
        ]

    outputs = llm.generate(prompts, sampling)
    return [
        [
            (sample.text, len(sample.token_ids))
            for sample in output.outputs
        ]
        for output in outputs
    ]


def write_outputs(rows: list[dict[str, Any]], output_dir: Path) -> None:
    output_dir.mkdir(parents=True, exist_ok=True)
    with (output_dir / "predictions.jsonl").open("w", encoding="utf-8") as file:
        for row in rows:
            file.write(json.dumps(row, ensure_ascii=False) + "\n")

    by_qid: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for row in rows:
        by_qid[str(row["qid"])].append(row)
    if all(len(problem_rows) == 1 for problem_rows in by_qid.values()):
        official_predictions: dict[str, Any] = {
            qid: problem_rows[0]["extracted_answer"] or ""
            for qid, problem_rows in by_qid.items()
        }
    else:
        official_predictions = {
            qid: [
                row["extracted_answer"] or ""
                for row in sorted(problem_rows, key=lambda item: int(item["sample_idx"]))
            ]
            for qid, problem_rows in by_qid.items()
        }
    (output_dir / "predictions_official.json").write_text(
        json.dumps(official_predictions, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )

    fieldnames = [
        "qid",
        "sample_idx",
        "language",
        "difficulty",
        "subject",
        "gold",
        "extracted_answer",
        "extraction_method",
        "is_correct",
        "judge_method",
        "output_tokens",
        "problem",
        "raw_output",
    ]
    with (output_dir / "predictions.csv").open("w", encoding="utf-8", newline="") as file:
        writer = csv.DictWriter(file, fieldnames=fieldnames)
        writer.writeheader()
        for row in rows:
            writer.writerow({key: row.get(key) for key in fieldnames})


def main() -> None:
    parser = argparse.ArgumentParser(description="Evaluate a base model on OlymMATH.")
    parser.add_argument("--data", type=Path, nargs="+", required=True)
    parser.add_argument("--model", required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--limit", type=int, default=None)
    parser.add_argument("--tensor-parallel-size", type=int, default=8)
    parser.add_argument("--dtype", default="bfloat16")
    parser.add_argument("--gpu-memory-utilization", type=float, default=0.9)
    parser.add_argument("--max-model-len", type=int, default=16384)
    parser.add_argument("--temperature", type=float, default=0.0)
    parser.add_argument("--top-p", type=float, default=1.0)
    parser.add_argument("--min-p", type=float, default=None)
    parser.add_argument("--max-tokens", type=int, default=8192)
    parser.add_argument("--sample", type=int, default=1)
    parser.add_argument(
        "--prompt-style",
        choices=("official", "official_no_think", "boxed_no_think"),
        default="official",
    )
    parser.add_argument("--enforce-eager", action="store_true")
    parser.add_argument("--no-chat-template", action="store_true")
    args = parser.parse_args()

    examples = load_olymmath(args.data, limit=args.limit)
    raw_outputs = generate_with_vllm(args, examples)

    rows = []
    for example, generations in tqdm(
        list(zip(examples, raw_outputs, strict=True)),
        desc="scoring",
    ):
        for sample_idx, (raw, output_tokens) in enumerate(generations):
            extracted, extraction_method = extract_answer(raw)
            is_correct, judge_method = judge_answer(extracted, example.answer)
            rows.append(
                {
                    "qid": example.qid,
                    "sample_idx": sample_idx,
                    "language": example.language,
                    "difficulty": example.difficulty,
                    "subject": example.subject,
                    "source_file": example.source_file,
                    "gold": example.answer,
                    "extracted_answer": extracted,
                    "extraction_method": extraction_method,
                    "is_correct": is_correct,
                    "judge_method": judge_method,
                    "output_tokens": output_tokens,
                    "problem": example.problem,
                    "raw_output": raw.strip(),
                }
            )

    metrics = compute_metrics(rows)
    args.output_dir.mkdir(parents=True, exist_ok=True)
    write_outputs(rows, args.output_dir)
    (args.output_dir / "metrics.json").write_text(
        json.dumps(metrics, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    print(json.dumps(metrics, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
