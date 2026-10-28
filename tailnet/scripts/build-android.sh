#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'HELP'
Usage: tailnet/scripts/build-android.sh [-h|--help]
Build Android shared libraries with 16 KB page alignment.
Environment:
  ANDROID_NDK_HOME / ANDROID_NDK_ROOT  Android NDK root (27.1 or newer)
  RS_ANDROID_ABIS   space-separated ABIs (default: arm64-v8a x86_64)
  RS_ANDROID_API    Android API level (default: 26)
  RS_TAILNET_OUT    output root (default: <repo>/tailnet/out)
  RS_TAILNET_TAGS   additional Go build tags (default: empty)
HELP
}
for arg in "$@"; do
  case "$arg" in
    -h|--help) usage; exit 0 ;;
    *) printf 'unknown argument: %s\n' "$arg" >&2; usage >&2; exit 2 ;;
  esac
done
RS_ABIS=()
read -r -a RS_ABIS <<< "${RS_ANDROID_ABIS:-arm64-v8a x86_64}"
if ((${#RS_ABIS[@]} == 0)); then
  echo 'error: set RS_ANDROID_ABIS to arm64-v8a and/or x86_64' >&2; exit 2
fi
for RS_ABI in "${RS_ABIS[@]}"; do
  case "$RS_ABI" in
    arm64-v8a|x86_64) ;;
    *) printf 'unsupported ABI: %s\n' "$RS_ABI" >&2; exit 2 ;;
  esac
done
RS_API="${RS_ANDROID_API:-26}"
if [[ ! "$RS_API" =~ ^[1-9][0-9]*$ ]]; then
  echo 'error: set RS_ANDROID_API to a positive integer supported by the NDK' >&2; exit 2
fi
RS_NDK="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-}}"
if [[ ! -d "$RS_NDK" ]]; then
  echo 'error: set ANDROID_NDK_HOME or ANDROID_NDK_ROOT to the Android NDK root' >&2; exit 2
fi
case "$(uname -s)" in
  Linux) RS_NDK_HOST=linux-x86_64 ;;
  Darwin) RS_NDK_HOST=darwin-x86_64 ;;
  *) echo 'error: NDK builds require Linux/macOS; set ANDROID_NDK_HOME on a supported host' >&2; exit 2 ;;
esac
RS_BIN="$RS_NDK/toolchains/llvm/prebuilt/$RS_NDK_HOST/bin"
for RS_TOOL in go git "$RS_BIN/llvm-readelf" "$RS_BIN/llvm-nm"; do
  if ! command -v "$RS_TOOL" >/dev/null 2>&1; then
    printf 'error: %s not found; set PATH or ANDROID_NDK_HOME\n' "$RS_TOOL" >&2; exit 2
  fi
done
for RS_ABI in "${RS_ABIS[@]}"; do
  case "$RS_ABI" in
    arm64-v8a) RS_TRIPLE=aarch64-linux-android ;;
    x86_64) RS_TRIPLE=x86_64-linux-android ;;
  esac
  if [[ ! -x "$RS_BIN/$RS_TRIPLE$RS_API-clang" ]]; then
    echo 'error: NDK compiler missing; set ANDROID_NDK_HOME and RS_ANDROID_API to supported values' >&2; exit 2
  fi
