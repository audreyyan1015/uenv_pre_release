import csv
import json
import sqlite3
import tempfile
import unittest
from pathlib import Path

from uenv_stress.core import suite_metrics


class DistributionTests(unittest.TestCase):
    def test_worker_load_distribution_has_required_statistics(self):
        result = suite_metrics.distribution([0, 10, 20, 30])
        self.assertEqual(result["minimum"], 0)
        self.assertEqual(result["mean"], 15)
        self.assertEqual(result["p95"], 30)
        self.assertEqual(result["maximum"], 30)
        self.assertAlmostEqual(result["coefficient_of_variation"], 0.7453559925)


class ScaleSuiteMetricsTests(unittest.TestCase):
    def test_all_five_datasets_and_three_modes_share_one_metrics_contract(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            documents = []
            for dataset in suite_metrics.DATASETS:
                for mode in suite_metrics.PARALLEL_MODES:
                    path = root / f"{dataset}-{mode}.jsonl"
                    row = {
                        "dataset": dataset,
                        "parallel_mode": mode,
                        "dispatch_started": True,
                        "terminal_at": 103.0,
                        "status": "completed",
                        "failure_class": "none",
                        "reward": 1.0,
                        "actual_steps": 2,
                        "end_to_end_ms": 2000.0,
                    }
                    path.write_text(json.dumps(row) + "\n", encoding="utf-8")
                    documents.append({
                        "run_id": "run-1",
                        "dataset": {"name": dataset},
                        "parallel_mode": mode,
                        "elapsed_seconds": 10.0,
                        "episode_observations": {
                            "local_artifact": str(path),
                            "row_count": 1,
                            "submitted_count": 1,
                            "complete": True,
                        },
                        "worker_dispatch_coverage": {
                            "expected_workers": 2,
                            "load_timeline": {
                                "per_worker": [
                                    {
                                        "worker_id": "worker-0",
                                        "started_episodes_observed": 1,
                                        "completed_episodes": 1,
                                    },
                                    {
                                        "worker_id": "worker-1",
                                        "started_episodes_observed": 0,
                                        "completed_episodes": 0,
                                    },
                                ]
                            },
                        },
                        "trace_replay": {
                            "calls": 1,
                            "hits": 1,
                            "misses": 0,
                            "assigned_episodes": 1,
                            "sampling_strategy": "round_robin_episode",
                        },
                        "resource_observations": {"sample_count": 1},
                    })
            document = {
                "suite_id": "suite-1",
                "executed": True,
                "scenarios": [{
                    "name": "all",
                    "status": "passed",
                    "returncode": 0,
                    "result": documents,
                    "cleanup": {
                        "attempted": True,
                        "passed": True,
                        "errors": [],
                    },
                }],
                "infrastructure": {"protected_process_unchanged": True},
            }
            result = suite_metrics.build_scale_suite_metrics(document)

        json.dumps(result)
        self.assertTrue(result["complete"])
        self.assertEqual(
            {key for key, value in result["by_dataset"].items()
             if value["planned_episodes"]},
            set(suite_metrics.DATASETS),
        )
        self.assertEqual(
            {key for key, value in result["by_parallel_mode"].items()
             if value["planned_episodes"]},
            set(suite_metrics.PARALLEL_MODES),
        )
        self.assertEqual(len(result["by_worker"]), 2)
        self.assertEqual(len(result["by_worker_dataset"]), 10)
        self.assertAlmostEqual(
            result["overall"]["throughput"]["submission_eps"],
            15 / 150,
        )
        self.assertAlmostEqual(
            result["overall"]["throughput"]["completion_eps"],
            15 / 150,
        )
        self.assertAlmostEqual(
            result["overall"]["throughput"]["successful_eps"],
            15 / 150,
        )
        self.assertAlmostEqual(
            result["by_worker"][0]["throughput"]["completion_eps"],
            15 / 150,
        )
        self.assertIsNone(
            result["by_worker"][0]["throughput"]["successful_eps"]
        )
        distribution = result["worker_load_distribution"]
        self.assertEqual(distribution["minimum"], 0)
        self.assertEqual(distribution["mean"], 7.5)
        self.assertEqual(distribution["p95"], 15)
        self.assertEqual(distribution["maximum"], 15)
        self.assertEqual(distribution["coefficient_of_variation"], 1.0)
        self.assertEqual(result["replay"]["overall"]["hit_rate"], 1.0)
        self.assertFalse(result["submission_rate"]["available"])
        self.assertEqual(result["resources"]["sample_count"], 15)
        self.assertTrue(result["cleanup"]["passed"])


class StabilitySuiteMetricsTests(unittest.TestCase):
    def test_stability_records_rates_workers_replay_resources_and_cleanup(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            database = root / "episode.sqlite"
            connection = sqlite3.connect(database)
            connection.execute("""
                CREATE TABLE episode(
                  task TEXT, parallel_mode TEXT, dispatch_started INTEGER,
                  terminal_at REAL, status TEXT, failure_class TEXT,
                  reward REAL, actual_steps INTEGER, end_to_end_ms REAL
                )
            """)
            for index, dataset in enumerate(suite_metrics.DATASETS):
                connection.execute(
                    "INSERT INTO episode VALUES(?,?,?,?,?,?,?,?,?)",
                    (
                        dataset, "fully_async", 1, 103.0, "completed", "none",
                        1.0, index + 1, 1000.0 + index,
                    ),
                )
            connection.commit()
            connection.close()

            server_log = root / "server.log"
            server_log.write_text(
                "\n".join([
                    f"request_id=run-1-{dataset}-{index} "
                    f"worker_id={'worker-0' if index < 3 else 'worker-1'} "
                    "episode_completed"
                    for index, dataset in enumerate(suite_metrics.DATASETS)
                ]),
                encoding="utf-8",
            )
            worker_load = suite_metrics.parse_worker_load_log(
                server_log,
                run_id="run-1",
                expected_worker_ids=["worker-0", "worker-1"],
            )
            resource_csv = root / "resource.csv"
            with resource_csv.open("w", encoding="utf-8", newline="") as target:
                writer = csv.DictWriter(target, fieldnames=[
                    "rss_bytes", "open_fds", "threads", "running_containers",
                    "worker_exits", "oom_events", "fd_exhaustions",
                    "thread_exhaustions", "uenv_crashes", "manual_restarts",
                ])
                writer.writeheader()
                for value in (10, 20):
                    writer.writerow({
                        "rss_bytes": value,
                        "open_fds": value,
                        "threads": value,
                        "running_containers": 0,
                        "worker_exits": 0,
                        "oom_events": 0,
                        "fd_exhaustions": 0,
                        "thread_exhaustions": 0,
                        "uenv_crashes": 0,
                        "manual_restarts": 0,
                    })
            replay_health = {
                "selection_strategy": "round_robin_episode",
                "replay": {
                    dataset: {
                        "calls": 1,
                        "hits": 1,
                        "misses": 0,
                        "assigned_episodes": 1,
                        "source_model_usage": {"doubao-test": 1},
                        "source_family_usage": {"doubao": 1},
                        "source_model_replay_stats": {
                            "doubao-test": {
                                "turns": 1,
                                "completion_tokens_p50": 10,
                                "completion_tokens_p95": 10,
                                "wait_seconds_p50": 2.0,
                                "wait_seconds_p95": 2.0,
                            }
                        },
                        "latency_source_usage": {
                            "observed_episode_elapsed_proxy": 1
                        },
                        "wait_seconds_p50": 2.0,
                        "wait_seconds_p95": 2.0,
                        "completion_tokens_p50": 10,
                        "completion_tokens_p95": 10,
                    }
                    for dataset in suite_metrics.DATASETS
                },
            }
            result = suite_metrics.build_stability_suite_metrics(
                ledger_path=database,
                run_id="run-1",
                phase="stability",
                duration_seconds=60.0,
                parallel_mode="fully_async",
                planned_rates={dataset: 1.0 for dataset in suite_metrics.DATASETS},
                expected_worker_ids=["worker-0", "worker-1"],
                worker_load=worker_load,
                replay_health=replay_health,
                resource_csv=resource_csv,
                resource_sample_seconds=30.0,
                cleanup={
                    "remaining_workers": 0,
                    "remaining_containers": 0,
                    "remaining_processes": 0,
                },
            )

        json.dumps(result)
        self.assertTrue(result["complete"])
        self.assertEqual(len(result["by_worker_dataset"]), 10)
        self.assertAlmostEqual(
            result["overall"]["throughput"]["submission_eps"], 5 / 60
        )
        self.assertAlmostEqual(
            result["overall"]["throughput"]["completion_eps"], 5 / 60
        )
        self.assertAlmostEqual(
            result["overall"]["throughput"]["successful_eps"], 5 / 60
        )
        self.assertAlmostEqual(
            result["by_worker"][0]["throughput"]["completion_eps"], 3 / 60
        )
        self.assertIsNone(
            result["by_worker"][0]["throughput"]["submission_eps"]
        )
        self.assertEqual(result["worker_load_distribution"]["minimum"], 2)
        self.assertEqual(result["worker_load_distribution"]["mean"], 2.5)
        self.assertEqual(result["worker_load_distribution"]["p95"], 3)
        self.assertEqual(result["worker_load_distribution"]["maximum"], 3)
        self.assertAlmostEqual(
            result["worker_load_distribution"]["coefficient_of_variation"], 0.2
        )
        self.assertEqual(result["replay"]["overall"]["hit_rate"], 1.0)
        load_profile = result["replay"]["load_profiles"][0]
        self.assertEqual(
            load_profile["source_model_usage"], {"doubao-test": 1}
        )
        self.assertEqual(load_profile["wait_seconds_p95"], 2.0)
        rate = result["by_dataset"]["dscodebench"]["submission_rate"]
        self.assertEqual(rate["planned_rate_eps"], 1.0)
        self.assertAlmostEqual(rate["actual_rate_eps"], 1 / 60)
        self.assertAlmostEqual(rate["relative_deviation"], -59 / 60)
        self.assertEqual(result["resources"]["coverage"], 1.0)
        self.assertTrue(result["cleanup"]["passed"])


if __name__ == "__main__":
    unittest.main()
