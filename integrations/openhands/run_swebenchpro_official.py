#!/usr/bin/env python3
"""Run an OpenHands SWE agent through the UEnv Worker Runtime Gateway.

End users should normally call ``examples/swe/evaluate.sh`` for evaluation or
``examples/swe/train_verl.sh`` for VeRL training. This module is their shared
low-level driver.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import tempfile
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
from uenv_runtime.gateway_tools import (  # noqa: E402
    collect_tool_patch_status,
    patch_openhands_tools_for_uenv,
)
from uenv_runtime.llm_rollout import (  # noqa: E402
    ROLLOUT_TRACE_MODES,
    finish_rollout_trace,
    start_rollout_trace,
)
from uenv_runtime.workspace import UEnvWorkspace  # noqa: E402
from uenv_runtime.workspace_probe import (  # noqa: E402
    merge_reset_observation,
    probe_workspace,
    validate_workspace_probe,
)


class WorkspaceProbeError(RuntimeError):
    """Container workspace does not match instance catalog (infrastructure error)."""


def _log_rollout_trace_warnings(logger: Any, status: dict[str, Any]) -> None:
    for warning in status.get("warnings") or []:
        logger.warning("%s", warning)


def _ensure_benchmarks_path() -> None:
    bench = os.environ.get("OPENHANDS_BENCHMARKS_DIR", "/opt/openhands/benchmarks")
    if bench not in sys.path:
        sys.path.insert(0, bench)


_MAX_LOCAL_CATALOG_BYTES = 64 * 1024 * 1024  # avoid loading multi-GB EnvPackage on Agent hosts


def _load_catalog(path: Path, instance_id: str) -> dict[str, Any]:
    data = json.loads(path.read_text(encoding="utf-8"))
    if instance_id not in data:
        raise SystemExit(f"instance {instance_id!r} not in {path}")
    return data[instance_id]


def _catalog_contains(path: Path, instance_id: str) -> bool:
    if not path.is_file():
        return False
    try:
        if path.stat().st_size > _MAX_LOCAL_CATALOG_BYTES:
            return False
        data = json.loads(path.read_text(encoding="utf-8"))
    except Exception:
        return False
    return isinstance(data, dict) and instance_id in data


def _smith_catalog_candidates(repo_root: Path) -> list[Path]:
    """Prefer EnvPackage / env overrides; keep smoke fixtures as last resort only."""
    env_pkg = os.environ.get("UENV_SWE_ENV_PACKAGE", "").strip()
    env_cat = os.environ.get("UENV_SWE_ENV_PACKAGE_CATALOG", "").strip()
    out: list[Path] = []
    if env_cat:
        out.append(Path(env_cat))
    if env_pkg:
        out.append(Path(env_pkg) / "catalog.json")
    out.extend(
        [
            Path("/var/lib/uenv/envs/swe-bench-smith/0.1.0-local/catalog.json"),
            Path("/data/uenv/envs/swe-bench-smith/0.1.0-local/catalog.json"),
            # Smoke fixtures last — never prefer these over a full EnvPackage.
            repo_root / "fixtures/swe/smith_catalog.json",
            repo_root / "config/swe/smith-smoke.json",
        ]
    )
    # de-dupe while preserving order
    seen: set[str] = set()
    uniq: list[Path] = []
    for p in out:
        key = str(p)
        if key in seen:
            continue
        seen.add(key)
        uniq.append(p)
    return uniq


def _resolve_instances_catalog(
    *,
    variant: str,
    explicit: str,
    repo_root: Path,
    instance_id: str = "",
) -> Path:
    """Prefer explicit path that contains the target; for smith fall back to EnvPackage then fixture."""
    from uenv_runtime.agent_job import normalize_benchmark_variant

    if explicit.strip():
        path = Path(explicit)
        if not path.is_absolute():
            path = repo_root / path
        if not instance_id or _catalog_contains(path, instance_id):
            return path
    if normalize_benchmark_variant(variant) == "smith":
        for cand in _smith_catalog_candidates(repo_root):
            if not cand.is_file():
                continue
            if instance_id and not _catalog_contains(cand, instance_id):
                continue
            return cand
        # Last resort: first existing candidate (may still miss instance → gateway fetch).
        for cand in _smith_catalog_candidates(repo_root):
            if cand.is_file():
                return cand
    return repo_root / "config/swe/pro-python-smoke.json"


def _fetch_instance_via_gateway(
    *,
    gateway: str,
    api_key: str | None,
    instance_id: str,
    run_id: str,
) -> dict[str, Any]:
    from uenv_runtime.client import UEnvGatewayClient

    client = UEnvGatewayClient(gateway, api_key=api_key, run_id=run_id)
    row = client.get_instance(instance_id)
    if not isinstance(row, dict) or not row.get("instance_id"):
        raise SystemExit(f"gateway returned invalid instance payload for {instance_id!r}")
    return row


def _write_mini_catalog(out_dir: Path, instance_id: str, row: dict[str, Any]) -> Path:
    out_dir.mkdir(parents=True, exist_ok=True)
    path = out_dir / "instance_catalog.json"
    path.write_text(
        json.dumps({instance_id: row}, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    return path


def _workspace_dir(variant: str, override: str = "") -> str:
    from uenv_runtime.agent_job import resolve_workspace_dir

    return resolve_workspace_dir(variant, override)


def _infer_repo_language(instance: dict[str, Any]) -> str:
    lang = str(instance.get("repo_language") or "").strip().lower()
    if lang:
        return lang
    repo = str(instance.get("repo") or "").lower()
    if repo.endswith("/go") or ".go" in repo:
        return "go"
    # Common Pro repos when catalog omits repo_language.
    go_repos = (
        "flipt-io/flipt",
        "gravitational/teleport",
        "navidrome/navidrome",
        "future-architect/vuls",
    )
    js_repos = (
        "element-hq/element-web",
        "nodebb/nodebb",
        "protonmail/webclients",
        "internetarchive/openlibrary",
    )
    if any(repo == r or repo.endswith(r) for r in go_repos):
        return "go"
    if any(repo == r or repo.endswith(r) for r in js_repos):
        return "javascript"
    if "tutao" in repo:
        return "typescript"
    if "ansible" in repo or "qutebrowser" in repo:
        return "python"
    return ""


def _build_instruction(instance: dict[str, Any], repo_path: str) -> str:
    ps = instance.get("problem_statement") or instance.get("issue_text") or ""
    repo = str(instance.get("repo") or "")
    repo_language = _infer_repo_language(instance)
    if repo_language in {"python", "py"}:
        language_hint = "Python files such as `*.py`"
    elif repo_language in {"go", "golang"}:
        language_hint = "Go files such as `*.go`"
    elif repo_language in {"javascript", "js", "typescript", "ts"}:
        language_hint = "JavaScript/TypeScript files such as `*.js`, `*.ts`, and `*.tsx`"
    else:
        language_hint = "files matching the repository language and nearby config/template files"
    repo_line = f"Verified repository: `{repo}`.\n" if repo else ""
    forbid_ol = ""
    if "openlibrary" not in (repo or "").lower() and "openlibrary" not in str(
        instance.get("instance_id") or ""
    ).lower():
        forbid_ol = (
            "CRITICAL: This is NOT the openlibrary repository. "
            "Never create or edit paths containing `openlibrary`. "
            "If a tool suggests openlibrary paths, ignore them and stay in this repo.\n"
        )
    return (
        f"The git repository is already checked out at `{repo_path}`.\n"
        f"{repo_line}"
        f"All investigation and edits must stay under `{repo_path}`.\n"
        f"{forbid_ol}"
        "Start by confirming the workspace:\n"
        f"1. `pwd`\n"
        f"2. `git -C {repo_path} remote get-url origin`\n"
        f"3. `git -C {repo_path} rev-parse --show-toplevel`\n"
        f"4. `ls -la {repo_path}`\n\n"
        "Inspect the repository structure and identify the relevant language/framework before searching.\n"
        f"This instance is labeled as `{repo_language or 'unknown'}`; prioritize {language_hint}.\n"
        "Use targeted searches with `rg` for symbols, error messages, routes, tests, or issue keywords.\n"
        "When relevant, also inspect non-test project files such as JSON, YAML, templates, and generated schemas.\n"
        f"Do not search or edit outside `{repo_path}`. "
        "Do NOT `git clone` or copy the repo under `/tmp` (or anywhere else); "
        f"work only inside the existing checkout at `{repo_path}`. "
        "Do not inspect `/opt/openhands`, benchmark harness directories, `/tmp`, or `/root` "
        "unless a tool explicitly requires a temp file under `/tmp`.\n\n"
        f"<issue_description>\n{ps}\n</issue_description>\n\n"
        "Implement the minimal fix in **non-test project source files** required by the issue.\n"
        "Do NOT modify files under `tests/`, `test/`, `*_test.*`, or `*.test.*` unless the issue text explicitly requires test changes.\n"
        "Do NOT only update whitelists, scripts, or docs if the issue asks for library/runtime behavior changes.\n"
        "Before finishing, run `git -C {repo_path} remote get-url origin` and `git -C {repo_path} diff --stat`, "
        "and confirm the diff is in the correct repository and touches the intended source paths.\n"
        "Use terminal and file_editor tools. When done, call the finish tool.\n"
    )


def _save_json(path: Path, obj: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(obj, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


_LLM_PROVIDER_PREFIXES = frozenset(
    {
        "anthropic",
        "ark",
        "azure",
        "bedrock",
        "deepseek",
        "gemini",
        "huggingface",
        "ollama",
        "openai",
        "together_ai",
        "vertex_ai",
        "vllm",
        "volcengine",
    }
)
_DIRECT_LLM_GENERATION_KEYS = (
    "temperature",
    "top_p",
    "top_k",
    "seed",
    "thinking_token_budget",
)
_MAX_OUTPUT_TOKEN_KEYS = (
    "max_output_tokens",
    "max_new_tokens",
    "max_tokens",
    "max_completion_tokens",
)


def _effective_model_name(
    template_model: str,
    requested_model: str,
    endpoint_type: str,
) -> str:
    """Keep the template's LiteLLM provider prefix for a dynamic model name."""

    requested = requested_model.strip()
    if not requested:
        return template_model
    requested_prefix = requested.partition("/")[0].lower()
    if requested_prefix in _LLM_PROVIDER_PREFIXES:
        return requested

    template_prefix = template_model.partition("/")[0].lower()
    if template_prefix in _LLM_PROVIDER_PREFIXES:
        return f"{template_prefix}/{requested}"
    if endpoint_type.strip().lower() in {
        "http",
        "openai",
        "openai-compatible",
        "openai_compatible",
    }:
        return f"openai/{requested}"
    return requested


