"""SWE evaluator (plan §5.3.4 / §7).

Primary path: delegate grading to the Worker gateway ``submit`` (which applies
``test_patch``, runs tests, and grades with the variant grader ``swebench`` /
``swebench_pro``). This keeps a single source of truth for reward.

Also exposes ``parse_pytest_report`` — a pure-Python mirror of the Rust pytest
parser — used by offline unit tests and as a fallback when grading from a raw log
(e.g. an externally captured test run).
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Dict, List, Optional

# pytest statuses that count as "passed" for resolution purposes.
_PASS_STATUSES = {"PASSED", "XFAIL", "XPASS"}
_ALL_STATUSES = _PASS_STATUSES | {"FAILED", "ERROR", "SKIPPED"}


@dataclass
class EvalResult:
    instance_id: str
    resolved: bool
    reward: float
    tests_passed: int
    tests_total: int
    per_test: List[dict] = field(default_factory=list)


def parse_pytest_report(output: str) -> Dict[str, bool]:
    """Map pytest node id -> passed?, tolerant of verbose and summary formats.

    Handles ``a.py::test_x PASSED`` and ``PASSED a.py::test_x`` line shapes.
    """
    report: Dict[str, bool] = {}
    for line in output.splitlines():
        line = line.strip()
        if "::" not in line:
            continue
        tokens = line.replace("\t", " ").split()
        node_id = None
        status = None
        for tok in tokens:
            if "::" in tok and node_id is None:
                node_id = tok
            up = tok.strip("[].").upper()
            if up in _ALL_STATUSES:
                status = up
        if node_id is not None and status is not None:
            report[node_id] = status in _PASS_STATUSES
    return report


class SweEvaluator:
    """Grade a SWE session via the gateway, or grade a raw log offline."""

    def evaluate(self, session) -> EvalResult:
        r = session.submit()
        return EvalResult(
            instance_id=r.instance_id,
            resolved=r.resolved,
            reward=r.reward,
            tests_passed=r.tests_passed,
            tests_total=r.tests_total,
            per_test=r.per_test,
        )

    def grade_log(
        self,
        instance_id: str,
        output: str,
        fail_to_pass: List[str],
        pass_to_pass: Optional[List[str]] = None,
    ) -> EvalResult:
        """Offline fallback: grade a captured pytest log against F2P/P2P."""
        pass_to_pass = pass_to_pass or []
        report = parse_pytest_report(output)
        per_test = []
        for nid in list(fail_to_pass) + list(pass_to_pass):
            per_test.append({"node_id": nid, "passed": report.get(nid, False)})
        resolved = all(t["passed"] for t in per_test) if per_test else False
        passed = sum(1 for t in per_test if t["passed"])
        return EvalResult(
            instance_id=instance_id,
            resolved=resolved,
            reward=1.0 if resolved else 0.0,
            tests_passed=passed,
            tests_total=len(per_test),
            per_test=per_test,
        )
