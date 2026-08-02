from __future__ import annotations

import csv
import json
import importlib.util
import os
import sqlite3
import tempfile
import unittest
from pathlib import Path

from uenv_stress.cli import run_formal_stability_suite as runner
from uenv_stress.core import stability_test_common as common
from uenv_stress.stability import replay_server as replay
from uenv_stress.stability import report
from uenv_stress.tools import prepare_datasets as datasets
from uenv_stress.tools import prepare_mixed_stability_corpora as mixed_corpora


HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
CONFIG_DIR = ROOT / "uenv_stress" / "config"


class StabilityConfigTests(unittest.TestCase):
    def test_formal_config_rates_and_capacity(self):
        config = common.load_config(CONFIG_DIR / "stability_suite.json")
        self.assertEqual(set(config["tasks"]), set(common.TASK_NAMES))
        self.assertEqual(
            config["traces"]["selection_strategy"], "paired_alternating_episode"
        )
        self.assertAlmostEqual(sum(task["allocation_share"] for task in config["tasks"].values()), 1.0)
        self.assertAlmostEqual(sum(task["target_rate_eps"] for task in config["tasks"].values()), 364.5864)
        self.assertEqual(config["load"]["rate_basis"], "100xa100_throughput_estimate")
        self.assertEqual(
            config["latency_replay"]["latency_basis"],
            "observed_episode_elapsed_proxy",
        )
        capacity = common.required_capacity(config, "reference")
        self.assertEqual(capacity["by_task"], {
            "dscodebench": 522,
            "swebench_pro": 130,
            "olymmath": 6051,
            "scitab": 1521,
            "pubmedqa": 1093,
        })
        self.assertEqual(capacity["total_slots"], 9317)

    def test_phase_rates(self):
        config = common.load_config(CONFIG_DIR / "stability_suite.json")
        task = config["tasks"]["dscodebench"]
        self.assertEqual(common.phase_rate(task, "selfcheck"), 31.6725)
        self.assertEqual(common.phase_rate(task, "reference"), 6.3345)
        self.assertEqual(common.phase_rate(task, "pressure"), 63.345)
        self.assertEqual(common.phase_rate(task, "capacity"), 7.6014)
        self.assertEqual(common.phase_rate(task, "burst"), 25.338)

    def test_pressure_records_overload_without_weakening_stability_gate(self):
        with tempfile.TemporaryDirectory() as temp:
            fleet_path = Path(temp) / "fleet.json"
            fleet_path.write_text(
                json.dumps(
                    {
                        "workers": [
                            {"worker_id": f"worker-{index}", "capacity": 8}
                            for index in range(1024)
                        ]
                    }
                ),
                encoding="utf-8",
            )
            with self.assertRaisesRegex(ValueError, "capacity"):
                runner.load_fleet(fleet_path, 9317)
            pressure = runner.load_fleet(
                fleet_path, 93144, allow_overload=True
            )
        assessment = pressure["capacity_assessment"]
        self.assertFalse(assessment["capacity_sufficient"])
        self.assertTrue(assessment["intentional_overload"])
        self.assertGreater(assessment["expected_overload_multiple"], 11)

    def test_reference_and_stability_share_constant_then_batch_segments(self):
        config = common.load_config(CONFIG_DIR / "stability_suite.json")
        reference = runner.arrival_segments(config, "reference", 100)
        stability = runner.arrival_segments(config, "stability", 100)
        pressure = runner.arrival_segments(config, "pressure", 100)
        self.assertEqual(reference, stability)
        self.assertEqual(reference, pressure)
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
    @staticmethod
    def trace(
        trace_id: str,
        *,
        episode_total_ms: float = 0,
        source_api_latency_ms: float = 0,
        tokens: tuple[int, ...] = (100, 300),
    ):
        return {
            "trace_id": trace_id,
            "dataset": "swe-bench-pro",
            "source_model": "doubao-test",
            "episode_total_ms": episode_total_ms,
            "source_api_latency_ms": source_api_latency_ms,
            "turns": [
                {
                    "target_qwen3_tokens": token,
                    "source_completion_tokens": token,
                    "env_latency_ms": 0,
                }
                for token in tokens
            ],
        }

    def test_recorded_latency_precedes_episode_proxy(self):
        trace = self.trace(
            "recorded", episode_total_ms=9000, source_api_latency_ms=2000
        )
        medians = common.latency_imputation_medians(
            [trace], max_missing_ratio=0.05
        )
        wait = common.trace_turn_waits(
            trace, imputation_medians=medians
        )
        self.assertEqual(wait["latency_source"], "recorded")
        self.assertEqual(wait["episode_elapsed_proxy_ms"], 2000)
        self.assertEqual(wait["turn_proxy_wait_seconds"], [0.5, 1.5])

    def test_multiturn_waits_sum_to_episode_proxy_without_token_floor(self):
        trace = self.trace("proxy", episode_total_ms=10000)
        medians = common.latency_imputation_medians(
            [trace], max_missing_ratio=0.05
        )
        wait = common.trace_turn_waits(
            trace, imputation_medians=medians
        )
        self.assertEqual(
            wait["latency_source"], "observed_episode_elapsed_proxy"
        )
        self.assertEqual(wait["turn_proxy_wait_seconds"], [2.5, 7.5])
        self.assertEqual(sum(wait["turn_proxy_wait_seconds"]), 10.0)

    def test_five_percent_missing_is_imputed_by_dataset_model_median(self):
        traces = [
            self.trace(
                f"trace-{index}",
                episode_total_ms=0 if index == 19 else 1000 + index,
                tokens=(10,),
            )
            for index in range(20)
        ]
        medians = common.latency_imputation_medians(
            traces, max_missing_ratio=0.05
        )
        wait = common.trace_turn_waits(
            traces[-1], imputation_medians=medians
        )
        self.assertEqual(wait["latency_source"], "dataset_median_imputed")
        self.assertEqual(wait["episode_elapsed_proxy_ms"], 1009)

    def test_more_than_five_percent_missing_fails_closed(self):
        traces = [
            self.trace(
                f"trace-{index}",
                episode_total_ms=0 if index >= 18 else 1000,
                tokens=(10,),
            )
            for index in range(20)
        ]
        with self.assertRaisesRegex(ValueError, "missing ratio"):
            common.latency_imputation_medians(
                traces, max_missing_ratio=0.05
            )


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
            "dataset_id": "item-1",
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
            result = common.validate_trace_file(
                path,
                dataset="scitab",
                minimum=1,
                max_latency_missing_ratio=0.05,
            )
            self.assertEqual(result["valid_traces"], 1)
            self.assertEqual(result["source_models"]["doubao"]["episodes"], 1)
            self.assertEqual(result["effective_replay_seconds_p95"], 0.003)
            self.assertEqual(
                result["effective_latency_sources"],
                {"observed_episode_elapsed_proxy": 1},
            )
            self.assertEqual(
                result["latency_sources"],
                {"observed_episode_elapsed_proxy": 1},
            )
            with self.assertRaises(ValueError):
                common.validate_trace_file(path, dataset="scitab", minimum=100)

    def test_pair_validation_uses_dataset_id_and_audits_prompt_hash(self):
        base = {
            "dataset": "scitab",
            "dataset_id": "item-1",
            "pair_id": "item-1",
            "prompt_hash": "same",
            "turns": [{"target_qwen3_tokens": 1}],
        }
        doubao = {
            **base,
            "trace_id": "d",
            "source_model": "doubao-test",
        }
        qwen = {
            **base,
            "trace_id": "q",
            "source_model": "qwen3-6.35b-a3b",
        }
        evidence = common.validate_paired_trace_order(
            [doubao, qwen], expected_pairs=1
        )
        self.assertEqual(
            evidence["source_family_counts"], {"doubao": 1, "qwen": 1}
        )
        with self.assertRaisesRegex(ValueError, "must be Doubao"):
            common.validate_paired_trace_order([qwen, doubao])
        mismatch = common.validate_paired_trace_order(
            [doubao, {**qwen, "prompt_hash": "different"}]
        )
        self.assertEqual(mismatch["prompt_hash_mismatches"], 1)
        self.assertTrue(mismatch["dataset_id_match"])

    def test_mixed_builder_selects_qwen_by_doubao_dataset_ids(self):
        def row(trace_id, dataset_id, model):
            return {
                "trace_id": trace_id,
                "dataset": "scitab",
                "dataset_id": dataset_id,
                "source_model": model,
                "prompt_hash": f"prompt-{dataset_id}",
                "episode_total_ms": 4000,
                "turns": [
                    {"target_qwen3_tokens": 1},
                    {"target_qwen3_tokens": 3},
                ],
            }

        doubao = [
            row("d-a", "a", "doubao-test"),
            row("d-b", "b", "doubao-test"),
        ]
        qwen = [
            row("q-extra", "extra", "qwen3-6.35b-a3b"),
            row("q-b", "b", "qwen3-6.35b-a3b"),
            row("q-a", "a", "qwen3-6.35b-a3b"),
        ]
        output, evidence = mixed_corpora.paired_rows(
            "scitab", doubao, qwen, expected_pairs=2
        )
        self.assertEqual(
            [item["trace_id"] for item in output],
            ["d-a", "q-a", "d-b", "q-b"],
        )
        self.assertEqual(evidence["qwen_extra_count"], 1)
        annotated = mixed_corpora.attach_replay_waits(output)
        self.assertEqual(
            [turn["replay_wait_ms"] for turn in annotated[0]["turns"]],
            [1000.0, 3000.0],
        )
        self.assertEqual(
            sum(turn["replay_wait_ms"] for turn in annotated[0]["turns"]),
            annotated[0]["episode_total_ms"],
        )


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
            CONFIG_DIR / "scale_suite.json",
            ROOT / "uenv_stress" / "cli" / "run_scale_suite.py",
            ROOT / "uenv_stress" / "scale" / "dscodebench_pressure.py",
            ROOT / "uenv_stress" / "scale" / "swebench_pro_pressure.py",
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
    @staticmethod
    def paired_trace(
        trace_id: str,
        pair_id: str,
        source_model: str,
        outputs: tuple[str, ...],
    ):
        return {
            "trace_id": trace_id,
            "dataset": "dscodebench",
            "dataset_id": pair_id,
            "pair_id": pair_id,
            "source_model": source_model,
            "prompt_hash": f"prompt-{pair_id}",
            "episode_total_ms": len(outputs),
            "turns": [
                {
                    "assistant_output": output,
                    "target_qwen3_tokens": 1,
                }
                for output in outputs
            ],
        }

    def test_paired_episode_binding_and_tokenization(self):
        import asyncio

        traces = [
            self.paired_trace(
                "p0-doubao", "pair-0", "doubao-test", ("d0-first", "d0-second")
            ),
            self.paired_trace(
                "p0-qwen", "pair-0", "qwen3-6.35b-a3b", ("q0-first", "q0-second")
            ),
            self.paired_trace(
                "p1-doubao", "pair-1", "doubao-test", ("d1-first", "d1-second")
            ),
            self.paired_trace(
                "p1-qwen", "pair-1", "qwen3-6.35b-a3b", ("q1-first", "q1-second")
            ),
        ]
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            path = root / "dscodebench.jsonl"
            path.write_text(
                "\n".join(json.dumps(trace) for trace in traces) + "\n",
                encoding="utf-8",
            )
            tasks = {
                "dscodebench": {
                    "trace_file": str(path),
                    "max_steps": 2,
                    "sampling_policy": "paired_doubao_qwen_alternating",
                    "expected_pairs": 2,
                }
            }
            service = replay.ReplayService(
                {
                    "latency_replay": {"max_missing_ratio": 0.05},
                    "tasks": tasks,
                },
                run_seed=1,
                log_path=root / "round.csv",
            )
            def complete(episode_id, sequence):
                headers = {
                    "x-uenv-episode-id": episode_id,
                    "x-uenv-dataset": "dscodebench",
                }
                return asyncio.run(
                    service.complete(
                        headers,
                        b'{"model":"uenv-trace-dscodebench"}',
                        {"uenv_sequence": [str(sequence)]},
                    )
                )

            first_status, first = complete("ep-0", 0)
            other_status, other = complete("ep-1", 1)
            second_status, second = complete("ep-0", 0)
            wrapped_status, wrapped = complete("ep-2", 2)
            self.assertEqual(
                (first_status, other_status, second_status, wrapped_status),
                (200, 200, 200, 200),
            )
            self.assertEqual(first["choices"][0]["message"]["content"], "d0-first")
            self.assertEqual(other["choices"][0]["message"]["content"], "q0-first")
            self.assertEqual(second["choices"][0]["message"]["content"], "d0-second")
            self.assertEqual(wrapped["choices"][0]["message"]["content"], "d1-first")
            health_status, health = asyncio.run(service.health())
            self.assertEqual(health_status, 200)
            self.assertEqual(health["selection_strategy"], "paired_alternating_episode")
            self.assertEqual(health["replay"]["dscodebench"]["completed_cycles"], 0)
            self.assertEqual(health["replay"]["dscodebench"]["calls"], 4)
            self.assertEqual(health["replay"]["dscodebench"]["hits"], 4)
            self.assertEqual(health["replay"]["dscodebench"]["misses"], 0)
            self.assertEqual(health["replay"]["dscodebench"]["hit_rate"], 1.0)
            self.assertEqual(
                health["replay"]["dscodebench"]["source_family_usage"],
                {"doubao": 2, "qwen": 1},
            )
            with (root / "round.csv").open(newline="", encoding="utf-8") as source:
                rows = list(csv.DictReader(source))
            self.assertEqual(
                [row["trace_id"] for row in rows],
                ["p0-doubao", "p0-qwen", "p0-doubao", "p1-doubao"],
            )
            self.assertTrue(
                all(
                    row["selection_strategy"]
                    == "paired_doubao_qwen_alternating"
                    for row in rows
                )
            )
            self.assertEqual(
                [row["pair_id"] for row in rows],
                ["pair-0", "pair-0", "pair-0", "pair-1"],
            )
            token_status, tokenized = asyncio.run(service.tokenize(b'{"text":["ab"]}'))
            self.assertEqual(token_status, 200)
            self.assertEqual(tokenized["data"][0]["token_ids"], [97, 98])

    def test_query_binding_supports_non_openhands_workers(self):
        import asyncio

        traces = [
            self.paired_trace("trace-d", "pair-0", "doubao-test", ("ok-d",)),
            self.paired_trace("trace-q", "pair-0", "qwen-test", ("ok-q",)),
        ]
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            path = root / "dscodebench.jsonl"
            path.write_text(
                "\n".join(json.dumps(trace) for trace in traces) + "\n",
                encoding="utf-8",
            )
            tasks = {
                "dscodebench": {
                    "trace_file": str(path),
                    "max_steps": 1,
                    "sampling_policy": "paired_doubao_qwen_alternating",
                    "expected_pairs": 1,
                }
            }
            service = replay.ReplayService(
                {
                    "latency_replay": {"max_missing_ratio": 0.05},
                    "tasks": tasks,
                },
                run_seed=1,
                log_path=root / "round.csv",
            )
            status, _ = asyncio.run(service.complete(
                {},
                b'{"model":"uenv-trace-dscodebench"}',
                {
                    "uenv_episode_id": ["ep-query"],
                    "uenv_dataset": ["dscodebench"],
                    "uenv_sequence": ["0"],
                    "uenv_trace_id": ["trace-d"],
                    "uenv_source_model": ["doubao-test"],
                    "uenv_pair_id": ["pair-0"],
                },
            ))
        self.assertEqual(status, 200)
        bound = runner.bind_replay_url(
            "http://127.0.0.1:8899/v1/chat/completions",
            episode_id="ep-query",
            task="dscodebench",
            sequence=0,
            trace_id="trace-d",
            source_model="doubao-test",
            pair_id="pair-0",
        )
        self.assertIn("uenv_episode_id=ep-query", bound)
        self.assertIn("uenv_dataset=dscodebench", bound)
        self.assertIn("uenv_sequence=0", bound)
        self.assertIn("uenv_trace_id=trace-d", bound)
        self.assertIn("uenv_source_model=doubao-test", bound)
        self.assertIn("uenv_pair_id=pair-0", bound)

    def test_openhands_rollout_adds_episode_binding_headers(self):
        repo_root = Path(
            os.environ.get("UENV_REPO_ROOT", str(ROOT.parent.parent))
        )
        module_path = (
            repo_root
            / "integrations"
            / "openhands"
            / "uenv_runtime"
            / "llm_rollout.py"
        )
        if not module_path.is_file():
            self.skipTest(f"repository OpenHands integration not present: {module_path}")
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
