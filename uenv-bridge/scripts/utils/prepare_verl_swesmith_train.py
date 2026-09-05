#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any, Iterable

import pandas as pd


DEFAULT_INPUT_DIR = Path("/data/ronghao/uenv/uenv-bridge/data/benchmarks/swesmith/raw/data")
DEFAULT_OUTPUT_DIR = Path("/data/ronghao/uenv/uenv-bridge/data/benchmarks/swesmith_train")
OFFICIAL_SWESMITH_PREFIX = "swebench/swesmith."

SYSTEM_PROMPT = (
    "You are fixing a real software issue in a checked-out repository. "
    "Implement the minimal fix to non-test source files using the available tools."
)


def _jsonable(value: Any) -> Any:
    if hasattr(value, "tolist"):
        return value.tolist()
    if isinstance(value, tuple):
        return list(value)
    return value


def _list_value(row: dict[str, Any], key: str) -> list[str]:
    value = _jsonable(row.get(key, []))
    if value is None:
        return []
    if isinstance(value, list):
        return [str(item) for item in value]
    return [str(value)]


def _smith_image_from_instance_id(instance_id: str) -> str:
    if "__" not in instance_id:
        return ""
    owner, rest = instance_id.split("__", 1)
    parts = rest.split(".")
    if len(parts) < 2 or not owner or not parts[0] or not parts[1]:
        return ""
    repo, commit = parts[0], parts[1][:8]
    return f"{OFFICIAL_SWESMITH_PREFIX}x86_64.{owner}_1776_{repo}.{commit}:latest".lower()


def _image_name(row: dict[str, Any]) -> str:
    image = str(row.get("image_cache_key") or row.get("image_name") or "").strip()
    if image and not image.startswith(OFFICIAL_SWESMITH_PREFIX):
        marker = image.find("swesmith.")
        if marker < 0:
            raise ValueError(f"invalid SWE-smith image: {image}")
        image = OFFICIAL_SWESMITH_PREFIX + image[marker + len("swesmith.") :]
    if not image:
        image = _smith_image_from_instance_id(str(row.get("instance_id") or ""))
    if image and ":" not in image.rsplit("/", 1)[-1]:
        image = f"{image}:latest"
    return image


def iter_rows(paths: Iterable[Path]) -> Iterable[tuple[int, dict[str, Any]]]:
    source_index = 0
    for path in paths:
        frame = pd.read_parquet(path)
        for record in frame.to_dict(orient="records"):
            row = {str(key): _jsonable(value) for key, value in record.items()}
            yield source_index, row
            source_index += 1


def build_prompt(row: dict[str, Any], workspace_dir: str) -> list[dict[str, str]]:
    repo = str(row.get("repo") or "").strip()
    user_lines = [
        f"Repository: {repo}",
        f"Workspace: {workspace_dir}",
        "",
        "<issue_description>",
        str(row.get("problem_statement") or "").strip(),
        "</issue_description>",
        "",
        "When finished, provide a concise summary of the final patch.",
    ]
    return [
        {"role": "system", "content": SYSTEM_PROMPT},
        {"role": "user", "content": "\n".join(user_lines)},
    ]


