#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'HELP'
Usage: tailnet/scripts/build-ios.sh [-h|--help]
Build ios-arm64 and ios-arm64-simulator slices of RsTailnet.xcframework on macOS.
Environment:
  RS_TAILNET_OUT                output root (default: <repo>/tailnet/out)
  RS_TAILNET_TAGS               additional Go build tags (default: empty)
  IPHONEOS_DEPLOYMENT_TARGET    minimum iOS version (default: 16.0)
  DEVELOPER_DIR / TOOLCHAINS    optional Xcode selection
Requires Go >= 1.26, Xcode, and iPhoneOS/iPhoneSimulator SDKs in PATH.
HELP
}
for arg in "$@"; do
  case "$arg" in
    -h|--help) usage; exit 0 ;;
    *) printf 'unknown argument: %s\n' "$arg" >&2; usage >&2; exit 2 ;;
  esac
done
if [[ "$(uname -s)" != Darwin ]]; then
  echo 'error: iOS builds require macOS and Xcode; run on a Mac with DEVELOPER_DIR set to Xcode' >&2
  exit 2
fi
for RS_TOOL in go git xcrun xcodebuild lipo otool nm; do
  if ! command -v "$RS_TOOL" >/dev/null 2>&1; then
    printf 'error: %s not found; set PATH and DEVELOPER_DIR to Go/Xcode\n' "$RS_TOOL" >&2; exit 2
  fi
done
RS_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-16.0}"
if [[ ! "$RS_TARGET" =~ ^[0-9]+\.[0-9]+(\.[0-9]+)?$ ]]; then
  echo 'error: set IPHONEOS_DEPLOYMENT_TARGET to an iOS version such as 16.0' >&2; exit 2
fi
RS_ENV=(env -u SDKROOT -u LIBRARY_PATH -u CPATH -u CFLAGS -u CXXFLAGS)
if ! RS_DEVICE_CC="$("${RS_ENV[@]}" xcrun --sdk iphoneos -f clang)" ||
   ! RS_SIM_CC="$("${RS_ENV[@]}" xcrun --sdk iphonesimulator -f clang)" ||
   ! RS_DEVICE_SDK="$("${RS_ENV[@]}" xcrun --sdk iphoneos --show-sdk-path)" ||
   ! RS_SIM_SDK="$("${RS_ENV[@]}" xcrun --sdk iphonesimulator --show-sdk-path)"; then
  echo 'error: iOS SDKs unavailable; set DEVELOPER_DIR to a complete Xcode installation' >&2; exit 2
