"""Validated, secret-free host inventory for isolated stress execution."""

from __future__ import annotations

from dataclasses import dataclass
import json
from pathlib import Path
from typing import Any


@dataclass(frozen=True)
class Host:
    ssh_host: str
    private_ip: str
    ssh_fingerprint_sha256: str

    @classmethod
    def from_document(cls, document: dict[str, Any]) -> "Host":
        host = cls(
            ssh_host=str(document.get("ssh_host", "")).strip(),
            private_ip=str(document.get("private_ip", "")).strip(),
            ssh_fingerprint_sha256=str(
                document.get("ssh_fingerprint_sha256", "")
            ).strip(),
        )
        if not host.ssh_host or not host.private_ip:
            raise ValueError("runtime host requires ssh_host and private_ip")
        if not host.ssh_fingerprint_sha256.startswith("SHA256:"):
            raise ValueError(
                f"{host.ssh_host}: ssh_fingerprint_sha256 must start with SHA256:"
            )
        return host


@dataclass(frozen=True)
class RuntimeInventory:
    server: Host
    workers: tuple[Host, ...]
    banned_worker_hosts: frozenset[str]
    protected_ports: tuple[int, ...]

    def worker_node_arguments(self) -> list[str]:
        return [f"{worker.ssh_host}:{worker.private_ip}" for worker in self.workers]


def load_runtime_inventory(path: Path) -> RuntimeInventory:
    document = json.loads(path.read_text(encoding="utf-8"))
    if document.get("schema_version") != 1:
        raise ValueError("runtime host inventory schema_version must be 1")
    forbidden_secret_keys = {
        "password",
        "worker_password",
        "server_password",
        "api_key",
        "token",
        "secret",
    }
    discovered_keys: set[str] = set()

    def visit(value: Any) -> None:
        if isinstance(value, dict):
            for key, child in value.items():
                discovered_keys.add(str(key).lower())
                visit(child)
        elif isinstance(value, list):
            for child in value:
                visit(child)

    visit(document)
    leaked_keys = forbidden_secret_keys & discovered_keys
    if leaked_keys:
        raise ValueError(
            f"runtime host inventory must not contain secret keys: {sorted(leaked_keys)}"
        )

    server_document = document.get("server")
    worker_documents = document.get("workers")
    if not isinstance(server_document, dict):
        raise ValueError("runtime host inventory requires a server object")
    if not isinstance(worker_documents, list) or not worker_documents:
        raise ValueError("runtime host inventory requires at least one worker")
    server = Host.from_document(server_document)
    workers = tuple(Host.from_document(item) for item in worker_documents)
    banned = frozenset(
        str(value).strip()
        for value in document.get("banned_worker_hosts", [])
        if str(value).strip()
    )
    if server.ssh_host in banned:
        raise ValueError("server host must not be in banned_worker_hosts")
    worker_hosts = [worker.ssh_host for worker in workers]
    if len(worker_hosts) != len(set(worker_hosts)):
        raise ValueError("runtime worker hosts must be unique")
    forbidden_workers = sorted(set(worker_hosts) & banned)
    if forbidden_workers:
        raise ValueError(
            f"banned worker hosts are present in active inventory: {forbidden_workers}"
        )
    production = document.get("production")
    if not isinstance(production, dict):
        raise ValueError("runtime host inventory requires a production object")
    protected_ports = tuple(int(port) for port in production.get("protected_ports", []))
    if not protected_ports or any(not 1 <= port <= 65535 for port in protected_ports):
        raise ValueError("production.protected_ports must contain valid TCP ports")
    return RuntimeInventory(
        server=server,
        workers=workers,
        banned_worker_hosts=banned,
        protected_ports=protected_ports,
    )
