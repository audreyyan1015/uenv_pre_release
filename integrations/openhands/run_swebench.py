#!/usr/bin/env python3
"""Run one SWE-bench instance through the UEnv gateway via ``UEnvRuntime``.

This is the integration entry an OpenHands ``swe_bench`` driver would call: it
creates a UEnv session, exposes the problem statement as the agent prompt, then
applies the agent's edits as OpenHands-shaped actions (``CmdRunAction`` /
``FileWriteAction``) routed through ``UEnvRuntime``, and finally submits for the
gateway-side grader (``swebench`` / ``swebench_pro``).

To make it runnable offline (no LLM), the "agent" here replays the gold patch
from the local catalog — proving the full runtime adapter path
(connect -> write -> run -> submit). Use ``--no-gold`` for the negative control.

Usage:
  python3 integrations/openhands/run_swebench.py \
      --gateway 127.0.0.1:48999 \
      --instance scikit-learn__scikit-learn-14141 \
      --instances fixtures/swe/swe_instances.json
"""

from __future__ import annotations

import argparse
import json
import sys
from dataclasses import dataclass

# Allow running as a script without installing the package.
sys.path.insert(0, __file__.rsplit("/", 1)[0])
from uenv_runtime import UEnvRuntime  # noqa: E402


# Minimal OpenHands-action-shaped objects (duck-typed by UEnvRuntime).
@dataclass
class CmdRunAction:
    command: str


@dataclass
class FileWriteAction:
    path: str
    content: str


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--gateway", default="127.0.0.1:48999")
    ap.add_argument("--instance", required=True)
    ap.add_argument("--instances", default="fixtures/swe/swe_instances.json")
    ap.add_argument("--benchmark-variant", default="verified")
    ap.add_argument("--command-mode", default="FullShell")
    ap.add_argument("--gold", dest="gold", action="store_true", default=True)
    ap.add_argument("--no-gold", dest="gold", action="store_false")
    args = ap.parse_args()

    with open(args.instances) as f:
        catalog = json.load(f)
    if args.instance not in catalog:
        print(f"instance {args.instance} not in {args.instances}", file=sys.stderr)
        return 1
    gold_patch = catalog[args.instance].get("patch", "")

    with UEnvRuntime(
        gateway_url=args.gateway,
        instance_id=args.instance,
        benchmark_variant=args.benchmark_variant,
        command_mode=args.command_mode,
    ) as rt:
        print(f"[connect] session={rt.session.session_id} variant={rt.session.benchmark_variant}")
        print(f"[prompt ] issue_text[:160]={rt.task_instruction[:160]!r}")

        if args.gold and gold_patch.strip():
            # Agent step 1: write the patch file (FileWriteAction -> Runtime.write)
            wobs = rt.run_action(FileWriteAction(path="/tmp/agent.patch", content=gold_patch))
            print(f"[write  ] {wobs}")
            # Agent step 2: apply it (CmdRunAction -> Runtime.run)
            robs = rt.run_action(
                CmdRunAction(
                    command="cd /testbed && (git apply -v /tmp/agent.patch "
                    "|| patch --batch --fuzz=5 -p1 < /tmp/agent.patch)"
                )
            )
            ec = robs.get("exit_code") if isinstance(robs, dict) else getattr(robs, "exit_code", "?")
            print(f"[run    ] git apply exit_code={ec}")
        else:
            print("[agent  ] (no edits — negative control)")

        result = rt.submit()
        print(
            f"[submit ] resolved={result.resolved} reward={result.reward} "
            f"tests={result.tests_passed}/{result.tests_total}"
        )
        for t in result.per_test:
            print(f"          [{'PASS' if t['passed'] else 'FAIL'}] {t['node_id']}")

    print(f"==== UEnvRuntime episode done: reward = {result.reward} ====")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
