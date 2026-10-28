from __future__ import annotations

import abc
import os
import signal
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Iterable, Sequence


@dataclass
class CmdResult:
    code: int
    out: str
    err: str


class BenchError(RuntimeError):
    """An expected benchmark orchestration error."""


def _argv_strings(argv: Iterable[object]) -> list[str]:
    return [os.fspath(value) if isinstance(value, Path) else str(value) for value in argv]


class Handle:
    """A running local process."""

    def __init__(self, process: subprocess.Popen[str], output=None):
        self._process = process
        self._output = output
        self.pid = int(process.pid)

    def wait(self, timeout: float) -> int | None:
        try:
            return self._process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            return None
        finally:
            if self._process.poll() is not None and self._output is not None:
                self._output.close()

    def stop(self) -> None:
        if self._process.poll() is not None:
            if self._output is not None:
                self._output.close()
            return
        try:
            self._process.send_signal(signal.SIGTERM)
        except OSError:
            if self._output is not None:
                self._output.close()
            return
        try:
            self._process.wait(timeout=3.0)
        except subprocess.TimeoutExpired:
            try:
                self._process.kill()
            except OSError:
                pass
            try:
                self._process.wait(timeout=3.0)
            except subprocess.TimeoutExpired:
                pass
        finally:
            if self._output is not None:
                self._output.close()


class Executor(abc.ABC):
    @abc.abstractmethod
    def run(
        self,
        argv: Sequence[object],
        *,
        input: str | None = None,
        timeout: float = 120.0,
        check: bool = True,
    ) -> CmdResult:
        raise NotImplementedError

    @abc.abstractmethod
    def spawn(self, argv: Sequence[object], *, stdout_path: str | Path | None = None) -> Handle:
        raise NotImplementedError


def _raise_for_result(argv: list[str], result: CmdResult) -> None:
    if result.code:
        detail = result.err[-500:]
        raise BenchError(f"command exited {result.code}: {argv[0]}: {detail}")


class LocalExecutor(Executor):
    def run(
        self,
        argv: Sequence[object],
        *,
        input: str | None = None,
        timeout: float = 120.0,
        check: bool = True,
    ) -> CmdResult:
        command = _argv_strings(argv)
        try:
            completed = subprocess.run(
                command,
                input=input,
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                timeout=timeout,
            )
        except subprocess.TimeoutExpired as exc:
            raise BenchError(f"timeout after {timeout}s: {command[0]}") from exc
        except OSError as exc:
            raise BenchError(f"cannot run {command[0]}: {exc}") from exc
        result = CmdResult(completed.returncode, completed.stdout or "", completed.stderr or "")
        if check:
            _raise_for_result(command, result)
        return result

    def spawn(self, argv: Sequence[object], *, stdout_path: str | Path | None = None) -> Handle:
        command = _argv_strings(argv)
        output = None
        stdout = subprocess.DEVNULL
        if stdout_path is not None:
            output = Path(stdout_path).open("w", encoding="utf-8", newline="")
            stdout = output
        try:
            process = subprocess.Popen(
                command,
                stdin=subprocess.DEVNULL,
                stdout=stdout,
                stderr=subprocess.DEVNULL,
                text=True,
                encoding="utf-8",
                errors="replace",
            )
        except Exception:
            if output is not None:
                output.close()
            raise
        return Handle(process, output)


_REJECTED_REMOTE = ('"', "%", "^", "&", "|", "<", ">", "`", "\r", "\n")


def _reject_remote_arg(arg: str) -> None:
    if any(character in arg for character in _REJECTED_REMOTE):
        raise BenchError(f"unsafe remote argument: {arg!r}")


def _cmd_quote(arg: str) -> str:
    if arg and not any(character.isspace() for character in arg):
        return arg
    # CommandLineToArgvW treats a run of backslashes before a closing quote
    # specially; double those backslashes, and those before embedded quotes.
    escaped: list[str] = []
    backslashes = 0
    for character in arg:
        if character == "\\":
            backslashes += 1
            continue
        if character == '"':
            escaped.append("\\" * (backslashes * 2 + 1) + '"')
        else:
            escaped.append("\\" * backslashes + character)
        backslashes = 0
    escaped.append("\\" * (backslashes * 2))
    return '"' + "".join(escaped) + '"'


