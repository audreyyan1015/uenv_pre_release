from __future__ import annotations

import json
import importlib.util
import sqlite3
import tempfile
import unittest
from pathlib import Path

import stability_test_common as common
import generate_stability_report as report
import trace_replay_server as replay
import prepare_stability_datasets as datasets
import run_stability_suite as runner


HERE = Path(__file__).resolve().parent


class StabilityConfigTests(unittest.TestCase):
    def test_formal_config_rates_and_capacity(self):
        config = common.load_config(HERE / "stability_suite.json")
        self.assertEqual(set(config["tasks"]), set(common.TASK_NAMES))
        self.assertAlmostEqual(sum(task["allocation_share"] for task in config["tasks"].values()), 1.0)
        self.assertAlmostEqual(sum(task["target_rate_eps"] for task in config["tasks"].values()), 302.64)
        capacity = common.required_capacity(config, "reference")
        self.assertEqual(capacity["by_task"], {
            "dscodebench": 99,
            "swebench_pro": 103,
            "olymmath": 2459,
            "scitab": 2528,
            "pubmedqa": 2528,
        })
        self.assertEqual(capacity["total_slots"], 7717)

    def test_phase_rates(self):
        config = common.load_config(HERE / "stability_suite.json")
        task = config["tasks"]["dscodebench"]
        self.assertEqual(common.phase_rate(task, "selfcheck"), 12.0)
        self.assertEqual(common.phase_rate(task, "reference"), 2.4)
        self.assertEqual(common.phase_rate(task, "capacity"), 2.88)
        self.assertEqual(common.phase_rate(task, "burst"), 9.6)

    def test_reference_and_stability_share_constant_then_batch_segments(self):
        config = common.load_config(HERE / "stability_suite.json")
        reference = runner.arrival_segments(config, "reference", 100)
        stability = runner.arrival_segments(config, "stability", 100)
        self.assertEqual(reference, stability)
        self.assertEqual([item["mode"] for item in reference], ["constant", "batch"])
        self.assertAlmostEqual(sum(item["duration_seconds"] for item in reference), 100)


class SchedulerTests(unittest.TestCase):
    def test_constant_offsets(self):
        self.assertEqual(common.scheduled_offsets(2.0, 4, mode="constant"), [0.0, 0.5, 1.0, 1.5])

    def test_batch_offsets_keep_long_term_rate(self):
        self.assertEqual(
            common.scheduled_offsets(4.0, 8, mode="batch", batch_size=4),
            [0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0],
        )

    def test_poisson_is_reproducible_and_monotonic(self):
        first = common.scheduled_offsets(3.0, 20, mode="poisson", seed=7)
        second = common.scheduled_offsets(3.0, 20, mode="poisson", seed=7)
        self.assertEqual(first, second)
        self.assertEqual(first, sorted(first))


class LatencyTests(unittest.TestCase):
    PROFILE = {"mean_seconds": 129.9, "std_seconds": 125.3, "base_tokens": 10406}

    def test_episode_seed_is_stable(self):
        one = common.deterministic_lognormal_base_seconds(self.PROFILE, run_seed=20260722, episode_id="ep-1")
        two = common.deterministic_lognormal_base_seconds(self.PROFILE, run_seed=20260722, episode_id="ep-1")
        other = common.deterministic_lognormal_base_seconds(self.PROFILE, run_seed=20260722, episode_id="ep-2")
        self.assertEqual(one, two)
        self.assertNotEqual(one, other)

    def test_turns_share_speed_and_scale_by_tokens(self):
        one = common.target_llm_delay_seconds(self.PROFILE, run_seed=1, episode_id="ep", generated_tokens=1000)
        two = common.target_llm_delay_seconds(self.PROFILE, run_seed=1, episode_id="ep", generated_tokens=2000)
        self.assertAlmostEqual(two, one * 2)


