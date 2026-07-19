"""Gateway-backed OpenHands tool executors (7142 agent, 7143 sandbox)."""

from __future__ import annotations

from typing import TYPE_CHECKING, Optional

from openhands.sdk.llm import TextContent
from openhands.sdk.tool import ToolExecutor
from openhands.tools.file_editor.definition import (
    FileEditorAction,
    FileEditorObservation,
)
from openhands.tools.terminal.definition import TerminalAction, TerminalObservation
from openhands.tools.terminal.metadata import CmdOutputMetadata

from .workspace import UEnvWorkspace

if TYPE_CHECKING:
    from openhands.sdk.conversation import LocalConversation


def _abs_path(workspace: UEnvWorkspace, path: str) -> str:
    p = path.strip()
    if p.startswith("/"):
        return p
    wd = workspace.working_dir.rstrip("/")
    return f"{wd}/{p}"


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
        r = self._ws.execute_command(cmd, cwd=self._ws.working_dir)
        text = r.stdout
        if r.stderr:
            text = (text + "\n" + r.stderr).strip() if text else r.stderr
        meta = CmdOutputMetadata(
            exit_code=r.exit_code,
            pid=-1,
            working_dir=self._ws.working_dir,
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


def patch_openhands_tools_for_uenv() -> None:
    """Route terminal/file_editor to UEnv gateway when workspace is UEnvWorkspace."""
    import os
    from collections.abc import Sequence

    from openhands.sdk.conversation.state import ConversationState
    from openhands.sdk.tool import ToolDefinition, ToolExecutor, register_tool
    from openhands.tools.file_editor.definition import FileEditorTool
    from openhands.tools.terminal.definition import TerminalTool

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
        if isinstance(ws, UEnvWorkspace):
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
        if isinstance(ws, UEnvWorkspace):
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
        working_dir = conv_state.workspace.working_dir
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
    # Registry resolvers capture ``create`` at registration time, so replacing the
    # classmethod alone leaves the old local executors active.
    register_tool(TerminalTool.name, TerminalTool)
    register_tool(FileEditorTool.name, FileEditorTool)
