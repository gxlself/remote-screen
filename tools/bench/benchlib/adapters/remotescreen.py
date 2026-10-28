from __future__ import annotations

import hashlib
import json
from pathlib import Path

from benchlib.adapters.base import Adapter, BenchError, Cell, Ctx
from benchlib.executor import CmdResult


APP_ID = "ai.gxlself.remotescreen.dev"


def _simctl(ctx: Ctx, *args: str, check: bool = True, timeout: float = 120.0) -> CmdResult:
    return ctx.ctl.run(["xcrun", "simctl", *args], check=check, timeout=timeout)


def _device_udid(ctx: Ctx) -> str:
    name = str(ctx.lab["controller"]["sim_device"])
    reply = _simctl(ctx, "list", "devices", "available", "-j", check=False)
    if reply.code:
        detail = reply.err.strip() or f"simctl list exited {reply.code}"
        raise BenchError(f"cannot list simulator devices: {detail}")
    try:
        payload = json.loads(reply.out)
    except (TypeError, ValueError) as exc:
        raise BenchError("simctl list devices returned invalid JSON") from exc
    devices = payload.get("devices") if isinstance(payload, dict) else None
    matches = []
    if isinstance(devices, dict):
        for runtime_devices in devices.values():
            if not isinstance(runtime_devices, list):
                continue
            for device in runtime_devices:
                if not isinstance(device, dict) or device.get("name") != name:
                    continue
                udid = device.get("udid")
                if isinstance(udid, str) and udid:
                    matches.append((device.get("state") == "Booted", udid))
    if not matches:
        raise BenchError(f"simulator device not found: {name}")
    matches.sort(key=lambda item: (not item[0], item[1]))
    return matches[0][1]


class RemoteScreenAdapter(Adapter):
    id = "remotescreen"
    label = "本产品"
    ports = (7400,)
    host_processes = ("rs-host", "rs-host-session")

    def settings(self) -> dict:
        return {
            "resolution": "1920x1080",
            "fps": 60,
            "codec": "default",
            "client": "ios-simulator",
        }

    def status(self, ctx: Ctx) -> dict:
        host = ctx.host_ps("product", "-Id", self.id, "-Action", "status")
        result = {
            "host_installed": bool(host.get("installed")),
            "host_version": host.get("version"),
            "client_installed": False,
            "client_version": None,
        }
        try:
            udid = _device_udid(ctx)
        except BenchError as exc:
            ctx.log(str(exc))
            return result
        app = _simctl(ctx, "get_app_container", udid, APP_ID, "app", check=False)
        if app.code:
            return result
        app_path = app.out.strip()
        if not app_path:
            return result
        result["client_installed"] = True
        plist = str(Path(app_path) / "Info.plist")
        version = ctx.ctl.run(
            ["/usr/bin/plutil", "-extract", "CFBundleShortVersionString", "raw", plist],
            check=False,
        )
        if version.code == 0:
            result["client_version"] = version.out.strip()
        return result

    @staticmethod
    def _sha256(path: Path) -> str:
        digest = hashlib.sha256()
        try:
            with path.open("rb") as handle:
                for chunk in iter(lambda: handle.read(1024 * 1024), b""):
                    digest.update(chunk)
        except OSError as exc:
            raise BenchError(f"cannot read Host MSI {path}: {exc}") from exc
        return digest.hexdigest()

    def install(self, ctx: Ctx) -> None:
        product = ctx.lab["products"][self.id]
        msi_value = product.get("host_msi")
        if msi_value is None:
            raise BenchError("set products.remotescreen.host_msi in lab.json, or install the Host by hand")
        msi = Path(msi_value).expanduser()
        if not msi.is_file():
            raise BenchError(f"Host MSI does not exist: {msi}")
        checksum = self._sha256(msi)
        remote_path = ctx.host_put(msi, "downloads\\" + msi.name)
        ctx.host_ps(
            "product",
            "-Id",
            self.id,
            "-Action",
            "install",
            stdin=json.dumps({"msi": remote_path, "sha256": checksum}),
        )

        sim_app = ctx.lab["controller"].get("sim_app")
        if sim_app is None:
            ctx.log("假定 App 已装好：controller.sim_app is null")
            return
        udid = _device_udid(ctx)
        _simctl(ctx, "install", udid, str(sim_app))

    def uninstall(self, ctx: Ctx) -> None:
        ctx.host_ps("product", "-Id", self.id, "-Action", "uninstall")
        sim_app = ctx.lab["controller"].get("sim_app")
        if sim_app is not None:
            udid = _device_udid(ctx)
            _simctl(ctx, "uninstall", udid, APP_ID)

    def host_start(self, ctx: Ctx) -> None:
        ctx.host_ps("product", "-Id", self.id, "-Action", "start")

    def host_stop(self, ctx: Ctx) -> None:
        ctx.host_ps("product", "-Id", self.id, "-Action", "stop")

    def client_start(self, ctx: Ctx, cell: Cell, *, seconds: int) -> None:
        del cell
        udid = _device_udid(ctx)
        _simctl(ctx, "bootstatus", udid, "-b", timeout=180)
        gui = ctx.gui(
            ["/usr/bin/open", "-a", "Simulator", "--args", "-CurrentDeviceUDID", udid],
            timeout=30,
        )
        if gui.code:
            raise BenchError(gui.err.strip() or f"could not open Simulator (exit {gui.code})")
        _simctl(ctx, "terminate", udid, APP_ID, check=False)
        launch = _simctl(
            ctx,
            "launch",
            udid,
            APP_ID,
            "-autoConnect",
            str(ctx.lab["host"]["addr"]),
            "-disconnectAfter",
            str(seconds),
            check=False,
        )
        if launch.code:
            raise BenchError(launch.err.strip() or f"simctl launch exited {launch.code}")

    def client_stop(self, ctx: Ctx, cell: Cell) -> None:
        del cell
        udid = _device_udid(ctx)
        _simctl(ctx, "terminate", udid, APP_ID, check=False)

    def window(self, ctx: Ctx) -> dict:
        return {"app": "com.apple.iphonesimulator", "title": ctx.lab["controller"]["sim_device"]}


def create() -> Adapter:
    return RemoteScreenAdapter()