class LedgerTests(unittest.TestCase):
    def record(self, ledger: common.EpisodeLedger, request_id: str, timeout: float = 10) -> None:
        ledger.plan(common.EpisodeRecord(request_id, "dscodebench", "batch", 100, timeout))
        ledger.mark_dispatched(request_id, 100)

    def test_all_failure_classes_and_stream_loss_denominator(self):
        ledger = common.EpisodeLedger()
        for request_id in ("ok", "error", "late", "missing", "duplicate"):
            self.record(ledger, request_id)
        ledger.record_terminal("ok", now=105, status="completed", checksum_valid=True)
        ledger.record_terminal("error", now=105, status="failed", error_code="WORKER_ERROR")
        ledger.record_terminal("late", now=111, status="completed", checksum_valid=True)
        ledger.record_terminal("duplicate", now=105, status="completed", checksum_valid=True)
        ledger.record_terminal("duplicate", now=106, status="completed", checksum_valid=True)
        ledger.reconcile(231, 120)
        self.assertEqual(len(ledger.dispatched()), 5)
        self.assertEqual(ledger.records["ok"].failure_class, "none")
        self.assertEqual(ledger.records["error"].failure_class, "uenv_error")
        self.assertEqual(ledger.records["late"].failure_class, "late_result")
        self.assertEqual(ledger.records["missing"].failure_class, "no_terminal_result")
        self.assertEqual(ledger.records["duplicate"].failure_class, "duplicate_terminal_result")
        ledger.assert_reconciled()

    def test_persistent_ledger_hashes_results_and_marks_late_as_failure(self):
        class Result:
            request_id = "late"
            batch_id = "batch"
            sample_index = 0
            status = "completed"
            error_code = ""
            error_message = ""
            trajectory_json = b'{"steps":[{"rollout_trace":{"response_ids":[1],"response_mask":[1]}}]}'
            rollout_log_probs = [0.0]
            rollout_param_version = 1
            rollout_policy_version = "p"
            reward = 0.0
            done = True
            termination_reason = "done"

        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "episode.sqlite"
            ledger = runner.PersistentLedger(path)
            ledger.plan("late", "dscodebench", "batch", 0, 100.0, 10.0)
            ledger.dispatched("late", 100.0)
            ledger.terminal(Result(), 111.0)
            ledger.close()
            connection = sqlite3.connect(path)
            row = connection.execute(
                "SELECT failure_class,result_checksum,result_checksum_valid FROM episode"
            ).fetchone()
            connection.close()
        self.assertEqual(row[0], "late_result")
        self.assertEqual(len(row[1]), 64)
        self.assertEqual(row[2], 1)


class AvailabilityTests(unittest.TestCase):
    def test_three_failures_and_three_recoveries_confirm_outage(self):
        samples = [{"timestamp": index, "ok": not (2 <= index <= 6)} for index in range(10)]
        self.assertEqual(common.classify_availability(samples), [(2.0, 7.0)])

    def test_short_blip_is_not_outage(self):
        samples = [{"timestamp": index, "ok": index != 2} for index in range(8)]
        self.assertEqual(common.classify_availability(samples), [])


class TraceAdmissionTests(unittest.TestCase):
    def test_trace_validation_requires_real_fields_and_minimum(self):
        trace = {
            "trace_id": "t1", "dataset": "scitab", "source_model": "doubao",
            "source_version": "frozen", "collected_at": "2026-07-22", "prompt_hash": "abc",
            "turns": [{
                "turn_index": 0, "assistant_output": "supports", "source_completion_tokens": 1,
                "target_qwen3_tokens": 1, "request_bytes": 10, "response_bytes": 8, "env_latency_ms": 3,
            }],
            "result_checksum": "def",
        }
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "scitab.jsonl"
            path.write_text(json.dumps(trace) + "\n", encoding="utf-8")
            result = common.validate_trace_file(path, dataset="scitab", minimum=1)
            self.assertEqual(result["valid_traces"], 1)
            with self.assertRaises(ValueError):
                common.validate_trace_file(path, dataset="scitab", minimum=100)


