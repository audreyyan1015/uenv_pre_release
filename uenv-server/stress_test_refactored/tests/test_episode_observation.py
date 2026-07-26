import json
import sqlite3
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace

from uenv_stress.cli import run_formal_stability_suite as stability_runner
from uenv_stress.core import result
from uenv_stress.scale import dscodebench_pressure
from uenv_stress.scale import rule_task_pressure
from uenv_stress.scale import swebench_pro_pressure


def envelope(request_id: str, sample_index: int = 0):
    return SimpleNamespace(
        request_id=request_id,
        batch_id="batch-1",
        sample_index=sample_index,
        env_type="code",
        parallel_mode="fully_async",
        timeout_seconds=30,
        sample_context_json=json.dumps({
            "stress_run_id": "run-1",
            "dataset": "dscodebench",
            "dataset_problem_id": "problem-1",
            "sequence": sample_index,
            "replay_strategy": "round_robin_episode",
        }).encode(),
    )


def sample_result(request_id: str):
    return SimpleNamespace(
        request_id=request_id,
        batch_id="batch-1",
        sample_index=0,
        status="completed",
        reward=1.0,
        done=True,
        termination_reason="done",
        error_code="",
        error_message="",
        trajectory_json=json.dumps({
            "total_steps": 2,
            "steps": [{
                "rollout_trace": {
                    "response_ids": [1, 2],
                    "response_mask": [1, 1],
                }
            }],
        }).encode(),
        rollout_param_version=1,
        rollout_policy_version="policy-1",
        rollout_log_probs=[-0.1, -0.2],
    )


class EpisodeObservationTests(unittest.TestCase):
    def test_success_observation_uses_strict_shared_contract(self):
        rows = result.observe_episode_batch(
            [envelope("episode-1")],
            [sample_result("episode-1")],
            suite="scale",
            run_id="run-1",
            phase="fully_async",
            planned_at=100.0,
            dispatched_at=101.0,
            terminal_at=103.0,
            batch_rpc_latency_ms=2000.0,
        )
        self.assertEqual(len(rows), 1)
        row = rows[0]
        self.assertEqual(tuple(row), result.EPISODE_OBSERVATION_FIELDS)
        self.assertEqual(row["episode_id"], "episode-1")
        self.assertEqual(row["dataset_item_id"], "problem-1")
        self.assertEqual(row["actual_steps"], 2)
        self.assertEqual(row["response_tokens"], 2)
        self.assertTrue(row["training_trace_valid"])
        self.assertEqual(row["failure_class"], "none")
        self.assertEqual(row["worker_attribution"], "unavailable_in_adapter_result")

    def test_batch_keeps_one_row_per_submitted_episode(self):
        rows = result.observe_episode_batch(
            [envelope("duplicate"), envelope("missing", 1)],
            [sample_result("duplicate"), sample_result("duplicate")],
            suite="scale",
            run_id="run-1",
            phase="sync",
            planned_at=100.0,
            dispatched_at=100.5,
            terminal_at=101.0,
            batch_rpc_latency_ms=500.0,
        )
        self.assertEqual([row["request_id"] for row in rows], ["duplicate", "missing"])
        self.assertEqual(rows[0]["terminal_count"], 2)
        self.assertEqual(rows[0]["failure_class"], "duplicate_terminal_result")
        self.assertEqual(rows[1]["status"], "missing_result")
        self.assertEqual(rows[1]["error_code"], "NO_TERMINAL_RESULT")

    def test_stability_sqlite_and_jsonl_use_the_same_fields(self):
        with tempfile.TemporaryDirectory() as temp:
            database = Path(temp) / "episode.sqlite"
            target = Path(temp) / "episode-observations.jsonl"
            ledger = stability_runner.PersistentLedger(database)
            ledger.plan(
                "episode-1", "dscodebench", "batch-1", 0, 100.0, 30.0,
                envelope=envelope("episode-1"),
                run_id="run-1",
                phase="stability",
            )
            ledger.dispatched("episode-1", 101.0)
            ledger.terminal(sample_result("episode-1"), 103.0)
            self.assertEqual(ledger.export_jsonl(target), 1)
            ledger.close()
            row = json.loads(target.read_text(encoding="utf-8"))
            self.assertEqual(tuple(row), result.EPISODE_OBSERVATION_FIELDS)
            connection = sqlite3.connect(database)
            columns = tuple(
                item[1] for item in connection.execute("PRAGMA table_info(episode)")
            )
            connection.close()
            self.assertEqual(columns, result.EPISODE_OBSERVATION_FIELDS)

    def test_all_five_scale_workloads_emit_the_shared_observation(self):
        self.assertIn("observe_episode_batch", dscodebench_pressure.LOAD_CLIENT)
        self.assertIn("observe_episode_batch", swebench_pro_pressure.SWE_CLIENT)
        self.assertIn("observe_episode_batch", rule_task_pressure.LOAD_CLIENT)
        self.assertIn("episode-observations", dscodebench_pressure.LOAD_CLIENT)
        self.assertIn("episode-observations", swebench_pro_pressure.SWE_CLIENT)
        self.assertIn("episode-observations", rule_task_pressure.LOAD_CLIENT)
        self.assertEqual(
            set(rule_task_pressure.TASKS),
            {"olymmath", "scitab", "pubmedqa"},
        )


if __name__ == "__main__":
    unittest.main()
