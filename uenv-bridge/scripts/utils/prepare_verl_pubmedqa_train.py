#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import random
from pathlib import Path
from typing import Any

import pandas as pd


LABELS = {"yes", "no", "maybe"}
SYSTEM_PROMPT = "You are answering PubMedQA biomedical reading comprehension questions."


def build_prompt(question: str, contexts: list[str]) -> list[dict[str, str]]:
    context = "\n".join(f"[{idx + 1}] {text}" for idx, text in enumerate(contexts))
    user_content = (
        "Read the abstract context and answer the biomedical question with exactly one label: yes, no, or maybe.\n\n"
        f"Context:\n{context}\n\n"
        f"Question: {question}\n\n"
        "Return only one word: yes, no, or maybe."
    )
    return [
        {"role": "system", "content": SYSTEM_PROMPT},
        {"role": "user", "content": user_content},
    ]


def load_examples(path: Path) -> list[dict[str, Any]]:
    data = json.loads(path.read_text(encoding="utf-8"))
    examples: list[dict[str, Any]] = []
    for qid, item in data.items():
        answer = str(item["final_decision"]).strip().lower()
        if answer not in LABELS:
            raise ValueError(f"unsupported PubMedQA label for {qid}: {answer}")
        examples.append(
            {
                "qid": str(qid),
                "question": str(item["QUESTION"]).strip(),
                "contexts": [str(text).strip() for text in item["CONTEXTS"]],
                "answer": answer,
            }
        )
    return examples


def to_verl_row(example: dict[str, Any], *, split: str, index: int, max_steps: int) -> dict[str, Any]:
    answer = str(example["answer"])
    return {
        "data_source": "pubmedqa",
        "prompt": build_prompt(str(example["question"]), list(example["contexts"])),
        "ability": "qa",
        "reward_model": {
            "style": "rule",
            "ground_truth": answer,
            "target": answer,
            "dataset": "pubmedqa",
        },
        "extra_info": {
            "split": split,
            "index": index,
            "qid": str(example["qid"]),
            "dataset": "pubmedqa",
            "benchmark": "pubmedqa",
            "answer": answer,
            "max_steps": max_steps,
        },
    }


def write_eval_json(examples: list[dict[str, Any]], path: Path) -> None:
    payload = {}
    for example in examples:
        payload[str(example["qid"])] = {
            "QUESTION": example["question"],
            "CONTEXTS": example["contexts"],
            "final_decision": example["answer"],
        }
    path.write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def main() -> None:
    parser = argparse.ArgumentParser(description="Prepare PubMedQA VeRL-format train/test parquet files.")
    parser.add_argument("--input", type=Path, default=Path("data/benchmarks/pubmedqa/ori_pqal.json"))
    parser.add_argument("--output-dir", type=Path, default=Path("data/benchmarks/pubmedqa_verl_90_10"))
    parser.add_argument("--train-ratio", type=float, default=0.9)
    parser.add_argument("--seed", type=int, default=0)
    parser.add_argument("--max-steps", type=int, default=1)
    parser.add_argument("--no-shuffle", action="store_true")
    args = parser.parse_args()

    if not 0.0 < args.train_ratio < 1.0:
        raise ValueError("--train-ratio must be between 0 and 1")

    examples = load_examples(args.input)
    if not args.no_shuffle:
        rng = random.Random(args.seed)
        rng.shuffle(examples)

    train_count = int(len(examples) * args.train_ratio)
    train_examples = examples[:train_count]
    test_examples = examples[train_count:]

    args.output_dir.mkdir(parents=True, exist_ok=True)
    train_rows = [
        to_verl_row(example, split="train", index=index, max_steps=args.max_steps)
        for index, example in enumerate(train_examples)
    ]
    test_rows = [
        to_verl_row(example, split="test", index=index, max_steps=args.max_steps)
        for index, example in enumerate(test_examples)
    ]
    pd.DataFrame(train_rows).to_parquet(args.output_dir / "train.parquet", index=False)
    pd.DataFrame(test_rows).to_parquet(args.output_dir / "test.parquet", index=False)
    write_eval_json(test_examples, args.output_dir / "eval_pqal.json")

    metadata = {
        "source": str(args.input),
        "train_ratio": args.train_ratio,
        "seed": args.seed,
        "shuffled": not args.no_shuffle,
        "max_steps": args.max_steps,
        "total_rows": len(examples),
        "train_rows": len(train_rows),
        "test_rows": len(test_rows),
    }
    (args.output_dir / "metadata.json").write_text(
        json.dumps(metadata, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    print(json.dumps(metadata, ensure_ascii=False))


if __name__ == "__main__":
    main()
