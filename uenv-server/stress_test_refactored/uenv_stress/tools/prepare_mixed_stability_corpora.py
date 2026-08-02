#!/usr/bin/env python3
"""Doubao/Qwen 混合稳定性 replay 语料构建工具。

这个文件把来自不同模型或不同采集批次的真实轨迹按数据集和样本 ID 对齐，生成可审计的混合 replay 语料。它用于在稳定性验收中覆盖多个模型来源，同时保持样本配对关系和输入输出可追溯。

实现逻辑是：read_jsonl 读取各源语料；index_by_dataset_id 按 dataset_id 建索引；paired_rows 选择两侧都存在或满足策略要求的样本；attach_replay_waits 根据观测时延补充 replay 等待时间；write_jsonl 写出新语料；main 复制必要输入、写 manifest 和摘要，记录源文件、样本数、配对数量和 checksum。"""

from __future__ import annotations

import argparse
import json
import shutil
import statistics
import time
from pathlib import Path
from typing import Any

from uenv_stress.core.stability_test_common import (
    PAIRED_TASK_NAMES,
    sha256_file,
    source_model_family,
    trace_dataset_id,
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


def positive_number(value: Any) -> float:
    if isinstance(value, bool):
        return 0.0
    try:
        number = float(value)
    except (TypeError, ValueError):
        return 0.0
    return number if number > 0 else 0.0


def freeze_wait_profile(row: dict[str, Any]) -> dict[str, Any]:
    total_ms = positive_number(row.get("source_api_latency_ms"))
    latency_source = "recorded" if total_ms else ""
    if not total_ms:
        total_ms = positive_number(row.get("episode_total_ms"))
        latency_source = "observed_episode_elapsed_proxy" if total_ms else ""
    turns = [
        turn
        for turn in row.get("turns", [])
        if isinstance(turn, dict)
    ]
    if not total_ms:
        total_ms = sum(positive_number(turn.get("env_latency_ms")) for turn in turns)
        latency_source = "observed_episode_elapsed_proxy" if total_ms else ""
    if total_ms <= 0 or not latency_source:
        raise ValueError(
            f"{trace_dataset_id(row)} has no positive latency to freeze into replay_wait_ms"
        )
    if not turns:
        raise ValueError(f"{trace_dataset_id(row)} has no turns")
    if len(turns) == 1:
        waits_ms = [total_ms]
    else:
        tokens = [
            positive_number(
                turn.get("target_qwen3_tokens")
                or turn.get("source_completion_tokens")
            )
            for turn in turns
        ]
        token_total = sum(tokens)
        if token_total <= 0:
            raise ValueError(
                f"{trace_dataset_id(row)} has no positive completion tokens"
            )
        waits_ms = [total_ms * value / token_total for value in tokens]
        waits_ms[-1] = total_ms - sum(waits_ms[:-1])
    return {
        "episode_elapsed_proxy_ms": total_ms,
        "latency_source": latency_source,
        "replay_wait_ms": waits_ms,
    }


def latency_proxy_ms(row: dict[str, Any]) -> float:
    """与 freeze_wait_profile 相同的优先级取 Episode 级时延代理。"""
    total_ms = positive_number(row.get("source_api_latency_ms"))
    if not total_ms:
        total_ms = positive_number(row.get("episode_total_ms"))
    if not total_ms:
        total_ms = sum(
            positive_number(turn.get("env_latency_ms"))
            for turn in row.get("turns", [])
            if isinstance(turn, dict)
        )
    return total_ms


def attach_replay_waits(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """Freeze the shared formal/scale per-turn waiting plan into each trace.

    缺失时延代理的轨迹允许用同文件（同数据集/同模型）中位数填充，
    但缺失比例超过 5% 时 fail closed（与 README 的 Mixed replay 语义一致）。
    """
    proxies = [latency_proxy_ms(row) for row in rows]
    missing = [index for index, proxy in enumerate(proxies) if proxy <= 0]
    median_ms = 0.0
    if missing:
        share = len(missing) / max(len(rows), 1)
        if share > 0.05:
            raise ValueError(
                f"{len(missing)}/{len(rows)} traces miss latency proxies; "
                "above the 5% median-imputation limit"
            )
        present = [proxy for proxy in proxies if proxy > 0]
        median_ms = statistics.median(present)
    output: list[dict[str, Any]] = []
    for index, original in enumerate(rows):
        row = dict(original)
        imputed = proxies[index] <= 0
        if imputed:
            row["episode_total_ms"] = median_ms
        turns = [
            dict(turn)
            for turn in row.get("turns", [])
            if isinstance(turn, dict)
        ]
        wait_profile = freeze_wait_profile(row)
        if imputed:
            wait_profile["latency_source"] = "dataset_median_imputed"
        waits = wait_profile["replay_wait_ms"]
        if len(turns) != len(waits):
            raise ValueError(
                f"{trace_dataset_id(row)} has inconsistent turn/wait counts"
            )
        for turn, wait_ms in zip(turns, waits, strict=True):
            turn["replay_wait_ms"] = float(wait_ms)
            turn["latency_source"] = str(wait_profile["latency_source"])
            turn["episode_elapsed_proxy_ms"] = float(
                wait_profile["episode_elapsed_proxy_ms"]
            )
        row["turns"] = turns
        row["latency_basis"] = "frozen_replay_wait_ms"
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
