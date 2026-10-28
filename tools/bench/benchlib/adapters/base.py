from __future__ import annotations

import abc
import hashlib
import json
import os
import secrets
import string
import sys
import tempfile
import time
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path, PureWindowsPath

from benchlib.executor import BenchError, CmdResult, Executor


@dataclass(frozen=True)
class Cell:
    product: str
    scene: str
    network: str
    repeat: int
    seconds: int
    calib_s: int
    run_id: str
    run_dir: Path
    host_run_dir: str | None

    def page_hash(self, delay: int = 0) -> str:
        return f"scene={self.scene}&seconds={self.seconds}&calib={self.calib_s}&delay={delay}"


class Ctx:
    def __init__(
        self,
        *,
        lab: dict,
        host: Executor,
        ctl: Executor,
        repo: Path,
        state_dir: Path,
        allow_install: bool = False,
        log_path: Path | None = None,
        sleep=time.sleep,
    ):
        self.lab = lab
        self.host = host
        self.ctl = ctl
        self.repo = Path(repo)
        self.state_dir = Path(state_dir)
        self.allow_install = allow_install
        self.log_path = Path(log_path) if log_path is not None else None
        self.sleep = sleep
        self._secret_values: set[str] = set()

    def log(self, msg: str) -> None:
        text = str(msg)
        for value in sorted(self._secret_values, key=len, reverse=True):
            if value:
                text = text.replace(value, "***")
        stamp = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
        line = f"{stamp} {text}\n"
        sys.stderr.write(line)
        if self.log_path is not None:
            self.log_path.parent.mkdir(parents=True, exist_ok=True)
            with self.log_path.open("a", encoding="utf-8") as handle:
                handle.write(line)

    def _host_argv(self, *args: str) -> list[str]:
        return [
            "powershell", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
            "-File", self.lab["host"]["bench_dir"].rstrip("\\/") + "\\host\\bench.ps1",
            *[str(arg) for arg in args],
        ]

    def host_ps(self, *args: str, stdin: str | None = None, timeout: float = 120.0) -> dict:
        command = args[0] if args else ""
        reply = self.host.run(
            self._host_argv(*args), input=stdin if stdin is not None else "",
            timeout=timeout, check=False,
        )
        lines = [line.strip() for line in reply.out.splitlines() if line.strip()]
        try:
            result = json.loads(lines[-1].lstrip("\ufeff")) if lines else None
        except (ValueError, TypeError):
            result = None
        if not isinstance(result, dict):
            raise BenchError(f"bench.ps1 {command}: bad output: {reply.out[-500:]}")
        if not result.get("ok"):
            raise BenchError(str(result.get("error") or f"bench.ps1 {command} failed"))
        if reply.code:
            raise BenchError(f"bench.ps1 {command}: exit {reply.code}: {reply.err[-500:]}")
        return result

    def host_cat(self, path: str) -> str:
        reply = self.host.run(self._host_argv("cat", "-Path", path), input="", check=False)
        if reply.code:
            raise BenchError(f"bench.ps1 cat: exit {reply.code}: {reply.err[-500:]}")
        return reply.out

    def host_put(self, local: Path, remote_rel: str) -> str:
        relative = PureWindowsPath(remote_rel)
        if relative.is_absolute() or relative.drive or relative.root or ".." in relative.parts:
            raise BenchError("host_put requires a path relative to bench_dir")
        remote = str(PureWindowsPath(self.lab["host"]["bench_dir"]) / relative)
        self.host.put(Path(local), remote.replace("\\", "/"))
        return remote

    def gui(self, argv: list[str], *, timeout: float = 60.0, detach: bool = False) -> CmdResult:
        command = [str(self.repo / "tools/bench/mac/gui-run.sh"), "--timeout", str(int(timeout))]
        if detach:
            command.append("--detach")
        command += ["--cwd", str(self.repo), "--", *[str(arg) for arg in argv]]
        return self.ctl.run(command, timeout=timeout + 15, check=False)

    def secret(self, name: str) -> str:
        self.state_dir.mkdir(parents=True, exist_ok=True, mode=0o700)
        self.state_dir.chmod(0o700)
        path = self.state_dir / "secrets.json"
        descriptor = os.open(str(path), os.O_RDWR | os.O_CREAT, 0o600)
        with os.fdopen(descriptor, "r+", encoding="utf-8") as handle:
            os.fchmod(handle.fileno(), 0o600)
            raw = handle.read()
            try:
                values = json.loads(raw) if raw.strip() else {}
            except ValueError as exc:
                raise BenchError(f"invalid secrets file: {path}") from exc
            if not isinstance(values, dict) or any(not isinstance(value, str) for value in values.values()):
                raise BenchError(f"invalid secrets file: {path}")
            if name not in values:
                alphabet = string.ascii_letters + string.digits
                values[name] = "".join(secrets.choice(alphabet) for _ in range(16))
                handle.seek(0)
                json.dump(values, handle, indent=2)
                handle.write("\n")
                handle.truncate()
            self._secret_values.update(values.values())
            return values[name]

    def download(self, item_id: str) -> Path:
        if not self.allow_install:
            raise BenchError("install not allowed; rerun with --yes")
        try:
            with (self.repo / "tools/bench/thirdparty.json").open(encoding="utf-8") as handle:
                item = json.load(handle).get("items", {}).get(item_id)
        except (OSError, ValueError) as exc:
            raise BenchError(f"cannot read thirdparty.json: {exc}") from exc
        if not isinstance(item, dict) or not item.get("sha256"):
            raise BenchError(f"sha256 not set for {item_id}: download and verify it first (see README)")
        expected = str(item["sha256"]).lower()
        filename = item.get("file")
        if not isinstance(filename, str) or Path(filename).name != filename or filename in ("", ".", ".."):
            raise BenchError(f"invalid download filename for {item_id}")
        cache = Path(os.environ.get("RS_BENCH_CACHE", "~/.cache/rs-bench/downloads")).expanduser()
        cache.mkdir(parents=True, exist_ok=True)
        destination = cache / filename

        def digest(path: Path) -> str:
            checksum = hashlib.sha256()
            with path.open("rb") as handle:
                for chunk in iter(lambda: handle.read(1024 * 1024), b""):
                    checksum.update(chunk)
            return checksum.hexdigest()

        if destination.is_file() and digest(destination) == expected:
            return destination
        descriptor, name = tempfile.mkstemp(prefix=filename + ".", suffix=".tmp", dir=str(cache))
        os.close(descriptor)
        temporary = Path(name)
        try:
            self.ctl.run(["curl", "-fL", "--retry", "2", "-o", str(temporary), item["url"]], timeout=1800)
            actual = digest(temporary)
            if actual != expected:
                raise BenchError(f"sha256 mismatch for {item_id}: expected {expected}, actual {actual}")
            temporary.replace(destination)
        finally:
            if temporary.exists():
                temporary.unlink()
        return destination


class Adapter(abc.ABC):
    id: str
    label: str
    ports: tuple = ()
    host_processes: tuple = ()
    needs_host: bool = True

    @abc.abstractmethod
    def settings(self) -> dict:
        raise NotImplementedError

    @abc.abstractmethod
    def status(self, ctx: Ctx) -> dict:
        raise NotImplementedError

    @abc.abstractmethod
    def install(self, ctx: Ctx) -> None:
        raise NotImplementedError

    @abc.abstractmethod
    def uninstall(self, ctx: Ctx) -> None:
        raise NotImplementedError

    @abc.abstractmethod
    def host_start(self, ctx: Ctx) -> None:
        raise NotImplementedError

    @abc.abstractmethod
    def host_stop(self, ctx: Ctx) -> None:
        raise NotImplementedError

    @abc.abstractmethod
    def client_start(self, ctx: Ctx, cell: Cell, *, seconds: int) -> None:
        raise NotImplementedError

    @abc.abstractmethod
    def client_stop(self, ctx: Ctx, cell: Cell) -> None:
        raise NotImplementedError

    @abc.abstractmethod
    def window(self, ctx: Ctx) -> dict:
        raise NotImplementedError
