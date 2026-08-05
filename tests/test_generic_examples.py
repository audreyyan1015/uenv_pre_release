from __future__ import annotations

import importlib.util
import json
import os
from pathlib import Path
import stat
import subprocess
import tarfile
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]


def load_prepare_module():
    path = ROOT / "examples/training/prepare_episode_data.py"
    spec = importlib.util.spec_from_file_location("prepare_episode_data", path)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class GenericExampleTests(unittest.TestCase):
    @staticmethod
    def _write_fake_evaluator(venv: Path) -> Path:
        binary = venv / "bin/uenv-evaluate"
        binary.parent.mkdir(parents=True, exist_ok=True)
        binary.write_text(
            "#!/usr/bin/env bash\nset -euo pipefail\nprintf '%s\\n' \"$@\"\n",
            encoding="utf-8",
        )
        binary.chmod(0o755)
        return binary

    def test_qa_rows_carry_explicit_environment(self) -> None:
        module = load_prepare_module()
        rows = module.load_rows(
            ROOT / "examples/training/qa-gsm8k.jsonl",
            dataset="gsm8k",
            env_type="qa",
            max_steps=1,
            limit=1,
        )
        self.assertEqual(rows[0]["ability"], "qa")
        self.assertEqual(rows[0]["extra_info"]["env_type"], "qa")
        self.assertEqual(rows[0]["extra_info"]["dataset"], "gsm8k")
        self.assertEqual(rows[0]["extra_info"]["max_steps"], 1)

    def test_training_converter_does_not_inject_dataset_specific_prompt(self) -> None:
        module = load_prepare_module()
        with tempfile.TemporaryDirectory() as temp_dir:
            source = Path(temp_dir) / "pubmedqa.jsonl"
            source.write_text(
                json.dumps(
                    {
                        "id": "medical-1",
                        "env_type": "qa",
                        "dataset": "pubmedqa",
                        "question": "Is the reported association significant?",
                        "target": "yes",
                        "max_steps": 1,
                    }
                )
                + "\n",
                encoding="utf-8",
            )
            rows = module.load_rows(
                source,
                dataset="pubmedqa",
                env_type="qa",
                max_steps=1,
            )
        self.assertEqual(
            rows[0]["prompt"],
            [{"role": "user", "content": "Is the reported association significant?"}],
        )
        self.assertNotIn("####", rows[0]["prompt"][0]["content"])

    def test_training_rows_can_route_a_custom_environment(self) -> None:
        module = load_prepare_module()
        with tempfile.TemporaryDirectory() as temp_dir:
            source = Path(temp_dir) / "custom.jsonl"
            source.write_text(
                json.dumps(
                    {
                        "id": "custom-1",
                        "env_type": "warehouse",
                        "dataset": "warehouse-v1",
                        "question": "Choose an action",
                        "env_config": {"map": "a"},
                        "reward_config": {"type": "plugin"},
                    }
                )
                + "\n",
                encoding="utf-8",
            )
            rows = module.load_rows(
                source,
                dataset="warehouse-v1",
                env_type="warehouse",
                max_steps=1,
            )
        self.assertEqual(rows[0]["ability"], "warehouse")
        self.assertEqual(rows[0]["extra_info"]["env_type"], "warehouse")
        self.assertEqual(rows[0]["extra_info"]["dataset"], "warehouse-v1")
        self.assertEqual(rows[0]["extra_info"]["max_steps"], 1)
        self.assertEqual(rows[0]["extra_info"]["env_config"], {"map": "a"})
        self.assertEqual(
            rows[0]["extra_info"]["reward_config"],
            {"type": "plugin"},
        )

    def test_training_entry_help(self) -> None:
        result = subprocess.run(
            ["bash", str(ROOT / "examples/training/train_verl.sh"), "--help"],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("run-task", result.stdout)
        self.assertIn("run-swe", result.stdout)
        self.assertIn("prepare-data", result.stdout)
        self.assertIn("prepare-swe-uenv", result.stdout)
        self.assertNotIn("quickstart", result.stdout.casefold())
        task_help = subprocess.run(
            [
                "bash",
                str(ROOT / "examples/training/train_verl.sh"),
                "run-task",
                "--help",
            ],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(task_help.returncode, 0, task_help.stderr)
        self.assertIn("run-task", task_help.stdout)

    def test_training_legacy_quickstarts_are_rejected(self) -> None:
        script = ROOT / "examples/training/train_verl.sh"
        for command in ("quickstart-env", "quickstart-qa", "quickstart-swe"):
            result = subprocess.run(
                ["bash", str(script), command],
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
            self.assertNotEqual(result.returncode, 0, command)
            self.assertIn(f"未知命令：{command}", result.stderr)

    def test_low_level_training_runner_does_not_default_task_or_scale(self) -> None:
        script = ROOT / "examples/training/verl_runner.sh"
        result = subprocess.run(
            ["bash", str(script), "run", "--model", "/tmp/model", "--data", "/tmp/data"],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("--env-type", result.stderr)

        help_result = subprocess.run(
            ["bash", str(script), "--help"],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(help_result.returncode, 0, help_result.stderr)
        self.assertIn("--env-type NAME", help_result.stdout)
        self.assertIn("--gpus N                 单节点 GPU 数（必填）", help_result.stdout)
        self.assertIn("--steps N                训练步数（必填）", help_result.stdout)

    def test_training_run_task_rejects_missing_explicit_inputs_before_setup(self) -> None:
        script = ROOT / "examples/training/train_verl.sh"
        required = {
            "--model": "/models/example",
            "--work-dir": "WORK_DIR",
            "--uenv-endpoint": "10.0.0.10:50051",
            "--env-type": "warehouse",
            "--dataset": "warehouse-v1",
            "--input": "/data/warehouse.jsonl",
            "--max-steps": "1",
            "--gpus": "1",
            "--steps": "2",
            "--rollouts": "4",
            "--train-batch-size": "8",
            "--runtime": "docker",
            "--image": "example.invalid/verl@sha256:deadbeef",
        }
        with tempfile.TemporaryDirectory() as temp_dir:
            work_dir = Path(temp_dir) / "must-not-be-created"
            values = {
                option: str(work_dir) if value == "WORK_DIR" else value
                for option, value in required.items()
            }
            for omitted in required:
                args = ["bash", str(script), "run-task"]
                for option, value in values.items():
                    if option != omitted:
                        args.extend([option, value])
                result = subprocess.run(
                    args,
                    text=True,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    check=False,
                )
                self.assertNotEqual(result.returncode, 0, omitted)
                self.assertIn(omitted, result.stderr, result.stderr)
                self.assertFalse(work_dir.exists(), omitted)

    def test_training_run_swe_requires_explicit_variant_before_setup(self) -> None:
        script = ROOT / "examples/training/train_verl.sh"
        with tempfile.TemporaryDirectory() as temp_dir:
            work_dir = Path(temp_dir) / "must-not-be-created"
            result = subprocess.run(
                [
                    "bash",
                    str(script),
                    "run-swe",
                    "--model",
                    "/models/example",
                    "--work-dir",
                    str(work_dir),
                    "--uenv-endpoint",
                    "10.0.0.10:50051",
                    "--catalog",
                    "/data/smith.json",
                    "--instance",
                    "owner__repo-1",
                    "--max-iterations",
                    "30",
                    "--gpus",
                    "1",
                    "--steps",
                    "1",
                    "--rollouts",
                    "2",
                    "--train-batch-size",
                    "1",
                    "--runtime",
                    "docker",
                    "--image",
                    "example.invalid/verl@sha256:deadbeef",
                ],
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("--benchmark-variant smith", result.stderr)
            self.assertFalse(work_dir.exists())

    def test_training_client_kit_entry_help(self) -> None:
        result = subprocess.run(
            ["bash", str(ROOT / "examples/training/create_client_kit.sh"), "--help"],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("GPU 训练客户端包", result.stdout)

    def test_hub_image_bundle_entry_help(self) -> None:
        script = ROOT / "examples/hub/image_bundle.sh"
        result = subprocess.run(
            ["bash", str(script), "--help"],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("离线容器镜像", result.stdout)
        self.assertIn(
            "runuser -u uenv -- podman load", script.read_text(encoding="utf-8")
        )
        self.assertIn(
            'manifest="$TARGET_DIR/envs/$PACKAGE/$VERSION/manifest.json"',
            script.read_text(encoding="utf-8"),
        )

    @unittest.skipUnless(
        hasattr(os, "geteuid") and os.geteuid() == 0,
        "image_bundle.sh intentionally requires root",
    )
    def test_hub_image_install_loads_cached_tar_into_correct_engine_store(self) -> None:
        script = ROOT / "examples/hub/image_bundle.sh"
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            bin_dir = root / "bin"
            bin_dir.mkdir()
            runuser_log = root / "runuser.log"
            docker_log = root / "docker.log"
            fake_uenv = bin_dir / "uenv"
            fake_uenv.write_text(
                """#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == version ]]; then
  echo 'uenv 0.1.2-trial'
  exit 0
fi
[[ "${1:-}" == env && "${2:-}" == sync ]]
package="$3"
shift 3
version=''
target=''
while (($#)); do
  case "$1" in
    --version) version="$2"; shift 2 ;;
    --target-dir) target="$2"; shift 2 ;;
    *) shift ;;
  esac
done
dir="$target/envs/$package/$version"
mkdir -p "$dir/images"
: > "$dir/images/example.tar"
cat > "$dir/manifest.json" <<JSON
{"artifacts":[{"kind":"image_tar","sync_mode":"inline","target_rel_path":"images/example.tar"}]}
JSON
""",
                encoding="utf-8",
            )
            fake_uenv.chmod(0o755)
            for name, body in {
                "podman": "#!/bin/sh\nexit 0\n",
                "docker": "#!/bin/sh\nprintf '%s\\n' \"$*\" > \"$UENV_TEST_DOCKER_LOG\"\n",
                "id": "#!/bin/sh\n[ \"${1:-}\" = uenv ]\n",
                "runuser": "#!/bin/sh\nprintf '%s\\n' \"$*\" > \"$UENV_TEST_RUNUSER_LOG\"\n",
            }.items():
                path = bin_dir / name
                path.write_text(body, encoding="utf-8")
                path.chmod(0o755)

            result = subprocess.run(
                [
                    "bash",
                    str(script),
                    "install",
                    "--package",
                    "example-images",
                    "--version",
                    "0.1.0",
                    "--engine",
                    "podman",
                    "--target-dir",
                    str(root / "target"),
                ],
                env={
                    **os.environ,
                    "PATH": f"{bin_dir}:/usr/bin:/bin",
                    "UENV_TEST_RUNUSER_LOG": str(runuser_log),
                    "UENV_TEST_DOCKER_LOG": str(docker_log),
                },
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
            invoked = (
                runuser_log.read_text(encoding="utf-8").strip()
                if runuser_log.is_file()
                else ""
            )
            docker_result = subprocess.run(
                [
                    "bash",
                    str(script),
                    "install",
                    "--package",
                    "example-images",
                    "--version",
                    "0.1.0",
                    "--engine",
                    "docker",
                    "--target-dir",
                    str(root / "target-docker"),
                ],
                env={
                    **os.environ,
                    "PATH": f"{bin_dir}:/usr/bin:/bin",
                    "UENV_TEST_RUNUSER_LOG": str(runuser_log),
                    "UENV_TEST_DOCKER_LOG": str(docker_log),
                },
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
            docker_invoked = (
                docker_log.read_text(encoding="utf-8").strip()
                if docker_log.is_file()
                else ""
            )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(invoked.startswith("-u uenv -- podman load -i "), invoked)
        self.assertIn("/envs/example-images/0.1.0/images/example.tar", invoked)
        self.assertEqual(docker_result.returncode, 0, docker_result.stderr)
        self.assertTrue(docker_invoked.startswith("load -i "), docker_invoked)
        self.assertIn(
            "/envs/example-images/0.1.0/images/example.tar", docker_invoked
        )

    def test_training_client_kit_contains_only_client_assets(self) -> None:
        required = [
            "VERSION",
            "manifest.json",
            "examples/training/train_verl.sh",
            "examples/training/verl_runner.sh",
            "examples/training/prepare_episode_data.py",
            "examples/training/qa-gsm8k.jsonl",
            "examples/training/code-dscodebench.jsonl",
            "examples/training/process-plugin.jsonl",
            "examples/training/README.md",
            "examples/swe/prepare_verl_data.py",
            "share/uenv-bridge/configs/uenv-agent-loop.yaml",
            "share/uenv-bridge/scripts/run_verl_main_ppo.py",
            "share/swe/smith-example.json",
        ]
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            release = root / "release"
            for relative in required:
                path = release / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("{}\n" if path.suffix == ".json" else "test\n")
            wheel = release / "wheels/uenv_bridge-0.1.0-py3-none-any.whl"
            wheel.parent.mkdir(parents=True)
            wheel.write_bytes(b"wheel")
            output = root / "client.tar.gz"
            result = subprocess.run(
                [
                    "bash",
                    str(ROOT / "examples/training/create_client_kit.sh"),
                    "--release",
                    str(release),
                    "--output",
                    str(output),
                ],
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            checksum = subprocess.run(
                ["sha256sum", "-c", output.name + ".sha256"],
                cwd=root,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
            self.assertEqual(checksum.returncode, 0, checksum.stderr)
            with tarfile.open(output, "r:gz") as archive:
                names = set(archive.getnames())
                readme_member = archive.extractfile("uenv-training-client/README.txt")
                self.assertIsNotNone(readme_member)
                client_readme = readme_member.read().decode("utf-8")
            helper = root / "install-training-client.sh"
            installed = root / "installed-client"
            unpack = subprocess.run(
                [
                    "bash",
                    str(helper),
                    "--archive",
                    str(output),
                    "--target",
                    str(installed),
                ],
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
            self.assertEqual(unpack.returncode, 0, unpack.stderr)
            self.assertTrue((installed / "examples/training/train_verl.sh").is_file())
            self.assertIn("run-task", unpack.stdout)
            self.assertNotIn("quickstart", unpack.stdout.casefold())
        self.assertIn("uenv-training-client/examples/training/train_verl.sh", names)
        self.assertIn("uenv-training-client/examples/training/README.md", names)
        self.assertIn(
            "uenv-training-client/wheels/uenv_bridge-0.1.0-py3-none-any.whl",
            names,
        )
        self.assertNotIn("uenv-training-client/bin/uenv-worker", names)
        self.assertIn("run-task", client_readme)
        self.assertNotIn("quickstart", client_readme.casefold())

    def test_evaluation_run_task_forwards_only_explicit_arguments(self) -> None:
        script = ROOT / "examples/evaluation/evaluate.sh"
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            venv = root / "venv"
            self._write_fake_evaluator(venv)
            env = os.environ.copy()
            env["UENV_EVAL_VENV"] = str(venv)

            expected = [
                "--endpoint",
                "10.0.0.10:50051",
                "--env-type",
                "warehouse",
                "--dataset",
                "warehouse-v1",
                "--input",
                "custom-cases.jsonl",
                "--output",
                "custom-results.jsonl",
                "--max-steps",
                "7",
                "--limit",
                "3",
            ]
            result = subprocess.run(
                [
                    "bash",
                    str(script),
                    "run-task",
                    *expected,
                ],
                cwd=root,
                env=env,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.splitlines(), expected)
        self.assertNotIn("qa", result.stdout.splitlines())
        self.assertNotIn("gsm8k", result.stdout.splitlines())

    def test_evaluation_run_task_requires_identity_and_files_before_setup(self) -> None:
        script = ROOT / "examples/evaluation/evaluate.sh"
        required = {
            "--endpoint": "10.0.0.10:50051",
            "--env-type": "warehouse",
            "--dataset": "warehouse-v1",
            "--input": "cases.jsonl",
            "--output": "results.jsonl",
            "--max-steps": "7",
        }
        explicitly_required = tuple(required)
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            marker = root / "setup-was-called"
            fake_setup = root / "setup.sh"
            fake_setup.write_text(
                "#!/usr/bin/env bash\ntouch \"$UENV_TEST_SETUP_MARKER\"\nexit 99\n",
                encoding="utf-8",
            )
            fake_setup.chmod(0o755)
            env = {
                **os.environ,
                "UENV_EVAL_VENV": str(root / "missing-venv"),
                "UENV_EVAL_SETUP_SCRIPT": str(fake_setup),
                "UENV_TEST_SETUP_MARKER": str(marker),
            }
            for omitted in explicitly_required:
                args = ["bash", str(script), "run-task"]
                for option, value in required.items():
                    if option != omitted:
                        args.extend([option, value])
                result = subprocess.run(
                    args,
                    cwd=root,
                    env=env,
                    text=True,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    check=False,
                )
                self.assertEqual(result.returncode, 2, omitted)
                self.assertIn(omitted, result.stderr, result.stderr)
                self.assertFalse(marker.exists(), omitted)

    def test_evaluation_legacy_presets_are_rejected_before_setup(self) -> None:
        script = ROOT / "examples/evaluation/evaluate.sh"
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            marker = root / "setup-was-called"
            fake_setup = root / "setup.sh"
            fake_setup.write_text(
                "#!/usr/bin/env bash\ntouch \"$UENV_TEST_SETUP_MARKER\"\nexit 99\n",
                encoding="utf-8",
            )
            fake_setup.chmod(0o755)
            env = {
                **os.environ,
                "UENV_EVAL_VENV": str(root / "missing-venv"),
                "UENV_EVAL_SETUP_SCRIPT": str(fake_setup),
                "UENV_TEST_SETUP_MARKER": str(marker),
            }
            for command in ("qa", "code", "raw"):
                result = subprocess.run(
                    ["bash", str(script), command],
                    cwd=root,
                    env=env,
                    text=True,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    check=False,
                )
                self.assertEqual(result.returncode, 2, command)
                self.assertIn(f"unknown evaluate command: {command}", result.stderr)
                self.assertFalse(marker.exists(), command)

    def test_evaluation_first_run_auto_setup_and_offline_forwarding(self) -> None:
        script = ROOT / "examples/evaluation/evaluate.sh"
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            venv = root / "venv"
            wheelhouse = root / "wheelhouse"
            wheelhouse.mkdir()
            setup_log = root / "setup.args"
            fake_setup = root / "setup.sh"
            fake_setup.write_text(
                """#!/usr/bin/env bash
set -euo pipefail
printf '%s\\n' "$@" > "$UENV_TEST_SETUP_LOG"
venv=''
while (($#)); do
  case "$1" in
    --venv) venv="$2"; shift 2 ;;
    --wheelhouse) shift 2 ;;
    --offline) shift ;;
    *) exit 2 ;;
  esac
done
mkdir -p "$venv/bin"
printf '#!/usr/bin/env bash\\nprintf "%%s\\\\n" "$@"\\n' > "$venv/bin/uenv-evaluate"
chmod 0755 "$venv/bin/uenv-evaluate"
""",
                encoding="utf-8",
            )
            fake_setup.chmod(0o755)
            env = os.environ.copy()
            env.update(
                {
                    "UENV_EVAL_VENV": str(venv),
                    "UENV_EVAL_SETUP_SCRIPT": str(fake_setup),
                    "UENV_EVAL_WHEELHOUSE": str(wheelhouse),
                    "UENV_TEST_SETUP_LOG": str(setup_log),
                }
            )
            result = subprocess.run(
                [
                    "bash",
                    str(script),
                    "run-task",
                    "--endpoint",
                    "10.0.0.10:50051",
                    "--env-type",
                    "warehouse",
                    "--dataset",
                    "warehouse-v1",
                    "--input",
                    "cases.jsonl",
                    "--output",
                    "results.jsonl",
                    "--max-steps",
                    "7",
                    "--offline",
                    "--limit",
                    "1",
                ],
                cwd=root,
                env=env,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
            setup_args = setup_log.read_text(encoding="utf-8").splitlines()

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("--offline", setup_args)
        self.assertIn("--wheelhouse", setup_args)
        self.assertNotIn("--offline", result.stdout.splitlines())
        self.assertIn("--limit", result.stdout.splitlines())

    def test_evaluation_can_disable_auto_setup(self) -> None:
        script = ROOT / "examples/evaluation/evaluate.sh"
        with tempfile.TemporaryDirectory() as temp_dir:
            env = os.environ.copy()
            env["UENV_EVAL_VENV"] = str(Path(temp_dir) / "missing")
            result = subprocess.run(
                [
                    "bash",
                    str(script),
                    "run-task",
                    "--endpoint",
                    "10.0.0.10:50051",
                    "--env-type",
                    "warehouse",
                    "--dataset",
                    "warehouse-v1",
                    "--input",
                    "cases.jsonl",
                    "--output",
                    "results.jsonl",
                    "--max-steps",
                    "7",
                    "--no-auto-setup",
                ],
                env=env,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("automatic setup is disabled", result.stderr)
        self.assertIn("wheelhouse", result.stderr)

    def test_evaluation_service_commands_skip_generic_python_setup(self) -> None:
        script = ROOT / "examples/evaluation/evaluate.sh"
        commands = {
            "configure-model": "configure_model.sh",
            "prepare-swe": "prepare-swe",
        }
        with tempfile.TemporaryDirectory() as temp_dir:
            env = os.environ.copy()
            env.update(
                {
                    "UENV_EVAL_VENV": str(Path(temp_dir) / "must-not-be-created"),
                    "UENV_EVAL_AUTO_SETUP": "0",
                }
            )
            results = {
                command: subprocess.run(
                    ["bash", str(script), command, "--help"],
                    env=env,
                    text=True,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    check=False,
                )
                for command in commands
            }
        for command, result in results.items():
            self.assertEqual(result.returncode, 0, f"{command}: {result.stderr}")
            self.assertIn(commands[command], result.stdout)
            self.assertNotIn("automatic setup is disabled", result.stderr)

    def test_evaluation_run_swe_requires_explicit_case_selection(self) -> None:
        script = ROOT / "examples/evaluation/evaluate.sh"
        required = {
            "--provider": "local",
            "--model": "local-model",
            "--base-url": "http://127.0.0.1:8000/v1",
            "--gateway": "http://10.0.0.10:28999",
            "--catalog": "/data/verified.json",
            "--benchmark-variant": "verified",
            "--instance": "owner__repo-1",
            "--output-dir": "/results/swe",
            "--max-iterations": "30",
        }
        explicitly_selected = (
            "--provider",
            "--catalog",
            "--benchmark-variant",
            "--instance",
            "--output-dir",
            "--max-iterations",
        )
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            marker = root / "swe-was-called"
            fake_swe = root / "swe-evaluate.sh"
            fake_swe.write_text(
                "#!/usr/bin/env bash\ntouch \"$UENV_TEST_SWE_MARKER\"\nprintf '%s\\n' \"$@\"\n",
                encoding="utf-8",
            )
            fake_swe.chmod(0o755)
            env = {
                **os.environ,
                "UENV_SWE_EVALUATE_SCRIPT": str(fake_swe),
                "UENV_TEST_SWE_MARKER": str(marker),
                "UENV_EVAL_VENV": str(root / "must-not-be-created"),
                "UENV_EVAL_AUTO_SETUP": "0",
            }
            for omitted in explicitly_selected:
                args = ["bash", str(script), "run-swe"]
                for option, value in required.items():
                    if option != omitted:
                        args.extend([option, value])
                result = subprocess.run(
                    args,
                    env=env,
                    text=True,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    check=False,
                )
                self.assertEqual(result.returncode, 2, omitted)
                self.assertIn(omitted, result.stderr, result.stderr)
                self.assertFalse(marker.exists(), omitted)

            expected = [
                "local",
                "--model",
                "local-model",
                "--base-url",
                "http://127.0.0.1:8000/v1",
                "--gateway",
                "http://10.0.0.10:28999",
                "--catalog",
                "/data/verified.json",
                "--benchmark-variant",
                "verified",
                "--instance",
                "owner__repo-1",
                "--output-dir",
                "/results/swe",
                "--max-iterations",
                "30",
            ]
            complete = subprocess.run(
                [
                    "bash",
                    str(script),
                    "run-swe",
                    *[
                        item
                        for option, value in required.items()
                        for item in (option, value)
                    ],
                ],
                env=env,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
            was_called = marker.is_file()

        self.assertEqual(complete.returncode, 0, complete.stderr)
        self.assertTrue(was_called)
        self.assertEqual(complete.stdout.splitlines(), expected)

    def test_prepare_swe_runs_installer_and_openhands_once(self) -> None:
        script = ROOT / "examples/evaluation/prepare_swe.sh"
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            bundle = root / "uenv-linux-x86_64.tar.gz"
            bundle.write_bytes(b"test bundle")
            installer_log = root / "installer.args"
            openhands_log = root / "openhands.args"
            installer = root / "install.sh"
            installer.write_text(
                "#!/usr/bin/env bash\nprintf '%s\\n' \"$@\" > \"$INSTALLER_LOG\"\n",
                encoding="utf-8",
            )
            openhands = root / "install_openhands.sh"
            openhands.write_text(
                "#!/usr/bin/env bash\nprintf 'called\\n' >> \"$OPENHANDS_LOG\"\n",
                encoding="utf-8",
            )
            env = os.environ.copy()
            env.update(
                {
                    "UENV_OPENHANDS_INSTALLER": str(openhands),
                    "INSTALLER_LOG": str(installer_log),
                    "OPENHANDS_LOG": str(openhands_log),
                }
            )
            result = subprocess.run(
                [
                    "bash",
                    str(script),
                    "--installer",
                    str(installer),
                    "--bundle",
                    str(bundle),
                    "--profile",
                    "full",
                    "--runtime",
                    "podman",
                    "--image-policy",
                    "allow_public",
                    "--gateway",
                    "0.0.0.0:28999",
                    "--gateway-public",
                    "http://10.0.0.20:28999",
                    "--force-swe-config",
                ],
                env=env,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
            installer_args = installer_log.read_text(encoding="utf-8").splitlines()
            openhands_args = openhands_log.read_text(encoding="utf-8").splitlines()

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            installer_args,
            [
                "--bundle",
                str(bundle),
                "--profile",
                "full",
                "--enable-swe",
                "--swe-runtime",
                "podman",
                "--swe-image-policy",
                "allow_public",
                "--swe-gateway",
                "0.0.0.0:28999",
                "--swe-gateway-public",
                "http://10.0.0.20:28999",
                "--force-swe-config",
            ],
        )
        self.assertEqual(openhands_args, ["called"])

    def test_prepare_swe_rejects_unsupported_profile_before_install(self) -> None:
        script = ROOT / "examples/evaluation/prepare_swe.sh"
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            bundle = root / "bundle.tar.gz"
            bundle.write_bytes(b"test")
            installer = root / "install.sh"
            installer.write_text("exit 99\n", encoding="utf-8")
            result = subprocess.run(
                [
                    "bash",
                    str(script),
                    "--installer",
                    str(installer),
                    "--bundle",
                    str(bundle),
                    "--profile",
                    "worker",
                    "--runtime",
                    "podman",
                    "--image-policy",
                    "local_only",
                    "--gateway",
                    "127.0.0.1:28999",
                ],
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
        self.assertEqual(result.returncode, 1)
        self.assertIn("single-node 或 full", result.stderr)

    def test_swe_volcengine_noninteractive_key_error_is_actionable(self) -> None:
        script = ROOT / "examples/swe/evaluate.sh"
        env = os.environ.copy()
        env.pop("ARK_API_KEY", None)
        result = subprocess.run(
            [
                "bash",
                str(script),
                "volcengine",
                "--model",
                "ep-test",
                "--gateway",
                "http://127.0.0.1:28999",
                "--catalog",
                "/data/verified.json",
                "--benchmark-variant",
                "verified",
                "--instance",
                "owner__repo-1",
                "--output-dir",
                "/tmp/uenv-test-results",
                "--max-iterations",
                "30",
            ],
            env=env,
            stdin=subprocess.DEVNULL,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("非交互运行需要设置 ARK_API_KEY", result.stderr)
        self.assertIn("隐藏地提示输入", result.stderr)

    def test_evaluation_offline_setup_requires_wheelhouse(self) -> None:
        script = ROOT / "examples/evaluation/setup.sh"
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            wheel = root / "uenv_bridge-test.whl"
            wheel.write_bytes(b"not needed: validation stops before installation")
            result = subprocess.run(
                ["bash", str(script), "--offline", "--wheel", str(wheel)],
                env={**os.environ, "UENV_EVAL_VENV": str(root / "venv")},
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("offline setup requires --wheelhouse", result.stderr)

    @unittest.skipUnless(
        hasattr(os, "geteuid") and os.geteuid() == 0,
        "configure_model.sh intentionally requires root",
    )
    def test_configure_model_writes_secret_once_and_restarts_on_change(self) -> None:
        script = ROOT / "examples/evaluation/configure_model.sh"
        secret = "test-key-that-must-not-be-printed"
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            target = root / "secrets/worker-llm.env"
            key_file = root / "api-key"
            key_file.write_text(secret + "\n", encoding="utf-8")
            systemctl_log = root / "systemctl.log"
            fake_systemctl = root / "systemctl"
            fake_systemctl.write_text(
                "#!/usr/bin/env bash\nprintf '%s\\n' \"$*\" >> \"$UENV_TEST_SYSTEMCTL_LOG\"\n",
                encoding="utf-8",
            )
            fake_systemctl.chmod(0o755)
            env = os.environ.copy()
            env.update(
                {
                    "UENV_WORKER_LLM_ENV_FILE": str(target),
                    "UENV_WORKER_USER": "root",
                    "UENV_WORKER_SERVICE": "test-worker.service",
                    "UENV_SYSTEMCTL": str(fake_systemctl),
                    "UENV_TEST_SYSTEMCTL_LOG": str(systemctl_log),
                }
            )
            command = [
                "bash",
                str(script),
                "--endpoint",
                "https://models.example.test/v1",
                "--model",
                "model-1",
                "--api-key-file",
                str(key_file),
            ]
            first = subprocess.run(
                command,
                env=env,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
            # Same bytes with a stale permissive mode must be repaired without
            # causing a needless second Worker restart.
            target.chmod(0o666)
            second = subprocess.run(
                command,
                env=env,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )

            content = target.read_text(encoding="utf-8")
            mode = stat.S_IMODE(target.stat().st_mode)
            directory_mode = stat.S_IMODE(target.parent.stat().st_mode)
            directory_owner = target.parent.stat().st_uid
            restarts = systemctl_log.read_text(encoding="utf-8").splitlines()

        self.assertEqual(first.returncode, 0, first.stderr)
        self.assertEqual(second.returncode, 0, second.stderr)
        self.assertEqual(mode, 0o640)
        self.assertEqual(directory_mode, 0o750)
        self.assertEqual(directory_owner, 0)
        self.assertIn('UENV_LLM_ENDPOINT="https://models.example.test/v1"', content)
        self.assertIn('UENV_LLM_MODEL_NAME="model-1"', content)
        self.assertIn(secret, content)
        self.assertNotIn(secret, first.stdout + first.stderr + second.stdout + second.stderr)
        self.assertEqual(restarts, ["restart test-worker.service"])
        self.assertIn("unchanged", second.stdout)


if __name__ == "__main__":
    unittest.main()
