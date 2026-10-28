from __future__ import annotations

from pathlib import Path
from urllib.parse import quote

from benchlib.adapters.base import Adapter, BenchError, Cell, Ctx


class LocalAdapter(Adapter):
    id = "local"
    label = "本机基线"
    needs_host = False
    ports = ()
    host_processes = ()

    def settings(self) -> dict:
        return {"client": "safari", "window": "1600x900"}

    def status(self, ctx: Ctx) -> dict:
        reply = ctx.ctl.run(
            [
                "/usr/bin/defaults",
                "read",
                "/Applications/Safari.app/Contents/Info",
                "CFBundleShortVersionString",
            ],
            check=False,
        )
        installed = reply.code == 0
        return {
            "host_installed": True,
            "host_version": None,
            "client_installed": installed,
            "client_version": reply.out.strip() if installed else None,
        }

    def install(self, ctx: Ctx) -> None:
        del ctx

    def uninstall(self, ctx: Ctx) -> None:
        del ctx

    def host_start(self, ctx: Ctx) -> None:
        del ctx

    def host_stop(self, ctx: Ctx) -> None:
        del ctx

    def client_start(self, ctx: Ctx, cell: Cell, *, seconds: int) -> None:
        del seconds
        page = Path(ctx.repo).resolve() / "tools/bench/page/bench.html"
        raw_path = str(page)
        if '"' in raw_path or "\\" in raw_path:
            raise BenchError("Safari benchmark URL contains an unsafe path character")
        url = "file://" + quote(raw_path, safe="/") + "#" + cell.page_hash(delay=3)
        if '"' in url or "\\" in url:
            raise BenchError("Safari benchmark URL contains an unsafe character")
        reply = ctx.gui(
            [
                "/usr/bin/osascript",
                "-e",
                'tell application "Safari"',
                "-e",
                "activate",
                "-e",
                f'make new document with properties {{URL:"{url}"}}',
                "-e",
                "set bounds of front window to {0, 30, 1600, 930}",
                "-e",
                "end tell",
            ],
            timeout=30,
        )
        if reply.code:
            detail = reply.err.strip() or f"osascript exited {reply.code}"
            raise BenchError(
                "could not open Safari benchmark page; authorize osascript to control Safari "
                f"in System Settings > Privacy & Security > Automation: {detail}"
            )

    def client_stop(self, ctx: Ctx, cell: Cell) -> None:
        del cell
        try:
            reply = ctx.gui(
                [
                    "/usr/bin/osascript",
                    "-e",
                    'tell application "Safari" to close (every document whose URL contains '
                    '"tools/bench/page/bench.html")',
                ],
                timeout=30,
            )
            if reply.code:
                ctx.log(f"Safari cleanup exited {reply.code}: {reply.err.strip()}")
        except BenchError as exc:
            ctx.log(f"Safari cleanup failed: {exc}")

    def window(self, ctx: Ctx) -> dict:
        del ctx
        return {"app": "com.apple.Safari", "title": "RS Bench"}


def create() -> Adapter:
    return LocalAdapter()
