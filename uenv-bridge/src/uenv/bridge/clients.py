from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable, Protocol

from .protocol import (
    EpisodeRequest,
    EpisodeResult,
    EpisodeSummary,
    StepRecord,
    Trajectory,
    request_to_jsonable,
)


class EpisodeClient(Protocol):
    def submit_episode(self, request: EpisodeRequest) -> EpisodeResult:
        raise NotImplementedError

    def submit_episode_stream(self, requests: Iterable[EpisodeRequest]) -> Iterable[EpisodeResult]:
        raise NotImplementedError


@dataclass(slots=True)
class GrpcEpisodeClientConfig:
    endpoint: str
    timeout_seconds: float = 300.0
    max_send_message_mb: int = 64
    max_receive_message_mb: int = 64
    compression: str | None = None
    tls_enabled: bool = False

    @classmethod
    def from_mapping(cls, data: dict[str, Any]) -> "GrpcEpisodeClientConfig":
        server = data.get("server") or {}
        grpc = server.get("grpc") or {}
        tls = server.get("tls") or {}
        endpoint = server.get("endpoint")
        if not endpoint:
            raise ValueError("server.endpoint is required for GrpcEpisodeClientConfig")
        return cls(
            endpoint=str(endpoint),
            timeout_seconds=float(grpc.get("timeout_seconds", 300.0)),
            max_send_message_mb=int(grpc.get("max_send_message_mb", 64)),
            max_receive_message_mb=int(grpc.get("max_receive_message_mb", 64)),
            compression=grpc.get("compression"),
            tls_enabled=bool(tls.get("enabled", False)),
        )


class GrpcEpisodeClient:
    """gRPC client boundary for the future UEnv Serve API.

    Serve protobuf modules are not available in this repo yet. Callers can pass
    a generated stub later; until then this class is instantiable but fails
    explicitly when used.
    """

    def __init__(self, config: GrpcEpisodeClientConfig, stub: object | None = None) -> None:
        self.config = config
        self.stub = stub

    def submit_episode(self, request: EpisodeRequest) -> EpisodeResult:
        if self.stub is None:
            raise RuntimeError("GrpcEpisodeClient requires a generated UEnvService stub before submitting episodes")
        if not hasattr(self.stub, "SubmitEpisode"):
            raise RuntimeError("GrpcEpisodeClient stub does not provide SubmitEpisode")
        return self._from_proto_result(self.stub.SubmitEpisode(self._to_proto_request(request)))

    def submit_episode_stream(self, requests: Iterable[EpisodeRequest]) -> Iterable[EpisodeResult]:
        if self.stub is None:
            raise RuntimeError("GrpcEpisodeClient requires a generated UEnvService stub before submitting episodes")
        if not hasattr(self.stub, "SubmitEpisodeStream"):
            raise RuntimeError("GrpcEpisodeClient stub does not provide SubmitEpisodeStream")
        proto_requests = (self._to_proto_request(request) for request in requests)
        for proto_result in self.stub.SubmitEpisodeStream(proto_requests):
            yield self._from_proto_result(proto_result)

    def _to_proto_request(self, request: EpisodeRequest) -> object:
        raise NotImplementedError("EpisodeRequest protobuf conversion will be added when Serve proto is available")

    def _from_proto_result(self, result: object) -> EpisodeResult:
        raise NotImplementedError("EpisodeResult protobuf conversion will be added when Serve proto is available")


class FakeEpisodeClient:
    def __init__(self, reward: float = 1.0, fail_request_ids: set[str] | None = None, math_reward: bool = False) -> None:
        self.reward = reward
        self.fail_request_ids = fail_request_ids or set()
        self.math_reward = math_reward

    def submit_episode(self, request: EpisodeRequest) -> EpisodeResult:
        if request.request_id in self.fail_request_ids:
            return EpisodeResult(
                request_id=request.request_id,
                status="failed",
                summary=EpisodeSummary(terminate_reason="fake_error"),
                error_code=5001,
                error_message="fake episode failure",
            )

        reward = self._reward_for_request(request)
        step = StepRecord(
            step_index=0,
            reward=reward,
            terminated=True,
            info={"source": "fake"},
        )
        return EpisodeResult(
            request_id=request.request_id,
            status="completed",
            trajectory=Trajectory(steps=[step], total_reward=reward, total_steps=1),
            summary=EpisodeSummary(total_reward=reward, total_steps=1, terminate_reason="done"),
        )

    def submit_episode_stream(self, requests: Iterable[EpisodeRequest]) -> Iterable[EpisodeResult]:
        for request in requests:
            yield self.submit_episode(request)

    def _reward_for_request(self, request: EpisodeRequest) -> float:
        if not self.math_reward:
            return self.reward
        try:
            payload = json.loads(request.payload.decode("utf-8"))
        except Exception:
            return self.reward
        reward_config = payload.get("reward_config") or {}
        rubric = reward_config.get("rubric_config") or {}
        ground_truth = str(rubric.get("ground_truth") or "")
        prompt = str((payload.get("env_config") or {}).get("raw_prompt") or "")
        if ground_truth and ground_truth in prompt:
            return 1.0
        return self.reward


class DryRunEpisodeClient:
    def __init__(self, output_dir: str | Path) -> None:
        self.output_dir = Path(output_dir)
        self.output_dir.mkdir(parents=True, exist_ok=True)
        self.requests: list[EpisodeRequest] = []

    def submit_episode(self, request: EpisodeRequest) -> EpisodeResult:
        self.requests.append(request)
        self._write_requests()
        return EpisodeResult(
            request_id=request.request_id,
            status="recorded",
            summary=EpisodeSummary(terminate_reason="dry_run"),
        )

    def submit_episode_stream(self, requests: Iterable[EpisodeRequest]) -> Iterable[EpisodeResult]:
        for request in requests:
            yield self.submit_episode(request)

    def _write_requests(self) -> None:
        payload = [request_to_jsonable(request) for request in self.requests]
        output = self.output_dir / "episode_requests.json"
        output.write_text(json.dumps(payload, ensure_ascii=False, indent=2), encoding="utf-8")
