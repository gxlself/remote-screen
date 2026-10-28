#!/bin/bash
# Source: geist@1.8.1 npm package (https://github.com/vercel/geist-font).
# Fonts are licensed under SIL Open Font License 1.1 (OFL-1.1).
set -euo pipefail
APP_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
GEIST_ROOT="${GEIST_PACKAGE_ROOT:-$APP_ROOT/node_modules/geist}"
DEST="$APP_ROOT/ios/RemoteScreen/Fonts"
if [[ ! -f "$GEIST_ROOT/dist/fonts/geist-sans/Geist-Regular.ttf" ]]; then
  echo "geist package is missing; run pnpm install first" >&2
  exit 1
fi
mkdir -p "$DEST"
for WEIGHT in Regular Medium SemiBold Bold; do
  cp "$GEIST_ROOT/dist/fonts/geist-sans/Geist-$WEIGHT.ttf" "$DEST/Geist-$WEIGHT.ttf"
done
for WEIGHT in Regular Medium SemiBold; do
  cp "$GEIST_ROOT/dist/fonts/geist-mono/GeistMono-$WEIGHT.ttf" "$DEST/GeistMono-$WEIGHT.ttf"
done
cp "$GEIST_ROOT/LICENSE.txt" "$DEST/OFL-1.1.txt"
echo "Copied Geist (Regular/Medium/SemiBold/Bold) and Geist Mono (Regular/Medium/SemiBold) from $GEIST_ROOT"
