#!/usr/bin/env python3
"""Local OpenHands agent-pool autoscaler for the 208.77 Agent node.

The supervisor runs on the Agent host. It does not require Server-side SSH or
Worker-side process control. It reads the Server's read-only fleet endpoints
and reconciles local systemd poller replicas to the observed SWE agent demand.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any
from urllib.error import URLError
from urllib.request import Request, urlopen


DEFAULT_ADMIN_BASE_URL = "http://8.130.75.157:8099"
DEFAULT_POOL_ID = "openhands-default"
DEFAULT_MAIN_SERVICE = "uenv-agent-poller.service"
DEFAULT_EXTRA_PREFIX = "uenv-agent-poller-extra"
DEFAULT_MAX_REPLICAS = 4
DEFAULT_MIN_REPLICAS = 1


@dataclass(frozen=True)
class Config:
    admin_base_url: str
    pool_id: str
    min_replicas: int
    max_replicas: int
    interval_sec: float
    main_service: str
    extra_prefix: str
    env_file: str
    runner_python: str
    runner_script: str
    gateway_local: str
    gateway_api_key: str
    runs_root: str
    api_base_port: int
    health_base_port: int
    dry_run: bool


def log(message: str) -> None:
    print(f"[agent-supervisor] {message}", flush=True)


def fetch_json(base_url: str, path: str, timeout: float = 5.0) -> dict[str, Any]:
    url = f"{base_url.rstrip('/')}/{path.lstrip('/')}"
    req = Request(url, headers={"Accept": "application/json"})
    try:
        with urlopen(req, timeout=timeout) as resp:  # noqa: S310 - configured internal URL
            return json.loads(resp.read().decode("utf-8"))
    except (OSError, URLError, json.JSONDecodeError) as exc:
        raise RuntimeError(f"fetch {url} failed: {exc}") from exc


def run(cmd: list[str], *, dry_run: bool = False, check: bool = True) -> subprocess.CompletedProcess[str]:
    log("$ " + " ".join(cmd))
    if dry_run:
        return subprocess.CompletedProcess(cmd, 0, "", "")
    return subprocess.run(cmd, text=True, capture_output=True, check=check)


def service_active(service: str) -> bool:
    if shutil.which("systemctl") is None:
        return False
    return subprocess.run(
        ["systemctl", "is-active", "--quiet", service],
        check=False,
    ).returncode == 0


def extra_service(prefix: str, slot: int) -> str:
    return f"{prefix}-{slot:02d}.service"


def managed_agent_id(slot: int | None) -> str:
    return "openhands-20877-main" if slot is None else f"openhands-20877-extra-{slot:02d}"


def write_extra_unit(cfg: Config, slot: int) -> Path:
    api_port = cfg.api_base_port + slot
    health_port = cfg.health_base_port + slot
    runs_dir = f"{cfg.runs_root}/openhands-extra-{slot:02d}"
    unit_path = Path(f"/etc/systemd/system/{extra_service(cfg.extra_prefix, slot)}")
    if not cfg.dry_run:
        Path(runs_dir).mkdir(parents=True, exist_ok=True)
    body = f"""[Unit]
Description=UEnv OpenHands Agent poller extra {slot:02d} (autoscaled)
After=network-online.target uenv-gateway-tunnel.service
Wants=network-online.target

