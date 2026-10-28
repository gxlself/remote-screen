from __future__ import annotations

import contextlib
import hashlib
import io
import json
import os
import shutil
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from benchlib.adapters import load
from benchlib.adapters.base import BenchError, Ctx
from benchlib.adapters import rustdesk
from benchlib.executor import CmdResult, FakeExecutor


class HostExecutor(FakeExecutor):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.puts = []

    def put(self, local: Path, remote: str, **kwargs) -> None:
        del kwargs
        self.puts.append((Path(local), remote))


class RustDeskAdapterTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.repo = self.root / "repo"
        (self.repo / "tools/bench").mkdir(parents=True)
        self.win_data = b"fake msi"
        self.mac_data = b"fake dmg"
        manifest = {
            "schema": 1,
            "items": {
                "rustdesk-win": {
                    "file": "rustdesk-win.msi",
                    "url": "https://hs.example.com/rustdesk-win.msi",
                    "sha256": hashlib.sha256(self.win_data).hexdigest(),
                },
                "rustdesk-mac": {
                    "file": "rustdesk-mac.dmg",
                    "url": "https://hs.example.com/rustdesk-mac.dmg",
                    "sha256": hashlib.sha256(self.mac_data).hexdigest(),
                },
            },
        }
        (self.repo / "tools/bench/thirdparty.json").write_text(json.dumps(manifest), encoding="utf-8")
        self.lab = {
            "host": {"bench_dir": "C:\\rs-bench", "addr": "192.168.31.20"},
            "controller": {"addr": "192.168.31.30"},
        }
        self.host = HostExecutor(default=self._host_reply)
        self.ctl = FakeExecutor(rules=[(("curl",), self._curl), (("hdiutil", "attach"), CmdResult(0, "", "")),
                                       (("hdiutil", "detach"), CmdResult(0, "", "")),
                                       (("ditto",), self._ditto)], default=CmdResult(0, "", ""))
        self.ctx = Ctx(
            lab=self.lab,
            host=self.host,
            ctl=self.ctl,
            repo=self.repo,
            state_dir=self.root / "state",
            allow_install=True,
            log_path=self.root / "run.log",
            sleep=lambda seconds: None,
        )
        self.adapter = rustdesk.create()

    def _host_reply(self, argv, input):
        del input
        action = argv[argv.index("-Action") + 1] if "-Action" in argv else ""
        if action == "status":
            return CmdResult(0, json.dumps({"ok": True, "installed": True, "version": "1.5.0"}), "")
        return CmdResult(0, json.dumps({"ok": True, "action": action}), "")

    def _curl(self, argv, input):
        del input
        output = Path(argv[argv.index("-o") + 1])
        output.write_bytes(self.mac_data if "mac" in argv[-1] else self.win_data)
        return CmdResult(0, "", "")

    def _ditto(self, argv, input):
        del input
        destination = Path(argv[-1])
        destination.mkdir(parents=True, exist_ok=True)
        (destination / "Contents").mkdir()
        return CmdResult(0, "", "")

    def _with_app_dir(self):
        return patch.dict(os.environ, {"RS_BENCH_APP_DIR": str(self.root / "apps"),
                                       "RS_BENCH_CACHE": str(self.root / "cache")})

    def test_registry_and_fields(self):
        adapter = load("rustdesk")
        self.assertEqual(adapter.id, "rustdesk")
        self.assertEqual(adapter.label, "RustDesk")
        self.assertEqual(adapter.ports, (21118,))
        self.assertEqual(adapter.host_processes, ("rustdesk",))
        self.assertTrue(adapter.needs_host)

    def test_install_requires_consent(self):
        self.ctx.allow_install = False
        with self.assertRaises(BenchError):
            self.adapter.install(self.ctx)
        self.assertEqual(self.host.calls, [])
        self.assertEqual(self.host.puts, [])
        self.assertEqual(self.ctl.calls, [])

    def test_install_sequence(self):
        with self._with_app_dir():
            self.adapter.install(self.ctx)
        curl_calls = [call for call in self.ctl.calls if call[0][0] == "curl"]
        self.assertEqual(len(curl_calls), 2)
        host_actions = [call[0][call[0].index("-Action") + 1] for call in self.host.calls if "-Action" in call[0]]
        self.assertEqual(host_actions, ["install", "configure"])
        self.assertEqual([call[0][0] for call in self.ctl.calls],
                         ["curl", "curl", "hdiutil", "ditto", "hdiutil"])
        self.assertEqual(self.host.puts[0][1], "C:/rs-bench/downloads/rustdesk-win.msi")
        install_input = next(input for argv, input in self.host.calls if "install" in argv)
        configure_input = next(input for argv, input in self.host.calls if "configure" in argv)
        self.assertEqual(json.loads(install_input)["sha256"], hashlib.sha256(self.win_data).hexdigest())
        configured = json.loads(configure_input)
        self.assertEqual(configured["password"], self.ctx.secret("rustdesk-password"))
        self.assertEqual(configured["options"], rustdesk.HOST_OPTIONS)

    def test_detach_on_failure(self):
        def fail_ditto(argv, input):
            del argv, input
            return CmdResult(1, "", "ditto failed")

        self.ctl.rules.insert(0, (("ditto",), fail_ditto))
        with self._with_app_dir(), self.assertRaises(BenchError):
            self.adapter.install(self.ctx)
        self.assertTrue(any(argv[:2] == ["hdiutil", "detach"] for argv, _ in self.ctl.calls))

    def test_client_start_args(self):
        app = self.root / "apps" / "RustDesk.app"
        app.mkdir(parents=True)
        captured = []

        def gui(argv, **kwargs):
            captured.append((argv, kwargs))
            return CmdResult(0, "", "")

        self.ctx.gui = gui
        with self._with_app_dir():
            self.adapter.client_start(self.ctx, None, seconds=10)
        self.assertEqual(captured[0][0], ["/usr/bin/open", "-n", "-a", str(app), "--args",
                                          "--connect", "192.168.31.20", "--password",
                                          self.ctx.secret("rustdesk-password")])
        (app).rmdir()
        with self._with_app_dir(), self.assertRaisesRegex(BenchError, "install rustdesk"):
            self.adapter.client_start(self.ctx, None, seconds=10)

    def test_client_stop_fallback(self):
        self.ctx.gui = lambda argv, **kwargs: CmdResult(1, "", "osascript failed")
        self.adapter.client_stop(self.ctx, None)
        self.assertTrue(any(argv == ["/usr/bin/pkill", "-x", "RustDesk"] for argv, _ in self.ctl.calls))

    def test_password_not_logged(self):
        app = self.root / "apps" / "RustDesk.app"
        app.mkdir(parents=True)
        self.ctx.gui = lambda argv, **kwargs: CmdResult(0, "", "")
        with self._with_app_dir(), contextlib.redirect_stderr(io.StringIO()) as stderr:
            self.adapter.install(self.ctx)
            self.adapter.client_start(self.ctx, None, seconds=10)
        password = self.ctx.secret("rustdesk-password")
        run_log = self.root / "run.log"
        self.assertNotIn(password, run_log.read_text(encoding="utf-8") if run_log.exists() else "")
        self.assertNotIn(password, stderr.getvalue())

    def test_status_parse(self):
        app = self.root / "apps" / "RustDesk.app"
        (app / "Contents").mkdir(parents=True)
        with self._with_app_dir(), patch.object(self.ctl, "run", side_effect=self._status_ctl):
            result = self.adapter.status(self.ctx)
        self.assertEqual(result["host_version"], "1.5.0")
        self.assertTrue(result["client_installed"])
        self.assertEqual(result["client_version"], "1.5.0")
        with self._with_app_dir():
            shutil.rmtree(app)
            self.assertFalse(self.adapter.status(self.ctx)["client_installed"])

    def _status_ctl(self, argv, **kwargs):
        del kwargs
        if argv[0] == "plutil":
            return CmdResult(0, "1.5.0\n", "")
        return CmdResult(0, "", "")

    def test_ps_static(self):
        text = (Path(__file__).parents[1] / "host/products/rustdesk.ps1").read_text(encoding="ascii")
        self.assertIn("function Invoke-Product", text)
        for action in ("status", "version", "install", "configure", "start", "stop", "uninstall"):
            self.assertIn("'" + action + "'", text)
        self.assertIn("C:\\Windows\\ServiceProfiles\\LocalService\\AppData\\Roaming\\RustDesk\\config\\RustDesk2.toml", text)
        self.assertIn("C:\\Windows\\System32\\config\\systemprofile\\AppData\\Roaming\\RustDesk\\config\\RustDesk2.toml", text)
        self.assertNotRegex(text, r"Write-Log[^\n]*\$Params\.password")


if __name__ == "__main__":
    unittest.main()
