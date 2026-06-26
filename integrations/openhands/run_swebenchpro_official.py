#!/usr/bin/env python3
"""OpenHands official SDK + SWE-bench Pro via UEnv Gateway (7142 → 7143).

Uses OpenHands Software Agent SDK (Agent, Conversation, default tools) with
gateway-backed terminal/file_editor executors. Grading via Worker submit().

Requires OpenHands/benchmarks venv on 7142 (see scripts/deploy-openhands-7142.sh).

Example:
  cd /opt/openhands/benchmarks
  uv run python /root/UEnv/integrations/openhands/run_swebenchpro_official.py \\
      --llm-config /root/UEnv/config/openhands-llm-7142.json \\
      --gateway http://10.10.20.143:28999 --api-key swe-pro-secret \\
      --instance instance_qutebrowser__... \\
      --instances /root/UEnv/config/swe/pro-python-smoke.json \\
      --output-dir /var/log/uenv/openhands-runs/run1 \\
      --max-iterations 30
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
import urllib.error
from pathlib import Path
from typing import Any

# UEnv integration (dependency-free client + workspace)
_INTEGRATION = Path(__file__).resolve().parent
sys.path.insert(0, str(_INTEGRATION))
from uenv_runtime.client import UEnvGatewayClient  # noqa: E402
from uenv_runtime.gateway_tools import patch_openhands_tools_for_uenv  # noqa: E402
from uenv_runtime.workspace import UEnvWorkspace  # noqa: E402


def _ensure_benchmarks_path() -> None:
    bench = os.environ.get("OPENHANDS_BENCHMARKS_DIR", "/opt/openhands/benchmarks")
    if bench not in sys.path:
        sys.path.insert(0, bench)


def _load_catalog(path: Path, instance_id: str) -> dict[str, Any]:
    data = json.loads(path.read_text(encoding="utf-8"))
    if instance_id not in data:
        raise SystemExit(f"instance {instance_id!r} not in {path}")
    return data[instance_id]


def _pro_workspace_dir(variant: str) -> str:
    return "/app" if variant.lower() == "pro" else "/testbed"


def _build_instruction(instance: dict[str, Any], repo_path: str) -> str:
    ps = instance.get("problem_statement") or instance.get("issue_text") or ""
    return (
        f"The git repository is already checked out at `{repo_path}`.\n"
        f"Start by running `ls -la {repo_path}` and `find {repo_path} -maxdepth 2 -type f -name '*.py' | head`.\n"
        f"All edits must be under `{repo_path}`.\n\n"
        f"<issue_description>\n{ps}\n</issue_description>\n\n"
        "Implement the minimal fix to non-test source files. Tests are already updated.\n"
        "Use terminal and file_editor tools. When done, call the finish tool.\n"
    )


def _save_json(path: Path, obj: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(obj, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def _run_conversation_loop(conversation, max_fake_responses: int = 5) -> None:
    """Like benchmarks fake_user_response helper but compatible with LocalConversation."""
    from benchmarks.utils.fake_user_response import (
        _agent_finished_with_finish_action,
        _agent_sent_message,
        fake_user_response,
    )
    from openhands.sdk.conversation.state import ConversationExecutionStatus

    fake_count = 0
    while True:
        conversation.run()
        status = conversation.state.execution_status
        if status != ConversationExecutionStatus.FINISHED:
            break
        events = list(conversation.state.events)
        if _agent_finished_with_finish_action(events):
            break
        if not _agent_sent_message(events):
            break
        if fake_count >= max_fake_responses:
            break
        msg = fake_user_response(conversation)
        if msg == "/exit":
            break
        conversation.send_message(msg)
        fake_count += 1


def main() -> int:
    ap = argparse.ArgumentParser(description="OpenHands SDK Pro eval via UEnv Gateway")
    ap.add_argument("--llm-config", required=True, help="OpenHands LLM JSON (openhands.sdk.LLM)")
    ap.add_argument("--gateway", default=os.environ.get("UENV_GATEWAY", "http://10.10.20.143:28999"))
    ap.add_argument("--api-key", default=os.environ.get("UENV_GATEWAY_API_KEY"))
    ap.add_argument("--instance", required=True)
    ap.add_argument("--instances", default="config/swe/pro-python-smoke.json")
    ap.add_argument("--benchmark-variant", default="pro")
    ap.add_argument("--output-dir", required=True)
    ap.add_argument("--max-iterations", type=int, default=30)
    ap.add_argument("--mode", choices=["llm", "gold"], default="llm")
    args = ap.parse_args()

    _ensure_benchmarks_path()
    patch_openhands_tools_for_uenv()

    from benchmarks.utils.llm_config import load_llm_config
    from openhands.sdk import Agent, Conversation, Tool, get_logger
    from openhands.tools.file_editor import FileEditorTool
    from openhands.tools.task_tracker import TaskTrackerTool
    from openhands.tools.terminal import TerminalTool

    logger = get_logger(__name__)

    out = Path(args.output_dir)
    run_log = out / "run.log"
    repo_root = Path(os.environ.get("UENV_REPO", "/root/UEnv"))
    catalog_path = Path(args.instances)
    if not catalog_path.is_absolute():
        catalog_path = repo_root / catalog_path

    row = _load_catalog(catalog_path, args.instance)
    workspace_dir = _pro_workspace_dir(args.benchmark_variant)

    client = UEnvGatewayClient(args.gateway, api_key=args.api_key)
    if not client.health():
        print("gateway health check failed", file=sys.stderr)
        return 1

    llm = load_llm_config(args.llm_config)
    logger.info("LLM model=%s", llm.model)

    ws = UEnvWorkspace(
        working_dir=workspace_dir,
        gateway_url=args.gateway,
        instance_id=args.instance,
        benchmark_variant=args.benchmark_variant,
        api_key=args.api_key,
    )

    _save_json(
        out / "config_snapshot.json",
        {
            "gateway": args.gateway,
            "instance": args.instance,
            "mode": args.mode,
            "max_iterations": args.max_iterations,
            "llm_model": str(llm.model),
            "benchmark_variant": args.benchmark_variant,
        },
    )

    t0 = time.time()
    try:
        with ws:
            _save_json(out / "reset_observation.json", ws.session.observation)

            if args.mode == "gold":
                patch = row.get("patch", "")
                if patch.strip():
                    ws.write_remote_text("/tmp/gold.patch", patch)
                    r = ws.execute_command(
                        "git apply -v /tmp/gold.patch || "
                        "patch --batch --fuzz=5 -p1 < /tmp/gold.patch"
                    )
                    _save_json(out / "gold_apply.json", r.model_dump())
                result = ws.submit()
            else:
                agent = Agent(
                    llm=llm,
                    tools=[
                        Tool(name=TerminalTool.name),
                        Tool(name=FileEditorTool.name),
                        Tool(name=TaskTrackerTool.name),
                    ],
                    system_prompt_kwargs={"cli_mode": True},
                )
                conversation = Conversation(
                    agent=agent,
                    workspace=ws,
                    max_iteration_per_run=args.max_iterations,
                    delete_on_close=True,
                )
                instruction = _build_instruction(row, workspace_dir)
                _save_json(out / "instruction.txt", {"text": instruction})
                conversation.send_message(instruction)
                _run_conversation_loop(conversation, max_fake_responses=5)
                _save_json(
                    out / "conversation_events.json",
                    {"count": len(list(conversation.state.events))},
                )
                result = ws.submit()

        elapsed = time.time() - t0
        submit_doc = {
            "instance_id": result.instance_id,
            "resolved": result.resolved,
            "reward": result.reward,
            "tests_passed": result.tests_passed,
            "tests_total": result.tests_total,
            "per_test": result.per_test,
            "trajectory_ref": result.trajectory_ref,
            "elapsed_sec": elapsed,
        }
        _save_json(out / "submit_result.json", submit_doc)

        ref = result.trajectory_ref
        if ref and ref.get("trajectory_id"):
            _save_json(out / "trajectory_ref.json", ref)
            try:
                bundle = client.get_trajectory(ref["trajectory_id"])
                _save_json(out / "trajectory_bundle.json", bundle)
            except urllib.error.HTTPError as e:
                print(f"get_trajectory failed: {e.code}", file=sys.stderr)

        with run_log.open("a", encoding="utf-8") as f:
            f.write(
                f"[done] mode={args.mode} reward={result.reward} "
                f"tests={result.tests_passed}/{result.tests_total} elapsed={elapsed:.1f}s\n"
            )

        print(
            json.dumps(
                {
                    "resolved": result.resolved,
                    "reward": result.reward,
                    "tests_passed": result.tests_passed,
                    "tests_total": result.tests_total,
                    "trajectory_id": (ref or {}).get("trajectory_id"),
                    "output_dir": str(out),
                }
            )
        )
        return 0 if result.reward >= 1.0 else 0  # exit 0 if run completed; reward in JSON

    except Exception as e:
        with run_log.open("a", encoding="utf-8") as f:
            f.write(f"[error] {e!r}\n")
        raise


if __name__ == "__main__":
    raise SystemExit(main())
