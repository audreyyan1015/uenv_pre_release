"""Dependency-free HTTP client for the UEnv External Runtime Gateway.

Mirrors the Rust gateway contract in ``uenv-worker/src/runtime_gateway/mod.rs``:

    POST   /runtime/v1/sessions               -> {session_id, observation, ...}
    POST   /runtime/v1/sessions/{id}/exec      {command}        -> {stdout, stderr, exit_code, truncated}
    POST   /runtime/v1/sessions/{id}/read      {path}           -> {content}
    POST   /runtime/v1/sessions/{id}/write     {path, content}  -> {ok}
    POST   /runtime/v1/sessions/{id}/submit                     -> {resolved, reward, tests_passed, ...}
    DELETE /runtime/v1/sessions/{id}                            -> {released}
    GET    /runtime/v1/health                                   -> "ok"

Standard library only (``urllib``) so it runs on the offline Worker host.
"""

from __future__ import annotations

import json
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass, field
from typing import Any, Dict, Optional


class GatewayError(RuntimeError):
    """Raised when the gateway returns a non-2xx status."""

    def __init__(self, status: int, message: str):
        super().__init__(f"gateway HTTP {status}: {message}")
        self.status = status
        self.message = message


@dataclass
class ExecResult:
    stdout: str
    stderr: str
    exit_code: int
    truncated: bool = False

    @property
    def ok(self) -> bool:
        return self.exit_code == 0


@dataclass
class SubmitResult:
    instance_id: str
    resolved: bool
    reward: float
    tests_passed: int
    tests_total: int
    per_test: list = field(default_factory=list)
    trajectory_ref: Optional[dict] = None


