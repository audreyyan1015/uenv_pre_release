#!/usr/bin/env python3
"""OpenHands official SDK + SWE-bench Pro via UEnv Gateway (208.77 → 7143).

Requires OpenHands/benchmarks on 208.77 (see scripts/deploy-openhands-20877.sh).

Example:
  bash /root/UEnv/scripts/run-openhands-pro-20877.sh gold
  # or manually:
  cd /opt/openhands/benchmarks/vendor/software-agent-sdk
  uv run python /root/UEnv/integrations/openhands/run_swebenchpro_official.py \\
      --llm-config /root/UEnv/config/openhands-llm-20877.json \\
      --gateway http://127.0.0.1:28097 --api-key swe-pro-secret \\
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
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any

# UEnv integration (dependency-free client + workspace)
_INTEGRATION = Path(__file__).resolve().parent
sys.path.insert(0, str(_INTEGRATION))
from uenv_runtime.client import UEnvGatewayClient, GatewayError  # noqa: E402
from uenv_runtime.agent_job import load_agent_job  # noqa: E402
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
    repo_language = str(instance.get("repo_language") or "").strip().lower()
    if repo_language in {"python", "py"}:
        language_hint = "Python files such as `*.py`"
    elif repo_language in {"go", "golang"}:
        language_hint = "Go files such as `*.go`"
    elif repo_language in {"javascript", "js", "typescript", "ts"}:
        language_hint = "JavaScript/TypeScript files such as `*.js`, `*.ts`, and `*.tsx`"
    else:
        language_hint = "files matching the repository language and nearby config/template files"
    return (
        f"The git repository is already checked out at `{repo_path}`.\n"
        f"All investigation and edits must stay under `{repo_path}`.\n"
        "Start by confirming the workspace:\n"
        f"1. `pwd`\n"
        f"2. `git -C {repo_path} rev-parse --show-toplevel`\n"
        f"3. `ls -la {repo_path}`\n\n"
        "Inspect the repository structure and identify the relevant language/framework before searching.\n"
        f"This instance is labeled as `{repo_language or 'unknown'}`; prioritize {language_hint}.\n"
        "Use targeted searches with `rg` for symbols, error messages, routes, tests, or issue keywords.\n"
        "When relevant, also inspect non-test project files such as JSON, YAML, templates, and generated schemas.\n"
        f"Do not search or edit outside `{repo_path}`. Do not inspect `/opt/openhands`, benchmark harness directories, `/tmp`, or `/root` unless explicitly required by a tool.\n\n"
        f"<issue_description>\n{ps}\n</issue_description>\n\n"
        "Implement the minimal fix in non-test project files required by the issue. Tests are already provided by the benchmark; do not modify tests unless the issue explicitly requires it.\n"
        "Before finishing, inspect `git diff` and make sure the patch is focused.\n"
        "Use terminal and file_editor tools. When done, call the finish tool.\n"
    )


def _save_json(path: Path, obj: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(obj, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def _verify_server_trajectory(
    trajectory_id: str,
    run_id: str,
    out: Path,
) -> dict[str, Any]:
    """Optional: GET trajectory from Server :8077 after Worker upload ack."""
    endpoint = os.environ.get("UENV_TRAJECTORY_ENDPOINT", "").rstrip("/")
    token = os.environ.get("UENV_TRAJECTORY_TOKEN", "").strip()
    if not endpoint or not trajectory_id:
        return {"skipped": True, "reason": "UENV_TRAJECTORY_ENDPOINT unset or no trajectory_id"}

    headers = {"X-Trajectory-Token": token} if token else {}
    doc: dict[str, Any] = {"endpoint": endpoint, "trajectory_id": trajectory_id, "run_id": run_id}

    def _get(path: str) -> tuple[int, str]:
        req = urllib.request.Request(f"{endpoint}{path}", method="GET")
        for k, v in headers.items():
            req.add_header(k, v)
        try:
            with urllib.request.urlopen(req, timeout=120) as resp:
                return resp.status, resp.read().decode()
        except urllib.error.HTTPError as e:
            return e.code, e.read().decode(errors="replace")

    # Wait for async uploader (spool drainer polls every 5s).
    body_ok = False
    list_ok = False
    for attempt in range(1, 25):
        status, raw = _get(f"/control/v1/trajectories/{trajectory_id}")
        doc[f"body_attempt_{attempt}"] = status
        if status == 200:
            body_ok = True
            try:
                doc["body_keys"] = list(json.loads(raw).keys())
            except json.JSONDecodeError:
                doc["body_keys"] = []
            break
        time.sleep(5)

    if run_id:
        status, raw = _get(f"/control/v1/trajectories?run_id={urllib.parse.quote(run_id)}&limit=10")
        doc["list_status"] = status
        if status == 200:
            try:
                arr = json.loads(raw).get("trajectories", [])
                doc["list_count"] = len(arr) if isinstance(arr, list) else 0
                list_ok = isinstance(arr, list) and any(
                    x.get("trajectory_id") == trajectory_id for x in arr
                )
            except json.JSONDecodeError:
                doc["list_count"] = 0

    doc["body_ok"] = body_ok
    doc["list_ok"] = list_ok
    doc["server_verified"] = body_ok
    _save_json(out / "server_trajectory_verify.json", doc)
    return doc


def _fetch_trajectory_bundle(client: UEnvGatewayClient, ref: dict, out: Path) -> dict | None:
    """Fetch full bundle from Server (preferred after upload) or Gateway."""
    tid = ref.get("trajectory_id")
    if not tid:
        return None
    endpoint = os.environ.get("UENV_TRAJECTORY_ENDPOINT", "").rstrip("/")
    token = os.environ.get("UENV_TRAJECTORY_TOKEN", "").strip()

    if endpoint:
        headers = {"X-Trajectory-Token": token} if token else {}
        for attempt in range(1, 25):
            req = urllib.request.Request(f"{endpoint}/control/v1/trajectories/{tid}", method="GET")
            for k, v in headers.items():
                req.add_header(k, v)
            try:
                with urllib.request.urlopen(req, timeout=120) as resp:
                    return json.loads(resp.read().decode())
            except urllib.error.HTTPError as e:
                if e.code in (404, 503) and attempt < 24:
                    time.sleep(5)
                    continue
                _save_json(out / "server_trajectory_fetch_error.json", {"status": e.code, "body": e.read().decode(errors="replace")})
                break

    try:
        return client.get_trajectory(tid)
    except GatewayError as e:
        _save_json(out / "gateway_trajectory_fetch_error.json", {"status": e.status, "message": e.message})
        return None


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
    ap.add_argument(
        "--llm-config",
        default=os.environ.get("OPENHANDS_LLM_CONFIG", ""),
        help="OpenHands LLM JSON (openhands.sdk.LLM); optional for gold mode",
    )
    ap.add_argument(
        "--gateway",
        default=os.environ.get("UENV_GATEWAY", ""),
        help="Runtime Gateway URL (optional when UENV_AGENT_JOB_FILE is set)",
    )
    ap.add_argument("--api-key", default=os.environ.get("UENV_GATEWAY_API_KEY"))
    ap.add_argument("--instance", default=os.environ.get("UENV_PRO_INSTANCE", ""))
    ap.add_argument(
        "--instances",
        default=os.environ.get(
            "UENV_SWE_INSTANCES",
            os.environ.get("UENV_SWE_ENV_PACKAGE_CATALOG", "config/swe/pro-python-smoke.json"),
        ),
    )
    ap.add_argument("--benchmark-variant", default="pro")
    ap.add_argument("--output-dir", required=True)
    ap.add_argument("--max-iterations", type=int, default=30)
    ap.add_argument("--mode", choices=["llm", "gold"], default="llm")
    ap.add_argument(
        "--run-id",
        default=os.environ.get("UENV_RUN_ID", ""),
        help="一次评测作业 ID（注入 X-UEnv-Run-Id；默认 UENV_RUN_ID 或自动生成）",
    )
    ap.add_argument(
        "--agent-job-file",
        default=os.environ.get("UENV_AGENT_JOB_FILE", ""),
        help="AgentJob JSON (Phase B); overrides gateway/session/run/instance when set",
    )
    args = ap.parse_args()

    agent_job = None
    if args.agent_job_file:
        os.environ["UENV_AGENT_JOB_FILE"] = args.agent_job_file
    try:
        agent_job = load_agent_job(args.agent_job_file or None)
    except (FileNotFoundError, ValueError) as exc:
        print(f"AgentJob load failed: {exc}", file=sys.stderr)
        return 1

    if agent_job:
        if agent_job.gateway_url:
            args.gateway = agent_job.gateway_url
        if agent_job.gateway_api_key:
            args.api_key = agent_job.gateway_api_key
        if agent_job.instance_id:
            args.instance = agent_job.instance_id
        if agent_job.benchmark_variant:
            args.benchmark_variant = agent_job.benchmark_variant
        if agent_job.max_iterations:
            args.max_iterations = agent_job.max_iterations
        if agent_job.mode in ("llm", "gold"):
            args.mode = agent_job.mode
        if agent_job.run_id:
            args.run_id = agent_job.run_id
        if agent_job.llm_config_path:
            args.llm_config = agent_job.llm_config_path
        if agent_job.instances_catalog:
            args.instances = agent_job.instances_catalog
        elif agent_job.env_package_id:
            sync_root = os.environ.get("UENV_SWE_ENV_PACKAGE", "")
            if sync_root:
                cat = Path(sync_root) / "catalog.json"
                if cat.is_file():
                    args.instances = str(cat)

    if not args.instance:
        ap.error("--instance or AgentJob.instance_id is required")
    if not args.gateway and not (agent_job and agent_job.session_id):
        ap.error("--gateway or AgentJob.gateway_url/session_id is required")

    run_id = (args.run_id or "").strip() or f"run-oh-{time.strftime('%Y%m%d-%H%M%S')}-pro-{args.mode}"

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

    if args.gateway:
        client = UEnvGatewayClient(args.gateway, api_key=args.api_key, run_id=run_id)
        if not client.health():
            print("gateway health check failed", file=sys.stderr)
            return 1
    else:
        client = UEnvGatewayClient("http://127.0.0.1:1", api_key=args.api_key, run_id=run_id)

    llm = None
    if args.mode == "llm":
        if not args.llm_config:
            print("--llm-config required for llm mode", file=sys.stderr)
            return 1
        llm = load_llm_config(args.llm_config)
        logger.info("LLM model=%s", llm.model)

    session_id = agent_job.session_id if agent_job else None
    ws = UEnvWorkspace(
        working_dir=workspace_dir,
        gateway_url=args.gateway or (agent_job.gateway_url if agent_job else ""),
        instance_id=args.instance,
        benchmark_variant=args.benchmark_variant,
        api_key=args.api_key,
        run_id=run_id,
        session_id=session_id,
    )

    _save_json(
        out / "config_snapshot.json",
        {
            "gateway": args.gateway,
            "instance": args.instance,
            "mode": args.mode,
            "run_id": run_id,
            "session_id": session_id,
            "agent_job_file": args.agent_job_file or None,
            "max_iterations": args.max_iterations,
            "llm_model": str(llm.model) if llm else None,
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
            bundle = _fetch_trajectory_bundle(client, ref, out)
            if bundle:
                _save_json(out / "trajectory_bundle.json", bundle)

        server_doc = _verify_server_trajectory(
            (ref or {}).get("trajectory_id", ""),
            run_id,
            out,
        )

        with run_log.open("a", encoding="utf-8") as f:
            f.write(
                f"[done] mode={args.mode} reward={result.reward} "
                f"tests={result.tests_passed}/{result.tests_total} elapsed={elapsed:.1f}s "
                f"run_id={run_id} server_verified={server_doc.get('server_verified')}\n"
            )

        print(
            json.dumps(
                {
                    "resolved": result.resolved,
                    "reward": result.reward,
                    "tests_passed": result.tests_passed,
                    "tests_total": result.tests_total,
                    "run_id": run_id,
                    "trajectory_id": (ref or {}).get("trajectory_id"),
                    "upload_status": (ref or {}).get("upload_status"),
                    "server_verified": server_doc.get("server_verified"),
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
