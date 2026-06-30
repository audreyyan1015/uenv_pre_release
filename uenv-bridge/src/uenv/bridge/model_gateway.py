from __future__ import annotations

import json
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass, field
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any


def normalize_openai_endpoint(value: str) -> str:
    text = str(value or "").strip()
    if not text:
        raise ValueError("model gateway upstream endpoint cannot be empty")
    if text.startswith(("http://", "https://")):
        base = text.rstrip("/")
    else:
        base = f"http://{text.rstrip('/')}"
    return base if base.endswith("/v1") else f"{base}/v1"


@dataclass(slots=True)
class ModelGatewayConfig:
    enabled: bool = False
    bind_host: str = "0.0.0.0"
    port: int = 18080
    public_url: str = ""
    request_timeout_seconds: float = 300.0
    log_path: str = ""


@dataclass(slots=True)
class ModelGateway:
    config: ModelGatewayConfig
    _server: ThreadingHTTPServer | None = field(default=None, init=False, repr=False)
    _thread: threading.Thread | None = field(default=None, init=False, repr=False)
    _upstreams: list[str] = field(default_factory=list, init=False, repr=False)
    _lock: threading.Lock = field(default_factory=threading.Lock, init=False, repr=False)
    _next_index: int = field(default=0, init=False, repr=False)

    def start(self, upstreams: list[str]) -> str:
        self.set_upstreams(upstreams)
        if self._server is None:
            self._server = self._build_server()
            self._thread = threading.Thread(target=self._server.serve_forever, name="uenv-model-gateway", daemon=True)
            self._thread.start()
        return self.public_url

    def stop(self) -> None:
        if self._server is not None:
            self._server.shutdown()
            self._server.server_close()
            self._server = None
        if self._thread is not None:
            self._thread.join(timeout=5)
            self._thread = None

    @property
    def public_url(self) -> str:
        if self.config.public_url:
            return normalize_openai_endpoint(self.config.public_url)
        host = self.config.bind_host
        if host in {"", "0.0.0.0", "::"}:
            host = "127.0.0.1"
        port = self._server.server_port if self._server is not None else self.config.port
        return normalize_openai_endpoint(f"http://{host}:{port}/v1")

    @property
    def upstreams(self) -> list[str]:
        with self._lock:
            return list(self._upstreams)

    def set_upstreams(self, upstreams: list[str]) -> None:
        normalized = []
        seen = set()
        for upstream in upstreams:
            endpoint = normalize_openai_endpoint(upstream)
            if endpoint in seen:
                continue
            seen.add(endpoint)
            normalized.append(endpoint)
        if not normalized:
            raise ValueError("model gateway requires at least one upstream endpoint")
        with self._lock:
            self._upstreams = normalized
            self._next_index %= len(normalized)

    def choose_upstream(self) -> tuple[int, str]:
        with self._lock:
            if not self._upstreams:
                raise RuntimeError("model gateway has no upstream endpoints")
            index = self._next_index % len(self._upstreams)
            self._next_index += 1
            return index, self._upstreams[index]

    def _build_server(self) -> ThreadingHTTPServer:
        gateway = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def do_GET(self) -> None:  # noqa: N802
                self._handle_proxy()

            def do_POST(self) -> None:  # noqa: N802
                self._handle_proxy()

            def log_message(self, _format: str, *_args: Any) -> None:
                return

            def _handle_proxy(self) -> None:
                started = time.time()
                upstream_index = -1
                upstream = ""
                status_code = 502
                error = ""
                body = b""
                try:
                    upstream_index, upstream = gateway.choose_upstream()
                    body = self.rfile.read(int(self.headers.get("Content-Length", "0") or "0"))
                    response_body, status_code, response_headers = gateway._forward(
                        method=self.command,
                        path=self.path,
                        headers=self.headers,
                        body=body,
                        upstream=upstream,
                    )
                    self.send_response(status_code)
                    for key, value in response_headers.items():
                        if key.lower() in {"connection", "content-length", "transfer-encoding", "content-encoding"}:
                            continue
                        self.send_header(key, value)
                    self.send_header("Content-Length", str(len(response_body)))
                    self.end_headers()
                    self.wfile.write(response_body)
                except Exception as exc:
                    error = str(exc)
                    response_body = json.dumps({"error": {"message": error, "type": "uenv_model_gateway_error"}}).encode("utf-8")
                    self.send_response(status_code)
                    self.send_header("Content-Type", "application/json")
                    self.send_header("Content-Length", str(len(response_body)))
                    self.end_headers()
                    self.wfile.write(response_body)
                finally:
                    gateway._record(
                        {
                            "ts": started,
                            "method": self.command,
                            "path": self.path,
                            "upstream_index": upstream_index,
                            "upstream_url": upstream,
                            "status_code": status_code,
                            "latency_ms": round((time.time() - started) * 1000, 3),
                            "request_bytes": len(body),
                            "error": error,
                        }
                    )

        return ThreadingHTTPServer((self.config.bind_host, self.config.port), Handler)

    def _forward(
        self,
        *,
        method: str,
        path: str,
        headers: Any,
        body: bytes,
        upstream: str,
    ) -> tuple[bytes, int, dict[str, str]]:
        upstream_url = self._upstream_url(upstream, path)
        request_headers = {}
        for key, value in headers.items():
            if key.lower() in {"host", "content-length", "connection", "accept-encoding"}:
                continue
            request_headers[key] = value
        request = urllib.request.Request(upstream_url, data=body if method != "GET" else None, headers=request_headers, method=method)
        try:
            with urllib.request.urlopen(request, timeout=self.config.request_timeout_seconds) as response:
                return response.read(), int(response.status), dict(response.headers.items())
        except urllib.error.HTTPError as exc:
            return exc.read(), int(exc.code), dict(exc.headers.items())

    def _upstream_url(self, upstream: str, path: str) -> str:
        parsed = urllib.parse.urlsplit(path)
        route = parsed.path or "/"
        if route.startswith("/v1/"):
            route = route[3:]
        elif route == "/v1":
            route = ""
        elif route.startswith("v1/"):
            route = route[2:]
        query = f"?{parsed.query}" if parsed.query else ""
        return f"{upstream}{route}{query}"

    def _record(self, record: dict[str, Any]) -> None:
        if not self.config.log_path:
            return
        path = Path(self.config.log_path)
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("a", encoding="utf-8") as file:
            file.write(json.dumps(record, ensure_ascii=False, separators=(",", ":")) + "\n")
