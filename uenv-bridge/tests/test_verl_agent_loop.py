from __future__ import annotations

import asyncio
import json
import tempfile
import unittest
import unittest.mock
from pathlib import Path

from uenv.bridge.agent_loop_clients import AgentLoopClientConfig
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
        self.stream_calls = []

    def submit_episode(self, request):
        self.last_request = request
        self.result.request_id = request.request_id
        return self.result

    def submit_episode_stream(self, requests):
        request_list = list(requests)
        self.stream_calls.append(request_list)
        for request in request_list:
            yield self.submit_episode(request)


class BatchRecordingEpisodeClient:
    def __init__(self) -> None:
        self.stream_calls = []

    def submit_episode(self, request):
        return next(self.submit_episode_stream([request]))

    def submit_episode_stream(self, requests):
        request_list = list(requests)
        self.stream_calls.append(request_list)
        for index, request in enumerate(request_list):
            yield EpisodeResult(
                request_id=request.request_id,
                status="completed",
                trajectory=Trajectory(
                    steps=[
                        StepRecord(
                            step_index=0,
                            action=f"answer-{index}".encode("utf-8"),
                            reward=float(index + 1),
                            terminated=True,
                            info={
                                "response_ids": json.dumps([200 + index]),
                                "response_mask": "[1]",
                                "response_text": f"answer-{index}",
                            },
                        )
                    ],
                    total_reward=float(index + 1),
                    total_steps=1,
                ),
                summary=EpisodeSummary(total_reward=float(index + 1), total_steps=1, terminate_reason="done"),
            )


class CapacityAwareEpisodeClient(BatchRecordingEpisodeClient):
    def submit_episode_stream(self, requests):
        request_list = list(requests)
        self.stream_calls.append(request_list)
        if len(request_list) > 1:
            for request in request_list:
                yield EpisodeResult(
                    request_id=request.request_id,
                    status="failed",
                    trajectory=Trajectory(steps=[], total_reward=0.0, total_steps=0),
                    summary=EpisodeSummary(total_reward=0.0, total_steps=0, terminate_reason="failed"),
                    error_message="no worker available: all workers at capacity",
                )
            return

        request = request_list[0]
        yield EpisodeResult(
            request_id=request.request_id,
            status="completed",
            trajectory=Trajectory(
                steps=[
                    StepRecord(
                        step_index=0,
                        action=b"ok",
                        reward=1.0,
                        terminated=True,
                        info={"response_ids": "[42]", "response_mask": "[1]", "response_text": "ok"},
                    )
                ],
                total_reward=1.0,
                total_steps=1,
            ),
            summary=EpisodeSummary(total_reward=1.0, total_steps=1, terminate_reason="done"),
        )


class FakeServerManager:
    def __init__(self, addresses):
        self.server_addresses = addresses


class FakeRemoteMethod:
    def __init__(self, value):
        self.value = value

    async def remote(self):
        return self.value


class FakeLoadBalancer:
    def __init__(self, addresses):
        self.get_all_servers = FakeRemoteMethod(addresses)


class FakeLLMServerClient:
    def __init__(self, addresses):
        self._load_balancer = FakeLoadBalancer(addresses)


class AttrDict(dict):
    def __getattr__(self, name):
        try:
            return self[name]
        except KeyError as exc:
            raise AttributeError(name) from exc


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


class FakeCoreBatchStub:
    def __init__(self) -> None:
        self.last_request = None

    def ExecuteBatch(self, request, timeout=None):
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
                    "reward": float(sample["sample_index"]),
                    "done": True,
                    "termination_reason": "done",
                    "trajectory_json": json.dumps(
                        {
                            "steps": [
                                {
                                    "step_index": 0,
                                    "action": str(sample["sample_index"]),
                                    "reward": float(sample["sample_index"]),
                                    "terminated": True,
                                }
                            ],
                            "total_reward": float(sample["sample_index"]),
                            "total_steps": 1,
                        }
                    ).encode("utf-8"),
                    "error_code": "",
                    "error_message": "",
                }
                for sample in request["samples"]
            ],
        }


