"""Workspace self-check for SWE-bench-Pro Agent runs (Gateway container vs Agent host)."""

from __future__ import annotations

import re
from typing import Any

_PROBE_CMD = (
    "pwd && "
    "git -C /app rev-parse --show-toplevel 2>/dev/null && "
    "git -C /app rev-parse HEAD 2>/dev/null && "
    "(git -C /app remote get-url origin 2>/dev/null || git -C /app remote -v 2>/dev/null | head -1) && "
    "ls -la /app 2>/dev/null | head -25"
)


def probe_workspace(ws: Any, repo_path: str = "/app") -> dict[str, Any]:
    """Run workspace probe via gateway-backed workspace.execute_command."""
    cmd = _PROBE_CMD.replace("/app", repo_path.rstrip("/") or "/app")
    r = ws.execute_command(cmd, cwd=repo_path)
    stdout = (r.stdout or "") + ("\n" + r.stderr if r.stderr else "")
    return {
        "exit_code": r.exit_code,
        "stdout": r.stdout or "",
        "stderr": r.stderr or "",
        "combined": stdout.strip(),
    }


def _repo_slug(repo: str) -> str:
    repo = (repo or "").strip().lower()
    if "/" in repo:
        repo = repo.split("/")[-1]
    return re.sub(r"[^a-z0-9]+", "", repo)


def validate_workspace_probe(
    probe: dict[str, Any],
    *,
    instance_id: str,
    repo: str,
    base_commit: str = "",
) -> tuple[bool, str]:
    """Return (ok, reason). Hard-fail when container content clearly wrong."""
    combined = probe.get("combined") or probe.get("stdout") or ""
    lower = combined.lower()
    repo_l = (repo or "").lower()
    slug = _repo_slug(repo)

    if probe.get("exit_code", 0) != 0 and not combined.strip():
        return False, "workspace probe command failed with empty output"

    # Cross-repo contamination: openlibrary paths on non-openlibrary tasks.
    if "openlibrary" in lower and "internetarchive/openlibrary" not in repo_l:
        return False, "probe output contains openlibrary but instance repo is not openlibrary"

    if slug and slug not in ("", "openlibrary"):
        # Heuristic: qutebrowser repo should mention qutebrowser in ls or remote.
        if "qutebrowser" in repo_l and "qutebrowser" not in lower and "openlibrary" in lower:
            return False, "qutebrowser instance but probe looks like openlibrary"

    if base_commit:
        head = ""
        for line in combined.splitlines():
            line = line.strip()
            if re.fullmatch(r"[0-9a-f]{7,40}", line):
                head = line
                break
        if head and not (head.startswith(base_commit[:7]) or base_commit.startswith(head[:7])):
            return False, f"git HEAD {head} does not match base_commit {base_commit[:12]}"

    if not combined.strip():
        return False, "workspace probe produced empty output"

    return True, "ok"


def merge_reset_observation(
    session_observation: dict[str, Any],
    probe: dict[str, Any],
    *,
    instance_id: str,
    session_id: str | None,
    repo: str,
    base_commit: str = "",
    ok: bool,
    reason: str,
) -> dict[str, Any]:
    return {
        **(session_observation or {}),
        "instance_id": instance_id,
        "session_id": session_id,
        "repo": repo,
        "base_commit": base_commit,
        "workspace_probe_ok": ok,
        "workspace_probe_reason": reason,
        "workspace_probe": probe,
    }
