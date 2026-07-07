"""CommandPolicy mirror for the SWE plugin (plan §1.4).

Keeps parity with the Rust ``swe::command_policy``: architecture-level modes are
only ``RestrictedShell`` / ``FullShell``; the real boundary is the container
capability profile. ``deny_patterns`` is an **MVP-only** substring helper and is
explicitly *not* a long-term security boundary (it is trivially bypassable via
``python -c``, ``/bin/curl``, ``eval`` …). Do not grow it into a blacklist.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum
from typing import List, Optional

DEFAULT_TIMEOUT_SEC = 120
DEFAULT_MAX_OUTPUT_BYTES = 65_536


class CommandMode(str, Enum):
    RESTRICTED_SHELL = "RestrictedShell"
    FULL_SHELL = "FullShell"

    @classmethod
    def parse(cls, value: str) -> Optional["CommandMode"]:
        norm = value.strip().lower().replace("_", "").replace("-", "")
        if norm in ("restrictedshell", "restricted"):
            return cls.RESTRICTED_SHELL
        if norm in ("fullshell", "full"):
            return cls.FULL_SHELL
        return None


@dataclass
class CommandPolicy:
    mode: CommandMode = CommandMode.RESTRICTED_SHELL
    timeout_sec: int = DEFAULT_TIMEOUT_SEC
    max_output_bytes: int = DEFAULT_MAX_OUTPUT_BYTES
    # ⚠️ MVP-only — see module docstring.
    deny_patterns: List[str] = field(default_factory=list)

    def wrap_command(self, command: str) -> List[str]:
        """Unified entry: every shell command runs via ``bash -lc`` (plan §1.4)."""
        return ["bash", "-lc", command]

    def first_denied(self, command: str) -> Optional[str]:
        for pat in self.deny_patterns:
            if pat and pat in command:
                return pat
        return None

    def truncate_output(self, output: str):
        data = output.encode()
        if len(data) > self.max_output_bytes:
            return data[: self.max_output_bytes].decode(errors="replace"), True
        return output, False
