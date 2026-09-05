from __future__ import annotations

import asyncio
import json
import os
import shlex
import subprocess
import time
import uuid
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any

from .protocol import EpisodeRequest, EpisodeResult, EpisodeSummary, StepRecord, Trajectory
from .utils import to_jsonable
from .verl_agent_loop import (
    AgentLoopMetrics,
    AgentLoopOutput,
    UEnvAgentLoop,
    _float_value,
    _int_value,
    _optional_string,
    register,
    rollout_trace_op,
    simple_timer,
)

try:
    from verl.experimental.agent_loop.agent_loop import AgentLoopMetrics as _NativeVerlAgentLoopMetrics
except Exception:
    _NativeVerlAgentLoopMetrics = AgentLoopMetrics


class _UnusedEpisodeClient:
    def submit_episode(self, _request: EpisodeRequest) -> EpisodeResult:
        raise RuntimeError("NativeSweAgentLoop does not use UEnv Adapter Core")

    def submit_episode_stream(self, _requests: list[EpisodeRequest]):
        raise RuntimeError("NativeSweAgentLoop does not use UEnv Adapter Core")


@dataclass(slots=True)
class NativeSweAgentLoopConfig:
    driver_path: str
    python_executable: str = "python3"
    output_root: str = ""
    execution_backend: str = "local"
    runtime_gateway_url: str = ""
    runtime_gateway_api_key: str = ""
    llm_config_path: str = ""
    instances_catalog: str = ""
    openhands_mode: str = "llm"
    rollout_trace: str = "required"
    run_timeout_seconds: float = 7200.0
    max_concurrency: int = 1
    remote_host: str = ""
    remote_user: str = "root"
    remote_port: int = 22
    remote_identity_file: str = ""
    remote_password: str = ""
    remote_work_root: str = "/tmp/uenv-native-swe-agent-loop"
    remote_runner: str = "/root/UEnv/scripts/run-openhands-pro-20877.sh"
    remote_bridge_dir: str = ""
    remote_llm_config_path: str = ""
    remote_ssh_extra_args: str = ""
    agent_control_host: str = "0.0.0.0"
    agent_control_port: int = 19051
    agent_control_public_endpoint: str = ""


