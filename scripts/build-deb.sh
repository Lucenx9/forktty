#!/usr/bin/env bash

set -euo pipefail

umask 022

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="${FORKTTY_VERSION:-$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$ROOT_DIR/Cargo.toml" | head -1)}"
DEB_VERSION="${FORKTTY_DEB_VERSION:-$VERSION}"
TARGET_DIR="$ROOT_DIR/target/packaging/deb"
ARCH="$(dpkg --print-architecture 2>/dev/null || true)"
DESKTOP_ID="dev.forktty.forktty"
DESKTOP_FILE="$ROOT_DIR/packaging/linux/$DESKTOP_ID.desktop"
APPSTREAM_FILE="$ROOT_DIR/packaging/linux/$DESKTOP_ID.metainfo.xml"

referenced_forktty_symbolic_icons() {
  grep -Roh '"forktty-[A-Za-z0-9_-]*-symbolic"' "$ROOT_DIR/crates/forktty-ui-gtk/src" |
    tr -d '"' |
    sort -u
}

verify_forktty_icon_assets() {
  local root="$1"
  local missing=0
  local icon
  local icon_path

  if [[ ! -f "$root/usr/share/icons/hicolor/128x128/apps/forktty.png" ]]; then
    echo "Missing packaged app icon: forktty.png" >&2
    missing=1
  fi

  while IFS= read -r icon; do
    [[ -n "$icon" ]] || continue
    icon_path="$root/usr/share/icons/hicolor/scalable/actions/$icon.svg"
    if [[ ! -f "$icon_path" ]]; then
      echo "Missing packaged symbolic icon: $icon_path" >&2
      missing=1
    fi
  done < <(referenced_forktty_symbolic_icons)

  if [[ "$missing" -ne 0 ]]; then
    exit 1
  fi
}

copy_vendored_ghostty_shell_integration() {
  local source_dir
  source_dir="$(find "$ROOT_DIR/target/release/build" -path '*/ghostty-src/src/shell-integration' -print -quit)"
  if [[ -z "$source_dir" ]]; then
    echo "Could not find vendored Ghostty shell-integration resources in target/release/build" >&2
    exit 1
  fi

  mkdir -p "$PKG_ROOT/usr/share/ghostty"
  cp -a "$source_dir" "$PKG_ROOT/usr/share/ghostty/shell-integration"
}

ghostty_iterm2_themes_field() {
  local field="$1"
  sed -n "/\\.iterm2_themes = /,/},/s/.*\\.${field} = \"\\([^\"]*\\)\".*/\\1/p" \
    "$ROOT_DIR/vendor/ghostty/build.zig.zon" |
    head -1
}

copy_vendored_ghostty_themes() {
  command -v curl >/dev/null || {
    echo "curl is required to package Ghostty themes" >&2
    exit 1
  }
  command -v tar >/dev/null || {
    echo "tar is required to package Ghostty themes" >&2
    exit 1
  }
  command -v zig >/dev/null || {
    echo "zig is required to verify Ghostty themes" >&2
    exit 1
  }

  local url hash actual tmp_dir
  url="$(ghostty_iterm2_themes_field "url")"
  hash="$(ghostty_iterm2_themes_field "hash")"
  if [[ -z "$url" || -z "$hash" ]]; then
    echo "Could not read pinned Ghostty theme URL/hash from vendor/ghostty/build.zig.zon" >&2
    exit 1
  fi

  tmp_dir="$(mktemp -d)"
  curl -fsSL "$url" -o "$tmp_dir/themes.tgz"
  actual="$(zig fetch "$tmp_dir/themes.tgz")"
  if [[ "$actual" != "$hash" ]]; then
    echo "Ghostty theme archive hash mismatch: expected $hash, got $actual" >&2
    rm -rf "$tmp_dir"
    exit 1
  fi

  mkdir -p "$PKG_ROOT/usr/share/ghostty"
  rm -rf "$PKG_ROOT/usr/share/ghostty/themes"
  mkdir -p "$PKG_ROOT/usr/share/ghostty/themes"
  tar -xzf "$tmp_dir/themes.tgz" -C "$PKG_ROOT/usr/share/ghostty/themes" --strip-components=1
  rm -rf "$tmp_dir"
  test -f "$PKG_ROOT/usr/share/ghostty/themes/Catppuccin Mocha"
}

