"""Unit tests for workspace probe and gateway tool patch helpers."""

import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(__file__)))

from uenv_runtime.workspace_utils import is_uenv_gateway_workspace  # noqa: E402
from uenv_runtime.workspace_probe import validate_workspace_probe  # noqa: E402


class FakeUEnvWs:
    uenv_gateway_workspace = True
    gateway_url = "http://127.0.0.1:28097"
    instance_id = "instance_test"
    working_dir = "/app"


class WorkspaceProbeTests(unittest.TestCase):
    def test_openlibrary_cross_repo_fails(self):
        probe = {
            "exit_code": 0,
            "stdout": "openlibrary/plugins/worksearch/code.py\n",
            "combined": "openlibrary/plugins/worksearch/code.py",
        }
        ok, reason = validate_workspace_probe(
            probe,
            instance_id="instance_qutebrowser__x",
            repo="qutebrowser/qutebrowser",
        )
        self.assertFalse(ok)
        self.assertIn("openlibrary", reason)

    def test_qutebrowser_probe_ok(self):
        probe = {
            "exit_code": 0,
            "combined": "/app\norigin https://github.com/qutebrowser/qutebrowser.git\nqutebrowser/",
        }
        ok, reason = validate_workspace_probe(
            probe,
            instance_id="instance_qutebrowser__x",
            repo="qutebrowser/qutebrowser",
        )
        self.assertTrue(ok)
        self.assertEqual(reason, "ok")


class GatewayPatchDetectionTests(unittest.TestCase):
    def test_is_uenv_gateway_workspace_marker(self):
        self.assertTrue(is_uenv_gateway_workspace(FakeUEnvWs()))

    def test_is_uenv_gateway_workspace_by_gateway_url(self):
        class Ws:
            gateway_url = "http://h:1"

        self.assertTrue(is_uenv_gateway_workspace(Ws()))

    def test_is_uenv_gateway_workspace_negative(self):
        class Ws:
            working_dir = "/app"

        self.assertFalse(is_uenv_gateway_workspace(Ws()))


if __name__ == "__main__":
    unittest.main()
