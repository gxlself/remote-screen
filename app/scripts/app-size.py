#!/usr/bin/env python3
"""Logical on-disk .app size, excluding symlinks; optional full/base delta."""

import json
import pathlib
import plistlib
import sys


def measure(argument):
    app = pathlib.Path(argument).resolve(strict=True)
    with (app / "Info.plist").open("rb") as info_file:
        info = plistlib.load(info_file)
    total = sum(
        file.stat().st_size
        for file in app.rglob("*")
        if not file.is_symlink() and file.is_file()
    )
    return {
        "app": str(app),
        "bundleId": info["CFBundleIdentifier"],
        "platform": info["DTPlatformName"],
        "bytes": total,
        "MiB": round(total / 1024**2, 3),
        "executableBytes": (app / info["CFBundleExecutable"]).stat().st_size,
        "embeddedJS": (app / "main.jsbundle").is_file(),
    }


if len(sys.argv) not in (2, 3):
    sys.exit("usage: app-size.py FULL.app [BASELINE.app]")
sizes = [measure(argument) for argument in sys.argv[1:]]
result = {"apps": sizes}
if len(sizes) == 2:
    delta = sizes[0]["bytes"] - sizes[1]["bytes"]
    result.update(deltaBytes=delta, deltaMiB=round(delta / 1024**2, 3))
print(json.dumps(result, indent=2))
