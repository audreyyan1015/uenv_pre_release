"""Stable value objects used between environment.py and UEnv's transport adapter.

This file belongs to the template runtime. Environment authors normally do not
edit it; put task behavior in environment.py instead.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any


Observation = bytes | str | dict[str, Any] | list[Any]


@dataclass(slots=True)
class ResetResult:
    """The first observation returned for one Episode."""

    observation: Observation
    info: dict[str, Any] = field(default_factory=dict)


@dataclass(slots=True)
class StepResult:
    """The result of applying one action to the environment."""

    observation: Observation
    reward: float
    terminated: bool = False
    truncated: bool = False
    info: dict[str, Any] = field(default_factory=dict)

