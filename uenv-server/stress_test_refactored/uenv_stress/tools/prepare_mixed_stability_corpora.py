#!/usr/bin/env python3
"""Build versioned, auditable Doubao/Qwen stability replay corpora."""

from __future__ import annotations

import argparse
import json
import shutil
import time
from pathlib import Path
from typing import Any

from uenv_stress.core.stability_test_common import (
    PAIRED_TASK_NAMES,
    latency_imputation_medians,
    sha256_file,
    source_model_family,
    trace_dataset_id,
    trace_turn_waits,
    validate_paired_trace_order,
)


RELATIVE_PATHS = {
    "dscodebench": Path("dscodebench/dscodebench.jsonl"),
    "swebench_pro": Path("swebench_pro/swebench_pro.jsonl"),
    "olymmath": Path("math/olymmath.jsonl"),
    "scitab": Path("math/scitab.jsonl"),
    "pubmedqa": Path("math/pubmedqa.jsonl"),
}


def read_jsonl(path: Path) -> list[dict[str, Any]]:
    if not path.is_file():
        raise ValueError(f"trace corpus is missing: {path}")
    rows = [
        json.loads(line)
        for line in path.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]
    if not rows:
        raise ValueError(f"trace corpus is empty: {path}")
    return rows


def index_by_dataset_id(
    rows: list[dict[str, Any]], *, path: Path
) -> dict[str, dict[str, Any]]:
    result: dict[str, dict[str, Any]] = {}
    for row in rows:
        dataset_id = trace_dataset_id(row)
        if dataset_id in result:
            raise ValueError(f"{path} contains duplicate dataset_id {dataset_id}")
        result[dataset_id] = row
    return result


def paired_rows(
    task: str,
    doubao_rows: list[dict[str, Any]],
    qwen_rows: list[dict[str, Any]],
    *,
    expected_pairs: int,
) -> tuple[list[dict[str, Any]], dict[str, Any]]:
    if len(doubao_rows) != expected_pairs:
        raise ValueError(
            f"{task} Doubao corpus has {len(doubao_rows)} rows; "
            f"expected exactly {expected_pairs}"
        )
    doubao_index = index_by_dataset_id(
        doubao_rows, path=Path(f"{task}:doubao")
    )
    qwen_index = index_by_dataset_id(qwen_rows, path=Path(f"{task}:qwen"))
    missing_qwen = sorted(set(doubao_index) - set(qwen_index))
    if missing_qwen:
        raise ValueError(f"{task} Qwen corpus misses paired IDs: {missing_qwen}")

    output: list[dict[str, Any]] = []
    for pair_index, doubao_original in enumerate(doubao_rows):
        pair_id = trace_dataset_id(doubao_original)
        qwen_original = qwen_index[pair_id]
        if source_model_family(doubao_original.get("source_model")) != "doubao":
            raise ValueError(f"{task}/{pair_id} baseline row is not Doubao")
        if source_model_family(qwen_original.get("source_model")) != "qwen":
            raise ValueError(f"{task}/{pair_id} paired row is not Qwen")
        doubao = dict(doubao_original)
        qwen = dict(qwen_original)
        for row, family in ((doubao, "doubao"), (qwen, "qwen")):
            row["dataset_id"] = pair_id
            row["pair_id"] = pair_id
            row["pair_index"] = pair_index
            row["source_family"] = family
            row["pairing_strategy"] = "same_dataset_id_doubao_then_qwen"
        output.extend((doubao, qwen))

    evidence = validate_paired_trace_order(
        output, expected_pairs=expected_pairs
    )
    evidence.update(
        {
            "doubao_input_count": len(doubao_rows),
            "qwen_input_count": len(qwen_rows),
            "qwen_selected_count": expected_pairs,
            "qwen_extra_count": len(set(qwen_index) - set(doubao_index)),
            "pair_identity_basis": "dataset_id",
        }
    )
    return output, evidence


