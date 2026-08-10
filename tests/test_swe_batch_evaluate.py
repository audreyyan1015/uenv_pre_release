from __future__ import annotations

import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import tempfile
import time
import unittest
from unittest import mock


ROOT = Path(__file__).resolve().parents[1]
RUNNER_PATH = ROOT / "libexec/uenv/swe/evaluate_batch.py"


def load_runner():
    spec = importlib.util.spec_from_file_location("uenv_swe_evaluate_batch", RUNNER_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot import {RUNNER_PATH}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def catalog_row(instance_id: str, *, variant: str | None = "verified") -> dict[str, object]:
    row: dict[str, object] = {
        "instance_id": instance_id,
        "repo": "owner/repository",
        "base_commit": "0123456789abcdef",
        "problem_statement": f"Fix {instance_id}",
        "image_cache_key": f"registry.invalid/{instance_id}:latest",
    }
    if variant is not None:
        row["benchmark_variant"] = variant
    return row


class FakeResponse:
    def __init__(self, value: object) -> None:
        self.value = value

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc, traceback) -> None:
        return None

    def read(self, _limit: int = -1) -> bytes:
        return json.dumps(self.value).encode("utf-8")


class SweBatchEvaluateTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.runner = load_runner()

    def _files(
        self,
        directory: Path,
        instances: list[str],
        *,
        variant: str = "verified",
    ) -> tuple[Path, Path]:
        catalog = directory / "catalog.json"
        input_path = directory / "cases.jsonl"
        catalog.write_text(
            json.dumps({instance: catalog_row(instance, variant=variant) for instance in instances}),
            encoding="utf-8",
        )
        input_path.write_text(
            "".join(
                json.dumps({"id": f"case-{number}", "instance_id": instance}) + "\n"
                for number, instance in enumerate(instances)
            ),
            encoding="utf-8",
        )
        return catalog, input_path

    @staticmethod
    def _argv(
        catalog: Path,
        input_path: Path,
        output: Path,
        artifacts: Path,
    ) -> list[str]:
        return [
            "local",
            "--model",
            "local-model",
            "--base-url",
            "http://127.0.0.1:8000/v1",
            "--gateway",
            "http://127.0.0.1:28999",
            "--catalog",
            str(catalog),
            "--benchmark-variant",
            "verified",
            "--input",
            str(input_path),
            "--output",
            str(output),
            "--artifacts-dir",
            str(artifacts),
            "--max-iterations",
            "30",
            "--batch-size",
            "3",
        ]

    def test_all_batch_arguments_are_required(self) -> None:
        complete = [
            "local",
            "--model", "model",
            "--base-url", "http://127.0.0.1:8000/v1",
            "--gateway", "http://127.0.0.1:28999",
            "--catalog", "catalog.json",
            "--benchmark-variant", "verified",
            "--input", "cases.jsonl",
            "--output", "results.jsonl",
            "--artifacts-dir", "artifacts",
            "--max-iterations", "30",
            "--batch-size", "2",
        ]
        required_options = (
            "--model",
            "--gateway",
            "--catalog",
            "--benchmark-variant",
            "--input",
            "--output",
            "--artifacts-dir",
            "--max-iterations",
            "--batch-size",
        )
        self.runner.parser().parse_args(complete)
        for option in required_options:
            index = complete.index(option)
            missing = complete[:index] + complete[index + 2 :]
            with self.subTest(option=option), contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit) as raised:
                    self.runner.parser().parse_args(missing)
                self.assertEqual(raised.exception.code, 2)

        with contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit) as raised:
                self.runner.parser().parse_args(complete[1:])
        self.assertEqual(raised.exception.code, 2)

    def test_batch_preserves_input_order_and_isolates_case_failure(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            directory = Path(temp_dir)
            instances = ["owner__repo-1", "owner__repo-2", "owner__repo-3"]
            catalog, input_path = self._files(directory, instances)
            output = directory / "results.jsonl"
            artifacts = directory / "artifacts"
            finished: list[str] = []

            def fake_secure(path: Path) -> None:
                path.mkdir()

            def fake_run_case(index, case, args, base_url, model_key, gateway_key, root):
                # Finish in the reverse of input order to prove collection is stable.
                time.sleep((2 - index) * 0.01)
                finished.append(case["id"])
                failed = index == 1
                return {
                    "case_id": case["id"],
                    "instance_id": case["instance_id"],
                    "status": "failed" if failed else "completed",
                    "exit_code": 1 if failed else 0,
                    "error": "intentional failure" if failed else "",
                    "artifact_dir": str(root / case["id"]),
                }

            with (
                mock.patch.object(self.runner, "_secret", return_value=("http://model/v1", "model-key")),
                mock.patch.object(self.runner, "_gateway_key", return_value="gateway-key"),
                mock.patch.object(self.runner, "_gateway_rows") as gateway_rows,
                mock.patch.object(self.runner, "_secure_artifact_root", side_effect=fake_secure),
                mock.patch.object(self.runner, "_run_case", side_effect=fake_run_case),
                contextlib.redirect_stdout(io.StringIO()),
                contextlib.redirect_stderr(io.StringIO()),
            ):
                status = self.runner.main(self._argv(catalog, input_path, output, artifacts))

            self.assertEqual(status, 1)
            gateway_rows.assert_called_once()
            self.assertEqual(set(finished), {"case-0", "case-1", "case-2"})
            results = [json.loads(line) for line in output.read_text(encoding="utf-8").splitlines()]
            self.assertEqual([row["case_id"] for row in results], ["case-0", "case-1", "case-2"])
            self.assertEqual([row["status"] for row in results], ["completed", "failed", "completed"])

    def test_missing_catalog_variant_defaults_to_verified(self) -> None:
        instance = "owner__repo-1"
        catalog = {instance: catalog_row(instance, variant=None)}
        with tempfile.TemporaryDirectory() as temp_dir:
            input_path = Path(temp_dir) / "cases.jsonl"
            input_path.write_text(json.dumps({"instance_id": instance}) + "\n", encoding="utf-8")
            rows = self.runner._cases(input_path, catalog, "verified")
            self.assertEqual(rows, [{"id": instance, "instance_id": instance}])
            with self.assertRaisesRegex(self.runner.UserError, "catalog variant"):
                self.runner._cases(input_path, catalog, "lite")

    def test_gateway_preflight_sends_key_and_compares_all_metadata(self) -> None:
        instance = "owner__repo-1"
        local = catalog_row(instance, variant=None)
        requests = []

        def urlopen(request, timeout):
            requests.append((request, timeout))
            return FakeResponse(dict(local))

        with mock.patch.object(self.runner.urllib.request, "urlopen", side_effect=urlopen):
            self.runner._gateway_rows(
                "http://127.0.0.1:28999",
                "gateway-secret",
                [{"id": "one", "instance_id": instance}],
                {instance: local},
            )
        self.assertEqual(len(requests), 1)
        request, timeout = requests[0]
        self.assertEqual(timeout, 10)
        self.assertEqual(request.full_url, f"http://127.0.0.1:28999/runtime/v1/instances/{instance}")
        self.assertEqual(request.get_header("X-api-key"), "gateway-secret")

        mismatched = dict(local)
        mismatched["image_cache_key"] = "registry.invalid/different:latest"
        with mock.patch.object(
            self.runner.urllib.request,
            "urlopen",
            return_value=FakeResponse(mismatched),
        ):
            with self.assertRaisesRegex(self.runner.UserError, "image_cache_key"):
                self.runner._gateway_rows(
                    "http://127.0.0.1:28999",
                    "gateway-secret",
                    [{"id": "one", "instance_id": instance}],
                    {instance: local},
                )

    def test_gateway_mismatch_stops_before_artifacts_or_runner(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            directory = Path(temp_dir)
            catalog, input_path = self._files(directory, ["owner__repo-1"])
            output = directory / "results.jsonl"
            artifacts = directory / "artifacts"
            with (
                mock.patch.object(self.runner, "_secret", return_value=("http://model/v1", "key")),
                mock.patch.object(self.runner, "_gateway_key", return_value="gateway-key"),
                mock.patch.object(
                    self.runner,
                    "_gateway_rows",
                    side_effect=self.runner.UserError("Worker metadata mismatch"),
                ),
                mock.patch.object(self.runner, "_secure_artifact_root") as secure,
                mock.patch.object(self.runner, "_run_case") as run_case,
                contextlib.redirect_stdout(io.StringIO()),
                contextlib.redirect_stderr(io.StringIO()),
            ):
                status = self.runner.main(self._argv(catalog, input_path, output, artifacts))
            self.assertEqual(status, 2)
            secure.assert_not_called()
            run_case.assert_not_called()
            self.assertFalse(output.exists())
            self.assertFalse(artifacts.exists())

    def test_existing_artifact_directory_is_never_reused(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            directory = Path(temp_dir)
            catalog, input_path = self._files(directory, ["owner__repo-1"])
            output = directory / "results.jsonl"
            artifacts = directory / "artifacts"
            artifacts.mkdir()
            sentinel = artifacts / "keep.txt"
            sentinel.write_text("keep", encoding="utf-8")
            with (
                mock.patch.object(self.runner, "_secret", return_value=("http://model/v1", "key")),
                mock.patch.object(self.runner, "_gateway_key", return_value="gateway-key"),
                mock.patch.object(self.runner, "_gateway_rows"),
                mock.patch.object(self.runner, "_run_case") as run_case,
                contextlib.redirect_stdout(io.StringIO()),
                contextlib.redirect_stderr(io.StringIO()),
            ):
                status = self.runner.main(self._argv(catalog, input_path, output, artifacts))
            self.assertEqual(status, 2)
            run_case.assert_not_called()
            self.assertEqual(sentinel.read_text(encoding="utf-8"), "keep")
            self.assertFalse(output.exists())

    def test_api_key_file_accepts_one_conventional_trailing_newline(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            key_file = Path(temp_dir) / "ark.key"
            key_file.write_text("ark-secret\n", encoding="utf-8")
            key_file.chmod(0o600)
            args = self.runner.parser().parse_args(
                [
                    "volcengine",
                    "--model", "ep-test",
                    "--api-key-file", str(key_file),
                    "--gateway", "http://127.0.0.1:28999",
                    "--catalog", "catalog.json",
                    "--benchmark-variant", "verified",
                    "--input", "cases.jsonl",
                    "--output", "results.jsonl",
                    "--artifacts-dir", "artifacts",
                    "--max-iterations", "30",
                    "--batch-size", "1",
                ]
            )
            with mock.patch.dict(os.environ, {}, clear=True):
                base_url, key = self.runner._secret(args)
            self.assertEqual(base_url, "https://ark.cn-beijing.volces.com/api/v3")
            self.assertEqual(key, "ark-secret")


if __name__ == "__main__":
    unittest.main()
