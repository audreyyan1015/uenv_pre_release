from __future__ import annotations

import json
import tempfile
import threading
import time
import unittest
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from uenv.bridge.model_gateway import ModelGateway, ModelGatewayConfig


class MockOpenAIServer:
    def __init__(
        self,
        name: str,
        model_version: dict | None = None,
        response_model_version: dict | None = None,
        response_model_version_headers: dict[str, str] | None = None,
    ) -> None:
        self.name = name
        self.model_version = model_version
        self.model_version_requests = 0
        self.response_model_version = response_model_version
        self.response_model_version_headers = response_model_version_headers or {}
        self.requests = []
        parent = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def do_GET(self) -> None:  # noqa: N802
                if self.path == "/uenv/model_version":
                    parent.model_version_requests += 1
                    self._reply(parent.model_version or {"rollout_param_version": 0, "rollout_policy_version": "actor-step-0"})
                    return
                self._reply({"object": "list", "data": [{"id": parent.name}]})

            def do_POST(self) -> None:  # noqa: N802
                body = self.rfile.read(int(self.headers.get("Content-Length", "0") or "0"))
                parent.requests.append({"path": self.path, "body": body.decode("utf-8")})
                payload = {"upstream": parent.name, "choices": [{"message": {"content": parent.name}}]}
                if parent.response_model_version is not None:
                    payload["uenv_model_version"] = parent.response_model_version
                self._reply(payload, headers=parent.response_model_version_headers)

            def log_message(self, _format: str, *_args) -> None:
                return

            def _reply(self, payload, headers: dict[str, str] | None = None) -> None:
                body = json.dumps(payload).encode("utf-8")
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                for key, value in (headers or {}).items():
                    self.send_header(key, value)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    @property
    def url(self) -> str:
        return f"http://127.0.0.1:{self.server.server_port}/v1"

    def close(self) -> None:
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)


