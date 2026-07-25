"""Gateway-backed OpenHands tool executors (7142 agent, 7143 sandbox)."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any, Optional

from openhands.sdk.llm import TextContent
from openhands.sdk.tool import ToolExecutor
from openhands.tools.file_editor.definition import (
    FileEditorAction,
    FileEditorObservation,
)
from openhands.tools.terminal.definition import TerminalAction, TerminalObservation
from openhands.tools.terminal.metadata import CmdOutputMetadata

from .workspace import UEnvWorkspace
from .workspace_utils import is_uenv_gateway_workspace

_UENV_PATCH_APPLIED = False

if TYPE_CHECKING:
    from openhands.sdk.conversation import LocalConversation


def collect_tool_patch_status(conv_state: Any) -> dict[str, Any]:
    """Instantiate terminal/file_editor tools and record executor class names."""
    from openhands.sdk.tool.registry import resolve_tool
    from openhands.sdk.tool.spec import Tool
    from openhands.tools.file_editor.definition import FileEditorTool
    from openhands.tools.terminal.definition import TerminalTool

    ws = conv_state.workspace
    doc: dict[str, Any] = {
        "workspace_type": type(ws).__name__,
        "workspace_module": type(ws).__module__,
        "is_uenv_gateway_workspace": is_uenv_gateway_workspace(ws),
        "gateway_url": getattr(ws, "gateway_url", None),
        "instance_id": getattr(ws, "instance_id", None),
        "working_dir": getattr(ws, "working_dir", None),
        "container_working_dir": getattr(ws, "remote_working_dir", None)
        or getattr(ws, "container_working_dir", None),
    }
    # Prefer registry resolve_tool (same path as Agent._initialize), not class.create
    # which can look patched while the registry still holds a stale create closure.
    try:
        term = resolve_tool(Tool(name=TerminalTool.name), conv_state)
        doc["terminal_executor"] = type(term[0].executor).__name__ if term else None
        doc["terminal_uses_gateway"] = "UEnvGateway" in (doc["terminal_executor"] or "")
    except Exception as e:  # noqa: BLE001
        doc["terminal_error"] = repr(e)
    try:
        fe = resolve_tool(Tool(name=FileEditorTool.name), conv_state)
        doc["file_editor_executor"] = type(fe[0].executor).__name__ if fe else None
        doc["file_editor_uses_gateway"] = "UEnvGateway" in (
            doc["file_editor_executor"] or ""
        )
    except Exception as e:  # noqa: BLE001
        doc["file_editor_error"] = repr(e)
    doc["patch_ok"] = bool(
        doc.get("terminal_uses_gateway") and doc.get("file_editor_uses_gateway")
    )
    return doc


def _abs_path(workspace: UEnvWorkspace, path: str) -> str:
    p = path.strip()
    if p.startswith("/"):
        return p
    wd = getattr(workspace, "remote_working_dir", None) or workspace.working_dir
    wd = str(wd).rstrip("/")
    return f"{wd}/{p}"


def _assert_path_in_workspace(workspace: UEnvWorkspace, path: str) -> None:
    """Reject cross-repo hallucination (e.g. writing openlibrary paths in qutebrowser)."""
    wd = (
        getattr(workspace, "remote_working_dir", None) or workspace.working_dir or "/app"
    )
    wd = str(wd).rstrip("/") or "/app"
    abs_path = _abs_path(workspace, path)
    if not (abs_path == wd or abs_path.startswith(wd + "/")):
        raise RuntimeError(
            f"path {abs_path!r} is outside workspace {wd!r}; "
            f"only edit under {wd}"
        )
    instance = (getattr(workspace, "instance_id", None) or "").lower()
    lowered = abs_path.lower()
    if "openlibrary" in lowered and "openlibrary" not in instance:
        raise RuntimeError(
            f"refusing path {abs_path!r}: this instance is {instance!r}, "
            "not openlibrary. Stay inside the checked-out repository under "
            f"{wd} (e.g. qutebrowser/, internal/, src/)."
        )


class UEnvGatewayTerminalExecutor(ToolExecutor[TerminalAction, TerminalObservation]):
    """One-shot bash via UEnv Gateway (no local tmux)."""

    def __init__(self, workspace: UEnvWorkspace):
        self._ws = workspace

    @property
    def is_pooled(self) -> bool:
        return False

    def __call__(
        self,
        action: TerminalAction,
        conversation: "LocalConversation | None" = None,  # noqa: ARG002
    ) -> TerminalObservation:
        cmd = action.command or ""
        if action.is_input:
            return TerminalObservation.from_text(
                text="Interactive input is not supported on UEnv gateway terminal.",
                command=cmd,
                is_error=True,
            )
        if action.reset:
            return TerminalObservation.from_text(
                text="Terminal reset is a no-op on UEnv gateway (stateless shell).",
                command=cmd,
                is_error=False,
            )
        # Soft-guard: refuse cd/edits that clearly target wrong repos / host clones.
        instance = (getattr(self._ws, "instance_id", None) or "").lower()
        wd = getattr(self._ws, "remote_working_dir", None) or getattr(
            self._ws, "working_dir", "/app"
        )
        lowered = cmd.lower()
        if "openlibrary" not in instance and "openlibrary" in lowered:
            return TerminalObservation.from_text(
                text=(
                    "Refusing command that references openlibrary — this instance "
                    f"is {instance!r}. Stay under {wd}."
                ),
                command=cmd,
                is_error=True,
            )
        # Agent often `git clone` / `cp -r` into /tmp/<repo> then edits there;
        # grader only sees /app. Block the common failure modes.
        if any(
            p in lowered
            for p in (
                " /tmp/",
                "cd /tmp",
                "clone /tmp",
                "/tmp/qutebrowser",
                "/tmp/nodebb",
                "/tmp/ansible",
                "/tmp/openlibrary",
            )
        ) and "/tmp/gold.patch" not in lowered and "/tmp/uenv" not in lowered:
            return TerminalObservation.from_text(
                text=(
                    f"Refusing command that uses /tmp worktrees. "
                    f"The repository is already at {wd}; stay there "
                    f"(do not clone or copy the repo under /tmp)."
                ),
                command=cmd,
                is_error=True,
            )
        r = self._ws.execute_command(
            cmd, cwd=getattr(self._ws, "remote_working_dir", self._ws.working_dir)
        )
        text = r.stdout
        if r.stderr:
            text = (text + "\n" + r.stderr).strip() if text else r.stderr
        meta = CmdOutputMetadata(
            exit_code=r.exit_code,
            pid=-1,
            working_dir=self._ws.remote_working_dir,
        )
        return TerminalObservation(
            command=cmd,
            exit_code=r.exit_code,
            timeout=False,
            metadata=meta,
            content=[TextContent(text=text or "")],
            is_error=r.exit_code != 0,
        )


class UEnvGatewayFileEditorExecutor(ToolExecutor[FileEditorAction, FileEditorObservation]):
    """Minimal file_editor commands over gateway read/write."""

    def __init__(self, workspace: UEnvWorkspace):
        self._ws = workspace

    def __call__(
        self,
        action: FileEditorAction,
        conversation: "LocalConversation | None" = None,  # noqa: ARG002
    ) -> FileEditorObservation:
        path = _abs_path(self._ws, action.path)
        cmd = action.command
        try:
            _assert_path_in_workspace(self._ws, path)
            if cmd == "view":
                return self._view(path, action.view_range)
            if cmd == "create":
                assert action.file_text is not None
                ok = self._ws.write_remote_text(path, action.file_text)
                if not ok:
                    raise RuntimeError("gateway write failed")
                return FileEditorObservation.from_text(
                    text=f"File created at {path}",
                    command=cmd,
                    path=path,
                )
            if cmd == "str_replace":
                content = self._ws.read_remote_text(path)
                old = action.old_str or ""
                if old not in content:
                    raise RuntimeError(f"old_str not found in {path}")
                new_content = content.replace(old, action.new_str or "", 1)
                self._ws.write_remote_text(path, new_content)
                return FileEditorObservation.from_text(
                    text=f"Replacement applied in {path}",
                    command=cmd,
                    path=path,
                )
            if cmd == "insert":
                content = self._ws.read_remote_text(path)
                lines = content.splitlines(keepends=True)
                idx = action.insert_line or 0
                insert = action.new_str or ""
                if not insert.endswith("\n"):
                    insert += "\n"
                lines.insert(idx, insert)
                self._ws.write_remote_text(path, "".join(lines))
                return FileEditorObservation.from_text(
                    text=f"Inserted at line {idx} in {path}",
                    command=cmd,
                    path=path,
                )
            if cmd == "undo_edit":
                return FileEditorObservation.from_text(
                    text="undo_edit is not supported on UEnv gateway file editor.",
                    command=cmd,
                    is_error=True,
                )
            raise RuntimeError(f"unsupported command {cmd}")
        except Exception as e:
            return FileEditorObservation.from_text(
                text=str(e),
                command=cmd,
                is_error=True,
            )

    def _view(self, path: str, view_range: Optional[list[int]]) -> FileEditorObservation:
        if view_range is None:
            r = self._ws.execute_command(f"ls -la {path}", cwd="/")
            if r.exit_code == 0 and "No such file" not in r.stderr:
                if r.stdout.strip().startswith("total ") or " " in r.stdout.split()[0:1]:
                    return FileEditorObservation.from_text(
                        text=r.stdout,
                        command="view",
                        path=path,
                    )
        content = self._ws.read_remote_text(path)
        lines = content.splitlines()
        if view_range and len(view_range) == 2:
            start, end = view_range
            start = max(1, start)
            end = min(len(lines), end)
            chunk = lines[start - 1 : end]
        else:
            start = 1
            chunk = lines
        numbered = "\n".join(f"{i + start:6d}\t{line}" for i, line in enumerate(chunk))
        return FileEditorObservation.from_text(
            text=numbered or "(empty file)",
            command="view",
            path=path,
        )


def patch_openhands_tools_for_uenv() -> dict[str, Any]:
    """Route terminal/file_editor to UEnv gateway when workspace is UEnvWorkspace."""
    global _UENV_PATCH_APPLIED
    import os
    from collections.abc import Sequence

    from openhands.sdk.conversation.state import ConversationState
    from openhands.sdk.tool import ToolDefinition, ToolExecutor
    from openhands.tools.file_editor.definition import FileEditorTool
    from openhands.tools.terminal.definition import TerminalTool

    status: dict[str, Any] = {"patched": False}

    if _UENV_PATCH_APPLIED:
        status["patched"] = True
        status["already_patched"] = True
        return status

    _orig_terminal = TerminalTool.create.__func__  # type: ignore[attr-defined]
    _orig_file = FileEditorTool.create.__func__  # type: ignore[attr-defined]

    @classmethod
    def _terminal_create(
        cls,
        conv_state: ConversationState,
        username: str | None = None,
        no_change_timeout_seconds: int | None = None,
        terminal_type=None,
        shell_path: str | None = None,
        executor: ToolExecutor | None = None,
    ) -> Sequence[ToolDefinition]:
        ws = conv_state.workspace
        if is_uenv_gateway_workspace(ws):
            import platform

            from openhands.sdk.tool import ToolAnnotations
            from openhands.tools.terminal.definition import (
                UNIX_TOOL_DESCRIPTION,
                WINDOWS_TOOL_DESCRIPTION,
                TerminalAction,
                TerminalObservation,
                TerminalTool,
            )

            executor = UEnvGatewayTerminalExecutor(ws)
            tool_description = (
                WINDOWS_TOOL_DESCRIPTION
                if platform.system() == "Windows"
                else UNIX_TOOL_DESCRIPTION
            )
            return [
                TerminalTool(
                    action_type=TerminalAction,
                    observation_type=TerminalObservation,
                    description=tool_description,
                    annotations=ToolAnnotations(
                        title="terminal",
                        readOnlyHint=False,
                        destructiveHint=True,
                        idempotentHint=False,
                        openWorldHint=True,
                    ),
                    executor=executor,
                )
            ]
        return _orig_terminal(
            cls,
            conv_state,
            username=username,
            no_change_timeout_seconds=no_change_timeout_seconds,
            terminal_type=terminal_type,
            shell_path=shell_path,
            executor=executor,
        )

    @classmethod
    def _file_create(
        cls,
        conv_state: ConversationState,
    ) -> Sequence[ToolDefinition]:
        ws = conv_state.workspace
        if is_uenv_gateway_workspace(ws):
            return _build_file_editor_tool(conv_state, UEnvGatewayFileEditorExecutor(ws))
        return _orig_file(cls, conv_state)

    def _build_file_editor_tool(conv_state, executor):
        from openhands.sdk.tool import ToolAnnotations
        from openhands.tools.file_editor.definition import (
            TOOL_DESCRIPTION,
            FileEditorAction,
            FileEditorObservation,
            FileEditorTool,
        )

        description_lines = TOOL_DESCRIPTION.split("\n")
        base_description = "\n".join(description_lines[:2])
        remaining_description = "\n".join(description_lines[2:])
        if conv_state.agent.llm.vision_is_active():
            tool_description = (
                f"{base_description}\n"
                "* If `path` is an image file (.png, .jpg, .jpeg, .gif, .webp, "
                ".bmp), `view` displays the image content\n"
                f"{remaining_description}"
            )
        else:
            tool_description = TOOL_DESCRIPTION
        working_dir = getattr(
            conv_state.workspace, "remote_working_dir", None
        ) or conv_state.workspace.working_dir
        enhanced_description = (
            f"{tool_description}\n\n"
            f"Your current working directory is: {working_dir}\n"
            f"When exploring project structure, start with this directory "
            f"instead of root.\n"
        )
        return [
            FileEditorTool(
                action_type=FileEditorAction,
                observation_type=FileEditorObservation,
                description=enhanced_description,
                annotations=ToolAnnotations(
                    title="file_editor",
                    readOnlyHint=False,
                    destructiveHint=True,
                    idempotentHint=False,
                    openWorldHint=True,
                ),
                executor=executor,
            )
        ]

    # Skip local working_dir existence check for remote container paths.
    _orig_isdir = os.path.isdir

    def _isdir(path: str) -> bool:
        if path.startswith("/app") or path.startswith("/testbed"):
            return True
        return _orig_isdir(path)

    os.path.isdir = _isdir  # type: ignore[assignment]
    TerminalTool.create = _terminal_create  # type: ignore[method-assign]
    FileEditorTool.create = _file_create  # type: ignore[method-assign]
    # CRITICAL: Tool registry closes over create() at import/register time.
    # Patching the classmethod alone leaves resolve_tool() on the stale local
    # executors — re-register so Agent._initialize picks up gateway tools.
    from openhands.sdk.tool.registry import register_tool

    register_tool(TerminalTool.name, TerminalTool)
    register_tool(FileEditorTool.name, FileEditorTool)
    status["reregistered"] = [TerminalTool.name, FileEditorTool.name]
    _UENV_PATCH_APPLIED = True
    status["patched"] = True
    return status
