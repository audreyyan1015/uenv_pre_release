from __future__ import annotations

import importlib
import json
import os
import subprocess
import sys
import time
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


@dataclass(slots=True)
class RustCoreClientConfig:
    endpoint: str = "127.0.0.1:50051"
    timeout_seconds: float = 300.0
    startup_timeout_seconds: float = 30.0
    auto_start: bool = False
    binary: str | None = None

    @classmethod
    def from_mapping(cls, data: dict[str, Any]) -> "RustCoreClientConfig":
        core = data.get("core") or {}
        return cls(
            endpoint=str(core.get("endpoint", "127.0.0.1:50051")),
            timeout_seconds=float(core.get("timeout_seconds", 300.0)),
            startup_timeout_seconds=float(core.get("startup_timeout_seconds", 30.0)),
            auto_start=bool(core.get("auto_start", False)),
            binary=str(core["binary"]) if core.get("binary") else None,
        )


class RustCoreEpisodeClient:
    """Client boundary for Python shim -> Rust adapter core.

    The Rust core API is intentionally local to the adapter. It receives the
    already-normalized EpisodeRequest payload from Python, then calls UEnv
    Server through Rust functions rather than a second gRPC hop.
    """

    def __init__(self, config: RustCoreClientConfig, stub: object | None = None) -> None:
        self.config = config
        self._core_pb2: object | None = None
        self._channel: object | None = None
        self._process: subprocess.Popen[str] | None = None
        if stub is not None:
            self.stub = stub
            return

        try:
            if config.auto_start:
                self._process = self._start_local_core()
            self.stub = self._build_generated_stub()
            if config.auto_start:
                self._wait_for_health()
        except Exception:
            self.close()
            raise

    def close(self) -> None:
        if self._channel is not None and hasattr(self._channel, "close"):
            self._channel.close()
            self._channel = None

        if self._process is None:
            return
        if self._process.poll() is None:
            self._process.terminate()
            try:
                self._process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self._process.kill()
                self._process.wait(timeout=5)
        if self._process.stdout is not None:
            self._process.stdout.close()
        self._process = None

    def __enter__(self) -> "RustCoreEpisodeClient":
        return self

    def __exit__(self, _exc_type: object, _exc: object, _traceback: object) -> None:
        self.close()

    def __del__(self) -> None:
        try:
            self.close()
        except Exception:
            pass

    def _build_generated_stub(self) -> object | None:
        try:
            grpc = importlib.import_module("grpc")
            pb2 = importlib.import_module("uenv.bridge.gen.adapter_core_pb2")
            sys.modules.setdefault("adapter_core_pb2", pb2)
            pb2_grpc = importlib.import_module("uenv.bridge.gen.adapter_core_pb2_grpc")
        except Exception:
            return None
        self._core_pb2 = pb2
        channel = grpc.insecure_channel(self.config.endpoint)
        self._channel = channel
        return pb2_grpc.AdapterCoreServiceStub(channel)

    def _start_local_core(self) -> subprocess.Popen[str]:
        command = [self._resolve_binary()]
        env = os.environ.copy()
        env["UENV_ADDR"] = self.config.endpoint
        return subprocess.Popen(
            command,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )

    def _resolve_binary(self) -> str:
        if not self.config.binary:
            return "uenv-adapter-core"

        binary = Path(self.config.binary).expanduser()
        if binary.is_absolute():
            return str(binary)

        cwd_binary = Path.cwd() / binary
        if cwd_binary.exists():
            return str(cwd_binary)

        project_binary = Path(__file__).resolve().parents[3] / binary
        return str(project_binary)

    def _wait_for_health(self) -> None:
        if self.stub is None or self._core_pb2 is None:
            raise RuntimeError("RustCoreEpisodeClient auto_start requires generated adapter_core protobuf modules")
        if not hasattr(self.stub, "HealthCheck"):
            raise RuntimeError("RustCoreEpisodeClient stub does not provide HealthCheck")

        deadline = time.time() + self.config.startup_timeout_seconds
        last_error: Exception | None = None
        while time.time() < deadline:
            if self._process is not None and self._process.poll() is not None:
                output = self._process.stdout.read() if self._process.stdout is not None else ""
                raise RuntimeError(f"Rust adapter core exited during startup with code {self._process.returncode}: {output}")
            try:
                response = self.stub.HealthCheck(self._core_pb2.HealthCheckRequest(), timeout=1)
                if bool(getattr(response, "ok", False)):
                    return
            except Exception as exc:
                last_error = exc
                time.sleep(0.2)
        raise RuntimeError(f"Rust adapter core did not become healthy at {self.config.endpoint}: {last_error}")

    def submit_episode(self, request: EpisodeRequest) -> EpisodeResult:
        return next(self.submit_episode_stream([request]))

    def submit_episode_stream(self, requests: Iterable[EpisodeRequest]) -> Iterable[EpisodeResult]:
        if self.stub is None:
            raise RuntimeError("RustCoreEpisodeClient requires an AdapterCoreService stub before submitting episodes")
        if not hasattr(self.stub, "ExecuteBatch"):
            raise RuntimeError("RustCoreEpisodeClient stub does not provide ExecuteBatch")

        request_list = list(requests)
        core_request = self._to_core_execute_batch_request(request_list)
        try:
            core_response = self.stub.ExecuteBatch(core_request, timeout=self.config.timeout_seconds)
        except TypeError:
            core_response = self.stub.ExecuteBatch(core_request)
        for result in self._core_response_results(core_response):
            yield self._from_core_result(result)

    def _to_core_execute_batch_request(self, requests: list[EpisodeRequest]) -> dict[str, Any]:
        batch_id = ""
        samples = []
        for idx, request in enumerate(requests):
            payload = self._payload_json(request)
            metadata = payload.get("metadata") or {}
            batch_id = batch_id or str(metadata.get("batch_id") or "")
            samples.append(
                {
                    "request_id": request.request_id,
                    "batch_id": str(metadata.get("batch_id") or batch_id),
                    "sample_index": int(metadata.get("sample_index", idx)),
                    "framework": str(payload.get("framework") or "verl"),
                    "env_type": request.env_type,
                    "payload_json": request.payload,
                    "meta_json": json.dumps(metadata, ensure_ascii=False, separators=(",", ":")).encode("utf-8"),
                }
            )
        core_request = {
            "request_id": f"core-batch-{batch_id or 'unknown'}",
            "batch_id": batch_id,
            "samples": samples,
        }
        if self._core_pb2 is None:
            return core_request
        return self._core_pb2.ExecuteBatchRequest(
            request_id=core_request["request_id"],
            batch_id=core_request["batch_id"],
            samples=[self._core_pb2.SampleEnvelope(**sample) for sample in samples],
        )

    def _from_core_result(self, result: Any) -> EpisodeResult:
        request_id = str(self._field(result, "request_id", ""))
        status = str(self._field(result, "status", "failed"))
        reward = float(self._field(result, "reward", 0.0) or 0.0)
        done = bool(self._field(result, "done", status == "completed"))
        termination_reason = str(self._field(result, "termination_reason", status))
        error_code = self._field(result, "error_code", None)
        error_message = str(self._field(result, "error_message", ""))
        trajectory = self._decode_core_trajectory(result, reward=reward, done=done, termination_reason=termination_reason)
        return EpisodeResult(
            request_id=request_id,
            status=status,
            trajectory=trajectory,
            summary=EpisodeSummary(
                total_reward=reward,
                total_steps=trajectory.total_steps,
                terminate_reason=termination_reason,
            ),
            error_code=int(error_code) if str(error_code or "").isdigit() else None,
            error_message=error_message,
        )

    def _decode_core_trajectory(
        self,
        result: Any,
        *,
        reward: float,
        done: bool,
        termination_reason: str,
    ) -> Trajectory:
        raw = self._field(result, "trajectory_json", b"")
        if raw:
            try:
                data = json.loads(raw.decode("utf-8") if isinstance(raw, bytes) else str(raw))
                trajectory = self._trajectory_from_jsonable(data, reward=reward)
                if trajectory.steps:
                    return trajectory
            except Exception:
                pass

        step = StepRecord(
            step_index=0,
            reward=reward,
            terminated=done,
            info={"source": "rust_core", "termination_reason": termination_reason},
        )
        return Trajectory(steps=[step], total_reward=reward, total_steps=1)

    def _trajectory_from_jsonable(self, data: Any, *, reward: float) -> Trajectory:
        if isinstance(data, list):
            steps_data = data
            total_reward = reward
            total_steps = len(steps_data)
        elif isinstance(data, dict):
            steps_data = data.get("steps") or []
            total_reward = float(data.get("total_reward", reward) or 0.0)
            total_steps = int(data.get("total_steps", len(steps_data)) or 0)
        else:
            return Trajectory(total_reward=reward)

        steps = [self._step_from_jsonable(idx, item) for idx, item in enumerate(steps_data) if isinstance(item, dict)]
        return Trajectory(steps=steps, total_reward=total_reward, total_steps=total_steps or len(steps))

    def _step_from_jsonable(self, idx: int, data: dict[str, Any]) -> StepRecord:
        info = data.get("info") or {}
        if not isinstance(info, dict):
            info = {}
        return StepRecord(
            step_index=int(data.get("step_index", idx) or 0),
            observation=self._bytes_from_jsonable(data.get("observation", b"")),
            action=self._bytes_from_jsonable(data.get("action", b"")),
            reward=float(data.get("reward", 0.0) or 0.0),
            terminated=bool(data.get("terminated", False)),
            truncated=bool(data.get("truncated", False)),
            info={str(key): self._string_from_jsonable(value) for key, value in info.items()},
            duration_ms=int(data.get("duration_ms", 0) or 0),
        )

    def _bytes_from_jsonable(self, value: Any) -> bytes:
        if isinstance(value, bytes):
            return value
        if isinstance(value, str):
            return value.encode("utf-8")
        return json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode("utf-8")

    def _string_from_jsonable(self, value: Any) -> str:
        if isinstance(value, str):
            return value
        return json.dumps(value, ensure_ascii=False, separators=(",", ":"))

    def _payload_json(self, request: EpisodeRequest) -> dict[str, Any]:
        try:
            payload = json.loads(request.payload.decode("utf-8"))
        except Exception:
            return {}
        return payload if isinstance(payload, dict) else {}

    def _core_response_results(self, response: Any) -> Iterable[Any]:
        if isinstance(response, dict):
            return response.get("results") or []
        return getattr(response, "results", [])

    def _field(self, value: Any, name: str, default: Any) -> Any:
        if isinstance(value, dict):
            return value.get(name, default)
        return getattr(value, name, default)


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
