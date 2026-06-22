"""OpenHands ``Runtime``-compatible adapter over the UEnv gateway.

OpenHands' action-execution contract (classic ``openhands.runtime.base.Runtime``
used by ``evaluation/benchmarks/swe_bench``) centres on:

    run(action: CmdRunAction)   -> CmdOutputObservation
    read(action: FileReadAction)  -> FileReadObservation
    write(action: FileWriteAction) -> FileWriteObservation

This adapter implements those entry points by **duck-typing** the action objects
(reading ``.command`` / ``.path`` / ``.content``) and forwarding to a
``UEnvSession``. **It has zero dependency on the OpenHands package** (we never
``import openhands``): actions are read by attribute/key and observations are
returned as OpenHands-shaped plain dicts (same field names). Duck-typing avoids
pinning UEnv to any OpenHands release.

Decoupling decision (confirmed): this is an **independent rewrite**, not a
subclass of OpenHands' ``Runtime``. This deviates from plan §5.3.3 (which assumed
implementing/subclassing the classic OpenHands ``Runtime`` and driving via
``evaluation/benchmarks/swe_bench``). Reason: the vendored OpenHands (``openhands-ai``)
is the new ``app_server``/SDK architecture and ships **no** classic
``openhands.runtime.base.Runtime``, **no** ``openhands.events.observation``, and
**no** ``benchmarks/swe_bench``. See README "Design notes".
"""

from __future__ import annotations

from typing import Any, Optional

from .client import UEnvGatewayClient, UEnvSession


def _attr(obj: Any, *names: str, default: Any = None) -> Any:
    """Read the first present attribute (or dict key) from ``obj``."""
    for name in names:
        if isinstance(obj, dict) and name in obj:
            return obj[name]
        if hasattr(obj, name):
            return getattr(obj, name)
    return default


class UEnvRuntime:
    """Drives a single UEnv gateway session through OpenHands-style actions."""

    def __init__(
        self,
        gateway_url: str,
        instance_id: str,
        benchmark_variant: str = "verified",
        command_mode: str = "FullShell",
        api_key: Optional[str] = None,
        timeout: float = 600.0,
    ):
        self._client = UEnvGatewayClient(gateway_url, timeout=timeout, api_key=api_key)
        self._instance_id = instance_id
        self._benchmark_variant = benchmark_variant
        self._command_mode = command_mode
        self._session: Optional[UEnvSession] = None

    # ── lifecycle (mirrors Runtime.connect / close) ──────────────────
    def connect(self) -> UEnvSession:
        """Create the gateway session (OpenHands calls this before stepping)."""
        if self._session is None:
            self._session = self._client.create_session(
                self._instance_id, self._benchmark_variant, self._command_mode
            )
        return self._session

    def close(self) -> None:
        if self._session is not None:
            try:
                self._session.destroy()
            finally:
                self._session = None

    @property
    def session(self) -> UEnvSession:
        if self._session is None:
            return self.connect()
        return self._session

    @property
    def task_instruction(self) -> str:
        """The SWE-bench problem statement (OpenHands uses this as the prompt)."""
        return self.session.issue_text

    # ── OpenHands Runtime action handlers (duck-typed) ───────────────
    def run(self, action: Any) -> Any:
        """Handle a ``CmdRunAction``-shaped object."""
        command = _attr(action, "command", "cmd", default="")
        r = self.session.exec(command)
        return _make_cmd_observation(command, r.stdout, r.stderr, r.exit_code)

    def read(self, action: Any) -> Any:
        """Handle a ``FileReadAction``-shaped object."""
        path = _attr(action, "path", "filepath", default="")
        content = self.session.read(path)
        return _make_file_read_observation(path, content)

    def write(self, action: Any) -> Any:
        """Handle a ``FileWriteAction``-shaped object."""
        path = _attr(action, "path", "filepath", default="")
        content = _attr(action, "content", "contents", default="")
        ok = self.session.write(path, content)
        return _make_file_write_observation(path, ok)

    def run_action(self, action: Any) -> Any:
        """Dispatch by action class name (OpenHands' generic entry point)."""
        cls = type(action).__name__
        if "Read" in cls:
            return self.read(action)
        if "Write" in cls:
            return self.write(action)
        return self.run(action)

    # ── SWE-bench convenience (used by the driver / eval harness) ────
    def apply_patch(self, patch: str):
        return self.session.apply_patch(patch)

    def submit(self):
        return self.session.submit()

    def __enter__(self) -> "UEnvRuntime":
        self.connect()
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        self.close()


# ── observation factories ────────────────────────────────────────────
# Decoupling decision (confirmed): this integration does NOT depend on the
# OpenHands package. Observations are plain OpenHands-shaped dicts (same field
# names as `CmdOutputObservation` / `FileReadObservation` / `FileWriteObservation`)
# so a real OpenHands driver can consume them, but we never `import openhands`.
# This deviates from plan §5.3.3 (which assumed subclassing OpenHands' classic
# `Runtime`); see README "Design notes" for why (the vendored OpenHands is the
# new app_server/SDK architecture with no classic Runtime / benchmarks/swe_bench).
def _make_cmd_observation(command: str, stdout: str, stderr: str, exit_code: int):
    return {
        "observation": "run",
        "command": command,
        "content": stdout,
        "stderr": stderr,
        "exit_code": exit_code,
    }


def _make_file_read_observation(path: str, content: str):
    return {"observation": "read", "path": path, "content": content}


def _make_file_write_observation(path: str, ok: bool):
    return {"observation": "write", "path": path, "ok": ok}