@register("native_swe_agent")
class NativeSweAgentLoop(UEnvAgentLoop):
    """Native VeRL baseline AgentLoop for SWE/OpenHands tasks.

    This loop is intentionally not a UEnv Server/Adapter client.  It runs inside
    VeRL, writes a one-sample AgentJob file, invokes the existing OpenHands SWE
    driver directly, then converts the driver's JSON artifacts back to
    AgentLoopOutput.
    """

    def __init__(
        self,
        *args: Any,
        driver_path: str | None = None,
        python_executable: str | None = None,
        output_root: str | None = None,
        execution_backend: str | None = None,
        runtime_gateway_url: str | None = None,
        runtime_gateway_api_key: str | None = None,
        llm_config_path: str | None = None,
        instances_catalog: str | None = None,
        openhands_mode: str | None = None,
        rollout_trace: str | None = None,
        run_timeout_seconds: float | None = None,
        max_concurrency: int | None = None,
        remote_host: str | None = None,
        remote_user: str | None = None,
        remote_port: int | None = None,
        remote_identity_file: str | None = None,
        remote_password: str | None = None,
        remote_work_root: str | None = None,
        remote_runner: str | None = None,
        remote_bridge_dir: str | None = None,
        remote_llm_config_path: str | None = None,
        remote_ssh_extra_args: str | None = None,
        agent_control_host: str | None = None,
        agent_control_port: int | None = None,
        agent_control_public_endpoint: str | None = None,
        **kwargs: Any,
    ) -> None:
        kwargs.pop("client", None)
        kwargs.pop("client_mode", None)
        kwargs.pop("mode", None)
        super().__init__(*args, client=_UnusedEpisodeClient(), client_mode="fake", mode="fake", **kwargs)
        self.native_swe_config = NativeSweAgentLoopConfig(
            driver_path=_optional_string(driver_path) or self._default_driver_path(),
            python_executable=_optional_string(python_executable) or os.environ.get("NATIVE_SWE_PYTHON", "python3"),
            output_root=_optional_string(output_root) or self._default_output_root(),
            execution_backend=(
                _optional_string(execution_backend) or os.environ.get("NATIVE_SWE_EXECUTION_BACKEND", "local")
            ).lower(),
            runtime_gateway_url=(
                _optional_string(runtime_gateway_url)
                or os.environ.get("NATIVE_SWE_RUNTIME_GATEWAY_URL", "")
                or os.environ.get("UENV_SWE_RUNTIME_GATEWAY_URL", "")
                or os.environ.get("UENV_GATEWAY", "")
            ),
            runtime_gateway_api_key=(
                _optional_string(runtime_gateway_api_key)
                or os.environ.get("NATIVE_SWE_RUNTIME_GATEWAY_API_KEY", "")
                or os.environ.get("UENV_GATEWAY_API_KEY", "")
            ),
            llm_config_path=(
                _optional_string(llm_config_path)
                or os.environ.get("NATIVE_SWE_LLM_CONFIG_PATH", "")
                or os.environ.get("SWE_LLM_CONFIG_PATH", "")
                or os.environ.get("OPENHANDS_LLM_CONFIG", "")
            ),
            instances_catalog=(
                _optional_string(instances_catalog)
                or os.environ.get("NATIVE_SWE_INSTANCES_CATALOG", "")
                or os.environ.get("UENV_SWE_INSTANCES", "")
                or os.environ.get("UENV_SWE_ENV_PACKAGE_CATALOG", "")
            ),
            openhands_mode=(_optional_string(openhands_mode) or os.environ.get("NATIVE_SWE_OPENHANDS_MODE", "llm")).lower(),
            rollout_trace=_optional_string(rollout_trace) or os.environ.get("NATIVE_SWE_ROLLOUT_TRACE", "required"),
            run_timeout_seconds=max(1.0, _float_value(run_timeout_seconds, 7200.0)),
            max_concurrency=max(1, _int_value(max_concurrency, 1)),
            remote_host=_optional_string(remote_host) or os.environ.get("NATIVE_SWE_REMOTE_HOST", ""),
            remote_user=_optional_string(remote_user) or os.environ.get("NATIVE_SWE_REMOTE_USER", "root"),
            remote_port=max(1, _int_value(remote_port or os.environ.get("NATIVE_SWE_REMOTE_PORT"), 22)),
            remote_identity_file=(
                _optional_string(remote_identity_file) or os.environ.get("NATIVE_SWE_REMOTE_IDENTITY_FILE", "")
            ),
            remote_password=_optional_string(remote_password) or os.environ.get("NATIVE_SWE_REMOTE_PASSWORD", ""),
            remote_work_root=(
                _optional_string(remote_work_root)
                or os.environ.get("NATIVE_SWE_REMOTE_WORK_ROOT", "/tmp/uenv-native-swe-agent-loop")
            ),
            remote_runner=(
                _optional_string(remote_runner)
                or os.environ.get("NATIVE_SWE_REMOTE_RUNNER", "/root/UEnv/scripts/run-openhands-pro-20877.sh")
            ),
            remote_bridge_dir=_optional_string(remote_bridge_dir) or os.environ.get("NATIVE_SWE_REMOTE_BRIDGE_DIR", ""),
            remote_llm_config_path=(
                _optional_string(remote_llm_config_path) or os.environ.get("NATIVE_SWE_REMOTE_LLM_CONFIG_PATH", "")
            ),
            remote_ssh_extra_args=(
                _optional_string(remote_ssh_extra_args) or os.environ.get("NATIVE_SWE_REMOTE_SSH_EXTRA_ARGS", "")
            ),
            agent_control_host=(
                _optional_string(agent_control_host) or os.environ.get("NATIVE_SWE_AGENT_CONTROL_HOST", "0.0.0.0")
            ),
            agent_control_port=max(1, _int_value(agent_control_port or os.environ.get("NATIVE_SWE_AGENT_CONTROL_PORT"), 19051)),
            agent_control_public_endpoint=(
                _optional_string(agent_control_public_endpoint)
                or os.environ.get("NATIVE_SWE_AGENT_CONTROL_PUBLIC_ENDPOINT", "")
            ),
        )

    @rollout_trace_op
    async def run(self, sampling_params: dict[str, Any], **kwargs: Any) -> AgentLoopOutput:
        messages = self._messages_from_raw_prompt(kwargs.get("raw_prompt"))
        prompt_ids = await self._prompt_ids(messages)
        runtime_model = await self._runtime_model_endpoint(sampling_params, kwargs)
        request = self.build_episode_request(
            sampling_params=sampling_params,
            prompt_ids=prompt_ids,
            raw_prompt=kwargs.get("raw_prompt"),
            sample_kwargs=kwargs,
            model_endpoint_override=runtime_model[0],
            model_name_override=runtime_model[1],
            model_upstream_overrides=runtime_model[2],
        )

        metrics: dict[str, float] = {}
        with simple_timer("generate_sequences", metrics):
            self._record_episode_requests([request], phase="native_swe_submit_single")
            result = await asyncio.to_thread(self._run_driver_for_request, request)
            self._record_episode_results([result], [request], phase="native_swe_result_single")
        if result.status not in {"completed", "recorded"}:
            self._raise_if_failed([result])
            output = self._failed_output_from_result(request, result, prompt_ids=prompt_ids, metrics=metrics)
            return self._with_native_metrics(output, metrics)
        return self._with_native_metrics(self._output_from_result(request, result), metrics)

    async def run_batch(
        self,
        sampling_params_by_sample: list[dict[str, Any]],
        sample_kwargs_by_sample: list[dict[str, Any]],
        *,
        batch_id: str,
    ) -> list[AgentLoopOutput]:
        training_run_id = str(os.environ.get("UENV_TRAINING_RUN_ID") or batch_id or f"native-swe-{uuid.uuid4().hex[:8]}")
        requests = await self._build_batch_requests(
            sampling_params_by_sample,
            sample_kwargs_by_sample,
            batch_id=batch_id,
            training_run_id=training_run_id,
        )
        self._record_episode_requests(requests, phase="native_swe_submit_batch")
        semaphore = asyncio.Semaphore(self.native_swe_config.max_concurrency)

        async def run_one(request: EpisodeRequest) -> EpisodeResult:
            async with semaphore:
                return await asyncio.to_thread(self._run_driver_for_request, request)

        results = await asyncio.gather(*(run_one(request) for request in requests))
        self._record_episode_results(list(results), requests, phase="native_swe_result_batch")
        self._raise_if_failed(list(results))
        return [
            self._with_native_metrics(self._output_from_result(request, result))
            for request, result in zip(requests, results, strict=True)
        ]

    def _native_agent_metrics(self, metrics: dict[str, float] | None = None) -> dict[str, float | int]:
        # Keep this as a plain dict. Ray may deserialize an identically named
        # AgentLoopMetrics class from a different module object; Pydantic accepts
        # a dict and rebuilds the exact class required by VeRL postprocess.
        return {
            "generate_sequences": float((metrics or {}).get("generate_sequences", 0.0)),
            "tool_calls": 0.0,
            "compute_score": 0.0,
            "num_preempted": -1,
        }

    def _with_native_metrics(self, output: AgentLoopOutput, metrics: dict[str, float] | None = None) -> AgentLoopOutput:
        output.metrics = self._native_agent_metrics(metrics)
        return output

    def _raise_if_failed(self, results: list[EpisodeResult]) -> None:
        if self.config_for_uenv.failed_episode_policy == "zero_reward":
            return
        for result in results:
            if result.status not in {"completed", "recorded"}:
                raise RuntimeError(
                    f"native SWE AgentLoop episode failed: request_id={result.request_id} "
                    f"status={result.status} error={result.error_message}"
                )

    def _run_driver_for_request(self, request: EpisodeRequest) -> EpisodeResult:
        started = time.time()
        payload = self._payload_dict(request)
        metadata = payload.get("metadata") if isinstance(payload.get("metadata"), dict) else {}
        env_config = payload.get("env_config") if isinstance(payload.get("env_config"), dict) else {}
        model_endpoint = payload.get("model_endpoint") if isinstance(payload.get("model_endpoint"), dict) else {}
        batch_id = str(metadata.get("batch_id") or "batch")
        sample_index = str(metadata.get("sample_index") or "0")
        run_id = str(metadata.get("training_run_id") or os.environ.get("UENV_TRAINING_RUN_ID") or batch_id)
        output_dir = self._output_dir(run_id, batch_id, sample_index, request.request_id)
        output_dir.mkdir(parents=True, exist_ok=True)

        try:
            agent_job_path = self._write_agent_job(
                request=request,
                payload=payload,
                env_config=env_config,
                model_endpoint=model_endpoint,
                output_dir=output_dir,
                run_id=run_id,
            )
            proc = self._run_driver_subprocess(agent_job_path, output_dir)
        except Exception as exc:  # noqa: BLE001
            return self._failed_result(
                request,
                output_dir=output_dir,
                started=started,
                error_message=f"{type(exc).__name__}: {exc}",
            )

        if proc.returncode != 0:
            submit_result = output_dir / "submit_result.json"
            if not submit_result.is_file():
                return self._failed_result(
                    request,
                    output_dir=output_dir,
                    started=started,
                    error_message=self._driver_error_message(proc),
                    error_code=proc.returncode,
                )

        try:
            return self._result_from_output_dir(request, output_dir=output_dir, started=started, returncode=proc.returncode)
        except Exception as exc:  # noqa: BLE001
            return self._failed_result(
                request,
                output_dir=output_dir,
                started=started,
                error_message=f"parse driver output failed: {type(exc).__name__}: {exc}",
                error_code=proc.returncode,
            )

    def _write_agent_job(
        self,
        *,
        request: EpisodeRequest,
        payload: dict[str, Any],
        env_config: dict[str, Any],
        model_endpoint: dict[str, Any],
        output_dir: Path,
        run_id: str,
    ) -> Path:
        gateway_url = str(self._first_nonempty(env_config.get("gateway_url"), self.native_swe_config.runtime_gateway_url))
        if not gateway_url and not env_config.get("session_id"):
            raise ValueError("native SWE AgentLoop requires NATIVE_SWE_RUNTIME_GATEWAY_URL, UENV_GATEWAY, or extra_info.session_id")
        instance_id = str(env_config.get("instance_id") or "")
        if not instance_id:
            raise ValueError("native SWE AgentLoop requires SWE instance_id")
        generation_config = model_endpoint.get("generation_config") if isinstance(model_endpoint.get("generation_config"), dict) else {}
        job = {
            "job_id": request.request_id,
            "run_id": run_id,
            "gateway_url": gateway_url,
            "gateway_api_key": self._first_nonempty(env_config.get("gateway_api_key"), self.native_swe_config.runtime_gateway_api_key),
            "session_id": self._first_nonempty(env_config.get("session_id"), ""),
            "instance_id": instance_id,
            "benchmark_variant": env_config.get("benchmark_variant") or "smith",
            "env_package_id": env_config.get("env_package_id") or "",
            "env_package_version": env_config.get("env_package_version") or "",
            "agent_bridge_id": "native-swe-agent-loop",
            "agent_bridge_version": "0.1.0",
            "driver_entrypoint": env_config.get("driver_entrypoint") or Path(self.native_swe_config.driver_path).name,
            "model_endpoint_type": "http",
            "model_endpoint": model_endpoint.get("url") or request.model_endpoint,
            "model_name": model_endpoint.get("model_name") or "",
            "generation_config": generation_config,
            "model_max_retries": model_endpoint.get("max_retries") or 3,
            "max_iterations": env_config.get("max_iterations") or request.max_steps,
            "workspace_dir": env_config.get("workspace_dir") or "/testbed",
            "episode_id": request.request_id,
            "llm_config_path": self._llm_config_path_for_job(env_config),
            "mode": self.native_swe_config.openhands_mode,
            "instances_catalog": self._first_nonempty(env_config.get("instances_catalog"), self.native_swe_config.instances_catalog),
            "instance_catalog_json": self._first_nonempty(env_config.get("instance_catalog_json"), ""),
        }
        path = output_dir / "agent_job.json"
        path.write_text(json.dumps(to_jsonable(job), ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        return path

    def _run_driver_subprocess(self, agent_job_path: Path, output_dir: Path) -> subprocess.CompletedProcess[str]:
        if self.native_swe_config.execution_backend == "grpc":
            proc = self._run_driver_via_agent_control(agent_job_path, output_dir)
        elif self.native_swe_config.execution_backend == "ssh":
            proc = self._run_driver_via_ssh(agent_job_path, output_dir)
        elif self.native_swe_config.execution_backend == "local":
            proc = self._run_driver_local(agent_job_path, output_dir)
        else:
            raise ValueError(f"unsupported native SWE execution backend: {self.native_swe_config.execution_backend}")
        (output_dir / "native_swe_driver_stdout.log").write_text(proc.stdout or "", encoding="utf-8")
        (output_dir / "native_swe_driver_stderr.log").write_text(proc.stderr or "", encoding="utf-8")
        (output_dir / "native_swe_driver_exit.json").write_text(
            json.dumps({"returncode": proc.returncode, "cmd": proc.args}, ensure_ascii=False, indent=2) + "\n",
            encoding="utf-8",
        )
        return proc

    def _run_driver_via_agent_control(self, agent_job_path: Path, output_dir: Path) -> subprocess.CompletedProcess[str]:
        from .native_agent_control_server import build_agent_job_proto, get_native_agent_control_server

        server = get_native_agent_control_server(
            self.native_swe_config.agent_control_host,
            self.native_swe_config.agent_control_port,
        )
        job = self._read_json_object(agent_job_path)
        pending = server.enqueue(build_agent_job_proto(job))
        try:
            result = server.wait_result(pending, self.native_swe_config.run_timeout_seconds)
        except TimeoutError as exc:
            return subprocess.CompletedProcess(args=["native-agent-control", server.endpoint], returncode=124, stdout="", stderr=str(exc))
        submit = {
            "instance_id": job.get("instance_id", ""),
            "resolved": result.metadata.get("resolved", ""),
            "reward": result.reward,
            "tests_passed": result.metadata.get("tests_passed", ""),
            "tests_total": result.metadata.get("tests_total", ""),
            "trajectory_ref": {"trajectory_id": result.trajectory_id},
            "rollout_trace": {
                "response_ids": result.response_ids,
                "response_mask": result.response_mask,
            },
            "rollout_log_probs": result.rollout_log_probs,
            "parallel_mode": result.parallel_mode,
            "rollout_policy_version": result.rollout_policy_version,
        }
        if result.rollout_param_version is not None:
            submit["rollout_param_version"] = result.rollout_param_version
        status_ok = result.status in {"completed", "recorded"}
        if not status_ok:
            (output_dir / "native_agent_control_result.json").write_text(
                json.dumps(to_jsonable(submit | {"status": result.status, "error_message": result.error_message}), ensure_ascii=False, indent=2) + "\n",
                encoding="utf-8",
            )
            return subprocess.CompletedProcess(
                args=["native-agent-control", self._agent_control_endpoint_for_user(server.endpoint)],
                returncode=1,
                stdout=f"native AgentControl job completed status={result.status} reward={result.reward}\n",
                stderr=result.error_message,
            )
        (output_dir / "submit_result.json").write_text(json.dumps(to_jsonable(submit), ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        trace = {
            "rollout_trace": submit["rollout_trace"],
            "rollout_log_probs": result.rollout_log_probs,
            "parallel_mode": result.parallel_mode,
            "rollout_policy_version": result.rollout_policy_version,
        }
        if result.rollout_param_version is not None:
            trace["rollout_param_version"] = result.rollout_param_version
        (output_dir / "llm_rollout_trace.json").write_text(json.dumps(to_jsonable(trace), ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        bundle = {
            "artifact": {
                "git_diff": "",
                "metadata": result.metadata,
            },
            "reward": result.reward,
            "resolved": result.metadata.get("resolved", ""),
        }
        (output_dir / "trajectory_bundle.json").write_text(json.dumps(to_jsonable(bundle), ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        return subprocess.CompletedProcess(
            args=["native-agent-control", self._agent_control_endpoint_for_user(server.endpoint)],
            returncode=0,
            stdout=f"native AgentControl job completed status={result.status} reward={result.reward}\n",
            stderr=result.error_message,
        )

    def _run_driver_local(self, agent_job_path: Path, output_dir: Path) -> subprocess.CompletedProcess[str]:
        driver_path = Path(self.native_swe_config.driver_path)
        if not driver_path.is_file():
            raise FileNotFoundError(f"native SWE driver not found: {driver_path}")
        env = os.environ.copy()
        env["UENV_AGENT_JOB_FILE"] = str(agent_job_path)
        env.setdefault("UENV_ROLLOUT_TRACE", self.native_swe_config.rollout_trace)
        cmd = [
            self.native_swe_config.python_executable,
            str(driver_path),
            "--agent-job-file",
            str(agent_job_path),
            "--output-dir",
            str(output_dir),
            "--mode",
            self.native_swe_config.openhands_mode,
            "--rollout-trace",
            self.native_swe_config.rollout_trace,
        ]
        proc = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            env=env,
            timeout=self.native_swe_config.run_timeout_seconds,
            check=False,
        )
        return proc

    def _run_driver_via_ssh(self, agent_job_path: Path, output_dir: Path) -> subprocess.CompletedProcess[str]:
        remote_dir = self._remote_output_dir(output_dir)
        remote_job = str(PurePosixPath(remote_dir) / "agent_job.json")
        self._ssh_checked(f"mkdir -p {shlex.quote(remote_dir)}")
        self._scp_to_remote(agent_job_path, remote_job)
        job = self._read_json_object(agent_job_path)
        remote_env = {
            "UENV_AGENT_JOB_FILE": remote_job,
            "OPENHANDS_OUT_DIR": remote_dir,
            "UENV_RUN_ID": str(job.get("run_id") or ""),
            "UENV_ROLLOUT_TRACE": self.native_swe_config.rollout_trace,
        }
        if self.native_swe_config.remote_bridge_dir:
            remote_env["UENV_AGENT_BRIDGE_DIR"] = self.native_swe_config.remote_bridge_dir
        if self.native_swe_config.runtime_gateway_api_key:
            remote_env["UENV_GATEWAY_API_KEY"] = self.native_swe_config.runtime_gateway_api_key
        if self.native_swe_config.remote_llm_config_path:
            remote_env["OPENHANDS_LLM_CONFIG"] = self.native_swe_config.remote_llm_config_path
        if job.get("benchmark_variant"):
            remote_env["UENV_BENCHMARK_VARIANT"] = str(job.get("benchmark_variant"))
        if job.get("max_iterations"):
            remote_env["MAX_ITERATIONS"] = str(job.get("max_iterations"))

        env_prefix = " ".join(f"{key}={shlex.quote(str(value))}" for key, value in remote_env.items() if value)
        remote_cmd = (
            f"env {env_prefix} bash {shlex.quote(self.native_swe_config.remote_runner)} "
            f"{shlex.quote(self.native_swe_config.openhands_mode)}"
        )
        cmd = [*self._ssh_base_cmd(), remote_cmd]
        proc = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=self.native_swe_config.run_timeout_seconds,
            env=self._ssh_subprocess_env(),
            check=False,
        )
        fetch_error = self._fetch_remote_output(remote_dir, output_dir)
        stderr = proc.stderr or ""
        if fetch_error:
            stderr = f"{stderr}\n[fetch-remote-output] {fetch_error}".strip()
        return subprocess.CompletedProcess(args=cmd, returncode=proc.returncode, stdout=proc.stdout or "", stderr=stderr)

    def _ssh_base_cmd(self) -> list[str]:
        if not self.native_swe_config.remote_host:
            raise ValueError("NATIVE_SWE_REMOTE_HOST is required when NATIVE_SWE_EXECUTION_BACKEND=ssh")
        cmd = []
        if self.native_swe_config.remote_password:
            cmd.extend(["sshpass", "-e"])
        cmd.extend([
            "ssh",
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "UserKnownHostsFile=/dev/null",
            "-p",
            str(self.native_swe_config.remote_port),
        ])
        if self.native_swe_config.remote_identity_file:
            cmd.extend(["-i", self.native_swe_config.remote_identity_file, "-o", "IdentitiesOnly=yes"])
        if self.native_swe_config.remote_ssh_extra_args:
            cmd.extend(shlex.split(self.native_swe_config.remote_ssh_extra_args))
        cmd.append(self._ssh_target())
        return cmd

    def _ssh_subprocess_env(self) -> dict[str, str] | None:
        if not self.native_swe_config.remote_password:
            return None
        env = os.environ.copy()
        env["SSHPASS"] = self.native_swe_config.remote_password
        return env

    def _ssh_target(self) -> str:
        if self.native_swe_config.remote_user:
            return f"{self.native_swe_config.remote_user}@{self.native_swe_config.remote_host}"
        return self.native_swe_config.remote_host

    def _ssh_checked(self, remote_cmd: str) -> None:
        proc = subprocess.run(
            [*self._ssh_base_cmd(), remote_cmd],
            capture_output=True,
            text=True,
            timeout=120,
            env=self._ssh_subprocess_env(),
            check=False,
        )
        if proc.returncode != 0:
            raise RuntimeError(f"ssh command failed: {proc.stderr or proc.stdout}")

    def _scp_to_remote(self, local_path: Path, remote_path: str) -> None:
        cmd = []
        if self.native_swe_config.remote_password:
            cmd.extend(["sshpass", "-e"])
        cmd.extend([
            "scp",
            "-q",
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "UserKnownHostsFile=/dev/null",
            "-P",
            str(self.native_swe_config.remote_port),
        ])
        if self.native_swe_config.remote_identity_file:
            cmd.extend(["-i", self.native_swe_config.remote_identity_file, "-o", "IdentitiesOnly=yes"])
        if self.native_swe_config.remote_ssh_extra_args:
            cmd.extend(shlex.split(self.native_swe_config.remote_ssh_extra_args))
        cmd.extend([str(local_path), f"{self._ssh_target()}:{remote_path}"])
        proc = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=120,
            env=self._ssh_subprocess_env(),
            check=False,
        )
        if proc.returncode != 0:
            raise RuntimeError(f"scp agent job failed: {proc.stderr or proc.stdout}")

    def _fetch_remote_output(self, remote_dir: str, output_dir: Path) -> str:
        output_dir.mkdir(parents=True, exist_ok=True)
        remote_cmd = f"test -d {shlex.quote(remote_dir)} && tar -C {shlex.quote(remote_dir)} -cf - ."
        proc = subprocess.run(
            [*self._ssh_base_cmd(), remote_cmd],
            capture_output=True,
            timeout=300,
            env=self._ssh_subprocess_env(),
            check=False,
        )
        if proc.returncode != 0:
            return proc.stderr.decode(errors="replace") if proc.stderr else "remote output directory not found"
        if not proc.stdout:
            return ""
        extract = subprocess.run(["tar", "-C", str(output_dir), "-xf", "-"], input=proc.stdout, capture_output=True, check=False)
        if extract.returncode != 0:
            return extract.stderr.decode(errors="replace")
        return ""

    def _remote_output_dir(self, output_dir: Path) -> str:
        root = Path(self.native_swe_config.output_root)
        try:
            rel = output_dir.relative_to(root)
        except ValueError:
            rel = Path(self._safe_path_component(output_dir.name))
        return str(PurePosixPath(self.native_swe_config.remote_work_root, *rel.parts))

    def _llm_config_path_for_job(self, env_config: dict[str, Any]) -> str:
        if self.native_swe_config.execution_backend in {"ssh", "grpc"} and self.native_swe_config.remote_llm_config_path:
            return self.native_swe_config.remote_llm_config_path
        return self._first_nonempty(env_config.get("llm_config_path"), self.native_swe_config.llm_config_path)

    def _agent_control_endpoint_for_user(self, server_endpoint: str) -> str:
        return self.native_swe_config.agent_control_public_endpoint or server_endpoint

    def _result_from_output_dir(
        self,
        request: EpisodeRequest,
        *,
        output_dir: Path,
        started: float,
        returncode: int,
    ) -> EpisodeResult:
        submit_doc = self._read_json_object(output_dir / "submit_result.json")
        trace_doc = self._read_json_object(output_dir / "llm_rollout_trace.json")
        bundle_doc = self._read_json_object(output_dir / "trajectory_bundle.json")
        response_ids, response_mask = self._rollout_trace_from_docs(submit_doc, trace_doc, bundle_doc)
        rollout_log_probs = self._rollout_logprobs_from_docs(submit_doc, trace_doc, bundle_doc)
        response_text = self._response_text_from_trace_docs(submit_doc, trace_doc)
        reward = float(submit_doc.get("reward", 0.0) or 0.0)
        steps = max(1, self._turn_count_from_docs(submit_doc, trace_doc))
        trajectory_ref = submit_doc.get("trajectory_ref") if isinstance(submit_doc.get("trajectory_ref"), dict) else {}
        metadata = self._result_metadata(submit_doc, bundle_doc, output_dir=output_dir, returncode=returncode)
        elapsed_ms = int(max(0.0, time.time() - started) * 1000)
        step = StepRecord(
            step_index=0,
            action=response_text.encode("utf-8"),
            reward=reward,
            terminated=True,
            info={key: str(value) for key, value in metadata.items()},
            duration_ms=elapsed_ms,
            response_ids=response_ids,
            response_mask=response_mask,
        )
        return EpisodeResult(
            request_id=request.request_id,
            status="completed",
            trajectory=Trajectory(steps=[step], total_reward=reward, total_steps=steps),
            summary=EpisodeSummary(total_reward=reward, total_steps=steps, total_duration_ms=elapsed_ms, terminate_reason="swe_evaluated"),
            trajectory_id=str(trajectory_ref.get("trajectory_id") or ""),
            metadata={key: str(value) for key, value in metadata.items()},
            rollout_log_probs=rollout_log_probs,
            rollout_param_version=self._int_or_none(self._first_present(submit_doc, trace_doc, bundle_doc, key="rollout_param_version")),
            rollout_policy_version=str(self._first_present(submit_doc, trace_doc, bundle_doc, key="rollout_policy_version") or ""),
        )

    def _failed_result(
        self,
        request: EpisodeRequest,
        *,
        output_dir: Path,
        started: float,
        error_message: str,
        error_code: int | None = None,
    ) -> EpisodeResult:
        elapsed_ms = int(max(0.0, time.time() - started) * 1000)
        metadata = {
            "native_swe_output_dir": str(output_dir),
            "native_swe_error_message": error_message[:4000],
        }
        return EpisodeResult(
            request_id=request.request_id,
            status="failed",
            trajectory=Trajectory(steps=[], total_reward=0.0, total_steps=0),
            summary=EpisodeSummary(total_reward=0.0, total_steps=0, total_duration_ms=elapsed_ms, terminate_reason="failed"),
            error_code=error_code,
            error_message=error_message,
            metadata=metadata,
        )

    def _output_dir(self, run_id: str, batch_id: str, sample_index: str, request_id: str) -> Path:
        safe_run = self._safe_path_component(run_id)
        safe_batch = self._safe_path_component(batch_id)
        safe_request = self._safe_path_component(request_id)
        return Path(self.native_swe_config.output_root) / safe_run / safe_batch / f"{sample_index}-{safe_request}"

    def _default_driver_path(self) -> str:
        env_path = os.environ.get("NATIVE_SWE_DRIVER_PATH", "")
        if env_path:
            return env_path
        repo_root = Path(__file__).resolve().parents[3]
        candidates = [
            repo_root / "integrations/openhands/run_swebenchpro_official.py",
            Path("/uenv/integrations/openhands/run_swebenchpro_official.py"),
        ]
        for candidate in candidates:
            if candidate.is_file():
                return str(candidate)
        return str(candidates[0])

    def _default_output_root(self) -> str:
        repo_root = Path(__file__).resolve().parents[3]
        return os.environ.get("NATIVE_SWE_OUTPUT_ROOT", str(repo_root / "temp/logs/native_swe_agent_loop"))

    def _read_json_object(self, path: Path) -> dict[str, Any]:
        if not path.is_file():
            return {}
        value = json.loads(path.read_text(encoding="utf-8"))
        return value if isinstance(value, dict) else {}

    def _rollout_trace_from_docs(self, *docs: dict[str, Any]) -> tuple[list[int], list[int]]:
        for doc in docs:
            trace = doc.get("rollout_trace") if isinstance(doc, dict) else None
            if isinstance(trace, dict):
                ids = self._int_list(trace.get("response_ids"))
                mask = self._int_list(trace.get("response_mask"))
                if ids:
                    return ids, mask or [1] * len(ids)
        return [], []

    def _rollout_logprobs_from_docs(self, *docs: dict[str, Any]) -> list[float]:
        for doc in docs:
            raw = doc.get("rollout_log_probs") if isinstance(doc, dict) else None
            if isinstance(raw, list) and raw:
                return [float(item) for item in raw]
        return []

    def _response_text_from_trace_docs(self, *docs: dict[str, Any]) -> str:
        parts: list[str] = []
        for doc in docs:
            turns = doc.get("turns") if isinstance(doc, dict) else None
            if not isinstance(turns, list):
                continue
            for turn in turns:
                if isinstance(turn, dict) and turn.get("assistant_output"):
                    parts.append(str(turn.get("assistant_output")))
        return "\n".join(parts)

    def _turn_count_from_docs(self, *docs: dict[str, Any]) -> int:
        for doc in docs:
            turns = doc.get("turns") if isinstance(doc, dict) else None
            if isinstance(turns, list) and turns:
                return len(turns)
        return 1

    def _result_metadata(self, submit_doc: dict[str, Any], bundle_doc: dict[str, Any], *, output_dir: Path, returncode: int) -> dict[str, Any]:
        artifact = bundle_doc.get("artifact") if isinstance(bundle_doc.get("artifact"), dict) else {}
        git_diff = str(artifact.get("git_diff") or "")
        trajectory_ref = submit_doc.get("trajectory_ref") if isinstance(submit_doc.get("trajectory_ref"), dict) else {}
        return {
            "native_swe_output_dir": str(output_dir),
            "native_swe_driver_returncode": returncode,
            "resolved": submit_doc.get("resolved", ""),
            "tests_passed": submit_doc.get("tests_passed", ""),
            "tests_total": submit_doc.get("tests_total", ""),
            "git_diff_bytes": len(git_diff.encode("utf-8")),
            "git_diff_nonempty": int(bool(git_diff.strip())),
            "trajectory_id": trajectory_ref.get("trajectory_id", ""),
        }

    def _driver_error_message(self, proc: subprocess.CompletedProcess[str]) -> str:
        text = "\n".join(part for part in (proc.stderr, proc.stdout) if part)
        return text[-4000:] if text else f"driver exited with returncode={proc.returncode}"

    def _int_list(self, value: Any) -> list[int]:
        if not isinstance(value, list):
            return []
        out: list[int] = []
        for item in value:
            try:
                out.append(int(item))
            except (TypeError, ValueError):
                return []
        return out

    def _int_or_none(self, value: Any) -> int | None:
        if value in (None, ""):
            return None
        try:
            return int(value)
        except (TypeError, ValueError):
            return None

    def _first_nonempty(self, *values: Any) -> Any:
        for value in values:
            if value not in (None, ""):
                return value
        return ""

    def _first_present(self, *docs: dict[str, Any], key: str) -> Any:
        for doc in docs:
            if isinstance(doc, dict) and key in doc and doc[key] not in (None, ""):
                return doc[key]
        return None

    def _safe_path_component(self, value: str) -> str:
        safe = []
        for char in str(value):
            safe.append(char if char.isalnum() or char in {"-", "_", "."} else "_")
        return "".join(safe)[:160] or "unknown"
