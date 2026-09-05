from __future__ import annotations

import asyncio
import json
import subprocess
import tempfile
import unittest
from pathlib import Path

import grpc

from uenv.bridge.native_agent_control_server import NativeAgentControlServer, build_agent_job_proto
from uenv.bridge.native_swe_agent_loop import NativeSweAgentLoop
from uenv.v1 import agent_pb2, agent_pb2_grpc, episode_pb2


class FakeTokenizer:
    pad_token_id = 0

    def apply_chat_template(self, messages, tokenize=True, add_generation_prompt=True, **_kwargs):
        return [10, 11, 12]

    def encode(self, text, add_special_tokens=False):
        return [ord(char) for char in text]


class MockDriverNativeSweAgentLoop(NativeSweAgentLoop):
    def _run_driver_subprocess(self, agent_job_path: Path, output_dir: Path) -> subprocess.CompletedProcess[str]:
        job = json.loads(agent_job_path.read_text(encoding="utf-8"))
        (output_dir / "seen_agent_job.json").write_text(json.dumps(job, indent=2) + "\n", encoding="utf-8")
        trace = {
            "turns": [
                {
                    "turn_index": 0,
                    "assistant_output": "patched answer",
                    "response_ids": [101, 102],
                    "logprobs": [-0.1, -0.2],
                    "finish_reason": "stop",
                }
            ],
            "rollout_trace": {
                "response_ids": [101, 102],
                "response_mask": [1, 1],
            },
            "rollout_log_probs": [-0.1, -0.2],
            "rollout_param_version": 7,
            "rollout_policy_version": "actor-step-7",
        }
        submit = {
            "instance_id": job["instance_id"],
            "resolved": True,
            "reward": 1.0,
            "tests_passed": 3,
            "tests_total": 3,
            "trajectory_ref": {"trajectory_id": "traj-native-1"},
            **trace,
        }
        bundle = {
            "artifact": {"git_diff": "diff --git a/pkg.py b/pkg.py\n+fix\n"},
            "steps": [],
        }
        (output_dir / "submit_result.json").write_text(json.dumps(submit) + "\n", encoding="utf-8")
        (output_dir / "llm_rollout_trace.json").write_text(json.dumps(trace) + "\n", encoding="utf-8")
        (output_dir / "trajectory_bundle.json").write_text(json.dumps(bundle) + "\n", encoding="utf-8")
        return subprocess.CompletedProcess(args=["mock-driver"], returncode=0, stdout="ok", stderr="")


