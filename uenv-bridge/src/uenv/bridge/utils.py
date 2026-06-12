from __future__ import annotations

from typing import Any


def to_jsonable(value: Any) -> Any:
    if value is None or isinstance(value, (str, int, float, bool)):
        return value
    if isinstance(value, bytes):
        return value.decode("utf-8", errors="replace")
    if isinstance(value, dict):
        return {str(key): to_jsonable(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [to_jsonable(item) for item in value]
    if hasattr(value, "tolist"):
        return to_jsonable(value.tolist())
    if hasattr(value, "item"):
        try:
            return value.item()
        except Exception:
            pass
    return value


def prompt_text(raw_prompt: Any) -> str:
    if isinstance(raw_prompt, list):
        parts = []
        for message in raw_prompt:
            if isinstance(message, dict):
                role = message.get("role", "")
                content = message.get("content", "")
                parts.append(f"{role}: {content}" if role else str(content))
            else:
                parts.append(str(message))
        return "\n".join(parts)
    return "" if raw_prompt is None else str(raw_prompt)
