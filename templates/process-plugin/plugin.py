#!/usr/bin/env python3
"""UEnv generated gRPC/UDS adapter.

DO NOT put task logic in this file. Edit environment.py. This adapter only
translates the stable Worker protocol into Environment.reset/step/close calls.
"""

from __future__ import annotations

import argparse
import json
import signal
import sys
import threading
from concurrent import futures
from pathlib import Path
from typing import Any

import grpc

PLUGIN_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(PLUGIN_DIR / "generated"))

import plugin_pb2  # noqa: E402
import plugin_pb2_grpc  # noqa: E402
from environment import Environment  # noqa: E402
from uenv_plugin_api import ResetResult, StepResult  # noqa: E402


def _load_sidecar(uds_path: Path) -> dict[str, Any]:
    path = Path(f"{uds_path}.episode.json")
    if not path.is_file():
        return {}
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"episode sidecar must contain a JSON object: {path}")
    return value


def _observation_bytes(value: Any) -> bytes:
    if isinstance(value, bytes):
        return value
    if isinstance(value, str):
        return value.encode("utf-8")
    return json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode("utf-8")


def _info_strings(values: dict[str, Any]) -> dict[str, str]:
    result: dict[str, str] = {}
    for key, value in values.items():
        if value is None:
            result[str(key)] = ""
        elif isinstance(value, bool):
            result[str(key)] = str(value).lower()
        elif isinstance(value, (dict, list)):
            result[str(key)] = json.dumps(value, ensure_ascii=False, separators=(",", ":"))
        else:
            result[str(key)] = str(value)
    return result


class PluginAdapter(plugin_pb2_grpc.PluginServiceServicer):
    """Protocol adapter. Environment methods are serialized per Episode."""

    def __init__(self, uds_path: Path, stop_event: threading.Event) -> None:
        self.uds_path = uds_path
        self.stop_event = stop_event
        self.lock = threading.Lock()
        self.environment = Environment()

    def Reset(self, request, context):  # noqa: N802 - generated RPC name
        try:
            config = _load_sidecar(self.uds_path)
            seed = request.seed if request.HasField("seed") else None
            with self.lock:
                result = self.environment.reset(config, seed)
            if not isinstance(result, ResetResult):
                raise TypeError("Environment.reset() must return ResetResult")
        except (OSError, ValueError, TypeError, json.JSONDecodeError) as exc:
            context.abort(grpc.StatusCode.INVALID_ARGUMENT, str(exc))
        envelope = config.get("_uenv") if isinstance(config.get("_uenv"), dict) else {}
        info = dict(result.info)
        info.setdefault("sidecar_schema_version", envelope.get("sidecar_schema_version", 0))
        return plugin_pb2.ResetResponse(
            observation=_observation_bytes(result.observation),
            info=_info_strings(info),
        )

    def Step(self, request, context):  # noqa: N802 - generated RPC name
        try:
            with self.lock:
                result = self.environment.step(bytes(request.action))
            if not isinstance(result, StepResult):
                raise TypeError("Environment.step() must return StepResult")
        except (ValueError, TypeError) as exc:
            context.abort(grpc.StatusCode.INVALID_ARGUMENT, str(exc))
        return plugin_pb2.StepResponse(
            observation=_observation_bytes(result.observation),
            reward=float(result.reward),
            terminated=bool(result.terminated),
            truncated=bool(result.truncated),
            info=_info_strings(result.info),
        )

    def Close(self, request, context):  # noqa: N802 - generated RPC name
        try:
            with self.lock:
                self.environment.close()
        finally:
            self.stop_event.set()
        return plugin_pb2.CloseResponse(ok=True)

    def HealthCheck(self, request, context):  # noqa: N802 - generated RPC name
        return plugin_pb2.HealthCheckResponse(ok=True, message="ready")


def serve(uds_path: Path) -> int:
    try:
        uds_path.unlink(missing_ok=True)
    except OSError as exc:
        raise RuntimeError(f"cannot remove stale UDS {uds_path}: {exc}") from exc

    stop_event = threading.Event()
    server = grpc.server(futures.ThreadPoolExecutor(max_workers=4))
    plugin_pb2_grpc.add_PluginServiceServicer_to_server(
        PluginAdapter(uds_path, stop_event), server
    )
    if server.add_insecure_port(f"unix:{uds_path}") != 1:
        raise RuntimeError(f"failed to bind UDS: {uds_path}")

    def request_stop(_signum=None, _frame=None) -> None:
        stop_event.set()

    signal.signal(signal.SIGTERM, request_stop)
    signal.signal(signal.SIGINT, request_stop)
    server.start()
    try:
        stop_event.wait()
    finally:
        server.stop(grace=1).wait()
        uds_path.unlink(missing_ok=True)
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--uds-path", required=True, type=Path)
    args = parser.parse_args()
    return serve(args.uds_path)


if __name__ == "__main__":
    raise SystemExit(main())