class DatasetSelectionTests(unittest.TestCase):
    def test_swe_selection_round_robins_repository_language_buckets(self):
        catalog = {}
        for repo, size in (("large", 20), ("small-a", 3), ("small-b", 3)):
            for index in range(size):
                instance_id = f"{repo}-{index}"
                catalog[instance_id] = {
                    "instance_id": instance_id,
                    "repo": repo,
                    "repo_language": "python",
                }
        selected = datasets.select_swe_instances(catalog, 9, 7)
        counts = {repo: sum(item.startswith(repo + "-") for item in selected) for repo in ("large", "small-a", "small-b")}
        self.assertEqual(counts, {"large": 3, "small-a": 3, "small-b": 3})
        self.assertEqual(selected, datasets.select_swe_instances(catalog, 9, 7))


class NamingTests(unittest.TestCase):
    def test_scale_pressure_names_have_no_historical_gate_or_verified_labels(self):
        paths = [
            HERE / "stress_suite.json", HERE / "run_stress_suite.py",
            HERE / "run_dscodebench_pressure.py", HERE / "run_swebench_pro_pressure.py",
        ]
        forbidden = ("gate3", "gate4", "swebench_verified", "verified.json", "SWE-bench Verified")
        for path in paths:
            text = path.read_text(encoding="utf-8").lower()
            for value in forbidden:
                self.assertNotIn(value.lower(), text, f"{value} remains in {path.name}")


class ReportCalculationTests(unittest.TestCase):
    def test_failure_rate_uses_every_dispatched_unique_id(self):
        rows = [
            {"request_id": "ok", "dispatch_started": "1", "failure_class": "none", "result_checksum_valid": "1", "terminal_count": "1"},
            {"request_id": "missing", "dispatch_started": "1", "failure_class": "no_terminal_result", "result_checksum_valid": "0", "terminal_count": "0"},
            {"request_id": "late", "dispatch_started": "1", "failure_class": "late_result", "result_checksum_valid": "1", "terminal_count": "1"},
            {"request_id": "not-sent", "dispatch_started": "0", "failure_class": "pending", "result_checksum_valid": "0", "terminal_count": "0"},
        ]
        metrics = report.episode_metrics(rows, 10)
        self.assertEqual(metrics["dispatch_started_unique"], 3)
        self.assertEqual(metrics["system_failure_unique"], 2)
        self.assertEqual(metrics["system_failure_rate"], 2 / 3)
        self.assertEqual(metrics["allowed_system_failures"], 0)
        self.assertEqual(metrics["throughput_eps"], 0.1)

    def test_resource_p95_and_growth(self):
        reference = report.resource_p95([
            {"rss_bytes": "100", "open_fds": "10", "threads": "20"},
            {"rss_bytes": "200", "open_fds": "20", "threads": "40"},
        ])
        growth = report.resource_growth(reference, {key: value * 1.1 for key, value in reference.items()})
        for value in growth.values():
            self.assertAlmostEqual(value, 0.1)

    def test_missing_resource_evidence_fails_closed(self):
        evidence = report.resource_evidence([], 14400)
        self.assertFalse(evidence["valid"])
        self.assertEqual(evidence["oom_events"], -1)


