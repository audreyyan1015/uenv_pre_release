from __future__ import annotations

import contextlib
import importlib.util
import io
import json
from pathlib import Path
import shlex
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
TOOL_PATH = ROOT / "tools/swe/build_catalog.py"


def load_tool():
    spec = importlib.util.spec_from_file_location("uenv_build_swe_catalog", TOOL_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot import {TOOL_PATH}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class SweCatalogToolTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.tool = load_tool()

    def _convert(self, directory: Path, variant: str, rows: list[dict[str, object]]):
        source = directory / f"{variant}.jsonl"
        destination = directory / f"{variant}-catalog.json"
        source.write_text(
            "".join(json.dumps(row) + "\n" for row in rows),
            encoding="utf-8",
        )
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()) as stderr:
            status = self.tool.main(
                [
                    "--variant",
                    variant,
                    "--input",
                    str(source),
                    "--output",
                    str(destination),
                ]
            )
        return status, destination, stderr.getvalue()

    @staticmethod
    def _base(instance_id: str) -> dict[str, object]:
        return {
            "instance_id": instance_id,
            "repo": "owner/repository",
            "base_commit": "0123456789abcdef",
            "problem_statement": "Fix the regression",
        }

    def test_official_smith_image_name_is_normalized(self) -> None:
        row = self._base("oauthlib__oauthlib.issue-1")
        row.update(
            {
                "repo": "swesmith/oauthlib__oauthlib.issue-1",
                "image_name": "swebench/swesmith.x86_64.oauthlib_1776_oauthlib.issue-1",
                "test_cmd": None,
            }
        )
        with tempfile.TemporaryDirectory() as temp_dir:
            status, output, stderr = self._convert(Path(temp_dir), "smith", [row])
            self.assertEqual(status, 0, stderr)
            value = json.loads(output.read_text(encoding="utf-8"))[row["instance_id"]]
        self.assertEqual(value["benchmark_variant"], "smith")
        self.assertEqual(value["image_cache_key"], row["image_name"])
        self.assertNotIn("test_cmd", value)

    def test_pro_javascript_selected_tests_are_shell_quoted(self) -> None:
        row = self._base("owner__javascript-1")
        row.update(
            {
                "dockerhub_tag": "javascript-1",
                "repo_language": "javascript",
                "selected_test_files_to_run": [
                    "test/safe.spec.js | shard 1",
                    "test/a file.js; touch /tmp/should-not-run",
                ],
            }
        )
        with tempfile.TemporaryDirectory() as temp_dir:
            status, output, stderr = self._convert(Path(temp_dir), "pro", [row])
            self.assertEqual(status, 0, stderr)
            value = json.loads(output.read_text(encoding="utf-8"))[row["instance_id"]]

        self.assertEqual(value["image_cache_key"], "jefzda/sweap-images:javascript-1")
        self.assertEqual(value["pre_test_cmd"], "redis-server --daemonize yes")
        self.assertEqual(
            shlex.split(value["test_cmd"]),
            [
                "npm",
                "test",
                "--",
                "test/safe.spec.js",
                "test/a file.js; touch /tmp/should-not-run",
            ],
        )
        self.assertNotIn("shard 1", value["test_cmd"])

    def test_verified_and_lite_generate_official_image_namespace(self) -> None:
        for variant in ("verified", "lite"):
            with self.subTest(variant=variant), tempfile.TemporaryDirectory() as temp_dir:
                row = self._base(f"owner__{variant}-1")
                status, output, stderr = self._convert(Path(temp_dir), variant, [row])
                self.assertEqual(status, 0, stderr)
                value = json.loads(output.read_text(encoding="utf-8"))[row["instance_id"]]
                self.assertTrue(
                    value["image_cache_key"].startswith("swebench/sweb.eval.x86_64."),
                    value["image_cache_key"],
                )
                self.assertEqual(value["benchmark_variant"], variant)

    def test_verified_and_lite_reject_non_official_image_namespace(self) -> None:
        for variant in ("verified", "lite"):
            with self.subTest(variant=variant), tempfile.TemporaryDirectory() as temp_dir:
                row = self._base(f"owner__{variant}-1")
                row["image_cache_key"] = "private.invalid/arbitrary:latest"
                status, output, stderr = self._convert(Path(temp_dir), variant, [row])
                self.assertEqual(status, 2)
                self.assertFalse(output.exists())
                self.assertIn("命名空间", stderr)

    def test_smith_requires_official_smith_image_namespace(self) -> None:
        row = self._base("owner__smith-1")
        row["image_name"] = "private.invalid/arbitrary:latest"
        with tempfile.TemporaryDirectory() as temp_dir:
            status, output, stderr = self._convert(Path(temp_dir), "smith", [row])
        self.assertEqual(status, 2)
        self.assertFalse(output.exists())
        self.assertIn("swesmith", stderr)


if __name__ == "__main__":
    unittest.main()
