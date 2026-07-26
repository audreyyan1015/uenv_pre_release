from __future__ import annotations

import json
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest

from uenv_stress.cli import run_scale_suite
from uenv_stress.core import distributed_runtime
from uenv_stress.core import stress_test_common
from uenv_stress.scale import dscodebench_pressure
from uenv_stress.scale import rule_task_pressure
from uenv_stress.scale import swebench_pro_pressure
from uenv_stress.workloads import rule_tasks


ROOT = Path(__file__).resolve().parents[1]
CONFIG_DIR = ROOT / "uenv_stress" / "config"
SCALE_DIR = ROOT / "uenv_stress" / "scale"


class StressSuiteTests(unittest.TestCase):
    def test_embedded_pressure_clients_compile(self):
        compile(dscodebench_pressure.LOAD_CLIENT, "<dscodebench-client>", "exec")
        compile(dscodebench_pressure.MODEL_SIMULATOR, "<dscodebench-model-simulator>", "exec")
        compile(swebench_pro_pressure.SWE_CLIENT, "<swebench-pro-client>", "exec")
        compile(rule_task_pressure.LOAD_CLIENT, "<math-rule-client>", "exec")
        compile(rule_task_pressure.TRACE_REPLAY_SERVER, "<math-rule-replay>", "exec")

    def test_dscodebench_model_stats_prefix_matches_task_ids(self):
        source = (SCALE_DIR / "dscodebench_pressure.py").read_text(
            encoding="utf-8-sig"
        )
        self.assertIn(
            'task_id = f"dscodebench-pressure-{args.run_id}-{args.mode}-{batch_id}-{index}"',
            source,
        )
        self.assertIn(
            'step_stats = _model_step_stats(worker_clients, f"dscodebench-pressure-{run_id}-{mode}-")',
            source,
        )
        self.assertIn("def task_id_matches_prefix(task_id, prefix):", source)

    def test_runtime_defaults_use_current_worker_hosts(self):
        self.assertEqual(distributed_runtime.WORKER_HOST, "8.130.65.20")
        self.assertEqual(distributed_runtime.WORKER_PRIVATE_IP, "192.168.0.139")
        self.assertIn("8.145.51.129", distributed_runtime.EXPECTED_HOST_FINGERPRINTS)
        self.assertNotIn("8.130.86.71", distributed_runtime.EXPECTED_HOST_FINGERPRINTS)

    def test_scale_config_requires_1024_simulator_and_10_waves(self):
        config = run_scale_suite.load_suite_config(
            CONFIG_DIR / "scale_suite.json"
        )
        self.assertEqual(config["dscodebench_pressure"]["model_mode"], "simulator")
        self.assertEqual(config["dscodebench_pressure"]["workers"], 1024)
        self.assertEqual(
            config["dscodebench_pressure"]["episode_batch_size"] * config["dscodebench_pressure"]["exact_batches_per_mode"],
            config["dscodebench_pressure"]["workers"] * config["dscodebench_pressure"]["capacity_per_worker"] * config["dscodebench_pressure"]["min_episode_waves"],
        )
        self.assertEqual(config["dscodebench_pressure"]["simulator_wrong_steps"]["mean"], 2.0)
        self.assertEqual(config["dscodebench_pressure"]["simulator_wrong_steps"]["std"], 1.0)
        self.assertEqual(
            config["dscodebench_pressure"]["trace_sampling_strategy"],
            "round_robin_episode",
        )
        self.assertEqual(config["dscodebench_pressure"]["code_python"], "/opt/uenv-stress/venvs/dscodebench/bin/python")
        self.assertEqual(config["swebench_pro_pressure"]["mode"], "llm")
        self.assertEqual(config["swebench_pro_pressure"]["llm_kind"], "simulator")
        self.assertEqual(
            config["swebench_pro_pressure"]["trace_sampling_strategy"],
            "round_robin_episode",
        )
        self.assertGreaterEqual(config["swebench_pro_pressure"]["instance_count"], 2)
        self.assertEqual(config["trace_collection"]["swebench_pro"]["source_model"], "doubao")
        self.assertEqual(config["trace_collection"]["swebench_pro"]["instance_count"], 50)
        self.assertFalse(config["trace_collection"]["swebench_pro"]["uses_1024_workers"])
        self.assertFalse(config["worker_scale"]["enabled"])
        math_gate = config["math_rule_pressure"]
        self.assertEqual(set(math_gate["tasks"]), {"olymmath", "scitab", "pubmedqa"})
        self.assertEqual(set(math_gate["modes"]), run_scale_suite.PARALLEL_MODES)
        self.assertEqual(math_gate["workers"], 1024)
        self.assertEqual(
            math_gate["trace_sampling_strategy"], "round_robin_episode"
        )
        self.assertGreaterEqual(
            math_gate["episode_batch_size"] * math_gate["exact_batches_per_dataset_mode"],
            math_gate["workers"] * math_gate["capacity_per_worker"] * math_gate["min_episode_waves"],
        )
        self.assertEqual(config["worker_scale"]["tiers"], [1024])
        self.assertEqual(config["worker_scale"]["model_port"], 6379)
        self.assertEqual(config["worker_scale"]["episode_batch_size"], 256)
        self.assertEqual(config["worker_scale"]["episodes_per_worker"], 10)
        self.assertEqual(config["worker_scale"]["simulator_latency_ms"]["mean"], 500.0)
        self.assertEqual(config["worker_scale"]["plugin_ready_timeout_seconds"], 30)
        self.assertEqual(config["worker_scale"]["worker_register_max_attempts"], 20)
        self.assertEqual(config["worker_scale"]["worker_register_retry_backoff_ms"], 100)
        self.assertEqual(config["worker_scale"]["code_python"], "/opt/uenv-stress/venvs/dscodebench/bin/python")
        self.assertEqual(
            config["worker_scale"]["trace_sampling_strategy"],
            "round_robin_episode",
        )

    def test_all_scale_replayers_bind_whole_episodes_and_rotate_corpora(self):
        dscode = dscodebench_pressure.MODEL_SIMULATOR
        swe = swebench_pro_pressure.LLM_SIMULATOR
        math = rule_task_pressure.TRACE_REPLAY_SERVER
        self.assertIn("trace_assignment_by_task", dscode)
        self.assertIn("trace_cursor % len(trace_episodes)", dscode)
        self.assertIn("turns[turn_slot]", dscode)
        self.assertIn("trace_assignment_by_task", swe)
        self.assertIn("trace_cursor % len(REPLAY_EPISODES)", swe)
        self.assertIn("turns[turn_slot]", swe)
        self.assertIn("trace_cursors[task]", math)
        self.assertIn("selection_ordinal % len(records[task])", math)
        for source in (dscode, swe, math):
            self.assertIn("round_robin_episode", source)
            self.assertIn("replay_wait_ms", source)
            self.assertIn("latency_source", source)
            self.assertNotIn("problem_then_turn", source)
            self.assertNotIn("instance_then_turn", source)
            self.assertNotIn('"turn_only"', source)

    def test_embedded_multiturn_replayers_rotate_episodes_not_turns(self):
        def load_without_server(source, argv):
            executable = source.rsplit(
                'ThreadingHTTPServer(("127.0.0.1", args.port), Handler).serve_forever()',
                1,
            )[0]
            namespace = {"__name__": "embedded_replay_test"}
            previous = sys.argv
            try:
                sys.argv = argv
                exec(compile(executable, "<embedded-replay>", "exec"), namespace)
            finally:
                sys.argv = previous
            return namespace

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            dscode_path = root / "dscode.jsonl"
            dscode_path.write_text(
                "\n".join([
                    json.dumps({
                        "trace_id": "ds-a",
                        "turns": [
                            {
                                "turn_index": 1, "assistant_output": "a1",
                                "replay_wait_ms": 1000,
                                "latency_source": "observed_episode_elapsed_proxy",
                            },
                            {
                                "turn_index": 2, "assistant_output": "a2",
                                "replay_wait_ms": 2000,
                                "latency_source": "observed_episode_elapsed_proxy",
                            },
                        ],
                    }),
                    json.dumps({
                        "trace_id": "ds-b",
                        "turns": [
                            {
                                "turn_index": 1, "assistant_output": "b1",
                                "replay_wait_ms": 3000,
                                "latency_source": "observed_episode_elapsed_proxy",
                            },
                            {
                                "turn_index": 2, "assistant_output": "b2",
                                "replay_wait_ms": 4000,
                                "latency_source": "observed_episode_elapsed_proxy",
                            },
                        ],
                    }),
                ]) + "\n",
                encoding="utf-8",
            )
            dscode = load_without_server(
                dscodebench_pressure.MODEL_SIMULATOR,
                [
                    "model_simulator.py", "--port", "0",
                    "--simulator-mode", "trace_replay",
                    "--trace-corpus-path", str(dscode_path),
                    "--trace-sampling-strategy", "round_robin_episode",
                ],
            )
            first_episode, first_turn = dscode["replay_record"]("ep-a", 1)
            second_episode, second_turn = dscode["replay_record"]("ep-b", 1)
            same_episode, same_turn = dscode["replay_record"]("ep-a", 2)
            wrapped_episode, _ = dscode["replay_record"]("ep-c", 1)
            self.assertEqual(
                [
                    first_episode["trace_id"],
                    second_episode["trace_id"],
                    same_episode["trace_id"],
                    wrapped_episode["trace_id"],
                ],
                ["ds-a", "ds-b", "ds-a", "ds-a"],
            )
            self.assertEqual(
                [first_turn["assistant_output"], same_turn["assistant_output"]],
                ["a1", "a2"],
            )

            swe_path = root / "swe.jsonl"
            swe_path.write_text(
                "\n".join([
                    json.dumps({
                        "trace_id": "swe-a",
                        "turns": [
                            {
                                "turn_index": 1, "assistant_output": "a1",
                                "replay_wait_ms": 1000,
                                "latency_source": "observed_episode_elapsed_proxy",
                            },
                            {
                                "turn_index": 2, "assistant_output": "a2",
                                "replay_wait_ms": 2000,
                                "latency_source": "observed_episode_elapsed_proxy",
                            },
                        ],
                    }),
                    json.dumps({
                        "trace_id": "swe-b",
                        "turns": [
                            {
                                "turn_index": 1, "assistant_output": "b1",
                                "replay_wait_ms": 3000,
                                "latency_source": "observed_episode_elapsed_proxy",
                            },
                            {
                                "turn_index": 2, "assistant_output": "b2",
                                "replay_wait_ms": 4000,
                                "latency_source": "observed_episode_elapsed_proxy",
                            },
                        ],
                    }),
                ]) + "\n",
                encoding="utf-8",
            )
            swe = load_without_server(
                swebench_pro_pressure.LLM_SIMULATOR,
                [
                    "llm_simulator.py", "--port", "0",
                    "--simulator-mode", "trace_replay",
                    "--trace-corpus-path", str(swe_path),
                    "--trace-sampling-strategy", "round_robin_episode",
                ],
            )
            first_key, _ = swe["request_fingerprint"]({}, "ep-a")
            second_key, _ = swe["request_fingerprint"]({}, "ep-b")
            self.assertEqual((first_key, second_key), ("episode:ep-a", "episode:ep-b"))
            first_episode, first_turn = swe["choose_trace_turn"]("ep-a", 1)
            second_episode, _ = swe["choose_trace_turn"]("ep-b", 1)
            same_episode, same_turn = swe["choose_trace_turn"]("ep-a", 2)
            wrapped_episode, _ = swe["choose_trace_turn"]("ep-c", 1)
            self.assertEqual(
                [
                    first_episode["trace_id"],
                    second_episode["trace_id"],
                    same_episode["trace_id"],
                    wrapped_episode["trace_id"],
                ],
                ["swe-a", "swe-b", "swe-a", "swe-a"],
            )
            self.assertEqual(
                [first_turn["assistant_output"], same_turn["assistant_output"]],
                ["a1", "a2"],
            )

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
        decision = run_scale_suite.scale_resource_gate(
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
        source = (SCALE_DIR / "dscodebench_pressure.py").read_text(
            encoding="utf-8"
        )
        self.assertIn(
            "if args.exact_batches > 0 and batch_sequence >= args.exact_batches",
            source,
        )

    def test_dscodebench_pressure_scale_command_receives_private_range_and_distribution(self):
        config = run_scale_suite.load_suite_config(
            CONFIG_DIR / "scale_suite.json"
        )
        args = SimpleNamespace(
            source_repo="/repo", server_bin="/server", worker_bin="/worker",
            code_plugin_bin="/plugin", math_plugin_bin="/math-plugin",
            protected_pid=1, protected_port=[8077, 8088],
            server_host="server", worker_host="worker", server_private_ip="10.0.0.1",
            worker_private_ip="10.0.0.2", server_port=8099, worker_port=8000,
            model_port=8888, obs_port=18002, llm_config="/secret/config.json",
            private_worker_port_range="8000-9023",
        )
        command = run_scale_suite.dscodebench_pressure_command(args, config, Path("/artifacts"))
        self.assertIn("--private-worker-port-range", command)
        self.assertIn("--simulator-wrong-steps-mean", command)
        self.assertIn("--min-scale-episode-waves", command)
        self.assertIn("--code-python", command)
        exact_batches_index = command.index("--exact-batches")
        self.assertEqual(command[exact_batches_index + 1], "40")
        scale_command = run_scale_suite.worker_scale_command(
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

    def test_swebench_pro_trace_collection_command_uses_doubao_real_llm(self):
        config = run_scale_suite.load_suite_config(
            CONFIG_DIR / "scale_suite.json"
        )
        args = SimpleNamespace(
            source_repo="/repo", server_bin="/server", worker_bin="/worker",
            code_plugin_bin="/plugin", math_plugin_bin="/math-plugin",
            protected_pid=1, protected_port=[8077, 8088],
            server_host="server", worker_host="worker", server_private_ip="10.0.0.1",
            worker_private_ip="10.0.0.2", server_port=8099, worker_port=8000,
            model_port=8888, obs_port=18002, gateway_port=8777,
            agent_api_port=8077, agent_health_port=8088,
            llm_config="/opt/uenv-stress/config/openhands-llm-doubao.json",
            private_worker_port_range="", private_gateway_port_range="",
        )
        command = run_scale_suite.swebench_pro_trace_collection_command(
            args, config, Path("/artifacts")
        )
        self.assertIn("--llm-kind", command)
        self.assertEqual(command[command.index("--llm-kind") + 1], "real")
        self.assertEqual(command[command.index("--trace-source-model") + 1], "doubao")
        self.assertEqual(command[command.index("--registered-workers") + 1], "1")
        self.assertEqual(command[command.index("--total-episodes") + 1], "50")
        self.assertEqual(command[command.index("--freeze-trace-corpus-path") + 1], config["trace_collection"]["swebench_pro"]["trace_corpus_path"])
        self.assertIn("--freeze-require-complete", command)

    def test_swebench_pro_pressure_command_uses_pressure_ports(self):
        config = run_scale_suite.load_suite_config(
            CONFIG_DIR / "scale_suite.json"
        )
        args = SimpleNamespace(
            source_repo="/repo", server_bin="/server", worker_bin="/worker",
            code_plugin_bin="/plugin", math_plugin_bin="/math-plugin",
            protected_pid=1, protected_port=[8077, 8088],
            server_host="server", worker_host="worker", server_private_ip="10.0.0.1",
            worker_private_ip="10.0.0.2", worker_node=[],
            server_port=8099, worker_port=8000, model_port=8888, obs_port=18002,
            gateway_port=8777, agent_api_port=18004, agent_health_port=18005,
            llm_config="", private_worker_port_range="8000-9023",
            private_gateway_port_range="22000-23023",
        )
        command = run_scale_suite.swebench_pro_pressure_command(
            args, config, Path("/artifacts")
        )
        self.assertEqual(
            command[:3],
            [run_scale_suite.sys.executable, "-m", "uenv_stress.scale.swebench_pro_pressure"],
        )
        self.assertEqual(
            command[command.index("--gateway-port") + 1],
            str(config["swebench_pro_pressure"]["gateway_port"]),
        )

    def test_math_rule_pressure_command_covers_three_datasets_and_real_math_plugin(self):
        config = run_scale_suite.load_suite_config(CONFIG_DIR / "scale_suite.json")
        args = SimpleNamespace(
            source_repo="/repo", server_bin="/server", worker_bin="/worker",
            code_plugin_bin="/code-plugin", math_plugin_bin="/math-plugin",
            protected_pid=1, protected_port=[8077, 8088],
            server_host="server", worker_host="worker",
            server_private_ip="10.0.0.1", worker_private_ip="10.0.0.2",
            worker_node=[
                "8.130.65.20:192.168.0.139",
                "8.145.51.129:192.168.0.138",
            ],
            server_port=8099, worker_port=8000, model_port=8888,
            obs_port=18002, private_worker_port_range="8000-9023",
        )
        command = run_scale_suite.math_rule_pressure_command(
            args, config, Path("/artifacts")
        )
        self.assertEqual(
            command[:3],
            [run_scale_suite.sys.executable, "-m", "uenv_stress.scale.rule_task_pressure"],
        )
        self.assertEqual(command[command.index("--code-plugin-bin") + 1], "/math-plugin")
        for task in rule_tasks.TASK_NAMES:
            self.assertIn(f"--{task}-dataset", command)
            self.assertIn(f"--{task}-trace", command)
        self.assertEqual(command.count("--mode"), 3)

    def test_suite_dataset_coverage_requires_all_five(self):
        scenarios = [
            {"name": "dscodebench-pressure-1024-simulator", "status": "passed", "result": {}},
            {"name": "swebench-pro-pressure-openhands", "status": "passed", "result": {}},
            {
                "name": "math-rule-pressure-1024workers-three-datasets",
                "status": "passed",
                "result": [{"datasets": ["olymmath", "scitab", "pubmedqa"]}],
            },
        ]
        self.assertEqual(
            run_scale_suite.scenario_dataset_coverage(scenarios),
            run_scale_suite.SCALE_DATASETS,
        )

    def test_rule_task_adapter_scitab_scale_marker_is_opt_in(self):
        row = {
            "id": "s1",
            "claim": "A is larger",
            "table_content_values": [["A", "1"]],
            "label": "refutes",
        }
        formal_env, reward = rule_tasks.build_env_payload(
            "scitab", row, index=0, task_id="formal"
        )
        scale_env, _ = rule_tasks.build_env_payload(
            "scitab", row, index=0, task_id="scale", scale_marker=True
        )
        self.assertNotIn("[UENV_SCALE", formal_env["question"])
        self.assertIn("[UENV_SCALE dataset=scitab item_id=s1]", scale_env["question"])
        self.assertEqual(reward, {"type": "rule_reward", "target": "refutes"})

    def test_trace_collection_effective_llm_kind_is_real(self):
        config = run_scale_suite.load_suite_config(
            CONFIG_DIR / "scale_suite.json"
        )
        args = SimpleNamespace(scenario="swebench-pro-trace-collection")
        self.assertEqual(run_scale_suite.effective_llm_kind(args, config), "real")

    def test_newest_summary_finds_child_output_under_absolute_root(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            target = root / "nested" / "dscodebench-pressure-summary-test.json"
            target.parent.mkdir()
            target.write_text("{}", encoding="utf-8")
            self.assertEqual(
                run_scale_suite.newest_summary(root, "dscodebench-pressure-summary-*.json"),
                target,
            )

    def test_validate_arguments_rejects_model_port_inside_worker_range(self):
        config = run_scale_suite.load_suite_config(
            CONFIG_DIR / "scale_suite.json"
        )
        args = SimpleNamespace(
            source_repo="/repo", server_bin="/server", worker_bin="/worker",
            code_plugin_bin="/plugin", math_plugin_bin="/math-plugin",
            protected_pid=1, protected_port=[8077, 8088],
            server_host="server", worker_host="worker", server_private_ip="10.0.0.1",
            worker_private_ip="10.0.0.2", server_port=8099, worker_port=8000,
            model_port=8888, gateway_port=8777, agent_api_port=18004,
            agent_health_port=18005, obs_port=18002, llm_config="",
            private_worker_port_range="8000-9023",
        )
        with self.assertRaisesRegex(ValueError, "model port 8888 overlaps"):
            run_scale_suite.validate_arguments(args, config)

    def test_parse_worker_nodes_validates_format_and_registry(self):
        base = distributed_runtime
        nodes = base.parse_worker_nodes(["8.130.65.20:192.168.0.139", " 8.145.51.129:192.168.0.138 "])
        self.assertEqual(
            nodes,
            [base.WorkerNode("8.130.65.20", "192.168.0.139"), base.WorkerNode("8.145.51.129", "192.168.0.138")],
        )
        self.assertEqual(base.parse_worker_nodes(None), [])
        self.assertEqual(base.parse_worker_nodes([]), [])
        with self.assertRaisesRegex(ValueError, "HOST:PRIVATE_IP"):
            base.parse_worker_nodes(["8.130.65.20"])
        with self.assertRaisesRegex(ValueError, "HOST:PRIVATE_IP"):
            base.parse_worker_nodes(["8.130.65.20:"])
        with self.assertRaisesRegex(ValueError, "no registered SSH fingerprint"):
            base.parse_worker_nodes(["203.0.113.10:192.168.0.1"])
        with self.assertRaisesRegex(ValueError, "duplicated"):
            base.parse_worker_nodes(["8.130.65.20:192.168.0.139", "8.130.65.20:192.168.0.140"])

    def _runtime_args(self, **overrides):
        values = {
            "protected_pid": 100,
            "protected_port": None,
            "source_repo": "/repo",
            "server_bin": "/server",
            "worker_bin": "/worker",
            "code_plugin_bin": "",
            "server_host": "8.130.75.157",
            "server_private_ip": "192.168.0.136",
            "worker_host": "8.130.65.20",
            "worker_private_ip": "192.168.0.139",
            "worker_node": [],
            "server_port": 8099,
            "worker_port": 8000,
            "model_port": 8888,
            "obs_port": 18002,
        }
        values.update(overrides)
        return SimpleNamespace(**values)

    def test_configure_from_args_single_host_fallback_keeps_compat_layer(self):
        base = distributed_runtime
        original = (
            base.WORKER_HOST, base.WORKER_PRIVATE_IP, list(base.WORKER_NODES),
            base.SERVER_PORT, base.WORKER_PORT, base.MODEL_PORT, base.OBS_PORT,
            base.PROTECTED_PID, base.PROTECTED_PORTS, base.SOURCE_REPO,
            base.SERVER_BIN, base.SOURCE_WORKER_BIN, base.SOURCE_CODE_BIN,
        )
        try:
            base.configure_from_args(self._runtime_args(worker_host="8.145.51.129", worker_private_ip="192.168.0.138"))
            self.assertEqual(base.WORKER_NODES, [base.WorkerNode("8.145.51.129", "192.168.0.138")])
            self.assertEqual(base.WORKER_HOST, "8.145.51.129")
            self.assertEqual(base.WORKER_PRIVATE_IP, "192.168.0.138")
        finally:
            (
                base.WORKER_HOST, base.WORKER_PRIVATE_IP, base.WORKER_NODES,
                base.SERVER_PORT, base.WORKER_PORT, base.MODEL_PORT, base.OBS_PORT,
                base.PROTECTED_PID, base.PROTECTED_PORTS, base.SOURCE_REPO,
                base.SERVER_BIN, base.SOURCE_WORKER_BIN, base.SOURCE_CODE_BIN,
            ) = original

    def test_configure_from_args_worker_node_overrides_single_host(self):
        base = distributed_runtime
        original = (
            base.WORKER_HOST, base.WORKER_PRIVATE_IP, list(base.WORKER_NODES),
            base.SERVER_PORT, base.WORKER_PORT, base.MODEL_PORT, base.OBS_PORT,
            base.PROTECTED_PID, base.PROTECTED_PORTS, base.SOURCE_REPO,
            base.SERVER_BIN, base.SOURCE_WORKER_BIN, base.SOURCE_CODE_BIN,
        )
        try:
            base.configure_from_args(self._runtime_args(worker_node=[
                "8.130.65.20:192.168.0.139",
                "8.145.51.129:192.168.0.138",
            ]))
            self.assertEqual(
                base.WORKER_NODES,
                [base.WorkerNode("8.130.65.20", "192.168.0.139"), base.WorkerNode("8.145.51.129", "192.168.0.138")],
            )
            # 兼容层指向第一个节点，只读旧代码行为不变。
            self.assertEqual(base.WORKER_HOST, "8.130.65.20")
            self.assertEqual(base.WORKER_PRIVATE_IP, "192.168.0.139")
        finally:
            (
                base.WORKER_HOST, base.WORKER_PRIVATE_IP, base.WORKER_NODES,
                base.SERVER_PORT, base.WORKER_PORT, base.MODEL_PORT, base.OBS_PORT,
                base.PROTECTED_PID, base.PROTECTED_PORTS, base.SOURCE_REPO,
                base.SERVER_BIN, base.SOURCE_WORKER_BIN, base.SOURCE_CODE_BIN,
            ) = original

    def test_worker_assignments_round_robin(self):
        base = distributed_runtime
        original = list(base.WORKER_NODES)
        try:
            base.WORKER_NODES = [
                base.WorkerNode("8.130.65.20", "192.168.0.139"),
                base.WorkerNode("8.145.51.129", "192.168.0.138"),
            ]
            self.assertEqual(base.worker_assignments(5), [[0, 2, 4], [1, 3]])
            self.assertEqual(base.worker_assignments(2), [[0], [1]])
            self.assertEqual(base.worker_assignments(1), [[0], []])
            base.WORKER_NODES = [base.WorkerNode("8.130.65.20", "192.168.0.139")]
            self.assertEqual(base.worker_assignments(3), [[0, 1, 2]])
            with self.assertRaisesRegex(ValueError, "positive"):
                base.worker_assignments(0)
        finally:
            base.WORKER_NODES = original

    def test_common_child_args_passthrough_worker_nodes(self):
        args = SimpleNamespace(
            source_repo="/repo", server_bin="/server", worker_bin="/worker",
            code_plugin_bin="/plugin", math_plugin_bin="/math-plugin",
            protected_pid=100, protected_port=[8077],
            server_host="server", worker_host="worker", server_private_ip="10.0.0.1",
            worker_private_ip="10.0.0.2", server_port=8099, worker_port=8000,
            model_port=8888, obs_port=18002,
            worker_node=["8.130.65.20:192.168.0.139", "8.145.51.129:192.168.0.138"],
        )
        command = run_scale_suite.common_child_args(args)
        node_indexes = [index for index, item in enumerate(command) if item == "--worker-node"]
        self.assertEqual(len(node_indexes), 2)
        self.assertEqual(command[node_indexes[0] + 1], "8.130.65.20:192.168.0.139")
        self.assertEqual(command[node_indexes[1] + 1], "8.145.51.129:192.168.0.138")
        # 未传 --worker-node 时命令行与改造前一致。
        args.worker_node = []
        self.assertNotIn("--worker-node", run_scale_suite.common_child_args(args))


if __name__ == "__main__":
    unittest.main()
