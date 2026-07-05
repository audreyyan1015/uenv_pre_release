from __future__ import annotations

import json
import os
import sys
from typing import Any


def apply_verl_vllm_model_version_patch() -> None:
    """Attach VeRL rollout weight version to OpenAI-compatible responses."""

    from verl.workers.rollout.vllm_rollout import vllm_async_server

    _patch_vllm_http_server_class(vllm_async_server)
    _patch_vllm_server_actor_env(vllm_async_server)


async def _run_server_with_model_version(self: Any, args: Any, parent_run_server: Any) -> Any:
    print("uenv_model_version_run_server_wrapper", file=sys.stderr, flush=True)
    original_run_globals = parent_run_server.__globals__
    original_run_uvicorn = original_run_globals["run_uvicorn"]

    async def run_uvicorn_with_model_version(app, uvicorn_args, server_address):
        _install_model_version_middleware(app, self)
        return await original_run_uvicorn(app, uvicorn_args, server_address)

    original_run_globals["run_uvicorn"] = run_uvicorn_with_model_version
    try:
        return await parent_run_server(self, args)
    finally:
        original_run_globals["run_uvicorn"] = original_run_uvicorn


def _patch_vllm_http_server_class(vllm_async_server: Any) -> Any:
    """Use a bridge-owned actor class so Ray serializes the patched behavior.

    Ray may import top-level actor classes by module path in the worker process,
    which can drop monkey patches made only in the driver. Replacing the module
    global with a local subclass makes the middleware override part of the actor
    class that Ray serializes.
    """

    current_cls = vllm_async_server.vLLMHttpServer
    if getattr(current_cls, "_uenv_model_version_actor_class", False):
        return current_cls

    parent_run_server = current_cls.run_server

    class UEnvModelVersionVLLMHttpServer(current_cls):
        async def run_server(self, args):
            return await _run_server_with_model_version(self, args, parent_run_server)

    UEnvModelVersionVLLMHttpServer._uenv_model_version_actor_class = True
    vllm_async_server.vLLMHttpServer = UEnvModelVersionVLLMHttpServer
    return UEnvModelVersionVLLMHttpServer


def _patch_vllm_server_actor_env(vllm_async_server: Any) -> None:
    ray_module = vllm_async_server.ray
    if getattr(ray_module, "_uenv_model_version_remote_patch_applied", False):
        return

    original_remote = ray_module.remote

    def remote(*args, **kwargs):
        actor_class = args[0] if args else None
        actor = original_remote(*args, **kwargs)
        if actor_class is vllm_async_server.vLLMHttpServer:
            return _ActorClassWithModelVersionEnv(actor)
        return actor

    ray_module.remote = remote
    ray_module._uenv_model_version_remote_patch_applied = True


class _ActorClassWithModelVersionEnv:
    def __init__(self, actor_class: Any) -> None:
        self._actor_class = actor_class

    def options(self, *args, **kwargs):
        runtime_env = dict(kwargs.get("runtime_env") or {})
        env_vars = dict(runtime_env.get("env_vars") or {})
        env_vars.setdefault("UENV_PATCH_VERL_MODEL_VERSION_RESPONSE", "enabled")
        pythonpath = os.environ.get("PYTHONPATH", "")
        if pythonpath:
            env_vars.setdefault("PYTHONPATH", pythonpath)
        runtime_env["env_vars"] = env_vars
        kwargs["runtime_env"] = runtime_env
        return self._actor_class.options(*args, **kwargs)

    def remote(self, *args, **kwargs):
        return self._actor_class.remote(*args, **kwargs)

    def __getattr__(self, name: str) -> Any:
        return getattr(self._actor_class, name)


def _install_model_version_middleware(app: Any, server: Any) -> None:
    if getattr(app.state, "_uenv_model_version_middleware_installed", False):
        return
    print("uenv_model_version_middleware_installed", file=sys.stderr, flush=True)

    @app.middleware("http")
    async def add_uenv_model_version(request, call_next):
        response = await call_next(request)
        if request.url.path.rstrip("/") not in {"/v1/chat/completions", "/chat/completions"}:
            return response
        return await _response_with_model_version(response, server)

    app.state._uenv_model_version_middleware_installed = True


async def _response_with_model_version(response: Any, server: Any) -> Any:
    version = _model_version(server)
    response.headers["X-UEnv-Model-Upstream"] = _server_url(server)
    response.headers["X-UEnv-Rollout-Param-Version"] = str(version["rollout_param_version"])
    response.headers["X-UEnv-Rollout-Policy-Version"] = version["rollout_policy_version"]

    content_type = response.headers.get("content-type", "")
    if "application/json" not in content_type.lower():
        return response

    body = await _response_body(response)

    try:
        data = json.loads(body.decode("utf-8"))
    except Exception:
        return _clone_response(response, body)

    if isinstance(data, dict):
        existing = data.get("uenv_model_version")
        if not isinstance(existing, dict):
            existing = {}
            data["uenv_model_version"] = existing
        existing.setdefault("model_upstream", version["model_upstream"])
        existing.setdefault("rollout_param_version", version["rollout_param_version"])
        existing.setdefault("rollout_policy_version", version["rollout_policy_version"])
        body = json.dumps(data, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
    return _clone_response(response, body)


def _clone_response(response: Any, body: bytes) -> Any:
    from starlette.responses import Response

    headers = dict(response.headers)
    headers.pop("content-length", None)
    return Response(
        content=body,
        status_code=response.status_code,
        headers=headers,
        media_type=response.media_type,
        background=response.background,
    )


async def _response_body(response: Any) -> bytes:
    body = getattr(response, "body", None)
    if body is not None:
        return body
    output = b""
    async for chunk in response.body_iterator:
        output += chunk
    return output


def _model_version(server: Any) -> dict[str, Any]:
    raw_step = getattr(server, "global_steps", None)
    try:
        step = int(raw_step)
    except Exception:
        step = 0
    return {
        "model_upstream": _server_url(server),
        "rollout_param_version": step,
        "rollout_policy_version": f"actor-step-{step}",
    }


def _server_url(server: Any) -> str:
    address = getattr(server, "_server_address", "") or "127.0.0.1"
    port = getattr(server, "_server_port", None)
    if port is None:
        return f"http://{address}/v1"
    return f"http://{address}:{port}/v1"
