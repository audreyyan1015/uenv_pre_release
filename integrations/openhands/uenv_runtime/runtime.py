"""OpenHands ``Runtime``-compatible adapter over the UEnv gateway.

OpenHands' action-execution contract (classic ``openhands.runtime.base.Runtime``
used by ``evaluation/benchmarks/swe_bench``) centres on:

    run(action: CmdRunAction)   -> CmdOutputObservation
    read(action: FileReadAction)  -> FileReadObservation
    write(action: FileWriteAction) -> FileWriteObservation

This adapter implements those entry points by **duck-typing** the action objects
(reading ``.command`` / ``.path`` / ``.content``) and forwarding to a
``UEnvSession``. Duck-typing avoids pinning UEnv to a single OpenHands release —
the same adapter works whether OpenHands ships ``Observation`` dataclasses or
plain dicts. If OpenHands observation types are importable we return those;
otherwise we return lightweight dicts with the same field names.

It deliberately does **not** subclass OpenHands' ``Runtime`` (which requires a
full sandbox/plugin stack); instead it is the minimal object an OpenHands
``swe_bench`` driver needs to send actions to UEnv. See ``run_swebench.py``.
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


# ── observation factories: prefer real OpenHands types if available ──
def _make_cmd_observation(command: str, stdout: str, stderr: str, exit_code: int):
    try:
        from openhands.events.observation import CmdOutputObservation  # type: ignore

        content = stdout if exit_code == 0 else (stdout + stderr)
        return CmdOutputObservation(content=content, command=command, exit_code=exit_code)
    except Exception:
        return {
            "observation": "run",
            "command": command,
            "content": stdout,
            "stderr": stderr,
            "exit_code": exit_code,
        }


def _make_file_read_observation(path: str, content: str):
    try:
        from openhands.events.observation import FileReadObservation  # type: ignore

        return FileReadObservation(path=path, content=content)
    except Exception:
        return {"observation": "read", "path": path, "content": content}


def _make_file_write_observation(path: str, ok: bool):
    try:
        from openhands.events.observation import FileWriteObservation  # type: ignore

        return FileWriteObservation(path=path, content="")
    except Exception:
        return {"observation": "write", "path": path, "ok": ok}
