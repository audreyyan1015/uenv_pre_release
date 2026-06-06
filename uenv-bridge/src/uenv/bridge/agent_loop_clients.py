from __future__ import annotations

import json
import os
from dataclasses import dataclass
from typing import Any

from .clients import EpisodeClient, RustCoreClientConfig, RustCoreEpisodeClient
from .protocol import EpisodeRequest, EpisodeResult, EpisodeSummary, StepRecord, Trajectory


@dataclass(slots=True)
class AgentLoopClientConfig:
    mode: str = "fake"
    endpoint: str = "127.0.0.1:50051"
    timeout_seconds: float = 300.0
    startup_timeout_seconds: float = 30.0
    auto_start: bool = False
    binary: str | None = None
    fake_reward: float = 1.0
    fake_response_text: str = ""

    @classmethod
    def from_env(
        cls,
        *,
        mode: str | None = None,
        endpoint: str | None = None,
        timeout_seconds: float | None = None,
        startup_timeout_seconds: float | None = None,
        auto_start: bool | None = None,
        binary: str | None = None,
        fake_reward: float | None = None,
        fake_response_text: str | None = None,
    ) -> "AgentLoopClientConfig":
        return cls(
            mode=mode or os.getenv("UENV_AGENT_LOOP_CLIENT", "fake"),
            endpoint=endpoint or os.getenv("UENV_ADAPTER_CORE_ENDPOINT", "127.0.0.1:50051"),
            timeout_seconds=float(timeout_seconds if timeout_seconds is not None else os.getenv("UENV_AGENT_LOOP_TIMEOUT_SECONDS", "300")),
            startup_timeout_seconds=float(
                startup_timeout_seconds
                if startup_timeout_seconds is not None
                else os.getenv("UENV_ADAPTER_CORE_STARTUP_TIMEOUT_SECONDS", "30")
            ),
            auto_start=(
                auto_start
                if auto_start is not None
                else os.getenv("UENV_ADAPTER_CORE_AUTO_START", "0") not in {"0", "false", "False"}
            ),
            binary=binary or os.getenv("UENV_ADAPTER_CORE_BINARY"),
            fake_reward=float(fake_reward if fake_reward is not None else os.getenv("UENV_AGENT_LOOP_FAKE_REWARD", "1.0")),
            fake_response_text=fake_response_text
            if fake_response_text is not None
            else os.getenv("UENV_AGENT_LOOP_FAKE_RESPONSE_TEXT", ""),
        )


class StaticRolloutEpisodeClient:
    """Small local stand-in for UEnv Server/Worker during AgentLoop tests."""

    def __init__(self, reward: float = 1.0, response_text: str = "") -> None:
        self.reward = reward
        self.response_text = response_text
        self.last_request: EpisodeRequest | None = None

    def submit_episode(self, request: EpisodeRequest) -> EpisodeResult:
        self.last_request = request
        payload = self._payload(request)
        response_text = self.response_text or self._ground_truth(payload) or "mock external rollout response"
        step = StepRecord(
            step_index=1,
            action=response_text.encode("utf-8"),
            reward=self.reward,
            terminated=True,
            info={
                "source": "static_agent_loop",
                "response_text": response_text,
                "finish_reason": "static_rollout",
            },
        )
        return EpisodeResult(
            request_id=request.request_id,
            status="completed",
            trajectory=Trajectory(steps=[step], total_reward=self.reward, total_steps=1),
            summary=EpisodeSummary(total_reward=self.reward, total_steps=1, terminate_reason="static_rollout"),
        )

    def submit_episode_stream(self, requests):
        for request in requests:
            yield self.submit_episode(request)

    def _payload(self, request: EpisodeRequest) -> dict[str, Any]:
        try:
            value = json.loads(request.payload.decode("utf-8"))
        except Exception:
            return {}
        return value if isinstance(value, dict) else {}

    def _ground_truth(self, payload: dict[str, Any]) -> str:
        rubric = (payload.get("reward_config") or {}).get("rubric_config") or {}
        if isinstance(rubric, dict) and rubric.get("ground_truth") is not None:
            return str(rubric["ground_truth"])
        return ""


def build_agent_loop_episode_client(
    *,
    mode: str | None = None,
    endpoint: str | None = None,
    timeout_seconds: float | None = None,
    startup_timeout_seconds: float | None = None,
    auto_start: bool | None = None,
    binary: str | None = None,
    fake_reward: float | None = None,
    fake_response_text: str | None = None,
) -> EpisodeClient:
    config = AgentLoopClientConfig.from_env(
        mode=mode,
        endpoint=endpoint,
        timeout_seconds=timeout_seconds,
        startup_timeout_seconds=startup_timeout_seconds,
        auto_start=auto_start,
        binary=binary,
        fake_reward=fake_reward,
        fake_response_text=fake_response_text,
    )
    if config.mode == "fake":
        return StaticRolloutEpisodeClient(reward=config.fake_reward, response_text=config.fake_response_text)
    if config.mode == "rust_core":
        return RustCoreEpisodeClient(
            RustCoreClientConfig(
                endpoint=config.endpoint,
                timeout_seconds=config.timeout_seconds,
                startup_timeout_seconds=config.startup_timeout_seconds,
                auto_start=config.auto_start,
                binary=config.binary,
            )
        )
    raise ValueError(f"Unsupported UENV_AGENT_LOOP_CLIENT={config.mode!r}; expected fake or rust_core")
