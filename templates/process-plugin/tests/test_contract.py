#!/usr/bin/env python3
"""Exercise the complete UEnv process-plugin contract over a real UDS."""

from __future__ import annotations

import json
import socket
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import grpc

PLUGIN_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(PLUGIN_DIR / "generated"))

import plugin_pb2  # noqa: E402
import plugin_pb2_grpc  # noqa: E402
sys.path.insert(0, str(PLUGIN_DIR))
from environment import SELF_TEST_CASE  # noqa: E402


def observation_bytes(value) -> bytes:
    if isinstance(value, bytes):
        return value
    if isinstance(value, str):
        return value.encode("utf-8")
    return json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode("utf-8")


def require_uds_support() -> None:
    try:
        probe = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    except PermissionError as exc:
        raise unittest.SkipTest(f"sandbox does not permit Unix sockets: {exc}") from exc
    else:
        probe.close()


class ProcessPluginContractTest(unittest.TestCase):
    def test_health_reset_step_and_close(self) -> None:
        require_uds_support()
        with tempfile.TemporaryDirectory(prefix="uenv-plugin-contract-") as temp_dir:
            uds_path = Path(temp_dir) / "example.sock"
            sidecar_path = Path(f"{uds_path}.episode.json")
            original_payload = {
                "env_config": SELF_TEST_CASE["config"],
                "metadata": {"custom": [1, 2, 3]},
            }
            original_reward = {"type": "plugin", "weights": {"success": 1.0}}
            sidecar_path.write_text(
                json.dumps(
                    {
                        **SELF_TEST_CASE["config"],
                        "_uenv": {
                            "sidecar_schema_version": 1,
                            "payload": original_payload,
                            "reward_config": original_reward,
                            "seed": SELF_TEST_CASE["seed"],
                        },
                    }
                ),
                encoding="utf-8",
            )
            process = subprocess.Popen(
                [sys.executable, str(PLUGIN_DIR / "plugin.py"), "--uds-path", str(uds_path)]
            )
            channel = grpc.insecure_channel(f"unix:{uds_path}")
            try:
                grpc.channel_ready_future(channel).result(timeout=10)
                client = plugin_pb2_grpc.PluginServiceStub(channel)

                health = client.HealthCheck(plugin_pb2.HealthCheckRequest(), timeout=5)
                self.assertTrue(health.ok)

                reset = client.Reset(
                    plugin_pb2.ResetRequest(seed=SELF_TEST_CASE["seed"]), timeout=5
                )
                self.assertEqual(
                    reset.observation,
                    observation_bytes(SELF_TEST_CASE["reset_observation"]),
                )
                self.assertEqual(reset.info["sidecar_schema_version"], "1")
                self.assertEqual(reset.info["seed"], str(SELF_TEST_CASE["seed"]))

                step = client.Step(
                    plugin_pb2.StepRequest(
                        action=SELF_TEST_CASE["action"].encode("utf-8")
                    ),
                    timeout=5,
                )
                self.assertEqual(step.reward, SELF_TEST_CASE["reward"])
                self.assertEqual(step.terminated, SELF_TEST_CASE["terminated"])
                self.assertEqual(step.truncated, SELF_TEST_CASE["truncated"])

                closed = client.Close(plugin_pb2.CloseRequest(), timeout=5)
                self.assertTrue(closed.ok)
                process.wait(timeout=10)
                self.assertEqual(process.returncode, 0)
            finally:
                channel.close()
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=10)


if __name__ == "__main__":
    unittest.main()
