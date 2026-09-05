"""Minimal GEM compatibility shim for ROLL reproduction runs.

ROLL currently ships a tiny ``gem`` stub in its repo root. That stub is enough
for simple ``gem.make`` calls, but Agentic env managers also import
``gem.tools.tool_env_wrapper``. Keeping this shim under the reproduction script
directory lets us satisfy that import without modifying the ROLL checkout.
"""

from __future__ import annotations

import importlib
from typing import Any

import gymnasium as gym


_REGISTRY: dict[str, str] = {}


class Env:
    """Small GEM base class.

    ROLL envs inherit from both ``gem.Env`` and concrete gymnasium env classes.
    Keeping this class independent from ``gymnasium.Env`` avoids MRO conflicts
    for built-in envs such as FrozenLake.
    """

    def reset(self, seed: int | None = None, **kwargs: Any) -> None:
        self.seed = seed
        return None


def register(id: str, entry_point: str, **kwargs: Any) -> None:
    _REGISTRY[id] = entry_point
    try:
        gym.register(id=id, entry_point=entry_point, **kwargs)
    except Exception:
        # ROLL may import its env registration more than once in Ray workers.
        pass


def make(id: str | None = None, env_id: str | None = None, **kwargs: Any):
    name = env_id or id
    if not name:
        raise ValueError("gem.make requires id or env_id")
    entry_point = _REGISTRY.get(name)
    if entry_point:
        module_name, class_name = entry_point.split(":", 1)
        module = importlib.import_module(module_name)
        env_cls = getattr(module, class_name)
        return env_cls(**kwargs)
    return gym.make(name, **kwargs)


spec = gym.spec