class SshExecutor(Executor):
    def __init__(self, alias: str, shell: str = "cmd", ssh: str = "ssh"):
        if shell not in ("cmd", "powershell"):
            raise ValueError("shell must be cmd or powershell")
        self.alias = alias
        self.shell = shell
        self.ssh = ssh
        self._local = LocalExecutor()

    def remote_command(self, argv: list[str]) -> str:
        values = _argv_strings(argv)
        for value in values:
            _reject_remote_arg(value)
        if self.shell == "powershell":
            return "& " + " ".join("'" + value.replace("'", "''") + "'" for value in values)
        return " ".join(_cmd_quote(value) for value in values)

    def run(
        self,
        argv: Sequence[object],
        *,
        input: str | None = None,
        timeout: float = 120.0,
        check: bool = True,
    ) -> CmdResult:
        remote = self.remote_command(_argv_strings(argv))
        command = [self.ssh, "-T", "-o", "BatchMode=yes", self.alias, remote]
        return self._local.run(command, input=input, timeout=timeout, check=check)

    def spawn(self, argv: Sequence[object], *, stdout_path: str | Path | None = None) -> Handle:
        remote = self.remote_command(_argv_strings(argv))
        command = [self.ssh, "-T", "-o", "BatchMode=yes", self.alias, remote]
        return self._local.spawn(command, stdout_path=stdout_path)

    def run_ps(
        self,
        script: str,
        *,
        input: str | None = None,
        timeout: float = 120.0,
        check: bool = True,
    ) -> CmdResult:
        import base64

        encoded = base64.b64encode(script.encode("utf-16le")).decode("ascii")
        return self.run(
            ["powershell", "-NoProfile", "-NonInteractive", "-EncodedCommand", encoded],
            input=input,
            timeout=timeout,
            check=check,
        )

    def put(self, local: Path, remote: str, *, timeout: float = 600.0) -> None:
        remote = remote.replace("\\", "/")
        self._local.run(
            ["scp", "-q", "-o", "BatchMode=yes", str(local), f"{self.alias}:{remote}"],
            timeout=timeout,
            check=True,
        )


class _FakeHandle:
    def __init__(self, pid: int):
        self.pid = pid

    def wait(self, timeout: float) -> int | None:
        del timeout
        return 0

    def stop(self) -> None:
        return None


class FakeExecutor(Executor):
    def __init__(self, rules=None, default: CmdResult | Callable | None = None):
        self.rules = list(rules or [])
        self.default = default
        self.calls: list[tuple[list[str], str | None]] = []
        self._next_pid = 1000

    @staticmethod
    def _matches(matcher, argv: list[str], input: str | None) -> bool:
        if callable(matcher):
            return bool(matcher(argv, input))
        if isinstance(matcher, tuple):
            return tuple(argv[: len(matcher)]) == matcher
        return False

    def run(
        self,
        argv: Sequence[object],
        *,
        input: str | None = None,
        timeout: float = 120.0,
        check: bool = True,
    ) -> CmdResult:
        del timeout
        command = _argv_strings(argv)
        self.calls.append((command, input))
        selected = None
        for matcher, outcome in self.rules:
            if self._matches(matcher, command, input):
                selected = outcome
                break
        if selected is None:
            selected = self.default
        if selected is None:
            raise AssertionError(f"unexpected call: {command!r}")
        result = selected(command, input) if callable(selected) else selected
        if not isinstance(result, CmdResult):
            raise TypeError("FakeExecutor rule must return CmdResult")
        if check:
            _raise_for_result(command, result)
        return result

    def spawn(self, argv: Sequence[object], *, stdout_path: str | Path | None = None) -> _FakeHandle:
        command = _argv_strings(argv)
        self.calls.append((command, None))
        if stdout_path is not None:
            Path(stdout_path).parent.mkdir(parents=True, exist_ok=True)
            Path(stdout_path).touch()
        handle = _FakeHandle(self._next_pid)
        self._next_pid += 1
        return handle
