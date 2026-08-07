from __future__ import annotations

import importlib.util
import json
import os
from pathlib import Path
import threading
import unittest
import urllib.error
import urllib.request
from http.server import ThreadingHTTPServer


ROOT = Path(__file__).resolve().parents[1]


class OpenHandsRunnerHealthTest(unittest.TestCase):
    def _load_runner(self):
        path = ROOT / "scripts/openhands/openhands_runner.py"
        spec = importlib.util.spec_from_file_location("test_openhands_runner", path)
        assert spec and spec.loader
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        return module

    def test_poll_health_waits_for_remote_server_registration(self) -> None:
        previous_poll = os.environ.get("OPENHANDS_AGENT_POLL")
        previous_server = os.environ.get("UENV_SERVER_ENDPOINT")
        os.environ["OPENHANDS_AGENT_POLL"] = "1"
        os.environ["UENV_SERVER_ENDPOINT"] = "10.0.0.10:50051"
        try:
            runner = self._load_runner()
        finally:
            if previous_poll is None:
                os.environ.pop("OPENHANDS_AGENT_POLL", None)
            else:
                os.environ["OPENHANDS_AGENT_POLL"] = previous_poll
            if previous_server is None:
                os.environ.pop("UENV_SERVER_ENDPOINT", None)
            else:
                os.environ["UENV_SERVER_ENDPOINT"] = previous_server

        server = ThreadingHTTPServer(("127.0.0.1", 0), runner.HealthHandler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        url = f"http://127.0.0.1:{server.server_port}/health"
        try:
            with self.assertRaises(urllib.error.HTTPError) as not_ready:
                urllib.request.urlopen(url, timeout=2)
            self.assertEqual(not_ready.exception.code, 503)
            initial = json.loads(not_ready.exception.read())
            self.assertFalse(initial["registered"])
            self.assertEqual(initial["server_endpoint"], "10.0.0.10:50051")

            with runner._registration_lock:
                runner._agent_state.update(
                    {"registered": True, "agent_id": "agent-1", "last_error": ""}
                )
            with urllib.request.urlopen(url, timeout=2) as response:
                ready = json.load(response)
            self.assertEqual(response.status, 200)
            self.assertTrue(ready["registered"])
            self.assertEqual(ready["agent_id"], "agent-1")
            self.assertEqual(ready["server_endpoint"], "10.0.0.10:50051")
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)


if __name__ == "__main__":
    unittest.main()
