#!/usr/bin/env python3
"""大规模本机 worker 进程监督器。

这个文件实现把大量真实 uenv-worker 子进程作为同一个测试进程组管理的能力，用于压测时在一台主机上启动、监控和停止 worker fleet。它关注进程生命周期和资源记录，不负责决定业务样本或验收标准。

实现逻辑是：启动后读取 worker 配置和输出目录，按计划启动多个 worker 子进程；通过信号处理 request_stop 捕获停止请求；周期性用 process_group_metrics 采集进程组 CPU、内存和子进程数量，append_resource_row 写入 CSV；write_json_atomic 用临时文件加替换的方式写 fleet 状态，避免中途写坏状态文件；退出时按同一进程组清理本次启动的 worker。"""

from __future__ import annotations

import argparse
import csv
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
parser.add_argument("--resource-csv", default="")
parser.add_argument("--resource-sample-seconds", type=float, default=30.0)
parser.add_argument("--run-id", default="")
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


def load_averages() -> tuple[float, float, float]:
    values = Path("/proc/loadavg").read_text(encoding="utf-8").split()
    return float(values[0]), float(values[1]), float(values[2])


def process_group_metrics() -> tuple[int, int, int, int]:
    """Return process count, RSS bytes, open FD count and thread count."""
    process_count = 0
    rss_bytes = 0
    open_fds = 0
    threads = 0
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
                elif line.startswith("Threads:"):
                    threads += int(line.split()[1])
            open_fds += sum(1 for _ in (proc_dir / "fd").iterdir())
        except (FileNotFoundError, PermissionError, ProcessLookupError, ValueError, IndexError):
            continue
    return process_count, rss_bytes, open_fds, threads


def append_resource_row(path: Path, row: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    exists = path.exists() and path.stat().st_size > 0
    with path.open("a", encoding="utf-8", newline="") as target:
        writer = csv.DictWriter(target, fieldnames=list(row))
        if not exists:
            writer.writeheader()
        writer.writerow(row)


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
    "initial_mem_available_bytes": mem_available,
    "min_mem_available_bytes": mem_available,
    "peak_processes": 0,
    "peak_rss_bytes": 0,
    "peak_open_fds": 0,
    "peak_threads": 0,
    "max_load1": 0.0,
    "max_load5": 0.0,
    "max_load15": 0.0,
    "worker_exit_count": 0,
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

    next_resource_sample = 0.0
    while not stopping:
        process_count, rss_bytes, open_fds, threads = process_group_metrics()
        mem_total, mem_available = memory_bytes()
        load1, load5, load15 = load_averages()
        metrics["sample_count"] += 1
        metrics["min_mem_available_bytes"] = min(metrics["min_mem_available_bytes"], mem_available)
        metrics["peak_processes"] = max(metrics["peak_processes"], process_count)
        metrics["peak_rss_bytes"] = max(metrics["peak_rss_bytes"], rss_bytes)
        metrics["peak_open_fds"] = max(metrics["peak_open_fds"], open_fds)
        metrics["peak_threads"] = max(metrics["peak_threads"], threads)
        metrics["max_load1"] = max(metrics["max_load1"], load1)
        metrics["max_load5"] = max(metrics["max_load5"], load5)
        metrics["max_load15"] = max(metrics["max_load15"], load15)
        metrics["updated_unix"] = time.time()
        write_json_atomic(metrics_target, metrics)
        failed = [
            {"pid": child.pid, "returncode": child.poll(), "worker_id": item.get("worker_id")}
            for child, _log, item in children
            if child.poll() is not None
        ]
        if failed:
            metrics["worker_exit_count"] += len(failed)
            print(json.dumps({"event": "worker_exited", "failed": failed[:20]}), flush=True)
            exit_code = 1
            break
        if args.resource_csv and time.monotonic() >= next_resource_sample:
            append_resource_row(Path(args.resource_csv), {
                "timestamp": time.time(), "run_id": args.run_id, "processes": process_count,
                "rss_bytes": rss_bytes, "open_fds": open_fds, "threads": threads,
                "mem_total_bytes": mem_total, "available_bytes": mem_available,
                "load1": load1, "load5": load5, "load15": load15,
                "worker_exits": metrics["worker_exit_count"], "oom_events": 0,
            })
            next_resource_sample = time.monotonic() + args.resource_sample_seconds
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
    remaining_processes, remaining_rss, remaining_fds, remaining_threads = process_group_metrics()
    mem_total, mem_available = memory_bytes()
    load1, load5, load15 = load_averages()
    metrics["cleanup"] = {
        "remaining_processes": remaining_processes,
        "remaining_rss_bytes": remaining_rss,
        "remaining_open_fds": remaining_fds,
        "remaining_threads": remaining_threads,
    }
    if args.resource_csv:
        append_resource_row(Path(args.resource_csv), {
            "timestamp": time.time(), "run_id": args.run_id, "processes": remaining_processes,
            "rss_bytes": remaining_rss, "open_fds": remaining_fds, "threads": remaining_threads,
            "mem_total_bytes": mem_total, "available_bytes": mem_available,
            "load1": load1, "load5": load5, "load15": load15,
            "worker_exits": metrics["worker_exit_count"], "oom_events": 0,
        })
    write_json_atomic(metrics_target, metrics)

raise SystemExit(exit_code)
