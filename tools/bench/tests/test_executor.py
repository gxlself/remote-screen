from __future__ import annotations

import base64
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

from benchlib.adapters.base import BenchError
from benchlib.executor import CmdResult, FakeExecutor, Handle, LocalExecutor, SshExecutor


class ExecutorTests(unittest.TestCase):
    def test_local_run_ok_and_fail(self):
        executor = LocalExecutor()
        reply = executor.run([sys.executable, "-c", "print(1)"])
        self.assertEqual((reply.code, reply.out, reply.err), (0, "1\n", ""))
        argv = [sys.executable, "-c", "import sys; sys.stderr.write('failure'); sys.exit(3)"]
        with self.assertRaisesRegex(BenchError, r"3.*failure"):
            executor.run(argv)
        self.assertEqual(executor.run(argv, check=False).code, 3)
        reply = executor.run([sys.executable, "-c", "import sys; sys.stdout.buffer.write(b'\\xff')"])
        self.assertIn("\ufffd", reply.out)

    def test_local_timeout(self):
        started = time.monotonic()
        with self.assertRaisesRegex(BenchError, "timeout after 0.5s"):
            LocalExecutor().run([sys.executable, "-c", "import time; time.sleep(5)"], timeout=0.5)
        self.assertGreaterEqual(time.monotonic() - started, 0.5)
        self.assertLess(time.monotonic() - started, 3)

    def test_local_spawn_wait_and_stop(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "out.txt"
            handle = LocalExecutor().spawn([sys.executable, "-c", "print('spawned')"], stdout_path=output)
            self.assertGreater(handle.pid, 0)
            self.assertEqual(handle.wait(3), 0)
            self.assertEqual(output.read_text(), "spawned\n")
            handle.stop()
            handle = LocalExecutor().spawn([sys.executable, "-c", "import time; time.sleep(30)"])
            self.assertIsNone(handle.wait(0.05))
            handle.stop()
            self.assertIsNotNone(handle.wait(1))

    def test_ssh_remote_command_cmd(self):
        executor = SshExecutor("bench-win")
        self.assertEqual(
            executor.remote_command(["C:\\Program Files\\x\\a.exe", "-P", "C:\\rs bench\\"]),
            '"C:\\Program Files\\x\\a.exe" -P "C:\\rs bench\\\\"',
        )
        self.assertEqual(executor.remote_command(["a", "", "a\tb"]), 'a "" "a\tb"')

    def test_stop_escalates_after_three_seconds(self):
        process = Mock(pid=123)
        process.poll.return_value = None
        process.wait.side_effect = [subprocess.TimeoutExpired("test", 3), -9]
        Handle(process).stop()
        process.send_signal.assert_called_once_with(signal.SIGTERM)
        process.kill.assert_called_once_with()
        self.assertEqual([call.kwargs for call in process.wait.call_args_list], [{"timeout": 3.0}, {"timeout": 3.0}])

    def test_ssh_remote_command_powershell(self):
        self.assertEqual(
            SshExecutor("bench-win", "powershell").remote_command(["a b", "it's"]),
            "& 'a b' 'it''s'",
        )

    def test_ssh_rejects_metachar(self):
        with patch("benchlib.executor.subprocess.run") as run:
            for shell in ("cmd", "powershell"):
                for character in ('&', '%', '"', '\n', '\r', '^', '|', '<', '>', '`'):
                    with self.subTest(shell=shell, character=character):
                        with self.assertRaises(BenchError):
                            SshExecutor("bench-win", shell).run(["test", "a" + character + "b"])
            run.assert_not_called()

    def test_ssh_run_ps_encoding(self):
        script = "Write-Output '\u00e9'"
        for shell in ("cmd", "powershell"):
            executor = SshExecutor("bench-win", shell)
            with patch.object(executor, "run", return_value=CmdResult(0, "", "")) as run:
                executor.run_ps(script, input="data", timeout=7, check=False)
            command = run.call_args.args[0]
            self.assertEqual(base64.b64decode(command[-1]).decode("utf-16le"), script)
            self.assertEqual(command[:-1], ["powershell", "-NoProfile", "-NonInteractive", "-EncodedCommand"])
            self.assertEqual(run.call_args.kwargs, {"input": "data", "timeout": 7, "check": False})

    def test_ssh_run_and_put(self):
        executor = SshExecutor("bench-win")
        with patch.object(executor._local, "run", return_value=CmdResult(0, "ok", "")) as run:
            executor.run(["C:\\rs bench\\a.exe", "status"], input="{}", timeout=9)
            self.assertEqual(run.call_args.args[0], [
                "ssh", "-T", "-o", "BatchMode=yes", "bench-win", '"C:\\rs bench\\a.exe" status',
            ])
            self.assertEqual(run.call_args.kwargs["input"], "{}")
            executor.put(Path("local.exe"), "C:\\rs-bench\\bin\\rs-bench.exe")
            self.assertEqual(run.call_args.args[0], [
                "scp", "-q", "-o", "BatchMode=yes", "local.exe", "bench-win:C:/rs-bench/bin/rs-bench.exe",
            ])

    def test_fake_executor_rules(self):
        with tempfile.TemporaryDirectory() as directory:
            destination = Path(directory) / "written"

            def write(argv, input):
                destination.write_text(input, encoding="utf-8")
                return CmdResult(0, argv[1], "")

            executor = FakeExecutor(rules=[
                (("prefix",), CmdResult(0, "prefix", "")),
                (lambda argv, input: argv == ["write", "file"] and input == "payload", write),
            ])
            self.assertEqual(executor.run(["prefix", "extra"]).out, "prefix")
            self.assertEqual(executor.run(["write", "file"], input="payload").out, "file")
            self.assertEqual(destination.read_text(), "payload")
            with self.assertRaisesRegex(AssertionError, "unexpected call"):
                executor.run(["missing"])
            self.assertEqual(executor.calls, [
                (["prefix", "extra"], None), (["write", "file"], "payload"), (["missing"], None),
            ])
            executor = FakeExecutor(default=CmdResult(3, "", "bad"))
            with self.assertRaisesRegex(BenchError, "3"):
                executor.run(["fail"])
            self.assertEqual(executor.run(["fail"], check=False).code, 3)
            fake = executor.spawn(["background"])
            self.assertEqual(fake.wait(1), 0)
            fake.stop()


if __name__ == "__main__":
    unittest.main()