class FakeCoreStreamStub:
    def __init__(self) -> None:
        self.seen_samples = []

    def ExecuteBatchStream(self, samples, timeout=None):
        for sample in samples:
            self.seen_samples.append(sample)
            yield {
                "request_id": sample["request_id"],
                "batch_id": sample["batch_id"],
                "sample_index": sample["sample_index"],
                "status": "completed",
                "reward": float(sample["sample_index"]) + 10.0,
                "done": True,
                "termination_reason": "stream_done",
                "trajectory_json": json.dumps(
                    {
                        "steps": [
                            {
                                "step_index": 0,
                                "action": f"stream-{sample['sample_index']}",
                                "reward": float(sample["sample_index"]) + 10.0,
                                "terminated": True,
                            }
                        ],
                        "total_reward": float(sample["sample_index"]) + 10.0,
                        "total_steps": 1,
                    }
                ).encode("utf-8"),
                "error_code": "",
                "error_message": "",
            }

    def ExecuteBatch(self, request, timeout=None):
        raise AssertionError("streaming client should not call ExecuteBatch")


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
        self.assertEqual(payload["metadata"]["extra_info"]["question"], "What is 2 + 2?")
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

    def test_run_batch_submits_one_core_batch(self) -> None:
        client = BatchRecordingEpisodeClient()
        loop = UEnvAgentLoop(tokenizer=FakeTokenizer(), client=client)

        outputs = asyncio.run(
            loop.run_batch(
                [{"temperature": 1.0}, {"temperature": 0.5}],
                [
                    {
                        "raw_prompt": [{"role": "user", "content": "1+1?"}],
                        "data_source": "gsm8k",
                        "reward_model": {"ground_truth": "2"},
                    },
                    {
                        "raw_prompt": [{"role": "user", "content": "2+2?"}],
                        "data_source": "gsm8k",
                        "reward_model": {"ground_truth": "4"},
                    },
                ],
                batch_id="batch-test",
            )
        )

        self.assertEqual(len(client.stream_calls), 1)
        self.assertEqual(len(client.stream_calls[0]), 2)
        self.assertEqual([output.response_ids for output in outputs], [[200], [201]])
        self.assertEqual([output.reward_score for output in outputs], [1.0, 2.0])
        payloads = [json.loads(request.payload.decode("utf-8")) for request in client.stream_calls[0]]
        self.assertEqual([payload["metadata"]["batch_id"] for payload in payloads], ["batch-test", "batch-test"])
        self.assertEqual([payload["metadata"]["sample_index"] for payload in payloads], [0, 1])

    def test_run_batch_splits_capacity_failures(self) -> None:
        client = CapacityAwareEpisodeClient()
        loop = UEnvAgentLoop(tokenizer=FakeTokenizer(), client=client, batch_size=4, batch_retry_delay_seconds=0)

        outputs = asyncio.run(
            loop.run_batch(
                [{}, {}, {}],
                [
                    {"raw_prompt": "a", "data_source": "gsm8k"},
                    {"raw_prompt": "b", "data_source": "gsm8k"},
                    {"raw_prompt": "c", "data_source": "gsm8k"},
                ],
                batch_id="batch-capacity",
            )
        )

        self.assertEqual([len(call) for call in client.stream_calls], [3, 1, 2, 1, 1])
        self.assertEqual([output.response_ids for output in outputs], [[42], [42], [42]])

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

    def test_run_concatenates_multi_step_trajectory_for_training_response(self) -> None:
        result = EpisodeResult(
            request_id="result-1",
            status="completed",
            trajectory=Trajectory(
                steps=[
                    StepRecord(
                        step_index=1,
                        action=b"first action",
                        reward=0.25,
                        terminated=False,
                        info={"response_ids": "[11, 12]", "response_mask": "[1, 0]", "response_text": "first"},
                    ),
                    StepRecord(
                        step_index=2,
                        action=b"second action",
                        reward=0.75,
                        terminated=True,
                        info={"response_ids": "[21, 22, 23]", "response_mask": "[1, 1, 0]", "response_text": "second"},
                    ),
                ],
                total_reward=1.0,
                total_steps=2,
            ),
            summary=EpisodeSummary(total_reward=1.0, total_steps=2, terminate_reason="done"),
        )
        loop = UEnvAgentLoop(tokenizer=FakeTokenizer(), client=RecordingEpisodeClient(result))

        output = asyncio.run(loop.run({}, raw_prompt="multi-step task", data_source="agent"))

        self.assertEqual(output.response_ids, [11, 12, 21, 22, 23])
        self.assertEqual(output.response_mask, [1, 0, 1, 1, 0])
        self.assertEqual(output.reward_score, 1.0)
        self.assertEqual(output.num_turns, 3)
        self.assertEqual(output.extra_fields["uenv_trajectory"][0]["action"], "first action")
        self.assertEqual(output.extra_fields["uenv_trajectory"][1]["action"], "second action")

    def test_run_records_episode_result_jsonl(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            record_path = Path(tmpdir) / "results.jsonl"
            client = RecordingEpisodeClient(self._result_with_token_ids())
            loop = UEnvAgentLoop(
                tokenizer=FakeTokenizer(),
                client=client,
                result_record_path=str(record_path),
            )

            asyncio.run(
                loop.run(
                    {},
                    raw_prompt=[{"role": "user", "content": "2+2?"}],
                    data_source="gsm8k",
                    reward_model={"ground_truth": "4"},
                    extra_info={"batch_id": "batch-record", "sample_index": 3},
                )
            )

            records = [json.loads(line) for line in record_path.read_text().splitlines()]
            self.assertEqual(len(records), 1)
            self.assertEqual(records[0]["batch_id"], "batch-record")
            self.assertEqual(records[0]["sample_index"], 3)
            self.assertEqual(records[0]["request_model_endpoint"], "https://openrouter.ai/api/v1")
            self.assertEqual(records[0]["request_model_name"], "qwen/qwen-2.5-7b-instruct")
            self.assertEqual(records[0]["reward"], 2.0)
            self.assertEqual(records[0]["response_text"], "4")
            self.assertEqual(records[0]["response_ids"], [101, 102])
            self.assertEqual(records[0]["verl_response_ids"], [101, 102])
            self.assertEqual(records[0]["verl_response_mask"], [1, 0])
            self.assertEqual(records[0]["trajectory"][0]["reward"], 2.0)

    def test_run_records_episode_request_jsonl_before_submit(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            record_path = Path(tmpdir) / "requests.jsonl"
            client = RecordingEpisodeClient(self._result_with_token_ids())
            loop = UEnvAgentLoop(
                tokenizer=FakeTokenizer(),
                client=client,
                request_record_path=str(record_path),
            )

            asyncio.run(
                loop.run(
                    {"temperature": 0.7},
                    raw_prompt=[{"role": "user", "content": "2+2?"}],
                    data_source="gsm8k",
                    reward_model={"ground_truth": "4"},
                    extra_info={"batch_id": "batch-request", "sample_index": 2},
                )
            )

            records = [json.loads(line) for line in record_path.read_text().splitlines()]
            self.assertEqual(len(records), 1)
            self.assertEqual(records[0]["phase"], "submit_single")
            self.assertEqual(records[0]["batch_id"], "batch-request")
            self.assertEqual(records[0]["sample_index"], 2)
            self.assertEqual(records[0]["prompt_text"], "user: 2+2?")
            self.assertEqual(records[0]["generation_config"]["temperature"], 0.7)
            self.assertEqual(records[0]["payload"]["metadata"]["batch_id"], "batch-request")

    def test_run_prefers_verl_runtime_model_endpoint(self) -> None:
        client = RecordingEpisodeClient(self._result_with_token_ids())
        loop = UEnvAgentLoop(
            tokenizer=FakeTokenizer(),
            client=client,
            server_manager=FakeServerManager(["10.10.20.142:46541"]),
            trainer_config=AttrDict(
                actor_rollout_ref=AttrDict(
                    model=AttrDict(path="/models/modelscope/Qwen/Qwen2___5-0___5B-Instruct"),
                    rollout=AttrDict(
                        prometheus=AttrDict(served_model_name="/models/modelscope/Qwen/Qwen2___5-0___5B-Instruct")
                    ),
                )
            ),
            default_model_endpoint="https://openrouter.ai/api/v1",
            default_model_name="mock-policy",
        )

        asyncio.run(
            loop.run(
                {},
                raw_prompt=[{"role": "user", "content": "2+2?"}],
                data_source="gsm8k",
                reward_model={"ground_truth": "4"},
            )
        )

        payload = json.loads(client.last_request.payload.decode("utf-8"))
        self.assertEqual(client.last_request.model_endpoint, "http://10.10.20.142:46541/v1")
        self.assertEqual(payload["model_endpoint"]["url"], "http://10.10.20.142:46541/v1")
        self.assertEqual(payload["model_endpoint"]["model_name"], "/models/modelscope/Qwen/Qwen2___5-0___5B-Instruct")

    def test_run_discovers_verl_llm_server_client_endpoint(self) -> None:
        client = RecordingEpisodeClient(self._result_with_token_ids())
        loop = UEnvAgentLoop(
            tokenizer=FakeTokenizer(),
            client=client,
            server_manager=FakeLLMServerClient(["10.0.2.100:38285"]),
            default_model_endpoint="https://openrouter.ai/api/v1",
        )

        asyncio.run(
            loop.run(
                {},
                raw_prompt=[{"role": "user", "content": "2+2?"}],
                data_source="gsm8k",
                reward_model={"ground_truth": "4"},
            )
        )

        payload = json.loads(client.last_request.payload.decode("utf-8"))
        self.assertEqual(client.last_request.model_endpoint, "http://10.0.2.100:38285/v1")
        self.assertEqual(payload["model_endpoint"]["url"], "http://10.0.2.100:38285/v1")

    def test_run_uses_model_gateway_for_multiple_verl_runtime_endpoints(self) -> None:
        client = RecordingEpisodeClient(self._result_with_token_ids())
        loop = UEnvAgentLoop(
            tokenizer=FakeTokenizer(),
            client=client,
            server_manager=FakeServerManager(["10.10.20.142:40203", "10.10.20.142:39755"]),
            model_gateway_enabled=True,
            model_gateway_bind_host="127.0.0.1",
            model_gateway_port=0,
            model_gateway_public_url="http://10.10.20.142:18080/v1",
        )
        try:
            asyncio.run(
                loop.run(
                    {},
                    raw_prompt=[{"role": "user", "content": "2+2?"}],
                    data_source="gsm8k",
                    reward_model={"ground_truth": "4"},
                )
            )

            payload = json.loads(client.last_request.payload.decode("utf-8"))
            self.assertEqual(client.last_request.model_endpoint, "http://10.10.20.142:18080/v1")
            self.assertEqual(payload["model_endpoint"]["url"], "http://10.10.20.142:18080/v1")
            self.assertEqual(
                payload["metadata"]["model_gateway_upstreams"],
                ["http://10.10.20.142:40203/v1", "http://10.10.20.142:39755/v1"],
            )
            self.assertEqual(loop.model_gateway.upstreams, ["http://10.10.20.142:40203/v1", "http://10.10.20.142:39755/v1"])
        finally:
            loop.close()

    def test_run_keeps_explicit_model_endpoint_override(self) -> None:
        client = RecordingEpisodeClient(self._result_with_token_ids())
        loop = UEnvAgentLoop(
            tokenizer=FakeTokenizer(),
            client=client,
            server_manager=FakeServerManager(["10.10.20.142:46541"]),
            default_model_endpoint="https://openrouter.ai/api/v1",
        )

        asyncio.run(
            loop.run(
                {},
                raw_prompt=[{"role": "user", "content": "2+2?"}],
                data_source="gsm8k",
                reward_model={"ground_truth": "4"},
                extra_info={"model_endpoint": "http://manual.example/v1", "model_name": "manual-model"},
            )
        )

        payload = json.loads(client.last_request.payload.decode("utf-8"))
        self.assertEqual(client.last_request.model_endpoint, "http://manual.example/v1")
        self.assertEqual(payload["model_endpoint"]["url"], "http://manual.example/v1")
        self.assertEqual(payload["model_endpoint"]["model_name"], "manual-model")

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

    def test_rust_core_client_sends_multiple_samples_in_one_execute_batch(self) -> None:
        stub = FakeCoreBatchStub()
        client = RustCoreEpisodeClient(RustCoreClientConfig(), stub=stub)
        loop = UEnvAgentLoop(tokenizer=FakeTokenizer(), client=client)
        requests = [
            loop.build_episode_request(
                sampling_params={},
                prompt_ids=[10],
                raw_prompt=f"question-{index}",
                sample_kwargs={"extra_info": {"batch_id": "batch-core", "sample_index": index}},
            )
            for index in range(3)
        ]

        results = list(client.submit_episode_stream(requests))

        self.assertEqual(len(stub.last_request["samples"]), 3)
        self.assertEqual([sample["sample_index"] for sample in stub.last_request["samples"]], [0, 1, 2])
        self.assertEqual([result.request_id for result in results], [request.request_id for request in requests])
        self.assertEqual([result.summary.total_reward for result in results], [0.0, 1.0, 2.0])

    def test_rust_core_client_can_use_execute_batch_stream(self) -> None:
        stub = FakeCoreStreamStub()
        client = RustCoreEpisodeClient(RustCoreClientConfig(streaming=True), stub=stub)
        loop = UEnvAgentLoop(tokenizer=FakeTokenizer(), client=client)
        requests = [
            loop.build_episode_request(
                sampling_params={},
                prompt_ids=[10],
                raw_prompt=f"question-{index}",
                sample_kwargs={"extra_info": {"batch_id": "batch-stream", "sample_index": index}},
            )
            for index in range(3)
        ]

        results = list(client.submit_episode_stream(requests))

        self.assertEqual([sample["sample_index"] for sample in stub.seen_samples], [0, 1, 2])
        self.assertEqual([result.request_id for result in results], [request.request_id for request in requests])
        self.assertEqual([result.summary.total_reward for result in results], [10.0, 11.0, 12.0])

    def test_agent_loop_client_config_reads_streaming_env(self) -> None:
        with unittest.mock.patch.dict("os.environ", {"UENV_ADAPTER_CORE_STREAMING": "1"}):
            config = AgentLoopClientConfig.from_env()

        self.assertTrue(config.streaming)

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
