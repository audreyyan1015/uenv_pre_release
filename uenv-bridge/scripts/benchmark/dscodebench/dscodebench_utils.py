from __future__ import annotations

import json
from collections import Counter
from pathlib import Path
from typing import Any


def load_dataset(
    path: Path,
    *,
    limit: int | None = None,
    library: str | None = None,
    max_per_library: int | None = None,
) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    per_library: Counter[str] = Counter()
    with path.open("r", encoding="utf-8") as file:
        for line in file:
            if not line.strip():
                continue
            row = json.loads(line)
            if library and row["library"] != library:
                continue
            if max_per_library is not None and per_library[row["library"]] >= max_per_library:
                continue
            rows.append(row)
            per_library[row["library"]] += 1
            if limit is not None and len(rows) >= limit:
                break
    return rows


def build_prompt(problem: str, *, prompt_style: str) -> str:
    if prompt_style == "official":
        return (
            "Please generate Python3 solution for the following code problem description:\n\n"
            "# Code problem description #\n"
            f"{problem}\n\n"
            "# Response #\n"
            "The return should follow the following format (replace {} with the solution). "
            'Do not generate additional code, such as "__main__" block.'
            "Solution:\n{}"
        )
    if prompt_style == "official_fenced":
        return (
            "Please generate Python3 solution for the following code problem description:\n\n"
            "# Code problem description #\n"
            f"{problem}\n\n"
            "# Response #\n"
            'Do not generate additional code, such as "__main__" block. '
            "Return only one Python markdown code block containing the solution code.\n"
            "Solution:\n```python\n"
        )
    raise ValueError(f"unknown prompt_style: {prompt_style}")
