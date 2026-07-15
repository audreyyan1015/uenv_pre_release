"""Agent 池控制面 gRPC 客户端（208.77 OpenHands runner 连 Server 用）。

封装 uenv.v1.AgentControlService 的四个 RPC：
  - RegisterAgent   ：启动时注册到 Server Agent 池，上报 pool/bridge/并发上限
  - PollAgentJob    ：循环领取 Server 下派的 AgentJob
  - CompleteAgentJob：回填 reward / trajectory_id
  - AgentHeartbeat  ：（可选）周期上报存活与 active_jobs

设计：
  - grpc / 生成 stub 用 importlib 懒加载（参考 uenv-bridge/.../clients.py），
    未安装 grpcio 或未生成 stub 时给清晰报错，不影响 runner 的 HTTP 旁路启动。
  - PollAgentJob 返回复用 agent_job.AgentJob（统一 driver 消费的数据结构）。
"""

from __future__ import annotations

import importlib
import sys
from typing import Any, Optional

from .agent_job import AgentJob


class AgentControlUnavailable(RuntimeError):
    """grpcio 未安装或 agent stub 未生成时抛出。"""


def _load_grpc_modules() -> tuple[Any, Any, Any]:
    """懒加载 grpc + 生成的 agent stub。失败抛 AgentControlUnavailable。"""
    try:
        grpc = importlib.import_module("grpc")
    except Exception as exc:  # noqa: BLE001
        raise AgentControlUnavailable(
            "grpcio not installed; run `pip install -r "
            "integrations/openhands/requirements-agent.txt`"
        ) from exc

    # 生成的 stub 位于 gen/uenv/v1/（包名 uenv.v1）；旧文档曾写扁平 gen/agent_pb2。
    pb2 = None
    pb2_grpc = None
    for pkg in (
        "uenv.v1",
        "uenv_runtime.gen.uenv.v1",
        "uenv_runtime.gen",
        "gen",
        "",
    ):
        prefix = f"{pkg}." if pkg else ""
        try:
            pb2 = importlib.import_module(f"{prefix}agent_pb2")
            sys.modules.setdefault("agent_pb2", pb2)
            pb2_grpc = importlib.import_module(f"{prefix}agent_pb2_grpc")
            break
        except Exception:  # noqa: BLE001
            continue
    if pb2 is None or pb2_grpc is None:
        raise AgentControlUnavailable(
            "agent gRPC stub not found; run `make proto-agent-python` to generate "
            "integrations/openhands/uenv_runtime/gen/agent_pb2*.py"
        )
    return grpc, pb2, pb2_grpc


