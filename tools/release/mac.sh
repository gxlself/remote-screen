#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
mac_dir="$script_dir/mac"
source "$mac_dir/common.sh"

usage() {
    cat <<'USAGE'
Usage: tools/release/mac.sh [--arch universal|native] [--skip-build]
                              [--skip-notarize] [--out DIR]

Build, sign, notarize, package, and verify a distributable Remote Host release.
The default architecture is universal and the default output is
target/rs-release/mac. Without Developer ID/notary credentials, the workflow
uses an ad-hoc signature and still produces a DMG, update zip, and manifest.
USAGE
}

arch=universal
skip_build=false
skip_notarize=false
out="$RS_MAC_REPO_ROOT/target/rs-release/mac"
while [[ $# -gt 0 ]]; do
    case "$1" in
        --arch)
            [[ $# -ge 2 ]] || die '--arch requires universal or native.'
            arch="$2"
            shift 2
            ;;
        --skip-build)
            skip_build=true
            shift
            ;;
        --skip-notarize)
            skip_notarize=true
            shift
            ;;
        --out)
            [[ $# -ge 2 && -n "$2" ]] || die '--out requires a nonempty directory.'
            out="$2"
            shift 2
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            usage >&2
            die "Unknown argument: $1"
            ;;
    esac
done
[[ "$arch" == native || "$arch" == universal ]] || die "Unsupported --arch: $arch"
if [[ "$out" != /* ]]; then
    out="$PWD/$out"
fi
mkdir -p "$out"
out="$(cd "$out" && pwd -P)"
require_macos
for tool in bash date plutil python3 stat; do
    require_command "$tool"
done

run_step() {
    local name="$1"
    shift
    local started finished elapsed
    started="$(date +%s)"
    log "$name"
    # A caller's `|| die` turns off errexit inside this function: propagate failures explicitly.
    "$@" || return $?
    finished="$(date +%s)"
    elapsed=$((finished - started))
    log "$name completed in ${elapsed}s"
}

build_script="$mac_dir/build.sh"
sign_script="$mac_dir/sign.sh"
notarize_script="$mac_dir/notarize.sh"
dmg_script="$mac_dir/dmg.sh"
update_script="$mac_dir/update-zip.sh"
verify_script="$mac_dir/verify.sh"
app="$out/$RS_MAC_APP_NAME"

if [[ "$skip_build" == true ]]; then
    log 'Build skipped by --skip-build'
    [[ -d "$app" ]] || die "--skip-build requested but app is missing: $app"
else
    run_step '1/7 Build' "$build_script" --arch "$arch" --out "$out"
fi

[[ -d "$app" ]] || die "Build did not produce $app"
version="$(plutil -extract CFBundleShortVersionString raw -o - "$app/Contents/Info.plist" 2>/dev/null || true)"
if [[ -z "$version" ]]; then
    version="$(plutil -extract CFBundleVersion raw -o - "$app/Contents/Info.plist" 2>/dev/null || true)"
fi
[[ -n "$version" ]] || die 'Could not read the built app version.'

run_step '2/7 Sign app (Developer ID or ad-hoc)' "$sign_script" "$app" --out "$out"

if [[ "$skip_notarize" == true ]]; then
    run_step '3/7 Notarize app (skipped)' "$notarize_script" app "$app" --out "$out" --skip-notarize
else
    run_step '3/7 Notarize app' "$notarize_script" app "$app" --out "$out"
fi

run_step '4/7 Create DMG' "$dmg_script" "$app" --out "$out" --arch "$arch"
dmg="$out/RemoteHost-$version.dmg"
[[ -f "$dmg" ]] || die "DMG was not produced: $dmg"

if [[ "$skip_notarize" == true ]]; then
    run_step '5/7 Notarize DMG (skipped)' "$notarize_script" dmg "$dmg" --out "$out" --skip-notarize
else
    run_step '5/7 Notarize DMG' "$notarize_script" dmg "$dmg" --out "$out"
fi

run_step '6/7 Create update zip and release.json' "$update_script" "$app" --out "$out" --arch "$arch"
update_zip="$out/RemoteHost-$version-mac-$arch.zip"
release_json="$out/release.json"
[[ -f "$update_zip" && -f "$release_json" ]] || die 'Update package or release.json is missing.'

run_step '7/7 Verify release' "$verify_script" --app "$app" --dmg "$dmg" \
    --update-zip "$update_zip" --out "$out" --arch "$arch"

signed=false
team_id=''
if [[ -f "$out/.mac-sign.json" ]]; then
    signed="$(python3 - "$out/.mac-sign.json" <<'PY'
import json
import sys
print("true" if json.load(open(sys.argv[1])).get("signed") else "false")
PY
)"
    team_id="$(python3 - "$out/.mac-sign.json" <<'PY'
import json
import sys
print(json.load(open(sys.argv[1])).get("team_id") or "")
PY
)"
fi
app_notarized=false
app_stapled=false
dmg_notarized=false
dmg_stapled=false
if [[ -f "$out/.mac-notary-app.json" ]]; then
    app_notarized="$(python3 - "$out/.mac-notary-app.json" <<'PY'
import json
import sys
value = json.load(open(sys.argv[1]))
print("true" if value.get("notarized") else "false")
PY
)"
    app_stapled="$(python3 - "$out/.mac-notary-app.json" <<'PY'
import json
import sys
value = json.load(open(sys.argv[1]))
print("true" if value.get("stapled") else "false")
PY
)"
fi
if [[ -f "$out/.mac-notary-dmg.json" ]]; then
    dmg_notarized="$(python3 - "$out/.mac-notary-dmg.json" <<'PY'
import json
import sys
value = json.load(open(sys.argv[1]))
print("true" if value.get("notarized") else "false")
PY
)"
    dmg_stapled="$(python3 - "$out/.mac-notary-dmg.json" <<'PY'
import json
import sys
value = json.load(open(sys.argv[1]))
print("true" if value.get("stapled") else "false")
PY
)"
fi
notarized=false
stapled=false
[[ "$app_notarized" == true && "$dmg_notarized" == true ]] && notarized=true
[[ "$app_stapled" == true && "$dmg_stapled" == true ]] && stapled=true

dmg_bytes="$(stat -f%z "$dmg")"
update_sha256="$(python3 - "$update_zip" <<'PY'
import hashlib
import pathlib
import sys
print(hashlib.sha256(pathlib.Path(sys.argv[1]).read_bytes()).hexdigest())
PY
)"
update_bytes="$(stat -f%z "$update_zip")"
summary_path="$out/release-summary.json"
summary="$(python3 - "$summary_path" "$version" "$arch" "$dmg" "$dmg_bytes" "$update_zip" "$update_sha256" "$update_bytes" "$signed" "$team_id" "$notarized" "$stapled" <<'PY'
import json
import pathlib
import sys
payload = {
    "version": sys.argv[2],
    "arch": sys.argv[3],
    "dmg": sys.argv[4],
    "dmg_bytes": int(sys.argv[5]),
    "update_zip": sys.argv[6],
    "update_sha256": sys.argv[7],
    "update_bytes": int(sys.argv[8]),
    "signed": sys.argv[9] == "true",
    "team_id": sys.argv[10] or None,
    "notarized": sys.argv[11] == "true",
    "stapled": sys.argv[12] == "true",
}
pathlib.Path(sys.argv[1]).write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n")
print(json.dumps(payload, ensure_ascii=False, separators=(",", ":")))
PY
)"
log 'Release complete. Auto-update packages use zip; the final publisher must add the Ed25519 manifest signature.'
log 'Entitlements include network server/client only; App Sandbox is not enabled, so those keys are harmless.'
printf '%s\n' "$summary"
