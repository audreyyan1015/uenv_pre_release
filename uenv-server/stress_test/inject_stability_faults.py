#!/usr/bin/env python3
"""Inject only manifest-owned stability faults and always remove nftables rules."""

from __future__ import annotations

import argparse
import csv
import json
import os
import random
import signal
import socket
import sqlite3
import subprocess
import time
from pathlib import Path
from typing import Any


FAULTS = ("worker_exit", "worker_network", "node_isolation")


def run(command: list[str], *, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(command, check=check, text=True, capture_output=True)


def tcp_ready(host: str, port: int, timeout: float = 1.0) -> bool:
    try:
        with socket.create_connection((host, port), timeout=timeout):
            return True
    except OSError:
        return False


def render_argv(template: list[Any], context: dict[str, Any]) -> list[str]:
    return [str(value).format(**context) for value in template]


def probe_json(template: list[Any], context: dict[str, Any]) -> dict[str, Any]:
    completed = run(render_argv(template, context))
    value = json.loads(completed.stdout)
    if not isinstance(value, dict):
        raise ValueError("probe command must emit one JSON object")
    return value


def wait_recovery(
    probe_argv: list[Any],
    context: dict[str, Any],
    timeout: float,
) -> tuple[float, dict[str, Any]]:
    started = time.monotonic()
    deadline = started + timeout
    last: dict[str, Any] = {}
    while time.monotonic() < deadline:
        try:
            last = probe_json(probe_argv, context)
        except (OSError, subprocess.SubprocessError, json.JSONDecodeError, ValueError):
            last = {}
        if (
            bool(last.get("connected"))
            and bool(last.get("registered"))
            and int(last.get("consecutive_episode_successes", 0)) >= 3
            and bool(last.get("checksums_consistent"))
        ):
            return time.monotonic() - started, last
        time.sleep(1)
    return time.monotonic() - started, last


def affected_consistency(
    database: Path,
    start: float,
    end: float,
    seed_ids: list[str] | None = None,
) -> tuple[list[str], int, int, int]:
    connection = sqlite3.connect(f"file:{database}?mode=ro", uri=True)
    try:
        dispatched_rows = connection.execute(
            """SELECT request_id,failure_class,terminal_count FROM episode
               WHERE dispatch_started=1 AND dispatched_at BETWEEN ? AND ?""",
            (start, end),
        ).fetchall()
        ids = sorted({str(row[0]) for row in dispatched_rows} | set(seed_ids or []))
        if ids:
            placeholders = ",".join("?" for _ in ids)
            rows = connection.execute(
                f"""SELECT request_id,failure_class,terminal_count FROM episode
                    WHERE request_id IN ({placeholders})""",
                ids,
            ).fetchall()
            events = connection.execute(
                f"""SELECT request_id,count(*),count(DISTINCT NULLIF(result_checksum,''))
                    FROM episode_terminal_event
                    WHERE request_id IN ({placeholders})
                    GROUP BY request_id""",
                ids,
            ).fetchall()
        else:
            rows, events = [], []
    finally:
        connection.close()
    lost = sum(1 for _request_id, failure, _count in rows if failure == "no_terminal_result")
    duplicates = sum(1 for _request_id, failure, count in rows if failure == "duplicate_terminal_result" or int(count) > 1)
    checksum_mismatches = sum(1 for _request_id, _count, distinct in events if int(distinct) > 1)
    return ids, lost, duplicates, checksum_mismatches


def state_regression_count(server_log: Path) -> int:
    markers = ("state_regression", "terminal_to_nonterminal", "status_regressed")
    return sum(
        1 for line in server_log.read_text(encoding="utf-8", errors="replace").splitlines()
        if any(marker in line.lower() for marker in markers)
    )


def append_fault(path: Path, row: dict[str, Any]) -> None:
    exists = path.exists() and path.stat().st_size > 0
    with path.open("a", encoding="utf-8", newline="") as target:
        writer = csv.DictWriter(target, fieldnames=list(row))
        if not exists:
            writer.writeheader()
        writer.writerow(row)


def nft_table_name(run_id: str) -> str:
    safe = "".join(character for character in run_id.lower() if character.isalnum())[:20]
    return "uenv_stab_" + safe


def network_fault(table: str, target_host: str, target_port: int | None, seconds: int, comment: str) -> None:
    run(["nft", "add", "table", "inet", table])
    run(["nft", "add", "chain", "inet", table, "output", "{ type filter hook output priority -10; policy accept; }"])
    command = ["nft", "add", "rule", "inet", table, "output", "ip", "daddr", target_host]
    if target_port is not None:
        command.extend(["tcp", "dport", str(target_port)])
    command.extend(["drop", "comment", comment])
    run(command)
    time.sleep(seconds)


def validate_fleet(path: Path, run_id: str) -> dict[str, Any]:
    fleet = json.loads(path.read_text(encoding="utf-8"))
    if str(fleet.get("run_id")) != run_id or not bool(fleet.get("test_only")):
        raise ValueError("fleet manifest must match --run-id and set test_only=true")
    workers = fleet.get("workers")
    if not workers or not all(worker.get("owned") for worker in workers):
        raise ValueError("every fault target Worker must be explicitly owned by this test")
    return fleet


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--fleet-manifest", type=Path, required=True)
    parser.add_argument("--ledger-db", type=Path, required=True)
    parser.add_argument("--fault-csv", type=Path, required=True)
    parser.add_argument("--server-log", type=Path, required=True)
    parser.add_argument("--fault", choices=FAULTS, action="append")
    parser.add_argument("--repetitions", type=int, default=5)
    parser.add_argument("--seed", type=int, default=20260722)
    parser.add_argument("--execute", action="store_true")
    args = parser.parse_args()
    faults = args.fault or list(FAULTS)
    if args.repetitions != 5:
        raise ValueError("formal fault phase requires exactly five repetitions per fault")
    fleet = validate_fleet(args.fleet_manifest, args.run_id)
    workers = list(fleet["workers"])
    rng = random.Random(args.seed)
    plans = []
    for fault in faults:
        for repetition in range(args.repetitions):
            plans.append({"fault": fault, "repetition": repetition + 1, "worker": rng.choice(workers)})
    if not args.execute:
        print(json.dumps(plans, indent=2, sort_keys=True))
        return 0
    if os.geteuid() != 0:
        raise PermissionError("fault injection requires root for owned PID signals and nftables")
    args.fault_csv.parent.mkdir(parents=True, exist_ok=True)
    table = nft_table_name(args.run_id)
    for field in (
        "inflight_probe_argv", "recovery_probe_argv",
        "network_fault_argv", "network_restore_argv", "rollback_watchdog_argv",
    ):
        if not isinstance(fleet.get(field), list) or not fleet[field]:
            raise ValueError(f"formal fault execution fleet manifest requires {field}")
    watchdog_context = {
        "run_id": args.run_id,
        "fault": "watchdog",
        "worker_id": "",
        "target_host": "",
        "target_port": 0,
        "duration_seconds": 0,
    }
    watchdog = subprocess.Popen(
        render_argv(fleet["rollback_watchdog_argv"], watchdog_context),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        for plan in plans:
            fault, worker = plan["fault"], plan["worker"]
            target_host, target_port = str(worker["host"]), int(worker["port"])
            context = {
                "run_id": args.run_id,
                "fault": fault,
                "worker_id": worker["worker_id"],
                "target_host": target_host,
                "target_port": target_port,
                "duration_seconds": 60 if fault == "worker_network" else 120,
            }
            inflight = probe_json(fleet["inflight_probe_argv"], context)
            inflight_ids = [str(value) for value in inflight.get("inflight_episode_ids", [])]
            if not inflight_ids or str(inflight.get("worker_id", "")) != str(worker["worker_id"]):
                raise RuntimeError(
                    f"fault target {worker['worker_id']} has no verified in-flight Episode"
                )
            injected = time.time()
            regression_before = state_regression_count(args.server_log)
            cleared = injected
            timeout = 120.0 if fault != "node_isolation" else 300.0
            if fault == "worker_exit":
                if str(worker.get("host")) not in {"127.0.0.1", "localhost", socket.gethostname()}:
                    raise ValueError("worker_exit must be executed on the Worker host; remote PID killing is refused")
                os.kill(int(worker["pid"]), signal.SIGKILL)
            else:
                seconds = 60 if fault == "worker_network" else 120
                run(render_argv(fleet["network_fault_argv"], context))
                time.sleep(seconds)
                cleared = time.time()
                run(render_argv(fleet["network_restore_argv"], context), check=False)
            recovery_seconds, recovery = wait_recovery(
                fleet["recovery_probe_argv"], context, timeout
            )
            recovered = (
                recovery_seconds <= timeout
                and bool(recovery.get("connected"))
                and bool(recovery.get("registered"))
                and int(recovery.get("consecutive_episode_successes", 0)) >= 3
                and bool(recovery.get("checksums_consistent"))
            )
            affected_ids, lost, duplicates, checksum_mismatches = affected_consistency(
                args.ledger_db, injected - 1, time.time(), inflight_ids
            )
            regressions = state_regression_count(args.server_log) - regression_before
            append_fault(args.fault_csv, {
                "run_id": args.run_id, "fault_type": fault, "repetition": plan["repetition"],
                "target_worker_id": worker["worker_id"], "target_host": target_host,
                "target_port": target_port, "injected_at": injected, "cleared_at": cleared,
                "recovered_at": time.time() if recovered else "", "recovery_seconds": recovery_seconds,
                "first_healthy_at": recovery.get("first_healthy_at", ""),
                "registered_at": recovery.get("registered_at", ""),
                "three_episode_successes_at": recovery.get("three_episode_successes_at", ""),
                "automatic_recovery": recovered, "affected_episode_ids": json.dumps(affected_ids),
                "lost_results": lost, "duplicate_results": duplicates,
                "checksum_mismatches": checksum_mismatches,
                "state_regressions": regressions,
            })
    finally:
        for plan in plans:
            worker = plan["worker"]
            context = {
                "run_id": args.run_id,
                "fault": plan["fault"],
                "worker_id": worker["worker_id"],
                "target_host": worker["host"],
                "target_port": worker["port"],
                "duration_seconds": 60 if plan["fault"] == "worker_network" else 120,
            }
            if plan["fault"] != "worker_exit" and isinstance(fleet.get("network_restore_argv"), list):
                run(render_argv(fleet["network_restore_argv"], context), check=False)
        run(["nft", "delete", "table", "inet", table], check=False)
        watchdog.terminate()
        try:
            watchdog.wait(timeout=10)
        except subprocess.TimeoutExpired:
            watchdog.kill()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
