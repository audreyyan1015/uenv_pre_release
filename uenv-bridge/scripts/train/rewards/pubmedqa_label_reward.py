"""Lightweight PubMedQA reward for native VeRL training.

VeRL's built-in rule rewards do not include PubMedQA.  This file is loaded
through ``reward.custom_reward_function.path`` and keeps the scoring rule aligned
with the UEnv QA worker path: extract the last whole-word label among
``yes/no/maybe`` and compare it with the gold label.
"""

from __future__ import annotations

import re
from typing import Any

LABELS = ("yes", "no", "maybe")


def _last_label(text: str) -> str | None:
    lowered = str(text or "").lower()
    best: tuple[int, str] | None = None
    for label in LABELS:
        pattern = rf"\b{re.escape(label)}\b"
        for match in re.finditer(pattern, lowered):
            if best is None or match.start() > best[0]:
                best = (match.start(), label)
    return best[1] if best else None


def _target_label(ground_truth: Any, extra_info: dict[str, Any] | None) -> str:
    for value in (
        ground_truth,
        (extra_info or {}).get("answer"),
        (extra_info or {}).get("target"),
        (extra_info or {}).get("label"),
    ):
        label = _last_label(str(value or ""))
        if label is not None:
            return label
    return str(ground_truth or "").strip().lower()


def compute_score(
    data_source: str,
    solution_str: str,
    ground_truth: Any,
    extra_info: dict[str, Any] | None = None,
    **_: Any,
) -> float:
    if str(data_source).lower() != "pubmedqa":
        raise NotImplementedError(f"PubMedQA reward received unsupported data_source={data_source!r}")
    prediction = _last_label(solution_str)
    target = _target_label(ground_truth, extra_info)
    return 1.0 if prediction is not None and prediction == target else 0.0
