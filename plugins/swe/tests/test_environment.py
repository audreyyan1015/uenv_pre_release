"""Offline unit tests for the SWE plugin (no Worker / network required)."""

import os
import sys
import unittest

_UENV_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))
sys.path.insert(0, _UENV_ROOT)

from plugins.swe.command_policy import CommandMode, CommandPolicy  # noqa: E402
from plugins.swe.evaluator.swe_evaluator import SweEvaluator, parse_pytest_report  # noqa: E402


class CommandPolicyTests(unittest.TestCase):
    def test_parse_aliases(self):
        self.assertEqual(CommandMode.parse("FullShell"), CommandMode.FULL_SHELL)
        self.assertEqual(CommandMode.parse("restricted_shell"), CommandMode.RESTRICTED_SHELL)
        self.assertIsNone(CommandMode.parse("nope"))

    def test_wrap_is_bash_lc(self):
        self.assertEqual(CommandPolicy().wrap_command("pytest -q"), ["bash", "-lc", "pytest -q"])

    def test_deny_patterns_mvp_substring(self):
        p = CommandPolicy(deny_patterns=["curl", "wget"])
        self.assertEqual(p.first_denied("curl http://x"), "curl")
        self.assertIsNone(p.first_denied("pytest -q"))

    def test_truncate(self):
        p = CommandPolicy(max_output_bytes=4)
        out, truncated = p.truncate_output("123456")
        self.assertEqual(out, "1234")
        self.assertTrue(truncated)


class ParserTests(unittest.TestCase):
    def test_verbose_and_summary(self):
        log = "a.py::test_x PASSED [ 50%]\nFAILED a.py::test_y\n"
        rep = parse_pytest_report(log)
        self.assertTrue(rep["a.py::test_x"])
        self.assertFalse(rep["a.py::test_y"])

    def test_grade_log_resolution(self):
        log = "a.py::f1 PASSED\na.py::p1 PASSED\n"
        ev = SweEvaluator().grade_log("inst", log, ["a.py::f1"], ["a.py::p1"])
        self.assertTrue(ev.resolved)
        self.assertEqual(ev.reward, 1.0)
        ev2 = SweEvaluator().grade_log("inst", "a.py::f1 FAILED", ["a.py::f1"], [])
        self.assertFalse(ev2.resolved)


if __name__ == "__main__":
    unittest.main()
