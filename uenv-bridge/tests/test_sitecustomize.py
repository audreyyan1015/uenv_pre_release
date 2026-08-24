from __future__ import annotations

import os
import importlib.util
import subprocess
import sys
import unittest
import types
from pathlib import Path

import torch


REPO_DIR = Path(__file__).resolve().parents[1]
SITECUSTOMIZE_PATH = REPO_DIR / "src" / "sitecustomize.py"


def _load_local_sitecustomize():
    spec = importlib.util.spec_from_file_location("uenv_sitecustomize_test", SITECUSTOMIZE_PATH)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


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

    def test_patch_verl_empty_response_batch_filter_turns_empty_masks_into_noop(self) -> None:
        saved_modules: dict[str, object | None] = {}
        module_names = [
            "verl",
            "verl.trainer",
            "verl.trainer.ppo",
            "verl.trainer.ppo.rollout_corr_helper",
            "verl.trainer.ppo.core_algos",
        ]

        try:
            for name in module_names:
                saved_modules[name] = sys.modules.get(name)

            verl_mod = types.ModuleType("verl")
            verl_mod.__path__ = []  # type: ignore[attr-defined]
            trainer_mod = types.ModuleType("verl.trainer")
            trainer_mod.__path__ = []  # type: ignore[attr-defined]
            ppo_mod = types.ModuleType("verl.trainer.ppo")
            ppo_mod.__path__ = []  # type: ignore[attr-defined]
            rollout_mod = types.ModuleType("verl.trainer.ppo.rollout_corr_helper")
            core_mod = types.ModuleType("verl.trainer.ppo.core_algos")

            rollout_calls: list[tuple[torch.Tensor, torch.Tensor, torch.Tensor]] = []
            loss_calls: list[tuple[torch.Tensor, torch.Tensor, str]] = []

            def original_rollout(old_log_prob, rollout_log_prob, response_mask, *args, **kwargs):
                rollout_calls.append((old_log_prob, rollout_log_prob, response_mask))
                return "original", response_mask, {"rollout_corr/original": 1.0}

            def original_agg_loss(loss_mat, loss_mask, loss_agg_mode, **kwargs):
                loss_calls.append((loss_mat, loss_mask, loss_agg_mode))
                return loss_mat.sum() + 1.0

            rollout_mod.compute_rollout_correction_and_rejection_mask = original_rollout
            core_mod.agg_loss = original_agg_loss
            ppo_mod.rollout_corr_helper = rollout_mod
            ppo_mod.core_algos = core_mod
            trainer_mod.ppo = ppo_mod
            verl_mod.trainer = trainer_mod

            sys.modules["verl"] = verl_mod
            sys.modules["verl.trainer"] = trainer_mod
            sys.modules["verl.trainer.ppo"] = ppo_mod
            sys.modules["verl.trainer.ppo.rollout_corr_helper"] = rollout_mod
            sys.modules["verl.trainer.ppo.core_algos"] = core_mod

            sitecustomize = _load_local_sitecustomize()
            sitecustomize._patch_verl_empty_response_batch_filter()

            empty_mask = torch.zeros((2, 3), dtype=torch.int64)
            loss_mat = torch.ones((2, 3), requires_grad=True)

            rollout_result = rollout_mod.compute_rollout_correction_and_rejection_mask(
                torch.zeros((2, 3)),
                torch.zeros((2, 3)),
                empty_mask,
            )
            self.assertIsNone(rollout_result[0])
            self.assertTrue(torch.equal(rollout_result[1], empty_mask))
            self.assertEqual(rollout_result[2]["rollout_corr/empty_response_batch"], 1.0)
            self.assertEqual(len(rollout_calls), 0)

            loss = core_mod.agg_loss(loss_mat, empty_mask, "token-mean")
            self.assertEqual(loss.item(), 0.0)
            loss.backward()
            self.assertTrue(torch.equal(loss_mat.grad, torch.zeros_like(loss_mat)))
            self.assertEqual(len(loss_calls), 0)

            non_empty_mask = torch.tensor([[1, 0, 0], [0, 0, 0]], dtype=torch.int64)
            rollout_result = rollout_mod.compute_rollout_correction_and_rejection_mask(
                torch.zeros((2, 3)),
                torch.zeros((2, 3)),
                non_empty_mask,
            )
            self.assertEqual(rollout_result[0], "original")
            self.assertEqual(len(rollout_calls), 1)

            loss_mat = torch.ones((2, 3), requires_grad=True)
            loss = core_mod.agg_loss(loss_mat, non_empty_mask, "token-mean")
            self.assertGreater(loss.item(), 0.0)
            self.assertEqual(len(loss_calls), 1)
        finally:
            for name in reversed(module_names):
                previous = saved_modules[name]
                if previous is None:
                    sys.modules.pop(name, None)
                else:
                    sys.modules[name] = previous


if __name__ == "__main__":
    unittest.main()
