from __future__ import annotations

import argparse
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
PREPARER_PATH = ROOT / "examples/swe/prepare_verl_data.py"


def load_preparer():
    spec = importlib.util.spec_from_file_location("uenv_prepare_verl_data", PREPARER_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot import {PREPARER_PATH}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class SweExamplesTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.preparer = load_preparer()

    def test_bundled_training_catalog_produces_hub_free_row(self) -> None:
        catalog = self.preparer.load_catalog(ROOT / "config/swe/smith-smoke.json")
        self.assertTrue(catalog)
        self.preparer.validate_smith_row(catalog[0])
        args = argparse.Namespace(
            llm_config_path="/etc/uenv/openhands-llm.json",
            max_iterations=7,
            benchmark_variant="smith",
        )
        row = self.preparer.verl_row(catalog[0], 0, args)

        self.assertEqual(row["ability"], "swe")
        self.assertEqual(row["extra_info"]["execution_mode"], "agent")
        self.assertEqual(row["extra_info"]["benchmark_variant"], "smith")
        self.assertEqual(row["extra_info"]["env_package_id"], "")
        self.assertEqual(row["extra_info"]["env_package_version"], "")
        self.assertEqual(row["extra_info"]["max_iterations"], 7)

    def test_non_smith_catalog_is_rejected_for_training_example(self) -> None:
        catalog = self.preparer.load_catalog(ROOT / "config/swe/verified.json")
        with self.assertRaisesRegex(SystemExit, "not SWE-smith"):
            self.preparer.validate_smith_row(catalog[0])

    def test_catalog_loader_accepts_object_and_list(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            object_path = Path(directory) / "object.json"
            list_path = Path(directory) / "list.json"
            object_path.write_text(
                json.dumps({"case-1": {"problem_statement": "fix it"}}),
                encoding="utf-8",
            )
            list_path.write_text(
                json.dumps([{"instance_id": "case-2"}]), encoding="utf-8"
            )

            self.assertEqual(
                self.preparer.load_catalog(object_path)[0]["instance_id"], "case-1"
            )
            self.assertEqual(
                self.preparer.load_catalog(list_path)[0]["instance_id"], "case-2"
            )

    def test_instance_and_limit_are_mutually_exclusive(self) -> None:
        result = subprocess.run(
            [
                sys.executable,
                str(PREPARER_PATH),
                "--catalog",
                str(ROOT / "config/swe/smith-smoke.json"),
                "--benchmark-variant",
                "smith",
                "--output-dir",
                "/tmp/uenv-unused-output",
                "--instance",
                "repo__issue-1",
                "--limit",
                "1",
                "--max-iterations",
                "5",
            ],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("mutually exclusive", result.stderr)


if __name__ == "__main__":
    unittest.main()