copy_vendored_ghostty_terminfo() {
  command -v tic >/dev/null || {
    echo "tic is required to package Ghostty terminfo" >&2
    exit 1
  }
  command -v zig >/dev/null || {
    echo "zig is required to generate Ghostty terminfo" >&2
    exit 1
  }

  local source_dir tmp_dir
  source_dir="$(find "$ROOT_DIR/target/release/build" -path '*/ghostty-src/src/terminfo' -type d -print -quit)"
  if [[ -z "$source_dir" ]]; then
    echo "Could not find vendored Ghostty terminfo sources in target/release/build" >&2
    exit 1
  fi

  tmp_dir="$(mktemp -d)"
  ln -s "$source_dir/main.zig" "$tmp_dir/main.zig"
  ln -s "$source_dir/Source.zig" "$tmp_dir/Source.zig"
  ln -s "$source_dir/ghostty.zig" "$tmp_dir/ghostty.zig"
  cat > "$tmp_dir/gen.zig" <<'ZIG'
const std = @import("std");
const terminfo = @import("main.zig");
pub fn main() !void {
    var stdout_buffer: [4096]u8 = undefined;
    var stdout_writer = std.fs.File.stdout().writer(&stdout_buffer);
    const stdout = &stdout_writer.interface;
    try terminfo.ghostty.encode(stdout);
    try stdout.flush();
}
ZIG

  zig run "$tmp_dir/gen.zig" > "$tmp_dir/ghostty.terminfo"
  tic -x -o "$PKG_ROOT/usr/share/terminfo" "$tmp_dir/ghostty.terminfo"
  rm -rf "$tmp_dir"
}

find_required_ghostty_gtk_lib() {
  "$ROOT_DIR/scripts/ghostty-gtk-lib-probe.sh" --ensure --print-path
}

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

cargo build -p forktty-ui-gtk --no-default-features --features gtk-ghostty --release

rm -rf "$PKG_ROOT"
install -Dm755 "$ROOT_DIR/target/release/forktty" "$PKG_ROOT/usr/bin/forktty"
GHOSTTY_LIB="$(find "$ROOT_DIR/target/release/build" -path '*/ghostty-install/lib/libghostty-vt.so.0.1.0' -print -quit)"
if [[ -z "$GHOSTTY_LIB" ]]; then
  echo "Could not find vendored libghostty-vt.so.0.1.0 in target/release/build" >&2
  exit 1
fi
install -Dm755 "$GHOSTTY_LIB" "$PKG_ROOT/usr/lib/libghostty-vt.so.0.1.0"
ln -s libghostty-vt.so.0.1.0 "$PKG_ROOT/usr/lib/libghostty-vt.so.0"
ln -s libghostty-vt.so.0 "$PKG_ROOT/usr/lib/libghostty-vt.so"
# Embedded Ghostty widget library. Required for release packages because
# terminal panes require the embedded renderer at runtime.
GHOSTTY_GTK_LIB="$(find_required_ghostty_gtk_lib)"
install -Dm755 "$GHOSTTY_GTK_LIB" "$PKG_ROOT/usr/lib/ghostty-gtk-embed.so"
test -f "$PKG_ROOT/usr/lib/ghostty-gtk-embed.so"
copy_vendored_ghostty_shell_integration
copy_vendored_ghostty_themes
copy_vendored_ghostty_terminfo
install -Dm644 "$DESKTOP_FILE" "$PKG_ROOT/usr/share/applications/$DESKTOP_ID.desktop"
install -Dm644 "$ROOT_DIR/packaging/linux/icons/forktty.png" \
  "$PKG_ROOT/usr/share/icons/hicolor/128x128/apps/forktty.png"
if [[ -d "$ROOT_DIR/packaging/linux/icons/hicolor" ]]; then
  mkdir -p "$PKG_ROOT/usr/share/icons/hicolor"
  cp -a "$ROOT_DIR/packaging/linux/icons/hicolor/." "$PKG_ROOT/usr/share/icons/hicolor/"
fi
install -Dm644 "$APPSTREAM_FILE" "$PKG_ROOT/usr/share/metainfo/$DESKTOP_ID.metainfo.xml"
verify_forktty_icon_assets "$PKG_ROOT"
"$ROOT_DIR/scripts/write-package-legal-docs.sh" "$PKG_ROOT"

mkdir -p "$PKG_ROOT/DEBIAN"
chmod 755 "$PKG_ROOT/DEBIAN"
INSTALLED_SIZE="$(du -sk "$PKG_ROOT/usr" | awk '{print $1}')"
cat > "$PKG_ROOT/DEBIAN/control" <<CONTROL
Package: $PKG_NAME
Version: $DEB_VERSION
Section: utils
Priority: optional
Architecture: $ARCH
Installed-Size: $INSTALLED_SIZE
Maintainer: Lucenx9
Homepage: https://forktty.dev/
Depends: libc6, libgcc-s1, libstdc++6, libgtk-4-1, libadwaita-1-0 (>= 1.4), libgtk4-layer-shell0, libssl3, libssh2-1, zlib1g, libzstd1, hicolor-icon-theme
Description: Linux-native multi-agent terminal
 ForkTTY is a Linux-native GTK4/libadwaita/Ghostty terminal for multi-agent
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
