#!/usr/bin/env bash

# Shared helpers for the Android release and verification workflows.  This
# file deliberately does not enable shell tracing: signing properties must
# never be echoed by a release command.

ANDROID_RELEASE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd -P)"

log() {
    printf '[android-release] %s\n' "$*" >&2
}

warn() {
    log "WARNING: $*"
}

die() {
    log "ERROR: $*"
    exit 1
}

tailnet_state_from_members() {
    local members="${1:-}"
    shift || true
    [[ -f "$members" ]] || return 2
    (($# > 0)) || return 2

    local abi present=0 missing=0 member
    for abi in "$@"; do
        member="lib/$abi/librs_tailnet.so"
        if grep -Fxq "$member" "$members"; then
            present=$((present + 1))
        else
            missing=$((missing + 1))
        fi
    done
    if ((missing == 0)); then
        printf 'on\n'
    elif ((present == 0)); then
        printf 'off\n'
    else
        printf 'partial\n'
    fi
}

# state: what the APK carries (on, off, partial, or unknown). with:
# --with-tailnet was asked for. allow: --allow-no-tailnet.
tailnet_release_gate() {
    local state="${1:-unknown}" with="${2:-false}" allow="${3:-false}"
    case "$state" in
        on)
            log 'This build includes the in-app tailnet.'
            return 0
            ;;
        unknown)
            return 0
            ;;
        partial)
            log 'ERROR: Tailnet is only present for some ABIs; every ABI must include librs_tailnet.so.'
            return 1
            ;;
        off)
            if [[ "$with" == false ]]; then
                log 'This build does not include the in-app tailnet (default; --with-tailnet to include it).'
                return 0
            fi
            if [[ "$allow" == true ]]; then
                log 'WARNING: Tailnet is not included in this release (--allow-no-tailnet).'
                return 0
            fi
            log 'ERROR: Tailnet is not included; build it or pass --allow-no-tailnet.'
            return 1
            ;;
        *)
            log "ERROR: Unknown tailnet state: $state"
            return 1
            ;;
    esac
}

repo_root() {
    printf '%s\n' "$ANDROID_RELEASE_ROOT"
}

resolve_sdk() {
    local root=""
    if [[ -n "${ANDROID_HOME:-}" ]]; then
        root="$ANDROID_HOME"
    elif [[ -n "${ANDROID_SDK_ROOT:-}" ]]; then
        root="$ANDROID_SDK_ROOT"
    else
        local properties="$ANDROID_RELEASE_ROOT/app/android/local.properties"
        if [[ -f "$properties" ]]; then
            root="$(awk '
                /^[[:space:]]*sdk\.dir[[:space:]]*=/ {
                    sub(/^[^=]*=/, "")
                    gsub(/^[[:space:]]+|[[:space:]]+$/, "")
                    print
                    exit
                }
            ' "$properties")"
            # local.properties uses Java properties escaping.  The Linux and
            # macOS SDK paths used by this project do not need other escapes;
            # handling the common escaped separator keeps this lookup useful
            # for paths copied from Android Studio.
            root="${root//\\:/:}"
        fi
    fi
    [[ -n "$root" ]] || die 'Android SDK not found: set ANDROID_HOME or ANDROID_SDK_ROOT, or add sdk.dir to app/android/local.properties.'
    if [[ "$root" != /* ]]; then
        root="$ANDROID_RELEASE_ROOT/app/android/$root"
    fi
    [[ -d "$root" ]] || die "Android SDK directory does not exist: $root"
    (cd "$root" && pwd -P)
}

resolve_build_tools() {
    local sdk="${1:-${ANDROID_RELEASE_SDK:-}}"
    [[ -n "$sdk" ]] || sdk="$(resolve_sdk)"
    local build_tools="$sdk/build-tools"
    [[ -d "$build_tools" ]] || die "Android build-tools directory does not exist: $build_tools"

    local version candidate major latest=''
    local versions=()
    for candidate in "$build_tools"/*; do
        [[ -d "$candidate" ]] || continue
        version="${candidate##*/}"
        [[ "$version" =~ ^[0-9]+([.][0-9]+)*([.-].*)?$ ]] || continue
        versions+=("$version")
    done
    ((${#versions[@]} > 0)) || die "No Android build-tools versions found under $build_tools"

    version_gt() {
        local left="$1" right="$2" base_left base_right index
        local -a left_parts right_parts
        base_left="${left%%[-+]*}"
        base_right="${right%%[-+]*}"
        IFS=. read -r -a left_parts <<<"$base_left"
        IFS=. read -r -a right_parts <<<"$base_right"
        for index in 0 1 2 3; do
            if ((10#${left_parts[$index]:-0} > 10#${right_parts[$index]:-0})); then
                return 0
            fi
            if ((10#${left_parts[$index]:-0} < 10#${right_parts[$index]:-0})); then
                return 1
            fi
        done
        return 1
    }
    for version in "${versions[@]}"; do
        candidate="$build_tools/$version"
        [[ -x "$candidate/apksigner" && -x "$candidate/zipalign" && -x "$candidate/aapt2" ]] || continue
        if [[ -z "$latest" ]] || version_gt "$version" "$latest"; then
            latest="$version"
        fi
    done
    [[ -n "$latest" ]] || die "No Android build-tools version has executable apksigner, zipalign, and aapt2."
    major="${latest%%.*}"
    [[ "$major" =~ ^[0-9]+$ ]] || die "Invalid Android build-tools version: $latest"
    ((10#$major >= 35)) || die "Android build-tools ${latest} is too old; version 35 or newer is required."
    (cd "$build_tools/$latest" && pwd -P)
}

resolve_readelf() {
    local sdk="${1:-${ANDROID_RELEASE_SDK:-}}"
    [[ -n "$sdk" ]] || sdk="$(resolve_sdk)"
    local gradle="$ANDROID_RELEASE_ROOT/app/android/build.gradle"
    local ndk_version
    ndk_version="$(sed -nE 's/^[[:space:]]*ndkVersion[[:space:]]*=[[:space:]]*"([^"]+)".*/\1/p' "$gradle" | head -n1)"
    [[ -n "$ndk_version" ]] || die 'Could not read ndkVersion from app/android/build.gradle.'

    local candidate
    for candidate in "$sdk/ndk/$ndk_version"/toolchains/llvm/prebuilt/*/bin/llvm-readelf; do
        if [[ -x "$candidate" ]]; then
            printf '%s/%s\n' "$(cd "$(dirname "$candidate")" && pwd -P)" "$(basename "$candidate")"
            return 0
        fi
    done
    die "llvm-readelf for NDK ${ndk_version} was not found under $sdk/ndk."
}

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
