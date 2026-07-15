#!/usr/bin/env python3
"""CodeEnv evaluator — inline test_code or DSCodeBench official-style harness."""

from __future__ import annotations

import json
import os
import sys
import time
import traceback
from pathlib import Path
from typing import Any

# Allow `import dscodebench_harness` when spawned with this script's directory.
_SCRIPTS_DIR = Path(__file__).resolve().parent
if str(_SCRIPTS_DIR) not in sys.path:
    sys.path.insert(0, str(_SCRIPTS_DIR))

from dscodebench_harness import evaluate_problem  # noqa: E402


def _result(
    passed: bool,
    tests_run: int,
    tests_passed: int,
    started: float,
    error: str | None = None,
) -> dict[str, Any]:
    return {
        "passed": passed,
        "tests_run": tests_run,
        "tests_passed": tests_passed,
        "execution_time_ms": int((time.time() - started) * 1000),
        "error": error,
    }


def run_inline_tests(cfg: dict[str, Any], started: float) -> dict[str, Any]:
    code = cfg["code"]
    test_code = cfg.get("test_code") or ""
    entry_point = cfg.get("entry_point")
    namespace: dict[str, Any] = {"__name__": "__uenv_eval__"}
    exec(compile(code, "<candidate>", "exec"), namespace)  # noqa: S102
    if entry_point and entry_point not in namespace:
        return _result(
            False,
            1,
            0,
            started,
            f"entry point {entry_point!r} not found in generated code",
        )
    exec(compile(test_code, "<tests>", "exec"), namespace)  # noqa: S102
    tests_run = int(cfg.get("num_tests") or 1)
    return _result(True, tests_run, tests_run, started)


def _resolve_under_root(root: str, rel_or_abs: str) -> Path:
    path = Path(rel_or_abs)
    if path.is_file():
        return path
    if root:
        candidate = Path(root) / rel_or_abs
        if candidate.is_file():
            return candidate
    return path


def _load_text(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def run_official_harness(cfg: dict[str, Any], started: float) -> dict[str, Any]:
    """Official DSCodeBench path: ground_truth + test_script with generator."""
    benchmark_root = cfg.get("benchmark_root") or os.environ.get("UENV_DSCODEBENCH_ROOT", "")
    script_rel = cfg.get("test_script_path") or ""
    if not script_rel:
        return _result(False, 0, 0, started, "test_script_path is required for official harness")

    script_path = _resolve_under_root(benchmark_root, script_rel)
    if not script_path.is_file():
        return _result(False, 0, 0, started, f"test script not found: {script_path}")

    ground_truth = cfg.get("ground_truth_code") or ""
    gt_rel = cfg.get("ground_truth_path") or ""
    if not ground_truth and gt_rel:
        gt_path = _resolve_under_root(benchmark_root, gt_rel)
        if not gt_path.is_file():
            return _result(False, 0, 0, started, f"ground_truth not found: {gt_path}")
        ground_truth = _load_text(gt_path)
    if not ground_truth.strip():
        return _result(
            False,
            0,
            0,
            started,
            "official harness requires ground_truth_code or ground_truth_path",
        )

    num_tests = int(cfg.get("num_tests") or 200)
    random_seed = int(cfg.get("random_seed") or 42)
    test_script = _load_text(script_path)

    out = evaluate_problem(
        ground_truth_code=ground_truth,
        candidate_code=cfg["code"],
        test_script=test_script,
        num_tests=num_tests,
        random_seed=random_seed,
    )
    out["execution_time_ms"] = int((time.time() - started) * 1000)
    return out


def main() -> None:
    started = time.time()
    try:
        cfg = json.load(sys.stdin)
        if cfg.get("test_code"):
            result = run_inline_tests(cfg, started)
        elif cfg.get("test_script_path"):
            result = run_official_harness(cfg, started)
        else:
            result = _result(False, 0, 0, started, "need test_code or test_script_path")
    except Exception as exc:  # noqa: BLE001
        result = _result(False, 0, 0, started, traceback.format_exc() or str(exc))
    print(json.dumps(result), flush=True)


if __name__ == "__main__":
    main()
