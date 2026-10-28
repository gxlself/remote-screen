#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
source "$script_dir/common.sh"

usage() {
    cat <<'USAGE'
Usage: tools/release/mac/notarize.sh app|dmg <path> [--out DIR] [--skip-notarize]

Submit an App or DMG to Apple's notary service and staple the ticket. Credentials
come from RS_NOTARY_PROFILE or the complete RS_NOTARY_KEY,
RS_NOTARY_KEY_ID, RS_NOTARY_ISSUER set. Credential values are never printed.
USAGE
}

kind=''
path=''
out=''
skip=false
while [[ $# -gt 0 ]]; do
    case "$1" in
        --out)
            [[ $# -ge 2 && -n "$2" ]] || die '--out requires a nonempty directory.'
            out="$2"
            shift 2
            ;;
        --skip-notarize)
            skip=true
            shift
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
            if [[ -z "$kind" ]]; then
                kind="$1"
            elif [[ -z "$path" ]]; then
                path="$1"
            else
                usage >&2
                die 'Too many positional arguments.'
            fi
            shift
            ;;
    esac
done

[[ "$kind" == app || "$kind" == dmg ]] || { usage >&2; die 'The kind must be app or dmg.'; }
[[ -n "$path" ]] || { usage >&2; die 'A path is required.'; }
require_macos
for tool in xcrun python3 ditto mktemp; do
    require_command "$tool"
done
[[ -e "$path" ]] || die "Path does not exist: $path"
path="$(cd "$(dirname "$path")" && pwd -P)/$(basename "$path")"
if [[ -z "$out" ]]; then
    out="$(dirname "$path")"
elif [[ "$out" != /* ]]; then
    out="$PWD/$out"
fi
mkdir -p "$out"
out="$(cd "$out" && pwd -P)"

state="$out/.mac-notary-$kind.json"
write_state() {
    local notarized="$1" stapled="$2"
    python3 - "$state" "$path" "$kind" "$notarized" "$stapled" <<'PY'
import json
import pathlib
import sys
payload = {
    "kind": sys.argv[3],
    "path": sys.argv[2],
    "notarized": sys.argv[4] == "true",
    "stapled": sys.argv[5] == "true",
}
pathlib.Path(sys.argv[1]).write_text(json.dumps(payload, indent=2) + "\n")
print(json.dumps(payload, separators=(",", ":")))
PY
}

profile="${RS_NOTARY_PROFILE:-}"
key="${RS_NOTARY_KEY:-}"
key_id="${RS_NOTARY_KEY_ID:-}"
issuer="${RS_NOTARY_ISSUER:-}"
api_count=0
[[ -n "$key" ]] && api_count=$((api_count + 1))
[[ -n "$key_id" ]] && api_count=$((api_count + 1))
[[ -n "$issuer" ]] && api_count=$((api_count + 1))

if [[ "$skip" == true ]]; then
    log "Notarization skipped by --skip-notarize ($kind)."
    if [[ -n "${RS_MAC_SIGN_IDENTITY:-}" ]]; then
        warn 'A signing identity is set but notarization was explicitly skipped.'
    fi
    write_state false false
    exit 0
fi

if [[ -n "$profile" ]]; then
    log 'RS_NOTARY_PROFILE: set'
    if [[ "$api_count" -gt 0 ]]; then
        warn 'RS_NOTARY_KEY/RS_NOTARY_KEY_ID/RS_NOTARY_ISSUER also set; using RS_NOTARY_PROFILE.'
    fi
    credential_args=(--keychain-profile "$profile")
elif [[ "$api_count" -eq 3 ]]; then
    log 'RS_NOTARY_PROFILE: unset; API key credentials: complete'
    credential_args=(--key "$key" --key-id "$key_id" --issuer "$issuer")
elif [[ "$api_count" -eq 0 ]]; then
    log "No notarization credentials set; skipping $kind notarization."
    if [[ -n "${RS_MAC_SIGN_IDENTITY:-}" ]]; then
        warn 'A signing identity is set but no notarization credentials are available; release is signed but not notarized.'
    fi
    write_state false false
    exit 0
else
    log 'RS_NOTARY_PROFILE: unset; API key credentials: incomplete'
    die 'Set RS_NOTARY_PROFILE or all three RS_NOTARY_KEY, RS_NOTARY_KEY_ID, RS_NOTARY_ISSUER.'
fi

if [[ "$kind" == app ]]; then
    temporary="$(mktemp -d "$out/.notary.XXXXXX")"
    cleanup() { rm -rf -- "$temporary"; }
    trap cleanup EXIT
    submission="$temporary/RemoteHost-notary.zip"
    ditto -c -k --keepParent "$path" "$submission"
else
    submission="$path"
fi

log "Submitting $kind to Apple notary service (waiting for result)"
set +e
submit_json="$(xcrun notarytool submit "$submission" --wait --output-format json "${credential_args[@]}" 2>&1)"
submit_status=$?
set -e
if [[ "$submit_status" -ne 0 ]]; then
    log "notarytool submit failed for $kind (exit $submit_status)."
    die 'Notarization submission failed.'
fi

parsed="$(python3 - "$submit_json" <<'PY'
import json
import sys
text = sys.argv[1]
try:
    value = json.loads(text)
except json.JSONDecodeError:
    start = text.rfind("{")
    value = json.loads(text[start:]) if start >= 0 else {}
identifier = str(value.get("id", ""))
status = str(value.get("status", ""))
print(identifier)
print(status)
PY
)"
notary_id="$(printf '%s\n' "$parsed" | sed -n '1p')"
status="$(printf '%s\n' "$parsed" | sed -n '2p')"
[[ "$notary_id" =~ ^[A-Za-z0-9._-]+$ ]] || die 'notarytool did not return a valid submission id.'
if [[ "$status" != Accepted ]]; then
    log "Notarization status for $kind: $status"
    log_file="$out/notary-$notary_id.json"
    set +e
    xcrun notarytool log "$notary_id" --output-format json "${credential_args[@]}" >"$log_file" 2>&1
    log_status=$?
    set -e
    [[ "$log_status" -eq 0 ]] || warn "Could not download notary log (exit $log_status)."
    die "Notarization was not accepted; see $log_file."
fi

log "Notarization accepted for $kind; stapling ticket"
xcrun stapler staple "$path"
xcrun stapler validate "$path"
write_state true true
