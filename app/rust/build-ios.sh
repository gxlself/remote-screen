#!/bin/bash
# One archive per platform owns both the UniFFI control plane and the C ABI.
set -euo pipefail

RS_APP_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RS_REPO_ROOT="$(cd "$RS_APP_ROOT/.." && pwd)"
if [[ -z "${RS_RUSTUP:-}" ]]; then
  if [[ -x /opt/homebrew/opt/rustup/bin/rustup ]]; then
    RS_RUSTUP=/opt/homebrew/opt/rustup/bin/rustup
  elif RS_RUSTUP="$(command -v rustup 2>/dev/null)"; then
    :
  elif [[ -x "${HOME:-}/.cargo/bin/rustup" ]]; then
    RS_RUSTUP="$HOME/.cargo/bin/rustup"
  else
    echo 'error: rustup not found; set RS_RUSTUP or install it via Homebrew or ~/.cargo/bin' >&2
    exit 127
  fi
fi
RS_PROFILE="${RS_MOBILE_PROFILE:-${CONFIGURATION:-release}}"
case "$RS_PROFILE" in
  Debug|debug) RS_PROFILE=debug; RS_PROFILE_ARGS=(--profile dev) ;;
  Release|release) RS_PROFILE=release; RS_PROFILE_ARGS=(--release) ;;
  *) echo 'RS_MOBILE_PROFILE must be debug or release' >&2; exit 2 ;;
esac
RS_TARGET_DIR="${CARGO_TARGET_DIR:-$RS_REPO_ROOT/target}"
RS_RUSTC="$("$RS_RUSTUP" which --toolchain stable rustc)"
RS_FEATURE_ARGS=()
RS_TAILNET_ENV=()
RS_TAILNET_PATH=off
if [[ "${RS_TAILNET_IOS:-}" == off ]]; then
  :
elif [[ -n "${RS_TAILNET_IOS:-}" ]]; then
  RS_TAILNET_PATH="$RS_TAILNET_IOS"
elif [[ -d "$RS_APP_ROOT/artifacts/RsTailnet.xcframework" ]]; then
  RS_TAILNET_PATH="$RS_APP_ROOT/artifacts/RsTailnet.xcframework"
elif [[ -d "${RS_TAILNET_OUT:-$RS_REPO_ROOT/tailnet/out}/ios/RsTailnet.xcframework" ]]; then
  RS_TAILNET_PATH="${RS_TAILNET_OUT:-$RS_REPO_ROOT/tailnet/out}/ios/RsTailnet.xcframework"
fi
if [[ "$RS_TAILNET_PATH" != off ]]; then
  if [[ ! -d "$RS_TAILNET_PATH" ]]; then
    echo "error: RsTailnet.xcframework not found: $RS_TAILNET_PATH" >&2
    exit 2
  fi
  RS_TAILNET_PATH="$(cd "$RS_TAILNET_PATH" && pwd)"
  if [[ ! -f "$RS_TAILNET_PATH/Info.plist" ]]; then
    echo "error: RsTailnet.xcframework has no Info.plist: $RS_TAILNET_PATH" >&2
    exit 2
  fi
  RS_FEATURE_ARGS=(--features tailnet-static)
  RS_TAILNET_ENV=("RS_TAILNET_IOS=$RS_TAILNET_PATH")
fi
# Xcode exports both values; shell builds must use the same environment or
# cc-rs invalidates ring and rebuilds the entire TLS stack on every transition.
RS_DEVELOPER_DIR="${DEVELOPER_DIR:-$(xcode-select -p)}"
RS_TOOLCHAINS="${TOOLCHAINS:-com.apple.dt.toolchain.XcodeDefault}"
RS_XCRUN_ENV=(
  env -u SDKROOT -u LIBRARY_PATH -u CPATH -u CFLAGS -u CXXFLAGS
  DEVELOPER_DIR="$RS_DEVELOPER_DIR" TOOLCHAINS="$RS_TOOLCHAINS"
)
# Resolve the SDK compilers once so cc-rs and cmake see the same absolute paths
# from the shell and from Xcode's Run Script environment.
RS_CC_IOS="$("${RS_XCRUN_ENV[@]}" xcrun --sdk iphoneos -f clang)"
RS_CXX_IOS="$("${RS_XCRUN_ENV[@]}" xcrun --sdk iphoneos -f clang++)"
RS_CC_IOS_SIM="$("${RS_XCRUN_ENV[@]}" xcrun --sdk iphonesimulator -f clang)"
RS_CXX_IOS_SIM="$("${RS_XCRUN_ENV[@]}" xcrun --sdk iphonesimulator -f clang++)"
# opusic-sys reruns its build script whenever ANDROID_NDK_HOME changes. Xcode's
# Run Script sees it from the login profile on machines with the Android SDK,
# a plain shell may not, so drop it: iOS builds never need it.
for RS_TARGET in aarch64-apple-ios aarch64-apple-ios-sim; do
  env -u SDKROOT -u LIBRARY_PATH -u CPATH -u CFLAGS -u CXXFLAGS -u ANDROID_NDK_HOME \
    DEVELOPER_DIR="$RS_DEVELOPER_DIR" TOOLCHAINS="$RS_TOOLCHAINS" \
    CC_aarch64_apple_ios="$RS_CC_IOS" \
    CXX_aarch64_apple_ios="$RS_CXX_IOS" \
    CC_aarch64_apple_ios_sim="$RS_CC_IOS_SIM" \
    CXX_aarch64_apple_ios_sim="$RS_CXX_IOS_SIM" \
    ${RS_TAILNET_ENV[@]+"${RS_TAILNET_ENV[@]}"} \
    IPHONEOS_DEPLOYMENT_TARGET=16.0 RUSTC="$RS_RUSTC" \
    "$RS_RUSTUP" run stable cargo build --locked \
      --manifest-path "$RS_REPO_ROOT/Cargo.toml" -p rs-mobile \
      --target "$RS_TARGET" "${RS_PROFILE_ARGS[@]}" ${RS_FEATURE_ARGS[@]+"${RS_FEATURE_ARGS[@]}"}
