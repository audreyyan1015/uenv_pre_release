from __future__ import annotations

import os
import subprocess
import sys
import unittest
from pathlib import Path


REPO_DIR = Path(__file__).resolve().parents[1]


class SiteCustomizeTest(unittest.TestCase):
    def test_ascend_backend_skips_cuda_only_patch_registration(self) -> None:
        env = os.environ.copy()
        env["PYTHONPATH"] = str(REPO_DIR / "src")
        env["UENV_DEVICE_BACKEND"] = "ascend"
        env["UENV_PATCH_TORCH_CUDA_IS_AVAILABLE_NO_DEVICES"] = "1"
        env["UENV_PATCH_VERL_DEVICE_CAPABILITY_FALLBACK"] = "1"

        script = (
            "import builtins, sitecustomize; "
            "print(hasattr(builtins, '_uenv_import_hook_callbacks'))"
        )
        output = subprocess.check_output(
            [sys.executable, "-c", script],
            env=env,
            text=True,
        ).strip()

        self.assertEqual(output, "False")

    def test_cuda_backend_keeps_cuda_patch_registration(self) -> None:
        env = os.environ.copy()
        env["PYTHONPATH"] = str(REPO_DIR / "src")
        env["UENV_DEVICE_BACKEND"] = "cuda"
        env["UENV_PATCH_TORCH_CUDA_IS_AVAILABLE_NO_DEVICES"] = "1"

        script = (
            "import builtins, sitecustomize; "
            "callbacks = getattr(builtins, '_uenv_import_hook_callbacks', {}); "
            "print('torch' in callbacks)"
        )
        output = subprocess.check_output(
            [sys.executable, "-c", script],
            env=env,
            text=True,
        ).strip()

        self.assertEqual(output, "True")


if __name__ == "__main__":
    unittest.main()