fi
RS_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RS_OUT="${RS_TAILNET_OUT:-$RS_ROOT/out}"
if [[ "$RS_OUT" != /* ]]; then RS_OUT="$PWD/$RS_OUT"; fi
RS_TAGS="${RS_TAILNET_TAGS:-}"
if ! mkdir -p "$RS_OUT" || [[ ! -w "$RS_OUT" ]]; then
  echo 'error: set RS_TAILNET_OUT to a writable output directory' >&2; exit 2
fi
RS_OUT="$(cd "$RS_OUT" && pwd)"
RS_STAGE="$(mktemp -d "$RS_OUT/.build-ios.XXXXXX")"
trap 'rm -rf "$RS_STAGE"' EXIT
cd "$RS_ROOT"
install_file() {
  local src="$1" dest="$RS_OUT/$2"
  mkdir -p "$(dirname "$dest")"
  if [[ -f "$dest" ]] && cmp -s "$src" "$dest"; then
    printf '%s: unchanged\n' "$2"
  else
    mv -f "$src" "$dest"
    printf '%s: updated (%s bytes)\n' "$2" "$(wc -c < "$dest" | tr -d '[:space:]')"
  fi
}
write_metadata() {
  "${RS_ENV[@]}" go list -tags "$RS_TAGS" -deps -f '{{with .Module}}{{.Path}} {{.Version}}{{end}}' ./cmd/rstailnet | LC_ALL=C sort -u > "$RS_STAGE/modules.txt"
  {
    go version
    printf 'tags: %s\n' "$RS_TAGS"
    printf 'tailnet commit: %s\n' "$(git log -1 --format=%H -- .)"
    for dir in include host android ios; do
      if [[ -d "$RS_OUT/$dir" ]]; then
        (cd "$RS_OUT" && find "$dir" -type f | LC_ALL=C sort | while IFS= read -r file; do
          printf '%s: %s bytes\n' "$file" "$(wc -c < "$file" | tr -d '[:space:]')"
        done)
      fi
    done
  } > "$RS_STAGE/BUILDINFO"
  install_file "$RS_STAGE/modules.txt" modules.txt
  install_file "$RS_STAGE/BUILDINFO" BUILDINFO
}

check_archive() {
  local archive="$1" platform="$2" slice="$3" arch
  lipo -info "$archive"
  arch="$(lipo -archs "$archive")"
  if [[ "$arch" != arm64 ]]; then
    printf 'error: %s architecture is %s, expected arm64\n' "$slice" "$arch" >&2; return 1
  fi
  otool -l "$archive" > "$RS_STAGE/load-commands"
  # Validate every LC_BUILD_VERSION, including all cgo archive members.
  awk '
    $1 == "cmd" {build=($2 == "LC_BUILD_VERSION"); platform=""}
    build && $1 == "platform" {platform=$2}
    build && $1 == "minos" {print platform, $2}
  ' "$RS_STAGE/load-commands" | LC_ALL=C sort -u > "$RS_STAGE/platforms"
  if [[ ! -s "$RS_STAGE/platforms" ]] || ! awk -v expected="$platform" -v target="$RS_TARGET" '
    function version(s) {sub(/\.0$/, "", s); sub(/\.0$/, "", s); return s}
    $1 != expected || version($2) != version(target) {bad=1}
    END {exit bad}
  ' "$RS_STAGE/platforms"; then
    printf 'error: %s requires LC_BUILD_VERSION platform %s, minos %s; got:\n' "$slice" "$platform" "$RS_TARGET" >&2
    cat "$RS_STAGE/platforms" >&2
    if [[ "$platform" == 7 ]]; then
      echo 'Simulator platform 2 is invalid: use a Go/Xcode toolchain supporting iOS simulator c-archive; do not rewrite with vtool.' >&2
    fi
    return 1
  fi
  nm -gU "$archive" | awk '$NF ~ /^_rs_ts_/ {sub(/^_/, "", $NF); print $NF}' | LC_ALL=C sort -u > "$RS_STAGE/actual"
  if [[ "$(wc -l < "$RS_STAGE/expected" | tr -d '[:space:]')" != 10 ]] || ! cmp -s "$RS_STAGE/expected" "$RS_STAGE/actual"; then
    echo 'error: ABI symbols differ; missing:' >&2
    LC_ALL=C comm -23 "$RS_STAGE/expected" "$RS_STAGE/actual" >&2
    echo 'extra:' >&2
    LC_ALL=C comm -13 "$RS_STAGE/expected" "$RS_STAGE/actual" >&2
    return 1
  fi
  printf 'ok %s: arm64, platform %s, minos %s, 10 symbols (%s bytes)\n' "$slice" "$platform" "$RS_TARGET" "$(wc -c < "$archive" | tr -d '[:space:]')"
}
sed -nE 's/^int(32|64)_t (rs_ts_[a-z_]+)\(.*/\2/p' include/rs_tailnet.h | LC_ALL=C sort -u > "$RS_STAGE/expected"
mkdir -p "$RS_STAGE/ios-arm64" "$RS_STAGE/ios-arm64-simulator" "$RS_STAGE/Headers"
cp include/rs_tailnet.h "$RS_STAGE/Headers/rs_tailnet.h"
export GOOS=ios GOARCH=arm64 CGO_ENABLED=1 IPHONEOS_DEPLOYMENT_TARGET="$RS_TARGET"
for RS_SLICE in ios-arm64 ios-arm64-simulator; do
  case "$RS_SLICE" in
    ios-arm64)
      export CC="$RS_DEVICE_CC"
      RS_FLAGS="-isysroot \"$RS_DEVICE_SDK\" -arch arm64 -miphoneos-version-min=$RS_TARGET"
      RS_PLATFORM=2
      ;;
    ios-arm64-simulator)
      export CC="$RS_SIM_CC"
      RS_FLAGS="-isysroot \"$RS_SIM_SDK\" -target arm64-apple-ios$RS_TARGET-simulator"
      RS_PLATFORM=7
      ;;
  esac
  export CGO_CFLAGS="$RS_FLAGS" CGO_LDFLAGS="$RS_FLAGS"
  "${RS_ENV[@]}" go build -buildvcs=false -tags "$RS_TAGS" -buildmode=c-archive -trimpath -ldflags '-s -w' -o "$RS_STAGE/$RS_SLICE/librs_tailnet.a" ./cmd/rstailnet
  check_archive "$RS_STAGE/$RS_SLICE/librs_tailnet.a" "$RS_PLATFORM" "$RS_SLICE"
