from __future__ import annotations

import contextlib
import copy
import hashlib
import io
import json
import os
import shutil
import sys
import tempfile
import types
import unittest
from datetime import datetime, timezone
from pathlib import Path
from unittest.mock import patch

from benchlib import model, runner
from benchlib.adapters.base import Adapter, BenchError, Ctx
from benchlib.executor import CmdResult, FakeExecutor

FIXTURES = Path(__file__).parent / "fixtures/runner"
REPO = Path(__file__).resolve().parents[3]
STARTED = datetime(2026, 10, 23, 10, 15, tzinfo=timezone.utc)
EPOCH = 1792507200.0


class FakeClock:
    def __init__(self):
        self.t = EPOCH - 20
        self.sleeps = []

    def now(self):
        return self.t

    def sleep(self, seconds):
        self.sleeps.append(seconds)
        self.t += seconds


class FakeAdapter(Adapter):
    ports = (7400,)
    host_processes = ("test-host",)

    def __init__(self, events, product="remotescreen", *, installed=True):
        self.events = events
        self.id = product
        self.label = product.title()
        self.needs_host = product != "local"
        self.installed = installed
        self.hashes = []

    def settings(self):
        return {"fps": 60}

    def status(self, ctx):
        return {"host_installed": self.installed, "client_installed": self.installed,
                "host_version": "1.0", "client_version": "1.0"}

    def install(self, ctx):
        self.events.append("install:" + self.id)

    def uninstall(self, ctx):
        self.events.append("uninstall:" + self.id)

    def host_start(self, ctx):
        self.events.append("host_start:" + self.id)

    def host_stop(self, ctx):
        self.events.append("host_stop:" + self.id)

    def client_start(self, ctx, cell, *, seconds):
        self.events.append("client_start:" + self.id)
        self.hashes.append(cell.page_hash(delay=3 if not self.needs_host else 0))

    def client_stop(self, ctx, cell):
        self.events.append("client_stop:" + self.id)

    def window(self, ctx):
        return {"app": "test.client", "title": "RS Bench"}


class RunnerTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.events = []
        self.clock = FakeClock()
        self.matrix = model.read_json(FIXTURES / "matrix_small.json")
        self.lab = model.read_json(FIXTURES / "lab.json")
        self.wait_code = 0
        self.interrupt = False
        self.netem_failure = False
        self.before_failure = False
        self.after_failure = False
        self.locked = False
        self.host_errors = {}
        self.capture_path = None
        self.adapter = FakeAdapter(self.events)
        self.host = FakeExecutor(rules=[(lambda argv, input: argv[0] == "powershell", self.host_reply)])
        self.ctl = FakeExecutor(rules=[(lambda argv, input: True, self.ctl_reply)])
        self.ctx = Ctx(lab=self.lab, host=self.host, ctl=self.ctl, repo=self.root / "repo",
                       state_dir=self.root / "state", sleep=self.clock.sleep)
        self.report = types.ModuleType("benchlib.report")
        self.report.render_report = lambda summary, baseline, verdict: f"# Benchmark\n\n{len(summary['results'])} results\n"
        self.judge = types.ModuleType("benchlib.judge")
        self.judge.judge = lambda current, baseline, thresholds: {"items": [], "schema": 1}
        self.judge.exit_code = lambda verdict: 0

    def host_reply(self, argv, input):
        self.assertEqual(input, "")
        command = argv[argv.index("-File") + 2]
        self.events.append("host:" + command)
        if command in self.host_errors:
            return CmdResult(1, json.dumps({"ok": False, "error": self.host_errors[command]}), "error")
        if command == "cat":
            return CmdResult(0, (FIXTURES / "host_stats_ok.jsonl").read_text(), "")
        data = {"ok": True}
        if command == "session":
            data.update({"logged_in": True, "locked": self.locked})
        if command == "hostinfo":
            data.update({"os_version": "10.0.22631", "cpu": "Example CPU", "gpus": ["Example GPU"], "display": "1920x1080@60"})
        return CmdResult(0, "progress\n" + json.dumps(data) + "\n\n", "")

    def ctl_reply(self, argv, input):
        if argv[0] == "sudo":
            self.events.append("netem:" + argv[-1])
            if self.netem_failure and argv[-1] != "off":
                return CmdResult(1, "", "password required")
            return CmdResult(0, "", "")
        if "clock" in argv:
            path = Path(argv[argv.index("--json") + 1])
            name = "before" if "before" in path.name else "after"
            self.events.append("clock:" + name)
            if (name == "before" and self.before_failure) or (name == "after" and self.after_failure):
                return CmdResult(1, "", "clock unavailable")
            model.write_json(path, {"schema": 1, "method": "stdio", "count": 200, "ok": 200,
                                   "t_us": int(self.clock.now() * 1_000_000), "offset_us": 0, "rtt_us": 500,
                                   "spread_us": 20, "used": 20})
            return CmdResult(0, "", "")
        if "--detach" in argv:
            self.events.append("capture:spawn")
            self.capture_path = Path(argv[argv.index("--out") + 1])
            return CmdResult(0, "test.capture.label\n", "")
        if "--wait" in argv:
            self.events.append("capture:wait")
            if self.interrupt:
                raise KeyboardInterrupt()
            if self.wait_code == 0:
                shutil.copyfile(FIXTURES / "frames_ok.jsonl", self.capture_path)
                self.clock.t += 14
            return CmdResult(self.wait_code, "", "")
        if "--stop" in argv:
            self.events.append("capture:stop")
            return CmdResult(0, "", "")
        if "ssim" in argv:
            self.events.append("ssim")
            model.write_json(Path(argv[argv.index("--json") + 1]), {"mean": 0.975, "n": 2})
            return CmdResult(0, "", "")
        raise AssertionError(f"unexpected controller call {argv!r}")

    def plan(self, **kwargs):
        return runner.plan_cells(self.matrix, started_at=STARTED, out_dir=self.root / "out",
                                 bench_dir=self.lab["host"]["bench_dir"], profiles=["weak", "wifi-good"], **kwargs)

    def base(self, adapter):
        return {"bench": {"git_rev": "abc", "rs_version": "0.1.0"},
                "topology": {"controller_os": "macos", "host_os": "windows"},
                "product": {"id": adapter.id, "label": adapter.label, "settings": adapter.settings()}}

    def run_one(self, *, product="remotescreen", scene="scroll", network="weak"):
        self.adapter = FakeAdapter(self.events, product)
        cell = self.plan(products=[product], scenes=[scene], networks=[network])[0]
        return runner.run_cell(self.ctx, self.adapter, cell, self.matrix, rs_bench=Path("rs-bench"),
                               base=self.base(self.adapter), now=self.clock.now), cell

    def test_plan_cells_order_and_filters(self):
        cells = self.plan(products=["local", "remotescreen"], repeats=2)
        expected = [(product, network, scene, repeat)
                    for product in ("local", "remotescreen") for network in ("lan", "weak")
                    if product != "local" or network == "lan"
                    for scene in ("static", "scroll") for repeat in (1, 2)]
        self.assertEqual([(cell.product, cell.network, cell.scene, cell.repeat) for cell in cells], expected)
        self.assertEqual(self.plan(products=["local"], networks=["weak"]), [])
        for name in ("handover", "bw-step", "nosuch"):
            with self.assertRaises(BenchError):
                self.plan(networks=[name])
        for value in ({"products": ["nosuch"]}, {"scenes": ["nosuch"]}, {"repeats": 0}, {"seconds": 3601}):
            with self.assertRaises(BenchError):
                self.plan(**value)
        for cell in cells:
            result = runner._result(cell, self.base(FakeAdapter(self.events, cell.product)))
            self.assertEqual(model.validate_result(result), [])
            if cell.product != "local":
                self.assertTrue(cell.host_run_dir.endswith("\\runs\\" + cell.run_id))
            else:
                self.assertIsNone(cell.host_run_dir)
        self.assertIn("weak", runner.netem_profiles(REPO))

    def test_remote_cell_happy_path(self):
        result, cell = self.run_one()
        self.assertEqual(self.events, [
            "host:mkdir", "host:sample-start", "clock:before", "client_start:remotescreen", "netem:weak",
            "capture:spawn", "host:page-start", "capture:wait", "netem:off", "host:page-stop",
            "client_stop:remotescreen", "host:sample-stop", "host:cat", "clock:after",
        ])
        self.assertEqual(model.validate_result(result), [])
        self.assertEqual(result["status"], "ok", result["error"])
        self.assertEqual(result["latency_ms"]["p50"], 40.0)
        self.assertEqual(result["delivery"]["frames_captured"], 600)
        self.assertEqual(result["delivery"]["distinct_codes"], 480)
        self.assertAlmostEqual(result["bandwidth"]["host_tx_mbps"], 9.9, delta=0.05)
        self.assertEqual(result["host_usage"]["cpu_pct_mean"], 5)
        self.assertEqual(result["network"], {"profile": "weak", "shaping": "mac-dummynet", "jitter_applied": False})
        self.assertEqual(result["clock"]["offset_before_us"], 0)
        self.assertEqual(model.read_json(cell.run_dir / "result.json"), result)
        self.assertEqual(self.clock.sleeps, [5, 8, 5, 2])
        self.assertIn(cell.run_id, (cell.run_dir / "run.log").read_text())
        self.assertIsNone(self.ctx.log_path)

    def test_lan_cell_no_netem(self):
        result, _ = self.run_one(network="lan")
        self.assertFalse(any(event.startswith("netem:") for event in self.events))
        self.assertEqual(result["network"]["shaping"], "none")
        self.assertEqual(self.clock.sleeps, [5, 8, 2])

    def test_capture_calib_timeout(self):
        self.wait_code = 3
        result, cell = self.run_one()
        self.assertEqual(result["status"], "failed")
        self.assertIn("calib-timeout", result["error"])
        self.assertEqual(self.events[-7:], [
            "netem:off", "capture:stop", "host:page-stop", "client_stop:remotescreen",
            "host:sample-stop", "host:cat", "clock:after",  # clock.json is a local write
        ])
        self.assertTrue((cell.run_dir / "clock.json").is_file())
        self.assertTrue((cell.run_dir / "result.json").is_file())

    def test_capture_error_codes(self):
        for code, message in ((4, "source-closed"), (5, "no-window"), (124, "timeout"), (7, "exit 7")):
            with self.subTest(code=code):
                self.wait_code = code
                result, _ = self.run_one()
                self.assertEqual(result["status"], "failed")
                self.assertIn(message, result["error"])

    def test_netem_unavailable(self):
        self.netem_failure = True
        result, _ = self.run_one()
        self.assertIn("netem unavailable", result["error"])
        self.assertNotIn("capture:spawn", self.events)
        for event in ("netem:off", "client_stop:remotescreen", "host:sample-stop"):
            self.assertIn(event, self.events)

    def test_interrupt_cleans_up(self):
        self.interrupt = True
        with self.assertRaises(KeyboardInterrupt):
            self.run_one()
        for event in ("netem:off", "capture:stop", "host:page-stop", "client_stop:remotescreen", "host:sample-stop", "clock:after"):
            self.assertIn(event, self.events)
        self.assertIsNone(self.ctx.log_path)

    def test_cleanup_continues_after_errors(self):
        self.wait_code = 3
        self.host_errors = {"page-stop": "page-stop failed", "sample-stop": "sample-stop failed"}
        result, _ = self.run_one()
        self.assertIn("calib-timeout", result["error"])
        self.assertIn("host:cat", self.events)
        self.assertIn("clock:after", self.events)

    def test_clock_probe_failures(self):
        self.before_failure = True
        result, cell = self.run_one()
        self.assertIn("clock probe failed", result["error"])
        self.assertNotIn("client_start:remotescreen", self.events)
        self.assertIn("clock:after", self.events)
        self.assertIsNone(model.read_json(cell.run_dir / "clock.json")["before"])
        self.before_failure = False
        self.after_failure = True
        result, cell = self.run_one()
        self.assertEqual(result["status"], "ok")
        self.assertIsNone(model.read_json(cell.run_dir / "clock.json")["after"])
        self.assertIsNone(result["clock"]["offset_after_us"])

    def test_local_cell_no_host_calls(self):
        result, cell = self.run_one(product="local", network="lan")
        self.assertEqual(self.host.calls, [])
        self.assertEqual(self.events[:3], ["capture:spawn", "client_start:local", "capture:wait"])
        for key in ("clock", "bandwidth", "host_usage"):
            self.assertIsNone(result[key])
        self.assertFalse((cell.run_dir / "clock.json").exists())
        self.assertIn("delay=3", self.adapter.hashes[0])
        self.assertEqual(result["latency_ms"]["p50"], 40.0)

    def test_static_runs_ssim(self):
        result, cell = self.run_one(scene="static")
        capture = next(argv for argv, _ in self.ctl.calls if "--detach" in argv)
        self.assertIn("--save-frames", capture)
        ssim = next(argv for argv, _ in self.ctl.calls if "ssim" in argv)
        self.assertEqual(ssim[ssim.index("--host-size") + 1], "1920x1080")
        frames = [ssim[index + 1] for index, argument in enumerate(ssim) if argument == "--frame"]
        self.assertEqual(frames, [str(cell.run_dir / "frames/frame-1.pgm"), str(cell.run_dir / "frames/frame-2.pgm")])
        self.assertEqual(result["quality"], {"ssim": 0.975, "frames": 2})

    def test_ssim_failure_is_nonfatal(self):
        original = self.ctl_reply

        def fail(argv, input):
            return CmdResult(1, "", "ssim failed") if "ssim" in argv else original(argv, input)

        self.ctl.rules = [(lambda argv, input: True, fail)]
        result, _ = self.run_one(scene="static")
        self.assertEqual(result["status"], "ok")
        self.assertIsNone(result["quality"])

    def test_run_matrix_blocks(self):
        adapters = {product: FakeAdapter(self.events, product, installed=product != "moonlight")
                    for product in ("remotescreen", "rustdesk", "moonlight")}
        cells = self.plan(products=list(adapters), scenes=["scroll"], networks=["lan"])
        with patch.dict(sys.modules, {"benchlib.report": self.report}):
            summary = runner.run_matrix(self.ctx, adapters, cells, self.matrix, rs_bench=Path("rs-bench"),
                                        base_for=self.base, out_dir=self.root / "out", rs_version="0.1.0")
        events = [event for event in self.events if event.startswith(("host_start:", "host_stop:"))]
        self.assertEqual(events, ["host_stop:rustdesk", "host_start:remotescreen", "host_stop:remotescreen",
                                  "host_stop:remotescreen", "host_start:rustdesk", "host_stop:rustdesk"])
        self.assertNotIn("client_start:moonlight", self.events)
        missing = next(result for result in summary["results"] if result["product"]["id"] == "moonlight")
        self.assertEqual(missing["status"], "failed")
        self.assertIn("not installed", missing["error"])
        self.assertEqual(model.validate_summary(summary), [])
        self.assertTrue((self.root / "out/report.md").is_file())
        self.assertEqual(model.read_json(self.root / "out/summary.json"), summary)
        self.assertEqual(self.events[0], "netem:off")

    def test_run_matrix_stops_host_on_interrupt(self):
        self.interrupt = True
        cells = self.plan(products=["remotescreen"], scenes=["scroll"], networks=["lan"])
        with self.assertRaises(KeyboardInterrupt):
            runner.run_matrix(self.ctx, {"remotescreen": self.adapter}, cells, self.matrix,
                              rs_bench=Path("rs-bench"), base_for=self.base, out_dir=self.root / "out", rs_version="0.1.0")
        self.assertEqual(self.events[-1], "host_stop:remotescreen")

    def test_run_matrix_baseline_report_contract(self):
        cells = self.plan(products=["local"], scenes=["scroll"], networks=["lan"])
        adapter = FakeAdapter(self.events, "local")
        baseline = model.read_json(FIXTURES / "summary_clean.json")
        thresholds = {"schema": 1}
        model.write_json(self.ctx.repo / "tools/bench/thresholds.json", thresholds)
        with patch.object(self.judge, "judge", wraps=self.judge.judge) as judge_call, \
                patch.object(self.report, "render_report", wraps=self.report.render_report) as report_call, \
                patch.dict(sys.modules, {"benchlib.report": self.report, "benchlib.judge": self.judge}):
            summary = runner.run_matrix(
                self.ctx, {"local": adapter}, cells, self.matrix, rs_bench=Path("rs-bench"),
                base_for=self.base, out_dir=self.root / "out", rs_version="0.1.0", baseline=baseline,
            )
        judge_call.assert_called_once_with(summary, baseline, thresholds)
        report_call.assert_called_once_with(summary, baseline, {"items": [], "schema": 1})

    def test_host_windows_include_clock_offset(self):
        with patch("benchlib.analyze.offset_at", return_value=1234), \
                patch("benchlib.hoststats.summarize", return_value=(None, None)) as summarize:
            self.run_one()
        _, window, baseline = summarize.call_args.args
        self.assertEqual(window, (int(EPOCH * 1_000_000) + 2_040_000 + 1234,
                                  int(EPOCH * 1_000_000) + 10_023_000 + 1234))
        self.assertEqual(baseline, (int((EPOCH - 20) * 1_000_000) + 1234,
                                    int((EPOCH - 15) * 1_000_000) + 1234))

    def test_host_start_failure_writes_failed_cells_and_stops(self):
        cells = self.plan(products=["remotescreen"], networks=["lan"])
        with patch.object(self.adapter, "host_start", side_effect=BenchError("start failed")), \
                patch.dict(sys.modules, {"benchlib.report": self.report}):
            summary = runner.run_matrix(
                self.ctx, {"remotescreen": self.adapter}, cells, self.matrix, rs_bench=Path("rs-bench"),
                base_for=self.base, out_dir=self.root / "out", rs_version="0.1.0",
            )
        self.assertTrue(all(result["error"] == "start failed" for result in summary["results"]))
        self.assertEqual(self.events[-1], "host_stop:remotescreen")
        self.assertTrue(all((cell.run_dir / "result.json").is_file() for cell in cells))

    def test_session_locked_skips_remote(self):
        self.locked = True
        adapters = {product: FakeAdapter(self.events, product) for product in ("local", "remotescreen")}
        cells = self.plan(products=list(adapters), scenes=["scroll"], networks=["lan"])
        with patch.dict(sys.modules, {"benchlib.report": self.report}):
            summary = runner.run_matrix(self.ctx, adapters, cells, self.matrix, rs_bench=Path("rs-bench"),
                                        base_for=self.base, out_dir=self.root / "out", rs_version="0.1.0")
        self.assertNotIn("client_start:remotescreen", self.events)
        self.assertIn("client_start:local", self.events)
        remote = next(result for result in summary["results"] if result["product"]["id"] == "remotescreen")
        self.assertEqual(remote["error"], "host console session not available")

    def test_secrets_file(self):
        value = self.ctx.secret("password")
        self.assertEqual(len(value), 16)
        self.assertEqual(self.ctx.secret("password"), value)
        self.assertEqual((self.ctx.state_dir / "secrets.json").stat().st_mode & 0o777, 0o600)
        self.assertEqual(self.ctx.state_dir.stat().st_mode & 0o777, 0o700)
        self.ctx.log_path = self.root / "run.log"
        output = io.StringIO()
        with contextlib.redirect_stderr(output):
            self.ctx.log("password=" + value)
        self.assertIn("password=***", output.getvalue())
        self.assertNotIn(value, self.ctx.log_path.read_text())
        fresh = Ctx(lab=self.lab, host=self.host, ctl=self.ctl, repo=self.ctx.repo, state_dir=self.ctx.state_dir)
        self.assertEqual(fresh.secret("password"), value)

    def test_host_helpers(self):
        self.ctx.host_ps("session")
        self.assertEqual(self.host.calls[-1][1], "")
        with patch.object(self.host, "put", create=True) as put:
            remote = self.ctx.host_put(Path("local"), "downloads/test.msi")
        self.assertEqual(remote, "C:\\rs-bench\\downloads\\test.msi")
        put.assert_called_once_with(Path("local"), "C:/rs-bench/downloads/test.msi")
        with self.assertRaises(BenchError):
            self.ctx.host_put(Path("local"), "../escape")
        self.host.rules = [(lambda argv, input: True, CmdResult(0, "not json", ""))]
        with self.assertRaisesRegex(BenchError, "bad output"):
            self.ctx.host_ps("session")
        with patch.object(self.ctl, "run", return_value=CmdResult(0, "label", "")) as call:
            self.ctx.gui(["rs-bench", "capture"], timeout=5.5, detach=True)
        self.assertEqual(call.call_args.args[0], [str(self.ctx.repo / "tools/bench/mac/gui-run.sh"),
                                                 "--timeout", "5", "--detach", "--cwd", str(self.ctx.repo),
                                                 "--", "rs-bench", "capture"])
        self.assertEqual(call.call_args.kwargs, {"timeout": 20.5, "check": False})

    def test_download_checks(self):
        cache = self.root / "cache"
        manifest = self.ctx.repo / "tools/bench/thirdparty.json"
        item = {"file": "package.zip", "url": "https://hs.example.com/package.zip", "sha256": None}
        model.write_json(manifest, {"items": {"test": item}})
        self.ctx.allow_install = True
        with patch.dict(os.environ, {"RS_BENCH_CACHE": str(cache)}):
            with self.assertRaisesRegex(BenchError, "sha256 not set"):
                self.ctx.download("test")
            with self.assertRaisesRegex(BenchError, "sha256 not set"):
                self.ctx.download("missing")
            item["sha256"] = hashlib.sha256(b"expected").hexdigest()
            model.write_json(manifest, {"items": {"test": item}})

            def curl(argv, input):
                Path(argv[argv.index("-o") + 1]).write_bytes(b"wrong")
                return CmdResult(0, "", "")

            self.ctl.rules = [(("curl",), curl)]
            with self.assertRaisesRegex(BenchError, "expected .*actual"):
                self.ctx.download("test")
            self.assertEqual(list(cache.iterdir()), [])

            def good(argv, input):
                Path(argv[argv.index("-o") + 1]).write_bytes(b"expected")
                return CmdResult(0, "", "")

            self.ctl.rules = [(("curl",), good)]
            destination = self.ctx.download("test")
            calls = len(self.ctl.calls)
            self.assertEqual(self.ctx.download("test"), destination)
            self.assertEqual(len(self.ctl.calls), calls)
            self.assertEqual(destination, cache / "package.zip")
            destination.write_bytes(b"corrupt")
            self.ctx.download("test")
            self.assertEqual(destination.read_bytes(), b"expected")
            self.assertEqual(len(self.ctl.calls), calls + 1)
            self.ctx.allow_install = False
            with self.assertRaisesRegex(BenchError, "install not allowed"):
                self.ctx.download("test")

    def test_inventory_diff_cases(self):
        before = {"Preferences": ["old", "same"], "Caches": ["gone"]}
        after = {"Preferences": ["new", "same"], "Caches": [], "LaunchAgents": ["agent"]}
        self.assertEqual(runner.inventory_diff(before, after),
                         {"Preferences": ["new"], "Caches": [], "LaunchAgents": ["agent"]})
        library = self.root / "home/Library/Preferences"
        library.mkdir(parents=True)
        (library / "b").touch()
        (library / "a").touch()
        self.assertEqual(runner.inventory_mac(self.root / "home"),
                         {"Preferences": ["a", "b"], "Application Support": [], "Caches": [], "LaunchAgents": []})

    def test_lab_and_matrix_validation(self):
        runner.validate_lab(self.lab)
        for mutate in (lambda lab: lab["host"].update(shell="bash"),
                       lambda lab: lab["host"].update(ssh="-ProxyCommand"),
                       lambda lab: lab["host"].update(bench_dir="relative")):
            lab = copy.deepcopy(self.lab)
            mutate(lab)
            with self.assertRaises(BenchError):
                runner.validate_lab(lab)
        bad = copy.deepcopy(self.matrix)
        bad["timing"]["baseline_idle_s"] = -1
        with self.assertRaises(BenchError):
            runner.validate_matrix(bad, ["weak"])

    def test_clock_command_quoting(self):
        import shlex

        self.lab["host"]["bench_dir"] = "C:\\rs bench"
        for shell, expected in (("cmd", '"C:\\rs bench\\bin\\rs-bench.exe" clock serve --stdio'),
                                ("powershell", '& "C:\\rs bench\\bin\\rs-bench.exe" clock serve --stdio')):
            self.lab["host"]["shell"] = shell
            command = shlex.split(runner._clock_command(self.ctx))
            self.assertEqual(command, ["ssh", "-T", "-o", "BatchMode=yes", "bench-win", expected])


if __name__ == "__main__":
    unittest.main()
