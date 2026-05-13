#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="$(node -p "require('$ROOT_DIR/package.json').version")"
TARGET_DIR="$ROOT_DIR/target/packaging/deb"
ARCH="$(dpkg --print-architecture 2>/dev/null || true)"

if [[ -z "$ARCH" ]]; then
  case "$(uname -m)" in
    x86_64) ARCH="amd64" ;;
    aarch64 | arm64) ARCH="arm64" ;;
    *) ARCH="$(uname -m)" ;;
  esac
fi

PKG_NAME="forktty-gtk"
PKG_ROOT="$TARGET_DIR/${PKG_NAME}_${VERSION}_${ARCH}"
DEB_PATH="$TARGET_DIR/${PKG_NAME}_${VERSION}_${ARCH}.deb"

command -v dpkg-deb >/dev/null || {
  echo "dpkg-deb is required to build a .deb package" >&2
  exit 1
}

cargo build -p forktty-ui-gtk --features gtk-vte --release

rm -rf "$PKG_ROOT"
install -Dm755 "$ROOT_DIR/target/release/forktty-ui-gtk" "$PKG_ROOT/usr/bin/forktty-gtk"
install -Dm644 "$ROOT_DIR/packaging/linux/forktty-gtk.desktop" \
  "$PKG_ROOT/usr/share/applications/forktty-gtk.desktop"
install -Dm644 "$ROOT_DIR/src-tauri/icons/128x128.png" \
  "$PKG_ROOT/usr/share/icons/hicolor/128x128/apps/forktty.png"

mkdir -p "$PKG_ROOT/DEBIAN"
cat > "$PKG_ROOT/DEBIAN/control" <<CONTROL
Package: $PKG_NAME
Version: $VERSION
Section: utils
Priority: optional
Architecture: $ARCH
Maintainer: Lucenx9
Homepage: https://github.com/Lucenx9/forktty
Depends: libc6, libgtk-4-1, libadwaita-1-0, libvte-2.91-gtk4-0
Description: Linux-native ForkTTY GTK/VTE preview
 ForkTTY GTK/VTE preview for multi-agent terminal workflows.
 This package intentionally installs as forktty-gtk while the Tauri path remains available.
CONTROL

dpkg-deb --build "$PKG_ROOT" "$DEB_PATH"
echo "$DEB_PATH"

