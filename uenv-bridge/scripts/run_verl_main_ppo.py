from __future__ import annotations

import os


def _apply_uenv_patches() -> None:
    if os.environ.get("UENV_PATCH_VERL_MODEL_VERSION_RESPONSE") in {"1", "true", "True", "enabled"}:
        from uenv.bridge.verl_model_version_patch import apply_verl_vllm_model_version_patch

        apply_verl_vllm_model_version_patch()


def _patch_task_runner(verl_main_ppo) -> None:
    runner_cls = verl_main_ppo.TaskRunner
    if getattr(runner_cls, "_uenv_patch_task_runner_applied", False):
        return

    original_run = runner_cls.run

    def run(self, config):
        _apply_uenv_patches()
        return original_run(self, config)

    runner_cls.run = run
    runner_cls._uenv_patch_task_runner_applied = True


def main() -> None:
    _apply_uenv_patches()

    from verl.trainer import main_ppo

    _patch_task_runner(main_ppo)

    main_ppo.main()


if __name__ == "__main__":
    main()