def write_jsonl(path: Path, rows: list[dict[str, Any]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8", newline="\n") as target:
        for row in rows:
            target.write(
                json.dumps(row, ensure_ascii=False, sort_keys=True)
                + "\n"
            )


def attach_replay_waits(
    rows: list[dict[str, Any]], *, max_missing_ratio: float = 0.05
) -> list[dict[str, Any]]:
    """Freeze the shared formal/scale per-turn waiting plan into each trace."""
    medians = latency_imputation_medians(
        rows, max_missing_ratio=max_missing_ratio
    )
    output: list[dict[str, Any]] = []
    for original in rows:
        row = dict(original)
        turns = [
            dict(turn)
            for turn in row.get("turns", [])
            if isinstance(turn, dict)
        ]
        wait_profile = trace_turn_waits(row, imputation_medians=medians)
        waits = wait_profile["turn_proxy_wait_seconds"]
        if len(turns) != len(waits):
            raise ValueError(
                f"{trace_dataset_id(row)} has inconsistent turn/wait counts"
            )
        for turn, wait_seconds in zip(turns, waits, strict=True):
            turn["replay_wait_ms"] = float(wait_seconds) * 1000.0
            turn["latency_source"] = str(wait_profile["latency_source"])
            turn["episode_elapsed_proxy_ms"] = float(
                wait_profile["episode_elapsed_proxy_ms"]
            )
        row["turns"] = turns
        row["latency_basis"] = "observed_episode_elapsed_proxy"
        output.append(row)
    return output


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--doubao-root",
        type=Path,
        default=Path("/home/uenv-stress/trace-corpora"),
    )
    parser.add_argument(
        "--qwen-root",
        type=Path,
        default=Path("/home/uenv-stress/trace-corpora-qwen3-backup-20260726"),
    )
    parser.add_argument(
        "--output-root",
        type=Path,
        default=Path("/home/uenv-stress/trace-corpora-mixed-20260726"),
    )
    parser.add_argument("--expected-pairs", type=int, default=100)
    args = parser.parse_args()
    if args.expected_pairs <= 0:
        parser.error("--expected-pairs must be positive")
    output_root = args.output_root.resolve()
    if output_root.exists():
        raise ValueError(
            f"refusing to overwrite existing versioned output: {output_root}"
        )
    temporary = output_root.with_name(
        f".{output_root.name}.tmp-{int(time.time())}"
    )
    if temporary.exists():
        raise ValueError(f"temporary output already exists: {temporary}")

    pairing: dict[str, Any] = {}
    files: list[dict[str, Any]] = []
    try:
        for task in PAIRED_TASK_NAMES:
            relative = RELATIVE_PATHS[task]
            doubao_path = args.doubao_root / relative
            qwen_path = args.qwen_root / relative
            rows, evidence = paired_rows(
                task,
                read_jsonl(doubao_path),
                read_jsonl(qwen_path),
                expected_pairs=args.expected_pairs,
            )
            rows = attach_replay_waits(rows)
            output_path = temporary / relative
            write_jsonl(output_path, rows)
            pairing[task] = evidence
            files.append(
                {
                    "task": task,
                    "path": relative.as_posix(),
                    "size_bytes": output_path.stat().st_size,
                    "sha256": sha256_file(output_path),
                    "source_files": {
                        "doubao": str(doubao_path.resolve()),
                        "qwen": str(qwen_path.resolve()),
                    },
                }
            )

        swe_relative = RELATIVE_PATHS["swebench_pro"]
        swe_source = args.doubao_root / swe_relative
        swe_rows = read_jsonl(swe_source)
        if len(swe_rows) != 50:
            raise ValueError(
                f"SWE-bench Pro corpus has {len(swe_rows)} rows; expected 50"
            )
        if any(
            source_model_family(row.get("source_model")) != "doubao"
            for row in swe_rows
        ):
            raise ValueError("SWE-bench Pro corpus must contain Doubao only")
        for index, row in enumerate(swe_rows):
            pair_id = trace_dataset_id(row)
            row["dataset_id"] = pair_id
            row["pair_id"] = pair_id
            row["pair_index"] = index
            row["source_family"] = "doubao"
            row["pairing_strategy"] = "doubao_only_round_robin"
        swe_rows = attach_replay_waits(swe_rows)
        swe_output = temporary / swe_relative
        write_jsonl(swe_output, swe_rows)
        files.append(
            {
                "task": "swebench_pro",
                "path": swe_relative.as_posix(),
                "size_bytes": swe_output.stat().st_size,
                "sha256": sha256_file(swe_output),
                "source_files": {"doubao": str(swe_source.resolve())},
            }
        )

        stats = {
            "schema_version": 1,
            "created_unix": time.time(),
            "selection_strategy": "paired_alternating_episode",
            "pairing_strategy": "same_dataset_id_doubao_then_qwen",
            "expected_pairs": args.expected_pairs,
            "pairing": pairing,
            "swebench_pro": {"doubao_only": True, "trace_count": 50},
            "files": files,
        }
        stats_path = temporary / "mixed_corpus_stats.json"
        stats_path.write_text(
            json.dumps(stats, ensure_ascii=False, indent=2, sort_keys=True),
            encoding="utf-8",
        )
        output_root.parent.mkdir(parents=True, exist_ok=True)
        temporary.replace(output_root)
    except Exception:
        if temporary.exists():
            shutil.rmtree(temporary)
        raise
    print(
        json.dumps(
            {
                "output_root": str(output_root),
                "paired_tasks": list(PAIRED_TASK_NAMES),
                "pairs_per_task": args.expected_pairs,
                "swebench_pro_traces": 50,
            },
            ensure_ascii=False,
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
