#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
repo_root="$(cd "$script_dir/../../.." && pwd -P)"
source "$script_dir/common.sh"

require_macos
for tool in ditto python3 mktemp; do
    require_command "$tool"
done

out_root="$repo_root/target"
mkdir -p "$out_root"
work="$(mktemp -d "$out_root/.mac-release-test.XXXXXX")"
cleanup() { rm -rf -- "$work"; }
trap cleanup EXIT
app="$work/$RS_MAC_APP_NAME"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Library/LaunchAgents"

cat > "$app/Contents/MacOS/Remote Host" <<'SCRIPT'
#!/bin/sh
exit 0
SCRIPT
cat > "$app/Contents/MacOS/rs-host-agent" <<'SCRIPT'
#!/bin/sh
exit 0
SCRIPT
chmod 755 "$app/Contents/MacOS/Remote Host" "$app/Contents/MacOS/rs-host-agent"

python3 - "$app" <<'PY'
import pathlib
import plistlib
import sys
app = pathlib.Path(sys.argv[1])
info = {
    "LSUIElement": True,
    "LSMinimumSystemVersion": "13.0",
    "CFBundleIdentifier": "ai.gxlself.remotehost",
    "CFBundleExecutable": "Remote Host",
    "CFBundleShortVersionString": "0.0.1-test",
    "CFBundleVersion": "0.0.1-test",
}
launch_agent = {
    "Label": "ai.gxlself.remotehost.agent",
    "BundleProgram": "Contents/MacOS/rs-host-agent",
    "AssociatedBundleIdentifiers": ["ai.gxlself.remotehost"],
    "RunAtLoad": True,
    "KeepAlive": {"SuccessfulExit": False},
    "LimitLoadToSessionType": "Aqua",
    "ProcessType": "Interactive",
    "ThrottleInterval": 10,
}
with (app / "Contents/Info.plist").open("wb") as target:
    plistlib.dump(info, target)
with (app / "Contents/Library/LaunchAgents/ai.gxlself.remotehost.agent.plist").open("wb") as target:
    plistlib.dump(launch_agent, target)
PY

# A minimal script is not a Mach-O file. These wrappers make lipo/otool and
# codesign deterministic while preserving all release-script control flow.
fake_bin="$work/fake-bin"
mkdir -p "$fake_bin"
cat > "$fake_bin/codesign" <<'SCRIPT'
#!/usr/bin/env bash
set -euo pipefail
for arg in "$@"; do
    if [[ "$arg" == -dv || "$arg" == --display ]]; then
        printf 'Executable=Remote Host\n' >&2
        printf 'TeamIdentifier=not set\n' >&2
        exit 0
    fi
done
for arg in "$@"; do
    [[ "$arg" == --verify ]] && exit 0
done
last="${@: -1}"
if [[ -d "$last" ]]; then
    : > "$last/.fake-code-signature"
elif [[ -f "$last" ]]; then
    : > "$last"
fi
exit 0
SCRIPT
cat > "$fake_bin/lipo" <<'SCRIPT'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == -archs ]]; then
    printf 'x86_64\n'
    exit 0
fi
if [[ "${1:-}" == -create ]]; then
    last="${@: -1}"
    : > "$last"
    exit 0
fi
exit 0
SCRIPT
cat > "$fake_bin/otool" <<'SCRIPT'
#!/usr/bin/env bash
set -euo pipefail
cat <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>ai.gxlself.remotehost.agent</string>
<key>CFBundleName</key><string>Remote Host</string>
</dict></plist>
PLIST
SCRIPT
chmod 755 "$fake_bin/codesign" "$fake_bin/lipo" "$fake_bin/otool"
cat > "$fake_bin/hdiutil" <<'SCRIPT'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == verify ]]; then
    exit 0
fi
if [[ "${1:-}" == create ]]; then
    source_dir=''
    previous=''
    for arg in "$@"; do
        if [[ "$previous" == -srcfolder ]]; then
            source_dir="$arg"
        fi
        previous="$arg"
    done
    output="${@: -1}"
    [[ -n "$source_dir" && -d "$source_dir" ]] || exit 2
    /usr/bin/ditto -c -k --keepParent "$source_dir" "$output"
    exit 0
