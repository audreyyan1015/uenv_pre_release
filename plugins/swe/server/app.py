"""Minimal OpenEnv-style HTTP server for the SWE environment (stdlib only).

Routes (JSON in/out):
    POST /reset      {instance_id, benchmark_variant?, command_mode?}  -> observation
    POST /step       {type, command?|path?|content?}                    -> step result
    POST /evaluate                                                       -> eval result
    POST /close                                                          -> {ok}
    GET  /health                                                         -> "ok"

This wraps ``SweEnvironment`` (which itself drives the Worker L4 gateway), giving
OpenHands / OpenEnv harnesses a process-local environment endpoint. Single-session
for MVP: ``/reset`` (re)binds the active session.

Run:
    python3 -m plugins.swe.server.app --listen 127.0.0.1:8900 --gateway 127.0.0.1:48999
"""

from __future__ import annotations

import argparse
import json
from dataclasses import asdict
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from ..command_policy import CommandMode, CommandPolicy
from ..environment import SweAction, SweEnvironment


def make_server(listen: str, gateway_url: str):
    host, _, port = listen.partition(":")
    state = {"env": None, "gateway_url": gateway_url}

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *args):  # quiet
            pass

        def _send(self, code, payload):
            body = json.dumps(payload).encode()
            self.send_response(code)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def _body(self):
            length = int(self.headers.get("Content-Length", 0) or 0)
            raw = self.rfile.read(length) if length else b""
            return json.loads(raw) if raw.strip() else {}

        def do_GET(self):
            if self.path == "/health":
                return self._send(200, {"status": "ok"})
            self._send(404, {"error": "not found"})

        def do_POST(self):
            try:
                body = self._body()
                if self.path == "/reset":
                    env = SweEnvironment(
                        instance_id=body["instance_id"],
                        gateway_url=state["gateway_url"],
                        benchmark_variant=body.get("benchmark_variant", "verified"),
                        policy=CommandPolicy(
                            mode=CommandMode.parse(body.get("command_mode", "FullShell"))
                            or CommandMode.FULL_SHELL
                        ),
                    )
                    obs = env.reset()
                    state["env"] = env
                    return self._send(200, asdict(obs))
                env = state["env"]
                if env is None:
                    return self._send(409, {"error": "no active session; POST /reset first"})
                if self.path == "/step":
                    res = env.step(SweAction(**body))
                    return self._send(200, asdict(res))
                if self.path == "/evaluate":
                    return self._send(200, asdict(env.evaluate()))
                if self.path == "/close":
                    env.close()
                    state["env"] = None
                    return self._send(200, {"ok": True})
                self._send(404, {"error": "not found"})
            except Exception as e:  # noqa: BLE001
                self._send(500, {"error": str(e)})

    return ThreadingHTTPServer((host, int(port)), Handler)


def run():
    ap = argparse.ArgumentParser()
    ap.add_argument("--listen", default="127.0.0.1:8900")
    ap.add_argument("--gateway", default="127.0.0.1:48999")
    args = ap.parse_args()
    srv = make_server(args.listen, args.gateway)
    print(f"swe-env server on http://{args.listen} -> gateway {args.gateway}")
    srv.serve_forever()


if __name__ == "__main__":
    run()
