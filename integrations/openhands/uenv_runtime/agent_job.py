"""Load AgentJob JSON for OpenHands / ToolEnv drivers.

字段分流（SWE vs code/ToolEnv）::

    SWE（OpenHands）
      - 必填：gateway_url 或 session_id、instance_id
      - 常用：benchmark_variant、workspace_dir、mode、max_iterations
      - task_payload_json 留空；不要走 gateway 以外的旁路判分

    code / ToolEnv
      - 必填：task_payload_json（完整 Episode payload JSON）或可由其解析出的 task_id
      - gateway_* / session_id 可空（Server CodeAgentBackend 不建 worker gateway）
      - instance_id 常填 task_id，便于日志对齐

同机双 bridge 版本策略::

    - OpenHands：agent_bridge_id=uenv-agent-openhands，pool=openhands-default
    - ToolEnv：  agent_bridge_id=uenv-agent-toolenv，  pool=toolenv-default
    - 两侧 agent_bridge_version **独立演进**；Server 按 bridge 匹配选池，
      升级一侧时勿顺手改另一侧版本号。

Phase B 也可不经 Server poll，直接用本模块从 JSON 文件加载（见 load_agent_job）。
"""

from __future__ import annotations

import json
import os
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Optional


@dataclass
class AgentJob:
    job_id: str
    run_id: str
    gateway_url: str
    gateway_api_key: Optional[str]
    session_id: Optional[str]
    instance_id: str
    benchmark_variant: str = "pro"
    env_package_id: str = ""
    env_package_version: str = ""
    agent_bridge_id: str = ""
    agent_bridge_version: str = ""
    driver_entrypoint: str = ""
    model_endpoint: str = ""
    max_iterations: int = 30
    workspace_dir: str = "/app"
    episode_id: str = ""
    llm_config_path: str = ""
    mode: str = "llm"
    instances_catalog: str = ""
    # 非 SWE 任务的完整 JSON 载荷（code/ToolEnv 用）；SWE 路径留空。
    task_payload_json: str = ""

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "AgentJob":
        return cls(
            job_id=str(data.get("job_id") or data.get("jobId") or ""),
            run_id=str(data.get("run_id") or data.get("runId") or ""),
            gateway_url=str(data.get("gateway_url") or data.get("gatewayUrl") or ""),
            gateway_api_key=data.get("gateway_api_key") or data.get("gatewayApiKey"),
            session_id=data.get("session_id") or data.get("sessionId"),
            instance_id=str(data.get("instance_id") or data.get("instanceId") or ""),
            benchmark_variant=str(data.get("benchmark_variant") or data.get("benchmarkVariant") or "pro"),
            env_package_id=str(data.get("env_package_id") or data.get("envPackageId") or ""),
            env_package_version=str(data.get("env_package_version") or data.get("envPackageVersion") or ""),
            agent_bridge_id=str(data.get("agent_bridge_id") or data.get("agentBridgeId") or ""),
            agent_bridge_version=str(data.get("agent_bridge_version") or data.get("agentBridgeVersion") or ""),
            driver_entrypoint=str(data.get("driver_entrypoint") or data.get("driverEntrypoint") or ""),
            model_endpoint=str(data.get("model_endpoint") or data.get("modelEndpoint") or ""),
            max_iterations=int(data.get("max_iterations") or data.get("maxIterations") or 30),
            workspace_dir=str(data.get("workspace_dir") or data.get("workspaceDir") or "/app"),
            episode_id=str(data.get("episode_id") or data.get("episodeId") or ""),
            llm_config_path=str(data.get("llm_config_path") or data.get("llmConfigPath") or ""),
            mode=str(data.get("mode") or "llm"),
            instances_catalog=str(data.get("instances_catalog") or data.get("instancesCatalog") or ""),
            task_payload_json=str(data.get("task_payload_json") or data.get("taskPayloadJson") or ""),
        )


def load_agent_job(path: Optional[str | Path] = None) -> Optional[AgentJob]:
    """Load from UENV_AGENT_JOB_FILE or explicit path."""
    raw_path = path or os.environ.get("UENV_AGENT_JOB_FILE", "")
    if not raw_path:
        return None
    p = Path(str(raw_path))
    if not p.is_file():
        raise FileNotFoundError(f"AgentJob file not found: {p}")
    data = json.loads(p.read_text(encoding="utf-8"))
    job = AgentJob.from_dict(data)
    if not job.instance_id and not job.task_payload_json:
        raise ValueError("AgentJob missing instance_id (and no task_payload_json)")
    # SWE 路径需要 gateway；code/ToolEnv 路径只带 task_payload_json，不强制 gateway。
    if not job.task_payload_json and not job.gateway_url and not job.session_id:
        raise ValueError("AgentJob requires gateway_url or pre-created session_id")
    return job


def write_agent_job_template(path: Path, **overrides: Any) -> AgentJob:
    """Write a sample AgentJob JSON for local / for-episode testing."""
    job = AgentJob(
        job_id=overrides.get("job_id", "job-local-1"),
        run_id=overrides.get("run_id", "run-local-1"),
        gateway_url=overrides.get("gateway_url", "http://127.0.0.1:28097"),
        gateway_api_key=overrides.get("gateway_api_key", "swe-pro-secret"),
        session_id=overrides.get("session_id"),
        instance_id=overrides["instance_id"],
        benchmark_variant=overrides.get("benchmark_variant", "pro"),
        mode=overrides.get("mode", "gold"),
        max_iterations=int(overrides.get("max_iterations", 30)),
        llm_config_path=str(overrides.get("llm_config_path", "")),
        instances_catalog=str(overrides.get("instances_catalog", "")),
    )
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(job.__dict__, indent=2) + "\n", encoding="utf-8")
    return job