done
RS_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RS_OUT="${RS_TAILNET_OUT:-$RS_ROOT/out}"
if [[ "$RS_OUT" != /* ]]; then RS_OUT="$PWD/$RS_OUT"; fi
RS_TAGS="${RS_TAILNET_TAGS:-}"
if ! mkdir -p "$RS_OUT" || [[ ! -w "$RS_OUT" ]]; then
  echo 'error: set RS_TAILNET_OUT to a writable output directory' >&2; exit 2
fi
RS_OUT="$(cd "$RS_OUT" && pwd)"
RS_STAGE="$(mktemp -d "$RS_OUT/.build-android.XXXXXX")"
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
  go list -tags "$RS_TAGS" -deps -f '{{with .Module}}{{.Path}} {{.Version}}{{end}}' ./cmd/rstailnet | LC_ALL=C sort -u > "$RS_STAGE/modules.txt"
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

check_library() {
  local lib="$1" align count=0 soname needed
  "$RS_BIN/llvm-readelf" -d "$lib" > "$RS_STAGE/dynamic"
  "$RS_BIN/llvm-readelf" -lW "$lib" > "$RS_STAGE/segments"
  soname="$(sed -nE 's/.*\(SONAME\).*\[([^]]+)\].*/\1/p' "$RS_STAGE/dynamic")"
  if [[ "$soname" != librs_tailnet.so ]]; then
    printf 'error: unexpected SONAME: %s\n' "$soname" >&2; return 1
  fi
  while IFS= read -r needed; do
    case "$needed" in
      libc.so|libdl.so|libm.so|liblog.so) ;;
      *) printf 'error: unexpected NEEDED: %s\n' "$needed" >&2; return 1 ;;
    esac
  done < <(sed -nE 's/.*\(NEEDED\).*\[([^]]+)\].*/\1/p' "$RS_STAGE/dynamic")
  while IFS= read -r align; do
    if [[ ! "$align" =~ ^0x[0-9a-fA-F]+$ ]] || ((align == 0 || align % 16384 != 0)); then
      printf 'error: LOAD alignment %s is not a positive multiple of 0x4000\n' "$align" >&2; return 1
    fi
    count=$((count+1))
  done < <(awk '$1 == "LOAD" {print $NF}' "$RS_STAGE/segments")
  if ((count == 0)); then echo 'error: no LOAD segments' >&2; return 1; fi
  "$RS_BIN/llvm-nm" -D --defined-only "$lib" | awk '$NF ~ /^rs_ts_/ {print $NF}' | LC_ALL=C sort -u > "$RS_STAGE/actual"
  if [[ "$(wc -l < "$RS_STAGE/expected" | tr -d '[:space:]')" != 10 ]] || ! cmp -s "$RS_STAGE/expected" "$RS_STAGE/actual"; then
    echo 'error: ABI symbols differ; missing:' >&2
    LC_ALL=C comm -23 "$RS_STAGE/expected" "$RS_STAGE/actual" >&2
    echo 'extra:' >&2
    LC_ALL=C comm -13 "$RS_STAGE/expected" "$RS_STAGE/actual" >&2
    return 1
  fi
  printf 'ok %s: SONAME, NEEDED, %s LOAD alignments, 10 symbols\n' "$RS_ABI" "$count"
  awk '/\(SONAME\)|\(NEEDED\)/' "$RS_STAGE/dynamic"
  awk '$1 == "LOAD"' "$RS_STAGE/segments"
}
sed -nE 's/^int(32|64)_t (rs_ts_[a-z_]+)\(.*/\2/p' include/rs_tailnet.h | LC_ALL=C sort -u > "$RS_STAGE/expected"
export CGO_ENABLED=1 GOOS=android
for RS_ABI in "${RS_ABIS[@]}"; do
  case "$RS_ABI" in
    arm64-v8a) export GOARCH=arm64; RS_TRIPLE=aarch64-linux-android ;;
    x86_64) export GOARCH=amd64; RS_TRIPLE=x86_64-linux-android ;;
  esac
  export CC="$RS_BIN/$RS_TRIPLE$RS_API-clang"
  mkdir -p "$RS_STAGE/$RS_ABI"
  RS_LIB="$RS_STAGE/$RS_ABI/librs_tailnet.so"
  go build -buildvcs=false -tags "$RS_TAGS" -buildmode=c-shared -trimpath -ldflags "-s -w -extldflags '-Wl,-z,max-page-size=16384 -Wl,-soname,librs_tailnet.so'" -o "$RS_LIB" ./cmd/rstailnet
  check_library "$RS_LIB"
  install_file "$RS_LIB" "android/$RS_ABI/librs_tailnet.so"
done
cp include/rs_tailnet.h "$RS_STAGE/rs_tailnet.h"
install_file "$RS_STAGE/rs_tailnet.h" include/rs_tailnet.h
write_metadata
printf 'rs-tailnet android: abis=%s; api=%s; out=%s\n' "${RS_ABIS[*]}" "$RS_API" "$RS_OUT"
