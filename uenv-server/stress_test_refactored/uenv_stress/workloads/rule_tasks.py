"""Shared workload adapters for OlymMATH, SciTab, and PubMedQA.

The scale and stability suites intentionally share these pure data adapters.
Infrastructure orchestration remains separate: scale tests create a large
isolated Worker fleet, while stability tests use the formal acceptance fleet.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from uenv_stress.core.stress_test_common import rule_reward_config


TASK_NAMES = ("olymmath", "scitab", "pubmedqa")


def _read_jsonl(path: Path) -> list[dict[str, Any]]:
    return [
        json.loads(line)
        for line in path.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]


def load_task_rows(task: str, dataset_path: str | Path) -> list[dict[str, Any]]:
    """Load one of the three frozen rule-reward datasets."""
    path = Path(dataset_path)
    if task == "olymmath":
        if not path.is_dir():
            raise ValueError(f"OlymMATH dataset path must be a directory: {path}")
        rows: list[dict[str, Any]] = []
        for candidate in sorted(path.glob("OlymMATH-*.jsonl")):
            rows.extend(_read_jsonl(candidate))
    elif task == "scitab":
        value = json.loads(path.read_text(encoding="utf-8"))
        rows = value if isinstance(value, list) else list(value.values())
    elif task == "pubmedqa":
        value = json.loads(path.read_text(encoding="utf-8"))
        if not isinstance(value, dict):
            raise ValueError("PubMedQA dataset must be a PMID-keyed JSON object")
        rows = [{"pmid": str(pmid), **row} for pmid, row in value.items()]
    else:
        raise ValueError(f"unsupported rule task {task!r}; expected one of {TASK_NAMES}")
    if not rows:
        raise ValueError(f"no rows loaded for {task} from {path}")
    return rows


def item_id(task: str, row: dict[str, Any], index: int) -> str:
    if task == "olymmath":
        return str(row.get("unique_id") or row.get("id") or f"olymmath-{index}")
    if task == "scitab":
        return str(row.get("id") or row.get("qid") or f"scitab-{index}")
    if task == "pubmedqa":
        return str(row.get("pmid") or f"pubmedqa-{index}")
    raise ValueError(f"unsupported rule task {task!r}")


def build_env_payload(
    task: str,
    row: dict[str, Any],
    *,
    index: int,
    task_id: str,
    scale_marker: bool = False,
) -> tuple[dict[str, Any], dict[str, Any]]:
    """Map a frozen row to the real Math-plugin Episode contract."""
    dataset_item_id = item_id(task, row, index)
    marker = (
        f"[UENV_SCALE dataset={task} item_id={dataset_item_id}]\n"
        if scale_marker
        else ""
    )
    if task == "olymmath":
        difficulty = str(row.get("difficulty", "")).upper()
        dataset = "olymmath-hard" if difficulty == "HARD" else "olymmath-easy"
        question = f"{marker}{row['problem']}"
        env = {
            "task_name": dataset,
            "data_source": dataset,
            "dataset": dataset,
            "question": question,
            "language": row.get("language", ""),
            "difficulty": row.get("difficulty", ""),
            "subject": row.get("subject", ""),
            "task_id": task_id,
        }
        target = str(row["answer"])
    elif task == "scitab":
        table = row["table_content_values"]
        question = (
            f"{marker}Table:\n{json.dumps(table, ensure_ascii=False)}\n"
            f"Claim: {row['claim']}\nReturn supports, refutes, or not enough info."
        )
        env = {
            "task_name": "scitab",
            "data_source": "scitab",
            "dataset": "scitab",
            "question": question,
            "claim": row["claim"],
            "table_content_values": table,
            "task_id": task_id,
        }
        target = str(row["label"])
    elif task == "pubmedqa":
        context = "\n".join(str(value) for value in row["CONTEXTS"])
        question = (
            f"{marker}Context:\n{context}\nQuestion: {row['QUESTION']}\n"
            "Return yes, no, or maybe."
        )
        env = {
            "task_name": "pubmedqa",
            "data_source": "pubmedqa",
            "dataset": "pubmedqa",
            "question": question,
            "pmid": row["pmid"],
            "task_id": task_id,
        }
        target = str(row["final_decision"])
    else:
        raise ValueError(f"unsupported rule task {task!r}")
    if scale_marker:
        env["dataset_item_id"] = dataset_item_id
    return env, rule_reward_config(target)


__all__ = [
    "TASK_NAMES",
    "build_env_payload",
    "item_id",
    "load_task_rows",
    "rule_reward_config",
]
