import unittest
from pathlib import Path

from uenv_stress.core import stress_test_common
from uenv_stress.scale import (
    dscodebench_pressure,
    rule_task_pressure,
    swebench_pro_pressure,
)


class PressureResultSchemaTest(unittest.TestCase):
    def test_shared_metrics_cover_capacity_submission_outcome_and_latency(self):
        metrics = stress_test_common.pressure_result_metrics(
            configured_workers=64,
            worker_capacity=4,
            batch_size=128,
            planned_batches=20,
            concurrent_batches=20,
            submission_strategy="submit_all_batches_then_collect",
            client_submit_seconds=2.0,
            submitted_batches=20,
            elapsed_seconds=100.0,
            submitted=2560,
            completed=2560,
            failed=0,
            rpc_error_episodes=0,
            protocol_errors=0,
            latencies_ms=[10.0, 20.0, 30.0],
        )

        stress_test_common.assert_pressure_result_schema(metrics)
        self.assertEqual(metrics["worker_slots"], 256)
        self.assertEqual(metrics["requested_episode_concurrency"], 2560)
        self.assertEqual(metrics["capacity_waves"], 10.0)
        self.assertEqual(metrics["client_submit_rate_eps"], 1280.0)
        self.assertEqual(metrics["batch_latency_ms"]["p95"], 30.0)

    def test_missing_required_metric_fails_closed(self):
        metrics = stress_test_common.pressure_result_metrics(
            configured_workers=1,
            worker_capacity=1,
            batch_size=1,
            planned_batches=1,
            concurrent_batches=1,
            submission_strategy="submit_all_batches_then_collect",
            client_submit_seconds=0.1,
            submitted_batches=1,
            elapsed_seconds=1.0,
            submitted=1,
            completed=1,
            failed=0,
            rpc_error_episodes=0,
            protocol_errors=0,
            latencies_ms=[1.0],
        )
        del metrics["capacity_waves"]

        with self.assertRaisesRegex(ValueError, "capacity_waves"):
            stress_test_common.assert_pressure_result_schema(metrics)

    def test_every_generated_load_client_enforces_shared_schema(self):
        for name, source in (
            ("Math", rule_task_pressure.LOAD_CLIENT),
            ("DSCodeBench", dscodebench_pressure.LOAD_CLIENT),
            ("SWE-bench Pro", swebench_pro_pressure.SWE_CLIENT),
        ):
            with self.subTest(dataset=name):
                self.assertIn("pressure_result_metrics(", source)
                self.assertIn("assert_pressure_result_schema(", source)

    def test_fleet_resource_csv_covers_host_capacity_and_process_metrics(self):
        source = (
            Path(stress_test_common.__file__).with_name("fleet_supervisor.py")
            .read_text(encoding="utf-8")
        )
        for field in (
            '"processes"',
            '"rss_bytes"',
            '"open_fds"',
            '"threads"',
            '"mem_total_bytes"',
            '"available_bytes"',
            '"load1"',
            '"load5"',
            '"load15"',
        ):
            with self.subTest(field=field):
                self.assertIn(field, source)

    def test_every_scale_runner_persists_common_resource_and_server_evidence(self):
        for module in (
            rule_task_pressure,
            dscodebench_pressure,
            swebench_pro_pressure,
        ):
            source = Path(module.__file__).read_text(encoding="utf-8")
            with self.subTest(module=module.__name__):
                self.assertIn("--resource-csv", source)
                self.assertIn('local_run / "server.log"', source)
                self.assertIn("episode-observations", source)


if __name__ == "__main__":
    unittest.main()
