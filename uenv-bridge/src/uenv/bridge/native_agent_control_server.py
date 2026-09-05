from __future__ import annotations

import json
import socket
import threading
import time
from collections import deque
from concurrent import futures
from dataclasses import dataclass, field
from typing import Any

import grpc

from uenv.v1 import agent_pb2, agent_pb2_grpc, episode_pb2


@dataclass(slots=True)
class NativeAgentJobResult:
    job_id: str
    run_id: str
    status: str
    reward: float
    trajectory_id: str = ""
    error_message: str = ""
    agent_id: str = ""
    parallel_mode: str = ""
    rollout_param_version: int | None = None
    rollout_policy_version: str = ""
    rollout_log_probs: list[float] = field(default_factory=list)
    worker_start_ts: float | None = None
    worker_finish_ts: float | None = None
    result_ready_ts: float | None = None
    worker_latency_ms: int | None = None
    model_latency_ms: int | None = None
    response_ids: list[int] = field(default_factory=list)
    response_mask: list[int] = field(default_factory=list)
    metadata: dict[str, str] = field(default_factory=dict)


class _PendingJob:
    def __init__(self, job: agent_pb2.AgentJob) -> None:
        self.job = job
        self.done = threading.Event()
        self.result: NativeAgentJobResult | None = None


class NativeAgentControlServicer(agent_pb2_grpc.AgentControlServiceServicer):
    def __init__(self, heartbeat_interval_ms: int = 10_000) -> None:
        self._heartbeat_interval_ms = heartbeat_interval_ms
        self._lock = threading.Lock()
        self._registered_agents: dict[str, dict[str, Any]] = {}
        self._queue: deque[str] = deque()
        self._pending: dict[str, _PendingJob] = {}
        self._inflight: dict[str, str] = {}
        self._seq = 0

    def enqueue(self, job: agent_pb2.AgentJob) -> _PendingJob:
        if not job.job_id:
            raise ValueError("AgentJob.job_id is required")
        pending = _PendingJob(job)
        with self._lock:
            if job.job_id in self._pending:
                raise ValueError(f"duplicate AgentJob.job_id: {job.job_id}")
            self._pending[job.job_id] = pending
            self._queue.append(job.job_id)
        return pending

    def wait_result(self, pending: _PendingJob, timeout_seconds: float) -> NativeAgentJobResult:
        if not pending.done.wait(timeout_seconds):
            raise TimeoutError(f"native AgentJob timed out: {pending.job.job_id}")
        if pending.result is None:
            raise RuntimeError(f"native AgentJob completed without result: {pending.job.job_id}")
        return pending.result

    def RegisterAgent(self, request, context):  # noqa: N802, ANN001, ANN201
        agent_id = request.agent_id.strip() if request.agent_id else ""
        with self._lock:
            if not agent_id:
                self._seq += 1
                agent_id = f"native-agent-{self._seq}"
            self._registered_agents[agent_id] = {
                "pool": request.agent_pool_id,
                "max_concurrent": int(request.max_concurrent_jobs or 1),
                "labels": dict(request.labels),
                "registered_at": time.time(),
                "last_heartbeat": time.time(),
            }
        return agent_pb2.RegisterAgentResponse(accepted=True, agent_id=agent_id, message="ok")

    def AgentHeartbeat(self, request, context):  # noqa: N802, ANN001, ANN201
        with self._lock:
            if request.agent_id not in self._registered_agents:
                context.set_code(grpc.StatusCode.NOT_FOUND)
                context.set_details("agent not registered")
                return agent_pb2.AgentHeartbeatResponse(ok=False)
            self._registered_agents[request.agent_id]["active_jobs"] = int(request.active_jobs)
            self._registered_agents[request.agent_id]["last_heartbeat"] = time.time()
        return agent_pb2.AgentHeartbeatResponse(ok=True, next_heartbeat_interval_ms=self._heartbeat_interval_ms)

    def PollAgentJob(self, request, context):  # noqa: N802, ANN001, ANN201
        agent_id = request.worker_id
        with self._lock:
            if agent_id not in self._registered_agents:
                context.set_code(grpc.StatusCode.NOT_FOUND)
                context.set_details("agent not registered")
                return agent_pb2.PollAgentJobResponse(has_job=False)
            while self._queue:
                job_id = self._queue.popleft()
                pending = self._pending.get(job_id)
                if pending is None or pending.result is not None:
                    continue
                self._inflight[job_id] = agent_id
                return agent_pb2.PollAgentJobResponse(has_job=True, job=pending.job)
        return agent_pb2.PollAgentJobResponse(has_job=False)

    def CompleteAgentJob(self, request, context):  # noqa: N802, ANN001, ANN201
        with self._lock:
            pending = self._pending.get(request.job_id)
            if pending is None:
                return agent_pb2.AgentJobCompleteResponse(
                    ack=False,
                    code="UNKNOWN_JOB",
                    message=f"unknown job_id={request.job_id}",
                )
            expected_agent = self._inflight.get(request.job_id)
            if expected_agent and request.agent_id and expected_agent != request.agent_id:
                return agent_pb2.AgentJobCompleteResponse(
                    ack=False,
                    code="AGENT_MISMATCH",
                    message=f"expected agent_id={expected_agent}, got {request.agent_id}",
                )
            pending.result = _result_from_complete_request(request)
            self._inflight.pop(request.job_id, None)
            pending.done.set()
        return agent_pb2.AgentJobCompleteResponse(ack=True, code="OK", message="ok")


