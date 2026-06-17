#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import threading
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from uenv.bridge.model_gateway import ModelGateway, ModelGatewayConfig


class MockOpenAIEndpoint:
    def __init__(self, name: str) -> None:
        self.name = name
        self.requests = 0
        parent = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def do_GET(self) -> None:  # noqa: N802
                self._reply({"object": "list", "data": [{"id": parent.name, "object": "model"}]})

            def do_POST(self) -> None:  # noqa: N802
                body = self.rfile.read(int(self.headers.get("Content-Length", "0") or "0"))
                parent.requests += 1
                payload = {
                    "id": f"chatcmpl-{parent.name}",
                    "object": "chat.completion",
                    "model": parent.name,
                    "choices": [
                        {
                            "index": 0,
                            "message": {"role": "assistant", "content": f"response from {parent.name}"},
                            "finish_reason": "stop",
                        }
                    ],
                    "usage": {"prompt_tokens": len(body), "completion_tokens": 4, "total_tokens": len(body) + 4},
                }
                self._reply(payload)

            def log_message(self, _format: str, *_args: object) -> None:
                return

            def _reply(self, payload: dict[str, object]) -> None:
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


def main() -> None:
    parser = argparse.ArgumentParser(description="Verify Adapter model gateway round-robin locally.")
    parser.add_argument("--requests", type=int, default=4, help="Number of chat completion requests to send.")
    parser.add_argument("--log-path", default="", help="Optional gateway JSONL log path.")
    args = parser.parse_args()

    upstream_a = MockOpenAIEndpoint("mock-vllm-a")
    upstream_b = MockOpenAIEndpoint("mock-vllm-b")
    gateway = ModelGateway(
        ModelGatewayConfig(
            enabled=True,
            bind_host="127.0.0.1",
            port=0,
            log_path=args.log_path,
        )
    )
    try:
        gateway_url = gateway.start([upstream_a.url, upstream_b.url])
        print(f"gateway_url={gateway_url}")
        print(f"upstreams={gateway.upstreams}")

        seen = []
        for index in range(args.requests):
            request = urllib.request.Request(
                f"{gateway_url}/chat/completions",
                data=json.dumps(
                    {
                        "model": "mock-policy",
                        "messages": [{"role": "user", "content": f"request {index}"}],
                        "max_tokens": 16,
                    }
                ).encode("utf-8"),
                headers={"Content-Type": "application/json"},
                method="POST",
            )
            with urllib.request.urlopen(request, timeout=5) as response:
                payload = json.loads(response.read())
            model = payload["model"]
            seen.append(model)
            print(f"request={index} upstream={model} content={payload['choices'][0]['message']['content']}")

        print(f"distribution={{'mock-vllm-a': {seen.count('mock-vllm-a')}, 'mock-vllm-b': {seen.count('mock-vllm-b')}}}")
    finally:
        gateway.stop()
        upstream_a.close()
        upstream_b.close()


if __name__ == "__main__":
    main()

