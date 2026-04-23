#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
APPIMAGE_DIR="$ROOT_DIR/src-tauri/target/release/bundle/appimage"
APPDIR="$APPIMAGE_DIR/ForkTTY.AppDir"
APPIMAGE_PLUGIN="${HOME}/.cache/tauri/linuxdeploy-plugin-appimage.AppImage"
PRODUCT_NAME="$(node -p "require('$ROOT_DIR/src-tauri/tauri.conf.json').productName")"
VERSION="$(node -p "require('$ROOT_DIR/src-tauri/tauri.conf.json').version")"
EXPECTED_APPIMAGE="$APPIMAGE_DIR/${PRODUCT_NAME}_${VERSION}_amd64.AppImage"

normalize_appimage_root() {
  local root_desktop="$APPDIR/${PRODUCT_NAME}.desktop"
  local packaged_desktop="$APPDIR/usr/share/applications/${PRODUCT_NAME}.desktop"

  if [[ ! -f "$packaged_desktop" ]]; then
    echo "Expected desktop file not found at $packaged_desktop" >&2
    exit 1
  fi

  # Tauri currently emits absolute symlinks for the AppDir root desktop entry and
  # .DirIcon. Those paths survive inside the AppImage and become broken on other
  # machines, causing the generic AppRun launcher to crash before our app starts.
  rm -f "$root_desktop"
  cp "$packaged_desktop" "$root_desktop"

  local icon_name=""
  icon_name="$(awk -F= '/^Icon=/{print $2; exit}' "$root_desktop")"

  if [[ -n "$icon_name" ]]; then
    local root_icon=""
    local candidate=""

    for ext in png svg xpm; do
      if [[ -f "$APPDIR/${icon_name}.${ext}" ]]; then
        root_icon="$APPDIR/${icon_name}.${ext}"
        break
      fi
    done

    if [[ -z "$root_icon" ]]; then
      candidate="$(
        find "$APPDIR/usr/share/icons" -type f \
          \( -name "${icon_name}.png" -o -name "${icon_name}.svg" -o -name "${icon_name}.xpm" \) \
          | sort | head -n 1
      )"
      if [[ -z "$candidate" && -f "$APPDIR/${PRODUCT_NAME}.png" ]]; then
        candidate="$APPDIR/${PRODUCT_NAME}.png"
      fi

      if [[ -n "$candidate" ]]; then
        local ext="${candidate##*.}"
        cp "$candidate" "$APPDIR/${icon_name}.${ext}"
        root_icon="$APPDIR/${icon_name}.${ext}"
      fi
    fi

    if [[ -n "$root_icon" ]]; then
      rm -f "$APPDIR/.DirIcon"
      ln -s "$(basename "$root_icon")" "$APPDIR/.DirIcon"
    fi
  fi

  local absolute_root_links=""
  absolute_root_links="$(find "$APPDIR" -maxdepth 1 -type l -lname '/*' -printf '%P -> %l\n')"
  if [[ -n "$absolute_root_links" ]]; then
    echo "Refusing to package AppDir with absolute root symlinks:" >&2
    echo "$absolute_root_links" >&2
    exit 1
  fi
}

patch_appimage_runtime_env() {
  local hook="$APPDIR/apprun-hooks/linuxdeploy-plugin-gtk.sh"

  if [[ ! -f "$hook" ]]; then
    echo "Expected AppImage GTK hook not found at $hook" >&2
    exit 1
  fi

  sed -i 's/^export GDK_BACKEND=x11\b/export GDK_BACKEND="${GDK_BACKEND:-x11}"/' "$hook"

  if ! grep -q '^export GDK_BACKEND=' "$hook"; then
    echo "Expected GDK_BACKEND setting not found in $hook" >&2
    exit 1
  fi

  sed -i \
    -e '/^export WEBKIT_DISABLE_DMABUF_RENDERER=/d' \
    -e '/^export WEBKIT_DISABLE_COMPOSITING_MODE=/d' \
    -e '/^export LIBGL_ALWAYS_SOFTWARE=/d' \
    "$hook"

  sed -i '/^export GDK_BACKEND=/a\
export LIBGL_ALWAYS_SOFTWARE="${LIBGL_ALWAYS_SOFTWARE:-1}" # Avoid host GPU/EGL crashes in WebKitGTK AppImage runs.\
export WEBKIT_DISABLE_DMABUF_RENDERER="${WEBKIT_DISABLE_DMABUF_RENDERER:-1}" # Avoid WebKitGTK GBM/DMABUF aborts on Fedora/NVIDIA AppImage runs.\
export WEBKIT_DISABLE_COMPOSITING_MODE="${WEBKIT_DISABLE_COMPOSITING_MODE:-1}" # Fall back when WebKitGTK accelerated compositing aborts.' "$hook"
}

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

normalize_appimage_root
patch_appimage_runtime_env

rm -f "$APPIMAGE_DIR"/ForkTTY_*.AppImage "$APPIMAGE_DIR"/ForkTTY-*.AppImage
(
  cd "$APPIMAGE_DIR"
  ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$APPIMAGE_PLUGIN" --appdir "$APPDIR"
)

if [[ -f "$APPIMAGE_DIR/${PRODUCT_NAME}-x86_64.AppImage" ]]; then
  mv "$APPIMAGE_DIR/${PRODUCT_NAME}-x86_64.AppImage" "$EXPECTED_APPIMAGE"
fi
