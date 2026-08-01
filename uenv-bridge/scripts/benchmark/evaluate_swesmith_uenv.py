#!/usr/bin/env python3
"""Evaluate SWE-smith through UEnv SWE+Agent (thin wrapper over evaluate_swebenchpro_uenv).

Defaults:
  benchmark_variant=smith
  workspace_dir=/testbed
  env_package_id=swe-bench-smith
  env_package_version=0.1.0-local
  driver_entrypoint=run_swebenchpro_official.py  # variant-aware (reverse gold for smith)
"""
from __future__ import annotations

import os
import sys
from pathlib import Path

# 本机较新的 protobuf 与仓库内旧版 *_pb2.py 不兼容时，回退纯 Python 实现。
os.environ.setdefault("PROTOCOL_BUFFERS_PYTHON_IMPLEMENTATION", "python")

ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(ROOT))

import evaluate_swebenchpro_uenv as base  # noqa: E402


DEFAULT_DATA = base.ROOT / "data/benchmarks/swesmith/smoke.jsonl"
DEFAULT_OUTPUT = base.ROOT / "temp/benchmarks/swesmith/uenv_agent_smoke"


def main() -> int:
    # Inject argv defaults before the shared parser runs.
    argv = sys.argv[1:]
    injected: list[str] = []
    flags = {argv[i] for i in range(len(argv)) if argv[i].startswith("--")}

    def need(flag: str, *values: str) -> None:
        if flag not in flags:
            injected.extend([flag, *values])

    need("--benchmark-variant", "smith")
    need("--workspace-dir", "/testbed")
    need("--env-package-id", "swe-bench-smith")
    need("--env-package-version", "0.1.0-local")
    need("--driver-entrypoint", "run_swebenchpro_official.py")
    if "--data" not in flags:
        injected.extend(["--data", str(DEFAULT_DATA)])
    if "--output-dir" not in flags:
        injected.extend(["--output-dir", str(DEFAULT_OUTPUT)])

    sys.argv = [sys.argv[0], *injected, *argv]
    return base.main()


if __name__ == "__main__":
    raise SystemExit(main())