class UEnvGatewayClient:
    """Thin HTTP client over the Worker L4 gateway."""

    def __init__(
        self,
        base_url: str,
        timeout: float = 600.0,
        api_key: Optional[str] = None,
        run_id: Optional[str] = None,
    ):
        # Accept "host:port" or "http://host:port".
        if not base_url.startswith(("http://", "https://")):
            base_url = "http://" + base_url
        self.base_url = base_url.rstrip("/")
        self.timeout = timeout
        self.api_key = api_key
        self.run_id = (run_id or "").strip() or None

    def _request(
        self,
        method: str,
        path: str,
        body: Optional[dict] = None,
        extra_headers: Optional[Dict[str, str]] = None,
    ) -> Any:
        url = f"{self.base_url}{path}"
        data = json.dumps(body).encode() if body is not None else None
        req = urllib.request.Request(url, data=data, method=method)
        req.add_header("Content-Type", "application/json")
        if self.api_key:
            req.add_header("X-API-Key", self.api_key)
        for key, val in (extra_headers or {}).items():
            if val:
                req.add_header(key, val)
        try:
            with urllib.request.urlopen(req, timeout=self.timeout) as resp:
                raw = resp.read().decode()
        except urllib.error.HTTPError as e:
            detail = e.read().decode(errors="replace")
            raise GatewayError(e.code, detail) from None
        if not raw.strip():
            return {}
        try:
            return json.loads(raw)
        except json.JSONDecodeError:
            return {"raw": raw}

    # ── lifecycle ────────────────────────────────────────────────────
    def health(self) -> bool:
        try:
            self._request("GET", "/runtime/v1/health")
            return True
        except Exception:
            return False

    def create_session(
        self,
        instance_id: str,
        benchmark_variant: str = "verified",
        command_mode: str = "FullShell",
        run_id: Optional[str] = None,
    ) -> "UEnvSession":
        rid = (run_id or self.run_id or "").strip()
        extra_headers = {"X-UEnv-Run-Id": rid} if rid else None
        resp = self._request(
            "POST",
            "/runtime/v1/sessions",
            {
                "instance_id": instance_id,
                "benchmark_variant": benchmark_variant,
                "command_mode": command_mode,
            },
            extra_headers=extra_headers,
        )
        return UEnvSession(
            client=self,
            session_id=resp["session_id"],
            instance_id=resp.get("instance_id", instance_id),
            benchmark_variant=resp.get("benchmark_variant", benchmark_variant),
            command_mode=resp.get("command_mode", command_mode),
            observation=resp.get("observation", {}),
        )

    def create_session_for_episode(
        self,
        instance_id: str,
        episode_id: str,
        run_id: str,
        benchmark_variant: str = "verified",
        command_mode: str = "FullShell",
    ) -> "UEnvSession":
        resp = self._request(
            "POST",
            "/runtime/v1/sessions/for-episode",
            {
                "instance_id": instance_id,
                "episode_id": episode_id,
                "run_id": run_id,
                "benchmark_variant": benchmark_variant,
                "command_mode": command_mode,
            },
        )
        return UEnvSession(
            client=self,
            session_id=resp["session_id"],
            instance_id=resp.get("instance_id", instance_id),
            benchmark_variant=resp.get("benchmark_variant", benchmark_variant),
            command_mode=resp.get("command_mode", command_mode),
            observation=resp.get("observation", {}),
        )

    def attach_session(self, session_id: str, instance_id: str) -> "UEnvSession":
        """Use a Server/Worker pre-created session (skip POST /sessions)."""
        return UEnvSession(
            client=self,
            session_id=session_id,
            instance_id=instance_id,
            benchmark_variant="pro",
            command_mode="FullShell",
            observation={},
        )

    # ── per-session ops (used by UEnvSession) ────────────────────────
    def exec(self, session_id: str, command: str) -> ExecResult:
        r = self._request(
            "POST", f"/runtime/v1/sessions/{session_id}/exec", {"command": command}
        )
        return ExecResult(
            stdout=r.get("stdout", ""),
            stderr=r.get("stderr", ""),
            exit_code=int(r.get("exit_code", -1)),
            truncated=bool(r.get("truncated", False)),
        )

    def read(self, session_id: str, path: str) -> str:
        r = self._request("POST", f"/runtime/v1/sessions/{session_id}/read", {"path": path})
        return r.get("content", "")

    def write(self, session_id: str, path: str, content: str) -> bool:
        r = self._request(
            "POST",
            f"/runtime/v1/sessions/{session_id}/write",
            {"path": path, "content": content},
        )
        return bool(r.get("ok", False))

    def submit(self, session_id: str) -> SubmitResult:
        r = self._request("POST", f"/runtime/v1/sessions/{session_id}/submit")
        return SubmitResult(
            instance_id=r.get("instance_id", ""),
            resolved=bool(r.get("resolved", False)),
            reward=float(r.get("reward", 0.0)),
            tests_passed=int(r.get("tests_passed", 0)),
            tests_total=int(r.get("tests_total", 0)),
            per_test=r.get("per_test", []),
            trajectory_ref=r.get("trajectory_ref"),
        )

    def get_trajectory(self, trajectory_id: str) -> dict:
        return self._request("GET", f"/runtime/v1/trajectories/{trajectory_id}")

    def list_trajectories(
        self,
        instance_id: Optional[str] = None,
        since_ms: Optional[int] = None,
        limit: int = 50,
    ) -> list:
        params = []
        if instance_id:
            params.append(f"instance_id={urllib.parse.quote(instance_id)}")
        if since_ms is not None:
            params.append(f"since_ms={since_ms}")
        params.append(f"limit={limit}")
        qs = "?" + "&".join(params) if params else ""
        return self._request("GET", f"/runtime/v1/trajectories{qs}")

    def destroy(self, session_id: str) -> bool:
        r = self._request("DELETE", f"/runtime/v1/sessions/{session_id}")
        return bool(r.get("released", False))


@dataclass
class UEnvSession:
    """Handle to one gateway session; also a context manager (auto-destroy)."""

    client: UEnvGatewayClient
    session_id: str
    instance_id: str
    benchmark_variant: str
    command_mode: str
    observation: Dict[str, Any] = field(default_factory=dict)

    @property
    def issue_text(self) -> str:
        return self.observation.get("issue_text", "")

    def exec(self, command: str) -> ExecResult:
        return self.client.exec(self.session_id, command)

    def read(self, path: str) -> str:
        return self.client.read(self.session_id, path)

    def write(self, path: str, content: str) -> bool:
        return self.client.write(self.session_id, path, content)

    def apply_patch(self, patch: str, patch_path: str = "/tmp/uenv_agent.patch") -> ExecResult:
        """Write a unified diff into the container and apply it under /testbed."""
        self.write(patch_path, patch)
        return self.exec(
            f"cd /testbed && (git apply -v {patch_path} "
            f"|| patch --batch --fuzz=5 -p1 < {patch_path})"
        )

    def submit(self) -> SubmitResult:
        return self.client.submit(self.session_id)

    def destroy(self) -> bool:
        return self.client.destroy(self.session_id)

    def __enter__(self) -> "UEnvSession":
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        try:
            self.destroy()
        except Exception:
            pass