done

RS_DEVICE_LIB="$RS_TARGET_DIR/aarch64-apple-ios/$RS_PROFILE/librs_mobile.a"
RS_SIM_LIB="$RS_TARGET_DIR/aarch64-apple-ios-sim/$RS_PROFILE/librs_mobile.a"
RS_FRAMEWORK="$RS_APP_ROOT/ios/RsMobile.xcframework"
# Generate into a mirror of the app layout so the relative includes remain
# identical. Copy back only changed bytes, keeping Xcode's C++ cache useful.
mkdir -p "$RS_APP_ROOT/artifacts"
RS_STAGE="$(mktemp -d "$RS_APP_ROOT/artifacts/bindgen.XXXXXX")"
trap 'rm -rf "$RS_STAGE"' EXIT
python3 - "$RS_APP_ROOT" "$RS_REPO_ROOT" "$RS_STAGE" <<'PY'
from pathlib import Path
import json
import shutil
import sys

app, repo, stage = map(Path, sys.argv[1:])
core = stage / 'app/native/core'
core.mkdir(parents=True)
shutil.copyfile(app / 'native/core/package.json', core / 'package.json')
config = (app / 'native/core/ubrn.config.yaml').read_text()
config = config.replace('directory: ../../../', 'directory: ' + json.dumps(str(repo)), 1)
(core / 'ubrn.config.yaml').write_text(config)
PY
cd "$RS_STAGE/app/native/core"
RS_UBRN_ARGS=(build ios --config ubrn.config.yaml --no-cargo --no-xcodebuild --and-generate)
if [[ "$RS_PROFILE" == release ]]; then RS_UBRN_ARGS+=(--release); fi
"$RS_APP_ROOT/node_modules/.bin/ubrn" "${RS_UBRN_ARGS[@]}"
for RS_GENERATED in src/native/generated cpp/generated native/core/src native/core/cpp native/core/ios; do
  python3 "$RS_APP_ROOT/rust/sync-generated.py" "$RS_STAGE/app/$RS_GENERATED" "$RS_APP_ROOT/$RS_GENERATED"
done

# Xcode's copy phase must see the archive built for this configuration. Avoid
# recreating unchanged slices (and needlessly relinking the app) on no-op builds.
if [[ ! -f "$RS_FRAMEWORK/Info.plist" ]] || \
    ! cmp -s "$RS_DEVICE_LIB" "$RS_FRAMEWORK/ios-arm64/librs_mobile.a" || \
    ! cmp -s "$RS_SIM_LIB" "$RS_FRAMEWORK/ios-arm64-simulator/librs_mobile.a"; then
  xcodebuild -create-xcframework -library "$RS_DEVICE_LIB" -library "$RS_SIM_LIB" \
    -output "$RS_STAGE/RsMobile.xcframework"
  python3 "$RS_APP_ROOT/rust/sync-generated.py" "$RS_STAGE/RsMobile.xcframework" "$RS_FRAMEWORK"
fi
if [[ -n "${DERIVED_FILE_DIR:-}" ]]; then
  mkdir -p "$DERIVED_FILE_DIR"
  touch "$DERIVED_FILE_DIR/rs-mobile-${CONFIGURATION:-$RS_PROFILE}.stamp"
fi
echo "rs-mobile: $RS_PROFILE; xcframework=$RS_FRAMEWORK; bindings=src/native/generated + cpp/generated; tailnet=$RS_TAILNET_PATH"