fi
exit 0
SCRIPT
chmod 755 "$fake_bin/codesign" "$fake_bin/lipo" "$fake_bin/otool" "$fake_bin/hdiutil"

export PATH="$fake_bin:$PATH"
unset RS_MAC_SIGN_IDENTITY RS_NOTARY_PROFILE RS_NOTARY_KEY RS_NOTARY_KEY_ID RS_NOTARY_ISSUER

log 'Testing ad-hoc sign, DMG, update zip, and verification with a minimal fake app'
"$script_dir/sign.sh" "$app" --out "$work"
"$script_dir/dmg.sh" "$app" --out "$work" --arch native
"$script_dir/update-zip.sh" "$app" --out "$work" --arch native

dmg="$work/RemoteHost-0.0.1-test.dmg"
update_zip="$work/RemoteHost-0.0.1-test-mac-native.zip"
"$script_dir/verify.sh" --app "$app" --dmg "$dmg" --update-zip "$update_zip" --out "$work" --arch native

[[ -f "$dmg" ]] || die 'test: DMG was not created.'
[[ -f "$update_zip" ]] || die 'test: update zip was not created.'
[[ -f "$work/release.json" ]] || die 'test: release.json was not created.'
python3 - "$work/release.json" "$update_zip" "$work/.mac-sign.json" <<'PY'
import hashlib
import json
import pathlib
import sys
manifest = json.load(open(sys.argv[1]))
zip_path = pathlib.Path(sys.argv[2])
sign = json.load(open(sys.argv[3]))
actual = hashlib.sha256(zip_path.read_bytes()).hexdigest()
if manifest["sha256"] != actual:
    raise SystemExit("test: release.json sha256 mismatch")
if manifest["size"] != zip_path.stat().st_size:
    raise SystemExit("test: release.json size mismatch")
if sign.get("signed") is not False or manifest.get("team_id") is not None:
    raise SystemExit("test: ad-hoc release was marked as Developer ID signed")
PY

# SEC-11 regression: a changed update byte must fail before extraction.
backup_zip="$work/update-original.zip"
ditto "$update_zip" "$backup_zip"
printf 'tamper' >> "$update_zip"
set +e
tamper_output="$("$script_dir/verify.sh" --app "$app" --dmg "$dmg" --update-zip "$update_zip" --out "$work" --arch native 2>&1)"
tamper_status=$?
set -e
ditto "$backup_zip" "$update_zip"
[[ "$tamper_status" -ne 0 ]] || die 'test: tampered update zip was accepted.'
[[ "$tamper_output" == *'sha256'* ]] || die 'test: tampered update rejection did not mention sha256.'

# Exercise the public entry point with the same fake app and no credentials.
entry_out="$work/entry"
mkdir -p "$entry_out"
ditto "$app" "$entry_out/$RS_MAC_APP_NAME"
entry_output="$("$script_dir/../mac.sh" --skip-build --arch native --out "$entry_out" 2>&1)"
printf '%s\n' "$entry_output" | tail -n 1 | python3 -c 'import json, sys; value=json.load(sys.stdin); assert value["signed"] is False; assert value["notarized"] is False; assert value["stapled"] is False'

# Credential values are deliberately synthetic. The output may say set, but
# must never echo the values themselves.
secret_identity='fake-identity-do-not-print'
secret_profile='fake-profile-do-not-print'
secret_output="$(RS_MAC_SIGN_IDENTITY="$secret_identity" "$script_dir/sign.sh" "$app" --out "$work/secret" 2>&1)"
[[ "$secret_output" != *"$secret_identity"* ]] || die 'test: signing identity leaked to output.'
secret_output="$(RS_NOTARY_PROFILE="$secret_profile" "$script_dir/notarize.sh" app "$app" --out "$work" --skip-notarize 2>&1)"
[[ "$secret_output" != *"$secret_profile"* ]] || die 'test: notary profile leaked to output.'

log 'mac release script self-test passed (no credentials, no network)'
