"""UEnv workspace helpers without OpenHands SDK dependencies."""

from __future__ import annotations

from typing import Any


def is_uenv_gateway_workspace(ws: Any) -> bool:
    """Detect UEnv remote workspace without relying on isinstance (uv dual-import safe)."""
    if ws is None:
        return False
    if getattr(ws, "uenv_gateway_workspace", False):
        return True
    if getattr(ws, "gateway_url", None):
        return True
    return type(ws).__name__ == "UEnvWorkspace"