[Service]
Type=simple
ExecStart=/bin/bash -lc 'set -a; . {cfg.env_file}; set +a; export OPENHANDS_AGENT_POLL=1 UENV_SERVER_ENDPOINT=8.130.75.157:8088 OPENHANDS_AGENT_POOL_ID={cfg.pool_id} OPENHANDS_AGENT_BRIDGE_ID=uenv-agent-openhands OPENHANDS_AGENT_BRIDGE_VERSION=1.0.0 OPENHANDS_AGENT_MAX_CONCURRENT=1 OPENHANDS_POLL_INTERVAL_SEC=1 OPENHANDS_HEARTBEAT_INTERVAL_SEC=5 UENV_AGENT_BRIDGE_DIR=/root/UEnv/integrations/openhands OPENHANDS_RUN_SCRIPT=/root/UEnv/scripts/run-openhands-pro-20877.sh UENV_GATEWAY_LOCAL={cfg.gateway_local} UENV_GATEWAY_API_KEY={cfg.gateway_api_key} OPENHANDS_AGENT_ID={managed_agent_id(slot)} OPENHANDS_RUNS_DIR={runs_dir} OPENHANDS_COMPLETION_SPOOL_DIR={runs_dir}/completion-spool OPENHANDS_RUNNER_API_BIND=127.0.0.1:{api_port} OPENHANDS_RUNNER_HEALTH_BIND=127.0.0.1:{health_port} OPENHANDS_AGENT_LABELS=role=openhands,slot=extra-{slot:02d},autoscaled=1; exec {cfg.runner_python} {cfg.runner_script}'
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
"""
    if cfg.dry_run:
        log(f"would write {unit_path}")
    else:
        unit_path.write_text(body, encoding="utf-8")
    return unit_path


def openhands_pool(agents: dict[str, Any], pool_id: str) -> dict[str, Any]:
    for pool in agents.get("pools") or []:
        if pool.get("agent_pool_id") == pool_id:
            return pool
    return {}


def agent_loads(agents: dict[str, Any], pool_id: str) -> dict[str, int]:
    out: dict[str, int] = {}
    for agent in agents.get("agents") or []:
        if agent.get("agent_pool_id") != pool_id or agent.get("stale"):
            continue
        agent_id = str(agent.get("agent_id") or "")
        out[agent_id] = int(agent.get("current_load") or agent.get("reserved_load") or 0)
    return out


def worker_capacity(workers: dict[str, Any]) -> int:
    total = int(workers.get("total_capacity") or 0)
    if total > 0:
        return total
    return sum(int(worker.get("capacity") or 0) for worker in workers.get("workers") or [])


def worker_load(workers: dict[str, Any]) -> int:
    return sum(int(worker.get("load") or 0) for worker in workers.get("workers") or [])


def desired_replicas(cfg: Config, agents: dict[str, Any], workers: dict[str, Any]) -> int:
    pool = openhands_pool(agents, cfg.pool_id)
    pending = int(pool.get("pending_jobs") or agents.get("pending_jobs") or 0)
    running = max(int(agents.get("running_jobs") or 0), int(pool.get("total_load") or 0))
    active_episodes = int(workers.get("active_episodes") or 0)
    demand = max(pending + running, active_episodes, worker_load(workers))
    capacity = max(1, worker_capacity(workers))
    if demand <= 0:
        return cfg.min_replicas
    return max(cfg.min_replicas, min(cfg.max_replicas, capacity, demand))


def active_managed_replicas(cfg: Config) -> int:
    count = 1 if service_active(cfg.main_service) else 0
    for slot in range(1, cfg.max_replicas):
        if service_active(extra_service(cfg.extra_prefix, slot)):
            count += 1
    return count


def ensure_main(cfg: Config) -> bool:
    if not service_active(cfg.main_service):
        run(["systemctl", "restart", cfg.main_service], dry_run=cfg.dry_run)
        return cfg.dry_run
    return True


def start_extra(cfg: Config, slot: int) -> None:
    write_extra_unit(cfg, slot)
    run(["systemctl", "daemon-reload"], dry_run=cfg.dry_run)
    service = extra_service(cfg.extra_prefix, slot)
    run(["systemctl", "enable", service], dry_run=cfg.dry_run)
    run(["systemctl", "restart", service], dry_run=cfg.dry_run)


def stop_extra(cfg: Config, slot: int) -> None:
    service = extra_service(cfg.extra_prefix, slot)
    run(["systemctl", "stop", service], dry_run=cfg.dry_run)
    run(["systemctl", "disable", service], dry_run=cfg.dry_run, check=False)


def reconcile(cfg: Config) -> None:
    agents = fetch_json(cfg.admin_base_url, "/fleet/agents")
    workers = fetch_json(cfg.admin_base_url, "/fleet/workers")
    desired = desired_replicas(cfg, agents, workers)
    current = active_managed_replicas(cfg)
    loads = agent_loads(agents, cfg.pool_id)
    pool = openhands_pool(agents, cfg.pool_id)
    log(
        "state "
        f"desired={desired} current={current} "
        f"pool_capacity={pool.get('total_capacity')} pool_load={pool.get('total_load')} "
        f"pending={pool.get('pending_jobs')} worker_capacity={worker_capacity(workers)} "
        f"worker_load={worker_load(workers)} active_episodes={workers.get('active_episodes')}"
    )

    if not ensure_main(cfg):
        log("main poller is not active yet; deferring extra replica reconciliation")
        return

    current = active_managed_replicas(cfg)
    if cfg.dry_run and current == 0:
        current = 1
    if current < desired:
        for slot in range(1, cfg.max_replicas):
            if current >= desired:
                break
            if not service_active(extra_service(cfg.extra_prefix, slot)):
                log(f"scale up slot={slot:02d}")
                start_extra(cfg, slot)
                current += 1

    elif current > desired:
        for slot in range(cfg.max_replicas - 1, 0, -1):
            if current <= desired:
                break
            service = extra_service(cfg.extra_prefix, slot)
            if not service_active(service):
                continue
            agent_id = managed_agent_id(slot)
            if loads.get(agent_id, 0) > 0:
                log(f"skip scale down busy slot={slot:02d} load={loads[agent_id]}")
                continue
            log(f"scale down idle slot={slot:02d}")
            stop_extra(cfg, slot)
            current -= 1


def parse_args() -> Config:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--once", action="store_true")
    p.add_argument("--dry-run", action="store_true")
    p.add_argument("--admin-base-url", default=os.environ.get("UENV_AGENT_SUPERVISOR_ADMIN_BASE_URL", DEFAULT_ADMIN_BASE_URL))
    p.add_argument("--pool-id", default=os.environ.get("OPENHANDS_AGENT_POOL_ID", DEFAULT_POOL_ID))
    p.add_argument("--min-replicas", type=int, default=int(os.environ.get("OPENHANDS_AGENT_AUTOSCALE_MIN", DEFAULT_MIN_REPLICAS)))
    p.add_argument("--max-replicas", type=int, default=int(os.environ.get("OPENHANDS_AGENT_AUTOSCALE_MAX", DEFAULT_MAX_REPLICAS)))
    p.add_argument("--interval-sec", type=float, default=float(os.environ.get("OPENHANDS_AGENT_AUTOSCALE_INTERVAL_SEC", "10")))
    p.add_argument("--main-service", default=os.environ.get("OPENHANDS_AGENT_MAIN_SERVICE", DEFAULT_MAIN_SERVICE))
    p.add_argument("--extra-prefix", default=os.environ.get("OPENHANDS_AGENT_EXTRA_PREFIX", DEFAULT_EXTRA_PREFIX))
    p.add_argument("--env-file", default=os.environ.get("OPENHANDS_AGENT_ENV_FILE", "/root/.openhands-20877.env"))
    p.add_argument("--runner-python", default=os.environ.get("OPENHANDS_AGENT_RUNNER_PYTHON", "/root/uenv-agent-venv/bin/python"))
    p.add_argument("--runner-script", default=os.environ.get("OPENHANDS_AGENT_RUNNER_SCRIPT", "/root/UEnv/scripts/openhands/openhands_runner.py"))
    p.add_argument("--gateway-local", default=os.environ.get("UENV_GATEWAY_LOCAL", "http://127.0.0.1:28097"))
    p.add_argument("--gateway-api-key", default=os.environ.get("UENV_GATEWAY_API_KEY", "swe-pro-secret"))
    p.add_argument("--runs-root", default=os.environ.get("OPENHANDS_AGENT_RUNS_ROOT", "/var/log/uenv"))
    p.add_argument("--api-base-port", type=int, default=int(os.environ.get("OPENHANDS_AGENT_API_BASE_PORT", "8888")))
    p.add_argument("--health-base-port", type=int, default=int(os.environ.get("OPENHANDS_AGENT_HEALTH_BASE_PORT", "8777")))
    args = p.parse_args()
    if args.min_replicas < 1:
        p.error("--min-replicas must be >= 1")
    if args.max_replicas < args.min_replicas:
        p.error("--max-replicas must be >= --min-replicas")
    if args.max_replicas > 32:
        p.error("--max-replicas > 32 is intentionally unsupported")
    return Config(
        admin_base_url=args.admin_base_url,
        pool_id=args.pool_id,
        min_replicas=args.min_replicas,
        max_replicas=args.max_replicas,
        interval_sec=args.interval_sec,
        main_service=args.main_service,
        extra_prefix=args.extra_prefix,
        env_file=args.env_file,
        runner_python=args.runner_python,
        runner_script=args.runner_script,
        gateway_local=args.gateway_local,
        gateway_api_key=args.gateway_api_key,
        runs_root=args.runs_root,
        api_base_port=args.api_base_port,
        health_base_port=args.health_base_port,
        dry_run=args.dry_run,
    )


def main() -> int:
    cfg = parse_args()
    once = "--once" in sys.argv
    while True:
        try:
            reconcile(cfg)
        except Exception as exc:  # noqa: BLE001
            log(f"reconcile failed: {type(exc).__name__}: {exc}")
            if once:
                return 1
        if once:
            return 0
        time.sleep(cfg.interval_sec)


if __name__ == "__main__":
    raise SystemExit(main())
