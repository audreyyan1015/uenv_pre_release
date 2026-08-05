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
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Optional


def decode_generation_config(value: Any) -> dict[str, Any]:
    """Decode ``ModelEndpoint.generation_config_json`` into a JSON object.

    Agent jobs written by the poller already contain a dictionary, while the
    protobuf transport carries UTF-8 JSON bytes.  Keep both representations
    accepted so the on-disk AgentJob remains a lossless hand-off format.
    """

    if value in (None, "", b""):
        return {}
    if isinstance(value, dict):
        return dict(value)
    if isinstance(value, bytes):
        try:
            value = value.decode("utf-8")
        except UnicodeDecodeError as exc:
            raise ValueError("generation_config_json is not valid UTF-8") from exc
    if not isinstance(value, str):
        raise ValueError("generation_config must be a JSON object or UTF-8 JSON")
    try:
        decoded = json.loads(value)
    except json.JSONDecodeError as exc:
        raise ValueError("generation_config_json is not valid JSON") from exc
    if not isinstance(decoded, dict):
        raise ValueError("generation_config_json must decode to a JSON object")
    return decoded


def normalize_benchmark_variant(value: str | None) -> str:
    raw = (value or "").strip().lower()
    if raw in {
        "smith",
        "swe-smith",
        "swe_smith",
        "swe-bench-smith",
        "swe_bench_smith",
        "swesmith",
    }:
        return "smith"
    if raw in {"pro", "swe-bench-pro", "swe_bench_pro", "swe-bench_pro"}:
        return "pro"
    if raw in {"lite", "swe-bench-lite", "swe_bench_lite"}:
        return "lite"
    if raw in {"verified", "swe-bench-verified", "swe_bench_verified", ""}:
        return "verified"
    return raw


def default_workspace_dir(variant: str | None) -> str:
    """Pro 镜像仓库根为 /app；Verified/Lite/Smith 为 /testbed。"""
    return "/app" if normalize_benchmark_variant(variant) == "pro" else "/testbed"


