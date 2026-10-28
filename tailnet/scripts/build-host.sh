#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'HELP'
Usage: tailnet/scripts/build-host.sh [-h|--help]
Build and smoke-test the native Linux/macOS C shared library and testnet helper.
Environment:
  RS_TAILNET_OUT   output root (default: <repo>/tailnet/out)
  RS_TAILNET_TAGS  additional Go build tags (default: empty)
  CC              native C compiler (default: cc); Go and nm must be in PATH
HELP
}
for arg in "$@"; do
  case "$arg" in
    -h|--help) usage; exit 0 ;;
    *) printf 'unknown argument: %s\n' "$arg" >&2; usage >&2; exit 2 ;;
  esac
done
RS_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RS_OUT="${RS_TAILNET_OUT:-$RS_ROOT/out}"
if [[ "$RS_OUT" != /* ]]; then RS_OUT="$PWD/$RS_OUT"; fi
RS_TAGS="${RS_TAILNET_TAGS:-}"
RS_CC="${CC:-cc}"
for RS_TOOL in go nm git "$RS_CC"; do
  if ! command -v "$RS_TOOL" >/dev/null 2>&1; then
    printf 'error: %s not found; set PATH (or CC for the C compiler)\n' "$RS_TOOL" >&2
    exit 2
  fi
done
cd "$RS_ROOT"
RS_GOOS="$(go env GOOS)"
RS_GOARCH="$(go env GOARCH)"
case "$RS_GOOS:$(uname -s)" in
  linux:Linux)
    RS_LIB=librs_tailnet.so
    RS_EXTLD='-Wl,-soname,librs_tailnet.so'
    RS_NM_ARGS=(-D --defined-only)
    ;;
  darwin:Darwin)
    RS_LIB=librs_tailnet.dylib
    RS_EXTLD='-Wl,-install_name,@rpath/librs_tailnet.dylib'
    RS_NM_ARGS=(-gU)
    ;;
  *) echo 'error: native Linux/macOS build required; unset GOOS/GOARCH or set a native Go toolchain in PATH' >&2; exit 2 ;;
esac
if [[ "$RS_GOARCH" != "$(go env GOHOSTARCH)" ]]; then
  echo 'error: smoke test requires the native architecture; unset GOARCH' >&2
  exit 2
fi
if ! mkdir -p "$RS_OUT" || [[ ! -w "$RS_OUT" ]]; then
  echo 'error: set RS_TAILNET_OUT to a writable output directory' >&2; exit 2
fi
RS_OUT="$(cd "$RS_OUT" && pwd)"
RS_STAGE="$(mktemp -d "$RS_OUT/.build-host.XXXXXX")"
trap 'rm -rf "$RS_STAGE"' EXIT
RS_DEST="$RS_OUT/host/$RS_GOOS-$RS_GOARCH"
export CGO_ENABLED=1

generate_symbols() {
  sed -nE 's/^int(32|64)_t (rs_ts_[a-z_]+)\(.*/\2/p' include/rs_tailnet.h | LC_ALL=C sort -u > "$RS_STAGE/expected"
  nm "${RS_NM_ARGS[@]}" "$RS_STAGE/$RS_LIB" | awk '{s=$NF; sub(/^_/, "", s); if (s ~ /^rs_ts_/) print s}' | LC_ALL=C sort -u > "$RS_STAGE/actual"
  if [[ "$(wc -l < "$RS_STAGE/expected" | tr -d '[:space:]')" != 10 ]] || ! cmp -s "$RS_STAGE/expected" "$RS_STAGE/actual"; then
    echo 'error: ABI symbols differ; missing:' >&2
    LC_ALL=C comm -23 "$RS_STAGE/expected" "$RS_STAGE/actual" >&2
    echo 'extra:' >&2
    LC_ALL=C comm -13 "$RS_STAGE/expected" "$RS_STAGE/actual" >&2
    return 1
  fi
  echo 'ok symbols (10)'
}
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

go build -buildvcs=false -tags "$RS_TAGS" -buildmode=c-shared -trimpath -ldflags "-s -w -extldflags '$RS_EXTLD'" -o "$RS_STAGE/$RS_LIB" ./cmd/rstailnet
generate_symbols
go test -c -tags "$RS_TAGS" -trimpath -ldflags '-s -w' -o "$RS_STAGE/rs-tailnet-testnet" ./internal/testnet
"$RS_CC" -Wall -Wextra -Werror scripts/abi_smoke.c -I include -L "$RS_STAGE" -lrs_tailnet "-Wl,-rpath,$RS_STAGE" -o "$RS_STAGE/abi-smoke"
"$RS_STAGE/abi-smoke"
install_file "$RS_STAGE/$RS_LIB" "host/$RS_GOOS-$RS_GOARCH/$RS_LIB"
install_file "$RS_STAGE/rs-tailnet-testnet" "host/$RS_GOOS-$RS_GOARCH/rs-tailnet-testnet"
cp include/rs_tailnet.h "$RS_STAGE/rs_tailnet.h"
install_file "$RS_STAGE/rs_tailnet.h" include/rs_tailnet.h
write_metadata
printf 'rs-tailnet host: %s-%s; lib=%s/%s; testnet=%s/rs-tailnet-testnet\n' "$RS_GOOS" "$RS_GOARCH" "$RS_DEST" "$RS_LIB" "$RS_DEST"
