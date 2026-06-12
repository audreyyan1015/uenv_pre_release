from __future__ import annotations

import asyncio
import json
import unittest

from uenv.bridge.clients import RustCoreClientConfig, RustCoreEpisodeClient
from uenv.bridge.protocol import EpisodeResult, EpisodeSummary, StepRecord, Trajectory
from uenv.bridge.verl_agent_loop import UEnvAgentLoop


class FakeTokenizer:
    pad_token_id = 0

    def apply_chat_template(self, messages, tokenize=True, add_generation_prompt=True):
        self.last_messages = messages
        return [10, 11, 12]

    def encode(self, text, add_special_tokens=False):
        return [ord(char) for char in text]


class RecordingEpisodeClient:
    def __init__(self, result: EpisodeResult) -> None:
        self.result = result
        self.last_request = None

    def submit_episode(self, request):
        self.last_request = request
        self.result.request_id = request.request_id
        return self.result

    def submit_episode_stream(self, requests):
        for request in requests:
            yield self.submit_episode(request)


class FakeCoreTrajectoryStub:
    def __init__(self) -> None:
        self.last_request = None

    def ExecuteBatch(self, request, timeout=None):
        self.last_request = request
        sample = request["samples"][0]
        trajectory = {
            "steps": [
                {
                    "step_index": 0,
                    "action": "42",
                    "reward": 0.75,
                    "terminated": True,
                    "info": {
                        "response_ids": [101, 102],
                        "response_mask": [1, 1],
                        "finish_reason": "done",
                    },
                }
            ],
            "total_reward": 0.75,
            "total_steps": 1,
        }
        return {
            "request_id": request["request_id"],
            "batch_id": request["batch_id"],
            "results": [
                {
                    "request_id": sample["request_id"],
                    "batch_id": sample["batch_id"],
                    "sample_index": sample["sample_index"],
                    "status": "completed",
                    "reward": 0.75,
                    "done": True,
                    "termination_reason": "done",
                    "trajectory_json": json.dumps(trajectory).encode("utf-8"),
                    "error_code": "",
                    "error_message": "",
                }
            ],
        }


class UEnvAgentLoopTest(unittest.TestCase):
    def test_build_episode_request_uses_prd_episode_shape(self) -> None:
        loop = UEnvAgentLoop(
            tokenizer=FakeTokenizer(),
            client=RecordingEpisodeClient(self._result_with_token_ids()),
            default_model_endpoint="http://policy.example/v1",
            default_max_steps=7,
        )

        request = loop.build_episode_request(
            sampling_params={"temperature": 0.7, "top_p": 0.9},
            prompt_ids=[10, 11],
            raw_prompt=[{"role": "user", "content": "What is 2 + 2?"}],
            sample_kwargs={
                "data_source": "openai/gsm8k",
                "index": 5,
                "reward_model": {"ground_truth": "4"},
                "extra_info": {"batch_id": "batch-a", "global_steps": 3, "rollout_n": 1},
            },
        )

        payload = json.loads(request.payload.decode("utf-8"))
        self.assertEqual(request.env_type, "math")
        self.assertEqual(request.max_steps, 7)
        self.assertEqual(request.model_endpoint, "http://policy.example/v1")
        self.assertEqual(payload["framework"], "verl")
        self.assertEqual(payload["correlation_id"], "batch-a-5")
        self.assertEqual(payload["env_config"]["raw_prompt"], "user: What is 2 + 2?")
        self.assertEqual(payload["episode_config"]["initial_observation"]["prompt_ids"], [10, 11])
        self.assertEqual(payload["reward_config"]["rubric_config"]["ground_truth"], "4")
        self.assertEqual(payload["metadata"]["sample_index"], 5)
        self.assertIn("response_ids", payload["metadata"]["required_result_fields"])

    def test_run_returns_agent_loop_output_from_episode_result(self) -> None:
        client = RecordingEpisodeClient(self._result_with_token_ids())
        loop = UEnvAgentLoop(tokenizer=FakeTokenizer(), client=client)

        output = asyncio.run(
            loop.run(
                {"temperature": 1.0},
                raw_prompt=[{"role": "user", "content": "2+2?"}],
                data_source="openai/gsm8k",
                reward_model={"ground_truth": "4"},
            )
        )

        self.assertEqual(output.prompt_ids, [10, 11, 12])
        self.assertEqual(output.response_ids, [101, 102])
        self.assertEqual(output.response_mask, [1, 0])
        self.assertEqual(output.reward_score, 2.0)
        self.assertEqual(output.extra_fields["uenv_status"], "completed")
        self.assertIsNotNone(client.last_request)

    def test_run_can_fall_back_to_action_text(self) -> None:
        result = EpisodeResult(
            request_id="result-1",
            status="completed",
            trajectory=Trajectory(
                steps=[StepRecord(step_index=0, action=b"4", reward=0.5, terminated=True)],
                total_reward=0.5,
                total_steps=1,
            ),
            summary=EpisodeSummary(total_reward=0.5, total_steps=1, terminate_reason="done"),
        )
        loop = UEnvAgentLoop(tokenizer=FakeTokenizer(), client=RecordingEpisodeClient(result))

        output = asyncio.run(loop.run({}, raw_prompt="2+2?", data_source="gsm8k"))

        self.assertEqual(output.response_ids, [52])
        self.assertEqual(output.response_mask, [1])
        self.assertEqual(output.reward_score, 0.5)

    def test_rust_core_client_preserves_trajectory_json(self) -> None:
        client = RustCoreEpisodeClient(RustCoreClientConfig(), stub=FakeCoreTrajectoryStub())
        loop = UEnvAgentLoop(tokenizer=FakeTokenizer(), client=client)
        request = loop.build_episode_request(
            sampling_params={},
            prompt_ids=[10],
            raw_prompt="question",
            sample_kwargs={"extra_info": {"batch_id": "batch-a"}},
        )

        result = client.submit_episode(request)

        self.assertEqual(result.summary.total_reward, 0.75)
        self.assertEqual(result.trajectory.steps[0].action, b"42")
        self.assertEqual(result.trajectory.steps[0].info["response_ids"], "[101,102]")

    def _result_with_token_ids(self) -> EpisodeResult:
        step = StepRecord(
            step_index=0,
            action=b"ignored when response_ids are present",
            reward=2.0,
            terminated=True,
            info={
                "response_ids": "[101, 102]",
                "response_mask": "[1, 0]",
                "response_text": "4",
                "finish_reason": "done",
            },
        )
        return EpisodeResult(
            request_id="result-1",
            status="completed",
            trajectory=Trajectory(steps=[step], total_reward=2.0, total_steps=1),
            summary=EpisodeSummary(total_reward=2.0, total_steps=1, terminate_reason="done"),
        )


if __name__ == "__main__":
    unittest.main()
