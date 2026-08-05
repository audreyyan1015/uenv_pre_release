from __future__ import annotations

import json
from pathlib import Path
import tempfile
import unittest

from uenv.bridge.clients import RustCoreClientConfig, RustCoreEpisodeClient
from uenv.bridge.evaluate import build_request, load_cases, result_record
from uenv.bridge.protocol import EpisodeResult, EpisodeSummary, StepRecord, Trajectory


class EvaluateTests(unittest.TestCase):
    def test_load_cases_ignores_comments_and_honors_limit(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "cases.jsonl"
            path.write_text('# note\n{"id":"a","target":"1"}\n{"id":"b","target":"2"}\n')
            cases = load_cases(path, limit=1)
        self.assertEqual([case["id"] for case in cases], ["a"])

    def test_build_request_allows_explicit_custom_environment(self) -> None:
        request = build_request(
            {
                "id": "one",
                "env_type": "browser",
                "dataset": "browserbench",
                "question": "open the page",
                "target": "done",
                "env_config": {"start_url": "https://example.invalid"},
            },
            index=0,
            batch_id="batch",
            default_env_type="browser",
            default_dataset="browserbench",
            model_endpoint="http://127.0.0.1:8000/v1",
            model_name="model",
            max_tokens=64,
            temperature=0.0,
            top_p=1.0,
            max_steps=2,
            timeout_seconds=60,
            seed=42,
        )
        payload = json.loads(request.payload)
        self.assertEqual(request.env_type, "browser")
        self.assertEqual(payload["env_config"]["start_url"], "https://example.invalid")
        self.assertEqual(payload["reward_config"]["target"], "done")

        # The Python client must place the complete environment object in the
        # typed SampleEnvelope consumed by Adapter Core; otherwise arbitrary
        # plugin fields would disappear before reaching Worker.
        client = RustCoreEpisodeClient(RustCoreClientConfig(), stub=object())
        core_request = client._to_core_execute_batch_request([request])
        envelope_env_config = json.loads(
            core_request["samples"][0]["env_config_json"].decode("utf-8")
        )
        self.assertEqual(envelope_env_config, payload["env_config"])

    def test_result_record_is_json_serializable(self) -> None:
        request = build_request(
            {"id": "one", "question": "1+1", "target": "2"},
            index=0,
            batch_id="batch",
            default_env_type="qa",
            default_dataset="gsm8k",
            model_endpoint="",
            model_name="",
            max_tokens=64,
            temperature=0.0,
            top_p=1.0,
            max_steps=1,
            timeout_seconds=60,
            seed=42,
        )
        result = EpisodeResult(
            request_id=request.request_id,
            status="completed",
            trajectory=Trajectory(
                steps=[StepRecord(step_index=1, action=b"#### 2", reward=1.0, terminated=True)],
                total_reward=1.0,
                total_steps=1,
            ),
            summary=EpisodeSummary(total_reward=1.0, total_steps=1, terminate_reason="terminated"),
        )
        record = result_record({"id": "one"}, request, result)
        encoded = json.dumps(record)
        self.assertIn("#### 2", encoded)


if __name__ == "__main__":
    unittest.main()
