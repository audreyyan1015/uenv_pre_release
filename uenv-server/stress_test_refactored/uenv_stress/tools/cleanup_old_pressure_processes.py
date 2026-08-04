#!/usr/bin/env python3
"""Safely stop stale UEnv pressure-test processes on the test fleet.

The script is dry-run by default.  Pass ``--execute`` to send signals and
remove Docker containers carrying the pressure-run ownership label.  It never
deletes run directories or artifacts, and it snapshots the production adapter
listeners before making any change.
"""

from __future__ import annotations

import argparse
import dataclasses
import getpass
import os
import re
import shlex
import sys
import time
from collections.abc import Iterable

from uenv_stress.core import distributed_runtime as runtime


SERVER_HOST = "8.130.75.157"
WORKER_HOSTS = ("8.130.65.20", "8.145.51.129")
PROTECTED_PORTS = (50052, 50053, 8077, 8088)
SWE_CONTAINER_RUN_LABEL = "io.uenv.swe.run_id"
SERVER_RUN_ROOT = "/tmp/uenv-"
WORKER_RUN_ROOT = "/opt/uenv-stress/runs/"
ISOLATED_ADAPTER_BIN = "/home/uenv-frontend-add/target/release/uenv-adapter-core"
ISOLATED_CONFIG_RE = re.compile(r"^/tmp/uenv-([^/]+)/server\.yaml$")
PRESSURE_ENTRY_MARKERS = (
    "scale/swebench_pro_pressure.py",
    "scale/dscodebench_pressure.py",
    "scale/rule_task_pressure.py",
    "cli/run_scale_suite.py",
    "uenv_stress.scale.swebench_pro_pressure",
    "uenv_stress.scale.dscodebench_pressure",
    "uenv_stress.scale.rule_task_pressure",
    "uenv_stress.cli.run_scale_suite",
)
PID_RE = re.compile(r"pid=(\d+)")
CONTAINER_ID_RE = re.compile(r"^[0-9a-f]{12,64}$")


@dataclasses.dataclass(frozen=True)
class ProcessRecord:
    host: str
    pid: int
    pgid: int
    elapsed_seconds: int
    command: str
    reason: str
    run_id: str = ""


@dataclasses.dataclass(frozen=True)
class ContainerRecord:
    host: str
    container_id: str
    run_id: str
    name: str
    status: str


def shell_quote(value: object) -> str:
    return shlex.quote(str(value))


def run_id_from_command_root(command: str, root: str) -> str:
    match = re.search(re.escape(root) + r"([^/\s]+)", command)
    return match.group(1) if match else ""


def isolated_adapter_run_id(client, pid: int, command: str) -> str:
    """Return the run ID of an env-configured isolated adapter, if any.

    The adapter is launched with only the binary in argv.  Its ownership marker
    lives in UENV_CONFIG_PATH, so command-line-only discovery misses it.  Both
    the executable path and the config path must match the pressure-owned
    locations; the production binary under /usr/local/bin can never match.
    """
    argv0 = command.split(None, 1)[0] if command.strip() else ""
    if argv0 != ISOLATED_ADAPTER_BIN:
        return ""
    try:
        if runtime.process_exe(client, pid) != ISOLATED_ADAPTER_BIN:
            return ""
        with client.open_sftp() as sftp:
            with sftp.open(f"/proc/{pid}/environ", "rb") as remote:
                raw_environment = remote.read()
    except OSError:
        # The process may have exited between ps and /proc inspection.
        return ""
    environment: dict[str, str] = {}
    for item in raw_environment.split(b"\0"):
        if b"=" not in item:
            continue
        key, value = item.split(b"=", 1)
        environment[key.decode(errors="replace")] = value.decode(errors="replace")
    match = ISOLATED_CONFIG_RE.fullmatch(environment.get("UENV_CONFIG_PATH", ""))
    return match.group(1) if match else ""


def process_ownership(client, pid: int, command: str, *, server: bool) -> tuple[str, str] | None:
    worker_run_id = run_id_from_command_root(command, WORKER_RUN_ROOT)
    if worker_run_id:
        return "worker pressure run directory", worker_run_id
    server_run_id = run_id_from_command_root(command, SERVER_RUN_ROOT) if server else ""
    if server_run_id:
        return "isolated server pressure run directory", server_run_id
    if server:
        adapter_run_id = isolated_adapter_run_id(client, pid, command)
        if adapter_run_id:
            return "isolated adapter UENV_CONFIG_PATH", adapter_run_id
    if server and any(marker in command for marker in PRESSURE_ENTRY_MARKERS):
        return "pressure orchestrator entry", ""
    return None


