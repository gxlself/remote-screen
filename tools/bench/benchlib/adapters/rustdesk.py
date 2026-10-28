from __future__ import annotations

import json
import os
import shutil
from pathlib import Path

from benchlib.adapters.base import Adapter, BenchError, Cell, Ctx


_configured_app_dir = os.environ.get("RS_BENCH_APP_DIR")
APP_DIR = (
    Path(_configured_app_dir).expanduser()
    if _configured_app_dir
    else Path.home() / "Applications" / "rs-bench"
)
APP = APP_DIR / "RustDesk.app"
BUNDLE = "com.carriez.rustdesk"
PASSWORD_SECRET = "rustdesk-password"

HOST_OPTIONS = {
    "direct-server": "Y",
    "direct-access-port": "21118",
    "approve-mode": "password",
    "verification-method": "use-permanent-password",
    "custom-rendezvous-server": "127.0.0.1",
}


def CLIENT_CONNECT_ARGS(addr: str, password: str) -> list[str]:
    """Return the RustDesk client arguments for an IP/password connection."""

    return ["--connect", str(addr), "--password", str(password)]


def _app_paths() -> tuple[Path, Path]:
    """Return the current application paths, including the test override."""

    override = os.environ.get("RS_BENCH_APP_DIR")
    if override:
        directory = Path(override).expanduser()
        return directory, directory / "RustDesk.app"
    return APP_DIR, APP


def _manifest_sha256(ctx: Ctx, item_id: str) -> str:
    path = ctx.repo / "tools" / "bench" / "thirdparty.json"
    try:
        with path.open(encoding="utf-8") as handle:
            manifest = json.load(handle)
        value = manifest["items"][item_id]["sha256"]
    except (KeyError, OSError, TypeError, ValueError) as exc:
        raise BenchError(f"cannot read sha256 for {item_id}: {exc}") from exc
    if not isinstance(value, str) or not value:
        raise BenchError(f"sha256 not set for {item_id}: download and verify it first (see README)")
    return value


def _remove_path(path: Path) -> None:
    if path.is_dir() and not path.is_symlink():
        shutil.rmtree(path)
    elif path.exists() or path.is_symlink():
        path.unlink()


class RustDeskAdapter(Adapter):
    id = "rustdesk"
    label = "RustDesk"
    ports = (21118,)
    host_processes = ("rustdesk",)
    needs_host = True

    def settings(self) -> dict:
        return {
            "resolution": "1920x1080",
            "fps": "default",
            "codec": "auto",
            "quality": "balanced",
            "connection": "direct-ip",
        }

    def status(self, ctx: Ctx) -> dict:
        host = ctx.host_ps("product", "-Id", self.id, "-Action", "status")
        _, app = _app_paths()
        client_installed = app.exists()
        client_version = None
        if client_installed:
            reply = ctx.ctl.run(
                [
                    "plutil",
                    "-extract",
                    "CFBundleShortVersionString",
                    "raw",
                    str(app / "Contents" / "Info.plist"),
                ],
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
        }

    def install(self, ctx: Ctx) -> None:
        host_package = ctx.download("rustdesk-win")
        remote = ctx.host_put(host_package, "downloads\\" + host_package.name)
        install_params = {
            "msi": remote,
            "sha256": _manifest_sha256(ctx, "rustdesk-win"),
        }
        ctx.host_ps(
            "product",
            "-Id",
            self.id,
            "-Action",
            "install",
            stdin=json.dumps(install_params, separators=(",", ":")),
        )

        password = ctx.secret(PASSWORD_SECRET)
        ctx.host_ps(
            "product",
            "-Id",
            self.id,
            "-Action",
            "configure",
            stdin=json.dumps(
                {"password": password, "options": HOST_OPTIONS},
                separators=(",", ":"),
            ),
        )

        client_package = ctx.download("rustdesk-mac")
        app_dir, app = _app_paths()
        app_dir.mkdir(parents=True, exist_ok=True)
        _remove_path(app)

        mountpoint = ctx.state_dir / "mnt-rustdesk"
        mountpoint.mkdir(parents=True, exist_ok=True)
        try:
            ctx.ctl.run(
                [
                    "hdiutil",
                    "attach",
                    "-nobrowse",
                    "-readonly",
                    "-mountpoint",
                    str(mountpoint),
                    str(client_package),
                ]
            )
            ctx.ctl.run(["ditto", str(mountpoint / "RustDesk.app"), str(app)])
        finally:
            try:
                ctx.ctl.run(["hdiutil", "detach", str(mountpoint)], check=False)
            except BenchError:
                # Keep the original install error when an unmount command is unavailable.
                pass

    def uninstall(self, ctx: Ctx) -> None:
        ctx.host_ps("product", "-Id", self.id, "-Action", "uninstall")
        self.client_stop(ctx, None)  # type: ignore[arg-type]
        _, app = _app_paths()
        _remove_path(app)

    def host_start(self, ctx: Ctx) -> None:
        ctx.host_ps("product", "-Id", self.id, "-Action", "start")

    def host_stop(self, ctx: Ctx) -> None:
        ctx.host_ps("product", "-Id", self.id, "-Action", "stop")

    def client_start(self, ctx: Ctx, cell: Cell, *, seconds: int) -> None:
        del cell, seconds
        _, app = _app_paths()
        if not app.exists():
            raise BenchError("RustDesk client not installed: run bench.py install rustdesk --yes")
        password = ctx.secret(PASSWORD_SECRET)
        reply = ctx.gui(
            [
                "/usr/bin/open",
                "-n",
                "-a",
                str(app),
                "--args",
                *CLIENT_CONNECT_ARGS(ctx.lab["host"]["addr"], password),
            ],
            timeout=30,
        )
        if reply.code:
            raise BenchError(f"RustDesk client start failed (exit {reply.code})")

    def client_stop(self, ctx: Ctx, cell: Cell) -> None:
        del cell
        try:
            ctx.gui(
                ["/usr/bin/osascript", "-e", f'tell application id "{BUNDLE}" to quit'],
                timeout=20,
            )
        except BenchError:
            pass
        ctx.sleep(3)
        ctx.ctl.run(["/usr/bin/pkill", "-x", "RustDesk"], check=False)

    def window(self, ctx: Ctx) -> dict:
        del ctx
        return {"app": BUNDLE, "title": None}


def create() -> Adapter:
    return RustDeskAdapter()
