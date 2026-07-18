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
    disable_thinking: bool = False
    force_enable_thinking: bool = False
    preserve_thinking: bool = False
    strip_reasoning: bool = False
    thinking_token_budget: int | None = None


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
                    response_body, status_code, response_headers, model_version = gateway._forward(
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
                    gateway._send_model_version_headers(self.send_header, model_version)
                    self.send_header("Content-Length", str(len(response_body)))
                    self.end_headers()
                    self.wfile.write(response_body)
                except Exception as exc:
                    error = str(exc)
                    model_version = {}
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
                            "model_version": model_version,
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
    ) -> tuple[bytes, int, dict[str, str], dict[str, Any]]:
        upstream_url = self._upstream_url(upstream, path)
        request_headers = {}
        for key, value in headers.items():
            if key.lower() in {"host", "content-length", "connection", "accept-encoding"}:
                continue
            request_headers[key] = value
        forward_body = self._forward_request_body(method=method, path=path, headers=headers, body=body)
        request = urllib.request.Request(
            upstream_url,
            data=forward_body if method != "GET" else None,
            headers=request_headers,
            method=method,
        )
        try:
            with urllib.request.urlopen(request, timeout=self.config.request_timeout_seconds) as response:
                response_headers = dict(response.headers.items())
                raw_body = response.read()
                response_body, model_version = self._response_with_model_version(
                    raw_body,
                    upstream=upstream,
                    response_headers=response_headers,
                )
                return response_body, int(response.status), response_headers, model_version
        except urllib.error.HTTPError as exc:
            response_headers = dict(exc.headers.items())
            raw_body = exc.read()
            response_body, model_version = self._response_with_model_version(
                raw_body,
                upstream=upstream,
                response_headers=response_headers,
            )
            return response_body, int(exc.code), response_headers, model_version

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

    def _forward_request_body(self, *, method: str, path: str, headers: Any, body: bytes) -> bytes:
        should_rewrite = (
            self.config.disable_thinking
            or self.config.force_enable_thinking
            or self.config.preserve_thinking
            or self.config.thinking_token_budget is not None
        )
        if method.upper() == "GET" or not should_rewrite or not body:
            return body
        if not self._is_chat_completions_path(path):
            return body
        content_type = str(headers.get("Content-Type", "") or headers.get("content-type", "")).lower()
        if "json" not in content_type:
            return body
        try:
            data = json.loads(body.decode("utf-8"))
        except Exception:
            return body
        if not isinstance(data, dict):
            return body
        chat_template_kwargs = data.get("chat_template_kwargs")
        if not isinstance(chat_template_kwargs, dict):
            chat_template_kwargs = {}
            data["chat_template_kwargs"] = chat_template_kwargs
        if self.config.disable_thinking:
            chat_template_kwargs["enable_thinking"] = False
        elif self.config.force_enable_thinking:
            chat_template_kwargs["enable_thinking"] = True
        if self.config.preserve_thinking:
            chat_template_kwargs["preserve_thinking"] = True
        if self.config.thinking_token_budget is not None:
            data["thinking_token_budget"] = self.config.thinking_token_budget
        return json.dumps(data, ensure_ascii=False, separators=(",", ":")).encode("utf-8")

    def _is_chat_completions_path(self, path: str) -> bool:
        route = urllib.parse.urlsplit(path).path.rstrip("/")
        return route.endswith("/chat/completions")

    def _response_with_model_version(
        self,
        response_body: bytes,
        *,
        upstream: str,
        response_headers: dict[str, str],
    ) -> tuple[bytes, dict[str, Any]]:
        response_body = self._response_without_reasoning(response_body)
        model_version = self._extract_response_model_version(
            response_body,
            upstream=upstream,
            response_headers=response_headers,
        )
        if not self._has_bound_model_version(model_version):
            return response_body, {}
        return self._attach_model_version(response_body, upstream=upstream, model_version=model_version), model_version

    def _response_without_reasoning(self, response_body: bytes) -> bytes:
        if not self.config.strip_reasoning:
            return response_body
        try:
            data = json.loads(response_body.decode("utf-8"))
        except Exception:
            return response_body
        if not isinstance(data, dict):
            return response_body

        changed = False
        for choice in data.get("choices") or []:
            if not isinstance(choice, dict):
                continue
            message = choice.get("message")
            if not isinstance(message, dict):
                continue
            for key in ("reasoning", "reasoning_content", "reasoning_details"):
                if key in message:
                    message.pop(key, None)
                    changed = True
        if not changed:
            return response_body
        return json.dumps(data, ensure_ascii=False, separators=(",", ":")).encode("utf-8")

    def _extract_response_model_version(
        self,
        response_body: bytes,
        *,
        upstream: str,
        response_headers: dict[str, str],
    ) -> dict[str, Any]:
        data = self._json_object(response_body)
        nested = data.get("uenv_model_version") if data else None
        if isinstance(nested, dict):
            normalized = self._normalize_model_version(
                nested,
                upstream=upstream,
                source="generation_response_body",
                include_raw=False,
            )
            if self._has_bound_model_version(normalized):
                return normalized

        normalized = self._normalize_model_version(
            {str(key).lower(): value for key, value in response_headers.items()},
            upstream=upstream,
            source="generation_response_header",
            include_raw=False,
        )
        if self._has_bound_model_version(normalized):
            return normalized
        return {"model_upstream": upstream}

    def _normalize_model_version(
        self,
        data: dict[str, Any],
        *,
        upstream: str,
        source: str,
        include_raw: bool,
    ) -> dict[str, Any]:
        param_version = self._first_value(
            data,
            "rollout_param_version",
            "X-UEnv-Rollout-Param-Version",
            "x-uenv-rollout-param-version",
            "param_version",
            "current_param_version",
            "global_step",
            "global_steps",
        )
        policy_version = self._first_value(
            data,
            "rollout_policy_version",
            "X-UEnv-Rollout-Policy-Version",
            "x-uenv-rollout-policy-version",
            "policy_version",
        )
        parameter_sync_id = self._first_value(
            data,
            "parameter_sync_id",
            "X-UEnv-Parameter-Sync-Id",
            "x-uenv-parameter-sync-id",
        )
        model_upstream = self._first_value(
            data,
            "model_upstream",
            "X-UEnv-Model-Upstream",
            "x-uenv-model-upstream",
        )
        normalized = {
            "model_upstream": str(model_upstream) if model_upstream is not None else upstream,
            "model_version_source": source,
            "model_version_source_kind": source,
        }
        if include_raw:
            normalized["model_version_raw"] = data
        if param_version is not None:
            normalized["rollout_param_version"] = param_version
        if policy_version is not None:
            normalized["rollout_policy_version"] = str(policy_version)
        elif param_version is not None:
            normalized["rollout_policy_version"] = f"actor-step-{param_version}"
        if parameter_sync_id is not None:
            normalized["parameter_sync_id"] = str(parameter_sync_id)
        return normalized

    def _attach_model_version(self, response_body: bytes, *, upstream: str, model_version: dict[str, Any]) -> bytes:
        if not model_version:
            return response_body
        try:
            data = json.loads(response_body.decode("utf-8"))
        except Exception:
            return response_body
        if not isinstance(data, dict):
            return response_body
        version_payload = {
            "model_upstream": upstream,
            **{key: value for key, value in model_version.items() if key != "model_version_raw"},
        }
        existing = data.get("uenv_model_version")
        if isinstance(existing, dict):
            for key, value in version_payload.items():
                existing.setdefault(key, value)
        else:
            data["uenv_model_version"] = version_payload
        return json.dumps(data, ensure_ascii=False, separators=(",", ":")).encode("utf-8")

    def _send_model_version_headers(self, send_header: Any, model_version: dict[str, Any]) -> None:
        if not model_version:
            return
        header_map = {
            "model_upstream": "X-UEnv-Model-Upstream",
            "rollout_param_version": "X-UEnv-Rollout-Param-Version",
            "rollout_policy_version": "X-UEnv-Rollout-Policy-Version",
            "parameter_sync_id": "X-UEnv-Parameter-Sync-Id",
        }
        for key, header in header_map.items():
            value = model_version.get(key)
            if value is not None:
                send_header(header, str(value))

    def _first_value(self, data: dict[str, Any], *keys: str) -> Any:
        for key in keys:
            value = data.get(key)
            if value not in (None, ""):
                return value
        return None

    def _json_object(self, response_body: bytes) -> dict[str, Any]:
        try:
            data = json.loads(response_body.decode("utf-8"))
        except Exception:
            return {}
        return data if isinstance(data, dict) else {}

    def _has_bound_model_version(self, model_version: dict[str, Any]) -> bool:
        return model_version.get("rollout_param_version") not in (None, "") or model_version.get(
            "rollout_policy_version"
        ) not in (None, "")

    def _record(self, record: dict[str, Any]) -> None:
        if not self.config.log_path:
            return
        path = Path(self.config.log_path)
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("a", encoding="utf-8") as file:
            file.write(json.dumps(record, ensure_ascii=False, separators=(",", ":")) + "\n")
