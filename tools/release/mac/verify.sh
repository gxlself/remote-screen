#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
source "$script_dir/common.sh"

usage() {
    cat <<'USAGE'
Usage: tools/release/mac/verify.sh --app APP --dmg DMG --update-zip ZIP [options]
       tools/release/mac/verify.sh APP [--dmg DMG] [--update-zip ZIP] [--out DIR]

Verify bundle structure, Hardened Runtime signatures, the DMG, and the update
zip. A Developer ID release also gets Gatekeeper and stapler checks. The update
zip is extracted into a temporary directory and checked before installation.
USAGE
}

app=''
dmg=''
update_zip=''
out=''
arch='native'
while [[ $# -gt 0 ]]; do
    case "$1" in
        --app)
            [[ $# -ge 2 && -n "$2" ]] || die '--app requires a path.'
            app="$2"
            shift 2
            ;;
        --dmg)
            [[ $# -ge 2 && -n "$2" ]] || die '--dmg requires a path.'
            dmg="$2"
            shift 2
            ;;
        --update-zip|--zip)
            [[ $# -ge 2 && -n "$2" ]] || die '--update-zip requires a path.'
            update_zip="$2"
            shift 2
            ;;
        --out)
            [[ $# -ge 2 && -n "$2" ]] || die '--out requires a nonempty directory.'
            out="$2"
            shift 2
            ;;
        --arch)
            [[ $# -ge 2 ]] || die '--arch requires native or universal.'
            arch="$2"
            shift 2
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        --*)
            usage >&2
            die "Unknown argument: $1"
            ;;
        *)
            [[ -z "$app" ]] || die 'Only one positional App path may be supplied.'
            app="$1"
            shift
            ;;
    esac
done

[[ -n "$app" && -n "$dmg" && -n "$update_zip" ]] || { usage >&2; die 'App, DMG, and update zip are required.'; }
[[ "$arch" == native || "$arch" == universal ]] || die "Unsupported --arch: $arch"
require_macos
for tool in codesign ditto hdiutil lipo otool python3 stat xcrun; do
    require_command "$tool"
done
[[ -d "$app" ]] || die "App does not exist: $app"
[[ -f "$dmg" ]] || die "DMG does not exist: $dmg"
[[ -f "$update_zip" ]] || die "Update zip does not exist: $update_zip"
app="$(cd "$app" && pwd -P)"
dmg="$(cd "$(dirname "$dmg")" && pwd -P)/$(basename "$dmg")"
update_zip="$(cd "$(dirname "$update_zip")" && pwd -P)/$(basename "$update_zip")"
if [[ -z "$out" ]]; then
    out="$(dirname "$app")"
elif [[ "$out" != /* ]]; then
    out="$PWD/$out"
fi
mkdir -p "$out"
out="$(cd "$out" && pwd -P)"

read_team_id() {
    local target="$1" display status team
    set +e
    display="$(codesign -dv --verbose=4 "$target" 2>&1)"
    status=$?
    set -e
    [[ "$status" -eq 0 ]] || return 1
    team="$(printf '%s\n' "$display" | awk -F= '/^TeamIdentifier=/{print $2; exit}')"
    case "$team" in
        ''|'not set'|'none'|'null') printf '\n' ;;
        *) printf '%s\n' "$team" ;;
    esac
}

state="$out/.mac-sign.json"
state_present=false
if [[ -f "$state" ]]; then
    state_present=true
    signed="$(python3 - "$state" <<'PY'
import json
import sys
print("true" if json.load(open(sys.argv[1])).get("signed") else "false")
PY
)"
    expected_team="$(python3 - "$state" <<'PY'
import json
import sys
print(json.load(open(sys.argv[1])).get("team_id") or "")
PY
)"
else
    signed=false
    expected_team=''
fi

log "Verifying app bundle structure ($arch)"
verify_structure "$app" "$arch"
codesign --verify --strict --deep --verbose=2 "$app"
actual_team="$(read_team_id "$app")" || die 'Could not inspect app signature.'
if [[ "$state_present" == false ]]; then
    expected_team="$actual_team"
    [[ -n "$actual_team" ]] && signed=true || signed=false
fi
if [[ "$expected_team" != "$actual_team" ]]; then
    die "App Team ID mismatch (expected ${expected_team:-none}, got ${actual_team:-none})."
fi
if [[ "$signed" == true && -z "$actual_team" ]]; then
    die 'Release state says Developer ID signing, but the app has no Team ID.'
fi
if [[ "$signed" == false && -n "$actual_team" ]]; then
    die 'Ad-hoc release unexpectedly has a Team ID.'
fi

log 'Verifying DMG image'
hdiutil verify "$dmg"
app_notarized=false
dmg_notarized=false
if [[ -f "$out/.mac-notary-app.json" ]]; then
    app_notarized="$(python3 - "$out/.mac-notary-app.json" <<'PY'
import json
import sys
print("true" if json.load(open(sys.argv[1])).get("notarized") else "false")
PY
)"
fi
if [[ -f "$out/.mac-notary-dmg.json" ]]; then
    dmg_notarized="$(python3 - "$out/.mac-notary-dmg.json" <<'PY'
import json
import sys
print("true" if json.load(open(sys.argv[1])).get("notarized") else "false")
PY
)"
fi
notarized=false
[[ "$app_notarized" == true && "$dmg_notarized" == true ]] && notarized=true
if [[ "$signed" == true && "$notarized" == true ]]; then
    require_command spctl
    require_command stapler
    spctl --assess --type execute -vv "$app"
    spctl --assess --type open --context context:primary-signature -vv "$dmg"
    xcrun stapler validate "$app"
    xcrun stapler validate "$dmg"
elif [[ "$signed" == true ]]; then
    warn 'Developer ID signature is present but notarization was skipped; Gatekeeper/stapler checks are deferred.'
fi

release_json="$out/release.json"
[[ -f "$release_json" ]] || die "Missing update manifest: $release_json"
manifest_values="$(python3 - "$release_json" "$update_zip" <<'PY'
import hashlib
import json
import pathlib
import sys
manifest = json.load(open(sys.argv[1]))
zip_path = pathlib.Path(sys.argv[2])
expected_hash = str(manifest.get("sha256", "")).lower()
actual_hash = hashlib.sha256(zip_path.read_bytes()).hexdigest()
actual_size = zip_path.stat().st_size
if manifest.get("os") != "mac":
    raise SystemExit("release.json os is not mac")
if manifest.get("file") != zip_path.name:
    raise SystemExit("release.json file does not match update zip")
if expected_hash != actual_hash:
    raise SystemExit(f"update zip sha256 mismatch: expected {expected_hash}, got {actual_hash}")
if int(manifest.get("size", -1)) != actual_size:
    raise SystemExit("update zip size mismatch")
print(manifest.get("version", ""))
print(manifest.get("arch", ""))
print(manifest.get("team_id") or "")
print("true" if manifest.get("notarized") else "false")
PY
)" || die 'Update manifest does not match the update zip.'
manifest_version="$(printf '%s\n' "$manifest_values" | sed -n '1p')"
manifest_arch="$(printf '%s\n' "$manifest_values" | sed -n '2p')"
manifest_team="$(printf '%s\n' "$manifest_values" | sed -n '3p')"
manifest_notarized="$(printf '%s\n' "$manifest_values" | sed -n '4p')"
[[ -n "$manifest_version" ]] || die 'release.json has no version.'
app_version="$(/usr/bin/plutil -extract CFBundleShortVersionString raw -o - "$app/Contents/Info.plist" 2>/dev/null || true)"
if [[ -z "$app_version" ]]; then
    app_version="$(/usr/bin/plutil -extract CFBundleVersion raw -o - "$app/Contents/Info.plist" 2>/dev/null || true)"
fi
[[ "$manifest_version" == "$app_version" ]] || die "release.json version mismatch (expected $app_version, got $manifest_version)."
[[ "$manifest_arch" == "$arch" ]] || die "release.json architecture mismatch (expected $arch, got $manifest_arch)."
[[ "$manifest_team" == "$actual_team" ]] || die "release.json Team ID mismatch (expected ${actual_team:-none}, got ${manifest_team:-none})."
[[ "$manifest_notarized" == "$notarized" ]] || die 'release.json notarized state does not match the release state.'

extract_dir="$(mktemp -d "$out/.verify-update.XXXXXX")"
cleanup() { rm -rf -- "$extract_dir"; }
trap cleanup EXIT
log 'Extracting update zip for installation-time signature checks'
ditto -x -k "$update_zip" "$extract_dir"
extracted_app="$extract_dir/$RS_MAC_APP_NAME"
[[ -d "$extracted_app" ]] || die 'Update zip does not contain a top-level Remote Host.app.'
verify_structure "$extracted_app" "$arch"
codesign --verify --strict --deep --verbose=2 "$extracted_app"
extracted_team="$(read_team_id "$extracted_app")" || die 'Could not inspect extracted app signature.'
[[ "$extracted_team" == "$actual_team" ]] || die "Extracted app Team ID mismatch (expected ${actual_team:-none}, got ${extracted_team:-none})."
if [[ "$signed" == true && "$notarized" == true ]]; then
    xcrun stapler validate "$extracted_app"
fi

state_out="$out/.mac-verify.json"
python3 - "$state_out" "$signed" "$actual_team" "$notarized" "$dmg" "$update_zip" <<'PY'
import json
import pathlib
import sys
payload = {
    "verified": True,
    "signed": sys.argv[2] == "true",
    "team_id": sys.argv[3] or None,
    "notarized": sys.argv[4] == "true",
    "dmg": sys.argv[5],
    "update_zip": sys.argv[6],
}
pathlib.Path(sys.argv[1]).write_text(json.dumps(payload, indent=2) + "\n")
print(json.dumps(payload, separators=(",", ":")))
PY
