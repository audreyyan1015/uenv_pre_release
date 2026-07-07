#!/usr/bin/env python3
from __future__ import annotations

import argparse
import csv
import json
import re
from collections import Counter
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from tqdm import tqdm


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
    return (
        "Read the abstract context and answer the biomedical question with exactly one label: yes, no, or maybe.\n\n"
        f"Context:\n{context}\n\n"
        f"Question: {example.question}\n\n"
        "Return only one word: yes, no, or maybe."
    )


def build_messages(example: Example, *, prompt_style: str = "default") -> list[dict[str, str]]:
    system_content = "You are answering PubMedQA biomedical reading comprehension questions."
    if prompt_style == "strict_label":
        system_content = "You are a PubMedQA label classifier. Output only one lowercase label: yes, no, or maybe."
    return [
        {
            "role": "system",
            "content": system_content,
        },
        {"role": "user", "content": build_prompt(example, prompt_style=prompt_style)},
    ]


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


def generate_with_transformers(args: argparse.Namespace, examples: list[Example]) -> list[str]:
    import torch
    from transformers import AutoModelForCausalLM, AutoTokenizer

    tokenizer = AutoTokenizer.from_pretrained(args.model, trust_remote_code=True)
    if tokenizer.pad_token_id is None:
        tokenizer.pad_token = tokenizer.eos_token

    dtype = getattr(torch, args.dtype) if args.dtype != "auto" else "auto"
    model = AutoModelForCausalLM.from_pretrained(
        args.model,
        trust_remote_code=True,
        dtype=dtype,
        device_map=args.transformers_device_map,
    )
    model.eval()

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

    input_device = next(model.parameters()).device
    outputs: list[str] = []
    for start in tqdm(range(0, len(prompts), args.transformers_batch_size), desc="generating"):
        batch = prompts[start : start + args.transformers_batch_size]
        encoded = tokenizer(
            batch,
            return_tensors="pt",
            padding=True,
            truncation=True,
            max_length=args.max_model_len,
        )
        encoded = {key: value.to(input_device) for key, value in encoded.items()}
        with torch.inference_mode():
            generated = model.generate(
                **encoded,
                do_sample=args.temperature > 0.0,
                temperature=args.temperature if args.temperature > 0.0 else None,
                top_p=args.top_p,
                max_new_tokens=args.max_tokens,
                pad_token_id=tokenizer.pad_token_id,
            )
        prompt_len = encoded["input_ids"].shape[1]
        decoded = tokenizer.batch_decode(generated[:, prompt_len:], skip_special_tokens=True)
        outputs.extend(decoded)
    return outputs


