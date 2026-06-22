"""UEnv SWE-bench plugin (OpenEnv-style) — plan §4.2 / §5.3.4 / §7.

Exposes a SWE-bench task as an OpenEnv-shaped environment (``reset`` / ``step`` /
``close``) plus an ``SweEvaluator``. Both drive the Worker **External Runtime
Gateway** (L4 HTTP) so the plugin shares the exact sandbox + grader path used by
native ``DispatchEpisode(env_type=swe)`` and the OpenHands integration — no
divergent execution contract.
"""

from .command_policy import CommandMode, CommandPolicy
from .environment import SweAction, SweEnvironment, SweObservation, StepResult
from .evaluator.swe_evaluator import EvalResult, SweEvaluator

__all__ = [
    "CommandMode",
    "CommandPolicy",
    "SweEnvironment",
    "SweAction",
    "SweObservation",
    "StepResult",
    "SweEvaluator",
    "EvalResult",
]