def discover_processes(client, host: str, *, server: bool) -> list[ProcessRecord]:
    _, output, _ = runtime.run(
        client,
        "ps -eo pid=,pgid=,etimes=,args=",
        timeout=20,
    )
    records: list[ProcessRecord] = []
    for raw_line in output.splitlines():
        fields = raw_line.strip().split(None, 3)
        if len(fields) != 4:
            continue
        pid_text, pgid_text, elapsed_text, command = fields
        if not (pid_text.isdigit() and pgid_text.isdigit() and elapsed_text.isdigit()):
            continue
        ownership = process_ownership(client, int(pid_text), command, server=server)
        if ownership is None:
            continue
        reason, run_id = ownership
        records.append(
            ProcessRecord(
                host=host,
                pid=int(pid_text),
                pgid=int(pgid_text),
                elapsed_seconds=int(elapsed_text),
                command=command,
                reason=reason,
                run_id=run_id,
            )
        )
    return records


def protected_snapshot(client) -> dict[str, object]:
    _, listeners, _ = runtime.run(client, "ss -H -lntp", timeout=20)
    records: dict[str, str] = {}
    pids: set[int] = set()
    for port in PROTECTED_PORTS:
        matches = [line for line in listeners.splitlines() if f":{port} " in line]
        if len(matches) != 1:
            raise RuntimeError(
                f"protected port {port} must have exactly one listener, got {matches}"
            )
        observed = {int(value) for value in PID_RE.findall(matches[0])}
        if len(observed) != 1:
            raise RuntimeError(f"protected port {port} has ambiguous owner: {matches[0]}")
        pids.update(observed)
        records[str(port)] = matches[0]
    if len(pids) != 1:
        raise RuntimeError(f"protected ports are not owned by one process: {sorted(pids)}")
    pid = next(iter(pids))
    return {
        "pid": pid,
        "exe": runtime.process_exe(client, pid),
        "cmdline": runtime.process_cmdline(client, pid),
        "starttime_ticks": runtime.process_starttime(client, pid),
        "listeners": records,
    }


def discover_containers(client, host: str) -> list[ContainerRecord]:
    command = (
        "docker ps -a --filter "
        f"label={shell_quote(SWE_CONTAINER_RUN_LABEL)} "
        "--format '{{.ID}}\t{{.Names}}\t{{.Status}}'"
    )
    status, output, error = runtime.run(client, command, timeout=30, check=False)
    if status != 0:
        if "command not found" in error or "Cannot connect to the Docker daemon" in error:
            print(f"[warn] {host}: Docker unavailable: {error.strip()}")
            return []
        raise RuntimeError(f"{host}: docker discovery failed: {error.strip()}")
    records: list[ContainerRecord] = []
    for line in output.splitlines():
        fields = line.split("\t", 2)
        if len(fields) != 3 or not CONTAINER_ID_RE.fullmatch(fields[0]):
            continue
        container_id, name, container_status = fields
        inspect_format = '{{ index .Config.Labels "io.uenv.swe.run_id" }}'
        _, run_id, _ = runtime.run(
            client,
            "docker inspect --format "
            + shell_quote(inspect_format)
            + " "
            + shell_quote(container_id),
            timeout=20,
        )
        run_id = run_id.strip()
        if not run_id:
            raise RuntimeError(f"{host}: labeled container {container_id} has empty run_id")
        records.append(
            ContainerRecord(
                host=host,
                container_id=container_id,
                run_id=run_id,
                name=name,
                status=container_status,
            )
        )
    return records


def filter_run_ids(records: Iterable, run_ids: set[str]) -> list:
    if not run_ids:
        return list(records)
    selected = []
    for record in records:
        haystacks = (getattr(record, "run_id", ""), getattr(record, "command", ""))
        if any(run_id in haystack for run_id in run_ids for haystack in haystacks):
            selected.append(record)
    return selected


def signal_processes(client, records: list[ProcessRecord], signal_name: str) -> None:
    if not records:
        return
    pids = sorted({record.pid for record in records if record.pid > 1})
    command = "kill -" + signal_name + " -- " + " ".join(str(pid) for pid in pids)
    runtime.run(client, command, timeout=20, check=False)
    print(f"[signal] {records[0].host}: {signal_name} pids={pids}")


def remove_containers(client, records: list[ContainerRecord]) -> None:
    if not records:
        return
    container_ids = sorted({record.container_id for record in records})
    if not all(CONTAINER_ID_RE.fullmatch(value) for value in container_ids):
        raise RuntimeError("refusing invalid Docker container ID")
    runtime.run(
        client,
        "docker rm -f -- " + " ".join(shell_quote(value) for value in container_ids),
        timeout=180,
    )
    print(f"[container] {records[0].host}: removed IDs={container_ids}")


