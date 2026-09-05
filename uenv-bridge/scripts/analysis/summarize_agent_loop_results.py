#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
from collections import Counter
from pathlib import Path
from typing import Any


def load_jsonl(path: Path) -> list[dict[str, Any]]:
    records = []
    for line_no, line in enumerate(path.read_text(encoding="utf-8").splitlines(), start=1):
        if not line.strip():
            continue
        try:
            value = json.loads(line)
        except json.JSONDecodeError as exc:
            raise SystemExit(f"{path}:{line_no}: invalid JSON: {exc}") from exc
        if isinstance(value, dict):
            records.append(value)
    return records


def main() -> int:
    parser = argparse.ArgumentParser(description="Summarize UEnv AgentLoop result JSONL records.")
    parser.add_argument("jsonl", type=Path, help="Path to agent-loop-results.jsonl.")
    parser.add_argument("--examples", type=int, default=5, help="Number of records to print.")
    args = parser.parse_args()

    records = load_jsonl(args.jsonl)
    rewards = [record.get("reward") for record in records]
    statuses = Counter(str(record.get("status")) for record in records)
    reward_counts = Counter(str(reward) for reward in rewards)

    print(f"records: {len(records)}")
    print("statuses:")
    for status, count in statuses.most_common():
        print(f"  {status}: {count}")
    print("rewards:")
    for reward, count in reward_counts.most_common():
        print(f"  {reward}: {count}")

    numeric_rewards = [float(reward) for reward in rewards if isinstance(reward, (int, float))]
    if numeric_rewards:
        print(
            "reward_stats: "
            f"min={min(numeric_rewards):.6g} "
            f"max={max(numeric_rewards):.6g} "
            f"mean={sum(numeric_rewards) / len(numeric_rewards):.6g}"
        )

    raw_response_id_lengths = [
        len(record.get("response_ids") or []) for record in records if isinstance(record.get("response_ids"), list)
    ]
    verl_response_id_lengths = [
        len(record.get("verl_response_ids") or [])
        for record in records
        if isinstance(record.get("verl_response_ids"), list)
    ]
    if raw_response_id_lengths:
        print(
            "response_ids_len: "
            f"min={min(raw_response_id_lengths)} "
            f"max={max(raw_response_id_lengths)} "
            f"mean={sum(raw_response_id_lengths) / len(raw_response_id_lengths):.3g}"
        )
    if verl_response_id_lengths:
        print(
            "verl_response_ids_len: "
            f"min={min(verl_response_id_lengths)} "
            f"max={max(verl_response_id_lengths)} "
            f"mean={sum(verl_response_id_lengths) / len(verl_response_id_lengths):.3g}"
        )

    print("examples:")
    for record in records[: args.examples]:
        response_text = str(record.get("response_text") or "")
        if len(response_text) > 160:
            response_text = response_text[:157] + "..."
        print(
            json.dumps(
                {
                    "request_id": record.get("request_id"),
                    "batch_id": record.get("batch_id"),
                    "sample_index": record.get("sample_index"),
                    "status": record.get("status"),
                    "reward": record.get("reward"),
                    "total_steps": record.get("total_steps"),
                    "response_ids_len": len(record.get("response_ids") or []),
                    "verl_response_ids_len": len(record.get("verl_response_ids") or []),
                    "response_text": response_text,
                },
                ensure_ascii=False,
            )
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
