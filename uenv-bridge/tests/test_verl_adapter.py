from __future__ import annotations

import json
import stat
import tempfile
import time
import unittest
from pathlib import Path

from uenv.bridge.clients import (
    FakeEpisodeClient,
    GrpcEpisodeClient,
    GrpcEpisodeClientConfig,
    DryRunEpisodeClient,
    RustCoreClientConfig,
    RustCoreEpisodeClient,
)
from uenv.bridge.protocol import request_to_jsonable
from uenv.bridge.verl import VeRLAdapter, VeRLAdapterConfig


def make_batch() -> dict:
    return {
        "meta_info": {
            "batch_id": "pre-rollout-batch-0001",
            "global_steps": 7,
            "temperature": 0.7,
            "top_p": 0.9,
            "max_response_length": 128,
            "rollout_n": 2,
            "model_endpoint": "http://policy.example/v1",
            "model_name": "policy-model",
        },
        "batch": {
            "prompts": [[1, 2, 3], [4, 5, 0]],
            "attention_mask": [[1, 1, 1], [1, 1, 0]],
            "position_ids": [[0, 1, 2], [0, 1, 2]],
        },
        "non_tensor_batch": {
            "uid": ["uid-1", "uid-2"],
            "prompt_id": ["prompt-1", "prompt-2"],
            "task_name": ["math", "code"],
            "raw_prompt": [
                [{"role": "user", "content": "What is 2 + 2?"}],
                [{"role": "user", "content": "Write add(a, b)."}],
            ],
            "data_source": ["gsm8k", "humaneval"],
            "reward_model": [
                {"style": "rule", "ground_truth": "4"},
                {"style": "unit_test", "tests": ["assert add(2, 3) == 5"]},
            ],
            "agent_name": ["uenv_agent", "uenv_agent"],
            "extra_info": [
                {
                    "index": 1,
                    "rollout_n": 0,
                    "required_result_fields": ["response_ids", "response_mask", "reward", "trajectory"],
                },
                {
                    "index": 2,
                    "rollout_n": 1,
                    "required_result_fields": ["response_ids", "response_mask", "reward", "trajectory"],
                },
            ],
        },
    }


class FakeRustCoreStub:
    def __init__(self, reward: float = 3.0) -> None:
        self.reward = reward
        self.last_request = None

    def ExecuteBatch(self, request):
        self.last_request = request
        return {
            "request_id": request["request_id"],
            "batch_id": request["batch_id"],
            "results": [
                {
                    "request_id": sample["request_id"],
                    "batch_id": sample["batch_id"],
                    "sample_index": sample["sample_index"],
                    "status": "completed",
                    "reward": self.reward,
                    "done": True,
                    "termination_reason": "fake_core",
                    "trajectory_json": b"[]",
                    "error_code": "",
                    "error_message": "",
                }
                for sample in request["samples"]
            ],
        }


class MissingGeneratedStubRustCoreClient(RustCoreEpisodeClient):
    def _build_generated_stub(self):
        return None


class AutoStartTestRustCoreClient(RustCoreEpisodeClient):
    def _build_generated_stub(self):
        return FakeRustCoreStub()

    def _wait_for_health(self) -> None:
        return None


