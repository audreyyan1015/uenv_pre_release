#!/usr/bin/env python3
"""Convert a local UEnv SWE-smith catalog into the Parquet rows expected by VeRL."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any


SYSTEM_PROMPT = (
    "You are fixing a real software issue in a checked-out repository. "
    "Inspect the repository, implement the smallest correct source change, and run relevant tests."
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Prepare a small SWE-smith training dataset for VeRL and UEnv.")
    parser.add_argument("--catalog", type=Path, required=True, help="UEnv SWE catalog JSON.")
    parser.add_argument(
        "--benchmark-variant",
        required=True,
        choices=("smith",),
        help="Benchmark variant. This release's VeRL data adapter supports smith.",
    )
    parser.add_argument("--output-dir", type=Path, required=True, help="Directory for train.parquet and test.parquet.")
    parser.add_argument(
        "--limit",
        type=int,
        help="Maximum rows when --instance is not used; 0 selects all rows.",
    )
    parser.add_argument("--instance", action="append", default=[], help="Select an instance ID; may be repeated.")
    parser.add_argument(
        "--max-iterations",
        type=int,
        required=True,
        help="Maximum OpenHands steps for each episode.",
    )
    parser.add_argument(
        "--llm-config-path",
        default="/etc/uenv/openhands-llm.json",
        help="LLM template path on the UEnv Agent host.",
    )
    return parser.parse_args()


def load_catalog(path: Path) -> list[dict[str, Any]]:
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError as exc:
        raise SystemExit(f"catalog not found: {path}") from exc
    except json.JSONDecodeError as exc:
        raise SystemExit(f"catalog is not valid JSON: {path}: {exc}") from exc

    if isinstance(document, dict):
        rows = []
        for instance_id, value in document.items():
            if not isinstance(value, dict):
                continue
            row = dict(value)
            row.setdefault("instance_id", instance_id)
            rows.append(row)
        return rows
    if isinstance(document, list):
        return [dict(value) for value in document if isinstance(value, dict)]
    raise SystemExit("catalog root must be an object or a list")


def image_name(row: dict[str, Any]) -> str:
    image = str(row.get("image_cache_key") or row.get("image_name") or "").strip()
    if image and ":" not in image.rsplit("/", 1)[-1]:
        image += ":latest"
    return image


def validate_smith_row(row: dict[str, Any]) -> None:
    instance_id = str(row.get("instance_id") or "<unknown>")
    variant = str(row.get("benchmark_variant") or row.get("version") or "").lower()
    image = str(row.get("image_cache_key") or "").strip().lower()
    if "smith" not in variant:
        raise SystemExit(f"catalog row is not SWE-smith: {instance_id} (variant={variant!r})")
    if "swesmith" not in image:
        raise SystemExit(f"catalog row has no SWE-smith image_cache_key: {instance_id} (image={image!r})")


def prompt(row: dict[str, Any]) -> list[dict[str, str]]:
    user = "\n".join(
        [
            f"Repository: {str(row.get('repo') or '').strip()}",
            "Workspace: /testbed",
            "",
            "<issue_description>",
            str(row.get("problem_statement") or "").strip(),
            "</issue_description>",
            "",
            "When finished, provide a concise summary of the final patch.",
        ]
    )
    return [{"role": "system", "content": SYSTEM_PROMPT}, {"role": "user", "content": user}]


def verl_row(row: dict[str, Any], index: int, args: argparse.Namespace) -> dict[str, Any]:
    instance_id = str(row["instance_id"])
    image = image_name(row)
    return {
        "data_source": "swesmith",
        "prompt": prompt(row),
        "ability": "swe",
        "reward_model": {"style": "external", "ground_truth": instance_id},
        "extra_info": {
            "dataset": "swesmith",
            "split": "train",
            "index": index,
            "source_index": index,
            "instance_id": instance_id,
            "repo": str(row.get("repo") or ""),
            "repo_language": str(row.get("repo_language") or "python"),
            "base_commit": str(row.get("base_commit") or ""),
            "dockerhub_tag": image,
            "image_cache_key": image,
            "benchmark_variant": args.benchmark_variant,
            "command_mode": "full_shell",
            "env_package_id": "",
            "env_package_version": "",
            "execution_mode": "agent",
            "agent_mode": "llm",
            "agent_bridge_id": "uenv-agent-openhands",
            "agent_bridge_version": "1.0.0",
            "agent_pool_id": "openhands-default",
            "driver_entrypoint": "run_swebenchpro_official.py",
            "workspace_dir": "/testbed",
            "llm_config_path": args.llm_config_path,
            "max_iterations": args.max_iterations,
            "max_steps": args.max_iterations,
        },
    }


def main() -> None:
    args = parse_args()
    if not args.instance and args.limit is None:
        raise SystemExit("provide --instance ID (repeatable) or --limit N")
    if args.instance and args.limit is not None:
        raise SystemExit("--instance and --limit are mutually exclusive")
    if args.limit is not None and args.limit < 0:
        raise SystemExit("--limit must be 0 or greater")
    if args.max_iterations < 1:
        raise SystemExit("--max-iterations must be positive")

    catalog_rows = load_catalog(args.catalog)
    for row in catalog_rows:
        validate_smith_row(row)

    requested = set(args.instance)
    source_rows = [
        row
        for row in catalog_rows
        if str(row.get("instance_id") or "")
        and str(row.get("problem_statement") or "").strip()
        and (not requested or str(row.get("instance_id")) in requested)
    ]
    source_rows.sort(key=lambda row: str(row["instance_id"]))
    if requested:
        missing = sorted(requested - {str(row["instance_id"]) for row in source_rows})
        if missing:
            raise SystemExit(f"instances not found or missing a problem statement: {', '.join(missing)}")
    if args.limit and not requested:
        source_rows = source_rows[: args.limit]
    if not source_rows:
        raise SystemExit("no usable rows selected")

    try:
        import pandas as pd
    except ImportError as exc:
        raise SystemExit("pandas and pyarrow are required: python3 -m pip install pandas pyarrow") from exc

    rows = [verl_row(row, index, args) for index, row in enumerate(source_rows)]
    args.output_dir.mkdir(parents=True, exist_ok=True)
    frame = pd.DataFrame(rows)
    try:
        frame.to_parquet(args.output_dir / "train.parquet", index=False)
        frame.to_parquet(args.output_dir / "test.parquet", index=False)
    except ImportError as exc:
        raise SystemExit("a Parquet engine is required: python3 -m pip install pyarrow") from exc

    summary = {
        "catalog": str(args.catalog.resolve()),
        "output_dir": str(args.output_dir.resolve()),
        "rows": len(rows),
        "instances": [str(row["instance_id"]) for row in source_rows],
        "max_iterations": args.max_iterations,
        "benchmark_variant": args.benchmark_variant,
    }
    (args.output_dir / "dataset_summary.json").write_text(
        json.dumps(summary, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    print(json.dumps(summary, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
