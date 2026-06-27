from __future__ import annotations

import json
import tempfile
import threading
import unittest
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from uenv.bridge.model_gateway import ModelGateway, ModelGatewayConfig


class MockOpenAIServer:
    def __init__(self, name: str) -> None:
        self.name = name
        self.requests = []
        parent = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def do_GET(self) -> None:  # noqa: N802
                self._reply({"object": "list", "data": [{"id": parent.name}]})

            def do_POST(self) -> None:  # noqa: N802
                body = self.rfile.read(int(self.headers.get("Content-Length", "0") or "0"))
                parent.requests.append({"path": self.path, "body": body.decode("utf-8")})
                self._reply({"upstream": parent.name, "choices": [{"message": {"content": parent.name}}]})

            def log_message(self, _format: str, *_args) -> None:
                return

            def _reply(self, payload) -> None:
                body = json.dumps(payload).encode("utf-8")
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
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
                records = [json.loads(line) for line in Path(tmpdir, "gateway.jsonl").read_text().splitlines()]
                self.assertEqual([record["upstream_index"] for record in records], [0, 1, 0, 1])
            finally:
                gateway.stop()
                upstream_a.close()
                upstream_b.close()


if __name__ == "__main__":
    unittest.main()

