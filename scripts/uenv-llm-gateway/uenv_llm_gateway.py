#!/usr/bin/env python3
"""UEnv LLM Gateway — OpenAI-compatible proxy with auth and backend readiness."""

from __future__ import annotations

import argparse
import asyncio
import contextlib
import json
import logging
import os
import sys
import time
import uuid
from contextlib import asynccontextmanager
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, AsyncIterator

import httpx
import uvicorn
import yaml
from starlette.applications import Starlette
from starlette.middleware.base import BaseHTTPMiddleware
from starlette.requests import Request
from starlette.responses import JSONResponse, Response, StreamingResponse
from starlette.routing import Route

LOG = logging.getLogger("uenv-llm-gateway")


@dataclass
class GatewayConfig:
    listen: str = "0.0.0.0:18888"
    advertise_endpoint: str = ""
    health_listen: str = "0.0.0.0:18777"
    backend_base_url: str = "http://127.0.0.1:8000/v1"
    readiness_path: str = "/models"
    readiness_interval_sec: float = 5.0
    readiness_timeout_sec: float = 900.0
    api_key_env: str = "UENV_LLM_GATEWAY_API_KEY"
    model_id: str = "deepseek-v3-0324-awq"
    model_enabled: bool = True
    proxy_timeout_sec: float = 600.0
    max_body_bytes: int = 8_388_608
    log_file: str = "/var/log/uenv/uenv-llm-gateway.log"


@dataclass
class GatewayState:
    config: GatewayConfig
    api_key: str = ""
    backend_ready: bool = False
    backend_last_error: str = ""
    backend_checked_at: float = 0.0
    started_at: float = field(default_factory=time.time)
    _poll_task: asyncio.Task[None] | None = None


def load_config(path: Path) -> GatewayConfig:
    raw = yaml.safe_load(path.read_text(encoding="utf-8")) or {}
    obs = raw.get("observability") or {}
    backend = raw.get("backend") or {}
    auth = raw.get("auth") or {}
    model = raw.get("model") or {}
    proxy = raw.get("proxy") or {}
    return GatewayConfig(
        listen=str(raw.get("listen", "0.0.0.0:18888")),
        advertise_endpoint=str(raw.get("advertise_endpoint", "")),
        health_listen=str(obs.get("health_listen", "0.0.0.0:18777")),
        backend_base_url=str(backend.get("base_url", "http://127.0.0.1:8000/v1")).rstrip("/"),
        readiness_path=str(backend.get("readiness_path", "/models")),
        readiness_interval_sec=float(backend.get("readiness_interval_sec", 5)),
        readiness_timeout_sec=float(backend.get("readiness_timeout_sec", 900)),
        api_key_env=str(auth.get("api_key_env", "UENV_LLM_GATEWAY_API_KEY")),
        model_id=str(model.get("id", "deepseek-v3-0324-awq")),
        model_enabled=bool(model.get("enabled", True)),
        proxy_timeout_sec=float(proxy.get("timeout_sec", 600)),
        max_body_bytes=int(proxy.get("max_body_bytes", 8_388_608)),
        log_file=str(raw.get("log_file", "/var/log/uenv/uenv-llm-gateway.log")),
    )


def setup_logging(log_file: str) -> None:
    handlers: list[logging.Handler] = [logging.StreamHandler(sys.stdout)]
    try:
        Path(log_file).parent.mkdir(parents=True, exist_ok=True)
        handlers.append(logging.FileHandler(log_file, encoding="utf-8"))
    except OSError as exc:
        LOG.warning("cannot open log file %s: %s", log_file, exc)
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s %(levelname)s %(name)s %(message)s",
        handlers=handlers,
    )


def extract_api_key(request: Request) -> str | None:
    auth = request.headers.get("authorization", "")
    if auth.lower().startswith("bearer "):
        return auth[7:].strip()
    return request.headers.get("x-api-key")


def json_error(status: int, message: str) -> JSONResponse:
    return JSONResponse({"error": message}, status_code=status)


async def poll_backend_ready(state: GatewayState) -> None:
    cfg = state.config
    url = f"{cfg.backend_base_url}{cfg.readiness_path}"
    timeout = httpx.Timeout(10.0, connect=5.0)
    async with httpx.AsyncClient(timeout=timeout) as client:
        while True:
            try:
                resp = await client.get(url)
                ready = resp.status_code == 200
                state.backend_ready = ready
                state.backend_last_error = "" if ready else f"HTTP {resp.status_code}"
            except Exception as exc:  # noqa: BLE001
                state.backend_ready = False
                state.backend_last_error = str(exc)
            state.backend_checked_at = time.time()
            await asyncio.sleep(cfg.readiness_interval_sec)


def backend_status(state: GatewayState) -> str:
    if not state.config.model_enabled:
        return "model_disabled"
    if state.backend_ready:
        return "ok"
    elapsed = time.time() - state.started_at
    if elapsed < state.config.readiness_timeout_sec:
        return "starting"
    return "backend_down"


async def health_handler(request: Request) -> Response:
    state: GatewayState = request.app.state.gateway
    status = backend_status(state)
    body = {
        "status": status,
        "model_enabled": state.config.model_enabled,
        "backend_ready": state.backend_ready,
        "model_id": state.config.model_id,
        "backend_last_error": state.backend_last_error,
        "uptime_sec": round(time.time() - state.started_at, 1),
    }
    code = 200 if status == "ok" else 503
    return JSONResponse(body, status_code=code)


