#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
APPIMAGE_DIR="$ROOT_DIR/src-tauri/target/release/bundle/appimage"
APPDIR="$APPIMAGE_DIR/ForkTTY.AppDir"
APPIMAGE_PLUGIN="${HOME}/.cache/tauri/linuxdeploy-plugin-appimage.AppImage"
PRODUCT_NAME="$(node -p "require('$ROOT_DIR/src-tauri/tauri.conf.json').productName")"
VERSION="$(node -p "require('$ROOT_DIR/src-tauri/tauri.conf.json').version")"
EXPECTED_APPIMAGE="$APPIMAGE_DIR/${PRODUCT_NAME}_${VERSION}_amd64.AppImage"

cd "$ROOT_DIR"
tauri build "$@"

# Tauri only creates AppDir when an AppImage bundle is requested.
if [[ ! -d "$APPDIR" ]]; then
  exit 0
fi

# linuxdeploy occasionally bundles a partial lib stack. Those copies can clash
# with host libraries that remain system-provided (notably libgit2/libgpg-error),
# causing runtime warnings or symbol lookup failures when launching the AppImage.
rm -f \
  "$APPDIR/usr/lib/libpcre2-8.so.0" \
  "$APPDIR/usr/lib/libgcrypt.so.20"

if [[ ! -x "$APPIMAGE_PLUGIN" ]]; then
  echo "Expected AppImage packaging tool not found at $APPIMAGE_PLUGIN" >&2
  exit 1
fi

rm -f "$APPIMAGE_DIR"/ForkTTY_*.AppImage "$APPIMAGE_DIR"/ForkTTY-*.AppImage
(
  cd "$APPIMAGE_DIR"
  ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$APPIMAGE_PLUGIN" --appdir "$APPDIR"
)

if [[ -f "$APPIMAGE_DIR/${PRODUCT_NAME}-x86_64.AppImage" ]]; then
  mv "$APPIMAGE_DIR/${PRODUCT_NAME}-x86_64.AppImage" "$EXPECTED_APPIMAGE"
fi
