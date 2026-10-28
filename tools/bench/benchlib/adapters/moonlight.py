from __future__ import annotations

import hashlib
import json
import os
import re
import secrets
import shutil
from pathlib import Path

from benchlib.adapters.base import Adapter, BenchError, Cell, Ctx


def _application_dir() -> Path:
    configured = os.environ.get("RS_BENCH_APP_DIR")
    if configured:
        return Path(configured).expanduser()
    return Path.home() / "Applications" / "rs-bench"


APP_DIR = _application_dir()
APP = APP_DIR / "Moonlight.app"
BIN = APP / "Contents" / "MacOS" / "Moonlight"
BUNDLE = "com.moonlight-stream.Moonlight"
WEB_USER = "rsbench"
WEB_SECRET = "sunshine-web"
APP_NAME = "Desktop"
STREAM_ARGS = (
    "--resolution",
    "1920x1080",
    "--fps",
    "60",
    "--bitrate",
    "20000",
    "--video-codec",
    "HEVC",
    "--display-mode",
    "windowed",
    "--no-vsync",
    "--no-frame-pacing",
)


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _manifest_sha256(ctx: Ctx, item_id: str, downloaded: Path) -> str:
    """Return the verified digest without making the manifest a new API."""
    try:
        manifest = json.loads((ctx.repo / "tools/bench/thirdparty.json").read_text(encoding="utf-8"))
        value = manifest.get("items", {}).get(item_id, {}).get("sha256")
    except (OSError, TypeError, ValueError):
        value = None
    return str(value).lower() if value else _sha256(downloaded)


def _product(ctx: Ctx, action: str, params: dict | None = None) -> dict:
    stdin = json.dumps(params, separators=(",", ":")) if params is not None else None
    return ctx.host_ps("product", "-Id", "moonlight", "-Action", action, stdin=stdin)


def _gui_path(ctx: Ctx) -> str:
    return str(ctx.repo / "tools/bench/mac/gui-run.sh")


def _remove_path(path: Path) -> None:
    if path.is_dir() and not path.is_symlink():
        shutil.rmtree(path)
    elif path.exists() or path.is_symlink():
        path.unlink()