class AgentControlClient:
    """连 Server AgentControlService 的轻量客户端（同步 gRPC）。"""

    def __init__(self, server_endpoint: str, timeout_sec: float = 10.0) -> None:
        self.endpoint = server_endpoint
        self.timeout_sec = timeout_sec
        self._grpc, self._pb2, pb2_grpc = _load_grpc_modules()
        self._channel = self._grpc.insecure_channel(server_endpoint)
        self._stub = pb2_grpc.AgentControlServiceStub(self._channel)

    def close(self) -> None:
        ch = getattr(self, "_channel", None)
        if ch is not None:
            ch.close()
            self._channel = None

    def __enter__(self) -> "AgentControlClient":
        return self

    def __exit__(self, *_exc: object) -> None:
        self.close()

    # ── RPCs ────────────────────────────────────────────────────────────────
    def register_agent(
        self,
        agent_id: str,
        agent_pool_id: str,
        synced_bridges: list[dict[str, str]],
        max_concurrent: int,
        endpoint: str = "",
        labels: dict[str, str] | None = None,
    ) -> str:
        """注册到 Server Agent 池，返回 Server 确认的 agent_id。

        labels：路由标签（如 {"region": "bj"}），供 Server 多池标签亲和选池用。
        """
        bridges = [
            self._pb2.SyncedAgentBridge(
                package_id=b.get("package_id", ""),
                version=b.get("version", ""),
                bundle_digest=b.get("bundle_digest", ""),
            )
            for b in synced_bridges
        ]
        req = self._pb2.RegisterAgentRequest(
            agent_id=agent_id,
            agent_pool_id=agent_pool_id,
            synced_agent_bridges=bridges,
            max_concurrent_jobs=int(max_concurrent),
            endpoint=endpoint,
            labels=labels or {},
        )
        resp = self._stub.RegisterAgent(req, timeout=self.timeout_sec)
        if not resp.accepted:
            raise RuntimeError(f"RegisterAgent rejected: {resp.message}")
        return resp.agent_id

    def poll_agent_job(self, agent_pool_id: str, agent_id: str) -> Optional[AgentJob]:
        """领取一个 AgentJob；无任务返回 None。

        注：proto PollAgentJobRequest 的 worker_id 字段被 Server 复用为 agent_id
        （见 uenv-server/src/agent_job.rs try_reserve 的 preferred_agent_id）。
        """
        req = self._pb2.PollAgentJobRequest(
            agent_pool_id=agent_pool_id,
            worker_id=agent_id,
        )
        resp = self._stub.PollAgentJob(req, timeout=self.timeout_sec)
        if not resp.has_job:
            return None
        return _job_from_proto(resp.job)

    def complete_agent_job(
        self,
        job_id: str,
        run_id: str,
        status: str,
        reward: float,
        trajectory_id: str = "",
        error_message: str = "",
        parallel_mode: str = "",
        rollout_param_version: int | None = None,
        rollout_policy_version: str | None = None,
        rollout_log_probs: list[float] | None = None,
        worker_start_ts: float | None = None,
        worker_finish_ts: float | None = None,
        result_ready_ts: float | None = None,
        worker_latency_ms: int | None = None,
        model_latency_ms: int | None = None,
        response_ids: list[int] | None = None,
        response_mask: list[int] | None = None,
    ) -> bool:
        """回填结果；返回 Server 是否 ack。"""
        ids = [int(item) for item in (response_ids or [])]
        mask = [int(item) for item in (response_mask or [])]
        log_probs = [float(item) for item in (rollout_log_probs or [])]
        req_kwargs = {
            "job_id": job_id,
            "run_id": run_id,
            "status": status,
            "reward": float(reward),
            "trajectory_id": trajectory_id,
            "error_message": error_message,
            "parallel_mode": parallel_mode,
            "rollout_log_probs": log_probs,
        }
        if rollout_param_version is not None:
            req_kwargs["rollout_param_version"] = int(rollout_param_version)
        if rollout_policy_version is not None:
            req_kwargs["rollout_policy_version"] = str(rollout_policy_version)
        if worker_start_ts is not None:
            req_kwargs["worker_start_ts"] = float(worker_start_ts)
        if worker_finish_ts is not None:
            req_kwargs["worker_finish_ts"] = float(worker_finish_ts)
        if result_ready_ts is not None:
            req_kwargs["result_ready_ts"] = float(result_ready_ts)
        if worker_latency_ms is not None:
            req_kwargs["worker_latency_ms"] = int(worker_latency_ms)
        if model_latency_ms is not None:
            req_kwargs["model_latency_ms"] = int(model_latency_ms)
        req = self._pb2.AgentJobCompleteRequest(**req_kwargs)
        if ids or mask:
            if ids and not mask:
                mask = [1] * len(ids)
            req.rollout_trace.response_ids.extend(ids)
            req.rollout_trace.response_mask.extend(mask)
        resp = self._stub.CompleteAgentJob(req, timeout=self.timeout_sec)
        return bool(resp.ack)

    def agent_heartbeat(self, agent_id: str, active_jobs: int, timestamp_ms: int = 0) -> int:
        """上报心跳，返回 Server 建议的下次间隔（ms）。"""
        req = self._pb2.AgentHeartbeatRequest(
            agent_id=agent_id,
            active_jobs=int(active_jobs),
            timestamp_ms=int(timestamp_ms),
        )
        resp = self._stub.AgentHeartbeat(req, timeout=self.timeout_sec)
        return int(resp.next_heartbeat_interval_ms)


def _job_from_proto(job: Any) -> AgentJob:
    """proto AgentJob → agent_job.AgentJob（driver 消费的 dataclass）。"""
    return AgentJob(
        job_id=job.job_id,
        run_id=job.run_id,
        gateway_url=job.gateway_url,
        gateway_api_key=job.gateway_api_key or None,
        session_id=job.session_id or None,
        instance_id=job.instance_id,
        benchmark_variant=job.benchmark_variant or "pro",
        env_package_id=job.env_package_id,
        env_package_version=job.env_package_version,
        agent_bridge_id=job.agent_bridge_id,
        agent_bridge_version=job.agent_bridge_version,
        driver_entrypoint=job.driver_entrypoint,
        model_endpoint=job.model_endpoint,
        max_iterations=int(job.max_iterations) or 30,
        workspace_dir=job.workspace_dir or "/app",
        episode_id=job.episode_id,
        llm_config_path=job.llm_config_path,
        mode=job.mode or "llm",
    )
