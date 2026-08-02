#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any

import pandas as pd


DEFAULT_DATA = Path("/data/ronghao/uenv/uenv-bridge/data/benchmarks/swebenchpro/test.jsonl")
DEFAULT_OUTPUT_DIR = Path("/data/ronghao/uenv/uenv-bridge/data/benchmarks/swebenchpro_train_smoke_10")


SYSTEM_PROMPT = (
    "You are fixing a real software issue in a checked-out repository. "
    "Implement the minimal fix to non-test source files using the available tools."
)


def read_jsonl(path: Path) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    with path.open("r", encoding="utf-8") as file:
        for line in file:
            if line.strip():
                rows.append(json.loads(line))
    return rows


def build_prompt(row: dict[str, Any], workspace_dir: str) -> list[dict[str, str]]:
    user_content = "\n".join(
        [
            f"Repository: {row.get('repo', '')}",
            f"Base commit: {row.get('base_commit', '')}",
            f"Workspace: {workspace_dir}",
            "",
            "<issue_description>",
            str(row.get("problem_statement") or "").strip(),
            "</issue_description>",
            "",
            "When finished, provide a concise summary of the final patch.",
        ]
    )
    return [
        {"role": "system", "content": SYSTEM_PROMPT},
        {"role": "user", "content": user_content},
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
) -> dict[str, Any]:
    instance_id = str(row["instance_id"])
    fail_to_pass = row.get("fail_to_pass") or row.get("FAIL_TO_PASS") or []
    pass_to_pass = row.get("pass_to_pass") or row.get("PASS_TO_PASS") or []
    patch = str(row.get("patch") or "")
    extra_info = {
        "dataset": "swe-bench-pro",
        "split": "test-smoke",
        "source_index": source_index,
        "instance_id": instance_id,
        "repo": row.get("repo", ""),
        "repo_language": row.get("repo_language", ""),
        "base_commit": row.get("base_commit", ""),
        "dockerhub_tag": row.get("dockerhub_tag", ""),
        "benchmark_variant": "pro",
        "command_mode": "full_shell",
        "env_package_id": "swe-bench-pro",
        "env_package_version": env_package_version,
        "execution_mode": "agent",
        "agent_mode": agent_mode,
        "agent_bridge_id": "uenv-agent-openhands",
        "agent_bridge_version": "1.0.0",
        "agent_pool_id": "openhands-default",
        "driver_entrypoint": "run_swebenchpro_official.py",
        "workspace_dir": workspace_dir,
        "llm_config_path": llm_config_path,
        "max_iterations": max_iterations,
        "max_steps": max_iterations,
        "fail_to_pass_count": len(fail_to_pass),
        "pass_to_pass_count": len(pass_to_pass),
        "reference_patch_bytes": len(patch.encode("utf-8")),
    }
    return {
        "data_source": "swe-bench-pro",
        "prompt": build_prompt(row, workspace_dir),
        "ability": "swe",
        "reward_model": {"style": "external", "ground_truth": instance_id},
        "extra_info": extra_info,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description="Prepare a small VeRL-format SWE-bench-Pro smoke dataset.")
    parser.add_argument("--input", type=Path, default=DEFAULT_DATA)
    parser.add_argument("--output-dir", type=Path, default=DEFAULT_OUTPUT_DIR)
    parser.add_argument("--limit", type=int, default=10)
    parser.add_argument("--offset", type=int, default=0)
    parser.add_argument("--workspace-dir", default="/app")
    parser.add_argument("--llm-config-path", default="/root/UEnv/config/openhands-llm-qwen3-thinking-max-token-8192.json")
    parser.add_argument("--max-iterations", type=int, default=20)
    parser.add_argument("--max-steps", type=int, default=None, help="Alias for --max-iterations.")
    parser.add_argument("--env-package-version", default="0.3.4")
    parser.add_argument("--agent-mode", choices=["llm", "gold"], default="llm")
    args = parser.parse_args()

    if args.max_steps is not None:
        args.max_iterations = args.max_steps

    raw_rows = read_jsonl(args.input)
    selected: list[tuple[int, dict[str, Any]]] = []
    for source_index, row in enumerate(raw_rows[args.offset :], start=args.offset):
        if not str(row.get("problem_statement") or "").strip():
            continue
        selected.append((source_index, row))
        if len(selected) >= args.limit:
            break
    if len(selected) < args.limit:
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
        )
        for source_index, row in selected
    ]
    output_dir = args.output_dir
    output_dir.mkdir(parents=True, exist_ok=True)
    df = pd.DataFrame(verl_rows)
    df.to_parquet(output_dir / "train.parquet", index=False)
    df.to_parquet(output_dir / "test.parquet", index=False)
    (output_dir / "source_rows.jsonl").write_text(
        "".join(json.dumps(row, ensure_ascii=False) + "\n" for _, row in selected),
        encoding="utf-8",
    )
    summary = {
        "input": str(args.input),
        "output_dir": str(output_dir),
        "rows": len(df),
        "source_indices": [source_index for source_index, _ in selected],
        "instance_ids": [row["instance_id"] for _, row in selected],
        "workspace_dir": args.workspace_dir,
        "llm_config_path": args.llm_config_path,
        "max_iterations": args.max_iterations,
        "max_steps": args.max_iterations,
        "agent_mode": args.agent_mode,
        "note": "Smoke dataset only; source is SWE-bench-Pro public test and must not be used for final training results.",
    }
    (output_dir / "dataset_summary.json").write_text(json.dumps(summary, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(summary, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
