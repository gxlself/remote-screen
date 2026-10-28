#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
source "$script_dir/common.sh"

usage() {
    cat <<'USAGE'
Usage: tools/release/mac/sign.sh <Remote Host.app> [--out DIR]

Sign the agent and app from the inside out. RS_MAC_SIGN_IDENTITY selects a
Developer ID identity; when it is unset, an ad-hoc hardened-runtime signature
is produced. The identity itself is never printed.
USAGE
}

app=''
out=''
while [[ $# -gt 0 ]]; do
    case "$1" in
        --out)
            [[ $# -ge 2 && -n "$2" ]] || die '--out requires a nonempty directory.'
            out="$2"
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
require_macos
for tool in codesign python3 find; do
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

agent="$app/Contents/MacOS/rs-host-agent"
app_entitlements="$script_dir/entitlements/app.plist"
agent_entitlements="$script_dir/entitlements/agent.plist"
[[ -x "$agent" ]] || die "Missing executable agent: $agent"
[[ -f "$app_entitlements" && -f "$agent_entitlements" ]] || die 'Missing release entitlements.'

identity="${RS_MAC_SIGN_IDENTITY:-}"
if [[ -n "$identity" ]]; then
    log 'RS_MAC_SIGN_IDENTITY: set (Developer ID signing enabled)'
    signed=true
    sign_identity="$identity"
    sign_flags=(--force --sign "$sign_identity" --options runtime --timestamp)
else
    log 'RS_MAC_SIGN_IDENTITY: unset (using ad-hoc signing)'
    signed=false
    sign_identity='-'
    sign_flags=(--force --sign "$sign_identity" --options runtime)
fi

sign_one() {
    local path="$1"
    local identifier_args=()
    if [[ "$path" == "$agent" ]]; then
        identifier_args=(--identifier "$RS_MAC_AGENT_ID" --entitlements "$agent_entitlements")
    elif [[ "$path" == "$app" ]]; then
        identifier_args=(--entitlements "$app_entitlements")
    fi
    local sign_status
    set +e
    codesign "${sign_flags[@]}" "${identifier_args[@]}" "$path" >/dev/null 2>&1
    sign_status=$?
    set -e
    if [[ "$sign_status" -ne 0 ]]; then
        # codesign includes an unknown identity verbatim in its diagnostic.
        # Do not forward that output because the identity is a credential-like
        # environment value and release logs must not contain it.
        warn "codesign failed while signing (exit $sign_status); diagnostic suppressed."
        return "$sign_status"
    fi
}

log "Signing agent: $RS_MAC_AGENT_ID"
sign_one "$agent"

frameworks="$app/Contents/Frameworks"
if [[ -d "$frameworks" ]]; then
    # Sign leaf dynamic libraries before their containing framework bundles.
    while IFS= read -r -d '' path; do
        log "Signing framework library: $(basename "$path")"
        sign_one "$path"
    done < <(find "$frameworks" -type f -name '*.dylib' -print0 | sort -z)
    while IFS= read -r -d '' path; do
        log "Signing framework: $(basename "$path")"
        sign_one "$path"
    done < <(find "$frameworks" -type d -name '*.framework' -print0 | sort -zr)
fi

log 'Signing app bundle with Hardened Runtime'
sign_one "$app"
codesign --verify --strict --deep --verbose=2 "$app"

# codesign writes display information to stderr. Keep only the TeamIdentifier
# field and normalize Apple's "not set" spelling to JSON null.
display_info=''
set +e
display_info="$(codesign -dv --verbose=4 "$app" 2>&1)"
display_status=$?
set -e
[[ "$display_status" -eq 0 ]] || die 'Could not inspect the signed app.'
team_id="$(printf '%s\n' "$display_info" | awk -F= '/^TeamIdentifier=/{print $2; exit}')"
case "$team_id" in
    ''|'not set'|'none'|'null') team_id='' ;;
esac

state="$out/.mac-sign.json"
python3 - "$state" "$app" "$signed" "$team_id" <<'PY'
import json
import pathlib
import sys

state_path = pathlib.Path(sys.argv[1])
payload = {
    "app": sys.argv[2],
    "signed": sys.argv[3] == "true",
    "team_id": sys.argv[4] or None,
}
state_path.write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n")
print(json.dumps(payload, ensure_ascii=False, separators=(",", ":")))
PY
