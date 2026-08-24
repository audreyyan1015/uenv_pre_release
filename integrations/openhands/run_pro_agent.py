#!/usr/bin/env python3
"""OpenHands-shaped SWE-bench Pro agent via UEnv Gateway (gold or LLM).

Uses ``UEnvRuntime`` duck-typed actions (FileWriteAction / CmdRunAction) against
the Worker Runtime Gateway. On submit, persists ``TrajectoryRef`` and fetches the
full step bundle for acceptance.

Modes:
  gold — replay catalog patch (connectivity / grader smoke)
  llm  — call OpenAI-compatible LLM (``config/uenv-worker-llm.env``) to generate a patch

Usage (7143):
  python3 integrations/openhands/run_pro_agent.py \\
      --gateway 127.0.0.1:28999 --api-key swe-pro-secret \\
      --instance instance_NodeBB__... \\
      --instances config/swe/pro.json \\
      --mode llm \\
      --output-dir /tmp/uenv-pro-acceptance-run1
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
import urllib.error
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Optional

sys.path.insert(0, str(Path(__file__).resolve().parent))
from uenv_runtime import UEnvGatewayClient, UEnvRuntime  # noqa: E402


@dataclass
class CmdRunAction:
    command: str


@dataclass
class FileWriteAction:
    path: str
    content: str


def load_dotenv(path: Path) -> dict[str, str]:
    out: dict[str, str] = {}
    if not path.is_file():
        return out
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        k, v = line.split("=", 1)
        out[k.strip()] = v.strip().strip('"').strip("'")
    return out


def llm_chat(env: dict[str, str], system: str, user: str) -> str:
    endpoint = env.get("UENV_LLM_ENDPOINT", "https://openrouter.ai/api/v1").rstrip("/")
    model = env.get("UENV_LLM_MODEL_NAME", "qwen/qwen-2.5-7b-instruct")
    api_key = env.get("UENV_LLM_API_KEY", "")
    if not api_key:
        raise RuntimeError("UENV_LLM_API_KEY missing; set config/uenv-worker-llm.env")
    timeout = float(env.get("UENV_LLM_HTTP_TIMEOUT_SECS", "120"))
    max_tokens = int(env.get("UENV_LLM_MAX_TOKENS", "4096"))
    temperature = float(env.get("UENV_LLM_TEMPERATURE", "0.2"))
    url = f"{endpoint}/chat/completions"
    body = {
        "model": model,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user},
        ],
        "max_tokens": max_tokens,
        "temperature": temperature,
    }
    req = urllib.request.Request(
        url,
        data=json.dumps(body).encode(),
        method="POST",
        headers={
            "Content-Type": "application/json",
            "Authorization": f"Bearer {api_key}",
        },
    )
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        data = json.loads(resp.read().decode())
    return data["choices"][0]["message"]["content"]


def extract_patch(text: str) -> str:
    if "```" in text:
        parts = text.split("```")
        for i in range(1, len(parts), 2):
            chunk = parts[i]
            if chunk.startswith("diff"):
                chunk = chunk.split("\n", 1)[-1]
            elif chunk.startswith("\n"):
                chunk = chunk[1:]
            if "diff --git" in chunk or "@@" in chunk:
                return chunk.strip()
    if "diff --git" in text or "@@" in text:
        return text.strip()
    return text.strip()


def save_json(path: Path, obj: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(obj, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def append_log(path: Path, line: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a", encoding="utf-8") as f:
        f.write(line.rstrip() + "\n")


def run_gold(rt: UEnvRuntime, workspace: str, gold_patch: str, log: Path) -> None:
    append_log(log, "[agent] mode=gold")
    if not gold_patch.strip():
        append_log(log, "[agent] empty gold patch, skip edits")
        return
    wobs = rt.run_action(FileWriteAction(path="/tmp/agent.patch", content=gold_patch))
    append_log(log, f"[write] {json.dumps(wobs, ensure_ascii=False)[:500]}")
    cmd = (
        f"cd {workspace} && (git apply -v /tmp/agent.patch "
        "|| patch --batch --fuzz=5 -p1 < /tmp/agent.patch)"
    )
    robs = rt.run_action(CmdRunAction(command=cmd))
    append_log(log, f"[run  ] {json.dumps(robs, ensure_ascii=False)[:800]}")


def run_llm(rt: UEnvRuntime, workspace: str, issue_text: str, env: dict[str, str], log: Path) -> None:
    append_log(log, "[agent] mode=llm")
    system = (
        "You are a software engineer fixing a bug. Output ONLY a valid unified diff patch "
        "(git diff format) that modifies source files under the repository. "
        "Do not include explanations outside the patch."
    )
    user = f"Repository workspace: {workspace}\n\nProblem:\n{issue_text}\n\nProvide the patch:"
    append_log(log, f"[llm  ] requesting model={env.get('UENV_LLM_MODEL_NAME', '?')}")
    raw = llm_chat(env, system, user)
    save_json(log.parent / "llm_raw_response.txt", {"content": raw})
    patch = extract_patch(raw)
    append_log(log, f"[llm  ] patch_chars={len(patch)}")
    if not patch.strip():
        append_log(log, "[llm  ] empty patch from model")
        return
    wobs = rt.run_action(FileWriteAction(path="/tmp/agent.patch", content=patch))
    append_log(log, f"[write] {json.dumps(wobs, ensure_ascii=False)[:500]}")
    cmd = (
        f"cd {workspace} && (git apply -v /tmp/agent.patch "
        "|| patch --batch --fuzz=5 -p1 < /tmp/agent.patch)"
    )
    robs = rt.run_action(CmdRunAction(command=cmd))
    append_log(log, f"[run  ] {json.dumps(robs, ensure_ascii=False)[:800]}")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--gateway", default="127.0.0.1:28999")
    ap.add_argument("--instance", required=True)
    ap.add_argument("--instances", default="config/swe/pro.json")
    ap.add_argument("--benchmark-variant", default="pro")
    ap.add_argument("--command-mode", default="FullShell")
    ap.add_argument("--api-key", default=None)
    ap.add_argument("--mode", choices=["gold", "llm"], default="llm")
    ap.add_argument("--output-dir", required=True)
    ap.add_argument("--llm-env", default="config/uenv-worker-llm.env")
    args = ap.parse_args()

    out = Path(args.output_dir)
    run_log = out / "run.log"
    append_log(run_log, f"[start] ts={time.time()} instance={args.instance} mode={args.mode}")

    with open(args.instances, encoding="utf-8") as f:
        catalog = json.load(f)
    if args.instance not in catalog:
        append_log(run_log, f"[error] instance not in catalog")
        return 1
    row = catalog[args.instance]
    gold_patch = row.get("patch", "")
    workspace = "/app" if args.benchmark_variant.lower() == "pro" else "/testbed"

    llm_env = load_dotenv(Path(args.llm_env))
    save_json(out / "config_snapshot.json", {
        "gateway": args.gateway,
        "instance": args.instance,
        "mode": args.mode,
        "benchmark_variant": args.benchmark_variant,
        "llm_model": llm_env.get("UENV_LLM_MODEL_NAME"),
        "llm_endpoint": llm_env.get("UENV_LLM_ENDPOINT"),
    })

    client = UEnvGatewayClient(args.gateway, api_key=args.api_key)
    if not client.health():
        append_log(run_log, "[error] gateway health failed")
        return 1

    with UEnvRuntime(
        gateway_url=args.gateway,
        instance_id=args.instance,
        benchmark_variant=args.benchmark_variant,
        command_mode=args.command_mode,
        api_key=args.api_key,
    ) as rt:
        append_log(run_log, f"[connect] session={rt.session.session_id}")
        save_json(out / "reset_observation.json", rt.session.observation)

        if args.mode == "gold":
            run_gold(rt, workspace, gold_patch, run_log)
        else:
            run_llm(rt, workspace, rt.task_instruction, llm_env, run_log)

        result = rt.submit()
        append_log(
            run_log,
            f"[submit] resolved={result.resolved} reward={result.reward} "
            f"tests={result.tests_passed}/{result.tests_total}",
        )
        save_json(out / "submit_result.json", {
            "instance_id": result.instance_id,
            "resolved": result.resolved,
            "reward": result.reward,
            "tests_passed": result.tests_passed,
            "tests_total": result.tests_total,
            "per_test": result.per_test,
            "trajectory_ref": result.trajectory_ref,
        })

        ref = result.trajectory_ref
        if not ref or not ref.get("trajectory_id"):
            append_log(run_log, "[error] missing trajectory_ref in submit response")
            return 2

        save_json(out / "trajectory_ref.json", ref)
        tid = ref["trajectory_id"]
        try:
            bundle = client.get_trajectory(tid)
            save_json(out / "trajectory_bundle.json", bundle)
            append_log(run_log, f"[trace ] fetched trajectory_id={tid} steps={len(bundle.get('steps', []))}")
        except urllib.error.HTTPError as e:
            append_log(run_log, f"[error] get_trajectory HTTP {e.code}: {e.read().decode()[:500]}")
            return 3

        listed = client.list_trajectories(instance_id=args.instance, limit=5)
        save_json(out / "trajectory_list.json", listed)

    append_log(run_log, "[done] ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
