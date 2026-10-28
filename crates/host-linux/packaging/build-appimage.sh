#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
REPO_ROOT=$(cd -- "$SCRIPT_DIR/../../.." && pwd)
OUT_DIR="$SCRIPT_DIR/out"
APP_DIR="$OUT_DIR/AppDir"
version=
while [[ $# -gt 0 ]]; do
  case "$1" in
    --version)
      [[ $# -ge 2 ]] || { echo "--version requires a value" >&2; exit 2; }
      version=$2; shift 2;;
    *) echo "usage: $0 [--version x.y.z]" >&2; exit 2;;
  esac
done
if [[ -z "$version" ]]; then
  metadata=$(cargo metadata --no-deps --format-version 1)
  version=$(printf '%s' "$metadata" | python3 -c 'import json,sys; d=json.load(sys.stdin); print(next(p["version"] for p in d["packages"] if p["name"] == "rs-host-agent"))')
fi
appimagetool=${APPIMAGETOOL:-}
if [[ -z "$appimagetool" || ! -x "$appimagetool" ]]; then
  echo "APPIMAGETOOL must point to an executable" >&2
  exit 3
fi
cd "$REPO_ROOT"
# The caller should prefix these build commands with heavy.
cargo build --release -p rs-host-agent --bin rs-host
cargo build --release -p host-linux --bin remote-host-ctl
rm -rf "$APP_DIR"
mkdir -p "$APP_DIR/usr/bin" "$APP_DIR/usr/share/remote-host"
cp "$SCRIPT_DIR/AppRun" "$APP_DIR/AppRun"; chmod 0755 "$APP_DIR/AppRun"
cp "$SCRIPT_DIR/remote-host.desktop" "$APP_DIR/remote-host.desktop"
cp "$SCRIPT_DIR/remote-host.svg" "$APP_DIR/remote-host.svg"; cp "$SCRIPT_DIR/remote-host.svg" "$APP_DIR/.DirIcon"
cp target/release/rs-host "$APP_DIR/usr/bin/rs-host"; cp target/release/remote-host-ctl "$APP_DIR/usr/bin/remote-host-ctl"
chmod 0755 "$APP_DIR/usr/bin/rs-host" "$APP_DIR/usr/bin/remote-host-ctl"
printf '%s\n' "$version" > "$APP_DIR/usr/share/remote-host/VERSION"
ffmpeg_dir="$SCRIPT_DIR/../third_party/ffmpeg"
if [[ -d "$ffmpeg_dir" ]]; then
  mkdir -p "$APP_DIR/usr/lib/ffmpeg" "$APP_DIR/usr/share/licenses/ffmpeg"
  for library in libavutil.so.60 libswscale.so.9 libavcodec.so.62; do
    [[ -e "$ffmpeg_dir/$library" ]] || { echo "FFmpeg 缺少 $library" >&2; exit 1; }
    cp -a "$ffmpeg_dir/$library" "$APP_DIR/usr/lib/ffmpeg/"
    for soname in "$ffmpeg_dir/${library}."*; do [[ -e "$soname" || -L "$soname" ]] && cp -a "$soname" "$APP_DIR/usr/lib/ffmpeg/"; done
  done
  [[ -f "$ffmpeg_dir/COPYING.LGPLv2.1" ]] || { echo "FFmpeg 缺少 COPYING.LGPLv2.1" >&2; exit 1; }
  cp "$ffmpeg_dir/COPYING.LGPLv2.1" "$APP_DIR/usr/share/licenses/ffmpeg/"
else
  echo "不带 FFmpeg，只能用 openh264"
fi
arch=$(uname -m)
artifact="$OUT_DIR/RemoteHost-${version}-${arch}.AppImage"
"$appimagetool" "$APP_DIR" "$artifact"
printf 'AppImage: %s\n' "$artifact"
sha256sum "$artifact"
