#!/usr/bin/env python3
from __future__ import annotations

import argparse
import csv
import json
from collections import Counter
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from tqdm import tqdm


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
    return (
        "Given a scientific paper table and a claim, choose exactly one label: supports, refutes, or not enough info.\n\n"
        f"Paper: {example.paper}\n"
        f"Table caption: {example.table_caption}\n"
        f"Table:\n{table_text}\n\n"
        f"Claim: {example.claim}\n\n"
        "Return only one label: supports, refutes, or not enough info."
    )


def build_messages(example: Example, *, prompt_style: str = "default") -> list[dict[str, str]]:
    system_content = "You are a scientific table claim verification classifier."
    if prompt_style == "strict_label":
        system_content = "Output only one lowercase label: supports, refutes, or not enough info."
    return [
        {"role": "system", "content": system_content},
        {"role": "user", "content": build_prompt(example, prompt_style=prompt_style)},
    ]


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
    # Keep this aligned with plugins/math/src/backends/scitab/scoring.rs.
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
        "prediction_distribution": dict(
            Counter(row["pred"] if row["pred"] in LABELS else "unparsed" for row in rows)
        ),
        "per_class": per_class,
        "confusion": confusion,
    }


def generate_with_vllm(args: argparse.Namespace, examples: list[Example]) -> list[str]:
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
    sampling = SamplingParams(
        temperature=args.temperature,
        top_p=args.top_p,
        max_tokens=args.max_tokens,
        stop=args.stop,
    )
    if args.no_chat_template:
        prompts = [build_prompt(example, prompt_style=args.prompt_style) for example in examples]
    else:
        from transformers import AutoTokenizer

        tokenizer = AutoTokenizer.from_pretrained(args.model, trust_remote_code=True)
        prompts = [
            tokenizer.apply_chat_template(
                build_messages(example, prompt_style=args.prompt_style),
                tokenize=False,
                add_generation_prompt=True,
            )
            for example in examples
        ]
    outputs = llm.generate(prompts, sampling)
    return [output.outputs[0].text for output in outputs]


def _extract_vllm_token_logprob(token_logprobs: Any, token_id: int) -> float:
    if token_logprobs is None:
        raise ValueError("vLLM did not return prompt logprobs; check SamplingParams(prompt_logprobs=...)")
    if isinstance(token_logprobs, dict):
        value = token_logprobs.get(token_id) or token_logprobs.get(str(token_id))
        if value is None:
            raise ValueError(f"vLLM prompt logprobs missing token id {token_id}")
        return float(getattr(value, "logprob", value))
    logprob = getattr(token_logprobs, "logprob", None)
    if logprob is not None:
        return float(logprob)
    raise TypeError(f"unsupported vLLM prompt logprob item: {type(token_logprobs)!r}")


def predict_with_vllm_label_logprob(args: argparse.Namespace, examples: list[Example]) -> list[str]:
    from transformers import AutoTokenizer
    from vllm import LLM, SamplingParams

    tokenizer = AutoTokenizer.from_pretrained(args.model, trust_remote_code=True)
    if args.no_chat_template:
        prompts = [build_prompt(example, prompt_style=args.prompt_style) for example in examples]
    else:
        prompts = [
            tokenizer.apply_chat_template(
                build_messages(example, prompt_style=args.prompt_style),
                tokenize=False,
                add_generation_prompt=True,
            )
            for example in examples
        ]

    candidate_ids = {
        label: tokenizer(label, add_special_tokens=False).input_ids for label in LABELS
    }
    if any(not ids for ids in candidate_ids.values()):
        raise ValueError(f"failed to tokenize label candidates: {candidate_ids}")

    llm = LLM(
        model=args.model,
        tensor_parallel_size=args.tensor_parallel_size,
        trust_remote_code=True,
        dtype=args.dtype,
        gpu_memory_utilization=args.gpu_memory_utilization,
        max_model_len=args.max_model_len,
        enforce_eager=args.enforce_eager,
    )
    sampling = SamplingParams(
        temperature=0.0,
        max_tokens=1,
        prompt_logprobs=1,
    )

    predictions: list[str] = []
    for start in tqdm(range(0, len(prompts), args.vllm_label_batch_size), desc="vLLM label scoring"):
        batch_prompts = prompts[start : start + args.vllm_label_batch_size]
        sequences: list[str] = []
        metadata: list[tuple[int, str, int, list[int]]] = []
        for local_index, prompt in enumerate(batch_prompts):
            prompt_ids = tokenizer(prompt, add_special_tokens=False).input_ids
            for label, label_ids in candidate_ids.items():
                max_prompt_len = args.max_model_len - len(label_ids)
                if max_prompt_len <= 1:
                    raise ValueError("--max-model-len is too small for label scoring")
                clipped_prompt_ids = prompt_ids[-max_prompt_len:]
                sequences.append(tokenizer.decode(clipped_prompt_ids + label_ids))
                metadata.append((local_index, label, len(clipped_prompt_ids), label_ids))

        outputs = llm.generate(sequences, sampling, use_tqdm=False)
        grouped_scores: list[dict[str, float]] = [dict() for _ in batch_prompts]
        for output, (local_index, label, prompt_len, label_ids) in zip(outputs, metadata, strict=True):
            prompt_logprobs = output.prompt_logprobs
            if prompt_logprobs is None:
                raise ValueError("vLLM output.prompt_logprobs is None")
            token_scores = []
            for offset, token_id in enumerate(label_ids):
                position = prompt_len + offset
                token_scores.append(_extract_vllm_token_logprob(prompt_logprobs[position], token_id))
            score = sum(token_scores) / len(token_scores) if args.label_score_normalization == "mean" else sum(token_scores)
            grouped_scores[local_index][label] = score

        for scores in grouped_scores:
            predictions.append(max(LABELS, key=lambda label: scores[label]))

    return predictions


