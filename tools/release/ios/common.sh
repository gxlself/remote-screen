#!/usr/bin/env bash

# Shared, deliberately platform-neutral helpers for the iOS release scripts.
# Do not enable shell tracing in this file: callers handle signing values.

RS_IOS_RELEASE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd -P)"

log() { printf '[ios-release] %s\n' "$*" >&2; }
warn() { log "WARNING: $*"; }
die() { log "ERROR: $*"; exit 1; }

# tailnet: what build-ios.sh linked (off, an xcframework path, or unknown when
# Rust was not rebuilt). with: --with-tailnet was asked for. allow:
# --allow-no-tailnet.
tailnet_release_gate() {
    local tailnet="${1:-unknown}" with="${2:-false}" allow="${3:-false}"
    case "$tailnet" in
        unknown)
            if [[ "$with" == false ]]; then
                warn 'Rust was not rebuilt; cannot confirm that this build leaves the in-app tailnet out.'
            fi
            return 0
            ;;
        off)
            if [[ "$with" == false ]]; then
                log 'This build does not include the in-app tailnet (App Store default; --with-tailnet to include it).'
                return 0
            fi
            if [[ "$allow" == true ]]; then
                warn 'Tailnet is not included in this release (--allow-no-tailnet).'
                return 0
            fi
            log 'ERROR: Tailnet is not included; build it or pass --allow-no-tailnet.'
            return 1
            ;;
        *)
            log "This build includes the in-app tailnet: $tailnet"
            return 0
            ;;
    esac
}

repo_root() { printf '%s\n' "$RS_IOS_RELEASE_ROOT"; }

sha256_of() {
    local file="$1"
    [[ -f "$file" ]] || die "File does not exist: $file"
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$file" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$file" | awk '{print $1}'
    else
        die 'Neither sha256sum nor shasum is available.'
    fi
}

read_workspace_version() {
    local file="${1:-$RS_IOS_RELEASE_ROOT/Cargo.toml}"
    [[ -f "$file" ]] || return 1
    local value
    value="$(awk '
        /^\[workspace\.package\][[:space:]]*(#.*)?$/ { in_package=1; next }
        /^\[/ { in_package=0 }
        in_package && /^[[:space:]]*version[[:space:]]*=/ {
            if (match($0, /"[^"]+"/)) {
                print substr($0, RSTART + 1, RLENGTH - 2)
                exit
            }
        }
    ' "$file")"
    [[ -n "$value" ]] || return 1
    printf '%s\n' "$value"
}

bundle_version() {
    local version="$1" build="$2"
    [[ "$version" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]] || {
        printf 'invalid version: expected x.y.z\n' >&2
        return 1
    }
    local major="${BASH_REMATCH[1]}"
    local minor="${BASH_REMATCH[2]}"
    local patch="${BASH_REMATCH[3]}"
    [[ "$build" =~ ^[0-9]+$ ]] || {
        printf 'invalid build: expected an integer in 0..99\n' >&2
        return 1
    }
    python3 - "$major" "$minor" "$patch" "$build" <<'PY'
import sys

major, minor, patch, build = (int(value, 10) for value in sys.argv[1:])
if minor > 99 or patch > 99:
    raise SystemExit("invalid version: minor and patch must be <= 99")
if build > 99:
    raise SystemExit("invalid build: expected an integer in 0..99")
result = major * 1_000_000 + minor * 10_000 + patch * 100 + build
if result >= 2_100_000_000:
    raise SystemExit("invalid version: bundle version exceeds 2100000000")
print(result)
PY
}

# Read one shell-properties key without sourcing the file. Only the requested
# value is printed, which keeps credentials and unrelated routing keys private.
read_prop() {
    local file="$1" wanted="$2"
    [[ -f "$file" ]] || return 0
    awk -v wanted="$wanted" '
        /^[[:space:]]*#/ || /^[[:space:]]*$/ { next }
        {
            line=$0
            sub(/^[[:space:]]*/, "", line)
            eq=index(line, "=")
            if (!eq) next
            name=substr(line, 1, eq - 1)
            gsub(/[[:space:]]+$/, "", name)
            if (name != wanted) next
            value=substr(line, eq + 1)
            sub(/[[:space:]]+#.*$/, "", value)
            gsub(/^[[:space:]]+|[[:space:]]+$/, "", value)
            if (value ~ /^".*"$/ || value ~ /^\x27.*\x27$/)
                value=substr(value, 2, length(value) - 2)
            print value
            exit
        }
    ' "$file"
}

valid_team_id() { [[ "${1:-}" =~ ^[A-Z0-9]{10}$ ]]; }
valid_key_id() { [[ "${1:-}" =~ ^[A-Z0-9]{10}$ ]]; }
valid_issuer_id() { [[ "${1:-}" =~ ^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$ ]]; }

mask_file() {
    local file="$1"
    [[ -f "$file" ]] || return 0
    [[ -n "${RS_MASK_TEAM:-}${RS_MASK_KEY:-}${RS_MASK_ISSUER:-}" ]] || return 0
    python3 - "$file" <<'PY'
import os
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
data = path.read_text(encoding="utf-8", errors="replace")
for env, replacement in (("RS_MASK_TEAM", "<TEAM_ID>"), ("RS_MASK_KEY", "<KEY_ID>"), ("RS_MASK_ISSUER", "<ISSUER_ID>")):
    value = os.environ.get(env, "")
    if value:
        data = data.replace(value, replacement)
path.write_text(data, encoding="utf-8")
PY
}

mask_stream() {
    python3 - <<'PY'
import os
import sys

data = sys.stdin.read()
for env, replacement in (("RS_MASK_TEAM", "<TEAM_ID>"), ("RS_MASK_KEY", "<KEY_ID>"), ("RS_MASK_ISSUER", "<ISSUER_ID>")):
    value = os.environ.get(env, "")
    if value:
        data = data.replace(value, replacement)
sys.stdout.write(data)
PY
}

write_export_options() {
    local output="$1" team="$2" destination="$3"
    [[ "$destination" == export || "$destination" == upload ]] || {
        printf 'invalid export destination: %s\n' "$destination" >&2
        return 1
    }
    python3 - "$output" "$team" "$destination" <<'PY'
import pathlib
import plistlib
import sys

output = pathlib.Path(sys.argv[1])
team = sys.argv[2]
destination = sys.argv[3]
output.parent.mkdir(parents=True, exist_ok=True)
with output.open("wb") as stream:
    plistlib.dump({
        "method": "app-store-connect",
        "destination": destination,
        "teamID": team,
        "signingStyle": "automatic",
        "uploadSymbols": True,
        "manageAppVersionAndBuildNumber": False,
        "stripSwiftSymbols": True,
    }, stream, sort_keys=False)
PY
}
