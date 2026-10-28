from __future__ import annotations

import contextlib
import ast
import importlib.util
import io
import json
import os
import shlex
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from benchlib import model
from benchlib.adapters.base import Ctx
from benchlib.executor import CmdResult, FakeExecutor
from tests.test_runner import FakeAdapter

BENCH_DIR = Path(__file__).resolve().parents[1]
REPO = BENCH_DIR.parent.parent
FIXTURES = Path(__file__).parent / "fixtures/runner"
CLI = BENCH_DIR / "bench.py"


def cli_module():
    spec = importlib.util.spec_from_file_location("bench_cli_test", CLI)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class CliTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.env = {**os.environ, "PYTHONPYCACHEPREFIX": str(self.root / "pycache"), "PYTHONDONTWRITEBYTECODE": "1"}

    def invoke(self, *args, with_judge=False, env=None):
        command = [sys.executable, str(CLI)]
        if with_judge and importlib.util.find_spec("benchlib.judge") is None:
            bootstrap = (
                "import sys, runpy; "
                f"sys.path.insert(0, {str(BENCH_DIR)!r}); "
                f"sys.path.insert(0, {str(FIXTURES)!r}); "
                "import cli_dependencies; cli_dependencies.install(); "
                f"sys.argv = [{str(CLI)!r}] + sys.argv[1:]; "
                f"runpy.run_path({str(CLI)!r}, run_name='__main__')"
            )
            command = [sys.executable, "-c", bootstrap]
        return subprocess.run(command + [str(arg) for arg in args], cwd=REPO,
                              env=env or self.env, capture_output=True, text=True, timeout=30)

    def dry_args(self):
        return ["run", "--dry-run", "--lab", FIXTURES / "lab.json", "--matrix", FIXTURES / "matrix_small.json"]

    def forbidden_tools(self):
        directory = self.root / "bin"
        directory.mkdir()
        sentinel = self.root / "called"
        for name in ("ssh", "scp", "curl", "sudo", "git", "sw_vers", "sysctl"):
            script = directory / name
            script.write_text("#!/bin/sh\nprintf called >> " + shlex.quote(str(sentinel)) + "\nexit 99\n")
            script.chmod(0o700)
        return {**self.env, "PATH": str(directory) + os.pathsep + self.env.get("PATH", "")}, sentinel

    def test_dry_run(self):
        env, sentinel = self.forbidden_tools()
        result = self.invoke(*self.dry_args(), "--out-root", self.root / "out", env=env)
        self.assertEqual(result.returncode, 0, result.stderr)
        lines = result.stdout.splitlines()
        self.assertEqual(len(lines), 14)
        for line in lines:
            self.assertRegex(line, r"^\d{8}T\d{6}Z-[a-z]+-[a-z]+-[a-z0-9-]+-r1$")
        self.assertFalse(sentinel.exists())
        self.assertFalse((self.root / "out").exists())
        result = self.invoke(*self.dry_args(), "--products", "local", "--networks", "weak", env=env)
        self.assertEqual((result.returncode, result.stdout), (0, ""))

    def test_judge_exit_codes(self):
        thresholds = BENCH_DIR / "thresholds.json"
        if not thresholds.exists():
            thresholds = FIXTURES / "thresholds.json"
        for fixture, code in (("summary_regression.json", 1), ("summary_clean.json", 0)):
            with self.subTest(fixture=fixture):
                result = self.invoke("judge", FIXTURES / fixture, "--baseline", FIXTURES / "summary_clean.json",
                                     "--thresholds", thresholds, "--json", self.root / "verdict.json", with_judge=True)
                self.assertEqual(result.returncode, code, result.stderr)
                self.assertTrue(result.stdout.strip())
                self.assertTrue((self.root / "verdict.json").is_file())
        result = self.invoke("judge", self.root / "missing.json")
        self.assertEqual(result.returncode, 2)

    def test_judge_bad_input(self):
        path = self.root / "invalid.json"
        path.write_text('{"schema": 1, "results": []}')
        result = self.invoke("judge", path)
        self.assertEqual(result.returncode, 2)
        self.assertIn("invalid summary", result.stderr)

    def test_install_without_yes_is_preview_only(self):
        env, sentinel = self.forbidden_tools()
        result = self.invoke("install", "rustdesk", "--lab", self.root / "no-lab.json", env=env)
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("AGPL-3.0", result.stdout)
        self.assertIn("https://github.com/rustdesk/rustdesk/releases/", result.stdout)
        self.assertFalse(sentinel.exists())
        self.assertNotIn("unavailable", result.stderr)

    def test_invalid_network_and_parameters(self):
        for arguments in (("--networks", "handover"), ("--networks", "bw-step"), ("--products", "unknown"),
                          ("--repeats", "0"), ("--seconds", "0"), ("--scenes", "static,,scroll")):
            result = self.invoke(*self.dry_args(), *arguments)
            self.assertEqual(result.returncode, 2, result.stdout)
        self.assertEqual(self.invoke("nosuch").returncode, 2)

    def test_py_compile_owned_files(self):
        files = [CLI, BENCH_DIR / "benchlib/executor.py", BENCH_DIR / "benchlib/runner.py",
                 BENCH_DIR / "benchlib/adapters/base.py", BENCH_DIR / "tests/test_executor.py",
                 BENCH_DIR / "tests/test_runner.py", Path(__file__)]
        result = subprocess.run([sys.executable, "-m", "py_compile", *map(str, files)],
                                cwd=REPO, env=self.env, capture_output=True, text=True, timeout=30)
        self.assertEqual(result.returncode, 0, result.stderr)
        for path in files:
            self.assertEqual(path.read_text().splitlines()[0], "from __future__ import annotations")
            ast.parse(path.read_text(), filename=str(path), feature_version=(3, 9))

    def test_run_exit_status_and_base_metadata(self):
        cli = cli_module()
        lab = model.read_json(FIXTURES / "lab.json")
        ctx = Ctx(lab=lab, host=FakeExecutor(), ctl=FakeExecutor(default=CmdResult(0, "fixture", "")),
                  repo=REPO, state_dir=self.root / "state")
        adapter = FakeAdapter([], "local")
        args = cli.parser().parse_args([
            "run", "--lab", str(FIXTURES / "lab.json"), "--matrix", str(FIXTURES / "matrix_small.json"),
            "--products", "local", "--out-root", str(self.root / "out"),
        ])
        with patch.object(cli, "_ctx", return_value=ctx), patch.object(cli, "_adapter", return_value=adapter), \
                patch.object(cli.runner, "run_matrix") as matrix, contextlib.redirect_stdout(io.StringIO()):
            for statuses, code in ((["failed"], 1), (["failed", "invalid"], 0), (["ok"], 0)):
                matrix.return_value = {"results": [{"status": value} for value in statuses]}
                self.assertEqual(cli.run(args), code)
            base = matrix.call_args.kwargs["base_for"](adapter)
        self.assertEqual(base["product"]["settings"], {"fps": 60})
        self.assertEqual(base["product"]["client_version"], "1.0")
        self.assertEqual(base["bench"]["rs_version"], cli.rs_version())
        self.assertEqual(ctx.host.calls, [])

    def test_selftest_and_report_wiring(self):
        cli = cli_module()
        lab = model.read_json(FIXTURES / "lab.json")
        ctx = Ctx(lab=lab, host=FakeExecutor(), ctl=FakeExecutor(default=CmdResult(0, "fixture", "")),
                  repo=REPO, state_dir=self.root / "state")
        adapter = FakeAdapter([], "local")
        args = cli.parser().parse_args(["selftest", "--out-root", str(self.root / "out")])
        result = model.read_json(FIXTURES / "summary_clean.json")["results"][0]
        result["product"]["id"] = "local"
        output = io.StringIO()
        with patch.object(cli, "_ctx", return_value=ctx), patch.object(cli, "_adapter", return_value=adapter), \
                patch.object(cli.runner, "run_cell", return_value=result) as run, contextlib.redirect_stdout(output):
            self.assertEqual(cli.selftest(args), 0)
        cell = run.call_args.args[2]
        self.assertEqual((cell.product, cell.scene, cell.network, cell.seconds), ("local", "static", "lan", 10))
        self.assertIn("selftest", cell.run_dir.parts)
        self.assertIn("p50=40", output.getvalue())
        self.assertIn("p95=40", output.getvalue())
        self.assertIn("fps=60", output.getvalue())
        self.assertEqual(ctx.host.calls, [])
        report_args = cli.parser().parse_args([
            "report", str(FIXTURES / "summary_clean.json"), "--out", str(self.root / "report.md"),
        ])
        with patch.object(cli.runner, "dependency") as dependency:
            dependency.return_value.render_report.return_value = "# Report\n"
            self.assertEqual(cli._report(report_args), 0)
        self.assertEqual((self.root / "report.md").read_text(), "# Report\n")

    def test_install_uninstall_inventory(self):
        cli = cli_module()
        lab = model.read_json(FIXTURES / "lab.json")
        inventory = {"services": ["existing"], "programs": [], "drivers": [], "firewall_rules": []}
        host = FakeExecutor(default=lambda argv, input: CmdResult(0, json.dumps({"ok": True, **inventory}), ""))
        ctx = Ctx(lab=lab, host=host, ctl=FakeExecutor(), repo=REPO, state_dir=self.root / "state")
        events = []
        adapter = FakeAdapter(events, "rustdesk")
        output = io.StringIO()
        with patch.object(Path, "home", return_value=self.root / "home"), contextlib.redirect_stdout(output):
            self.assertEqual(cli.install(ctx, adapter), 0)
            before = ctx.state_dir / "inventory/rustdesk-before.json"
            first = before.read_bytes()
            inventory["services"].append("installed")
            self.assertEqual(cli.install(ctx, adapter), 0)
            self.assertEqual(before.read_bytes(), first)
            self.assertEqual(cli.uninstall(ctx, adapter), 3)
            self.assertTrue(before.exists())
            leftovers = model.read_json(before.with_name("rustdesk-leftover.json"))
            self.assertEqual(leftovers["host"]["services"], ["installed"])
            inventory["services"].remove("installed")
            self.assertEqual(cli.uninstall(ctx, adapter), 0)
            self.assertFalse(before.exists())
        self.assertTrue(ctx.allow_install)

    def test_deploy_and_status(self):
        cli = cli_module()
        lab = model.read_json(FIXTURES / "lab.json")
        host = FakeExecutor(default=lambda argv, input: CmdResult(0, json.dumps({
            "ok": True, "logged_in": True, "locked": False, "rs_bench_exe": True, "page": True, "edge": "msedge.exe",
        }), ""))
        ctl = FakeExecutor(default=CmdResult(0, "ready", ""))
        ctx = Ctx(lab=lab, host=host, ctl=ctl, repo=REPO, state_dir=self.root / "state")
        exe = self.root / "rs-bench.exe"
        exe.write_text("fake")
        with patch.object(host, "run_ps", create=True) as ps, patch.object(host, "put", create=True) as put:
            self.assertEqual(cli.deploy(ctx, exe), 0)
        self.assertIn("host\\products", ps.call_args.args[0])
        destinations = [call.args[1] for call in put.call_args_list]
        self.assertIn("C:/rs-bench/page/bench.html", destinations)
        self.assertIn("C:/rs-bench/bin/rs-bench.exe", destinations)
        with patch.object(cli, "_adapter", side_effect=lambda product: FakeAdapter([], product)):
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                self.assertEqual(cli.status(ctx, as_json=True), 0)
            self.assertTrue(json.loads(output.getvalue())["host.session"]["ok"])


if __name__ == "__main__":
    unittest.main()
