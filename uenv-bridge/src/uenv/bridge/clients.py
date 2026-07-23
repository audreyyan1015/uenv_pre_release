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
)

_PROTOCOL_METADATA_KEYS = {
    "parallel_mode",
    "timeout_seconds",
    "correlation_id",
    "model_endpoint",
    "env_package_id",
    "env_package_version",
    "rollout_param_version",
    "rollout_policy_version",
    "rollout_log_probs",
    "response_ids",
    "response_mask",
    "enqueue_ts",
    "dispatch_ts",
    "result_ready_ts",
    "worker_start_ts",
    "worker_finish_ts",
    "server_latency_ms",
    "worker_latency_ms",
    "model_latency_ms",
    "dispatch_lease_id",
    "lease_expire_at",
    "scheduler_epoch",
    "dispatch_token",
}


class EpisodeClient(Protocol):
    def submit_episode(self, request: EpisodeRequest) -> EpisodeResult:
        raise NotImplementedError

    def submit_episode_stream(self, requests: Iterable[EpisodeRequest]) -> Iterable[EpisodeResult]:
        raise NotImplementedError


@dataclass(slots=True)
class RustCoreClientConfig:
    endpoint: str = "127.0.0.1:50051"
    timeout_seconds: float = 300.0
    startup_timeout_seconds: float = 30.0
    auto_start: bool = False
    binary: str | None = None
    streaming: bool = False
    transport_retry_attempts: int = 3
    transport_retry_delay_seconds: float = 1.0

    @classmethod
    def from_mapping(cls, data: dict[str, Any]) -> "RustCoreClientConfig":
        core = data.get("core") or {}
        return cls(
            endpoint=str(core.get("endpoint", "127.0.0.1:50051")),
            timeout_seconds=float(core.get("timeout_seconds", 300.0)),
            startup_timeout_seconds=float(core.get("startup_timeout_seconds", 30.0)),
            auto_start=bool(core.get("auto_start", False)),
            binary=str(core["binary"]) if core.get("binary") else None,
            streaming=bool(core.get("streaming", False)),
            transport_retry_attempts=max(1, int(core.get("transport_retry_attempts", 3))),
            transport_retry_delay_seconds=max(0.0, float(core.get("transport_retry_delay_seconds", 1.0))),
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
        request_list = list(requests)
        if self.config.streaming:
            yield from self._submit_episode_streaming(request_list)
            return

        if not hasattr(self.stub, "ExecuteBatch"):
            raise RuntimeError("RustCoreEpisodeClient stub does not provide ExecuteBatch")

        core_request = self._to_core_execute_batch_request(request_list)
        core_response = self._execute_batch_with_retry(core_request)
        for result in self._core_response_results(core_response):
            yield self._from_core_result(result)

    def _submit_episode_streaming(self, requests: list[EpisodeRequest]) -> Iterable[EpisodeResult]:
        if not hasattr(self.stub, "ExecuteBatchStream"):
            raise RuntimeError("RustCoreEpisodeClient streaming mode requires ExecuteBatchStream")
        remaining = list(requests)
        completed: set[str] = set()
        for attempt in range(self.config.transport_retry_attempts):
            core_request = self._to_core_execute_batch_request(remaining)
            samples = list(self._core_request_samples(core_request))
            try:
                core_results = self._execute_batch_stream(samples)
                for result in core_results:
                    request_id = str(self._field(result, "request_id", ""))
                    if request_id in completed:
                        continue
                    completed.add(request_id)
                    yield self._from_core_result(result)
                return
            except Exception as exc:
                remaining = [request for request in remaining if request.request_id not in completed]
                if not remaining or not self._should_retry_transport(exc, attempt):
                    raise
                self._wait_before_retry()

    def _execute_batch_with_retry(self, core_request: Any) -> Any:
        for attempt in range(self.config.transport_retry_attempts):
            try:
                try:
                    return self.stub.ExecuteBatch(core_request, timeout=self.config.timeout_seconds)
                except TypeError:
                    return self.stub.ExecuteBatch(core_request)
            except Exception as exc:
                if not self._should_retry_transport(exc, attempt):
                    raise
                self._wait_before_retry()
        raise AssertionError("transport retry loop exited unexpectedly")

    def _execute_batch_stream(self, samples: list[Any]) -> Iterable[Any]:
        try:
            return self.stub.ExecuteBatchStream(iter(samples), timeout=self.config.timeout_seconds)
        except TypeError:
            return self.stub.ExecuteBatchStream(iter(samples))

    def _should_retry_transport(self, exc: Exception, attempt: int) -> bool:
        if attempt + 1 >= self.config.transport_retry_attempts:
            return False
        code_method = getattr(exc, "code", None)
        if not callable(code_method):
            return False
        try:
            code = code_method()
        except Exception:
            return False
        name = str(getattr(code, "name", code)).upper().rsplit(".", 1)[-1]
        return name in {"UNAVAILABLE", "CANCELLED", "UNKNOWN", "INTERNAL", "DEADLINE_EXCEEDED"}

    def _wait_before_retry(self) -> None:
        if self.config.transport_retry_delay_seconds > 0:
            time.sleep(self.config.transport_retry_delay_seconds)

    def _to_core_execute_batch_request(self, requests: list[EpisodeRequest]) -> dict[str, Any]:
        batch_id = ""
        samples = []
        for idx, request in enumerate(requests):
            payload = self._payload_json(request)
            metadata = self._dict_field(payload, "metadata")
            sample_context = self._sample_context(metadata)
            env_config = self._dict_field(payload, "env_config")
            episode_config = self._dict_field(payload, "episode_config")
            reward_config = self._dict_field(payload, "reward_config")
            model_endpoint = self._dict_field(payload, "model_endpoint")
            generation_config = self._dict_field(model_endpoint, "generation_config")
            batch_id = batch_id or str(metadata.get("batch_id") or "")
            samples.append(
                {
                    "request_id": request.request_id,
                    "batch_id": str(metadata.get("batch_id") or batch_id),
                    "sample_index": int(metadata.get("sample_index", idx)),
                    "framework": str(payload.get("framework") or "verl"),
                    "env_type": request.env_type,
                    "parallel_mode": request.parallel_mode,
                    "env_config_json": self._json_bytes(env_config),
                    "episode_config_json": self._json_bytes(episode_config),
                    "reward_config_json": self._json_bytes(reward_config),
                    "model_endpoint": {
                        "endpoint_type": str(model_endpoint.get("endpoint_type") or "http"),
                        "url": str(request.model_endpoint or model_endpoint.get("url") or ""),
                        "model_name": str(model_endpoint.get("model_name") or ""),
                        "generation_config_json": self._json_bytes(generation_config),
                        "max_retries": int(model_endpoint.get("max_retries") or 0),
                    },
                    "timeout_seconds": int(payload.get("timeout_seconds") or 0),
                    "correlation_id": str(payload.get("correlation_id") or ""),
                    "sample_context_json": self._json_bytes(sample_context),
                    "env_package_id": str(env_config.get("env_package_id") or env_config.get("package_id") or ""),
                    "env_package_version": str(env_config.get("env_package_version") or env_config.get("package_version") or ""),
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
            rollout_param_version=self._optional_int_field(result, "rollout_param_version"),
            rollout_policy_version=self._optional_string_field(result, "rollout_policy_version"),
            rollout_log_probs=[float(v) for v in list(self._field(result, "rollout_log_probs", []) or [])],
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
        rollout_trace = data.get("rollout_trace") or {}
        if not isinstance(rollout_trace, dict):
            rollout_trace = {}
        return StepRecord(
            step_index=int(data.get("step_index", idx) or 0),
            observation=self._bytes_from_jsonable(data.get("observation", b"")),
            action=self._bytes_from_jsonable(data.get("action", b"")),
            reward=float(data.get("reward", 0.0) or 0.0),
            terminated=bool(data.get("terminated", False)),
            truncated=bool(data.get("truncated", False)),
            info={str(key): self._string_from_jsonable(value) for key, value in info.items()},
            duration_ms=int(data.get("duration_ms", 0) or 0),
            response_ids=self._int_list_from_jsonable(rollout_trace.get("response_ids")),
            response_mask=self._int_list_from_jsonable(rollout_trace.get("response_mask")),
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

    def _dict_field(self, value: dict[str, Any], name: str) -> dict[str, Any]:
        item = value.get(name) if isinstance(value, dict) else None
        return item if isinstance(item, dict) else {}

    def _sample_context(self, metadata: dict[str, Any]) -> dict[str, Any]:
        return {
            str(key): value
            for key, value in metadata.items()
            if str(key) not in _PROTOCOL_METADATA_KEYS
        }

    def _int_list_from_jsonable(self, value: Any) -> list[int]:
        if not isinstance(value, list):
            return []
        output: list[int] = []
        for item in value:
            try:
                output.append(int(item))
            except Exception:
                continue
        return output

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

    def _core_request_samples(self, request: Any) -> Iterable[Any]:
        if isinstance(request, dict):
            return request.get("samples") or []
        return getattr(request, "samples", [])

    def _field(self, value: Any, name: str, default: Any) -> Any:
        if isinstance(value, dict):
            return value.get(name, default)
        return getattr(value, name, default)

    def _optional_int_field(self, value: Any, name: str) -> int | None:
        raw = self._field(value, name, None)
        if raw in (None, "", 0):
            return None
        try:
            return int(raw)
        except Exception:
            return None

    def _optional_string_field(self, value: Any, name: str) -> str | None:
        raw = self._field(value, name, None)
        if raw in (None, ""):
            return None
        return str(raw)

    def _json_bytes(self, value: Any) -> bytes:
        return json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
