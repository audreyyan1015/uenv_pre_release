#!/usr/bin/env python3
"""Convert portable UEnv Episode JSONL into the rows expected by VeRL.

The converter is deliberately environment-neutral. A row selects an
``env_type`` and carries ``env_config`` / ``reward_config`` unchanged. Prompt
format requirements belong in the input JSONL instead of being inferred from
an environment name.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any


def json_object(value: Any, *, location: str) -> dict[str, Any]:
    if value is None:
        return {}
    if not isinstance(value, dict):
        raise ValueError(f"{location} must be a JSON object")
    return dict(value)


def load_rows(
    path: Path,
    *,
    dataset: str,
    env_type: str,
    max_steps: int,
    limit: int | None = None,
) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    with path.open("r", encoding="utf-8") as handle:
        for line_number, raw in enumerate(handle, start=1):
            line = raw.strip()
            if not line or line.startswith("#"):
                continue
            value = json.loads(line)
            if not isinstance(value, dict):
                raise ValueError(f"{path}:{line_number}: each row must be a JSON object")

            question = str(value.get("question") or "").strip()
            if not question:
                raise ValueError(f"{path}:{line_number}: question is required")
            index = len(rows)
            declared_env_type = str(value.get("env_type") or "").strip()
            declared_dataset = str(value.get("dataset") or "").strip()
            row_env_type = env_type.strip()
            row_dataset = dataset.strip()
            if not row_env_type or not row_dataset:
                raise ValueError(f"{path}:{line_number}: env_type and dataset are required")
            if declared_env_type and declared_env_type != row_env_type:
                raise ValueError(
                    f"{path}:{line_number}: env_type={declared_env_type!r} does not "
                    f"match --env-type {row_env_type!r}"
                )
            if declared_dataset and declared_dataset != row_dataset:
                raise ValueError(
                    f"{path}:{line_number}: dataset={declared_dataset!r} does not "
                    f"match --dataset {row_dataset!r}"
                )
            row_max_steps = int(value.get("max_steps", max_steps))
            if "max_steps" in value and row_max_steps != max_steps:
                raise ValueError(
                    f"{path}:{line_number}: max_steps={row_max_steps} does not "
                    f"match --max-steps {max_steps}"
                )
            if row_max_steps != 1:
                raise ValueError(
                    f"{path}:{line_number}: generic process-plugin training currently "
                    "supports max_steps=1 only; multi-step training needs an "
                    "environment-specific token-trace Bridge"
                )

            env_config = json_object(
                value.get("env_config"), location=f"{path}:{line_number}.env_config"
            )
            reward_config = json_object(
                value.get("reward_config"), location=f"{path}:{line_number}.reward_config"
            )
            target = value.get("target", value.get("answer"))
            if target is None and not reward_config:
                raise ValueError(
                    f"{path}:{line_number}: provide target/answer or reward_config"
                )
            if row_env_type == "qa" and target is None:
                raise ValueError(f"{path}:{line_number}: QA rows require target or answer")

            extra_info: dict[str, Any] = {
                "env_type": row_env_type,
                "dataset": row_dataset,
                "question": question,
                "case_id": str(value.get("id") or index),
                "index": index,
                "max_steps": row_max_steps,
            }
            if env_config:
                extra_info["env_config"] = env_config
            if reward_config:
                extra_info["reward_config"] = reward_config

            prompt = question
            # VeRL requires a reward_model column.  Plugin-scored environments
            # may not have a static gold answer; in that case the real score is
            # returned by UEnv and this placeholder is intentionally empty.
            ground_truth = "" if target is None else str(target)
            rows.append(
                {
                    "data_source": f"uenv/{row_dataset}",
                    "prompt": [{"role": "user", "content": prompt}],
                    "ability": row_env_type,
                    "reward_model": {"style": "rule", "ground_truth": ground_truth},
                    "extra_info": extra_info,
                }
            )
            if limit is not None and len(rows) >= limit:
                break
    if not rows:
        raise ValueError(f"{path}: no rows found")
    return rows


def write_parquet(rows: list[dict[str, Any]], output_dir: Path) -> None:
    try:
        import pandas as pd
    except ModuleNotFoundError as exc:
        raise RuntimeError(
            "pandas and pyarrow are required; install them with "
            "`python3 -m pip install pandas pyarrow`"
        ) from exc
    output_dir.mkdir(parents=True, exist_ok=True)
    frame = pd.DataFrame(rows)
    frame.to_parquet(output_dir / "train.parquet", index=False)
    # These rows are a connection check, not a benchmark split.
    frame.to_parquet(output_dir / "test.parquet", index=False)
    summary = {
        "schema_version": 1,
        "rows": len(rows),
        "note": "train.parquet and test.parquet intentionally contain the same connection-check rows",
        "columns": list(frame.columns),
    }
    (output_dir / "dataset_summary.json").write_text(
        json.dumps(summary, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Convert portable UEnv Episode JSONL to VeRL parquet."
    )
    parser.add_argument("--input", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument(
        "--env-type",
        required=True,
        help="authoritative env_type for this batch; repeated row values must match",
    )
    parser.add_argument(
        "--dataset",
        required=True,
        help="authoritative dataset route; repeated row values must match",
    )
    parser.add_argument(
        "--max-steps",
        type=int,
        required=True,
        help="authoritative maximum steps; repeated row values must match",
    )
    parser.add_argument("--limit", type=int)
    args = parser.parse_args()
    if args.limit is not None and args.limit < 1:
        parser.error("--limit must be at least 1")
    if args.max_steps < 1:
        parser.error("--max-steps must be at least 1")
    rows = load_rows(
        args.input,
        dataset=args.dataset,
        env_type=args.env_type,
        max_steps=args.max_steps,
        limit=args.limit,
    )
    write_parquet(rows, args.output_dir)
    print(f"wrote {len(rows)} row(s) to {args.output_dir}")


if __name__ == "__main__":
    main()
