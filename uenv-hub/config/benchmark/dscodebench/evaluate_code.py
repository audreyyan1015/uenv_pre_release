#!/usr/bin/env python3
"""CodeEnv evaluator — inline test_code or DSCodeBench test_script_path."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import time
import traceback
from pathlib import Path
from typing import Any


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
    exec(compile(code, "<candidate>", "exec"), namespace)
    if entry_point and entry_point not in namespace:
        return _result(
            False,
            1,
            0,
            started,
            f"entry point {entry_point!r} not found in generated code",
        )
    exec(compile(test_code, "<tests>", "exec"), namespace)
    tests_run = int(cfg.get("num_tests") or 1)
    return _result(True, tests_run, tests_run, started)


def run_test_script(cfg: dict[str, Any], started: float) -> dict[str, Any]:
    benchmark_root = cfg.get("benchmark_root") or os.environ.get("UENV_DSCODEBENCH_ROOT", "")
    script_path = cfg.get("test_script_path") or ""
    if benchmark_root:
        full_script = Path(benchmark_root) / script_path
    else:
        full_script = Path(script_path)
    if not full_script.is_file():
        return _result(
            False,
            0,
            0,
            started,
            f"test script not found: {full_script}",
        )

    harness_root = os.environ.get("UENV_DSCODEBENCH_EVAL_ROOT", "")
    official_runner = None
    if harness_root:
        candidate = Path(harness_root) / "benchmark_construction_evaluation" / "evaluate.py"
        if candidate.is_file():
            official_runner = candidate

    with tempfile.TemporaryDirectory(prefix="uenv-code-") as tmp:
        candidate_path = Path(tmp) / "candidate.py"
        candidate_path.write_text(cfg["code"], encoding="utf-8")
        num_tests = int(cfg.get("num_tests") or 10)
        random_seed = int(cfg.get("random_seed") or 42)

        if official_runner is not None:
            cmd = [
                sys.executable,
                str(official_runner),
                "--candidate",
                str(candidate_path),
                "--test-script",
                str(full_script),
                "--num-tests",
                str(num_tests),
                "--seed",
                str(random_seed),
            ]
        else:
            # Fallback: execute test script in subprocess with candidate injected.
            wrapper = Path(tmp) / "run_eval.py"
            wrapper.write_text(
                f"""import importlib.util, sys, traceback
candidate_path = {candidate_path!r}
test_script_path = {full_script!r}
spec = importlib.util.spec_from_file_location('candidate', candidate_path)
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)
ns = dict(mod.__dict__)
try:
    exec(open(test_script_path, encoding='utf-8').read(), ns)
    print(json.dumps({{"passed": True, "tests_run": {num_tests}, "tests_passed": {num_tests}}}))
except Exception as e:
    print(json.dumps({{"passed": False, "tests_run": {num_tests}, "tests_passed": 0, "error": str(e)}}))
    sys.exit(1)
""",
                encoding="utf-8",
            )
            cmd = [sys.executable, str(wrapper)]

        proc = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=int(cfg.get("timeout_secs") or 120),
        )
        if proc.returncode != 0:
            err = proc.stderr.strip() or proc.stdout.strip() or f"exit {proc.returncode}"
            return _result(False, num_tests, 0, started, err)
        try:
            out = json.loads(proc.stdout.strip().splitlines()[-1])
            out.setdefault("execution_time_ms", int((time.time() - started) * 1000))
            return out
        except (json.JSONDecodeError, IndexError):
            return _result(True, num_tests, num_tests, started)


def main() -> None:
    started = time.time()
    try:
        cfg = json.load(sys.stdin)
        if cfg.get("test_code"):
            result = run_inline_tests(cfg, started)
        elif cfg.get("test_script_path"):
            result = run_test_script(cfg, started)
        else:
            result = _result(False, 0, 0, started, "need test_code or test_script_path")
    except Exception as exc:  # noqa: BLE001
        result = _result(False, 0, 0, started, traceback.format_exc() or str(exc))
    print(json.dumps(result))


if __name__ == "__main__":
    main()
