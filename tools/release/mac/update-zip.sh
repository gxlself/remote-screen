#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
source "$script_dir/common.sh"

usage() {
    cat <<'USAGE'
Usage: tools/release/mac/update-zip.sh <Remote Host.app> [--out DIR] [--arch native|universal]

Create the auto-update zip and release.json. The zip keeps the app bundle as its
single top-level entry; the final publisher adds the Ed25519 manifest signature later.
USAGE
}

app=''
out=''
arch='native'
while [[ $# -gt 0 ]]; do
    case "$1" in
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
            [[ -z "$app" ]] || die 'Only one App path may be supplied.'
            app="$1"
            shift
            ;;
    esac
done

[[ -n "$app" ]] || { usage >&2; die 'An App path is required.'; }
[[ "$arch" == native || "$arch" == universal ]] || die "Unsupported --arch: $arch"
require_macos
for tool in codesign ditto plutil python3 shasum stat; do
    require_command "$tool"
done
[[ -d "$app" ]] || die "App does not exist: $app"
app="$(cd "$app" && pwd -P)"
[[ "$app" == *.app ]] || die "Expected a .app bundle: $app"
if [[ -z "$out" ]]; then
    out="$(dirname "$app")"
elif [[ "$out" != /* ]]; then
    out="$PWD/$out"
fi
mkdir -p "$out"
out="$(cd "$out" && pwd -P)"

info="$app/Contents/Info.plist"
[[ -f "$info" ]] || die "Missing app Info.plist: $info"
version="$(plutil -extract CFBundleShortVersionString raw -o - "$info" 2>/dev/null || true)"
if [[ -z "$version" ]]; then
    version="$(plutil -extract CFBundleVersion raw -o - "$info" 2>/dev/null || true)"
fi
[[ -n "$version" ]] || die 'Could not read the app version from Info.plist.'
[[ "$version" =~ ^[A-Za-z0-9._+-]+$ ]] || die 'App version contains unsafe filename characters.'

zip_path="$out/RemoteHost-$version-mac-$arch.zip"
rm -f -- "$zip_path"
log "Creating update zip (format: zip; installer atomically replaces the .app)"
ditto -c -k --sequesterRsrc --keepParent "$app" "$zip_path"
sha256="$(shasum -a 256 "$zip_path" | awk '{print $1}')"
size_bytes="$(stat -f%z "$zip_path")"

team_id=''
sign_state="$out/.mac-sign.json"
if [[ -f "$sign_state" ]]; then
    team_id="$(python3 - "$sign_state" <<'PY2'
import json
import sys
value = json.load(open(sys.argv[1]))
print(value.get("team_id") or "")
PY2
)"
fi
if [[ -z "$team_id" ]]; then
    # This is only a fallback for a manually invoked update-zip.sh. The normal
    # release flow gets the Team ID from sign.sh's state file.
    set +e
    display_info="$(codesign -dv --verbose=4 "$app" 2>&1)"
    codesign_status=$?
    set -e
    if [[ "$codesign_status" -eq 0 ]]; then
        detected_team="$(printf '%s\n' "$display_info" | awk -F= '/^TeamIdentifier=/{print $2; exit}')"
        case "$detected_team" in
            ''|'not set'|'none'|'null') ;;
            *) team_id="$detected_team" ;;
        esac
    fi
fi
case "$team_id" in
    ''|'not set'|'none'|'null') team_id='' ;;
esac

notarized=false
notary_state="$out/.mac-notary-app.json"
if [[ -f "$notary_state" ]]; then
    notarized="$(python3 - "$notary_state" <<'PY'
import json
import sys
print("true" if json.load(open(sys.argv[1])).get("notarized") else "false")
PY
)"
fi

release_json="$out/release.json"
python3 - "$release_json" "$version" "$arch" "$(basename "$zip_path")" "$sha256" "$size_bytes" "$team_id" "$notarized" <<'PY'
import json
import pathlib
import sys
payload = {
    "version": sys.argv[2],
    "os": "mac",
    "arch": sys.argv[3],
    "file": sys.argv[4],
    "sha256": sys.argv[5],
    "size": int(sys.argv[6]),
    "team_id": sys.argv[7] or None,
    "notarized": sys.argv[8] == "true",
}
target = pathlib.Path(sys.argv[1])
tmp = target.with_suffix(target.suffix + ".tmp")
tmp.write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n")
tmp.replace(target)
print(json.dumps(payload, ensure_ascii=False, separators=(",", ":")))
PY

state="$out/.mac-update.json"
python3 - "$state" "$zip_path" "$sha256" "$size_bytes" "$release_json" <<'PY'
import json
import pathlib
import sys
payload = {
    "update_zip": sys.argv[2],
    "sha256": sys.argv[3],
    "bytes": int(sys.argv[4]),
    "release_json": sys.argv[5],
}
pathlib.Path(sys.argv[1]).write_text(json.dumps(payload, indent=2) + "\n")
PY
