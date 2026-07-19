from __future__ import annotations

import json
from pathlib import Path
import tempfile
import unittest

import run_stress_suite
import stress_test_common


class StressSuiteTests(unittest.TestCase):
    def test_acceptance_config_requires_real_llm_and_1024_tier(self):
        config = run_stress_suite.load_suite_config(
            Path(__file__).with_name("stress_suite.json")
        )
        self.assertEqual(config["gate3"]["model_mode"], "real")
        self.assertEqual(config["gate4"]["mode"], "llm")
        self.assertEqual(config["worker_scale"]["tiers"], [32, 512, 1024])

    def test_real_dscodebench_row_maps_to_worker_contract(self):
        row = {
            "problem_id": "numpy_0",
            "library": "numpy",
            "code_problem": "Implement solve(values).",
            "ground_truth_code": "def solve(values):\n    return values",
            "test_script": "def generate_test_cases(num_tests):\n    return [([1],)]",
        }
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "dataset.jsonl"
            path.write_text(json.dumps(row) + "\n", encoding="utf-8")
            loaded = stress_test_common.load_dscodebench_jsonl(str(path), limit=1)
        payload = stress_test_common.dscodebench_env_payload(
            loaded[0],
            task_id="gate3-real-1",
            min_steps_before_terminate=3,
        )
        self.assertEqual(payload["dataset"], "dscodebench")
        self.assertEqual(payload["library"], "numpy")
        self.assertEqual(payload["min_steps_before_terminate"], 3)
        self.assertIn("Dataset Problem ID: numpy_0", payload["question"])
        self.assertNotIn(row["ground_truth_code"], payload["question"])
        self.assertIn("dscodebench_harness", payload["test_code"])

    def test_scale_resource_gate_projects_next_tier(self):
        scenario = {
            "result": {
                "fleet_resource_metrics": {
                    "mem_total_bytes": 16 * 1024**3,
                    "min_mem_available_bytes": 8 * 1024**3,
                    "peak_rss_bytes": 256 * 1024**2,
                    "peak_processes": 65,
                    "peak_open_fds": 512,
                    "sample_count": 2,
                }
            }
        }
        decision = run_stress_suite.scale_resource_gate(
            scenario,
            current_workers=32,
            next_workers=512,
            config={
                "minimum_mem_available_bytes": 2 * 1024**3,
                "maximum_projected_host_memory_fraction": 0.85,
            },
        )
        self.assertTrue(decision["passed"])
        self.assertEqual(decision["projected_next_fleet_rss_bytes"], 4 * 1024**3)


if __name__ == "__main__":
    unittest.main()
