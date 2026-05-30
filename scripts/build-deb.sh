#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="${FORKTTY_VERSION:-$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$ROOT_DIR/Cargo.toml" | head -1)}"
DEB_VERSION="${FORKTTY_DEB_VERSION:-$VERSION}"
TARGET_DIR="$ROOT_DIR/target/packaging/deb"
ARCH="$(dpkg --print-architecture 2>/dev/null || true)"
DESKTOP_ID="dev.forktty.forktty"
DESKTOP_FILE="$ROOT_DIR/packaging/linux/$DESKTOP_ID.desktop"
APPSTREAM_FILE="$ROOT_DIR/packaging/linux/$DESKTOP_ID.metainfo.xml"

if [[ -z "$VERSION" ]]; then
  echo "Could not determine ForkTTY version from Cargo.toml" >&2
  exit 1
fi

if [[ -z "${FORKTTY_DEB_VERSION:-}" && "$VERSION" == *-* ]]; then
  DEB_VERSION="${VERSION%%-*}~${VERSION#*-}"
fi

if [[ -z "$ARCH" ]]; then
  case "$(uname -m)" in
    x86_64) ARCH="amd64" ;;
    aarch64 | arm64) ARCH="arm64" ;;
    *) ARCH="$(uname -m)" ;;
  esac
fi

PKG_NAME="forktty"
PKG_ROOT="$TARGET_DIR/${PKG_NAME}_${DEB_VERSION}_${ARCH}"
DEB_PATH="$TARGET_DIR/${PKG_NAME}_${DEB_VERSION}_${ARCH}.deb"

command -v dpkg-deb >/dev/null || {
  echo "dpkg-deb is required to build a .deb package" >&2
  exit 1
}

if command -v desktop-file-validate >/dev/null; then
  desktop-file-validate "$DESKTOP_FILE"
fi

if command -v appstreamcli >/dev/null; then
  appstreamcli validate --no-net "$APPSTREAM_FILE"
else
  echo "appstreamcli not found; skipping AppStream metadata validation" >&2
fi

cargo build -p forktty-ui-gtk --features browser --release

rm -rf "$PKG_ROOT"
install -Dm755 "$ROOT_DIR/target/release/forktty" "$PKG_ROOT/usr/bin/forktty"
install -Dm644 "$DESKTOP_FILE" "$PKG_ROOT/usr/share/applications/$DESKTOP_ID.desktop"
install -Dm644 "$ROOT_DIR/packaging/linux/icons/forktty.png" \
  "$PKG_ROOT/usr/share/icons/hicolor/128x128/apps/forktty.png"
if [[ -d "$ROOT_DIR/packaging/linux/icons/hicolor" ]]; then
  mkdir -p "$PKG_ROOT/usr/share/icons/hicolor"
  cp -a "$ROOT_DIR/packaging/linux/icons/hicolor/." "$PKG_ROOT/usr/share/icons/hicolor/"
fi
install -Dm644 "$APPSTREAM_FILE" "$PKG_ROOT/usr/share/metainfo/$DESKTOP_ID.metainfo.xml"

mkdir -p "$PKG_ROOT/DEBIAN"
INSTALLED_SIZE="$(du -sk "$PKG_ROOT/usr" | awk '{print $1}')"
cat > "$PKG_ROOT/DEBIAN/control" <<CONTROL
Package: $PKG_NAME
Version: $DEB_VERSION
Section: utils
Priority: optional
Architecture: $ARCH
Installed-Size: $INSTALLED_SIZE
Maintainer: Lucenx9
Homepage: https://github.com/Lucenx9/forktty
Depends: libc6, libgcc-s1, libstdc++6, libgtk-4-1, libadwaita-1-0 (>= 1.4), libvte-2.91-gtk4-0 (>= 0.76), libwebkitgtk-6.0-4, libssl3, libssh2-1, zlib1g, libzstd1, hicolor-icon-theme
Description: Linux-native multi-agent terminal
 ForkTTY is a Linux-native GTK4/libadwaita/VTE terminal for multi-agent
 workflows, programmable socket automation, and git worktree isolation.
CONTROL

cat > "$PKG_ROOT/DEBIAN/postinst" <<'SCRIPT'
#!/bin/sh
set -e

if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor || true
fi

exit 0
SCRIPT

cat > "$PKG_ROOT/DEBIAN/postrm" <<'SCRIPT'
#!/bin/sh
set -e

if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor || true
fi

exit 0
SCRIPT

chmod 755 "$PKG_ROOT/DEBIAN/postinst" "$PKG_ROOT/DEBIAN/postrm"

dpkg-deb --build --root-owner-group "$PKG_ROOT" "$DEB_PATH"
dpkg-deb --info "$DEB_PATH" >/dev/null
dpkg-deb --contents "$DEB_PATH" >/dev/null
echo "$DEB_PATH"
