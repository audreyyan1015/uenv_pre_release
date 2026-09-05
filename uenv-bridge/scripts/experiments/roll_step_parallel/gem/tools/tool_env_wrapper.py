from __future__ import annotations

from typing import Any


class ToolEnvWrapper:
    """Small pass-through fallback for ROLL agentic env imports.

    The reproduction configs used here do not enable tool wrappers. If a config
    does enable tools, this wrapper keeps the standard reset/step API and stores
    the provided tool metadata, but does not execute external tools.
    """

    def __init__(self, env: Any, tools: list[Any] | None = None, **kwargs: Any) -> None:
        self.env = env
        self.tools = tools or []
        self.wrapper_kwargs = kwargs

    def reset(self, *args: Any, **kwargs: Any):
        return self.env.reset(*args, **kwargs)

    def step(self, *args: Any, **kwargs: Any):
        return self.env.step(*args, **kwargs)

    def close(self):
        close = getattr(self.env, "close", None)
        if close:
            return close()

    def __getattr__(self, name: str) -> Any:
        return getattr(self.env, name)
