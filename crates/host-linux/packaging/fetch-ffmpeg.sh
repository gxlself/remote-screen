#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
HOST_LINUX_DIR=$(cd -- "$SCRIPT_DIR/.." && pwd)
LOCK_FILE=${RS_FFMPEG_LOCK:-$SCRIPT_DIR/ffmpeg.lock}
FFMPEG_DIR=$HOST_LINUX_DIR/third_party/ffmpeg
url=$(sed -n 's/^url=//p' "$LOCK_FILE" | head -n 1)
sha256=$(sed -n 's/^sha256=//p' "$LOCK_FILE" | head -n 1)
if [[ -z "$url" || -z "$sha256" ]]; then
  echo "FFmpeg 构建未提供" >&2
  exit 3
fi
mkdir -p "$FFMPEG_DIR"
download="$FFMPEG_DIR/download.tmp"
trap 'rm -f "$download"' EXIT
curl --fail --location --output "$download" "$url"
actual=$(sha256sum "$download" | awk '{print $1}')
if [[ "$actual" != "$sha256" ]]; then
  echo "FFmpeg 校验失败" >&2
  rm -f "$download"
  exit 1
fi
extract_dir="$FFMPEG_DIR/.extract"
rm -rf "$extract_dir"
mkdir -p "$extract_dir"
tar -xf "$download" -C "$extract_dir" 2>/dev/null || {
  command -v unzip >/dev/null 2>&1 || { echo "无法解压 FFmpeg 归档" >&2; exit 1; }
  unzip -q "$download" -d "$extract_dir"
}
for library in libavutil.so.60 libswscale.so.9 libavcodec.so.62; do
  source=$(find "$extract_dir" -type f -o -type l | grep "/$library$" | head -n 1 || true)
  [[ -n "$source" ]] || { echo "FFmpeg 缺少 $library" >&2; exit 1; }
  cp -a "$source" "$FFMPEG_DIR/$library"
done
license=$(find "$extract_dir" -type f -name COPYING.LGPLv2.1 | head -n 1 || true)
[[ -n "$license" ]] || { echo "FFmpeg 缺少 COPYING.LGPLv2.1" >&2; exit 1; }
cp -a "$license" "$FFMPEG_DIR/COPYING.LGPLv2.1"
rm -rf "$extract_dir"
echo "FFmpeg 已准备到 $FFMPEG_DIR"