def to_verl_row(
    row: dict[str, Any],
    *,
    source_index: int,
    workspace_dir: str,
    llm_config_path: str,
    max_iterations: int,
    env_package_version: str,
    agent_mode: str,
    agent_pool_id: str,
) -> dict[str, Any]:
    instance_id = str(row["instance_id"])
    fail_to_pass = _list_value(row, "FAIL_TO_PASS")
    pass_to_pass = _list_value(row, "PASS_TO_PASS")
    patch = str(row.get("patch") or "")
    image_name = _image_name(row)
    extra_info = {
        "dataset": "swesmith",
        "split": "train",
        "source_index": source_index,
        "instance_id": instance_id,
        "repo": row.get("repo", ""),
        "repo_language": row.get("repo_language", "python"),
        "base_commit": row.get("base_commit", ""),
        "dockerhub_tag": image_name,
        "image_cache_key": image_name,
        "benchmark_variant": "smith",
        "command_mode": "full_shell",
        "env_package_id": "swe-bench-smith",
        "env_package_version": env_package_version,
        "execution_mode": "agent",
        "agent_mode": agent_mode,
        "agent_bridge_id": "uenv-agent-openhands",
        "agent_bridge_version": "1.0.0",
        "agent_pool_id": agent_pool_id,
        "driver_entrypoint": "run_swesmith_official.py",
        "workspace_dir": workspace_dir,
        "llm_config_path": llm_config_path,
        "max_iterations": max_iterations,
        "max_steps": max_iterations,
        "fail_to_pass_count": len(fail_to_pass),
        "pass_to_pass_count": len(pass_to_pass),
        "reference_patch_bytes": len(patch.encode("utf-8")),
    }
    return {
        "data_source": "swesmith",
        "prompt": build_prompt(row, workspace_dir),
        "ability": "swe",
        "reward_model": {"style": "external", "ground_truth": instance_id},
        "extra_info": extra_info,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description="Prepare VeRL-format SWE-smith training data.")
    parser.add_argument("--input-dir", type=Path, default=DEFAULT_INPUT_DIR)
    parser.add_argument("--input", type=Path, action="append", default=[])
    parser.add_argument("--output-dir", type=Path, default=DEFAULT_OUTPUT_DIR)
    parser.add_argument("--limit", type=int, default=1000, help="Number of non-empty training rows. 0 means all.")
    parser.add_argument("--offset", type=int, default=0, help="Skip this many non-empty rows before selecting.")
    parser.add_argument("--workspace-dir", default="/testbed")
    parser.add_argument("--llm-config-path", default="/root/UEnv/config/openhands-llm-qwen3-thinking-max-token-8192.json")
    parser.add_argument("--max-iterations", type=int, default=10)
    parser.add_argument("--max-steps", type=int, default=None, help="Alias for --max-iterations.")
    parser.add_argument("--env-package-version", default="0.1.0-local")
    parser.add_argument("--agent-mode", choices=["llm", "gold"], default="llm")
    parser.add_argument("--agent-pool-id", default="openhands-default")
    args = parser.parse_args()

    if args.limit < 0:
        raise SystemExit("--limit must be >= 0")
    if args.offset < 0:
        raise SystemExit("--offset must be >= 0")
    if args.max_steps is not None:
        args.max_iterations = args.max_steps

    input_paths = list(args.input)
    if not input_paths:
        input_paths = sorted(args.input_dir.glob("*.parquet"))
    if not input_paths:
        raise SystemExit(f"no parquet inputs found in {args.input_dir}")

    selected: list[tuple[int, dict[str, Any]]] = []
    seen_non_empty = 0
    for source_index, row in iter_rows(input_paths):
        if not str(row.get("problem_statement") or "").strip():
            continue
        if seen_non_empty < args.offset:
            seen_non_empty += 1
            continue
        seen_non_empty += 1
        selected.append((source_index, row))
        if args.limit and len(selected) >= args.limit:
            break

    if not selected:
        raise SystemExit(
            f"no non-empty rows selected from inputs={len(input_paths)} offset={args.offset} limit={args.limit}"
        )
    if args.limit and len(selected) < args.limit:
        raise SystemExit(f"only found {len(selected)} non-empty rows from offset={args.offset}")

    verl_rows = [
        to_verl_row(
            row,
            source_index=source_index,
            workspace_dir=args.workspace_dir,
            llm_config_path=args.llm_config_path,
            max_iterations=args.max_iterations,
            env_package_version=args.env_package_version,
            agent_mode=args.agent_mode,
            agent_pool_id=args.agent_pool_id,
        )
        for source_index, row in selected
    ]

    output_dir = args.output_dir
    output_dir.mkdir(parents=True, exist_ok=True)
    frame = pd.DataFrame(verl_rows)
    frame.to_parquet(output_dir / "train.parquet", index=False)
    frame.to_parquet(output_dir / "test.parquet", index=False)
    (output_dir / "source_rows.jsonl").write_text(
        "".join(json.dumps(row, ensure_ascii=False) + "\n" for _, row in selected),
        encoding="utf-8",
    )
    summary = {
        "input_paths": [str(path) for path in input_paths],
        "output_dir": str(output_dir),
        "rows": len(frame),
        "limit": args.limit,
        "offset": args.offset,
        "source_indices": [source_index for source_index, _ in selected],
        "instance_ids": [str(row["instance_id"]) for _, row in selected],
        "workspace_dir": args.workspace_dir,
        "llm_config_path": args.llm_config_path,
        "max_iterations": args.max_iterations,
        "max_steps": args.max_iterations,
        "env_package_id": "swe-bench-smith",
        "env_package_version": args.env_package_version,
        "benchmark_variant": "smith",
        "agent_mode": args.agent_mode,
        "note": "SWE-smith training data; selected rows exclude empty problem_statement.",
    }
    (output_dir / "dataset_summary.json").write_text(
        json.dumps(summary, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    print(json.dumps(summary, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
