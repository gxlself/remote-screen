#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
source "$script_dir/common.sh"

usage() {
    cat <<'USAGE'
Usage: tools/release/mac/dmg.sh <Remote Host.app> [--out DIR] [--arch native|universal]

Create RemoteHost-<version>.dmg with the app and an Applications shortcut.
RS_MAC_SIGN_IDENTITY signs the disk image when set. RS_DMG_MAX_MB defaults to 30.
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
for tool in hdiutil ditto plutil python3 stat awk mktemp; do
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
set +e
version="$(plutil -extract CFBundleShortVersionString raw -o - "$info" 2>/dev/null)"
version_status=$?
set -e
if [[ "$version_status" -ne 0 || -z "$version" ]]; then
    version="$(plutil -extract CFBundleVersion raw -o - "$info" 2>/dev/null || true)"
fi
[[ -n "$version" ]] || die 'Could not read the app version from Info.plist.'
[[ "$version" =~ ^[A-Za-z0-9._+-]+$ ]] || die 'App version contains unsafe filename characters.'

dmg="$out/RemoteHost-$version.dmg"
staging="$(mktemp -d "$out/.dmg-staging.XXXXXX")"
cleanup() { rm -rf -- "$staging"; }
trap cleanup EXIT
log "Preparing DMG staging directory"
ditto "$app" "$staging/$RS_MAC_APP_NAME"
ln -s /Applications "$staging/Applications"

log "Creating $dmg"
hdiutil create -volname 'Remote Host' -srcfolder "$staging" -fs HFS+ -format ULFO -ov "$dmg"

identity="${RS_MAC_SIGN_IDENTITY:-}"
if [[ -n "$identity" ]]; then
    require_command codesign
    log 'RS_MAC_SIGN_IDENTITY: set; signing DMG'
    set +e
    codesign --force --sign "$identity" --timestamp "$dmg" >/dev/null 2>&1
    codesign_status=$?
    set -e
    if [[ "$codesign_status" -ne 0 ]]; then
        warn "codesign failed while signing the DMG (exit $codesign_status); diagnostic suppressed."
        exit "$codesign_status"
    fi
    dmg_signed=true
else
    log 'RS_MAC_SIGN_IDENTITY: unset; DMG remains unsigned (app is ad-hoc signed)'
    dmg_signed=false
fi

max_mb="${RS_DMG_MAX_MB:-30}"
[[ "$max_mb" =~ ^[0-9]+([.][0-9]+)?$ ]] || die 'RS_DMG_MAX_MB must be a nonnegative number.'
size_bytes="$(stat -f%z "$dmg")"
if awk -v size="$size_bytes" -v max="$max_mb" 'BEGIN { exit !(size > max * 1024 * 1024) }'; then
    if [[ -n "$identity" ]]; then
        die "DMG is ${size_bytes} bytes, above RS_DMG_MAX_MB=$max_mb."
    fi
    warn "DMG is ${size_bytes} bytes, above RS_DMG_MAX_MB=$max_mb (ad-hoc build continues)."
fi

state="$out/.mac-dmg.json"
python3 - "$state" "$dmg" "$version" "$arch" "$size_bytes" "$dmg_signed" <<'PY'
import json
import pathlib
import sys
payload = {
    "dmg": sys.argv[2],
    "version": sys.argv[3],
    "arch": sys.argv[4],
    "bytes": int(sys.argv[5]),
    "signed": sys.argv[6] == "true",
}
pathlib.Path(sys.argv[1]).write_text(json.dumps(payload, indent=2) + "\n")
print(json.dumps(payload, separators=(",", ":")))
PY