def write_outputs(rows: list[dict[str, Any]], output_dir: Path) -> None:
    output_dir.mkdir(parents=True, exist_ok=True)
    with (output_dir / "predictions.jsonl").open("w", encoding="utf-8") as file:
        for row in rows:
            file.write(json.dumps(row, ensure_ascii=False) + "\n")

    official_predictions = {
        row["qid"]: row["pred"] if row["pred"] in LABELS else "not enough info" for row in rows
    }
    (output_dir / "predictions_official.json").write_text(
        json.dumps(official_predictions, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )

    with (output_dir / "predictions.csv").open("w", encoding="utf-8", newline="") as file:
        writer = csv.DictWriter(
            file,
            fieldnames=["qid", "gold", "pred", "raw_output", "claim", "table_id", "paper_id"],
        )
        writer.writeheader()
        for row in rows:
            writer.writerow(row)


def main() -> None:
    parser = argparse.ArgumentParser(description="Evaluate a base model on SciTab.")
    parser.add_argument("--data", type=Path, required=True)
    parser.add_argument("--model", required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--backend", choices=("vllm",), default="vllm")
    parser.add_argument("--inference-mode", choices=("generate", "label_logprob"), default="generate")
    parser.add_argument("--limit", type=int, default=None)
    parser.add_argument("--tensor-parallel-size", type=int, default=8)
    parser.add_argument("--dtype", default="bfloat16")
    parser.add_argument("--gpu-memory-utilization", type=float, default=0.9)
    parser.add_argument("--max-model-len", type=int, default=4096)
    parser.add_argument("--temperature", type=float, default=0.0)
    parser.add_argument("--top-p", type=float, default=1.0)
    parser.add_argument("--max-tokens", type=int, default=256)
    parser.add_argument("--stop", nargs="*", default=None)
    parser.add_argument("--prompt-style", choices=("default", "strict_label"), default="default")
    parser.add_argument("--enforce-eager", action="store_true")
    parser.add_argument("--no-chat-template", action="store_true")
    parser.add_argument("--vllm-label-batch-size", type=int, default=64)
    parser.add_argument("--label-score-normalization", choices=("sum", "mean"), default="mean")
    args = parser.parse_args()

    examples = load_scitab(args.data, limit=args.limit)
    if args.inference_mode == "label_logprob":
        raw_outputs = predict_with_vllm_label_logprob(args, examples)
    else:
        raw_outputs = generate_with_vllm(args, examples)

    rows = []
    for example, raw in tqdm(list(zip(examples, raw_outputs, strict=True)), desc="scoring"):
        rows.append(
            {
                "qid": example.qid,
                "gold": example.label,
                "pred": parse_label(raw),
                "raw_output": raw.strip(),
                "claim": example.claim,
                "table_id": example.table_id,
                "paper_id": example.paper_id,
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