async def proxy_handler(request: Request) -> Response:
    state: GatewayState = request.app.state.gateway
    cfg = state.config
    req_id = request.headers.get("x-request-id") or str(uuid.uuid4())[:8]
    t0 = time.time()

    if cfg.api_key_env and state.api_key:
        provided = extract_api_key(request)
        if provided != state.api_key:
            LOG.warning("request_id=%s auth_failed", req_id)
            return json_error(401, "unauthorized")

    if not cfg.model_enabled:
        return json_error(503, "model_disabled")

    if not state.backend_ready:
        return json_error(503, "backend_starting")

    body = await request.body()
    if len(body) > cfg.max_body_bytes:
        return json_error(413, "payload_too_large")

    suffix = request.url.path
    if request.url.query:
        suffix = f"{suffix}?{request.url.query}"
    target = f"{cfg.backend_base_url}{suffix}"

    headers = {
        k.decode(): v.decode()
        for k, v in request.headers.raw
        if k.decode().lower() not in {"host", "content-length", "transfer-encoding"}
    }

    timeout = httpx.Timeout(cfg.proxy_timeout_sec, connect=30.0)
    client = httpx.AsyncClient(timeout=timeout)

    try:
        is_stream = False
        if body:
            try:
                payload = json.loads(body)
                is_stream = bool(payload.get("stream"))
            except json.JSONDecodeError:
                pass

        if is_stream:
            stream_ctx = client.stream(
                request.method,
                target,
                content=body,
                headers=headers,
            )
            upstream = await stream_ctx.__aenter__()

            if upstream.status_code >= 400:
                err_body = await upstream.aread()
                await stream_ctx.__aexit__(None, None, None)
                await client.aclose()
                LOG.warning(
                    "request_id=%s stream_upstream_error status=%s latency=%.2fs",
                    req_id,
                    upstream.status_code,
                    time.time() - t0,
                )
                return Response(
                    content=err_body,
                    status_code=upstream.status_code,
                    media_type=upstream.headers.get("content-type"),
                )

            async def event_stream() -> AsyncIterator[bytes]:
                try:
                    async for chunk in upstream.aiter_bytes():
                        yield chunk
                finally:
                    await stream_ctx.__aexit__(None, None, None)
                    await client.aclose()
                    LOG.info(
                        "request_id=%s stream_done latency=%.2fs",
                        req_id,
                        time.time() - t0,
                    )

            return StreamingResponse(
                event_stream(),
                status_code=upstream.status_code,
                media_type=upstream.headers.get("content-type", "text/event-stream"),
            )

        resp = await client.request(request.method, target, content=body, headers=headers)
        LOG.info(
            "request_id=%s %s %s -> %s latency=%.2fs",
            req_id,
            request.method,
            request.url.path,
            resp.status_code,
            time.time() - t0,
        )
        return Response(
            content=resp.content,
            status_code=resp.status_code,
            media_type=resp.headers.get("content-type"),
        )
    except httpx.TimeoutException:
        await client.aclose()
        LOG.error("request_id=%s proxy_timeout path=%s", req_id, request.url.path)
        return json_error(504, "backend_timeout")
    except Exception as exc:  # noqa: BLE001
        await client.aclose()
        LOG.exception("request_id=%s proxy_error path=%s err=%s", req_id, request.url.path, exc)
        return json_error(502, "backend_error")


def build_api_app(state: GatewayState) -> Starlette:
    @asynccontextmanager
    async def lifespan(app: Starlette):  # noqa: ARG001
        state._poll_task = asyncio.create_task(poll_backend_ready(state))
        LOG.info(
            "gateway_listen=%s health=%s backend=%s model=%s",
            state.config.listen,
            state.config.health_listen,
            state.config.backend_base_url,
            state.config.model_id,
        )
        yield
        if state._poll_task:
            state._poll_task.cancel()
            with contextlib.suppress(asyncio.CancelledError):
                await state._poll_task

    routes = [
        Route("/v1/{path:path}", proxy_handler, methods=["GET", "POST", "PUT", "DELETE", "PATCH"]),
        Route("/v1", proxy_handler, methods=["GET", "POST"]),
    ]
    app = Starlette(routes=routes, lifespan=lifespan)
    app.state.gateway = state
    return app


def build_health_app(state: GatewayState) -> Starlette:
    app = Starlette(routes=[Route("/health", health_handler, methods=["GET"])])
    app.state.gateway = state
    return app


async def run_servers(state: GatewayState) -> None:
    api = uvicorn.Server(
        uvicorn.Config(
            build_api_app(state),
            host=state.config.listen.rsplit(":", 1)[0],
            port=int(state.config.listen.rsplit(":", 1)[1]),
            log_level="info",
            access_log=False,
        )
    )
    health = uvicorn.Server(
        uvicorn.Config(
            build_health_app(state),
            host=state.config.health_listen.rsplit(":", 1)[0],
            port=int(state.config.health_listen.rsplit(":", 1)[1]),
            log_level="info",
            access_log=False,
        )
    )
    await asyncio.gather(api.serve(), health.serve())


def main() -> None:
    parser = argparse.ArgumentParser(description="UEnv LLM Gateway")
    parser.add_argument("--config", required=True, help="Path to YAML config")
    args = parser.parse_args()

    cfg_path = Path(args.config)
    cfg = load_config(cfg_path)
    setup_logging(cfg.log_file)

    api_key = os.environ.get(cfg.api_key_env, "").strip()
    if not api_key:
        LOG.warning("env %s is empty; auth checks disabled", cfg.api_key_env)

    state = GatewayState(config=cfg, api_key=api_key)
    asyncio.run(run_servers(state))


if __name__ == "__main__":
    main()
