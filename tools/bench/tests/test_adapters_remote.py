from __future__ import annotations

import ast
import contextlib
import hashlib
import io
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from benchlib.adapters import load
from benchlib.adapters.base import BenchError, Cell, Ctx
from benchlib.adapters.local import create as create_local
from benchlib.adapters.remotescreen import APP_ID, _device_udid, create as create_remote
from benchlib.executor import CmdResult, FakeExecutor


REPO = Path(__file__).resolve().parents[3]
SIM_DEVICE = "iPhone 17 Pro"
DEVICE_LIST = ("xcrun", "simctl", "list", "devices", "available", "-j")
DEVICE_FIXTURE = json.dumps({
    "devices": {"com.apple.CoreSimulator.SimRuntime.iOS-18-0": [
        {"name": SIM_DEVICE, "udid": "BOOTED", "state": "Booted"},
    ]},
})


def make_cell(product: str = "local", seconds: int = 10) -> Cell:
    return Cell(
        product=product,
        scene="static",
        network="lan",
        repeat=1,
        seconds=seconds,
        calib_s=4,
        run_id="test",
        run_dir=Path("target/test"),
        host_run_dir=None,
    )


def make_lab(*, sim_app=None, host_msi=None, sim_device="iPhone 17 Pro") -> dict:
    return {
        "schema": 1,
        "host": {"os": "windows", "addr": "192.168.31.20", "bench_dir": "C:\\rs-bench"},
        "controller": {"os": "macos", "addr": "192.168.31.30", "sim_device": sim_device, "sim_app": sim_app},
        "products": {"remotescreen": {"host_msi": host_msi}},
    }


class AdapterTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()

    def ctx(self, *, lab=None, host=None, ctl=None, repo=None):
        return Ctx(
            lab=lab if lab is not None else make_lab(),
            host=host or FakeExecutor(default=CmdResult(0, json.dumps({"ok": True}), "")),
            ctl=ctl or FakeExecutor(default=CmdResult(0, "", "")),
            repo=repo or REPO,
            state_dir=self.root / "state",
            sleep=lambda _seconds: None,
        )

    def test_registry_loads(self):
        self.assertEqual(load("local").id, "local")
        self.assertEqual(load("remotescreen").id, "remotescreen")

    def test_settings_and_host_requirements(self):
        local, remote = create_local(), create_remote()
        self.assertEqual(local.label, "本机基线")
        self.assertEqual(local.settings(), {"client": "safari", "window": "1600x900"})
        self.assertEqual((local.needs_host, local.ports, local.host_processes), (False, (), ()))
        self.assertEqual(remote.label, "本产品")
        self.assertEqual(remote.settings(), {
            "resolution": "1920x1080", "fps": 60, "codec": "default", "client": "ios-simulator",
        })
        self.assertTrue(remote.needs_host)
        self.assertEqual(remote.ports, (7400,))
        self.assertEqual(remote.host_processes, ("rs-host", "rs-host-session"))
        context = self.ctx()
        for action in (local.install, local.uninstall, local.host_start, local.host_stop):
            action(context)
        self.assertEqual((context.host.calls, context.ctl.calls), ([], []))

    def test_local_client_start(self):
        ctl = FakeExecutor(default=CmdResult(0, "", ""))
        context = self.ctx(lab=make_lab(), ctl=ctl)
        create_local().client_start(context, make_cell(), seconds=77)
        self.assertEqual(len(ctl.calls), 1)
        command, _input = ctl.calls[0]
        self.assertIn("gui-run.sh", command[0])
        self.assertEqual(command[command.index("--") + 1], "/usr/bin/osascript")
        script = "\n".join(command)
        url_arg = next(item for item in command if item.startswith("make new document with properties"))
        expected_url = (REPO / "tools/bench/page/bench.html").as_uri()
        expected_url += "#scene=static&seconds=10&calib=4&delay=3"
        self.assertTrue(url_arg.endswith(expected_url + '\"}'))
        self.assertIn("set bounds of front window to {0, 30, 1600, 930}", script)
        self.assertEqual(command[1:3], ["--timeout", "30"])

    def test_local_url_encodes_path_only(self):
        repo = self.root / "repo space#percent%&apostrophe'"
        context = self.ctx(repo=repo)
        create_local().client_start(context, make_cell(), seconds=77)
        script = context.ctl.calls[0][0]
        expected = (
            self.root.as_uri() + "/repo%20space%23percent%25%26apostrophe%27"
            "/tools/bench/page/bench.html#scene=static&seconds=10&calib=4&delay=3"
        )
        self.assertIn(f'make new document with properties {{URL:"{expected}"}}', script)

    def test_local_rejects_bad_url(self):
        for name in ('repo"with-quote', "repo\\with-backslash"):
            with self.subTest(name=name):
                context = self.ctx(repo=self.root / name)
                with self.assertRaises(BenchError):
                    create_local().client_start(context, make_cell(), seconds=10)
                self.assertEqual(context.ctl.calls, [])

    def test_local_start_reports_automation_permission(self):
        ctl = FakeExecutor(default=CmdResult(1, "", "Not authorized (-1743)"))
        with self.assertRaises(BenchError) as error:
            create_local().client_start(self.ctx(ctl=ctl), make_cell(), seconds=10)
        message = str(error.exception)
        for detail in ("Not authorized (-1743)", "System Settings", "Automation", "osascript", "Safari"):
            self.assertIn(detail, message)

    def test_local_client_stop_ignores_failure(self):
        def fail(argv, input):
            raise BenchError("GUI timeout")

        for outcome in (CmdResult(1, "", "permission denied"), fail):
            with self.subTest(outcome=outcome):
                context = self.ctx(ctl=FakeExecutor(default=outcome))
                with contextlib.redirect_stderr(io.StringIO()) as log:
                    create_local().client_stop(context, make_cell())
                command = context.ctl.calls[0][0]
                self.assertEqual(command[command.index("--") + 1:], [
                    "/usr/bin/osascript", "-e",
                    'tell application "Safari" to close (every document whose URL contains '
                    '"tools/bench/page/bench.html")',
                ])
                self.assertIn("Safari cleanup", log.getvalue())

    def test_local_status_and_window(self):
        ctl = FakeExecutor(
            rules=[
                (("/usr/bin/defaults",), CmdResult(0, "26.0\n", "")),
            ],
            default=CmdResult(1, "", "missing"),
        )
        context = self.ctx(lab=make_lab(), ctl=ctl)
        status = create_local().status(context)
        self.assertEqual(status["client_version"], "26.0")
        self.assertTrue(status["client_installed"])
        self.assertTrue(status["host_installed"])
        self.assertIsNone(status["host_version"])
        self.assertEqual(ctl.calls[0][0], [
            "/usr/bin/defaults", "read", "/Applications/Safari.app/Contents/Info", "CFBundleShortVersionString",
        ])
        self.assertEqual(create_local().window(context), {"app": "com.apple.Safari", "title": "RS Bench"})
        failed = self.ctx(lab=make_lab(), ctl=FakeExecutor(default=CmdResult(1, "", "missing")))
        status = create_local().status(failed)
        self.assertFalse(status["client_installed"])
        self.assertIsNone(status["client_version"])
        self.assertEqual(context.host.calls, [])

    def test_remote_device_udid(self):
        fixture = json.dumps(
            {
                "devices": {
                    "com.apple.CoreSimulator.SimRuntime.iOS-18-0": [
                        {"name": SIM_DEVICE, "udid": "A-UDID", "state": "Shutdown"},
                        {"name": SIM_DEVICE, "udid": "Z-UDID", "state": "Booted"},
                    ],
                    "com.apple.CoreSimulator.SimRuntime.iOS-19-0": [
                        {"name": SIM_DEVICE, "udid": "B-UDID", "state": "Booted"},
                    ],
                }
            }
        )
        ctl = FakeExecutor(default=CmdResult(0, fixture, ""))
        context = self.ctx(lab=make_lab(), ctl=ctl)
        self.assertEqual(_device_udid(context), "B-UDID")
        self.assertEqual(ctl.calls[0][0], list(DEVICE_LIST))
        missing = self.ctx(
            lab=make_lab(sim_device="iPad absent"),
            ctl=FakeExecutor(default=CmdResult(0, fixture, "")),
        )
        with self.assertRaisesRegex(BenchError, "simulator device not found"):
            _device_udid(missing)

    def test_remote_device_sorts_shutdown_devices(self):
        fixture = json.dumps({"devices": {"runtime": [
            {"name": SIM_DEVICE, "udid": "Z-UDID", "state": "Shutdown"},
            {"name": SIM_DEVICE + " Max", "udid": "0-UDID", "state": "Booted"},
            {"name": SIM_DEVICE, "udid": "A-UDID", "state": "Shutdown"},
        ]}})
        context = self.ctx(ctl=FakeExecutor(default=CmdResult(0, fixture, "")))
        self.assertEqual(_device_udid(context), "A-UDID")

    def test_remote_device_reports_list_errors(self):
        for result, message in (
            (CmdResult(1, "", "Xcode not selected"), "Xcode not selected"),
            (CmdResult(0, "not JSON", ""), "invalid JSON"),
        ):
            with self.subTest(result=result):
                context = self.ctx(ctl=FakeExecutor(default=result))
                with self.assertRaisesRegex(BenchError, message):
                    _device_udid(context)

    def test_remote_client_start_sequence(self):
        ctl = FakeExecutor(rules=[
            (DEVICE_LIST, CmdResult(0, DEVICE_FIXTURE, "")),
            (("xcrun", "simctl", "terminate"), CmdResult(1, "", "not running")),
        ], default=CmdResult(0, "", ""))
        context = self.ctx(lab=make_lab(), ctl=ctl)
        with patch.object(ctl, "run", wraps=ctl.run) as run:
            create_remote().client_start(context, make_cell("remotescreen"), seconds=77)
        self.assertEqual([
            argv[2] if argv[:2] == ["xcrun", "simctl"] else "gui"
            for argv, _input in ctl.calls
        ], ["list", "bootstatus", "gui", "terminate", "launch"])
        self.assertEqual(run.call_args_list[1].kwargs["timeout"], 180)
        self.assertFalse(run.call_args_list[3].kwargs["check"])
        simctl = [argv for argv, _input in ctl.calls if argv[:2] == ["xcrun", "simctl"]]
        self.assertEqual(simctl[0][2:5], ["list", "devices", "available"])
        self.assertEqual(simctl[1][2:], ["bootstatus", "BOOTED", "-b"])
        self.assertEqual(simctl[2][2:], ["terminate", "BOOTED", APP_ID])
        self.assertEqual(simctl[3][2:7], ["launch", "BOOTED", APP_ID, "-autoConnect", "192.168.31.20"])
        self.assertEqual(simctl[3][-2:], ["-disconnectAfter", "77"])
        gui_calls = [call[0] for call in ctl.calls if "gui-run.sh" in call[0][0]]
        self.assertEqual(len(gui_calls), 1)
        self.assertEqual(gui_calls[0][gui_calls[0].index("--") + 1:], [
            "/usr/bin/open", "-a", "Simulator", "--args", "-CurrentDeviceUDID", "BOOTED",
        ])
        self.assertEqual(gui_calls[0][1:3], ["--timeout", "30"])

    def test_remote_client_start_fails_on_boot_gui_or_launch(self):
        gui_path = str(REPO / "tools/bench/mac/gui-run.sh")
        for matcher in (("xcrun", "simctl", "bootstatus"), (gui_path,), ("xcrun", "simctl", "launch")):
            with self.subTest(matcher=matcher):
                ctl = FakeExecutor(rules=[
                    (DEVICE_LIST, CmdResult(0, DEVICE_FIXTURE, "")),
                    (matcher, CmdResult(1, "", "fixture failure")),
                ], default=CmdResult(0, "", ""))
                with self.assertRaisesRegex(BenchError, "fixture failure"):
                    create_remote().client_start(self.ctx(ctl=ctl), make_cell("remotescreen"), seconds=77)
                self.assertEqual(ctl.calls[-1][0][:len(matcher)], list(matcher))

    def test_remote_client_stop_leaves_simulator_booted(self):
        ctl = FakeExecutor(rules=[
            (DEVICE_LIST, CmdResult(0, DEVICE_FIXTURE, "")),
            (("xcrun", "simctl", "terminate"), CmdResult(1, "", "not running")),
        ])
        context = self.ctx(ctl=ctl)
        create_remote().client_stop(context, make_cell("remotescreen"))
        self.assertEqual([argv for argv, _input in ctl.calls], [
            list(DEVICE_LIST), ["xcrun", "simctl", "terminate", "BOOTED", APP_ID],
        ])
        self.assertEqual(create_remote().window(context), {
            "app": "com.apple.iphonesimulator", "title": SIM_DEVICE,
        })

    def test_remote_install_requires_msi(self):
        context = self.ctx(lab=make_lab(host_msi=None))
        with self.assertRaisesRegex(BenchError, "host_msi"):
            create_remote().install(context)
        self.assertEqual((context.host.calls, context.ctl.calls), ([], []))

    def test_remote_install_uploads_and_verifies(self):
        msi = self.root / "Remote Host.msi"
        contents = b"test msi"
        msi.write_bytes(contents)
        events = []

        def host_reply(argv, input):
            events.append("install")
            return CmdResult(0, json.dumps({"ok": True}), "")

        host = FakeExecutor(default=host_reply)
        ctl = FakeExecutor()
        context = self.ctx(lab=make_lab(host_msi=str(msi)), host=host, ctl=ctl)
        self.assertFalse(context.allow_install)
        with patch.object(host, "put", create=True, side_effect=lambda *args: events.append("upload")) as upload:
            with contextlib.redirect_stderr(io.StringIO()) as log:
                create_remote().install(context)
        upload.assert_called_once_with(msi, "C:/rs-bench/downloads/Remote Host.msi")
        self.assertEqual(events, ["upload", "install"])
        self.assertEqual(len(host.calls), 1)
        argv, stdin = host.calls[0]
        self.assertEqual(argv[-5:], ["product", "-Id", "remotescreen", "-Action", "install"])
        self.assertEqual(json.loads(stdin), {
            "msi": "C:\\rs-bench\\downloads\\Remote Host.msi",
            "sha256": hashlib.sha256(contents).hexdigest(),
        })
        self.assertEqual(ctl.calls, [])
        self.assertIn("假定 App 已装好", log.getvalue())

    def test_remote_install_and_uninstall_sim_app_when_configured(self):
        msi = self.root / "RemoteHost.msi"
        msi.write_bytes(b"msi")
        app = str(self.root / "App build/RemoteScreen.app")
        ctl = FakeExecutor(rules=[(DEVICE_LIST, CmdResult(0, DEVICE_FIXTURE, ""))], default=CmdResult(0, "", ""))
        context = self.ctx(lab=make_lab(host_msi=str(msi), sim_app=app), ctl=ctl)
        with patch.object(context.host, "put", create=True):
            create_remote().install(context)
        self.assertEqual(ctl.calls[-1][0], ["xcrun", "simctl", "install", "BOOTED", app])
        create_remote().uninstall(context)
        self.assertEqual(ctl.calls[-1][0], ["xcrun", "simctl", "uninstall", "BOOTED", APP_ID])
        self.assertEqual(context.host.calls[-1][0][-5:], ["product", "-Id", "remotescreen", "-Action", "uninstall"])

    def test_remote_uninstall_preserves_unmanaged_client(self):
        context = self.ctx(ctl=FakeExecutor())
        create_remote().uninstall(context)
        self.assertEqual(context.host.calls[0][0][-5:], ["product", "-Id", "remotescreen", "-Action", "uninstall"])
        self.assertEqual(context.ctl.calls, [])

    def test_remote_host_start_stop(self):
        host = FakeExecutor(default=CmdResult(0, json.dumps({"ok": True}), ""))
        context = self.ctx(lab=make_lab(), host=host)
        adapter = create_remote()
        adapter.host_start(context)
        adapter.host_stop(context)
        self.assertEqual([argv[-5:] for argv, _input in host.calls], [
            ["product", "-Id", "remotescreen", "-Action", "start"],
            ["product", "-Id", "remotescreen", "-Action", "stop"],
        ])

    def test_remote_status_no_device(self):
        host = FakeExecutor(default=CmdResult(0, json.dumps({"ok": True, "installed": True, "version": "1.2"}), ""))
        ctl = FakeExecutor(default=CmdResult(0, json.dumps({"devices": {}}), ""))
        with contextlib.redirect_stderr(io.StringIO()):
            status = create_remote().status(self.ctx(lab=make_lab(), host=host, ctl=ctl))
        self.assertTrue(status["host_installed"])
        self.assertFalse(status["client_installed"])
        self.assertIsNone(status["client_version"])
        self.assertEqual(len(ctl.calls), 1)

    def test_remote_status_client_and_version(self):
        app_path = str(self.root / "Simulator Apps/RemoteScreen.app")
        for app_code, version_code in ((0, 0), (0, 1), (1, 0)):
            with self.subTest(app_code=app_code, version_code=version_code):
                host = FakeExecutor(default=CmdResult(0, json.dumps({
                    "ok": True, "installed": True, "version": "0.5.0",
                }), ""))
                ctl = FakeExecutor(rules=[
                    (DEVICE_LIST, CmdResult(0, DEVICE_FIXTURE, "")),
                    (("xcrun", "simctl", "get_app_container"), CmdResult(app_code, app_path + "\n", "")),
                    (("/usr/bin/plutil",), CmdResult(version_code, "1.2\n", "")),
                ])
                status = create_remote().status(self.ctx(host=host, ctl=ctl))
                self.assertEqual(status, {
                    "host_installed": True, "host_version": "0.5.0",
                    "client_installed": app_code == 0,
                    "client_version": "1.2" if app_code == 0 and version_code == 0 else None,
                })
                self.assertEqual(ctl.calls[1][0], ["xcrun", "simctl", "get_app_container", "BOOTED", APP_ID, "app"])
                if app_code == 0:
                    self.assertEqual(ctl.calls[2][0], [
                        "/usr/bin/plutil", "-extract", "CFBundleShortVersionString", "raw", app_path + "/Info.plist",
                    ])
                else:
                    self.assertEqual(len(ctl.calls), 2)

    def test_ps_actions_present(self):
        path = REPO / "tools/bench/host/products/remotescreen.ps1"
        text = path.read_text(encoding="ascii")
        self.assertIn("RemoteHostService", text)
        self.assertIn("function Invoke-Product", text)
        for action in ("status", "version", "install", "configure", "start", "stop", "uninstall"):
            self.assertIn("'" + action + "'", text)
        self.assertTrue(all(byte < 128 for byte in path.read_bytes()))

    def test_python39_syntax(self):
        for path in (REPO / "tools/bench/benchlib/adapters/local.py",
                     REPO / "tools/bench/benchlib/adapters/remotescreen.py", Path(__file__)):
            with self.subTest(path=path):
                text = path.read_text(encoding="utf-8")
                self.assertEqual(text.splitlines()[0], "from __future__ import annotations")
                ast.parse(text, filename=str(path), feature_version=(3, 9))


if __name__ == "__main__":
    unittest.main()