class NativeAgentControlServer:
    def __init__(self, host: str = "0.0.0.0", port: int = 19051, max_workers: int = 8) -> None:
        self.host = host
        self.port = int(port)
        self.servicer = NativeAgentControlServicer()
        self._server = grpc.server(futures.ThreadPoolExecutor(max_workers=max_workers))
        agent_pb2_grpc.add_AgentControlServiceServicer_to_server(self.servicer, self._server)
        bound = self._server.add_insecure_port(f"{host}:{port}")
        if bound == 0:
            raise RuntimeError(f"failed to bind native AgentControl server on {host}:{port}")
        self.port = bound
        self.endpoint = f"{_advertise_host(host)}:{bound}"
        self._started = False

    def start(self) -> None:
        if self._started:
            return
        self._server.start()
        self._started = True

    def stop(self, grace_seconds: float = 1.0) -> None:
        if not self._started:
            return
        self._server.stop(grace_seconds)
        self._started = False

    def enqueue(self, job: agent_pb2.AgentJob) -> _PendingJob:
        self.start()
        return self.servicer.enqueue(job)

    def wait_result(self, pending: _PendingJob, timeout_seconds: float) -> NativeAgentJobResult:
        return self.servicer.wait_result(pending, timeout_seconds)


_SERVER_LOCK = threading.Lock()
_SERVER: NativeAgentControlServer | None = None


def get_native_agent_control_server(host: str, port: int) -> NativeAgentControlServer:
    global _SERVER
    with _SERVER_LOCK:
        if _SERVER is None:
            _SERVER = NativeAgentControlServer(host=host, port=port)
            _SERVER.start()
        elif _SERVER.host != host or _SERVER.port != int(port):
            raise RuntimeError(
                f"native AgentControl server already started at {_SERVER.host}:{_SERVER.port}; "
                f"requested {host}:{port}"
            )
        return _SERVER


def build_agent_job_proto(job: dict[str, Any]) -> agent_pb2.AgentJob:
    generation = job.get("generation_config")
    generation_json = json.dumps(generation if isinstance(generation, dict) else {}, ensure_ascii=False).encode("utf-8")
    return agent_pb2.AgentJob(
        job_id=str(job.get("job_id") or ""),
        run_id=str(job.get("run_id") or ""),
        gateway_url=str(job.get("gateway_url") or ""),
        gateway_api_key=str(job.get("gateway_api_key") or ""),
        session_id=str(job.get("session_id") or ""),
        instance_id=str(job.get("instance_id") or ""),
        benchmark_variant=str(job.get("benchmark_variant") or ""),
        env_package_id=str(job.get("env_package_id") or ""),
        env_package_version=str(job.get("env_package_version") or ""),
        agent_bridge_id=str(job.get("agent_bridge_id") or ""),
        agent_bridge_version=str(job.get("agent_bridge_version") or ""),
        driver_entrypoint=str(job.get("driver_entrypoint") or ""),
        model_endpoint_config=episode_pb2.ModelEndpoint(
            endpoint_type=str(job.get("model_endpoint_type") or "http"),
            url=str(job.get("model_endpoint") or ""),
            model_name=str(job.get("model_name") or ""),
            generation_config_json=generation_json,
            max_retries=int(job.get("model_max_retries") or 0),
        ),
        max_iterations=int(job.get("max_iterations") or 0),
        workspace_dir=str(job.get("workspace_dir") or ""),
        episode_id=str(job.get("episode_id") or ""),
        llm_config_path=str(job.get("llm_config_path") or ""),
        mode=str(job.get("mode") or "llm"),
        parallel_mode=str(job.get("parallel_mode") or ""),
        metadata={str(k): str(v) for k, v in (job.get("metadata") or {}).items()},
        task_payload_json=str(job.get("task_payload_json") or ""),
        instances_catalog=str(job.get("instances_catalog") or ""),
        instance_catalog_json=str(job.get("instance_catalog_json") or ""),
    )


def _result_from_complete_request(request) -> NativeAgentJobResult:  # noqa: ANN001
    return NativeAgentJobResult(
        job_id=request.job_id,
        run_id=request.run_id,
        status=request.status,
        reward=float(request.reward),
        trajectory_id=request.trajectory_id,
        error_message=request.error_message,
        agent_id=request.agent_id,
        parallel_mode=request.parallel_mode,
        rollout_param_version=request.rollout_param_version if request.HasField("rollout_param_version") else None,
        rollout_policy_version=request.rollout_policy_version if request.HasField("rollout_policy_version") else "",
        rollout_log_probs=[float(item) for item in request.rollout_log_probs],
        worker_start_ts=request.worker_start_ts if request.HasField("worker_start_ts") else None,
        worker_finish_ts=request.worker_finish_ts if request.HasField("worker_finish_ts") else None,
        result_ready_ts=request.result_ready_ts if request.HasField("result_ready_ts") else None,
        worker_latency_ms=request.worker_latency_ms if request.HasField("worker_latency_ms") else None,
        model_latency_ms=request.model_latency_ms if request.HasField("model_latency_ms") else None,
        response_ids=[int(item) for item in request.rollout_trace.response_ids],
        response_mask=[int(item) for item in request.rollout_trace.response_mask],
        metadata=dict(request.metadata),
    )


def _advertise_host(host: str) -> str:
    if host not in {"", "0.0.0.0", "::"}:
        return host
    try:
        return socket.gethostbyname(socket.gethostname())
    except OSError:
        return "127.0.0.1"
