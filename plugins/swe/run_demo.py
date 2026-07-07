#!/usr/bin/env python3
"""End-to-end demo for the SWE OpenEnv plugin (plan §5.3.4).

Drives a SWE-bench instance through ``SweEnvironment`` over the Worker L4 gateway:
reset → apply gold patch (or skip for negative control) → evaluate → reward.

Usage:
  python3 plugins/swe/run_demo.py --gateway 127.0.0.1:48999 \
      --instance scikit-learn__scikit-learn-14141 \
      --instances fixtures/swe/swe_instances.json   [--no-gold]
"""

from __future__ import annotations

import argparse
import json
import os
import sys

sys.path.insert(0, os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..")))

from plugins.swe import SweAction, SweEnvironment  # noqa: E402
from plugins.swe.command_policy import CommandMode, CommandPolicy  # noqa: E402


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--gateway", default="127.0.0.1:48999")
    ap.add_argument("--instance", required=True)
    ap.add_argument("--instances", default="fixtures/swe/swe_instances.json")
    ap.add_argument("--benchmark-variant", default="verified")
    ap.add_argument("--gold", dest="gold", action="store_true", default=True)
    ap.add_argument("--no-gold", dest="gold", action="store_false")
    args = ap.parse_args()

    catalog = json.load(open(args.instances))
    gold = catalog.get(args.instance, {}).get("patch", "")

    env = SweEnvironment(
        instance_id=args.instance,
        gateway_url=args.gateway,
        benchmark_variant=args.benchmark_variant,
        policy=CommandPolicy(mode=CommandMode.FULL_SHELL),
    )
    obs = env.reset()
    print(f"[reset   ] session={obs.session_id} variant={obs.benchmark_variant}")
    print(f"[issue   ] {obs.issue_text[:140]!r}")
    try:
        if args.gold and gold.strip():
            r = env.step(SweAction(type="apply_patch", content=gold))
            print(f"[apply   ] exit_code={r.observation['exit_code']}")
        else:
            print("[agent   ] (no edits — negative control)")
        result = env.evaluate()
        print(f"[evaluate] resolved={result.resolved} reward={result.reward} "
              f"tests={result.tests_passed}/{result.tests_total}")
        for t in result.per_test:
            print(f"           [{'PASS' if t['passed'] else 'FAIL'}] {t['node_id']}")
    finally:
        env.close()
    print(f"==== plugins/swe episode done: reward = {result.reward} ====")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