def resolve_workspace_dir(variant: str | None, workspace_dir: str | None = None) -> str:
    explicit = (workspace_dir or "").strip()
    if explicit:
        return explicit
    return default_workspace_dir(variant)


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
    model_endpoint_type: str = ""
    model_endpoint: str = ""
    model_name: str = ""
    generation_config: dict[str, Any] = field(default_factory=dict)
    model_max_retries: int = 0
    max_iterations: int = 30
    workspace_dir: str = "/app"
    episode_id: str = ""
    llm_config_path: str = ""
    mode: str = "llm"
    instances_catalog: str = ""
    # SWE：Server/Worker 注入的单样本 catalog JSON（`{instance_id: row}`）。
    # driver 优先写盘并加载，避免依赖 Agent 主机本地 fixture / 全量 EnvPackage。
    instance_catalog_json: str = ""
    # 非 SWE 任务的完整 JSON 载荷（code/ToolEnv 用）；SWE 路径留空。
    task_payload_json: str = ""

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "AgentJob":
        endpoint_config = data.get("model_endpoint_config") or data.get(
            "modelEndpointConfig"
        )
        if not isinstance(endpoint_config, dict):
            endpoint_config = {}
        generation_config = data.get("generation_config")
        if generation_config is None:
            generation_config = data.get("generationConfig")
        if generation_config is None:
            generation_config = endpoint_config.get("generation_config")
        if generation_config is None:
            generation_config = endpoint_config.get("generationConfig")
        if generation_config is None:
            generation_config = endpoint_config.get("generation_config_json")
        if generation_config is None:
            generation_config = endpoint_config.get("generationConfigJson")

        variant = normalize_benchmark_variant(
            str(data.get("benchmark_variant") or data.get("benchmarkVariant") or "pro")
        )
        workspace = resolve_workspace_dir(
            variant,
            str(data.get("workspace_dir") or data.get("workspaceDir") or ""),
        )
        return cls(
            job_id=str(data.get("job_id") or data.get("jobId") or ""),
            run_id=str(data.get("run_id") or data.get("runId") or ""),
            gateway_url=str(data.get("gateway_url") or data.get("gatewayUrl") or ""),
            gateway_api_key=data.get("gateway_api_key") or data.get("gatewayApiKey"),
            session_id=data.get("session_id") or data.get("sessionId"),
            instance_id=str(data.get("instance_id") or data.get("instanceId") or ""),
            benchmark_variant=variant,
            env_package_id=str(data.get("env_package_id") or data.get("envPackageId") or ""),
            env_package_version=str(data.get("env_package_version") or data.get("envPackageVersion") or ""),
            agent_bridge_id=str(data.get("agent_bridge_id") or data.get("agentBridgeId") or ""),
            agent_bridge_version=str(data.get("agent_bridge_version") or data.get("agentBridgeVersion") or ""),
            driver_entrypoint=str(data.get("driver_entrypoint") or data.get("driverEntrypoint") or ""),
            model_endpoint_type=str(
                data.get("model_endpoint_type")
                or data.get("modelEndpointType")
                or endpoint_config.get("endpoint_type")
                or endpoint_config.get("endpointType")
                or ""
            ),
            model_endpoint=str(
                data.get("model_endpoint")
                or data.get("modelEndpoint")
                or endpoint_config.get("url")
                or ""
            ),
            model_name=str(
                data.get("model_name")
                or data.get("modelName")
                or endpoint_config.get("model_name")
                or endpoint_config.get("modelName")
                or ""
            ),
            generation_config=decode_generation_config(generation_config),
            model_max_retries=int(
                data.get("model_max_retries")
                or data.get("modelMaxRetries")
                or endpoint_config.get("max_retries")
                or endpoint_config.get("maxRetries")
                or 0
            ),
            max_iterations=int(data.get("max_iterations") or data.get("maxIterations") or 30),
            workspace_dir=workspace,
            episode_id=str(data.get("episode_id") or data.get("episodeId") or ""),
            llm_config_path=str(data.get("llm_config_path") or data.get("llmConfigPath") or ""),
            mode=str(data.get("mode") or "llm"),
            instances_catalog=str(data.get("instances_catalog") or data.get("instancesCatalog") or ""),
            instance_catalog_json=str(
                data.get("instance_catalog_json") or data.get("instanceCatalogJson") or ""
            ),
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
    variant = normalize_benchmark_variant(str(overrides.get("benchmark_variant", "pro")))
    job = AgentJob(
        job_id=overrides.get("job_id", "job-local-1"),
        run_id=overrides.get("run_id", "run-local-1"),
        gateway_url=overrides.get("gateway_url", "http://127.0.0.1:28097"),
        gateway_api_key=overrides.get("gateway_api_key", "swe-pro-secret"),
        session_id=overrides.get("session_id"),
        instance_id=overrides["instance_id"],
        benchmark_variant=variant,
        mode=overrides.get("mode", "gold"),
        max_iterations=int(overrides.get("max_iterations", 30)),
        workspace_dir=resolve_workspace_dir(variant, overrides.get("workspace_dir")),
        llm_config_path=str(overrides.get("llm_config_path", "")),
        model_endpoint_type=str(overrides.get("model_endpoint_type", "")),
        model_endpoint=str(overrides.get("model_endpoint", "")),
        model_name=str(overrides.get("model_name", "")),
        generation_config=decode_generation_config(
            overrides.get("generation_config", {})
        ),
        model_max_retries=int(overrides.get("model_max_retries", 0)),
        instances_catalog=str(overrides.get("instances_catalog", "")),
        instance_catalog_json=str(overrides.get("instance_catalog_json", "")),
    )
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(job.__dict__, indent=2) + "\n", encoding="utf-8")
    return job
