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
    parser.add_argument('--offset', type=int, default=0)
    parser.add_argument('--model-endpoint')
    parser.add_argument('--model-name')
    args = parser.parse_args()

    src = pd.read_parquet(args.input).iloc[args.offset:args.offset + args.n]
    rows = []
    for idx, row in src.iterrows():
        question_raw = row['question']
        answer_raw = row['answer']
        extra_info = {
            'split': 'train',
            'index': int(idx),
            'answer': answer_raw,
            'question': question_raw,
        }
        if args.model_endpoint:
            extra_info['model_endpoint'] = args.model_endpoint
        if args.model_name:
            extra_info['model_name'] = args.model_name

        rows.append({
            'data_source': 'openai/gsm8k',
            'prompt': [{'role': 'user', 'content': question_raw + ' ' + instruction_following}],
            'ability': 'math',
            'reward_model': {'style': 'rule', 'ground_truth': extract_solution(answer_raw)},
            'extra_info': extra_info,
        })
    out = Path(args.output)
    out.parent.mkdir(parents=True, exist_ok=True)
    pd.DataFrame(rows).to_parquet(out, index=False)
    print(
        f'wrote {out} rows={len(rows)} offset={args.offset} '
        f'columns={list(pd.DataFrame(rows).columns)}'
    )

if __name__ == '__main__':
    main()
