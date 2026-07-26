#!/usr/bin/env python3
"""Dedicated entrypoint for offline SWE-bench Pro trace collection.

This wrapper intentionally labels the operation as trace collection and
delegates to the protected scale-suite preflight/child orchestration without
changing its safety behavior.
"""

from __future__ import annotations

import sys

from uenv_stress.cli import run_scale_suite


def arguments(argv: list[str]) -> list[str]:
    return [
        "swebench-pro-trace-collection",
        *argv,
    ]


def main() -> int:
    return run_scale_suite.main(
        arguments(sys.argv[1:]),
        prog="run_trace_collection.py",
    )


if __name__ == "__main__":
    raise SystemExit(main())