class VeRLAdapterTest(unittest.TestCase):
    def test_to_episode_requests_preserves_metadata(self) -> None:
        adapter = VeRLAdapter()
        requests = adapter.to_episode_requests(make_batch())

        self.assertEqual(len(requests), 2)
        self.assertEqual(requests[0].env_type, "math")
        self.assertEqual(requests[0].max_steps, 10)
        self.assertEqual(requests[1].env_type, "code")
        self.assertEqual(requests[1].max_steps, 80)

        payload0 = json.loads(requests[0].payload.decode("utf-8"))
        self.assertEqual(payload0["framework"], "verl")
        self.assertEqual(payload0["correlation_id"], "pre-rollout-batch-0001-0")
        self.assertEqual(payload0["metadata"]["batch_id"], "pre-rollout-batch-0001")
        self.assertEqual(payload0["metadata"]["sample_index"], 0)
        self.assertEqual(payload0["metadata"]["uid"], "uid-1")
        self.assertEqual(payload0["metadata"]["prompt_id"], "prompt-1")
        self.assertEqual(payload0["metadata"]["global_steps"], 7)
        self.assertEqual(payload0["reward_config"]["reward_type"], "rubric")
        self.assertEqual(payload0["model_endpoint"]["generation_config"]["top_p"], 0.9)
        self.assertEqual(payload0["episode_config"]["initial_observation"]["prompts"], [1, 2, 3])
        self.assertNotIn("response_text", payload0["env_config"])

    def test_execute_batch_with_fake_client_returns_ordered_results(self) -> None:
        adapter = VeRLAdapter(client=FakeEpisodeClient(reward=2.5))
        output = adapter.execute_batch(make_batch())

        self.assertEqual(output["batch_id"], "pre-rollout-batch-0001")
        self.assertEqual(len(output["results"]), 2)
        self.assertEqual(output["results"][0]["reward"], 2.5)
        self.assertEqual(output["results"][1]["reward"], 2.5)
        self.assertTrue(output["results"][0]["done"])
        self.assertIsNone(output["results"][0]["uenv_error"])

    def test_fake_math_reward_uses_rubric_ground_truth(self) -> None:
        sample = {
            "task_name": "math",
            "raw_prompt": "What is 2 + 2? The expected final answer is 4.",
            "data_source": "gsm8k",
            "reward_model": {"style": "rule", "ground_truth": "4"},
        }
        adapter = VeRLAdapter(client=FakeEpisodeClient(reward=0.0, math_reward=True))
        result = adapter.execute_episode(sample)

        self.assertEqual(result["reward"], 1.0)
        self.assertTrue(result["done"])

    def test_dry_run_client_writes_episode_requests(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            client = DryRunEpisodeClient(temp_dir)
            adapter = VeRLAdapter(client=client)
            requests = adapter.to_episode_requests(make_batch())
            list(client.submit_episode_stream(requests))

            output = Path(temp_dir) / "episode_requests.json"
            self.assertTrue(output.exists())
            written = json.loads(output.read_text(encoding="utf-8"))
            self.assertEqual(len(written), 2)
            payload = json.loads(written[0]["payload"])
            self.assertEqual(payload["metadata"]["sample_index"], 0)
            self.assertNotIn("response_text", payload["env_config"])

    def test_request_to_jsonable_decodes_payload(self) -> None:
        request = VeRLAdapter().to_episode_requests(make_batch())[0]
        payload = request_to_jsonable(request)
        self.assertIsInstance(payload["payload"], str)
        self.assertIn("pre-rollout-batch-0001-0", payload["payload"])

    def test_verl_config_loads_from_mapping(self) -> None:
        config = VeRLAdapterConfig.from_mapping(
            {
                "mapping": {
                    "default_env_type": "agent",
                    "task_to_env_type": {"logic": "math"},
                    "default_max_steps": 12,
                    "math_max_steps": 4,
                    "seed_base": 100,
                },
                "model_endpoint": {
                    "url": "http://localhost:8000/v1",
                    "model_name": "test-policy",
                },
                "server": {"grpc": {"timeout_seconds": 9}},
            }
        )

        self.assertEqual(config.default_env_type, "agent")
        self.assertEqual(config.task_to_env_type["logic"], "math")
        self.assertEqual(config.default_model_endpoint, "http://localhost:8000/v1")
        self.assertEqual(config.default_model_name, "test-policy")
        self.assertEqual(config.default_timeout_seconds, 9)
        self.assertEqual(config.default_max_steps, 12)
        self.assertEqual(config.math_max_steps, 4)
        self.assertEqual(config.seed_base, 100)

    def test_verl_config_loads_from_default_yaml(self) -> None:
        config_path = Path(__file__).resolve().parents[1] / "configs" / "verl-adapter.yaml"
        config = VeRLAdapterConfig.from_file(config_path)
        self.assertEqual(config.task_to_env_type["gsm8k"], "math")
        self.assertEqual(config.default_model_name, "policy-model")


    def test_rust_core_config_loads_from_mapping(self) -> None:
        config = RustCoreClientConfig.from_mapping(
            {
                "core": {
                    "endpoint": "127.0.0.1:55102",
                    "timeout_seconds": 11,
                    "auto_start": True,
                    "binary": "./target/release/uenv-adapter-core",
                }
            }
        )

        self.assertEqual(config.endpoint, "127.0.0.1:55102")
        self.assertEqual(config.timeout_seconds, 11)
        self.assertEqual(config.startup_timeout_seconds, 30)
        self.assertTrue(config.auto_start)
        self.assertEqual(config.binary, "./target/release/uenv-adapter-core")

    def test_rust_core_episode_client_submits_batch_to_stub(self) -> None:
        stub = FakeRustCoreStub(reward=4.0)
        client = RustCoreEpisodeClient(RustCoreClientConfig(), stub=stub)
        adapter = VeRLAdapter(client=client)
        output = adapter.execute_batch(make_batch())

        self.assertEqual(output["batch_id"], "pre-rollout-batch-0001")
        self.assertEqual(len(output["results"]), 2)
        self.assertEqual(output["results"][0]["reward"], 4.0)
        self.assertEqual(output["results"][1]["reward"], 4.0)
        self.assertEqual(stub.last_request["batch_id"], "pre-rollout-batch-0001")
        self.assertEqual(stub.last_request["samples"][0]["framework"], "verl")
        self.assertEqual(stub.last_request["samples"][0]["sample_index"], 0)
        self.assertEqual(json.loads(stub.last_request["samples"][0]["payload_json"])["metadata"]["uid"], "uid-1")

    def test_rust_core_episode_client_requires_stub(self) -> None:
        client = MissingGeneratedStubRustCoreClient(RustCoreClientConfig())
        request = VeRLAdapter().to_episode_requests(make_batch())[0]
        with self.assertRaisesRegex(RuntimeError, "requires an AdapterCoreService stub"):
            client.submit_episode(request)

    def test_rust_core_auto_start_sets_addr_and_closes_process(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            binary = Path(temp_dir) / "fake-core"
            log_path = Path(temp_dir) / "env.log"
            binary.write_text(
                "#!/usr/bin/env python3\n"
                "import os, pathlib, time\n"
                f"pathlib.Path({str(log_path)!r}).write_text(os.environ.get('UENV_ADDR', ''))\n"
                "while True: time.sleep(0.1)\n",
                encoding="utf-8",
            )
            binary.chmod(binary.stat().st_mode | stat.S_IXUSR)

            proc = None
            client = AutoStartTestRustCoreClient(
                RustCoreClientConfig(endpoint="127.0.0.1:59999", auto_start=True, binary=str(binary)),
            )
            try:
                for _ in range(20):
                    if log_path.exists():
                        break
                    time.sleep(0.05)
                self.assertEqual(log_path.read_text(encoding="utf-8"), "127.0.0.1:59999")
                proc = client._process
                self.assertIsNotNone(proc)
                self.assertIsNone(proc.poll())
            finally:
                client.close()
            self.assertIsNotNone(proc)
            if proc is not None:
                self.assertIsNotNone(proc.poll())

    def test_grpc_config_loads_from_mapping(self) -> None:
        config = GrpcEpisodeClientConfig.from_mapping(
            {
                "server": {
                    "endpoint": "dns:///uenv-server:50051",
                    "tls": {"enabled": True},
                    "grpc": {
                        "timeout_seconds": 7,
                        "max_send_message_mb": 16,
                        "max_receive_message_mb": 32,
                        "compression": "gzip",
                    },
                }
            }
        )

        self.assertEqual(config.endpoint, "dns:///uenv-server:50051")
        self.assertEqual(config.timeout_seconds, 7)
        self.assertEqual(config.max_send_message_mb, 16)
        self.assertEqual(config.max_receive_message_mb, 32)
        self.assertEqual(config.compression, "gzip")
        self.assertTrue(config.tls_enabled)

    def test_grpc_episode_client_requires_stub(self) -> None:
        client = GrpcEpisodeClient(GrpcEpisodeClientConfig(endpoint="dns:///uenv-server:50051"))
        request = VeRLAdapter().to_episode_requests(make_batch())[0]
        with self.assertRaisesRegex(RuntimeError, "requires a generated UEnvService stub"):
            client.submit_episode(request)


if __name__ == "__main__":
    unittest.main()
