"""Thin HTTP client for UEnv Server Obs ingest (run lifecycle events).

Enabled when ``UENV_OBS_URL`` is set (e.g. ``http://127.0.0.1:50053``).
Failures are logged and never raised to callers — 训练链路不依赖可视化上报。

未配置 ``UENV_OBS_URL`` 时全部 emit 为 no-op（占位/跳过），不产生异常；
前端可自行使用 fixture / Mock 回落展示。
"""

from __future__ import annotations

import logging
import os
import threading
import time
import uuid
from typing import Any
from urllib import error, request

_LOG = logging.getLogger(__name__)

_SEQ_LOCK = threading.Lock()
_SEQ = 0
_SOURCE_ID = f"bridge:{os.getpid()}"


def _next_seq() -> int:
    global _SEQ
    with _SEQ_LOCK:
        _SEQ += 1
        return _SEQ


def obs_enabled() -> bool:
    return bool(os.environ.get("UENV_OBS_URL", "").strip())


def _post_event(event: dict[str, Any]) -> None:
    base = os.environ.get("UENV_OBS_URL", "").rstrip("/")
    if not base:
        return
    token = os.environ.get("UENV_OBS_TOKEN", "").strip()
    url = f"{base}/api/v1/events"
    body = __import__("json").dumps(event).encode("utf-8")
    headers = {"Content-Type": "application/json"}
    if token:
        headers["Authorization"] = f"Bearer {token}"
        headers["X-Obs-Token"] = token
    req = request.Request(url, data=body, headers=headers, method="POST")
    try:
        with request.urlopen(req, timeout=2.0) as resp:
            resp.read()
    except error.URLError as exc:
        _LOG.warning("obs_event_failed event_type=%s err=%s", event.get("event_type"), exc)
    except Exception as exc:  # noqa: BLE001 — never break training
        _LOG.warning("obs_event_failed event_type=%s err=%s", event.get("event_type"), exc)


def emit_run_event(
    event_type: str,
    training_run_id: str,
    *,
    correlation_id: str | None = None,
    payload: dict[str, Any] | None = None,
) -> None:
    if not obs_enabled():
        return
    now_ms = int(time.time() * 1000)
    event = {
        "event_id": str(uuid.uuid4()),
        "schema_version": "1",
        "correlation_id": correlation_id or f"run:{training_run_id}",
        "training_run_id": training_run_id,
        "source_id": _SOURCE_ID,
        "module": "adapter",
        "entity_type": "training_run",
        "entity_id": training_run_id,
        "event_type": event_type,
        "seq": _next_seq(),
        "source_ts": now_ms,
        "payload": payload or {},
    }
    _post_event(event)


def run_started(training_run_id: str, **kwargs: Any) -> None:
    emit_run_event("RUN_STARTED", training_run_id, **kwargs)


def run_stopped(training_run_id: str, **kwargs: Any) -> None:
    emit_run_event("RUN_STOPPED", training_run_id, **kwargs)


def run_closed(training_run_id: str, **kwargs: Any) -> None:
    emit_run_event("RUN_CLOSED", training_run_id, **kwargs)
