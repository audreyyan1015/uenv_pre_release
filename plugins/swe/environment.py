"""OpenEnv-style SWE environment over the UEnv gateway (plan §4.2 / §5.3.4).

Contract (OpenEnv ``Environment``):

    reset()        -> SweObservation        # provision sandbox, return issue_text
    step(action)   -> StepResult            # bash -lc / read / write inside sandbox
    evaluate()     -> EvalResult            # apply test_patch, run tests, grade
    close()                                  # release the sandbox

All container I/O is delegated to the Worker L4 gateway, so the plugin reuses the
same L2 pool / L1 backend / grader as native ``DispatchEpisode``. ``deny_patterns``
(if configured) are enforced client-side as an MVP convenience before forwarding.
"""

from __future__ import annotations

import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Dict, Optional

# Reuse the validated gateway client from the OpenHands integration.
_INTEG = Path(__file__).resolve().parents[2] / "integrations" / "openhands"
if str(_INTEG) not in sys.path:
    sys.path.insert(0, str(_INTEG))
from uenv_runtime.client import UEnvGatewayClient, UEnvSession  # noqa: E402

from .command_policy import CommandMode, CommandPolicy
from .evaluator.swe_evaluator import EvalResult, SweEvaluator


@dataclass
class SweObservation:
    instance_id: str
    issue_text: str
    benchmark_variant: str
    command_mode: str
    session_id: str


@dataclass
class SweAction:
    """An OpenEnv action. ``type`` is one of: exec | read | write | apply_patch."""

    type: str
    command: Optional[str] = None
    path: Optional[str] = None
    content: Optional[str] = None


@dataclass
class StepResult:
    observation: Dict[str, Any]
    reward: float = 0.0
    done: bool = False
    info: Dict[str, Any] = field(default_factory=dict)


class SweEnvironment:
    """One SWE-bench task bound to one gateway session."""

    def __init__(
        self,
        instance_id: str,
        gateway_url: str = "127.0.0.1:48999",
        benchmark_variant: str = "verified",
        policy: Optional[CommandPolicy] = None,
        api_key: Optional[str] = None,
    ):
        self.instance_id = instance_id
        self.benchmark_variant = benchmark_variant
        self.policy = policy or CommandPolicy(mode=CommandMode.FULL_SHELL)
        self._client = UEnvGatewayClient(gateway_url, api_key=api_key)
        self._session: Optional[UEnvSession] = None

    # ── OpenEnv API ──────────────────────────────────────────────────
    def reset(self) -> SweObservation:
        self.close()
        self._session = self._client.create_session(
            self.instance_id, self.benchmark_variant, self.policy.mode.value
        )
        s = self._session
        return SweObservation(
            instance_id=s.instance_id,
            issue_text=s.issue_text,
            benchmark_variant=s.benchmark_variant,
            command_mode=s.command_mode,
            session_id=s.session_id,
        )

    def step(self, action: SweAction) -> StepResult:
        if self._session is None:
            raise RuntimeError("call reset() before step()")
        s = self._session

        if action.type == "exec":
            cmd = action.command or ""
            denied = self.policy.first_denied(cmd)
            if denied is not None:
                return StepResult(
                    observation={"stdout": "", "stderr": f"denied pattern: {denied}", "exit_code": 126},
                    info={"denied": denied},
                )
            r = s.exec(cmd)
            out, truncated = self.policy.truncate_output(r.stdout)
            return StepResult(
                observation={"stdout": out, "stderr": r.stderr, "exit_code": r.exit_code, "truncated": truncated},
                info={"command": cmd},
            )
        if action.type == "read":
            return StepResult(observation={"content": s.read(action.path or "")})
        if action.type == "write":
            ok = s.write(action.path or "", action.content or "")
            return StepResult(observation={"ok": ok})
        if action.type == "apply_patch":
            r = s.apply_patch(action.content or "")
            return StepResult(
                observation={"stdout": r.stdout, "stderr": r.stderr, "exit_code": r.exit_code},
                info={"action": "apply_patch"},
            )
        raise ValueError(f"unknown action type: {action.type}")

    def evaluate(self) -> EvalResult:
        """Apply test_patch + run tests + grade (server-side grader)."""
        if self._session is None:
            raise RuntimeError("call reset() before evaluate()")
        return SweEvaluator().evaluate(self._session)

    def close(self) -> None:
        if self._session is not None:
            try:
                self._session.destroy()
            finally:
                self._session = None

    def __enter__(self) -> "SweEnvironment":
        self.reset()
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        self.close()
