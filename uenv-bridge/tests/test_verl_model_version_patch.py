from __future__ import annotations

import json
import types
import unittest

from starlette.responses import Response

from uenv.bridge.verl_model_version_patch import (
    _patch_vllm_http_server_class,
    _response_with_model_version,
    _run_server_with_model_version,
)


class DummyServer:
    global_steps = 7
    _server_address = "10.10.20.142"
    _server_port = 36383


class DummyApp:
    def __init__(self) -> None:
        self.state = types.SimpleNamespace()
        self.middleware_calls = []

    def middleware(self, kind):
        def register(callback):
            self.middleware_calls.append((kind, callback))
            return callback

        return register


class VerlModelVersionPatchTest(unittest.IsolatedAsyncioTestCase):
    async def test_attaches_model_version_to_json_response(self) -> None:
        response = Response(
            content=json.dumps({"choices": [{"message": {"content": "ok"}}]}),
            media_type="application/json",
        )

        patched = await _response_with_model_version(response, DummyServer())
        payload = json.loads(patched.body)

        self.assertEqual(payload["uenv_model_version"]["rollout_param_version"], 7)
        self.assertEqual(payload["uenv_model_version"]["rollout_policy_version"], "actor-step-7")
        self.assertEqual(payload["uenv_model_version"]["model_upstream"], "http://10.10.20.142:36383/v1")
        self.assertEqual(patched.headers["X-UEnv-Rollout-Param-Version"], "7")
        self.assertEqual(patched.headers["X-UEnv-Rollout-Policy-Version"], "actor-step-7")

    async def test_replaces_vllm_http_server_class(self) -> None:
        calls = []
        app = DummyApp()

        async def run_uvicorn(app_arg, _args, _server_address):
            self.assertIs(app_arg, app)
            calls.append("uvicorn")

        async def parent_run_server(self, args):
            calls.append(("parent", args))
            await parent_run_server.__globals__["run_uvicorn"](app, args, "127.0.0.1")
            return "ok"

        parent_run_server.__globals__["run_uvicorn"] = run_uvicorn

        class BaseServer:
            run_server = parent_run_server

        module = types.SimpleNamespace(vLLMHttpServer=BaseServer)

        patched_cls = _patch_vllm_http_server_class(module)

        self.assertIs(module.vLLMHttpServer, patched_cls)
        self.assertTrue(issubclass(patched_cls, BaseServer))
        self.assertTrue(patched_cls._uenv_model_version_actor_class)
        self.assertIs(_patch_vllm_http_server_class(module), patched_cls)
        self.assertEqual(await patched_cls().run_server("args"), "ok")
        self.assertEqual(calls[0], ("parent", "args"))
        self.assertEqual(calls[1], "uvicorn")
        self.assertEqual(app.middleware_calls[0][0], "http")

    async def test_run_server_wrapper_restores_original_run_uvicorn(self) -> None:
        calls = []
        app = DummyApp()

        async def run_uvicorn(app_arg, _args, _server_address):
            self.assertIs(app_arg, app)
            calls.append("original")

        async def parent_run_server(self, args):
            calls.append(args)
            await parent_run_server.__globals__["run_uvicorn"](app, args, "127.0.0.1")
            return "ok"

        parent_run_server.__globals__["run_uvicorn"] = run_uvicorn

        result = await _run_server_with_model_version(DummyServer(), "args", parent_run_server)

        self.assertEqual(result, "ok")
        self.assertIs(parent_run_server.__globals__["run_uvicorn"], run_uvicorn)
        self.assertEqual(app.middleware_calls[0][0], "http")


if __name__ == "__main__":
    unittest.main()