class MoonlightAdapter(Adapter):
    id = "moonlight"
    label = "Moonlight + Sunshine"
    ports = (47984, 47989, 48010, 47998, 47999, 48000, 48002)
    host_processes = ("sunshine",)

    def __init__(self):
        self._client_labels: dict[str, str] = {}

    def settings(self) -> dict:
        return {
            "resolution": "1920x1080",
            "fps": 60,
            "codec": "HEVC",
            "bitrate_kbps": 20000,
            "vsync": False,
            "frame_pacing": False,
        }

    def window(self, ctx: Ctx) -> dict:
        del ctx
        return {"app": BUNDLE, "title": None}

    def status(self, ctx: Ctx) -> dict:
        host = _product(ctx, "status")
        client_version = None
        client_installed = APP.is_dir()
        if client_installed:
            plist = APP / "Contents" / "Info.plist"
            reply = ctx.ctl.run(
                ["plutil", "-extract", "CFBundleShortVersionString", "raw", "-o", "-", str(plist)],
                check=False,
            )
            if reply.code == 0:
                value = reply.out.strip()
                client_version = value or None
        return {
            "host_installed": bool(host.get("installed")),
            "client_installed": client_installed,
            "host_version": host.get("version"),
            "client_version": client_version,
            "extra": {"paired_clients": host.get("paired_clients")},
        }

    def host_start(self, ctx: Ctx) -> None:
        _product(ctx, "start")

    def host_stop(self, ctx: Ctx) -> None:
        _product(ctx, "stop")

    def install(self, ctx: Ctx) -> None:
        sunshine = ctx.download("sunshine-win")
        remote_msi = ctx.host_put(sunshine, "downloads\\" + sunshine.name)
        _product(
            ctx,
            "install",
            {"msi": remote_msi, "sha256": _manifest_sha256(ctx, "sunshine-win", sunshine)},
        )
        _product(ctx, "configure", {"user": WEB_USER, "password": ctx.secret(WEB_SECRET)})
        self.host_start(ctx)

        moonlight = ctx.download("moonlight-mac")
        APP_DIR.mkdir(parents=True, exist_ok=True)
        _remove_path(APP)
        mounted = None
        try:
            reply = ctx.ctl.run(["hdiutil", "attach", "-nobrowse", "-readonly", str(moonlight)])
            mounted = self._mountpoint(reply.out)
            ctx.ctl.run(["ditto", str(Path(mounted) / "Moonlight.app"), str(APP)])
        finally:
            if mounted is not None:
                ctx.ctl.run(["hdiutil", "detach", mounted], check=False)

        self.pair(ctx)

    @staticmethod
    def _mountpoint(output: str) -> str:
        for line in reversed(output.splitlines()):
            matches = re.findall(r"(/Volumes/[^\s]+(?:\s[^\s]+)*)", line)
            if matches:
                return matches[-1].strip()
            fields = line.split()
            for field in reversed(fields):
                if field.startswith("/Volumes/"):
                    return field
        return "/Volumes/Moonlight"

    def _pin_path(self, ctx: Ctx) -> Path:
        ctx.state_dir.mkdir(parents=True, exist_ok=True, mode=0o700)
        ctx.state_dir.chmod(0o700)
        return ctx.state_dir / "moonlight-pin.txt"

    def _pin(self, ctx: Ctx) -> str:
        path = self._pin_path(ctx)
        if path.exists():
            try:
                value = path.read_text(encoding="ascii").strip()
            except (OSError, UnicodeError) as exc:
                raise BenchError(f"cannot read Moonlight PIN: {path}") from exc
            if not re.fullmatch(r"\d{4}", value):
                raise BenchError(f"invalid Moonlight PIN file: {path}")
            path.chmod(0o600)
            return value

        value = f"{secrets.randbelow(10000):04d}"
        descriptor = os.open(str(path), os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        try:
            with os.fdopen(descriptor, "w", encoding="ascii") as handle:
                handle.write(value + "\n")
        except Exception:
            try:
                path.unlink()
            except OSError:
                pass
            raise
        path.chmod(0o600)
        return value

    def pair(self, ctx: Ctx) -> None:
        pin = self._pin(ctx)
        host_address = str(ctx.lab["host"]["addr"])
        reply = ctx.gui([str(BIN), "pair", host_address, "--pin", pin], detach=True)
        label = reply.out.strip()
        stop_label = bool(label)
        failure = "moonlight pairing failed; pair once by hand in the Moonlight app, PIN entry via Sunshine web UI"
        try:
            if reply.code or not re.fullmatch(r"[A-Za-z0-9_.-]+", label):
                raise BenchError(failure)
            ctx.sleep(3)
            _product(
                ctx,
                "pin",
                {
                    "pin": pin,
                    "name": "rs-bench",
                    "user": WEB_USER,
                    "password": ctx.secret(WEB_SECRET),
                },
            )
            waited = ctx.ctl.run([_gui_path(ctx), "--wait", label, "--timeout", "60"], check=False)
            if waited.code:
                raise BenchError(failure)
        except (BenchError, OSError, ValueError, TypeError) as exc:
            if stop_label:
                ctx.ctl.run([_gui_path(ctx), "--stop", label], check=False)
            if isinstance(exc, BenchError) and str(exc) == failure:
                raise
            raise BenchError(failure) from exc

    def uninstall(self, ctx: Ctx) -> dict:
        host = _product(ctx, "uninstall")
        if host.get("vigem_present"):
            ctx.log("ViGEmBus driver remains after Sunshine uninstall")
        try:
            ctx.gui([str(BIN), "quit", str(ctx.lab["host"]["addr"])], timeout=30)
        except (BenchError, OSError):
            pass
        ctx.ctl.run(["pkill", "-x", "Moonlight"], check=False)
        if APP.exists():
            try:
                _remove_path(APP)
            except OSError as exc:
                raise BenchError(f"cannot remove Moonlight.app: {exc}") from exc
        result = {"host": host, "vigem_present": bool(host.get("vigem_present"))}
        if result["vigem_present"]:
            result["warning"] = "ViGEmBus driver remains"
        return result

    def client_start(self, ctx: Ctx, cell: Cell, *, seconds: int) -> None:
        del seconds
        if not APP.is_dir():
            raise BenchError(f"Moonlight.app not found: {APP}")
        host_address = str(ctx.lab["host"]["addr"])
        reply = ctx.gui(
            [str(BIN), "stream", host_address, APP_NAME, *STREAM_ARGS],
            detach=True,
        )
        if reply.code:
            raise BenchError(f"Moonlight stream failed: exit {reply.code}")
        label = reply.out.strip()
        if label:
            self._client_labels[cell.run_id] = label

    def client_stop(self, ctx: Ctx, cell: Cell) -> None:
        host_address = str(ctx.lab["host"]["addr"])
        try:
            ctx.gui([str(BIN), "quit", host_address], timeout=30)
        except (BenchError, OSError):
            pass
        label = self._client_labels.pop(cell.run_id, None)
        if label:
            ctx.ctl.run([_gui_path(ctx), "--stop", label], check=False)
        ctx.ctl.run(["pkill", "-x", "Moonlight"], check=False)


def create() -> Adapter:
    return MoonlightAdapter()
