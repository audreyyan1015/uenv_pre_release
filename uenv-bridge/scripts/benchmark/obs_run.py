#!/usr/bin/env python3
"""Small Obs helpers shared by UEnv benchmark drivers.

The benchmark itself must keep running even if Obs is unavailable, so all event
posting failures are reported as warnings and never raised.
"""

from __future__ import annotations

import json
import os
import sys
import time
import uuid
from pathlib import Path
from typing import Any
from urllib import error, request

_SEQ = 0


def add_obs_args(parser: Any) -> None:
    parser.add_argument(
        "--run-id",
        default=os.getenv("UENV_TRAINING_RUN_ID") or os.getenv("RUN_ID", ""),
        help="training_run_id for Server Obs / frontend visualization; defaults to batch_id.",
    )
    parser.add_argument(
        "--obs-url",
        default=os.getenv("UENV_OBS_URL", ""),
        help="Obs base URL, for example http://8.130.75.157:8888/obs.",
    )
    parser.add_argument(
        "--obs-token",
        default=os.getenv("UENV_OBS_TOKEN", ""),
        help="Optional Obs bearer token.",
    )


def resolve_run_id(explicit_run_id: str, fallback: str) -> str:
    run_id = (explicit_run_id or "").strip()
    return run_id or fallback


def attach_training_run_id(payload: dict[str, Any], training_run_id: str) -> None:
    if not training_run_id:
        return
    metadata = payload.setdefault("metadata", {})
    if isinstance(metadata, dict):
        metadata["training_run_id"] = training_run_id
        extra_info = metadata.setdefault("extra_info", {})
        if isinstance(extra_info, dict):
            extra_info["training_run_id"] = training_run_id


def emit_run_started(
    *,
    obs_url: str,
    obs_token: str,
    training_run_id: str,
    benchmark: str,
    batch_id: str,
    output_dir: Path,
    total_examples: int,
    payload: dict[str, Any] | None = None,
) -> None:
    emit_run_event(
        obs_url=obs_url,
        obs_token=obs_token,
        event_type="RUN_STARTED",
        training_run_id=training_run_id,
        correlation_id=f"benchmark:{batch_id}",
        payload={
            "entry": "benchmark_driver",
            "benchmark": benchmark,
            "batch_id": batch_id,
            "output_dir": str(output_dir),
            "total_examples": total_examples,
            **(payload or {}),
        },
    )


def emit_run_closed(
    *,
    obs_url: str,
    obs_token: str,
    training_run_id: str,
    benchmark: str,
    batch_id: str,
    output_dir: Path,
    ok: bool,
    result_count: int,
    payload: dict[str, Any] | None = None,
) -> None:
    emit_run_event(
        obs_url=obs_url,
        obs_token=obs_token,
        event_type="RUN_CLOSED",
        training_run_id=training_run_id,
        correlation_id=f"benchmark:{batch_id}",
        payload={
            "entry": "benchmark_driver",
            "benchmark": benchmark,
            "batch_id": batch_id,
            "output_dir": str(output_dir),
            "ok": ok,
            "result_count": result_count,
            **(payload or {}),
        },
    )


def emit_episode_result(
    *,
    obs_url: str,
    obs_token: str,
    training_run_id: str,
    benchmark: str,
    batch_id: str,
    request_id: str,
    status: str,
    reward: float,
    correlation_id: str = "",
    attempt_id: int | None = None,
    env_type: str = "",
    trajectory_id: str = "",
    error_code: int | None = None,
    error_message: str = "",
) -> None:
    normalized_status = (status or "").strip().lower()
    event_type = (
        "EPISODE_COMPLETED"
        if normalized_status in {"completed", "success", "succeeded"}
        else "EPISODE_FAILED"
    )
    emit_event(
        obs_url=obs_url,
        obs_token=obs_token,
        event_type=event_type,
        training_run_id=training_run_id,
        correlation_id=correlation_id or f"benchmark:{batch_id}:{request_id}",
        entity_type="episode",
        entity_id=request_id,
        batch_id=batch_id,
        episode_id=request_id,
        attempt_id=attempt_id,
        env_type=env_type,
        payload={
            "entry": "benchmark_driver",
            "benchmark": benchmark,
            "status": status,
            "reward": reward,
            "trajectory_id": trajectory_id,
            "error_code": error_code,
            "error_message": error_message,
        },
    )


def emit_run_event(
    *,
    obs_url: str,
    obs_token: str,
    event_type: str,
    training_run_id: str,
    correlation_id: str,
    payload: dict[str, Any],
) -> None:
    emit_event(
        obs_url=obs_url,
        obs_token=obs_token,
        event_type=event_type,
        training_run_id=training_run_id,
        correlation_id=correlation_id,
        entity_type="training_run",
        entity_id=training_run_id,
        payload=payload,
    )


def emit_event(
    *,
    obs_url: str,
    obs_token: str,
    event_type: str,
    training_run_id: str,
    correlation_id: str,
    entity_type: str,
    entity_id: str,
    payload: dict[str, Any],
    batch_id: str | None = None,
    episode_id: str | None = None,
    attempt_id: int | None = None,
    env_type: str | None = None,
) -> None:
    base = (obs_url or "").rstrip("/")
    if not base or not training_run_id:
        return
    global _SEQ
    _SEQ += 1
    now_ms = int(time.time() * 1000)
    event = {
        "event_id": str(uuid.uuid4()),
        "schema_version": "1",
        "correlation_id": correlation_id,
        "training_run_id": training_run_id,
        "batch_id": batch_id,
        "episode_id": episode_id,
        "attempt_id": attempt_id,
        "env_type": env_type,
        "source_id": f"benchmark-driver:{training_run_id}:{os.getpid()}",
        "module": "adapter",
        "entity_type": entity_type,
        "entity_id": entity_id,
        "event_type": event_type,
        "seq": _SEQ,
        "source_ts": now_ms,
        "payload": payload,
    }
    body = json.dumps(event, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
    headers = {"Content-Type": "application/json"}
    token = (obs_token or "").strip()
    if token:
        headers["Authorization"] = f"Bearer {token}"
        headers["X-Obs-Token"] = token
    req = request.Request(f"{base}/api/v1/events", data=body, headers=headers, method="POST")
    try:
        with request.urlopen(req, timeout=5.0) as resp:
            resp.read()
    except error.URLError as exc:
        print(f"WARN: obs {event_type} failed: {exc}", file=sys.stderr)
    except Exception as exc:  # noqa: BLE001 - visualization must not break evaluation
        print(f"WARN: obs {event_type} failed: {exc}", file=sys.stderr)
