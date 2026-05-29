#!/usr/bin/env python3
from __future__ import annotations

import argparse
import re
from pathlib import Path

import pandas as pd

instruction_following = "Let's think step by step and output the final answer after ####."

def extract_solution(answer: str) -> str:
    match = re.search(r"####\s*(.*)", answer)
    return match.group(1).strip() if match else answer.strip()

def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument('--input', default='/workspace/data/gsm8k/train.parquet')
    parser.add_argument('--output', required=True)
    parser.add_argument('--n', type=int, default=2)
    args = parser.parse_args()

    src = pd.read_parquet(args.input).head(args.n)
    rows = []
    for idx, row in src.iterrows():
        question_raw = row['question']
        answer_raw = row['answer']
        rows.append({
            'data_source': 'openai/gsm8k',
            'prompt': [{'role': 'user', 'content': question_raw + ' ' + instruction_following}],
            'ability': 'math',
            'reward_model': {'style': 'rule', 'ground_truth': extract_solution(answer_raw)},
            'extra_info': {
                'split': 'train',
                'index': int(idx),
                'answer': answer_raw,
                'question': question_raw,
            },
        })
    out = Path(args.output)
    out.parent.mkdir(parents=True, exist_ok=True)
    pd.DataFrame(rows).to_parquet(out, index=False)
    print(f'wrote {out} rows={len(rows)} columns={list(pd.DataFrame(rows).columns)}')

if __name__ == '__main__':
    main()