class ModelGatewayTest(unittest.TestCase):
    def test_round_robin_forwards_openai_requests(self) -> None:
        upstream_a = MockOpenAIServer("upstream-a")
        upstream_b = MockOpenAIServer("upstream-b")
        with tempfile.TemporaryDirectory() as tmpdir:
            gateway = ModelGateway(
                ModelGatewayConfig(
                    enabled=True,
                    bind_host="127.0.0.1",
                    port=0,
                    log_path=str(Path(tmpdir) / "gateway.jsonl"),
                )
            )
            try:
                gateway_url = gateway.start([upstream_a.url, upstream_b.url])
                seen = []
                for _ in range(4):
                    request = urllib.request.Request(
                        f"{gateway_url}/chat/completions",
                        data=b'{"model":"policy","messages":[]}',
                        headers={"Content-Type": "application/json"},
                        method="POST",
                    )
                    with urllib.request.urlopen(request, timeout=5) as response:
                        seen.append(json.loads(response.read())["upstream"])

                self.assertEqual(seen, ["upstream-a", "upstream-b", "upstream-a", "upstream-b"])
                self.assertEqual(len(upstream_a.requests), 2)
                self.assertEqual(len(upstream_b.requests), 2)
                log_path = Path(tmpdir, "gateway.jsonl")
                records = []
                for _ in range(20):
                    records = [json.loads(line) for line in log_path.read_text().splitlines()]
                    if len(records) == 4:
                        break
                    time.sleep(0.05)
                self.assertEqual([record["upstream_index"] for record in records], [0, 1, 0, 1])
            finally:
                gateway.stop()
                upstream_a.close()
                upstream_b.close()

    def test_disable_thinking_injects_qwen_chat_template_kwargs(self) -> None:
        upstream = MockOpenAIServer("upstream")
        gateway = ModelGateway(
            ModelGatewayConfig(
                enabled=True,
                bind_host="127.0.0.1",
                port=0,
                disable_thinking=True,
            )
        )
        try:
            gateway_url = gateway.start([upstream.url])
            request = urllib.request.Request(
                f"{gateway_url}/chat/completions",
                data=json.dumps(
                    {
                        "model": "policy",
                        "messages": [{"role": "user", "content": "hello"}],
                        "chat_template_kwargs": {"foo": "bar", "enable_thinking": True},
                    }
                ).encode("utf-8"),
                headers={"Content-Type": "application/json"},
                method="POST",
            )
            with urllib.request.urlopen(request, timeout=5) as response:
                json.loads(response.read())

            forwarded = json.loads(upstream.requests[0]["body"])
            self.assertEqual(
                forwarded["chat_template_kwargs"],
                {"foo": "bar", "enable_thinking": False},
            )
        finally:
            gateway.stop()
            upstream.close()

    def test_uses_generation_response_body_model_version(self) -> None:
        upstream = MockOpenAIServer(
            "upstream-versioned",
            model_version={"rollout_param_version": 99, "rollout_policy_version": "actor-step-99"},
            response_model_version={
                "rollout_param_version": 11,
                "rollout_policy_version": "actor-step-11",
                "parameter_sync_id": "sync-11",
            },
        )
        gateway = ModelGateway(ModelGatewayConfig(enabled=True, bind_host="127.0.0.1", port=0))
        try:
            gateway_url = gateway.start([upstream.url])
            request = urllib.request.Request(
                f"{gateway_url}/chat/completions",
                data=b'{"model":"policy","messages":[]}',
                headers={"Content-Type": "application/json"},
                method="POST",
            )
            with urllib.request.urlopen(request, timeout=5) as response:
                payload = json.loads(response.read())
                self.assertEqual(response.headers["X-UEnv-Rollout-Param-Version"], "11")
                self.assertEqual(response.headers["X-UEnv-Rollout-Policy-Version"], "actor-step-11")

            self.assertEqual(payload["uenv_model_version"]["rollout_param_version"], 11)
            self.assertEqual(payload["uenv_model_version"]["rollout_policy_version"], "actor-step-11")
            self.assertEqual(payload["uenv_model_version"]["parameter_sync_id"], "sync-11")
            self.assertEqual(payload["uenv_model_version"]["model_upstream"], upstream.url)
            self.assertEqual(payload["uenv_model_version"]["model_version_source_kind"], "generation_response_body")
            self.assertEqual(upstream.model_version_requests, 0)
        finally:
            gateway.stop()
            upstream.close()

    def test_uses_generation_response_header_model_version(self) -> None:
        upstream = MockOpenAIServer(
            "upstream-header-versioned",
            model_version={"rollout_param_version": 99, "rollout_policy_version": "actor-step-99"},
            response_model_version_headers={
                "X-UEnv-Rollout-Param-Version": "12",
                "X-UEnv-Rollout-Policy-Version": "actor-step-12",
                "X-UEnv-Parameter-Sync-Id": "sync-12",
            },
        )
        gateway = ModelGateway(ModelGatewayConfig(enabled=True, bind_host="127.0.0.1", port=0))
        try:
            gateway_url = gateway.start([upstream.url])
            request = urllib.request.Request(
                f"{gateway_url}/chat/completions",
                data=b'{"model":"policy","messages":[]}',
                headers={"Content-Type": "application/json"},
                method="POST",
            )
            with urllib.request.urlopen(request, timeout=5) as response:
                payload = json.loads(response.read())
                self.assertEqual(response.headers["X-UEnv-Rollout-Param-Version"], "12")
                self.assertEqual(response.headers["X-UEnv-Rollout-Policy-Version"], "actor-step-12")

            self.assertEqual(payload["uenv_model_version"]["rollout_param_version"], "12")
            self.assertEqual(payload["uenv_model_version"]["rollout_policy_version"], "actor-step-12")
            self.assertEqual(payload["uenv_model_version"]["parameter_sync_id"], "sync-12")
            self.assertEqual(payload["uenv_model_version"]["model_version_source_kind"], "generation_response_header")
            self.assertEqual(upstream.model_version_requests, 0)
        finally:
            gateway.stop()
            upstream.close()

    def test_does_not_query_model_version_endpoint_when_generation_response_lacks_version(self) -> None:
        upstream = MockOpenAIServer(
            "upstream-fallback-versioned",
            model_version={"rollout_param_version": 13, "rollout_policy_version": "actor-step-13"},
        )
        gateway = ModelGateway(ModelGatewayConfig(enabled=True, bind_host="127.0.0.1", port=0))
        try:
            gateway_url = gateway.start([upstream.url])
            request = urllib.request.Request(
                f"{gateway_url}/chat/completions",
                data=b'{"model":"policy","messages":[]}',
                headers={"Content-Type": "application/json"},
                method="POST",
            )
            with urllib.request.urlopen(request, timeout=5) as response:
                payload = json.loads(response.read())
                self.assertNotIn("X-UEnv-Rollout-Param-Version", response.headers)
                self.assertNotIn("X-UEnv-Rollout-Policy-Version", response.headers)

            self.assertNotIn("uenv_model_version", payload)
            self.assertEqual(upstream.model_version_requests, 0)
        finally:
            gateway.stop()
            upstream.close()


if __name__ == "__main__":
    unittest.main()
