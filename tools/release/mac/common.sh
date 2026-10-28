#!/usr/bin/env bash
# Shared by the ad-hoc package builder and the Developer ID release workflow.

RS_MAC_REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd -P)"
RS_MAC_APP_NAME='Remote Host.app'
RS_MAC_UI_ID='ai.gxlself.remotehost'
RS_MAC_AGENT_ID='ai.gxlself.remotehost.agent'

log() { printf '[mac-release] %s\n' "$*" >&2; }
warn() { log "WARNING: $*"; }
die() { log "ERROR: $*"; exit 1; }

require_command() {
    command -v "$1" >/dev/null 2>&1 || die "Required command not found: $1"
}

require_macos() {
    [[ "$(uname -s)" == Darwin ]] || die 'This script requires macOS.'
}

read_version() {
    /usr/bin/plutil -extract version raw -o - \
        "${1:-$RS_MAC_REPO_ROOT}/host-ui/src-tauri/tauri.conf.json"
}

read_workspace_version() {
    awk '
        /^\[workspace.package\]$/ { in_package = 1; next }
        /^\[/ { in_package = 0 }
        in_package && /^version[[:space:]]*=/ {
            sub(/^[^"]*"/, ""); sub(/".*$/, ""); print; exit
        }
    ' "${1:-$RS_MAC_REPO_ROOT}/Cargo.toml"
}

# verify_structure <Remote Host.app> [native|universal]
# Deliberately does not verify signatures: H18B can use this before signing too.
verify_structure() {
    local app="$1" mode="${2:-native}" executable agent plist
    [[ "$mode" == native || "$mode" == universal ]] || die "Unknown bundle architecture mode: $mode"
    executable="$app/Contents/MacOS/Remote Host"
    agent="$app/Contents/MacOS/rs-host-agent"
    plist="$app/Contents/Library/LaunchAgents/$RS_MAC_AGENT_ID.plist"
    [[ -x "$executable" && -x "$agent" ]] || die "Missing UI or agent executable in $app"
    [[ -f "$plist" ]] || die "Missing bundled LaunchAgent plist: $plist"
    /usr/bin/plutil -lint "$plist" >&2 || die 'LaunchAgent plist is invalid.'
    /usr/bin/plutil -lint "$app/Contents/Info.plist" >&2 || die 'App Info.plist is invalid.'

    python3 - "$app" "$mode" <<'PY' || return 1
import pathlib
import plistlib
import subprocess
import sys

app = pathlib.Path(sys.argv[1])
mode = sys.argv[2]
try:
    with (app / "Contents/Info.plist").open("rb") as source:
        info = plistlib.load(source)
    expected = {
        "LSUIElement": True,
        "LSMinimumSystemVersion": "13.0",
        "CFBundleIdentifier": "ai.gxlself.remotehost",
        "CFBundleExecutable": "Remote Host",
    }
    for key, value in expected.items():
        if info.get(key) != value:
            raise ValueError(f"Info.plist {key}: expected {value!r}, got {info.get(key)!r}")

    with (app / "Contents/Library/LaunchAgents/ai.gxlself.remotehost.agent.plist").open("rb") as source:
        launch_agent = plistlib.load(source)
    expected = {
        "Label": "ai.gxlself.remotehost.agent",
        "BundleProgram": "Contents/MacOS/rs-host-agent",
        "AssociatedBundleIdentifiers": ["ai.gxlself.remotehost"],
        "RunAtLoad": True,
        "KeepAlive": {"SuccessfulExit": False},
        "LimitLoadToSessionType": "Aqua",
        "ProcessType": "Interactive",
        "ThrottleInterval": 10,
    }
    for key, value in expected.items():
        if launch_agent.get(key) != value:
            raise ValueError(f"LaunchAgent {key}: expected {value!r}, got {launch_agent.get(key)!r}")

    agent = app / "Contents/MacOS/rs-host-agent"
    for binary in (app / "Contents/MacOS/Remote Host", agent):
        archs = subprocess.check_output(["lipo", "-archs", str(binary)], text=True).split()
        if not archs:
            raise ValueError(f"No Mach-O architectures found in {binary}")
        if mode == "universal" and not {"arm64", "x86_64"}.issubset(archs):
            raise ValueError(f"{binary} is not universal: {archs}")
        if binary != agent:
            continue
        # otool -P reads __TEXT,__info_plist; check every slice, not a string
        # elsewhere in the executable (which would miss a broken embedded plist).
        for arch in archs:
            output = subprocess.check_output(
                ["otool", "-arch", arch, "-X", "-P", str(agent)], text=True
            )
            start = output.find("<?xml")
            end = output.find("</plist>", start)
            if start < 0 or end < 0:
                raise ValueError(f"Agent has no embedded Info.plist for {arch}")
            embedded = plistlib.loads(output[start:end + len("</plist>")].encode())
            if embedded.get("CFBundleIdentifier") != "ai.gxlself.remotehost.agent":
                raise ValueError(f"Incorrect embedded agent identifier for {arch}")
            if embedded.get("CFBundleName") != "Remote Host":
                raise ValueError(f"Incorrect embedded agent display name for {arch}")
except (OSError, ValueError, subprocess.CalledProcessError, plistlib.InvalidFileException) as error:
    print(f"[mac-release] ERROR: {error}", file=sys.stderr)
    sys.exit(1)
PY
    log "Bundle structure verified ($mode): $app"
}
