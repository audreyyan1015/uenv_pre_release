#!/usr/bin/env python3
"""CodeEnv evaluator — inline test_code or DSCodeBench official-style harness."""

from __future__ import annotations

import json
import os
import re
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
    error_category: str | None = None,
) -> dict[str, Any]:
    out: dict[str, Any] = {
        "passed": passed,
        "tests_run": tests_run,
        "tests_passed": tests_passed,
        "execution_time_ms": int((time.time() - started) * 1000),
        "error": error,
    }
    if error_category:
        out["error_category"] = error_category
    return out


def _classify_exception(exc: BaseException, text: str) -> str:
    blob = f"{type(exc).__name__}: {text}".lower()
    if "timed out" in blob or "timeout" in blob:
        return "timeout"
    if any(
        key in blob
        for key in (
            "modulenotfounderror",
            "no module named",
            "importerror",
            "cannot import name",
        )
    ):
        return "dependency_error"
    if isinstance(exc, AssertionError):
        return "wrong_answer"
    return "candidate_runtime_error"


def _from_harness_dict(raw: dict[str, Any], started: float) -> dict[str, Any]:
    passed = bool(raw.get("passed"))
    tests_run = int(raw.get("tests_run") or 0)
    tests_passed = int(raw.get("tests_passed") or 0)
    error = raw.get("error")
    error_category = raw.get("error_category")
    if not error_category and not passed:
        if tests_run > 0 and tests_passed < tests_run:
            error_category = "wrong_answer"
        elif error and "no outputs" in str(error).lower():
            error_category = "candidate_runtime_error"
        elif error and any(
            k in str(error).lower()
            for k in ("modulenotfound", "import", "no module named")
        ):
            error_category = "dependency_error"
        elif error:
            error_category = "harness_error"
    return _result(
        passed,
        tests_run,
        tests_passed,
        started,
        None if error in (None, "") else str(error),
        None if error_category in (None, "", "none") else str(error_category),
    )


def _parse_assertion_result(exc: AssertionError) -> dict[str, Any] | None:
    """Recover structured harness result from legacy AssertionError payloads."""
    message = str(exc).strip()
    if not message:
        return None
    # Prefer a JSON object embedded in the message / traceback tail.
    candidates = [message]
    match = re.search(r"(\{.*\})\s*$", message, re.DOTALL)
    if match:
        candidates.insert(0, match.group(1))
    for candidate in candidates:
        try:
            raw = json.loads(candidate)
        except json.JSONDecodeError:
            continue
        if isinstance(raw, dict) and "passed" in raw:
            return raw
    return None


def run_inline_tests(cfg: dict[str, Any], started: float) -> dict[str, Any]:
    code = cfg["code"]
    test_code = cfg.get("test_code") or ""
    entry_point = cfg.get("entry_point")
    namespace: dict[str, Any] = {"__name__": "__uenv_eval__", "code": code}
    try:
        exec(compile(code, "<candidate>", "exec"), namespace)  # noqa: S102
    except Exception as exc:  # noqa: BLE001
        tb = traceback.format_exc()
        return _result(
            False,
            0,
            0,
            started,
            tb or str(exc),
            _classify_exception(exc, tb or str(exc)),
        )

    if entry_point and entry_point not in namespace:
        return _result(
            False,
            1,
            0,
            started,
            f"entry point {entry_point!r} not found in generated code",
            "candidate_runtime_error",
        )

    try:
        exec(compile(test_code, "<tests>", "exec"), namespace)  # noqa: S102
    except AssertionError as exc:
        parsed = _parse_assertion_result(exc)
        if parsed is not None:
            return _from_harness_dict(parsed, started)
        tb = traceback.format_exc()
        return _result(
            False,
            0,
            0,
            started,
            tb or str(exc),
            _classify_exception(exc, tb or str(exc)),
        )
    except Exception as exc:  # noqa: BLE001
        tb = traceback.format_exc()
        return _result(
            False,
            0,
            0,
            started,
            tb or str(exc),
            _classify_exception(exc, tb or str(exc)),
        )

    harness_result = namespace.get("_result")
    if isinstance(harness_result, dict) and "passed" in harness_result:
        return _from_harness_dict(harness_result, started)

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
        return _result(
            False,
            0,
            0,
            started,
            "test_script_path is required for official harness",
            "harness_error",
        )

    script_path = _resolve_under_root(benchmark_root, script_rel)
    if not script_path.is_file():
        return _result(
            False,
            0,
            0,
            started,
            f"test script not found: {script_path}",
            "harness_error",
        )

    ground_truth = cfg.get("ground_truth_code") or ""
    gt_rel = cfg.get("ground_truth_path") or ""
    if not ground_truth and gt_rel:
        gt_path = _resolve_under_root(benchmark_root, gt_rel)
        if not gt_path.is_file():
            return _result(
                False,
                0,
                0,
                started,
                f"ground_truth not found: {gt_path}",
                "harness_error",
            )
        ground_truth = _load_text(gt_path)
    if not ground_truth.strip():
        return _result(
            False,
            0,
            0,
            started,
            "official harness requires ground_truth_code or ground_truth_path",
            "harness_error",
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
    return _from_harness_dict(out, started)


def main() -> None:
    started = time.time()
    try:
        cfg = json.load(sys.stdin)
        if cfg.get("test_code"):
            result = run_inline_tests(cfg, started)
        elif cfg.get("test_script_path"):
            result = run_official_harness(cfg, started)
        else:
            result = _result(
                False,
                0,
                0,
                started,
                "need test_code or test_script_path",
                "harness_error",
            )
    except Exception as exc:  # noqa: BLE001
        tb = traceback.format_exc() or str(exc)
        result = _result(False, 0, 0, started, tb, _classify_exception(exc, tb))
    print(json.dumps(result), flush=True)


if __name__ == "__main__":
    main()
