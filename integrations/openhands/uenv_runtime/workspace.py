"""OpenHands Software Agent SDK ``LocalWorkspace`` backed by UEnv Gateway."""

from __future__ import annotations

import shlex
from pathlib import Path
from typing import Any, Optional

from .client import UEnvGatewayClient, UEnvSession

try:
    from openhands.sdk.git.models import GitChange, GitChangeStatus, GitDiff
    from openhands.sdk.workspace.local import LocalWorkspace
    from openhands.sdk.workspace.models import CommandResult, FileOperationResult
except ImportError as exc:  # pragma: no cover - optional dep on 7142 venv
    raise ImportError(
        "openhands SDK required; install OpenHands/benchmarks (make build) on 7142"
    ) from exc


class UEnvWorkspace(LocalWorkspace):
    """Remote sandbox via UEnv Runtime Gateway (7143).

    ``working_dir`` is the path **inside the provisioned container** (e.g. ``/app``).
    Tooling on 7142 must use gateway-backed executors (see ``gateway_tools.py``).
    """

    gateway_url: str
    instance_id: str
    benchmark_variant: str = "pro"
    command_mode: str = "FullShell"
    api_key: Optional[str] = None
    gateway_timeout: float = 600.0
    run_id: Optional[str] = None
    session_id: Optional[str] = None

    _client: Any = None
    _session: Any = None

    def model_post_init(self, __context: Any) -> None:
        super().model_post_init(__context)
        self._client = UEnvGatewayClient(
            self.gateway_url,
            timeout=self.gateway_timeout,
            api_key=self.api_key,
            run_id=self.run_id,
        )

    @property
    def session(self) -> UEnvSession:
        if self._session is None:
            if self.session_id:
                self._session = self._client.attach_session(self.session_id, self.instance_id)
            else:
                self._session = self._client.create_session(
                    self.instance_id,
                    self.benchmark_variant,
                    self.command_mode,
                )
        return self._session

    @property
    def issue_text(self) -> str:
        return self.session.issue_text

    def _shell(self, command: str, cwd: str | Path | None = None, timeout: float = 30.0) -> CommandResult:
        wd = str(cwd) if cwd is not None else self.working_dir
        wrapped = f"cd {shlex.quote(wd)} && {command}"
        r = self.session.exec(wrapped)
        return CommandResult(
            command=command,
            exit_code=r.exit_code,
            stdout=r.stdout,
            stderr=r.stderr,
            timeout_occurred=False,
        )

    def execute_command(
        self,
        command: str,
        cwd: str | Path | None = None,
        timeout: float = 30.0,
    ) -> CommandResult:
        return self._shell(command, cwd=cwd, timeout=timeout)

    def file_upload(
        self,
        source_path: str | Path,
        destination_path: str | Path,
    ) -> FileOperationResult:
        src = Path(source_path)
        dest = str(destination_path)
        try:
            content = src.read_text(encoding="utf-8")
            ok = self.session.write(dest, content)
            return FileOperationResult(
                success=ok,
                source_path=str(src),
                destination_path=dest,
                file_size=len(content.encode()) if ok else None,
                error=None if ok else "gateway write failed",
            )
        except Exception as e:
            return FileOperationResult(
                success=False,
                source_path=str(src),
                destination_path=dest,
                error=str(e),
            )

    def file_download(
        self,
        source_path: str | Path,
        destination_path: str | Path,
    ) -> FileOperationResult:
        src = str(source_path)
        dest = Path(destination_path)
        try:
            content = self.session.read(src)
            dest.parent.mkdir(parents=True, exist_ok=True)
            dest.write_text(content, encoding="utf-8")
            return FileOperationResult(
                success=True,
                source_path=src,
                destination_path=str(dest),
                file_size=len(content.encode()),
            )
        except Exception as e:
            return FileOperationResult(
                success=False,
                source_path=src,
                destination_path=str(dest),
                error=str(e),
            )

    def read_remote_text(self, path: str) -> str:
        return self.session.read(path)

    def write_remote_text(self, path: str, content: str) -> bool:
        return self.session.write(path, content)

    def git_changes(self, path: str | Path) -> list[GitChange]:
        repo = str(path)
        r = self._shell("git status --porcelain", cwd=repo)
        if r.exit_code != 0:
            raise RuntimeError(r.stderr or "git status failed")
        changes: list[GitChange] = []
        for line in r.stdout.splitlines():
            line = line.strip()
            if not line or len(line) < 4:
                continue
            xy = line[:2]
            fpath = line[3:].strip().strip('"')
            if "D" in xy:
                status = GitChangeStatus.DELETED
            elif "A" in xy or "?" in xy:
                status = GitChangeStatus.ADDED
            elif "R" in xy:
                status = GitChangeStatus.MOVED
            else:
                status = GitChangeStatus.UPDATED
            changes.append(GitChange(status=status, path=Path(fpath)))
        return changes

    def git_diff(self, path: str | Path) -> GitDiff:
        repo = str(path)
        r = self._shell(f"git diff -- {shlex.quote(str(path))}", cwd=repo)
        if r.exit_code != 0:
            raise RuntimeError(r.stderr or "git diff failed")
        return GitDiff(modified=r.stdout or None, original=None)

    def submit(self):
        return self.session.submit()

    def close_session(self) -> None:
        if self._session is not None:
            try:
                self._session.destroy()
            finally:
                self._session = None

    def __exit__(self, exc_type, exc, tb) -> None:
        self.close_session()
        super().__exit__(exc_type, exc, tb)

    def snapshot(self) -> dict:
        return {
            "session_id": self.session.session_id,
            "instance_id": self.instance_id,
            "observation_keys": list(self.session.observation.keys()),
        }
