from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = ROOT / "libexec/uenv/environment/plugin.sh"
TEMPLATE = ROOT / "templates/process-plugin"


class ProcessPluginWorkflowTest(unittest.TestCase):
    def fake_bootstrap_python(self, root: Path) -> Path:
        """Avoid the runtime's heavyweight venv hydration in offline unit tests."""

        path = root / "bootstrap-python"
        path.write_text(
            "#!/bin/sh\n"
            'if [ "$1" = "-m" ] && [ "$2" = "venv" ]; then\n'
            '  mkdir -p "$3/bin"\n'
            f'  ln -s "{sys.executable}" "$3/bin/python"\n'
            "  exit 0\n"
            "fi\n"
            f'exec "{sys.executable}" "$@"\n',
            encoding="utf-8",
        )
        path.chmod(0o755)
        return path

    def run_workflow(
        self,
        *args: str,
        env: dict[str, str] | None = None,
        check: bool = True,
    ) -> subprocess.CompletedProcess[str]:
        command_env = os.environ.copy()
        if env:
            command_env.update(env)
        return subprocess.run(
            ["bash", str(WORKFLOW), *args],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=command_env,
            check=check,
        )

    def test_template_keeps_task_logic_out_of_transport_adapter(self) -> None:
        adapter = (TEMPLATE / "plugin.py").read_text(encoding="utf-8")
        logic = (TEMPLATE / "environment.py").read_text(encoding="utf-8")
        self.assertIn("DO NOT put task logic", adapter)
        self.assertIn("from environment import Environment", adapter)
        self.assertNotIn("expected_action =", adapter)
        for method in ("def reset(", "def step(", "def reward("):
            self.assertIn(method, logic)

    def test_create_logic_test_local_install_and_publish_are_offline_testable(self) -> None:
        with tempfile.TemporaryDirectory(prefix="uenv-plugin-workflow-") as directory:
            root = Path(directory)
            bootstrap = self.fake_bootstrap_python(root)
            bootstrap_env = {"UENV_PLUGIN_BOOTSTRAP_PYTHON": str(bootstrap)}
            plugin = root / "demo-env"
            result = self.run_workflow(
                "create",
                "demo-env",
                "--dataset",
                "demo-dataset",
                "--dir",
                str(plugin),
                "--version",
                "1.2.3",
                "--description",
                "Demo environment",
                env=bootstrap_env,
            )
            self.assertIn("environment.py", result.stdout)
            manifest = (plugin / "manifest.yaml").read_text(encoding="utf-8")
            self.assertIn('env_type: "demo-env"', manifest)
            self.assertIn('version: "1.2.3"', manifest)
            example = json.loads((plugin / "example.jsonl").read_text(encoding="utf-8"))
            self.assertEqual(example["env_type"], "demo-env")
            self.assertEqual(example["dataset"], "demo-dataset")

            self.run_workflow("test", str(plugin), "--logic-only", env=bootstrap_env)

            # Use an empty dependency set so the automation itself can be
            # exercised without network or prebuilt third-party wheels.
            (plugin / "requirements.txt").write_text("# no dependencies in this test\n")
            if os.geteuid() == 0:
                # GNU tar preserves source ownership when invoked by root.
                # Simulate a plugin edited by an ordinary user and verify the
                # installed immutable copy is normalized back to root. Some
                # user-namespace filesystems reject unmapped IDs, so the core
                # permission assertions below remain authoritative there.
                try:
                    os.chown(plugin / "environment.py", 65534, 65534)
                except OSError:
                    pass
            plugin_root = root / "active"
            store_root = root / "store"
            self.run_workflow(
                "install-local",
                str(plugin),
                "--offline",
                "--skip-test",
                "--no-restart",
                "--plugin-root",
                str(plugin_root),
                "--store-root",
                str(store_root),
                env=bootstrap_env,
            )
            active = plugin_root / "demo-env"
            self.assertTrue(active.is_symlink())
            self.assertTrue((active.resolve() / "environment.py").is_file())
            self.assertTrue((active.resolve() / ".venv/bin/python").is_file())
            installed_logic = active.resolve() / "environment.py"
            self.assertEqual(installed_logic.stat().st_uid, os.geteuid())
            self.assertEqual(installed_logic.stat().st_mode & 0o022, 0)

            log = root / "uenv-arguments.txt"
            fake_cli = root / "fake-uenv"
            fake_cli.write_text(
                '#!/bin/sh\nprintf "%s\\n" "$@" > "$FAKE_UENV_LOG"\n',
                encoding="utf-8",
            )
            fake_cli.chmod(0o755)
            self.run_workflow(
                "publish",
                str(plugin),
                "--offline",
                "--skip-test",
                "--worker-min",
                "0.1.2-trial",
                env={
                    **bootstrap_env,
                    "UENV_CLI": str(fake_cli),
                    "FAKE_UENV_LOG": str(log),
                },
            )
            arguments = log.read_text(encoding="utf-8").splitlines()
            self.assertEqual(arguments[:2], ["env", "publish-plugin"])
            self.assertIn("demo-env", arguments)
            self.assertIn("1.2.3", arguments)
            self.assertIn("0.1.2-trial", arguments)

    def test_offline_publish_refuses_an_absent_wheelhouse_before_pip(self) -> None:
        with tempfile.TemporaryDirectory(prefix="uenv-plugin-offline-") as directory:
            root = Path(directory)
            bootstrap = self.fake_bootstrap_python(root)
            bootstrap_env = {"UENV_PLUGIN_BOOTSTRAP_PYTHON": str(bootstrap)}
            plugin = root / "offline-env"
            self.run_workflow(
                "create",
                "offline-env",
                "--dataset",
                "offline-dataset",
                "--dir",
                str(plugin),
                env=bootstrap_env,
            )
            result = self.run_workflow(
                "publish",
                str(plugin),
                "--offline",
                "--skip-test",
                env=bootstrap_env,
                check=False,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("wheelhouse", result.stderr)


if __name__ == "__main__":
    unittest.main()
