"""Focused rollout-trace mode tests; no OpenHands server, model, or GPU needed."""

from __future__ import annotations

import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest import mock


OPENHANDS_DIR = Path(__file__).resolve().parents[1]
REPO_ROOT = Path(__file__).resolve().parents[3]
if str(OPENHANDS_DIR) not in sys.path:
    sys.path.insert(0, str(OPENHANDS_DIR))

from uenv_runtime.llm_rollout import (  # noqa: E402
    RolloutTraceCollector,
    finish_rollout_trace,
    start_rollout_trace,
)


def _load_runner_module():
    path = REPO_ROOT / "scripts/openhands/openhands_runner.py"
    spec = importlib.util.spec_from_file_location("uenv_openhands_runner_test", path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"could not load {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class _FailingSetupCollector:
    def __init__(self, _config_path: str) -> None:
        pass

    def install(self, *_args, **_kwargs) -> None:
        raise ValueError("provider has no trace support")


class _FailingFinalizeCollector:
    warnings = ["provider fell back"]

    def finalize(self):
        raise ValueError("token ids unavailable")


class _SuccessfulCollector:
    warnings: list[str] = []

    def finalize(self):
        return {"rollout_trace": {"response_ids": [11, 12], "response_mask": [1, 1]}}


class _FakeLLM:
    def __init__(self) -> None:
        self.calls: list[dict] = []
        self.completion = self._completion
        self.acompletion = self._acompletion

    def _completion(self, **kwargs):
        self.calls.append(dict(kwargs))
        if "logprobs" in kwargs:
            raise RuntimeError("unsupported parameter: logprobs")
        return {"choices": []}

    async def _acompletion(self, **kwargs):
        return self._completion(**kwargs)


class RolloutTraceModeTests(unittest.TestCase):
    def test_off_never_constructs_or_installs_collector(self):
        def forbidden_factory(_path):
            raise AssertionError("collector must not be constructed in off mode")

        collector, status = start_rollout_trace(
            "off",
            llm=object(),
            config_path="unused.json",
            episode_id="episode-1",
            dataset="swebench_pro",
            collector_factory=forbidden_factory,
        )

        self.assertIsNone(collector)
        self.assertEqual(status["state"], "disabled")
        self.assertFalse(status["enabled"])

    def test_best_effort_setup_failure_becomes_warning(self):
        collector, status = start_rollout_trace(
            "best-effort",
            llm=object(),
            config_path="unused.json",
            episode_id="episode-1",
            dataset="swebench_pro",
            collector_factory=_FailingSetupCollector,
        )

        self.assertIsNone(collector)
        self.assertEqual(status["state"], "unavailable")
        self.assertIn("setup failed", status["warnings"][0])

    def test_required_setup_failure_is_strict(self):
        with self.assertRaisesRegex(RuntimeError, "rollout trace setup failed"):
            start_rollout_trace(
                "required",
                llm=object(),
                config_path="unused.json",
                episode_id="episode-1",
                dataset="swebench_pro",
                collector_factory=_FailingSetupCollector,
            )

    def test_best_effort_finalize_failure_becomes_warning(self):
        fields, status = finish_rollout_trace(
            "best-effort", _FailingFinalizeCollector()
        )

        self.assertEqual(fields, {})
        self.assertEqual(status["state"], "unavailable")
        self.assertEqual(len(status["warnings"]), 2)
        self.assertIn("finalize failed", status["warnings"][-1])

    def test_required_finalize_failure_is_strict(self):
        with self.assertRaisesRegex(RuntimeError, "rollout trace finalize failed"):
            finish_rollout_trace("required", _FailingFinalizeCollector())

    def test_successful_finalize_reports_token_count(self):
        fields, status = finish_rollout_trace("required", _SuccessfulCollector())

        self.assertEqual(fields["rollout_trace"]["response_ids"], [11, 12])
        self.assertEqual(status["state"], "collected")
        self.assertEqual(status["token_count"], 2)

    def test_best_effort_retries_only_without_rejected_trace_options(self):
        with tempfile.TemporaryDirectory() as tmp:
            config_path = Path(tmp) / "llm.json"
            config_path.write_text(
                json.dumps(
                    {
                        "base_url": "http://model.example/v1",
                        "model": "openai/test-model",
                        "api_key": "test",
                    }
                ),
                encoding="utf-8",
            )
            llm = _FakeLLM()
            collector = RolloutTraceCollector(config_path)
            collector.install(llm, best_effort=True)

            response = llm.completion(temperature=0.1)

        self.assertEqual(response, {"choices": []})
        self.assertEqual(len(llm.calls), 2)
        self.assertTrue(llm.calls[0]["logprobs"])
        self.assertNotIn("logprobs", llm.calls[1])
        self.assertEqual(llm.calls[1]["temperature"], 0.1)
        self.assertTrue(collector.warnings)

    def test_required_collector_does_not_retry_rejected_options(self):
        with tempfile.TemporaryDirectory() as tmp:
            config_path = Path(tmp) / "llm.json"
            config_path.write_text(
                json.dumps(
                    {
                        "base_url": "http://model.example/v1",
                        "model": "openai/test-model",
                    }
                ),
                encoding="utf-8",
            )
            llm = _FakeLLM()
            collector = RolloutTraceCollector(config_path)
            collector.install(llm, best_effort=False)

            with self.assertRaisesRegex(RuntimeError, "unsupported parameter"):
                llm.completion()

        self.assertEqual(len(llm.calls), 1)

    def test_agent_job_runner_forces_required_mode(self):
        runner = _load_runner_module()
        env = {
            "UENV_ROLLOUT_TRACE": "off",
            "UENV_REQUIRE_SWE_RESPONSE_TRACE": "0",
        }

        runner._require_agent_job_rollout_trace(env)

        self.assertEqual(env["UENV_ROLLOUT_TRACE"], "required")
        self.assertEqual(env["UENV_REQUIRE_SWE_RESPONSE_TRACE"], "1")

    def test_agent_job_subprocess_receives_required_mode(self):
        runner = _load_runner_module()

        class Client:
            def complete_agent_job(self, **_kwargs):
                return True

        job = SimpleNamespace(
            job_id="job-1",
            run_id="run-1",
            gateway_url="http://gateway.example",
            gateway_api_key="key",
            max_iterations=3,
            benchmark_variant="pro",
            mode="llm",
        )
        completed = runner.subprocess.CompletedProcess(
            args=["bash", "run.sh", "llm"],
            returncode=1,
            stdout="",
            stderr="expected test failure",
        )
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            runner.RUNS_DIR = root / "runs"
            runner.COMPLETION_SPOOL_DIR = root / "spool"
            runner.RUN_SCRIPT = "run.sh"
            runner._active_jobs = 1
            with mock.patch.object(
                runner.subprocess, "run", return_value=completed
            ) as run:
                runner._run_agent_job(Client(), job, "agent-1")

        child_env = run.call_args.kwargs["env"]
        self.assertEqual(child_env["UENV_ROLLOUT_TRACE"], "required")
        self.assertEqual(child_env["UENV_REQUIRE_SWE_RESPONSE_TRACE"], "1")

    def test_unknown_job_completion_spool_is_archived(self):
        runner = _load_runner_module()

        class Client:
            last_complete_code = ""
            last_complete_message = ""

            def complete_agent_job(self, **_kwargs):
                self.last_complete_code = "UNKNOWN_JOB"
                self.last_complete_message = "job no longer exists"
                return False

        with tempfile.TemporaryDirectory() as tmp:
            runner.COMPLETION_SPOOL_DIR = Path(tmp) / "spool"
            path = runner._spool_completion(
                {
                    "job_id": "missing-job",
                    "run_id": "run-1",
                    "status": "completed",
                    "reward": 0.0,
                    "trajectory_id": "",
                    "error_message": "",
                    "agent_id": "agent-1",
                    "metadata": {},
                }
            )

            replay_done = runner._submit_spooled_completion(Client(), path)

            self.assertTrue(replay_done)
            self.assertFalse(path.exists())
            archived = runner.COMPLETION_SPOOL_DIR / "archived-unknown-job" / path.name
            self.assertTrue(archived.is_file())


if __name__ == "__main__":
    unittest.main()