done
RS_FRAMEWORK="$RS_OUT/ios/RsTailnet.xcframework"
RS_CHANGED=0
if [[ ! -f "$RS_FRAMEWORK/Info.plist" ]]; then RS_CHANGED=1; fi
for RS_SLICE in ios-arm64 ios-arm64-simulator; do
  if ! cmp -s "$RS_STAGE/$RS_SLICE/librs_tailnet.a" "$RS_FRAMEWORK/$RS_SLICE/librs_tailnet.a" ||
     ! cmp -s include/rs_tailnet.h "$RS_FRAMEWORK/$RS_SLICE/Headers/rs_tailnet.h"; then
    RS_CHANGED=1
  fi
done
if ((RS_CHANGED)); then
  # Construct in staging first so a failed Xcode command preserves the last build.
  "${RS_ENV[@]}" xcodebuild -create-xcframework \
    -library "$RS_STAGE/ios-arm64/librs_tailnet.a" -headers "$RS_STAGE/Headers" \
    -library "$RS_STAGE/ios-arm64-simulator/librs_tailnet.a" -headers "$RS_STAGE/Headers" \
    -output "$RS_STAGE/RsTailnet.xcframework"
  for RS_SLICE in ios-arm64 ios-arm64-simulator; do
    if ! cmp -s "$RS_STAGE/$RS_SLICE/librs_tailnet.a" "$RS_STAGE/RsTailnet.xcframework/$RS_SLICE/librs_tailnet.a" ||
       ! cmp -s include/rs_tailnet.h "$RS_STAGE/RsTailnet.xcframework/$RS_SLICE/Headers/rs_tailnet.h"; then
      echo 'error: xcframework slice layout does not match the ABI contract' >&2; exit 1
    fi
  done
  mkdir -p "$RS_OUT/ios"
  rm -rf "$RS_FRAMEWORK"
  mv "$RS_STAGE/RsTailnet.xcframework" "$RS_FRAMEWORK"
  RS_BYTES="$(find "$RS_FRAMEWORK" -type f -exec wc -c {} \; | awk '{sum+=$1} END {printf "%.0f",sum}')"
  printf 'ios/RsTailnet.xcframework: updated (%s bytes)\n' "$RS_BYTES"
else
  echo 'ios/RsTailnet.xcframework: unchanged'
fi
cp include/rs_tailnet.h "$RS_STAGE/rs_tailnet.h"
install_file "$RS_STAGE/rs_tailnet.h" include/rs_tailnet.h
write_metadata
printf 'rs-tailnet ios: target=%s; xcframework=%s\n' "$RS_TARGET" "$RS_FRAMEWORK"
echo 'App linking requires: -lresolv -framework Security -framework CoreFoundation'
