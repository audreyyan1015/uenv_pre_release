from __future__ import annotations

import http.server
import json
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
        for path in (ROOT / "install.sh", ROOT / "scripts/build-release.sh"):
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

    def test_release_script_packages_every_service_binary(self) -> None:
        script = (ROOT / "scripts/build-release.sh").read_text(encoding="utf-8")
        for name in ("uenv-adapter-core", "uenv-worker", "uenv-hub-server", "uenv-hub-cli"):
            self.assertIn(name, script)


if __name__ == "__main__":
    unittest.main()
