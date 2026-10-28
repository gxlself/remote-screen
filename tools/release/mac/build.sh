#!/usr/bin/env bash
set -euo pipefail

source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)/common.sh"

usage() {
    cat <<'USAGE'
Usage: tools/release/mac/build.sh [--arch native|universal] [--out DIR] [--skip-ui-install]

Build an ad-hoc signed Remote Host.app. Defaults to the native architecture and
target/rs-release/mac/. RS_RELEASE_PUBKEYS is passed through without modification.
USAGE
}

arch=native
out="$RS_MAC_REPO_ROOT/target/rs-release/mac"
skip_ui_install=false
while [[ $# -gt 0 ]]; do
    case "$1" in
        --arch)
            [[ $# -ge 2 ]] || die '--arch requires native or universal.'
            arch="$2"; shift 2 ;;
        --out)
            [[ $# -ge 2 && -n "$2" ]] || die '--out requires a nonempty directory.'
            out="$2"; shift 2 ;;
        --skip-ui-install) skip_ui_install=true; shift ;;
        --help|-h) usage; exit 0 ;;
        *) usage >&2; die "Unknown argument: $1" ;;
    esac
done
[[ "$arch" == native || "$arch" == universal ]] || die "Unsupported --arch: $arch"
case "$out" in
    /*) ;;
    *) out="$PWD/$out" ;;
esac
require_macos
for tool in cargo rustc pnpm python3 lipo otool codesign ditto; do
    require_command "$tool"
done

case "$(uname -m)" in
    arm64) native_target=aarch64-apple-darwin ;;
    x86_64) native_target=x86_64-apple-darwin ;;
    *) die 'Unsupported Mac CPU architecture.' ;;
esac
if [[ "$arch" == universal ]]; then
    targets=(aarch64-apple-darwin x86_64-apple-darwin)
    ui_target=universal-apple-darwin
else
    targets=("$native_target")
    ui_target="$native_target"
fi

# Query the active compiler, not just rustup's installed-target list: Homebrew
# rustc can have a different sysroot from the rustup toolchain.
for target in "${targets[@]}"; do
    libdir="$(rustc --print target-libdir --target "$target")"
    if ! compgen -G "$libdir/libstd-*.rlib" >/dev/null; then
        die "Rust target $target is missing for the active rustc. Install it with: rustup target add $target (then use that rustup toolchain's cargo/rustc). No target was installed automatically."
    fi
done

version="$(read_version)"
workspace_version="$(read_workspace_version)"
[[ -n "$version" ]] || die 'Could not read the Tauri app version.'
if [[ "$version" != "$workspace_version" ]]; then
    warn "App version $version differs from workspace/agent version $workspace_version."
fi
if [[ -z "${RS_RELEASE_PUBKEYS:-}" ]]; then
    warn 'RS_RELEASE_PUBKEYS 为空，自动更新将关闭。'
fi

cd "$RS_MAC_REPO_ROOT"
cargo_target_dir="$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')"
for target in "${targets[@]}"; do
    log "Building agent $workspace_version for $target"
    cargo build --locked --release -p rs-host-agent --target "$target"
done

sidecar_dir="$RS_MAC_REPO_ROOT/host-ui/src-tauri/binaries"
mkdir -p "$sidecar_dir"
sidecar="$sidecar_dir/rs-host-agent-$ui_target"
if [[ "$arch" == universal ]]; then
    lipo -create \
        "$cargo_target_dir/aarch64-apple-darwin/release/rs-host" \
        "$cargo_target_dir/x86_64-apple-darwin/release/rs-host" \
        -output "$sidecar"
    # Tauri's universal build also compiles each thin UI, whose build.rs checks
    # externalBin with that thin target triple before the final lipo/bundle step.
    for target in "${targets[@]}"; do
        cp "$cargo_target_dir/$target/release/rs-host" "$sidecar_dir/rs-host-agent-$target"
    done
else
    cp "$cargo_target_dir/$native_target/release/rs-host" "$sidecar"
fi
chmod 755 "$sidecar"

cd "$RS_MAC_REPO_ROOT/host-ui"
if [[ "$skip_ui_install" != true ]]; then
    log 'Installing locked UI dependencies'
    pnpm install --frozen-lockfile
fi
ui_target_dir="$(cargo metadata --manifest-path src-tauri/Cargo.toml --no-deps --format-version 1 | python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')"
log "Building Tauri app for $ui_target"
# Cargo's UI executable is remote-host-ui. Rename only the packaged executable;
# the checked-in Tauri config and the development binary keep their existing names.
# Sign explicitly below so no Developer ID credentials or notarization are used.
pnpm tauri build --bundles app --target "$ui_target" --no-sign \
    --config '{"mainBinaryName":"Remote Host"}'
built_app="$ui_target_dir/$ui_target/release/bundle/macos/$RS_MAC_APP_NAME"
[[ -d "$built_app" ]] || die "Tauri did not produce $built_app"

mkdir -p "$out"
out="$(cd "$out" && pwd -P)"
staging="$(mktemp -d "$out/.rs-mac-build.XXXXXX")"
cleanup() {
    if [[ -n "${staging:-}" && "$staging" == "$out"/.rs-mac-build.* ]]; then
        rm -rf -- "$staging"
    fi
}
trap cleanup EXIT
app="$staging/$RS_MAC_APP_NAME"
ditto "$built_app" "$app"
verify_structure "$app" "$arch"

log 'Signing the agent, then the app (ad-hoc, hardened runtime)'
codesign --force --sign - --options runtime --identifier "$RS_MAC_AGENT_ID" \
    "$app/Contents/MacOS/rs-host-agent"
codesign --force --sign - --options runtime "$app"
codesign --verify --strict --verbose=2 "$app"

output_app="$out/$RS_MAC_APP_NAME"
[[ "$output_app" != "$built_app" ]] || die '--out must not be the Tauri bundle directory.'
if [[ -e "$output_app" || -L "$output_app" ]]; then
    log "Replacing previous build at $output_app"
    rm -rf -- "$output_app"
fi
mv "$app" "$output_app"
log "Built $output_app"
python3 - "$output_app" "$version" <<'PY'
import json
import pathlib
import subprocess
import sys

app = pathlib.Path(sys.argv[1])
print(json.dumps({
    "app": str(app),
    "version": sys.argv[2],
    "archs": subprocess.check_output(["lipo", "-archs", str(app / "Contents/MacOS/Remote Host")], text=True).split(),
    "size_bytes": sum(path.stat().st_size for path in app.rglob("*") if path.is_file() and not path.is_symlink()),
    "signed": "adhoc",
}, ensure_ascii=False))
PY
