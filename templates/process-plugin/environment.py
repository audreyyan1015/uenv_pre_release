"""Task logic for this environment — this is the main file to edit.

UEnv passes the complete Episode payload through ``config["_uenv"]`` while
also copying common ``env_config`` fields to the top level. Keep reset, state
transition and reward logic here; the gRPC/UDS adapter in plugin.py should not
need task-specific changes.

这是模板中主要需要修改的文件：
1. 用 SELF_TEST_CASE 写一条确定性的自测样例；
2. 在 Environment.reset/step/reward 中实现任务规则；
3. 不要把任务逻辑写进 plugin.py 或 generated/。
"""

from __future__ import annotations

from typing import Any

from uenv_plugin_api import ResetResult, StepResult


# 【需要修改】用一条最小样例描述 reset、action 和期望结果。
# Keep one small deterministic case beside your environment logic. The fixed
# tests/test_*.py files read this value, so custom environments do not need to
# edit the transport tests. Update the values when you replace the example.
SELF_TEST_CASE: dict[str, Any] = {
    "config": {
        "question": "Choose an action",
        "expected_action": "move-left",
        "domain_options": {"map": "warehouse-a"},
    },
    "seed": 123,
    "action": "move-left",
    "reset_observation": "Choose an action",
    "step_observation": "done",
    "reward": 1.0,
    "terminated": True,
    "truncated": False,
}


def _nested(config: dict[str, Any], *path: str) -> Any:
    value: Any = config
    for key in path:
        if not isinstance(value, dict):
            return None
        value = value.get(key)
    return value


class Environment:
    """A minimal one-step environment; replace its public task methods."""

    def __init__(self) -> None:
        self.expected_action = "ok"

    def reset(self, config: dict[str, Any], seed: int | None) -> ResetResult:
        """【需要修改】初始化 Episode，并返回模型看到的第一条 observation。"""

        question = (
            config.get("question")
            or config.get("observation")
            or _nested(config, "_uenv", "payload", "env_config", "raw_prompt")
            or "Provide an action"
        )
        self.expected_action = str(
            config.get("expected_action")
            or config.get("target")
            or _nested(config, "_uenv", "reward_config", "target")
            or _nested(
                config,
                "_uenv",
                "reward_config",
                "rubric_config",
                "ground_truth",
            )
            or "ok"
        )
        return ResetResult(
            observation=str(question),
            info={"seed": seed if seed is not None else ""},
        )

    def reward(self, action: str) -> float:
        """【需要修改】给 action 评分；多项 reward 可以在这里组合。"""

        return 1.0 if action == self.expected_action else 0.0

    def step(self, action: bytes) -> StepResult:
        """【需要修改】执行 action，并返回 observation、reward 和是否结束。"""

        action_text = action.decode("utf-8", errors="replace").strip()
        score = self.reward(action_text)
        return StepResult(
            observation="done",
            reward=score,
            terminated=True,
            info={
                "response_text": action_text,
                "expected_action": self.expected_action,
                "matched": score > 0,
            },
        )

    def close(self) -> None:
        """【按需修改】释放文件、进程、连接等任务资源。"""