def print_inventory(
    processes_by_host: dict[str, list[ProcessRecord]],
    containers_by_host: dict[str, list[ContainerRecord]],
) -> None:
    for host in (SERVER_HOST, *WORKER_HOSTS):
        processes = processes_by_host.get(host, [])
        containers = containers_by_host.get(host, [])
        print(f"[inventory] host={host} processes={len(processes)} containers={len(containers)}")
        for record in processes:
            print(
                f"  process pid={record.pid} pgid={record.pgid} age={record.elapsed_seconds}s "
                f"run_id={record.run_id or '-'} reason={record.reason} cmd={record.command}"
            )
        for record in containers:
            print(
                f"  container id={record.container_id} run_id={record.run_id} "
                f"name={record.name} status={record.status}"
            )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Preview or stop stale pressure-owned processes on the UEnv test fleet."
    )
    parser.add_argument(
        "--execute",
        action="store_true",
        help="Actually send TERM/KILL and remove run-labeled containers. Default is dry-run.",
    )
    parser.add_argument(
        "--run-id",
        action="append",
        default=[],
        help="Only clean this run ID. Repeat as needed. Omit to clean all pressure-owned runs.",
    )
    parser.add_argument(
        "--grace-seconds",
        type=float,
        default=8.0,
        help="Seconds to wait after SIGTERM before SIGKILL (default: 8).",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.grace_seconds < 0 or args.grace_seconds > 120:
        raise SystemExit("--grace-seconds must be between 0 and 120")
    run_ids = {value.strip() for value in args.run_id if value.strip()}
    password = os.environ.get("UENV_PASS") or getpass.getpass("SSH password for root@test fleet: ")
    clients = {}
    before_protected: dict[str, object] | None = None
    try:
        for host in (SERVER_HOST, *WORKER_HOSTS):
            clients[host] = runtime.connect(host, password)

        before_protected = protected_snapshot(clients[SERVER_HOST])
        protected_pid = int(before_protected["pid"])
        processes_by_host = {
            SERVER_HOST: filter_run_ids(
                discover_processes(clients[SERVER_HOST], SERVER_HOST, server=True), run_ids
            )
        }
        containers_by_host: dict[str, list[ContainerRecord]] = {SERVER_HOST: []}
        for host in WORKER_HOSTS:
            processes_by_host[host] = filter_run_ids(
                discover_processes(clients[host], host, server=False), run_ids
            )
            containers_by_host[host] = filter_run_ids(
                discover_containers(clients[host], host), run_ids
            )

        if any(record.pid == protected_pid for record in processes_by_host[SERVER_HOST]):
            raise RuntimeError(
                f"refusing to clean protected production PID {protected_pid}; matcher is too broad"
            )
        print_inventory(processes_by_host, containers_by_host)
        total_processes = sum(len(records) for records in processes_by_host.values())
        total_containers = sum(len(records) for records in containers_by_host.values())
        if not args.execute:
            print(
                f"[dry-run] would stop {total_processes} processes and remove "
                f"{total_containers} labeled containers; rerun with --execute to apply"
            )
            return 0

        # Stop orchestrators first so they cannot launch additional owned work.
        server_processes = processes_by_host[SERVER_HOST]
        orchestrators = [r for r in server_processes if r.reason == "pressure orchestrator entry"]
        signal_processes(clients[SERVER_HOST], orchestrators, "TERM")
        if orchestrators:
            time.sleep(min(args.grace_seconds, 5.0))

        # Rescan after orchestrator cleanup, then terminate all remaining owned processes.
        for host in (SERVER_HOST, *WORKER_HOSTS):
            remaining = filter_run_ids(
                discover_processes(clients[host], host, server=(host == SERVER_HOST)), run_ids
            )
            if any(record.pid == protected_pid for record in remaining):
                raise RuntimeError(f"refusing protected production PID {protected_pid}")
            signal_processes(clients[host], remaining, "TERM")

        time.sleep(args.grace_seconds)
        for host in (SERVER_HOST, *WORKER_HOSTS):
            remaining = filter_run_ids(
                discover_processes(clients[host], host, server=(host == SERVER_HOST)), run_ids
            )
            signal_processes(clients[host], remaining, "KILL")

        for host in WORKER_HOSTS:
            current_containers = filter_run_ids(
                discover_containers(clients[host], host), run_ids
            )
            remove_containers(clients[host], current_containers)

        time.sleep(0.5)
        leftovers: list[str] = []
        for host in (SERVER_HOST, *WORKER_HOSTS):
            remaining = filter_run_ids(
                discover_processes(clients[host], host, server=(host == SERVER_HOST)), run_ids
            )
            leftovers.extend(f"{host}:pid={record.pid}:{record.command}" for record in remaining)
        for host in WORKER_HOSTS:
            remaining = filter_run_ids(discover_containers(clients[host], host), run_ids)
            leftovers.extend(
                f"{host}:container={record.container_id}:run_id={record.run_id}"
                for record in remaining
            )

        after_protected = protected_snapshot(clients[SERVER_HOST])
        if after_protected != before_protected:
            raise RuntimeError(
                "production adapter snapshot changed during cleanup; inspect immediately"
            )
        if leftovers:
            print("[verify] leftovers remain:", file=sys.stderr)
            for item in leftovers:
                print(f"  {item}", file=sys.stderr)
            return 1
        print(
            "[verify] cleanup complete; no matching processes/containers remain; "
            f"production PID {protected_pid} and ports {PROTECTED_PORTS} are unchanged"
        )
        return 0
    finally:
        for client in clients.values():
            client.close()


if __name__ == "__main__":
    raise SystemExit(main())
