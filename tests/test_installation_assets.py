from __future__ import annotations

import http.server
import json
import os
from pathlib import Path
import subprocess
import tempfile
import threading
import unittest


ROOT = Path(__file__).resolve().parents[1]


class StatusHandler(http.server.BaseHTTPRequestHandler):
    def do_GET(self) -> None:  # noqa: N802
        if self.path == "/status":
            payload = {
                "server_epoch": 7,
                "worker_count": 1,
                "active_episodes": 0,
                "total_capacity": 4,
                "pending_results": 0,
                "workers": [
                    {
                        "worker_id": "test-worker",
                        "status": "ready",
                        "load": 0,
                        "capacity": 4,
                        "last_heartbeat_secs": 1,
                        "endpoint": "127.0.0.1:50054",
                    }
                ],
            }
            body = json.dumps(payload).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        self.send_error(404)

    def log_message(self, _format: str, *_args: object) -> None:
        return


class InstallationAssetsTest(unittest.TestCase):
    def test_shell_scripts_parse(self) -> None:
        scripts = [
            ROOT / "install.sh",
            ROOT / "scripts/build-release.sh",
            ROOT / "scripts/uenv-train",
        ]
        scripts.extend(sorted((ROOT / "libexec/uenv").rglob("*.sh")))
        scripts.extend(sorted((ROOT / "tools").rglob("*.sh")))
        for path in scripts:
            subprocess.run(["bash", "-n", str(path)], check=True)

    def test_unified_cli_reads_version(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "VERSION").write_text("0.1.0\n", encoding="utf-8")
            result = subprocess.run(
                ["python3", str(ROOT / "uenv"), "--install-root", str(root), "version"],
                text=True,
                stdout=subprocess.PIPE,
                check=True,
            )
        self.assertEqual(result.stdout.strip(), "uenv 0.1.0")

    def test_unified_cli_forwards_training_arguments(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            script = root / "bin/uenv-train"
            script.parent.mkdir(parents=True)
            script.write_text('printf "train:%s\\n" "$*"\n', encoding="utf-8")
            forwarded = [
                "run-task",
                "--env-type",
                "warehouse",
                "--dataset",
                "warehouse-v1",
                "--input",
                "/data/cases.jsonl",
                "--max-steps",
                "1",
            ]
            result = subprocess.run(
                [
                    "python3",
                    str(ROOT / "uenv"),
                    "--install-root",
                    str(root),
                    "train",
                    *forwarded,
                ],
                text=True,
                stdout=subprocess.PIPE,
                check=True,
            )
        self.assertEqual(result.stdout.strip(), f"train:{' '.join(forwarded)}")

    def test_unified_cli_routes_plugin_workflow_without_hub_cli(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            script = root / "libexec/uenv/environment/plugin.sh"
            script.parent.mkdir(parents=True)
            script.write_text('printf "plugin:%s\\n" "$*"\n', encoding="utf-8")
            forwarded = ["create", "warehouse", "--dataset", "warehouse-v1"]
            result = subprocess.run(
                [
                    "python3",
                    str(ROOT / "uenv"),
                    "--install-root",
                    str(root),
                    "env",
                    "plugin",
                    *forwarded,
                ],
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), f"plugin:{' '.join(forwarded)}")

    def test_unified_cli_lists_local_environments(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            plugin = root / "plugins" / "example"
            plugin.mkdir(parents=True)
            (plugin / "manifest.yaml").write_text(
                "env_type: example\nversion: '1.2.3'\ndatasets:\n  - demo-set\n",
                encoding="utf-8",
            )
            result = subprocess.run(
                [
                    "python3",
                    str(ROOT / "uenv"),
                    "--install-root",
                    str(root),
                    "--config-dir",
                    str(root / "config"),
                    "environments",
                    "--json",
                ],
                text=True,
                stdout=subprocess.PIPE,
                check=True,
            )
        items = json.loads(result.stdout)
        self.assertEqual(items[0]["env_type"], "example")
        self.assertEqual(items[0]["datasets"], ["demo-set"])
        self.assertEqual(items[0]["status"], "ready")

    def test_status_uses_admin_http_api(self) -> None:
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), StatusHandler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            result = subprocess.run(
                [
                    "python3",
                    str(ROOT / "uenv"),
                    "--admin-url",
                    f"http://127.0.0.1:{server.server_port}",
                    "status",
                ],
                text=True,
                stdout=subprocess.PIPE,
                check=True,
            )
        finally:
            server.shutdown()
            server.server_close()
        self.assertIn("Worker=1", result.stdout)
        self.assertIn("test-worker", result.stdout)

    def test_worker_template_has_no_port_collision(self) -> None:
        server = (ROOT / "deploy/config/server.yaml").read_text(encoding="utf-8")
        worker = (ROOT / "deploy/config/worker.yaml").read_text(encoding="utf-8")
        self.assertIn("admin_http_port: 50052", server)
        self.assertIn('listen: "0.0.0.0:50054"', worker)
        self.assertNotIn('listen: "0.0.0.0:50052"', worker)

    def test_hub_plugin_and_token_paths_are_installed(self) -> None:
        installer = (ROOT / "install.sh").read_text(encoding="utf-8")
        worker = (ROOT / "deploy/config/worker.yaml").read_text(encoding="utf-8")
        self.assertIn("--hub-token-file", installer)
        self.assertIn("/var/lib/uenv/plugins", installer)
        self.assertIn("/var/lib/uenv/hub/import", installer)
        self.assertIn('token_file: "@HUB_TOKEN_FILE@"', worker)
        self.assertIn('package_plugin_dir: "/var/lib/uenv/plugins"', worker)
        hub = (ROOT / "deploy/config/hub.toml").read_text(encoding="utf-8")
        self.assertIn('import_dir = "/var/lib/uenv/hub/import"', hub)

    def test_worker_model_secret_stays_root_owned_across_install(self) -> None:
        installer = (ROOT / "install.sh").read_text(encoding="utf-8")
        self.assertIn(
            "install -d -o root -g uenv -m 0750 /etc/uenv/secrets", installer
        )
        self.assertIn(
            "install -o root -g uenv -m 0640 /dev/null /etc/uenv/secrets/worker-llm.env",
            installer,
        )
        self.assertIn(
            "chown root:uenv /etc/uenv/secrets/worker-llm.env", installer
        )
        self.assertIn(
            "chmod 0640 /etc/uenv/secrets/worker-llm.env", installer
        )

    def test_release_script_packages_every_service_binary(self) -> None:
        script = (ROOT / "scripts/build-release.sh").read_text(encoding="utf-8")
        for name in ("uenv-adapter-core", "uenv-worker", "uenv-hub-server", "uenv-hub-cli"):
            self.assertIn(name, script)

    def test_clean_checkout_contains_runtime_env_templates(self) -> None:
        for name in ("server.env.example", "worker.env.example"):
            path = ROOT / "deploy/config" / name
            self.assertTrue(path.is_file(), name)
            self.assertTrue(path.read_text(encoding="utf-8").strip(), name)
        release = (ROOT / "scripts/build-release.sh").read_text(encoding="utf-8")
        self.assertIn('server.env.example" "$PAYLOAD/config/server.env', release)
        self.assertIn('worker.env.example" "$PAYLOAD/config/worker.env', release)
        worker_env = (ROOT / "deploy/config/worker.env.example").read_text(
            encoding="utf-8"
        )
        for name in (
            "UENV_QA_PLUGIN_BIN",
            "UENV_MATH_PLUGIN_BIN",
            "UENV_CODE_PLUGIN_BIN",
            "UENV_CODE_EVAL_SCRIPT",
        ):
            self.assertIn(name, worker_env)

    def test_swe_release_assets_are_wired(self) -> None:
        installer = (ROOT / "install.sh").read_text(encoding="utf-8")
        self.assertIn("/var/lib/uenv/evaluation-runs", installer)
        release = (ROOT / "scripts/build-release.sh").read_text(encoding="utf-8")
        worker_unit = (ROOT / "deploy/systemd/uenv-worker.service").read_text(
            encoding="utf-8"
        )
        for token in ("--enable-swe", "UENV_RUNTIME_GATEWAY_ENABLED", "UENV_SWE_INSTANCES"):
            self.assertIn(token, installer)
        for token in ("verified.json", "openhands-runner.py", "libexec/uenv/swe"):
            self.assertIn(token, release)
        for relative in (
            "libexec/uenv/swe/evaluate.sh",
            "libexec/uenv/swe/evaluate_batch.py",
            "libexec/uenv/swe/evaluate_one.sh",
            "tools/swe/build_catalog.py",
        ):
            self.assertTrue((ROOT / relative).is_file(), relative)
        self.assertIn("/etc/uenv/swe.env", worker_unit)
        self.assertTrue((ROOT / "deploy/systemd/uenv-swe-agent.service").is_file())

    def test_split_swe_uses_shared_key_without_putting_it_in_process_argv(self) -> None:
        installer = (ROOT / "install.sh").read_text(encoding="utf-8")
        agent_unit = (ROOT / "deploy/systemd/uenv-swe-agent.service").read_text(
            encoding="utf-8"
        )
        training_runner = (
            ROOT / "libexec/uenv/training/verl_runner.sh"
        ).read_text(encoding="utf-8")
        openhands_runner = (
            ROOT / "scripts/openhands/openhands_runner.py"
        ).read_text(encoding="utf-8")
        evaluate_one = (
            ROOT / "libexec/uenv/swe/evaluate_one.sh"
        ).read_text(encoding="utf-8")
        self.assertIn("--swe-shared-key-file", installer)
        self.assertIn("--swe-trajectory-endpoint", installer)
        self.assertIn('python3 - "$SWE_GATEWAY_BIND" <<\'PY\'', installer)
        self.assertNotIn('python3 - "$SWE_GATEWAY_BIND" "$SWE_HEALTH_KEY"', installer)
        self.assertNotIn("Requires=uenv-adapter-core.service", agent_unit)
        self.assertNotIn("Requires=uenv-worker.service", agent_unit)
        self.assertNotIn("/etc/uenv/secrets/swe.env", agent_unit)
        self.assertIn("UENV_SWE_AGENT_HEALTH_URL", training_runner)
        self.assertIn('document.get("registered") is True', training_runner)
        self.assertIn('"registered": registered', openhands_runner)
        self.assertIn('UENV_TRAJECTORY_ENDPOINT="$TRAJECTORY_ENDPOINT"', evaluate_one)

    @unittest.skipUnless(
        hasattr(os, "geteuid") and os.geteuid() == 0,
        "install.sh intentionally requires root",
    )
    def test_installer_rejects_world_readable_shared_key_without_echoing_it(self) -> None:
        secret = "do-not-print-this-shared-gateway-key-1234567890"
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            key_file = root / "swe.key"
            key_file.write_text(secret + "\n", encoding="ascii")
            key_file.chmod(0o644)
            result = subprocess.run(
                [
                    "bash",
                    str(ROOT / "install.sh"),
                    "--bundle",
                    str(root / "missing-bundle.tar.gz"),
                    "--profile",
                    "control-plane",
                    "--enable-swe",
                    "--swe-shared-key-file",
                    str(key_file),
                ],
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
        combined = result.stdout + result.stderr
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("不能允许 group/other 读取", combined)
        self.assertNotIn(secret, combined)

    def test_public_user_guides_are_packaged(self) -> None:
        release = (ROOT / "scripts/build-release.sh").read_text(encoding="utf-8")
        guide_root = ROOT / "Docs/guide"
        canonical_pages = (
            "index.md",
            "concepts/architecture.md",
            "concepts/episode-lifecycle.md",
            "deployment/single-node.md",
            "deployment/multi-node.md",
            "deployment/server.md",
            "deployment/worker-registration.md",
            "deployment/hub.md",
            "deployment/operations.md",
            "usage/README.md",
            "usage/evaluation.md",
            "usage/post-training.md",
            "usage/trajectory.md",
            "integration/README.md",
            "integration/contract.md",
            "integration/verl.md",
            "integration/custom-framework.md",
            "integration/support-matrix.md",
            "cases/README.md",
            "reference/glossary.md",
            "reference/ports.md",
            "reference/configuration.md",
            "reference/protocols.md",
            "reference/troubleshooting.md",
        )
        self.assertIn('cp -a "$ROOT/Docs/guide"', release)
        for old_name in (
            "UEnv基础部署指南.md",
            "UEnv多机部署指南.md",
            "UEnv Hub使用指南.md",
            "UEnv评测指南.md",
            "UEnv训练指南.md",
        ):
            self.assertNotIn(old_name, release)
        for relative in canonical_pages:
            path = guide_root / relative
            self.assertTrue(path.is_file(), relative)
            text = path.read_text(encoding="utf-8")
            self.assertNotIn("smoke test", text.casefold())
            self.assertNotIn("冒烟", text)
            self.assertNotIn("快速开始", text)
            self.assertNotIn("5分钟", text)
            self.assertNotIn("五分钟", text)

        for removed in (
            "deployment/adapter.md",
            "integration/openhands.md",
            "cases/trajectory-swe-openhands.md",
        ):
            self.assertFalse((guide_root / removed).exists(), removed)

        beginner_pages = (
            "index.md",
            "concepts/architecture.md",
            "concepts/episode-lifecycle.md",
            "usage/README.md",
            "usage/evaluation.md",
            "usage/post-training.md",
            "usage/trajectory.md",
            "cases/README.md",
        )
        for relative in beginner_pages:
            text = (guide_root / relative).read_text(encoding="utf-8")
            self.assertNotIn("UEnv Adapter", text, relative)
            self.assertNotIn("Adapter Core", text, relative)
            self.assertNotIn("Control Plane", text, relative)

        index = (guide_root / "index.md").read_text(encoding="utf-8")
        ordered_links = (
            "deployment/single-node.md",
            "deployment/multi-node.md",
            "usage/evaluation.md",
            "usage/post-training.md",
            "usage/trajectory.md",
        )
        positions = [index.index(link) for link in ordered_links]
        self.assertEqual(positions, sorted(positions))
        self.assertNotIn("二选一", index)

        case_titles = {
            "evaluation-gsm8k.md": "# 数学问答",
            "evaluation-code.md": "# 代码生成",
            "evaluation-swe-verified.md": "# 软件工程修复",
            "training-gsm8k-verl.md": "# 数学问答",
            "training-code-verl.md": "# 代码生成",
            "training-process-plugin.md": "# 自定义环境",
            "training-swe-smith-verl.md": "# 软件工程修复",
        }
        for name, title in case_titles.items():
            first_line = (guide_root / "cases" / name).read_text(
                encoding="utf-8"
            ).splitlines()[0]
            self.assertEqual(first_line, title, name)

        integration_text = "\n".join(
            path.read_text(encoding="utf-8")
            for path in sorted((guide_root / "integration").glob("*.md"))
        )
        self.assertNotIn("OpenHands", integration_text)

    def test_release_data_interfaces_are_loopback_by_default(self) -> None:
        server_env = (ROOT / "deploy/config/server.env.example").read_text(
            encoding="utf-8"
        )
        self.assertIn("UENV_OBS_HTTP_LISTEN=127.0.0.1:50053", server_env)
        self.assertIn("UENV_TRAJECTORY_HTTP_LISTEN=127.0.0.1:8077", server_env)
        self.assertNotIn("UENV_OBS_HTTP_LISTEN=0.0.0.0", server_env)
        self.assertNotIn("UENV_TRAJECTORY_HTTP_LISTEN=0.0.0.0", server_env)

        installer = (ROOT / "install.sh").read_text(encoding="utf-8")
        self.assertIn('SERVER_BIND="127.0.0.1:50051"', installer)
        self.assertIn('SERVER_BIND="0.0.0.0:50051"', installer)
        self.assertIn('"$PROFILE" == "single-node" || "$PROFILE" == "full"', installer)
        self.assertIn('"$TMP_DIR/server.env"', installer)

    def test_generic_examples_and_plugin_template_are_packaged(self) -> None:
        release = (ROOT / "scripts/build-release.sh").read_text(encoding="utf-8")
        for token in (
            "libexec/uenv/environment",
            "libexec/uenv/evaluation",
            "libexec/uenv/swe",
            "libexec/uenv/training",
            "examples/cases/evaluation",
            "examples/cases/training",
            "tools/hub",
            "tools/swe",
            "templates/process-plugin",
        ):
            self.assertIn(token, release)
        self.assertTrue((ROOT / "scripts/uenv-train").is_file())
        self.assertIn(
            "/usr/local/bin/uenv-train",
            (ROOT / "install.sh").read_text(encoding="utf-8"),
        )
        self.assertTrue((ROOT / "libexec/uenv/environment/plugin.sh").is_file())
        self.assertTrue((ROOT / "libexec/uenv/environment/README.md").is_file())
        self.assertTrue((ROOT / "examples/cases/evaluation/qa-gsm8k.jsonl").is_file())
        self.assertTrue((ROOT / "examples/cases/evaluation/README.md").is_file())
        self.assertTrue((ROOT / "examples/cases/evaluation/swe-verified.jsonl").is_file())
        self.assertTrue((ROOT / "examples/cases/training/qa-gsm8k.jsonl").is_file())
        self.assertTrue(
            (ROOT / "examples/cases/training/verl-grpo-overrides.conf").is_file()
        )
        self.assertTrue((ROOT / "examples/cases/training/README.md").is_file())
        self.assertTrue((ROOT / "tools/hub/image_bundle.sh").is_file())
        self.assertTrue((ROOT / "tools/swe/build_catalog.py").is_file())
        self.assertFalse(list((ROOT / "examples").rglob("*.sh")))
        self.assertFalse(list((ROOT / "examples").rglob("*.py")))
        for name in ("environment.py", "plugin.py", "uenv_plugin_api.py"):
            self.assertTrue((ROOT / "templates/process-plugin" / name).is_file())
        for excluded in (".venv", "wheelhouse", "__pycache__", "*.pyc"):
            self.assertIn(f"--exclude='{excluded}'", release)

    def test_release_directory_is_immutable_and_staged_atomically(self) -> None:
        installer = (ROOT / "install.sh").read_text(encoding="utf-8")
        self.assertIn(".bundle.sha256", installer)
        self.assertIn(".uenv-stage.", installer)
        self.assertIn('chmod 0755 "$RELEASE_STAGE"', installer)
        self.assertIn('mv -T "$RELEASE_STAGE" "$RELEASE_DIR"', installer)
        self.assertIn("不能覆盖现役 release", installer)


if __name__ == "__main__":
    unittest.main()
