#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="${FORKTTY_VERSION:-$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$ROOT_DIR/Cargo.toml" | head -1)}"
TARGET_DIR="$ROOT_DIR/target/packaging/deb"
ARCH="$(dpkg --print-architecture 2>/dev/null || true)"

if [[ -z "$VERSION" ]]; then
  echo "Could not determine ForkTTY version from Cargo.toml" >&2
  exit 1
fi

if [[ -z "$ARCH" ]]; then
  case "$(uname -m)" in
    x86_64) ARCH="amd64" ;;
    aarch64 | arm64) ARCH="arm64" ;;
    *) ARCH="$(uname -m)" ;;
  esac
fi

PKG_NAME="forktty"
PKG_ROOT="$TARGET_DIR/${PKG_NAME}_${VERSION}_${ARCH}"
DEB_PATH="$TARGET_DIR/${PKG_NAME}_${VERSION}_${ARCH}.deb"

command -v dpkg-deb >/dev/null || {
  echo "dpkg-deb is required to build a .deb package" >&2
  exit 1
}

if command -v desktop-file-validate >/dev/null; then
  desktop-file-validate "$ROOT_DIR/packaging/linux/forktty.desktop"
fi

cargo build -p forktty-ui-gtk --features gtk-vte --release

rm -rf "$PKG_ROOT"
install -Dm755 "$ROOT_DIR/target/release/forktty" "$PKG_ROOT/usr/bin/forktty"
install -Dm644 "$ROOT_DIR/packaging/linux/forktty.desktop" \
  "$PKG_ROOT/usr/share/applications/forktty.desktop"
install -Dm644 "$ROOT_DIR/packaging/linux/icons/forktty.png" \
  "$PKG_ROOT/usr/share/icons/hicolor/128x128/apps/forktty.png"

mkdir -p "$PKG_ROOT/DEBIAN"
INSTALLED_SIZE="$(du -sk "$PKG_ROOT/usr" | awk '{print $1}')"
cat > "$PKG_ROOT/DEBIAN/control" <<CONTROL
Package: $PKG_NAME
Version: $VERSION
Section: utils
Priority: optional
Architecture: $ARCH
Installed-Size: $INSTALLED_SIZE
Maintainer: Lucenx9
Homepage: https://github.com/Lucenx9/forktty
Depends: libc6, libgtk-4-1, libadwaita-1-0, libvte-2.91-gtk4-0 (>= 0.76)
Description: Linux-native multi-agent terminal
 ForkTTY is a Linux-native GTK4/libadwaita/VTE terminal for multi-agent
 workflows, programmable socket automation, and git worktree isolation.
CONTROL

dpkg-deb --build --root-owner-group "$PKG_ROOT" "$DEB_PATH"
echo "$DEB_PATH"