class ReplayServerTests(unittest.TestCase):
    def test_episode_bound_turn_replay_and_tokenization(self):
        import asyncio

        trace = {
            "trace_id": "trace-1", "turns": [
                {"assistant_output": "first", "target_qwen3_tokens": 0},
                {"assistant_output": "second", "target_qwen3_tokens": 0},
            ],
        }
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            tasks = {}
            for task in common.TASK_NAMES:
                path = root / f"{task}.jsonl"
                path.write_text(json.dumps(trace) + "\n", encoding="utf-8")
                tasks[task] = {"trace_file": str(path), "latency_profile": "p", "max_steps": 2}
            service = replay.ReplayService(
                {"latency_profiles": {"p": {"mean_seconds": 1, "std_seconds": 0, "base_tokens": 1}}, "tasks": tasks},
                run_seed=1,
                log_path=root / "round.csv",
            )
            headers = {"x-uenv-episode-id": "ep", "x-uenv-dataset": "dscodebench"}
            first_status, first = asyncio.run(service.complete(headers, b'{"model":"uenv-trace-dscodebench"}'))
            second_status, second = asyncio.run(service.complete(headers, b'{"model":"uenv-trace-dscodebench"}'))
            self.assertEqual((first_status, second_status), (200, 200))
            self.assertEqual(first["choices"][0]["message"]["content"], "first")
            self.assertEqual(second["choices"][0]["message"]["content"], "second")
            token_status, tokenized = asyncio.run(service.tokenize(b'{"text":["ab"]}'))
            self.assertEqual(token_status, 200)
            self.assertEqual(tokenized["data"][0]["token_ids"], [97, 98])

    def test_query_binding_supports_non_openhands_workers(self):
        import asyncio

        trace = {"trace_id": "trace-1", "turns": [{"assistant_output": "ok", "target_qwen3_tokens": 0}]}
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            tasks = {}
            for task in common.TASK_NAMES:
                path = root / f"{task}.jsonl"
                path.write_text(json.dumps(trace) + "\n", encoding="utf-8")
                tasks[task] = {"trace_file": str(path), "latency_profile": "p", "max_steps": 1}
            service = replay.ReplayService(
                {"latency_profiles": {"p": {"mean_seconds": 1, "std_seconds": 0, "base_tokens": 1}}, "tasks": tasks},
                run_seed=1,
                log_path=root / "round.csv",
            )
            status, _ = asyncio.run(service.complete(
                {},
                b'{"model":"uenv-trace-dscodebench"}',
                {"uenv_episode_id": ["ep-query"], "uenv_dataset": ["dscodebench"]},
            ))
        self.assertEqual(status, 200)
        bound = runner.bind_replay_url(
            "http://127.0.0.1:8899/v1/chat/completions",
            episode_id="ep-query",
            task="dscodebench",
        )
        self.assertIn("uenv_episode_id=ep-query", bound)
        self.assertIn("uenv_dataset=dscodebench", bound)

    def test_openhands_rollout_adds_episode_binding_headers(self):
        module_path = HERE.parents[1] / "integrations" / "openhands" / "uenv_runtime" / "llm_rollout.py"
        spec = importlib.util.spec_from_file_location("uenv_llm_rollout_test", module_path)
        self.assertIsNotNone(spec)
        module = importlib.util.module_from_spec(spec)
        assert spec and spec.loader
        spec.loader.exec_module(module)

        captured = {}

        class FakeLlm:
            def completion(self, *args, **kwargs):
                captured.update(kwargs)
                return {"raw_response": {"choices": []}}

            async def acompletion(self, *args, **kwargs):
                return {"raw_response": {"choices": []}}

        with tempfile.TemporaryDirectory() as temp:
            config = Path(temp) / "llm.json"
            config.write_text(json.dumps({"api_key": "x", "base_url": "http://127.0.0.1/v1", "model": "openai/replay"}), encoding="utf-8")
            collector = module.RolloutTraceCollector(config)
            llm = FakeLlm()
            collector.install(llm, episode_id="ep-42", dataset="swebench_pro")
            llm.completion(extra_headers={"Existing": "kept"})
        self.assertEqual(captured["extra_headers"]["Existing"], "kept")
        self.assertEqual(captured["extra_headers"]["X-UEnv-Episode-Id"], "ep-42")
        self.assertEqual(captured["extra_headers"]["X-UEnv-Dataset"], "swebench_pro")


if __name__ == "__main__":
    unittest.main()
