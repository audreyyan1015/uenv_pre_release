#!/usr/bin/env python3
"""Own a large fleet of real uenv-worker child processes as one test process group."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import time


parser = argparse.ArgumentParser()
parser.add_argument("--spec", required=True)
parser.add_argument("--pid-file", required=True)
parser.add_argument("--metrics-file", required=True)
args = parser.parse_args()

spec = json.loads(Path(args.spec).read_text(encoding="utf-8"))
workers = spec.get("workers")
if not isinstance(workers, list) or not workers:
    raise SystemExit("fleet spec requires a non-empty workers list")

stopping = False


def request_stop(_signum, _frame):
    global stopping
    stopping = True


signal.signal(signal.SIGTERM, request_stop)
signal.signal(signal.SIGINT, request_stop)

children: list[tuple[subprocess.Popen, object, dict]] = []
exit_code = 0


def memory_bytes() -> tuple[int, int]:
    values: dict[str, int] = {}
    for line in Path("/proc/meminfo").read_text(encoding="utf-8").splitlines():
        key, value = line.split(":", 1)
        values[key] = int(value.strip().split()[0]) * 1024
    return values["MemTotal"], values["MemAvailable"]


def process_group_metrics() -> tuple[int, int, int]:
    """Return process count, RSS bytes and open FD count for this owned process group."""
    process_count = 0
    rss_bytes = 0
    open_fds = 0
    group_id = os.getpgrp()
    for proc_dir in Path("/proc").iterdir():
        if not proc_dir.name.isdigit():
            continue
        try:
            fields = (proc_dir / "stat").read_text(encoding="utf-8").split()
            if int(fields[4]) != group_id:
                continue
            process_count += 1
            for line in (proc_dir / "status").read_text(encoding="utf-8").splitlines():
                if line.startswith("VmRSS:"):
                    rss_bytes += int(line.split()[1]) * 1024
                    break
            open_fds += sum(1 for _ in (proc_dir / "fd").iterdir())
        except (FileNotFoundError, PermissionError, ProcessLookupError, ValueError, IndexError):
            continue
    return process_count, rss_bytes, open_fds


def write_json_atomic(path: Path, document: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(document, indent=2, sort_keys=True), encoding="utf-8")
    temporary.replace(path)


metrics_target = Path(args.metrics_file)
mem_total, mem_available = memory_bytes()
metrics = {
    "sample_count": 0,
    "mem_total_bytes": mem_total,
    "min_mem_available_bytes": mem_available,
    "peak_processes": 0,
    "peak_rss_bytes": 0,
    "peak_open_fds": 0,
    "started_unix": time.time(),
}
try:
    for item in workers:
        argv = item.get("argv")
        log_path = Path(item.get("log", ""))
        if not isinstance(argv, list) or not argv or not log_path.is_absolute():
            raise RuntimeError(f"invalid fleet worker spec: {item!r}")
        log_path.parent.mkdir(parents=True, exist_ok=True)
        log = log_path.open("ab", buffering=0)
        env = os.environ.copy()
        env.update({str(key): str(value) for key, value in item.get("env", {}).items()})
        child = subprocess.Popen(
            [str(value) for value in argv],
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=log,
            stderr=subprocess.STDOUT,
            start_new_session=False,
        )
        children.append((child, log, item))

    pid_document = {
        "supervisor_pid": os.getpid(),
        "worker_count": len(children),
        "workers": [
            {"pid": child.pid, "worker_id": item.get("worker_id"), "config": item.get("config")}
            for child, _log, item in children
        ],
    }
    target = Path(args.pid_file)
    write_json_atomic(target, pid_document)

    while not stopping:
        process_count, rss_bytes, open_fds = process_group_metrics()
        _mem_total, mem_available = memory_bytes()
        metrics["sample_count"] += 1
        metrics["min_mem_available_bytes"] = min(metrics["min_mem_available_bytes"], mem_available)
        metrics["peak_processes"] = max(metrics["peak_processes"], process_count)
        metrics["peak_rss_bytes"] = max(metrics["peak_rss_bytes"], rss_bytes)
        metrics["peak_open_fds"] = max(metrics["peak_open_fds"], open_fds)
        metrics["updated_unix"] = time.time()
        write_json_atomic(metrics_target, metrics)
        failed = [
            {"pid": child.pid, "returncode": child.poll(), "worker_id": item.get("worker_id")}
            for child, _log, item in children
            if child.poll() is not None
        ]
        if failed:
            print(json.dumps({"event": "worker_exited", "failed": failed[:20]}), flush=True)
            exit_code = 1
            break
        time.sleep(0.5)
finally:
    deadline = time.monotonic() + 20
    for child, _log, _item in children:
        if child.poll() is None:
            child.terminate()
    for child, _log, _item in children:
        remaining = max(0.0, deadline - time.monotonic())
        try:
            child.wait(timeout=remaining)
        except subprocess.TimeoutExpired:
            child.kill()
    for child, log, _item in children:
        if child.poll() is None:
            child.wait(timeout=5)
        log.close()
    metrics["finished_unix"] = time.time()
    metrics["exit_code"] = exit_code
    write_json_atomic(metrics_target, metrics)

raise SystemExit(exit_code)
