"""AgentJob dynamic model config tests; no OpenHands, model, or GPU needed."""

from __future__ import annotations

import importlib.util
import json
import stat
import sys
import tempfile
import types
import unittest
from pathlib import Path


OPENHANDS_DIR = Path(__file__).resolve().parents[1]
if str(OPENHANDS_DIR) not in sys.path:
    sys.path.insert(0, str(OPENHANDS_DIR))

from uenv_runtime.agent_client import _job_from_proto  # noqa: E402
from uenv_runtime.agent_job import AgentJob  # noqa: E402


def _load_driver_module():
    path = OPENHANDS_DIR / "run_swebenchpro_official.py"
    spec = importlib.util.spec_from_file_location("uenv_swe_driver_model_test", path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"could not load {path}")
    module = importlib.util.module_from_spec(spec)
    # The helper under test is dependency-free; stub the two import-time
    # OpenHands adapters so importing the driver does not require the SDK.
    gateway_tools = types.ModuleType("uenv_runtime.gateway_tools")
    gateway_tools.collect_tool_patch_status = lambda *_args, **_kwargs: {}
    gateway_tools.patch_openhands_tools_for_uenv = lambda: {}
    workspace = types.ModuleType("uenv_runtime.workspace")
    workspace.UEnvWorkspace = object
    stubs = {
        "uenv_runtime.gateway_tools": gateway_tools,
        "uenv_runtime.workspace": workspace,
    }
    previous = {name: sys.modules.get(name) for name in stubs}
    sys.modules.update(stubs)
    try:
        spec.loader.exec_module(module)
    finally:
        for name, old_module in previous.items():
            if old_module is None:
                sys.modules.pop(name, None)
            else:
                sys.modules[name] = old_module
    return module


class _FakeModelEndpoint:
    endpoint_type = "http"
    url = "http://gpu.internal:18080/v1"
    model_name = "Qwen/Qwen3-8B"
    generation_config_json = json.dumps(
        {
            "temperature": 0.2,
            "top_p": 0.9,
            "max_new_tokens": 4096,
            "thinking_token_budget": 1024,
        }
    ).encode("utf-8")
    max_retries = 5


class _FakeProtoJob:
    job_id = "job-1"
    run_id = "run-1"
    gateway_url = "http://worker.internal:28999"
    gateway_api_key = "gateway-secret"
    session_id = "session-1"
    instance_id = "instance-1"
    benchmark_variant = "pro"
    env_package_id = ""
    env_package_version = ""
    agent_bridge_id = "uenv-agent-openhands"
    agent_bridge_version = "1.0.0"
    driver_entrypoint = "run_swebenchpro_official.py"
    model_endpoint_config = _FakeModelEndpoint()
    max_iterations = 12
    workspace_dir = "/app"
    episode_id = "episode-1"
    llm_config_path = "/etc/uenv/openhands-llm.json"
    mode = "llm"
    instances_catalog = ""
    instance_catalog_json = "{}"
    task_payload_json = ""

    @staticmethod
    def HasField(name: str) -> bool:  # noqa: N802 - protobuf API spelling
        return name == "model_endpoint_config"


class AgentModelConfigTests(unittest.TestCase):
    def test_proto_model_endpoint_is_fully_mapped(self):
        job = _job_from_proto(_FakeProtoJob())

        self.assertEqual(job.model_endpoint_type, "http")
        self.assertEqual(job.model_endpoint, "http://gpu.internal:18080/v1")
        self.assertEqual(job.model_name, "Qwen/Qwen3-8B")
        self.assertEqual(job.generation_config["max_new_tokens"], 4096)
        self.assertEqual(job.generation_config["temperature"], 0.2)
        self.assertEqual(job.model_max_retries, 5)

    def test_agent_job_json_round_trip_keeps_model_config(self):
        job = AgentJob.from_dict(
            {
                "job_id": "job-2",
                "run_id": "run-2",
                "gateway_url": "http://worker:28999",
                "instance_id": "instance-2",
                "model_endpoint_config": {
                    "endpoint_type": "http",
                    "url": "http://gpu:18080/v1",
                    "model_name": "Qwen/Qwen3-14B",
                    "generation_config_json": '{"temperature":0.1}',
                    "max_retries": 3,
                },
            }
        )

        self.assertEqual(job.model_endpoint, "http://gpu:18080/v1")
        self.assertEqual(job.model_name, "Qwen/Qwen3-14B")
        self.assertEqual(job.generation_config, {"temperature": 0.1})
        self.assertEqual(job.model_max_retries, 3)

    def test_effective_config_overrides_endpoint_and_preserves_api_key(self):
        driver = _load_driver_module()
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            template_path = root / "template.json"
            template = {
                "model": "openai/old-model",
                "base_url": "http://old-model:8000/v1",
                "api_key": "keep-this-secret",
                "temperature": 0.0,
                "max_output_tokens": 1024,
                "num_retries": 2,
                "request_timeout": 7200,
            }
            template_path.write_text(json.dumps(template), encoding="utf-8")
            job = _job_from_proto(_FakeProtoJob())

            effective_path = driver._write_effective_llm_config(
                agent_job=job,
                template_path=template_path,
                output_dir=root / "run-output",
            )
            effective = json.loads(effective_path.read_text(encoding="utf-8"))

            self.assertEqual(effective["base_url"], _FakeModelEndpoint.url)
            self.assertEqual(effective["model"], "openai/Qwen/Qwen3-8B")
            self.assertEqual(effective["api_key"], "keep-this-secret")
            self.assertEqual(effective["temperature"], 0.2)
            self.assertEqual(effective["top_p"], 0.9)
            self.assertEqual(effective["max_output_tokens"], 4096)
            self.assertEqual(effective["thinking_token_budget"], 1024)
            self.assertEqual(effective["num_retries"], 5)
            self.assertEqual(effective["request_timeout"], 7200)
            self.assertEqual(
                stat.S_IMODE(effective_path.stat().st_mode),
                0o600,
            )
            self.assertEqual(
                json.loads(template_path.read_text(encoding="utf-8")), template
            )
            self.assertFalse(
                any(path.suffix == ".tmp" for path in effective_path.parent.iterdir())
            )


if __name__ == "__main__":
    unittest.main()
