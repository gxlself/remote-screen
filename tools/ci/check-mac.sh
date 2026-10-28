#!/usr/bin/env bash
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

# Match check-linux.sh: run every section and report all failures at the end.
failed=()
section() {
    local name=$1
    shift
    echo "== $name"
    "$@" && return 0
    failed+=("$name")
    return 1
}

check_tools() {
    local tool missing=0
    tool_version() {
        case "$1" in
            go) go version ;;
            xcodebuild) xcodebuild -version | head -n 1 ;;
            xcode-select) xcode-select -p ;;
            codesign|lipo|otool|ditto|plutil) command -v "$1" ;;
            *) "$1" --version 2>&1 | head -n 1 ;;
        esac
    }
    for tool in cargo cargo-nextest go pnpm node rustc rustup xcodebuild swift pod \
        python3 lipo otool codesign ditto plutil; do
        if command -v "$tool" >/dev/null 2>&1; then
            printf '%-16s %s\n' "$tool" "$(tool_version "$tool")"
        else
            printf 'MISSING: %s\n' "$tool" >&2
            missing=1
        fi
    done
    return "$missing"
}

section tools check_tools

section clippy cargo clippy -p host-mac --all-targets -- -D warnings

# VT-unavailable native encoding tests skip according to the existing test
# rules; do not set RS_REQUIRE_VT here.
section tests cargo nextest run -p host-mac --no-fail-fast

section desktop-clippy cargo clippy -p rs-desktop --all-targets -- -D warnings
section desktop-tests cargo nextest run -p rs-desktop --no-fail-fast

section app-install bash -c 'cd app && pnpm install --frozen-lockfile'

if [[ -z ${RS_RUSTUP:-} ]] && command -v rustup >/dev/null 2>&1; then
    RS_RUSTUP="$(command -v rustup)"
    export RS_RUSTUP
fi
if [[ -z ${RS_POD:-} ]] && command -v pod >/dev/null 2>&1; then
    RS_POD="$(command -v pod)"
    export RS_POD
fi

section ios-build bash -c \
    'cd app && LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8 bash scripts/build-ios.sh Debug iphonesimulator'

section swift-test bash -c 'cd app/ios/NativeLogic && swift test'

# tools/release/mac/build.sh is the existing ad-hoc/no-Developer-ID packaging
# entry point. It signs locally with `codesign --sign -`; CI has no distribution
# signing key and does not notarize.
section mac-host tools/release/mac/build.sh --arch native --out "$ROOT/target/ci-mac-host"

if ((${#failed[@]})); then
    echo "CHECK FAILED: ${failed[*]}"
    exit 1
fi
echo "CHECK OK"
