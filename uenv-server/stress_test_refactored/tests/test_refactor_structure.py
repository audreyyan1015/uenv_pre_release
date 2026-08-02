from __future__ import annotations

import json
from pathlib import Path
import tempfile
import unittest

from uenv_stress.cli import run_trace_collection
from uenv_stress.core.runtime_config import load_runtime_inventory


ROOT = Path(__file__).resolve().parents[1]
CONFIG_DIR = ROOT / "uenv_stress" / "config"


class RefactorStructureTests(unittest.TestCase):
    def test_runtime_inventory_uses_two_current_workers_and_bans_retired_host(self):
        inventory = load_runtime_inventory(CONFIG_DIR / "runtime_hosts.json")
        self.assertEqual(inventory.server.ssh_host, "8.130.75.157")
        self.assertEqual(
            [worker.ssh_host for worker in inventory.workers],
            ["8.130.65.20", "8.145.51.129"],
        )
        self.assertIn("8.130.86.71", inventory.banned_worker_hosts)
        self.assertNotIn(
            "8.130.86.71",
            [worker.ssh_host for worker in inventory.workers],
        )
        self.assertEqual(
            inventory.worker_node_arguments(),
            [
                "8.130.65.20:192.168.0.139",
                "8.145.51.129:192.168.0.138",
            ],
        )

    def test_runtime_inventory_rejects_secret_fields(self):
        document = json.loads(
            (CONFIG_DIR / "runtime_hosts.json").read_text(encoding="utf-8")
        )
        document["password"] = "must-not-be-here"
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "runtime.json"
            path.write_text(json.dumps(document), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "secret keys"):
                load_runtime_inventory(path)

    def test_runtime_inventory_rejects_banned_active_worker(self):
        document = json.loads(
            (CONFIG_DIR / "runtime_hosts.json").read_text(encoding="utf-8")
        )
        document["workers"][0]["ssh_host"] = "8.130.86.71"
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "runtime.json"
            path.write_text(json.dumps(document), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "banned worker hosts"):
                load_runtime_inventory(path)

    def test_trace_collection_has_dedicated_entrypoint(self):
        arguments = run_trace_collection.arguments(["--help"])
        self.assertEqual(arguments[0], "swebench-pro-trace-collection")
        self.assertEqual(arguments[1], "--help")

    def test_scale_and_trace_configs_are_separate(self):
        scale = json.loads(
            (CONFIG_DIR / "scale_suite.json").read_text(encoding="utf-8")
        )
        traces = json.loads(
            (CONFIG_DIR / "trace_collection.json").read_text(encoding="utf-8")
        )
        self.assertNotIn("trace_collection", scale)
        self.assertEqual(traces["swebench_pro"]["instance_count"], 50)
        self.assertFalse(traces["swebench_pro"]["uses_1024_workers"])

    def test_refactored_trace_storage_uses_home_not_opt(self):
        sources = [
            path
            for path in (ROOT / "uenv_stress").rglob("*")
            if path.is_file() and path.suffix in {".py", ".json"}
        ]
        combined = "\n".join(
            path.read_text(encoding="utf-8") for path in sources
        )
        self.assertNotIn("/opt/uenv-stress/trace-corpora", combined)
        self.assertNotIn("/opt/uenv-stress/raw-traces", combined)
        self.assertIn("/home/uenv-stress/trace-corpora", combined)


if __name__ == "__main__":
    unittest.main()
