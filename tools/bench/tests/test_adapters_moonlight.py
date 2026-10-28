from __future__ import annotations

import hashlib
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from benchlib.adapters.base import BenchError, Ctx
import benchlib.adapters.moonlight as moonlight
from benchlib.adapters.moonlight import MoonlightAdapter
from benchlib.executor import CmdResult, FakeExecutor


REPO = Path(__file__).resolve().parents[3]


class HostFake(FakeExecutor):
    def __init__(self):
        super().__init__(default=self.reply)
        self.put_calls = []

    def put(self, local: Path, remote: str):
        self.put_calls.append((str(local), remote))
        self.calls.append((["scp", str(local), remote], None))

    def reply(self, argv, input):
        if "-Action" not in argv:
            return CmdResult(0, "{}", "")
        action = argv[argv.index("-Action") + 1]
        value = {"ok": True}
        if action == "status":
            value.update(installed=True, version="2026.914.233613", service="Running", paired_clients=2)
        elif action == "uninstall":
            value.update(uninstalled=True, vigem_present=True, leftover_dirs=[])
        return CmdResult(0, json.dumps(value), "")


class MoonlightAdapterTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.repo = self.root / "repo"
        (self.repo / "tools/bench").mkdir(parents=True)
        self.sunshine = b"sunshine-msi"
        self.moonlight = b"moonlight-dmg"
        manifest = {
            "schema": 1,
            "items": {
                "sunshine-win": {
                    "sha256": hashlib.sha256(self.sunshine).hexdigest(),
                    "file": "Sunshine.msi",
                    "url": "https://example.invalid/sunshine",
                },
                "moonlight-mac": {
                    "sha256": hashlib.sha256(self.moonlight).hexdigest(),
                    "file": "Moonlight.dmg",
                    "url": "https://example.invalid/moonlight",
                },
            },
        }
        (self.repo / "tools/bench/thirdparty.json").write_text(json.dumps(manifest), encoding="utf-8")
        self.cache = self.root / "cache"
        self.state = self.root / "state"
        self.app = self.root / "Applications" / "Moonlight.app"
        self.bin = self.app / "Contents" / "MacOS" / "Moonlight"
        self.lab = {"host": {"addr": "192.168.31.20", "bench_dir": "C:\\rs-bench"}}
        self.host = HostFake()
        self.wait_code = 0
        self.client_label = "moonlight-client"
        self.pair_label = "moonlight-pair"
        self.ctl = FakeExecutor(default=self.ctl_reply)
        self.ctx = Ctx(
            lab=self.lab,
            host=self.host,
            ctl=self.ctl,
            repo=self.repo,
            state_dir=self.state,
            allow_install=False,
            log_path=self.root / "run.log",
            sleep=lambda _seconds: None,
        )
        self.adapter = MoonlightAdapter()
        self.path_patch = patch("benchlib.adapters.moonlight.APP_DIR", self.app.parent)
        self.app_patch = patch("benchlib.adapters.moonlight.APP", self.app)
        self.bin_patch = patch("benchlib.adapters.moonlight.BIN", self.bin)
        self.path_patch.start()
        self.app_patch.start()
        self.bin_patch.start()
        self.addCleanup(self.path_patch.stop)
        self.addCleanup(self.app_patch.stop)
        self.addCleanup(self.bin_patch.stop)

    def ctl_reply(self, argv, input):
        if argv and argv[0] == "curl":
            destination = Path(argv[argv.index("-o") + 1])
            destination.write_bytes(self.sunshine if "sunshine" in argv[-1] else self.moonlight)
            return CmdResult(0, "", "")
        if argv and argv[0] == "hdiutil" and "attach" in argv:
            return CmdResult(0, "/dev/disk9s1\tApple_HFS\t/Volumes/Moonlight\n", "")
        if argv and argv[0] == "ditto":
            self.bin.parent.mkdir(parents=True, exist_ok=True)
            self.bin.write_text("fake", encoding="utf-8")
            return CmdResult(0, "", "")
        if "--wait" in argv:
            return CmdResult(self.wait_code, "", "")
        if "--detach" in argv and "pair" in argv:
            return CmdResult(0, self.pair_label + "\n", "")
        if "--detach" in argv and "stream" in argv:
            return CmdResult(0, self.client_label + "\n", "")
        if argv and argv[0] == "plutil":
            return CmdResult(0, "6.2.0\n", "")
        return CmdResult(0, "", "")

    def test_registry_and_fields(self):
        self.assertEqual(self.adapter.id, "moonlight")
        self.assertEqual(self.adapter.label, "Moonlight + Sunshine")
        self.assertEqual(self.adapter.ports, (47984, 47989, 48010, 47998, 47999, 48000, 48002))
        self.assertEqual(self.adapter.host_processes, ("sunshine",))

    def test_install_requires_consent(self):
        with self.assertRaises(BenchError):
            self.adapter.install(self.ctx)
        self.assertEqual(self.host.put_calls, [])
        self.assertFalse(any(call[0] and call[0][0] in {"scp", "ssh", "hdiutil"} for call in self.ctl.calls))

    def test_install_sequence(self):
        self.ctx.allow_install = True
        with patch.dict(os.environ, {"RS_BENCH_CACHE": str(self.cache)}):
            self.adapter.install(self.ctx)
        self.assertEqual(len(self.host.put_calls), 1)
        actions = [argv[argv.index("-Action") + 1] for argv, _ in self.host.calls if "-Action" in argv]
        self.assertEqual(actions, ["install", "configure", "start", "pin"])
        configure = next(input for argv, input in self.host.calls if "-Action" in argv and argv[argv.index("-Action") + 1] == "configure")
        self.assertEqual(json.loads(configure)["user"], "rsbench")
        self.assertIn("password", json.loads(configure))
        pin = next(input for argv, input in self.host.calls if "-Action" in argv and argv[argv.index("-Action") + 1] == "pin")
        pin_data = json.loads(pin)
        self.assertRegex(pin_data["pin"], r"^\d{4}$")
        pair = next(argv for argv, _ in self.ctl.calls if "--detach" in argv and "pair" in argv)
        self.assertEqual(pair[pair.index("--pin") + 1], pin_data["pin"])
        wait = next(argv for argv, _ in self.ctl.calls if "--wait" in argv)
        self.assertEqual(wait[-3:], [self.pair_label, "--timeout", "60"])

    def test_pair_failure_stops_job(self):
        self.wait_code = 1
        with self.assertRaisesRegex(BenchError, "moonlight pairing failed"):
            self.adapter.pair(self.ctx)
        self.assertTrue(any("--stop" in argv and self.pair_label in argv for argv, _ in self.ctl.calls))

    def test_pin_stable(self):
        self.adapter.pair(self.ctx)
        first = json.loads(next(input for argv, input in self.host.calls if "-Action" in argv and argv[argv.index("-Action") + 1] == "pin"))["pin"]
        self.adapter.pair(self.ctx)
        pins = [json.loads(input)["pin"] for argv, input in self.host.calls if "-Action" in argv and argv[argv.index("-Action") + 1] == "pin"]
        self.assertEqual(pins, [first, first])
        self.assertEqual((self.state / "moonlight-pin.txt").stat().st_mode & 0o777, 0o600)

    def test_client_start_args(self):
        self.bin.parent.mkdir(parents=True, exist_ok=True)
        self.bin.write_text("fake", encoding="utf-8")
        cell = type("Cell", (), {"run_id": "run-1"})()
        self.adapter.client_start(self.ctx, cell, seconds=30)
        command = next(argv for argv, _ in self.ctl.calls if "--detach" in argv and "stream" in argv)
        index = command.index("--") + 1
        self.assertEqual(command[index:], [str(moonlight.BIN), "stream", "192.168.31.20", "Desktop", *moonlight.STREAM_ARGS])

    def test_client_stop_sequence(self):
        self.bin.parent.mkdir(parents=True, exist_ok=True)
        self.bin.write_text("fake", encoding="utf-8")
        cell = type("Cell", (), {"run_id": "run-1"})()
        self.adapter.client_start(self.ctx, cell, seconds=30)
        self.adapter.client_stop(self.ctx, cell)
        relevant = [argv for argv, _ in self.ctl.calls if argv and ("quit" in argv or "--stop" in argv or argv[0] == "pkill")]
        self.assertEqual(relevant[0][relevant[0].index("--") + 1:relevant[0].index("--") + 3], [str(moonlight.BIN), "quit"])
        self.assertIn("--stop", relevant[1])
        self.assertEqual(relevant[2][:2], ["pkill", "-x"])

    def test_secrets_not_logged(self):
        self.ctx.allow_install = True
        with patch.dict(os.environ, {"RS_BENCH_CACHE": str(self.cache)}):
            self.adapter.install(self.ctx)
        secret = self.ctx.secret("sunshine-web")
        text = (self.root / "run.log").read_text(encoding="utf-8") if (self.root / "run.log").exists() else ""
        self.assertNotIn(secret, text)

    def test_uninstall_reports_vigem(self):
        logs = []
        self.ctx.log = logs.append
        result = self.adapter.uninstall(self.ctx)
        self.assertTrue(result["vigem_present"])
        self.assertIn("ViGEmBus", result["warning"])
        self.assertTrue(any("ViGEmBus" in value for value in logs))

    def test_ps_static(self):
        path = REPO / "tools/bench/host/products/moonlight.ps1"
        data = path.read_bytes()
        self.assertTrue(data)
        self.assertLess(max(data), 0x80)
        text = data.decode("ascii")
        self.assertIn("function Invoke-Product", text)
        for action in ("status", "version", "install", "configure", "start", "stop", "pin", "uninstall"):
            self.assertIn("'" + action + "'", text)
        self.assertIn("SunshineService", text)
        self.assertIn("ServerCertificateValidationCallback", text)
        finally_at = text.index("finally", text.index("ServerCertificateValidationCallback"))
        restore_at = text.index("ServerCertificateValidationCallback = $oldCallback", finally_at)
        self.assertGreater(restore_at, finally_at)

    def test_settings_and_status(self):
        self.assertEqual(self.adapter.settings(), {
            "resolution": "1920x1080", "fps": 60, "codec": "HEVC", "bitrate_kbps": 20000,
            "vsync": False, "frame_pacing": False,
        })
        self.assertEqual(self.adapter.status(self.ctx)["extra"], {"paired_clients": 2})


if __name__ == "__main__":
    unittest.main()