def _write_private_json(path: Path, value: dict[str, Any]) -> None:
    """Atomically write a secret-bearing JSON file with mode 0600."""

    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(
        prefix=f".{path.name}.", suffix=".tmp", dir=str(path.parent)
    )
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as stream:
            json.dump(value, stream, indent=2, ensure_ascii=False)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        os.chmod(temporary, 0o600)
        os.replace(temporary, path)
        os.chmod(path, 0o600)
    finally:
        try:
            os.close(fd)
        except OSError:
            pass
        try:
            os.unlink(temporary)
        except FileNotFoundError:
            pass


def _write_effective_llm_config(
    *,
    agent_job: Any,
    template_path: str | Path,
    output_dir: Path,
) -> Path:
    """Overlay an AgentJob model endpoint onto an existing OpenHands config.

    The template remains the authority for credentials and provider-specific
    settings.  Only typed endpoint fields and generation options understood by
    the OpenHands LLM config are replaced.
    """

    source = Path(template_path)
    try:
        template = json.loads(source.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise ValueError(f"LLM config is not valid JSON: {source}") from exc
    if not isinstance(template, dict):
        raise ValueError(f"LLM config must be a JSON object: {source}")

    effective = dict(template)
    effective["base_url"] = str(agent_job.model_endpoint)
    if agent_job.model_name:
        effective["model"] = _effective_model_name(
            str(template.get("model") or ""),
            str(agent_job.model_name),
            str(agent_job.model_endpoint_type or ""),
        )

    generation = agent_job.generation_config or {}
    if not isinstance(generation, dict):
        raise ValueError("AgentJob generation_config must be a JSON object")
    for key in _DIRECT_LLM_GENERATION_KEYS:
        if key in generation and generation[key] is not None:
            effective[key] = generation[key]
    for key in _MAX_OUTPUT_TOKEN_KEYS:
        if key in generation and generation[key] is not None:
            effective["max_output_tokens"] = generation[key]
            break
    if int(agent_job.model_max_retries or 0) > 0:
        effective["num_retries"] = int(agent_job.model_max_retries)

    destination = output_dir / "effective_llm_config.json"
    _write_private_json(destination, effective)
    return destination


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


def _run_conversation_loop(
    conversation,
    max_fake_responses: int = 5,
) -> dict[str, Any]:
    """Like benchmarks fake_user_response helper but compatible with LocalConversation."""
    from benchmarks.utils.fake_user_response import (
        _agent_finished_with_finish_action,
        _agent_sent_message,
        fake_user_response,
    )
    from openhands.sdk.conversation.state import ConversationExecutionStatus

    if max_fake_responses < 0:
        raise ValueError("max_fake_responses must be non-negative")
    fake_count = 0
    termination_reason = "unknown"
    while True:
        conversation.run()
        status = conversation.state.execution_status
        if status != ConversationExecutionStatus.FINISHED:
            termination_reason = (
                "execution_status_"
                + str(getattr(status, "value", status)).lower()
            )
            break
        events = list(conversation.state.events)
        if _agent_finished_with_finish_action(events):
            termination_reason = "finish_action"
            break
        if not _agent_sent_message(events):
            termination_reason = "no_agent_message"
            break
        if fake_count >= max_fake_responses:
            termination_reason = "fake_response_limit"
            break
        msg = fake_user_response(conversation)
        if msg == "/exit":
            termination_reason = "fake_user_exit"
            break
        conversation.send_message(msg)
        fake_count += 1
    return {
        "termination_reason": termination_reason,
        "fake_user_responses": fake_count,
        "max_fake_responses": max_fake_responses,
    }


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
        default=os.environ.get("UENV_SWE_INSTANCES", "")
        or os.environ.get("UENV_SWE_ENV_PACKAGE_CATALOG", ""),
        help="Local catalog JSON; empty → smith resolves EnvPackage/fixture or Gateway fetch",
    )
    ap.add_argument("--benchmark-variant", default="pro")
    ap.add_argument("--output-dir", required=True)
    ap.add_argument("--max-iterations", type=int, default=30)
    ap.add_argument("--mode", choices=["llm", "gold"], default="llm")
    ap.add_argument(
        "--rollout-trace",
        choices=ROLLOUT_TRACE_MODES,
        default=os.environ.get("UENV_ROLLOUT_TRACE", "best-effort"),
        help=(
            "LLM token trace: off disables injection/collection; best-effort records "
            "warnings without failing an evaluation; required fails if a training trace "
            "cannot be produced (default: UENV_ROLLOUT_TRACE or best-effort)"
        ),
    )
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
    patch_status = patch_openhands_tools_for_uenv()

    from benchmarks.utils.llm_config import load_llm_config
    from openhands.sdk import Agent, Conversation, Tool, get_logger
    from openhands.tools.file_editor import FileEditorTool
    from openhands.tools.task_tracker import TaskTrackerTool
    from openhands.tools.terminal import TerminalTool

    logger = get_logger(__name__)

    out = Path(args.output_dir)
    run_log = out / "run.log"
    repo_root = Path(os.environ.get("UENV_REPO", "/root/UEnv"))
    # CLI/default instances may still point at Pro smoke; resolve after AgentJob variant override.
    from uenv_runtime.agent_job import normalize_benchmark_variant

    catalog_path = Path(args.instances) if args.instances else Path()
    if args.instances and not catalog_path.is_absolute():
        catalog_path = repo_root / catalog_path

    catalog_source = str(catalog_path) if args.instances else ""
    row: dict[str, Any] | None = None

    # 1) Official path: AgentJob.instance_catalog_json from Server/Worker for-episode.
    if agent_job and (agent_job.instance_catalog_json or "").strip():
        try:
            payload = json.loads(agent_job.instance_catalog_json)
        except json.JSONDecodeError as exc:
            raise SystemExit(
                f"AgentJob.instance_catalog_json is not valid JSON: {exc}"
            ) from exc
        if isinstance(payload, dict) and args.instance in payload and isinstance(
            payload[args.instance], dict
        ):
            row = payload[args.instance]
            mini = _write_mini_catalog(out, args.instance, row)
            catalog_path = mini
            args.instances = str(mini)
            catalog_source = f"agent_job.instance_catalog_json -> {mini}"
            print(f"[catalog] using AgentJob.instance_catalog_json -> {mini}", flush=True)
        elif isinstance(payload, dict) and payload.get("instance_id") == args.instance:
            # Allow a bare SweInstance object as well as a mini catalog map.
            row = payload
            mini = _write_mini_catalog(out, args.instance, row)
            catalog_path = mini
            args.instances = str(mini)
            catalog_source = f"agent_job.instance_catalog_json(row) -> {mini}"
            print(f"[catalog] using AgentJob.instance_catalog_json row -> {mini}", flush=True)
        else:
            print(
                f"[catalog] AgentJob.instance_catalog_json missing {args.instance}; "
                "falling back to local/gateway",
                flush=True,
            )

    if row is None and normalize_benchmark_variant(args.benchmark_variant) == "smith" and not (
        agent_job and agent_job.instances_catalog
    ):
        catalog_path = _resolve_instances_catalog(
            variant=args.benchmark_variant,
            explicit=str(args.instances or ""),
            repo_root=repo_root,
            instance_id=args.instance,
        )
        args.instances = str(catalog_path)
        catalog_source = str(catalog_path)

    if row is None:
        if args.instances and _catalog_contains(catalog_path, args.instance):
            row = _load_catalog(catalog_path, args.instance)
            catalog_source = str(catalog_path)
        else:
            # Agent hosts (e.g. 208.77) often only have smoke fixtures; fetch one row
            # from Worker Gateway which already loaded the full EnvPackage catalog.
            gw = (args.gateway or (agent_job.gateway_url if agent_job else "") or "").strip()
            if not gw:
                raise SystemExit(
                    f"instance {args.instance!r} not in {catalog_path or '(no local catalog)'} "
                    "and no gateway / AgentJob.instance_catalog_json available"
                )
            print(
                f"[catalog] local miss for {args.instance} in {catalog_path or '(empty)'}; "
                f"fetching via gateway {gw}",
                flush=True,
            )
            row = _fetch_instance_via_gateway(
                gateway=gw,
                api_key=args.api_key,
                instance_id=args.instance,
                run_id=run_id,
            )
            mini = _write_mini_catalog(out, args.instance, row)
            catalog_path = mini
            args.instances = str(mini)
            catalog_source = f"gateway:{gw} -> {mini}"
            print(f"[catalog] wrote mini catalog {mini}", flush=True)

    _save_json(
        out / "catalog_resolve.json",
        {
            "instance_id": args.instance,
            "catalog_path": str(catalog_path),
            "catalog_source": catalog_source,
            "env_package_id": getattr(agent_job, "env_package_id", "") if agent_job else "",
            "benchmark_variant": args.benchmark_variant,
            "has_agent_job_catalog_json": bool(
                agent_job and (agent_job.instance_catalog_json or "").strip()
            ),
        },
    )

    job_workspace = agent_job.workspace_dir if agent_job else ""
    workspace_dir = _workspace_dir(args.benchmark_variant, job_workspace)

    if args.gateway:
        client = UEnvGatewayClient(args.gateway, api_key=args.api_key, run_id=run_id)
        if not client.health():
            print("gateway health check failed", file=sys.stderr)
            return 1
    else:
        client = UEnvGatewayClient("http://127.0.0.1:1", api_key=args.api_key, run_id=run_id)

    llm = None
    rollout_collector = None
    rollout_fields: dict[str, Any] = {}
    rollout_status: dict[str, Any] = {
        "mode": args.rollout_trace,
        "enabled": False,
        "state": "not_applicable",
        "warnings": [],
    }
    if args.mode == "llm":
        if not args.llm_config:
            print("--llm-config required for llm mode", file=sys.stderr)
            return 1
        if agent_job and agent_job.model_endpoint:
            try:
                args.llm_config = str(
                    _write_effective_llm_config(
                        agent_job=agent_job,
                        template_path=args.llm_config,
                        output_dir=out,
                    )
                )
            except (OSError, ValueError) as exc:
                print(f"failed to build effective LLM config: {exc}", file=sys.stderr)
                return 1
        llm = load_llm_config(args.llm_config)
        episode_id = str(agent_job.episode_id) if agent_job and agent_job.episode_id else run_id
        dataset = (
            "swesmith"
            if str(args.benchmark_variant).lower() in {"smith", "swesmith", "swe-smith"}
            else "swebench_pro"
        )
        rollout_collector, rollout_status = start_rollout_trace(
            args.rollout_trace,
            llm=llm,
            config_path=args.llm_config,
            episode_id=episode_id,
            dataset=dataset,
        )
        _log_rollout_trace_warnings(logger, rollout_status)
        _save_json(out / "rollout_trace_status.json", rollout_status)
        logger.info("LLM model=%s", llm.model)

    session_id = agent_job.session_id if agent_job else None
    ws = UEnvWorkspace(
        working_dir="/tmp/uenv-oh-local-ws",
        container_working_dir=workspace_dir,
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
            "rollout_trace_mode": args.rollout_trace,
            "rollout_trace_state": rollout_status["state"],
            "benchmark_variant": args.benchmark_variant,
            "repo": row.get("repo"),
            "base_commit": row.get("base_commit"),
            "patch_openhands_status": patch_status,
        },
    )

    def _run_workspace_checks() -> None:
        probe = probe_workspace(ws, workspace_dir)
        probe_doc = {
            "instance_id": args.instance,
            "session_id": ws.session.session_id,
            "repo": row.get("repo"),
            "base_commit": row.get("base_commit"),
            "workspace_dir": workspace_dir,
            **probe,
        }
        _save_json(out / "workspace_probe.json", probe_doc)
        ok, reason = validate_workspace_probe(
            probe,
            instance_id=args.instance,
            repo=str(row.get("repo") or ""),
            base_commit=str(row.get("base_commit") or ""),
        )
        reset_doc = merge_reset_observation(
            ws.session.observation,
            probe,
            instance_id=args.instance,
            session_id=ws.session.session_id,
            repo=str(row.get("repo") or ""),
            base_commit=str(row.get("base_commit") or ""),
            ok=ok,
            reason=reason,
        )
        _save_json(out / "reset_observation.json", reset_doc)
        if not ok:
            raise WorkspaceProbeError(reason)

    t0 = time.time()
    try:
        with ws:
            _run_workspace_checks()

            if args.mode == "gold":
                patch = row.get("patch", "")
                if patch.strip():
                    ws.write_remote_text("/tmp/gold.patch", patch)
                    # SWE-smith：数据集 patch 为造 bug 补丁；Worker provision 已注入，
                    # gold 用 git apply -R 还原。Pro/Verified 仍正向应用 gold。
                    from uenv_runtime.agent_job import normalize_benchmark_variant

                    is_smith = normalize_benchmark_variant(args.benchmark_variant) == "smith"
                    if is_smith:
                        apply_cmd = (
                            f"cd {workspace_dir} && git apply -R -v /tmp/gold.patch && "
                            f"source /opt/miniconda3/bin/activate testbed 2>/dev/null; "
                            f"cd {workspace_dir} && pip install -e . -q"
                        )
                    else:
                        apply_cmd = (
                            "git apply -v /tmp/gold.patch || "
                            "patch --batch --fuzz=5 -p1 < /tmp/gold.patch"
                        )
                    r = ws.execute_command(apply_cmd)
                    _save_json(
                        out / "gold_apply.json",
                        {
                            **(r.model_dump() if hasattr(r, "model_dump") else {"raw": str(r)}),
                            "reverse": is_smith,
                            "workspace_dir": workspace_dir,
                        },
                    )
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
                tool_status = collect_tool_patch_status(conversation.state)
                tool_status["patch_openhands_status"] = patch_status
                _save_json(out / "tool_patch_status.json", tool_status)
                if not tool_status.get("patch_ok"):
                    raise WorkspaceProbeError(
                        "OpenHands tools not routed to UEnv Gateway: "
                        f"terminal={tool_status.get('terminal_executor')} "
                        f"file_editor={tool_status.get('file_editor_executor')}"
                    )
                instruction = _build_instruction(row, workspace_dir)
                _save_json(out / "instruction.txt", {"text": instruction})
                conversation.send_message(instruction)
                loop_summary = _run_conversation_loop(
                    conversation, max_fake_responses=max(args.max_iterations - 1, 0)
                )
                if rollout_collector is not None:
                    rollout_fields, rollout_status = finish_rollout_trace(
                        args.rollout_trace, rollout_collector
                    )
                    _log_rollout_trace_warnings(logger, rollout_status)
                    _save_json(out / "rollout_trace_status.json", rollout_status)
                _save_json(
                    out / "conversation_events.json",
                    {
                        "count": len(list(conversation.state.events)),
                        "model_response_count": len(rollout_fields.get("turns", [])),
                        **loop_summary,
                    },
                )
                if rollout_fields:
                    _save_json(out / "llm_rollout_trace.json", rollout_fields)
                # Pre-submit: confirm agent stayed in the right repo.
                # Pro=/app；Verified/Lite/Smith=/testbed — 勿写死 /app。
                pre = ws.execute_command(
                    f"git -C {workspace_dir} remote get-url origin; "
                    f"git -C {workspace_dir} status --short | head -40; "
                    f"git -C {workspace_dir} diff --stat | tail -20"
                )
                _save_json(
                    out / "pre_submit_git.json",
                    {
                        "exit_code": pre.exit_code,
                        "stdout": pre.stdout,
                        "stderr": pre.stderr,
                    },
                )
                remote = (pre.stdout or "").splitlines()[0] if pre.stdout else ""
                expected_repo = str(row.get("repo") or "")
                if remote and expected_repo and expected_repo.split("/")[-1].lower() not in remote.lower():
                    raise WorkspaceProbeError(
                        f"pre-submit remote mismatch: got {remote!r}, expected repo {expected_repo!r}"
                    )
                if (
                    "openlibrary" not in str(args.instance).lower()
                    and "openlibrary" in (pre.stdout or "").lower()
                ):
                    raise WorkspaceProbeError(
                        "pre-submit git status references openlibrary on a non-openlibrary instance"
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
        submit_doc.update(rollout_fields)
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

    except WorkspaceProbeError as e:
        with run_log.open("a", encoding="utf-8") as f:
            f.write(f"[workspace_probe_error] {e!s}\n")
        _save_json(out / "infrastructure_error.json", {"error": str(e), "kind": "workspace_probe"})
        print(json.dumps({"error": str(e), "kind": "workspace_probe", "output_dir": str(out)}))
        return 2
    except Exception as e:
        with run_log.open("a", encoding="utf-8") as f:
            f.write(f"[error] {e!r}\n")
        raise


if __name__ == "__main__":
    raise SystemExit(main())
