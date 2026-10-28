#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
source "$script_dir/common.sh"

usage() {
    cat <<'USAGE'
Usage:
  tools/release/ios/verify.sh archive ARCHIVE --version V --bundle-version N [--unsigned]
  tools/release/ios/verify.sh ipa IPA --version V --bundle-version N --team TEAM
USAGE
}

mode=""
target=""
version=""
bundle=""
team=""
unsigned=false
while (($# > 0)); do
    case "$1" in
        archive|ipa)
            [[ -z "$mode" ]] || { usage >&2; exit 2; }
            mode="$1"
            shift
            [[ $# -gt 0 ]] || { usage >&2; exit 2; }
            target="$1"
            shift
            ;;
        --version)
            [[ $# -ge 2 ]] || { usage >&2; exit 2; }
            version="$2"
            shift 2
            ;;
        --bundle-version)
            [[ $# -ge 2 ]] || { usage >&2; exit 2; }
            bundle="$2"
            shift 2
            ;;
        --team)
            [[ $# -ge 2 ]] || { usage >&2; exit 2; }
            team="$2"
            shift 2
            ;;
        --unsigned)
            unsigned=true
            shift
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            usage >&2
            exit 2
            ;;
    esac
done

[[ "$mode" == archive || "$mode" == ipa ]] || { usage >&2; exit 2; }
[[ -n "$target" && -n "$version" && -n "$bundle" ]] || { usage >&2; exit 2; }
if [[ "$mode" == ipa ]]; then
    [[ "$unsigned" == false && -n "$team" ]] || { usage >&2; exit 2; }
    valid_team_id "$team" || { usage >&2; exit 2; }
else
    [[ -z "$team" ]] || { usage >&2; exit 2; }
fi

[[ -e "$target" ]] || die "$mode does not exist: $target"
if [[ "$mode" == archive ]]; then
    app="$target/Products/Applications/RemoteScreen.app"
    dsym="$target/dSYMs/RemoteScreen.app.dSYM"
    json_path="$(dirname "$target")/verify-archive.json"
else
    app=""
    dsym=""
    json_path="$(dirname "$target")/verify-ipa.json"
fi

result_tmp="$(mktemp "${TMPDIR:-$RS_IOS_RELEASE_ROOT/target}/ios-verify.XXXXXX")"
cleanup() {
    rm -f -- "$result_tmp"
    if [[ -n "${extract_dir:-}" ]]; then
        rm -rf -- "$extract_dir"
    fi
}
trap cleanup EXIT
failed=0
pass_check() {
    local name="$1"
    printf 'PASS %s\n' "$name"
    printf '%s\ttrue\n' "$name" >>"$result_tmp"
}
fail_check() {
    local name="$1" reason="${2:-check failed}"
    failed=$((failed + 1))
    printf 'FAIL %s: %s\n' "$name" "$reason"
    printf '%s\tfalse\n' "$name" >>"$result_tmp"
}
warn_check() { printf 'WARN %s\n' "$1"; }

check_info() {
    local app_path="$1"
    local info_file="$app_path/Info.plist"
    if [[ ! -f "$info_file" ]]; then
        fail_check 'Info.plist' 'missing'
        for item in 'CFBundleIdentifier' 'CFBundleShortVersionString' 'CFBundleVersion' 'DTPlatformName' 'MinimumOSVersion'; do
            fail_check "$item" 'Info.plist unavailable'
        done
        return
    fi
    local values
    if ! values="$(python3 - "$info_file" <<'PY'
import plistlib
import sys

try:
    with open(sys.argv[1], "rb") as stream:
        info = plistlib.load(stream)
except Exception:
    raise SystemExit(1)
def value(key):
    item = info.get(key)
    return "" if item is None else str(item).replace("\t", " ").replace("\n", " ")
print("\t".join((value("CFBundleIdentifier"), value("CFBundleShortVersionString"),
                  value("CFBundleVersion"), value("DTPlatformName"),
                  value("MinimumOSVersion"), value("CFBundleExecutable"),
                  "true" if "ITSAppUsesNonExemptEncryption" in info else "false")))
PY
)"; then
        fail_check 'Info.plist' 'could not read'
        return
    fi
    local bundle_id short_version bundle_version platform minimum executable encryption
    IFS=$'\t' read -r bundle_id short_version bundle_version platform minimum executable encryption <<<"$values"
    if [[ "$bundle_id" == ai.gxlself.remotescreen ]]; then pass_check 'CFBundleIdentifier'; else fail_check 'CFBundleIdentifier' 'bundle id mismatch'; fi
    if [[ "$short_version" == "$version" ]]; then pass_check 'CFBundleShortVersionString'; else fail_check 'CFBundleShortVersionString' 'version mismatch'; fi
    if [[ "$bundle_version" == "$bundle" ]]; then pass_check 'CFBundleVersion'; else fail_check 'CFBundleVersion' 'build number mismatch'; fi
    if [[ "$platform" == iphoneos ]]; then pass_check 'DTPlatformName'; else fail_check 'DTPlatformName' 'platform mismatch'; fi
    if python3 - "$minimum" <<'PY'
import sys
try:
    parts = [int(part) for part in sys.argv[1].split(".")]
    while len(parts) < 2:
        parts.append(0)
    actual = tuple(parts)
    raise SystemExit(0 if actual >= (16, 0) else 1)
except (ValueError, IndexError):
    raise SystemExit(1)
PY
    then pass_check 'MinimumOSVersion (>= 16.0)'; else fail_check 'MinimumOSVersion (>= 16.0)' 'missing or below 16.0'; fi
    [[ -n "$executable" ]] || executable=RemoteScreen
    if [[ -f "$app_path/$executable" ]]; then
        local archs
        archs="$(lipo -archs "$app_path/$executable" 2>/dev/null || true)"
        if [[ "$(printf '%s' "$archs" | awk '{$1=$1; print}')" == arm64 ]]; then
            pass_check 'arm64 executable'
        else
            fail_check 'arm64 executable' 'architecture is not exactly arm64'
        fi
    else
        fail_check 'arm64 executable' 'executable missing'
    fi
    if [[ -f "$app_path/PrivacyInfo.xcprivacy" ]]; then pass_check 'PrivacyInfo.xcprivacy'; else fail_check 'PrivacyInfo.xcprivacy' 'missing'; fi
    if [[ -f "$app_path/main.jsbundle" ]]; then pass_check 'main.jsbundle'; else fail_check 'main.jsbundle' 'missing'; fi
    [[ "$encryption" == true ]] || warn_check 'ITSAppUsesNonExemptEncryption is absent'
}

if [[ "$mode" == archive ]]; then
    if [[ -d "$app" ]]; then pass_check 'RemoteScreen.app'; else fail_check 'RemoteScreen.app' 'missing from archive'; fi
    if [[ -d "$dsym" ]]; then pass_check 'RemoteScreen.app.dSYM'; else fail_check 'RemoteScreen.app.dSYM' 'missing from archive'; fi
    if [[ -d "$app" ]]; then check_info "$app"; else
        fail_check 'Info.plist' 'app missing'
    fi
else
    extract_dir="$(mktemp -d "${TMPDIR:-$RS_IOS_RELEASE_ROOT/target}/ios-ipa.XXXXXX")"
    if command -v unzip >/dev/null 2>&1 && unzip -q "$target" -d "$extract_dir"; then
        app="$extract_dir/Payload/RemoteScreen.app"
        if [[ -d "$app" ]]; then pass_check 'Payload/RemoteScreen.app'; else fail_check 'Payload/RemoteScreen.app' 'missing'; fi
    else
        fail_check 'Payload/RemoteScreen.app' 'could not extract IPA'
    fi
    if [[ -d "$app" ]]; then
        check_info "$app"
        if codesign --verify --deep --strict "$app" >/dev/null 2>&1; then pass_check 'codesign verification'; else fail_check 'codesign verification' 'codesign verification failed'; fi
        codesign_display="$extract_dir/codesign-display.txt"
        if codesign -dvv "$app" >"$codesign_display" 2>&1; then
            first_authority="$(sed -n 's/^Authority=//p' "$codesign_display" | head -n1)"
            if [[ "$first_authority" == 'Apple Distribution:'* ]]; then
                pass_check 'Apple Distribution authority'
            else
                fail_check 'Apple Distribution authority' 'first authority is not Apple Distribution'
            fi
            actual_team="$(sed -n 's/^TeamIdentifier=//p' "$codesign_display" | head -n1)"
            if [[ -n "$actual_team" && "$actual_team" == "$team" ]]; then
                pass_check 'TeamIdentifier (team matches)'
            else
                fail_check 'TeamIdentifier (team matches)' 'team mismatch'
            fi
        else
            fail_check 'Apple Distribution authority' 'could not inspect signature'
            fail_check 'TeamIdentifier (team matches)' 'could not inspect signature'
        fi
        provisioning_plist="$extract_dir/provisioning.plist"
        if [[ -f "$app/embedded.mobileprovision" ]] && security cms -D -i "$app/embedded.mobileprovision" >"$provisioning_plist" 2>/dev/null; then
            if python3 - "$provisioning_plist" "$team" <<'PY'
import plistlib
import sys
try:
    with open(sys.argv[1], "rb") as stream:
        info = plistlib.load(stream)
    entitlements = info.get("Entitlements", {})
    ok = (
        info.get("TeamIdentifier", [None])[0] == sys.argv[2]
        and entitlements.get("get-task-allow") is False
        and "ProvisionedDevices" not in info
        and str(entitlements.get("application-identifier", "")).endswith(".ai.gxlself.remotescreen")
    )
except Exception:
    ok = False
raise SystemExit(0 if ok else 1)
PY
            then pass_check 'embedded.mobileprovision (distribution profile)'; else fail_check 'embedded.mobileprovision (distribution profile)' 'profile does not match distribution requirements'; fi
        else
            fail_check 'embedded.mobileprovision (distribution profile)' 'missing or unreadable'
        fi
        entitlements="$extract_dir/entitlements.plist"
        if codesign -d --entitlements :- "$app" >"$entitlements" 2>/dev/null && python3 - "$entitlements" <<'PY'
import plistlib
import sys
data = open(sys.argv[1], "rb").read()
start = data.find(b"<?xml")
end = data.find(b"</plist>")
try:
    info = plistlib.loads(data[start:end + len(b"</plist>")]) if start >= 0 and end >= 0 else {}
except Exception:
    info = {}
raise SystemExit(0 if info.get("get-task-allow") is not True else 1)
PY
        then pass_check 'codesign entitlements get-task-allow=false'; else fail_check 'codesign entitlements get-task-allow=false' 'debug entitlement is enabled'; fi
    fi
fi

mkdir -p "$(dirname "$json_path")"
python3 - "$result_tmp" "$json_path" "$mode" "$failed" <<'PY'
import json
import pathlib
import sys
checks = {}
for line in pathlib.Path(sys.argv[1]).read_text(encoding="utf-8").splitlines():
    if "\t" in line:
        name, value = line.split("\t", 1)
        checks[name] = value == "true"
pathlib.Path(sys.argv[2]).write_text(json.dumps({
    "mode": sys.argv[3], "checks": checks, "failed": int(sys.argv[4])
}, indent=2) + "\n", encoding="utf-8")
PY
if ((failed == 0)); then
    printf 'VERIFY OK\n'
    exit 0
fi
printf 'VERIFY FAILED (%d)\n' "$failed"
exit 1