def predict_with_transformers_label_logprob(args: argparse.Namespace, examples: list[Example]) -> list[str]:
    import torch
    from transformers import AutoModelForCausalLM, AutoTokenizer

    tokenizer = AutoTokenizer.from_pretrained(args.model, trust_remote_code=True)
    if tokenizer.pad_token_id is None:
        tokenizer.pad_token = tokenizer.eos_token

    dtype = getattr(torch, args.dtype) if args.dtype != "auto" else "auto"
    model = AutoModelForCausalLM.from_pretrained(
        args.model,
        trust_remote_code=True,
        dtype=dtype,
        device_map=args.transformers_device_map,
    )
    model.eval()

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

    input_device = next(model.parameters()).device
    predictions: list[str] = []
    for start in tqdm(range(0, len(prompts), args.transformers_batch_size), desc="label scoring"):
        batch_prompts = prompts[start : start + args.transformers_batch_size]
        sequences: list[list[int]] = []
        metadata: list[tuple[int, str, int, list[int]]] = []
        for local_index, prompt in enumerate(batch_prompts):
            prompt_ids = tokenizer(prompt, add_special_tokens=False).input_ids
            for label, label_ids in candidate_ids.items():
                max_prompt_len = args.max_model_len - len(label_ids)
                if max_prompt_len <= 1:
                    raise ValueError("--max-model-len is too small for label scoring")
                clipped_prompt_ids = prompt_ids[-max_prompt_len:]
                sequences.append(clipped_prompt_ids + label_ids)
                metadata.append((local_index, label, len(clipped_prompt_ids), label_ids))

        max_len = max(len(ids) for ids in sequences)
        padded = []
        attention = []
        for ids in sequences:
            pad_len = max_len - len(ids)
            padded.append(ids + [tokenizer.pad_token_id] * pad_len)
            attention.append([1] * len(ids) + [0] * pad_len)

        input_ids = torch.tensor(padded, dtype=torch.long, device=input_device)
        attention_mask = torch.tensor(attention, dtype=torch.long, device=input_device)
        with torch.inference_mode():
            logits = model(input_ids=input_ids, attention_mask=attention_mask).logits
            log_probs = torch.log_softmax(logits.float(), dim=-1)

        grouped_scores: list[dict[str, float]] = [dict() for _ in batch_prompts]
        for row_index, (local_index, label, prompt_len, label_ids) in enumerate(metadata):
            positions = torch.arange(
                prompt_len - 1,
                prompt_len - 1 + len(label_ids),
                device=input_device,
            )
            targets = torch.tensor(label_ids, dtype=torch.long, device=input_device)
            token_scores = log_probs[row_index, positions, targets]
            if args.label_score_normalization == "mean":
                score = token_scores.mean()
            else:
                score = token_scores.sum()
            grouped_scores[local_index][label] = float(score.detach().cpu())

        for scores in grouped_scores:
            predictions.append(max(LABELS, key=lambda label: scores[label]))

    return predictions


def write_outputs(rows: list[dict[str, Any]], output_dir: Path) -> None:
    output_dir.mkdir(parents=True, exist_ok=True)
    jsonl_path = output_dir / "predictions.jsonl"
    with jsonl_path.open("w", encoding="utf-8") as file:
        for row in rows:
            file.write(json.dumps(row, ensure_ascii=False) + "\n")

    official_path = output_dir / "predictions_official.json"
    official_predictions = {
        row["qid"]: row["pred"] if row["pred"] in LABELS else "maybe" for row in rows
    }
    official_path.write_text(
        json.dumps(official_predictions, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )

    csv_path = output_dir / "predictions.csv"
    with csv_path.open("w", encoding="utf-8", newline="") as file:
        writer = csv.DictWriter(file, fieldnames=["qid", "gold", "pred", "raw_output", "question"])
        writer.writeheader()
        for row in rows:
            writer.writerow(row)


def main() -> None:
    parser = argparse.ArgumentParser(description="Evaluate a base model on PubMedQA labeled set.")
    parser.add_argument("--data", type=Path, required=True)
    parser.add_argument("--model", required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--backend", choices=("vllm", "transformers"), default="vllm")
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
    parser.add_argument("--transformers-device-map", default="auto")
    parser.add_argument("--transformers-batch-size", type=int, default=1)
    parser.add_argument("--vllm-label-batch-size", type=int, default=64)
    parser.add_argument("--label-score-normalization", choices=("sum", "mean"), default="mean")
    args = parser.parse_args()

    examples = load_pubmedqa(args.data, limit=args.limit)
    if args.inference_mode == "label_logprob":
        if args.backend == "vllm":
            raw_outputs = predict_with_vllm_label_logprob(args, examples)
        else:
            raw_outputs = predict_with_transformers_label_logprob(args, examples)
    elif args.backend == "vllm":
        raw_outputs = generate_with_vllm(args, examples)
    else:
        raw_outputs = generate_with_transformers(args, examples)

    rows = []
    for example, raw in tqdm(list(zip(examples, raw_outputs, strict=True)), desc="scoring"):
        rows.append(
            {
                "qid": example.qid,
                "gold": example.answer,
                "pred": parse_label(raw),
                "raw_output": raw.strip(),
                "question": example.question,
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
