#!/usr/bin/env python3
"""OpenHands benchmark runner HTTP API (208.77 :8888, health :8777)."""
from __future__ import annotations

import json
import os
import subprocess
import threading
import time
import uuid
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from typing import Any
from urllib.parse import urlparse

API_BIND = os.environ.get("OPENHANDS_RUNNER_API_BIND", "0.0.0.0:8888")
HEALTH_BIND = os.environ.get("OPENHANDS_RUNNER_HEALTH_BIND", "0.0.0.0:8777")
RUN_SCRIPT = os.environ.get(
    "OPENHANDS_RUN_SCRIPT", "/root/UEnv/scripts/run-openhands-pro-20877.sh"
)
RUNS_DIR = Path(os.environ.get("OPENHANDS_RUNS_DIR", "/var/log/uenv/openhands-runs"))

_lock = threading.Lock()
_jobs: dict[str, dict[str, Any]] = {}


def _parse_bind(bind: str) -> tuple[str, int]:
    host, _, port = bind.rpartition(":")
    return host or "0.0.0.0", int(port or "8080")


def _run_job(job_id: str, mode: str, max_iterations: int, instance: str | None) -> None:
    env = os.environ.copy()
    if instance:
        env["UENV_PRO_INSTANCE"] = instance
    env["MAX_ITERATIONS"] = str(max_iterations)
    cmd = ["bash", RUN_SCRIPT, mode]
    with _lock:
        _jobs[job_id]["status"] = "running"
        _jobs[job_id]["started_at"] = time.time()
    try:
        proc = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            env=env,
            timeout=int(os.environ.get("OPENHANDS_RUN_TIMEOUT_SEC", "7200")),
            check=False,
        )
        with _lock:
            _jobs[job_id]["status"] = "succeeded" if proc.returncode == 0 else "failed"
            _jobs[job_id]["exit_code"] = proc.returncode
            _jobs[job_id]["stdout"] = proc.stdout[-8000:]
            _jobs[job_id]["stderr"] = proc.stderr[-8000:]
            _jobs[job_id]["finished_at"] = time.time()
    except subprocess.TimeoutExpired as exc:
        with _lock:
            _jobs[job_id]["status"] = "timeout"
            _jobs[job_id]["stderr"] = str(exc)[-8000:]
            _jobs[job_id]["finished_at"] = time.time()


class ApiHandler(BaseHTTPRequestHandler):
    server_version = "openhands-runner/1.0"

    def log_message(self, fmt: str, *args: Any) -> None:
        print(f"[runner-api] {self.address_string()} {fmt % args}", flush=True)

    def _json(self, code: int, body: dict[str, Any]) -> None:
        raw = json.dumps(body, ensure_ascii=False).encode("utf-8")
        self.send_response(code)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def do_GET(self) -> None:  # noqa: N802
        path = urlparse(self.path).path
        if path == "/health":
            self._json(200, {"status": "ok", "service": "openhands-runner"})
            return
        if path.startswith("/v1/runs/"):
            job_id = path.rsplit("/", 1)[-1]
            with _lock:
                job = _jobs.get(job_id)
            if not job:
                self._json(404, {"error": "run not found", "id": job_id})
                return
            self._json(200, job)
            return
        self._json(404, {"error": "not found"})

    def do_POST(self) -> None:  # noqa: N802
        path = urlparse(self.path).path
        if path != "/v1/runs":
            self._json(404, {"error": "not found"})
            return
        length = int(self.headers.get("Content-Length", "0"))
        raw = self.rfile.read(length) if length else b"{}"
        try:
            payload = json.loads(raw.decode("utf-8") or "{}")
        except json.JSONDecodeError:
            self._json(400, {"error": "invalid json"})
            return
        mode = str(payload.get("mode", "gold")).lower()
        if mode not in {"gold", "llm"}:
            self._json(400, {"error": "mode must be gold or llm"})
            return
        max_iterations = int(payload.get("max_iterations", 30))
        instance = payload.get("instance")
        job_id = str(uuid.uuid4())
        job = {
            "id": job_id,
            "mode": mode,
            "max_iterations": max_iterations,
            "instance": instance,
            "status": "queued",
            "created_at": time.time(),
        }
        with _lock:
            _jobs[job_id] = job
        threading.Thread(
            target=_run_job,
            args=(job_id, mode, max_iterations, instance),
            daemon=True,
        ).start()
        self._json(202, {"id": job_id, "status": "queued"})


class HealthHandler(BaseHTTPRequestHandler):
    def log_message(self, fmt: str, *args: Any) -> None:
        print(f"[runner-health] {self.address_string()} {fmt % args}", flush=True)

    def do_GET(self) -> None:  # noqa: N802
        body = b'{"status":"ok","service":"openhands-runner"}\n'
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


def _serve(name: str, bind: str, handler: type[BaseHTTPRequestHandler]) -> None:
    host, port = _parse_bind(bind)
    httpd = HTTPServer((host, port), handler)
    print(f"[{name}] listening on {host}:{port}", flush=True)
    httpd.serve_forever()


def main() -> None:
    RUNS_DIR.mkdir(parents=True, exist_ok=True)
    threading.Thread(
        target=_serve, args=("health", HEALTH_BIND, HealthHandler), daemon=True
    ).start()
    _serve("api", API_BIND, ApiHandler)


if __name__ == "__main__":
    main()
