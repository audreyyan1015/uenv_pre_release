from __future__ import annotations

import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest

import run_stress_suite
import run_dscodebench_pressure
import run_swebench_pro_pressure
import distributed_stress_runtime
import stress_test_common


class StressSuiteTests(unittest.TestCase):
    def test_embedded_pressure_clients_compile(self):
        compile(run_dscodebench_pressure.LOAD_CLIENT, "<dscodebench-client>", "exec")
        compile(run_dscodebench_pressure.MODEL_SIMULATOR, "<dscodebench-model-simulator>", "exec")
        compile(run_swebench_pro_pressure.SWE_CLIENT, "<swebench-pro-client>", "exec")

    def test_dscodebench_model_stats_prefix_matches_task_ids(self):
        source = Path(__file__).with_name("run_dscodebench_pressure.py").read_text(encoding="utf-8-sig")
        self.assertIn(
            'task_id = f"dscodebench-pressure-{args.run_id}-{args.mode}-{batch_id}-{index}"',
            source,
        )
        self.assertIn(
            'step_stats = _model_step_stats(worker, f"dscodebench-pressure-{run_id}-{mode}-")',
            source,
        )
        self.assertIn("def task_id_matches_prefix(task_id, prefix):", source)

    def test_runtime_defaults_use_current_worker_hosts(self):
        self.assertEqual(distributed_stress_runtime.WORKER_HOST, "8.130.65.20")
        self.assertEqual(distributed_stress_runtime.WORKER_PRIVATE_IP, "192.168.0.139")
        self.assertIn("8.145.51.129", distributed_stress_runtime.EXPECTED_HOST_FINGERPRINTS)
        self.assertNotIn("8.130.86.71", distributed_stress_runtime.EXPECTED_HOST_FINGERPRINTS)

    def test_scale_config_requires_1024_simulator_and_10_waves(self):
        config = run_stress_suite.load_suite_config(
            Path(__file__).with_name("stress_suite.json")
        )
        self.assertEqual(config["dscodebench_pressure"]["model_mode"], "simulator")
        self.assertEqual(config["dscodebench_pressure"]["workers"], 1024)
        self.assertEqual(
            config["dscodebench_pressure"]["episode_batch_size"] * config["dscodebench_pressure"]["exact_batches_per_mode"],
            config["dscodebench_pressure"]["workers"] * config["dscodebench_pressure"]["capacity_per_worker"] * config["dscodebench_pressure"]["min_episode_waves"],
        )
        self.assertEqual(config["dscodebench_pressure"]["simulator_wrong_steps"]["mean"], 2.0)
        self.assertEqual(config["dscodebench_pressure"]["simulator_wrong_steps"]["std"], 1.0)
        self.assertEqual(config["dscodebench_pressure"]["code_python"], "/opt/uenv-stress/venvs/dscodebench/bin/python")
        self.assertEqual(config["swebench_pro_pressure"]["mode"], "llm")
        self.assertEqual(config["swebench_pro_pressure"]["llm_kind"], "simulator")
        self.assertGreaterEqual(config["swebench_pro_pressure"]["instance_count"], 2)
        self.assertFalse(config["worker_scale"]["enabled"])
        self.assertEqual(config["worker_scale"]["tiers"], [1024])
        self.assertEqual(config["worker_scale"]["model_port"], 6379)
        self.assertEqual(config["worker_scale"]["episode_batch_size"], 256)
        self.assertEqual(config["worker_scale"]["episodes_per_worker"], 10)
        self.assertEqual(config["worker_scale"]["simulator_latency_ms"]["mean"], 500.0)
        self.assertEqual(config["worker_scale"]["plugin_ready_timeout_seconds"], 30)
        self.assertEqual(config["worker_scale"]["worker_register_max_attempts"], 20)
        self.assertEqual(config["worker_scale"]["worker_register_retry_backoff_ms"], 100)
        self.assertEqual(config["worker_scale"]["code_python"], "/opt/uenv-stress/venvs/dscodebench/bin/python")

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
            task_id="dscodebench_pressure-real-1",
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
                    "initial_mem_available_bytes": 12 * 1024**3,
                    "min_mem_available_bytes": 12 * 1024**3 - 256 * 1024**2,
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
        self.assertEqual(decision["projected_next_fleet_memory_bytes"], 4 * 1024**3)

    def test_exact_batch_loop_rechecks_after_semaphore_wait(self):
        source = Path(__file__).with_name("run_dscodebench_pressure.py").read_text(encoding="utf-8")
        self.assertIn(
            "if args.exact_batches > 0 and batch_sequence >= args.exact_batches",
            source,
        )

    def test_dscodebench_pressure_scale_command_receives_private_range_and_distribution(self):
        config = run_stress_suite.load_suite_config(
            Path(__file__).with_name("stress_suite.json")
        )
        args = SimpleNamespace(
            source_repo="/repo", server_bin="/server", worker_bin="/worker",
            code_plugin_bin="/plugin", protected_pid=1, protected_port=[8077, 8088],
            server_host="server", worker_host="worker", server_private_ip="10.0.0.1",
            worker_private_ip="10.0.0.2", server_port=8099, worker_port=8000,
            model_port=8888, obs_port=18002, llm_config="/secret/config.json",
            private_worker_port_range="8000-9023",
        )
        command = run_stress_suite.dscodebench_pressure_command(args, config, Path("/artifacts"))
        self.assertIn("--private-worker-port-range", command)
        self.assertIn("--simulator-wrong-steps-mean", command)
        self.assertIn("--min-scale-episode-waves", command)
        self.assertIn("--code-python", command)
        exact_batches_index = command.index("--exact-batches")
        self.assertEqual(command[exact_batches_index + 1], "40")
        scale_command = run_stress_suite.worker_scale_command(
            args, config, 1024, Path("/scale-artifacts")
        )
        model_port_index = scale_command.index("--model-port")
        self.assertEqual(scale_command[model_port_index + 1], "6379")
        batch_size_index = scale_command.index("--episode-batch-size")
        self.assertEqual(scale_command[batch_size_index + 1], "256")
        exact_batches_index = scale_command.index("--exact-batches")
        self.assertEqual(scale_command[exact_batches_index + 1], "40")
        concurrent_batches_index = scale_command.index("--concurrent-batches")
        self.assertEqual(scale_command[concurrent_batches_index + 1], "4")
        self.assertIn("--code-python", scale_command)

    def test_newest_summary_finds_child_output_under_absolute_root(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            target = root / "nested" / "dscodebench-pressure-summary-test.json"
            target.parent.mkdir()
            target.write_text("{}", encoding="utf-8")
            self.assertEqual(
                run_stress_suite.newest_summary(root, "dscodebench-pressure-summary-*.json"),
                target,
            )

    def test_validate_arguments_rejects_model_port_inside_worker_range(self):
        config = run_stress_suite.load_suite_config(
            Path(__file__).with_name("stress_suite.json")
        )
        args = SimpleNamespace(
            source_repo="/repo", server_bin="/server", worker_bin="/worker",
            code_plugin_bin="/plugin", protected_pid=1, protected_port=[8077, 8088],
            server_host="server", worker_host="worker", server_private_ip="10.0.0.1",
            worker_private_ip="10.0.0.2", server_port=8099, worker_port=8000,
            model_port=8888, gateway_port=8777, agent_api_port=18004,
            agent_health_port=18005, obs_port=18002, llm_config="",
            private_worker_port_range="8000-9023",
        )
        with self.assertRaisesRegex(ValueError, "model port 8888 overlaps"):
            run_stress_suite.validate_arguments(args, config)


if __name__ == "__main__":
    unittest.main()
