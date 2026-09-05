from __future__ import annotations

import importlib.util
import types
import unittest
from pathlib import Path


SCRIPT_PATH = Path(__file__).resolve().parents[1] / "scripts" / "train" / "wrappers" / "run_verl_main_ppo.py"


def _load_script_module():
    spec = importlib.util.spec_from_file_location("uenv_run_verl_main_ppo", SCRIPT_PATH)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


class RunVerlMainPPOTest(unittest.TestCase):
    def test_patch_task_runner_runs_uenv_patch_inside_actor(self) -> None:
        script = _load_script_module()
        calls = []

        class TaskRunner:
            def run(self, config):
                calls.append(("run", config))
                return "ok"

        def apply_patch():
            calls.append(("patch", None))

        script._apply_uenv_patches = apply_patch
        script._patch_task_runner(types.SimpleNamespace(TaskRunner=TaskRunner))

        self.assertEqual(TaskRunner().run({"x": 1}), "ok")
        self.assertEqual(calls, [("patch", None), ("run", {"x": 1})])

    def test_patch_task_runner_supports_task_runner_v1(self) -> None:
        script = _load_script_module()
        calls = []

        class TaskRunnerV1:
            def run(self, config):
                calls.append(("run_v1", config))
                return "ok-v1"

        def apply_patch():
            calls.append(("patch", None))

        script._apply_uenv_patches = apply_patch
        script._patch_task_runner(types.SimpleNamespace(TaskRunnerV1=TaskRunnerV1))

        self.assertEqual(TaskRunnerV1().run({"version": 1}), "ok-v1")
        self.assertEqual(calls, [("patch", None), ("run_v1", {"version": 1})])


if __name__ == "__main__":
    unittest.main()
