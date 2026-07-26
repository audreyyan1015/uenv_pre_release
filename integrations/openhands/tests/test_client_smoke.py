"""Smoke tests for the UEnv OpenHands integration.

Two tiers:
  * offline unit tests — adapter wiring / duck-typing, no network (always run).
  * live gateway test — skipped unless ``UENV_GATEWAY`` points at a reachable
    Worker gateway and ``UENV_GATEWAY_INSTANCE`` names a catalog instance.

Run:
  python3 -m pytest integrations/openhands/tests -q
  # live:
  UENV_GATEWAY=127.0.0.1:48999 UENV_GATEWAY_INSTANCE=scikit-learn__scikit-learn-14141 \
      python3 -m pytest integrations/openhands/tests -q -k live
"""

import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(__file__)))
from uenv_runtime import UEnvGatewayClient, UEnvRuntime  # noqa: E402
from uenv_runtime.llm_rollout import RolloutTraceCollector  # noqa: E402
from uenv_runtime.runtime import _attr  # noqa: E402


class FakeCmd:
    def __init__(self, command):
        self.command = command


class FakeWrite:
    def __init__(self, path, content):
        self.path = path
        self.content = content


class FakeLogprobRecord:
    def __init__(self, token, logprob):
        self.token = token
        self.logprob = logprob


class FakeChoice:
    def __init__(self):
        self.logprobs = {"content": [
            FakeLogprobRecord("OK", -0.1),
            FakeLogprobRecord(".", -0.2),
        ]}


class FakeRawResponse:
    id = "chatcmpl-test"
    choices = [FakeChoice()]


class FakeResponse:
    raw_response = FakeRawResponse()


class OfflineRolloutCollector(RolloutTraceCollector):
    def __init__(self):
        self._api_key = "test"
        self._base_url = "http://127.0.0.1"
        self.model = "volcengine/doubao-test"
        self._provider_model = "doubao-test"
        self._responses = []

    def _tokenize(self, texts):
        self.assert_texts = texts
        return [[101, 102]]


class OfflineAdapterTests(unittest.TestCase):
    def test_attr_reads_objects_and_dicts(self):
        self.assertEqual(_attr(FakeCmd("ls"), "command"), "ls")
        self.assertEqual(_attr({"path": "/a"}, "path", "filepath"), "/a")
        self.assertEqual(_attr({"filepath": "/b"}, "path", "filepath"), "/b")
        self.assertIsNone(_attr(object(), "nope"))

    def test_base_url_normalization(self):
        self.assertEqual(UEnvGatewayClient("127.0.0.1:48999").base_url, "http://127.0.0.1:48999")
        self.assertEqual(UEnvGatewayClient("http://h:1/").base_url, "http://h:1")

    def test_run_action_dispatch_by_classname(self):
        # No session is created; we only verify dispatch routing via monkeypatch.
        rt = UEnvRuntime("127.0.0.1:1", instance_id="x")
        calls = []
        rt.run = lambda a: calls.append(("run", a)) or {"exit_code": 0}
        rt.read = lambda a: calls.append(("read", a))
        rt.write = lambda a: calls.append(("write", a))
        rt.run_action(FakeCmd("ls"))
        rt.run_action(FakeWrite("/p", "c"))
        self.assertEqual([c[0] for c in calls], ["run", "write"])

    def test_rollout_collector_exports_replay_turns(self):
        collector = OfflineRolloutCollector()
        collector.record(FakeResponse())
        document = collector.finalize()
        self.assertEqual(document["source_model"], "volcengine/doubao-test")
        self.assertEqual(document["turns"][0]["assistant_output"], "OK.")
        self.assertEqual(document["turns"][0]["response_ids"], [101, 102])
        self.assertEqual(document["turns"][0]["logprobs"], [-0.1, -0.2])
        self.assertEqual(document["rollout_trace"]["response_ids"], [101, 102])


class LiveGatewayTest(unittest.TestCase):
    def test_live_reward(self):
        base = os.environ.get("UENV_GATEWAY")
        instance = os.environ.get("UENV_GATEWAY_INSTANCE")
        if not base or not instance:
            self.skipTest("set UENV_GATEWAY and UENV_GATEWAY_INSTANCE for live test")
        client = UEnvGatewayClient(base)
        self.assertTrue(client.health(), "gateway /health not reachable")


if __name__ == "__main__":
    unittest.main()
