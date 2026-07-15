from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any


MODE_SINGLE = 1
MODE_MULTI = 2
MODE_MODEL_CALLBACK = 3
MODE_CUSTOM = 4


@dataclass(slots=True)
class ResourceSpec:
    cpu_cores: int = 0
    memory_mb: int = 0
    gpu_count: int = 0
    gpu_type: str = ""


@dataclass(slots=True)
class EpisodeRequest:
    request_id: str
    env_type: str
    payload: bytes
    mode: int = MODE_MULTI
    max_steps: int = 0
    resource_spec: ResourceSpec = field(default_factory=ResourceSpec)
    model_endpoint: str = ""
    seed: int | None = None
    parallel_mode: str = "sync"


@dataclass(slots=True)
class StepRecord:
    step_index: int
    observation: bytes = b""
    action: bytes = b""
    reward: float = 0.0
    terminated: bool = False
    truncated: bool = False
    info: dict[str, str] = field(default_factory=dict)
    duration_ms: int = 0
    response_ids: list[int] = field(default_factory=list)
    response_mask: list[int] = field(default_factory=list)


@dataclass(slots=True)
class Trajectory:
    steps: list[StepRecord] = field(default_factory=list)
    total_reward: float = 0.0
    total_steps: int = 0


@dataclass(slots=True)
class EpisodeSummary:
    total_reward: float = 0.0
    total_steps: int = 0
    total_duration_ms: int = 0
    terminate_reason: str = ""


@dataclass(slots=True)
class EpisodeResult:
    request_id: str
    status: str
    trajectory: Trajectory = field(default_factory=Trajectory)
    summary: EpisodeSummary = field(default_factory=EpisodeSummary)
    error_code: int | None = None
    error_message: str = ""
    rollout_param_version: int | None = None
    rollout_policy_version: str | None = None
    rollout_log_probs: list[float] = field(default_factory=list)


def request_to_jsonable(request: EpisodeRequest) -> dict[str, Any]:
    return {
        "request_id": request.request_id,
        "env_type": request.env_type,
        "payload": request.payload.decode("utf-8", errors="replace"),
        "mode": request.mode,
        "max_steps": request.max_steps,
        "resource_spec": {
            "cpu_cores": request.resource_spec.cpu_cores,
            "memory_mb": request.resource_spec.memory_mb,
            "gpu_count": request.resource_spec.gpu_count,
            "gpu_type": request.resource_spec.gpu_type,
        },
        "model_endpoint": request.model_endpoint,
        "seed": request.seed,
        "parallel_mode": request.parallel_mode,
    }