class NativeSweAgentLoopTest(unittest.TestCase):
    def test_agent_control_grpc_register_poll_complete_roundtrip(self) -> None:
        server = NativeAgentControlServer(host="127.0.0.1", port=0)
        server.start()
        channel = grpc.insecure_channel(f"127.0.0.1:{server.port}")
        try:
            stub = agent_pb2_grpc.AgentControlServiceStub(channel)
            register = stub.RegisterAgent(
                agent_pb2.RegisterAgentRequest(
                    agent_id="agent-test",
                    agent_pool_id="openhands-default",
                    max_concurrent_jobs=1,
                ),
                timeout=1.0,
            )
            self.assertTrue(register.accepted)

            pending = server.enqueue(
                build_agent_job_proto(
                    {
                        "job_id": "job-1",
                        "run_id": "run-1",
                        "gateway_url": "http://runtime-gateway.example",
                        "instance_id": "repo__issue.native",
                        "benchmark_variant": "smith",
                        "model_endpoint": "http://policy.example/v1",
                        "model_name": "policy-model",
                        "max_iterations": 5,
                    }
                )
            )
            polled = stub.PollAgentJob(
                agent_pb2.PollAgentJobRequest(
                    agent_pool_id="openhands-default",
                    worker_id="agent-test",
                ),
                timeout=1.0,
            )
            self.assertTrue(polled.has_job)
            self.assertEqual(polled.job.job_id, "job-1")
            self.assertEqual(polled.job.model_endpoint_config.url, "http://policy.example/v1")

            ack = stub.CompleteAgentJob(
                agent_pb2.AgentJobCompleteRequest(
                    job_id="job-1",
                    run_id="run-1",
                    status="completed",
                    reward=1.0,
                    trajectory_id="traj-1",
                    agent_id="agent-test",
                    rollout_log_probs=[-0.1, -0.2],
                    metadata={"resolved": "True", "tests_passed": "3", "tests_total": "3"},
                    rollout_trace=episode_pb2.RolloutTrace(response_ids=[101, 102], response_mask=[1, 1]),
                ),
                timeout=1.0,
            )
            self.assertTrue(ack.ack)

            result = server.wait_result(pending, timeout_seconds=1.0)
            self.assertEqual(result.status, "completed")
            self.assertEqual(result.reward, 1.0)
            self.assertEqual(result.response_ids, [101, 102])
            self.assertEqual([round(item, 3) for item in result.rollout_log_probs], [-0.1, -0.2])
            self.assertEqual(result.metadata["resolved"], "True")

            unknown = stub.CompleteAgentJob(
                agent_pb2.AgentJobCompleteRequest(
                    job_id="missing-job",
                    run_id="run-1",
                    status="failed",
                    reward=0.0,
                    agent_id="agent-test",
                ),
                timeout=1.0,
            )
            self.assertFalse(unknown.ack)
            self.assertEqual(unknown.code, "UNKNOWN_JOB")
        finally:
            channel.close()
            server.stop(0)

    def test_run_converts_openhands_driver_artifacts_to_agent_loop_output(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            loop = MockDriverNativeSweAgentLoop(
                tokenizer=FakeTokenizer(),
                driver_path="/tmp/mock-driver.py",
                output_root=tmpdir,
                runtime_gateway_url="http://runtime-gateway.example",
                llm_config_path="/tmp/openhands-llm.json",
                default_model_endpoint="http://policy.example/v1",
                default_model_name="policy-model",
            )

            output = asyncio.run(
                loop.run(
                    {"temperature": 1.0},
                    raw_prompt=[{"role": "user", "content": "fix bug"}],
                    data_source="swesmith",
                    extra_info={
                        "batch_id": "batch-native",
                        "sample_index": 0,
                        "instance_id": "repo__issue.native",
                        "benchmark_variant": "smith",
                        "max_steps": 5,
                    },
                )
            )

            self.assertEqual(output.prompt_ids, [10, 11, 12])
            self.assertEqual(output.response_ids, [101, 102])
            self.assertEqual(output.response_mask, [1, 1])
            self.assertEqual(output.response_logprobs, [-0.1, -0.2])
            self.assertEqual(output.reward_score, 1.0)
            self.assertEqual(output.extra_fields["uenv_trajectory_id"], "traj-native-1")
            self.assertEqual(output.extra_fields["global_steps"], 7)

            jobs = list(Path(tmpdir).glob("**/seen_agent_job.json"))
            self.assertEqual(len(jobs), 1)
            job = json.loads(jobs[0].read_text(encoding="utf-8"))
            self.assertEqual(job["agent_bridge_id"], "native-swe-agent-loop")
            self.assertEqual(job["gateway_url"], "http://runtime-gateway.example")
            self.assertEqual(job["model_endpoint"], "http://policy.example/v1")
            self.assertEqual(job["model_name"], "policy-model")
            self.assertEqual(job["max_iterations"], 5)

    def test_ssh_backend_writes_remote_llm_config_path_into_agent_job(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            loop = MockDriverNativeSweAgentLoop(
                tokenizer=FakeTokenizer(),
                driver_path="/tmp/mock-driver.py",
                output_root=tmpdir,
                execution_backend="ssh",
                runtime_gateway_url="http://runtime-gateway.example",
                llm_config_path="/local/openhands-llm.json",
                remote_llm_config_path="/remote/openhands-llm.json",
                default_model_endpoint="http://policy.example/v1",
                default_model_name="policy-model",
            )

            asyncio.run(
                loop.run(
                    {"temperature": 1.0},
                    raw_prompt=[{"role": "user", "content": "fix bug"}],
                    data_source="swesmith",
                    extra_info={
                        "batch_id": "batch-native",
                        "sample_index": 0,
                        "instance_id": "repo__issue.native",
                        "benchmark_variant": "smith",
                        "max_steps": 5,
                    },
                )
            )

            jobs = list(Path(tmpdir).glob("**/seen_agent_job.json"))
            self.assertEqual(len(jobs), 1)
            job = json.loads(jobs[0].read_text(encoding="utf-8"))
            self.assertEqual(job["llm_config_path"], "/remote/openhands-llm.json")

    def test_ssh_base_cmd_supports_password_without_leaking_value(self) -> None:
        loop = NativeSweAgentLoop(
            tokenizer=FakeTokenizer(),
            execution_backend="ssh",
            remote_host="agent.example",
            remote_user="root",
            remote_port=2222,
            remote_password="secret",
        )

        cmd = loop._ssh_base_cmd()
        self.assertEqual(cmd[:3], ["sshpass", "-e", "ssh"])
        self.assertIn("-p", cmd)
        self.assertIn("2222", cmd)
        self.assertEqual(cmd[-1], "root@agent.example")
        self.assertNotIn("secret", cmd)


if __name__ == "__main__":
    unittest.main()
