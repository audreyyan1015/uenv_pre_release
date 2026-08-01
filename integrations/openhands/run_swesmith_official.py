#!/usr/bin/env python3
"""SWE-smith OpenHands driver — thin wrapper around run_swebenchpro_official.

Defaults: benchmark_variant=smith, workspace via /testbed (AgentJob helper).
Gold path uses git apply -R (handled inside the Pro driver when variant=smith).
"""
from __future__ import annotations

import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))

# Re-exec shared entry with smith-friendly argv defaults.
argv = sys.argv[1:]
flags = {a for a in argv if a.startswith("--")}
injected: list[str] = []


def need(flag: str, *values: str) -> None:
    if flag not in flags:
        injected.extend([flag, *values])


need("--benchmark-variant", "smith")
sys.argv = [sys.argv[0], *injected, *argv]

from run_swebenchpro_official import main  # noqa: E402

if __name__ == "__main__":
    raise SystemExit(main())
