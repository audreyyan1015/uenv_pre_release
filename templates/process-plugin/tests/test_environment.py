#!/usr/bin/env python3
"""Fast, dependency-free tests for the editable environment logic."""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

PLUGIN_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(PLUGIN_DIR))

from environment import Environment, SELF_TEST_CASE  # noqa: E402


class EnvironmentLogicTest(unittest.TestCase):
    def test_reset_step_and_reward(self) -> None:
        environment = Environment()
        config = dict(SELF_TEST_CASE["config"])
        config["_uenv"] = {"payload": {"metadata": {"case": "self-test"}}}
        reset = environment.reset(config, seed=SELF_TEST_CASE["seed"])
        self.assertEqual(reset.observation, SELF_TEST_CASE["reset_observation"])

        result = environment.step(SELF_TEST_CASE["action"].encode("utf-8"))
        self.assertEqual(result.observation, SELF_TEST_CASE["step_observation"])
        self.assertEqual(result.reward, SELF_TEST_CASE["reward"])
        self.assertEqual(result.terminated, SELF_TEST_CASE["terminated"])
        self.assertEqual(result.truncated, SELF_TEST_CASE["truncated"])


if __name__ == "__main__":
    unittest.main()
