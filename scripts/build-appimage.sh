#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="${FORKTTY_VERSION:-$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$ROOT_DIR/Cargo.toml" | head -1)}"
TARGET_DIR="$ROOT_DIR/target/packaging/appimage"
APPDIR="$TARGET_DIR/ForkTTY.AppDir"
APPIMAGE_DESKTOP_ID="dev.forktty.forktty"
DESKTOP_FILE="$ROOT_DIR/packaging/linux/$APPIMAGE_DESKTOP_ID.desktop"
ICON_FILE="$ROOT_DIR/packaging/linux/icons/forktty.png"
APPSTREAM_FILE="$ROOT_DIR/packaging/linux/$APPIMAGE_DESKTOP_ID.metainfo.xml"
BUNDLED_RUNTIME_LIBS=(
  "libgtk-4.so"
  "libadwaita-1.so"
  "libvte-2.91-gtk4.so"
  "libwebkitgtk-6.0.so"
  "libjavascriptcoregtk-6.0.so"
  "libsoup-3.0.so"
)

if [[ -z "$VERSION" ]]; then
  echo "Could not determine ForkTTY version from Cargo.toml" >&2
  exit 1
fi

case "$(uname -m)" in
  x86_64) APPIMAGE_ARCH="x86_64" ;;
  aarch64 | arm64) APPIMAGE_ARCH="aarch64" ;;
  *)
    echo "Unsupported AppImage architecture: $(uname -m)" >&2
    exit 1
    ;;
esac

APPIMAGE_PATH="$TARGET_DIR/forktty-${VERSION}-${APPIMAGE_ARCH}.AppImage"

resolve_tool() {
  local env_name="$1"
  local command_name="$2"
  local configured="${!env_name:-}"

  if [[ -n "$configured" ]]; then
    if [[ ! -x "$configured" ]]; then
      echo "$env_name is set to '$configured', but that file is not executable" >&2
      exit 1
    fi
    printf '%s\n' "$configured"
    return
  fi

  command -v "$command_name" 2>/dev/null || true
}

should_skip_appimage_lib() {
  local name="$1"

  case "$name" in
    ld-linux*.so* | libc.so.* | libm.so.* | libdl.so.* | libpthread.so.* | librt.so.* | \
      libresolv.so.* | libanl.so.* | libnsl.so.* | libutil.so.* | libnss_*.so.*)
      return 0
      ;;
    libEGL.so.* | libGL.so.* | libGLES*.so.* | libGLX.so.* | libOpenGL.so.* | \
      libGLdispatch.so.* | libglapi.so.* | libgbm.so.* | libdrm.so.* | \
      libdrm_*.so.* | libvulkan.so.* | libvulkan_*.so.*)
      # Keep the GPU/display-driver stack on the host side. Bundling Mesa/GL
      # dispatch libraries without the matching host driver modules is a common
      # source of broken GTK4 rendering in AppImages.
      return 0
      ;;
    *)
      return 1
      ;;
  esac
}

copy_appimage_runtime_libs() {
  local binary="$1"
  local lib_dir="$APPDIR/usr/lib"
  local copied=0
  local lib_path
  local lib_name

  mkdir -p "$lib_dir"
  while IFS= read -r lib_path; do
    [[ -n "$lib_path" && -f "$lib_path" ]] || continue
    lib_name="$(basename "$lib_path")"
    if should_skip_appimage_lib "$lib_name"; then
      continue
    fi
    if [[ -e "$lib_dir/$lib_name" ]]; then
      continue
    fi
    install -Dm644 "$lib_path" "$lib_dir/$lib_name"
    copied=$((copied + 1))
  done < <(
    ldd "$binary" |
      awk '
        $2 == "=>" && $3 ~ /^\// { print $3 }
        $1 ~ /^\// { print $1 }
      '
  )

  for required in "${BUNDLED_RUNTIME_LIBS[@]}"; do
    if ! compgen -G "$lib_dir/$required*" >/dev/null; then
      echo "Failed to bundle required AppImage runtime library: $required" >&2
      exit 1
    fi
  done
  echo "Bundled $copied shared libraries into $lib_dir" >&2
}

copy_forktty_icon_assets() {
  local source_dir="$ROOT_DIR/packaging/linux/icons/hicolor"
  local target_dir="$APPDIR/usr/share/icons/hicolor"

  if [[ -d "$source_dir" ]]; then
    mkdir -p "$target_dir"
    cp -a "$source_dir/." "$target_dir/"
  fi
}

write_appimage_hicolor_index_theme() {
  local index_file="$APPDIR/usr/share/icons/hicolor/index.theme"

  mkdir -p "$(dirname "$index_file")"
  cat > "$index_file" <<'EOF'
[Icon Theme]
Name=Hicolor
Comment=Fallback icon theme
Directories=16x16/apps,24x24/apps,32x32/apps,48x48/apps,64x64/apps,128x128/apps,256x256/apps,scalable/apps,scalable/actions

[16x16/apps]
Size=16
Context=Applications
Type=Fixed

[24x24/apps]
Size=24
Context=Applications
Type=Fixed

[32x32/apps]
Size=32
Context=Applications
Type=Fixed

[48x48/apps]
Size=48
Context=Applications
Type=Fixed

[64x64/apps]
Size=64
Context=Applications
Type=Fixed

[128x128/apps]
Size=128
Context=Applications
Type=Fixed

[256x256/apps]
Size=256
Context=Applications
Type=Fixed

[scalable/apps]
Size=128
Context=Applications
Type=Scalable
MinSize=16
MaxSize=512

[scalable/actions]
Size=16
Context=Actions
Type=Scalable
MinSize=8
MaxSize=64
EOF
}

APPIMAGETOOL_TOOL="$(resolve_tool APPIMAGETOOL appimagetool)"

if [[ -z "$APPIMAGETOOL_TOOL" ]]; then
  cat >&2 <<'ERROR'
Unable to build the AppImage: appimagetool is not available.

Install appimagetool, or set APPIMAGETOOL=/path/to/appimagetool.
ERROR
  exit 1
fi

APPIMAGETOOL_ARGS=("$APPDIR" "$APPIMAGE_PATH")
if [[ -n "${APPIMAGE_RUNTIME_FILE:-}" ]]; then
  if [[ ! -f "$APPIMAGE_RUNTIME_FILE" ]]; then
    echo "APPIMAGE_RUNTIME_FILE is set to '$APPIMAGE_RUNTIME_FILE', but that file does not exist" >&2
    exit 1
  fi
  APPIMAGETOOL_ARGS=(--runtime-file "$APPIMAGE_RUNTIME_FILE" "${APPIMAGETOOL_ARGS[@]}")
fi

if command -v desktop-file-validate >/dev/null; then
  desktop-file-validate "$DESKTOP_FILE"
else
  echo "desktop-file-validate not found; skipping desktop entry validation" >&2
fi

if command -v appstreamcli >/dev/null; then
  appstreamcli validate --no-net "$APPSTREAM_FILE"
else
  echo "appstreamcli not found; skipping AppStream metadata validation" >&2
fi

cargo build -p forktty-ui-gtk --features browser --release

rm -rf "$APPDIR" "$APPIMAGE_PATH"
install -Dm755 "$ROOT_DIR/target/release/forktty" "$APPDIR/usr/bin/forktty"
install -Dm644 "$DESKTOP_FILE" "$APPDIR/usr/share/applications/$APPIMAGE_DESKTOP_ID.desktop"
install -Dm644 "$ICON_FILE" "$APPDIR/usr/share/icons/hicolor/128x128/apps/forktty.png"
install -Dm644 "$APPSTREAM_FILE" "$APPDIR/usr/share/metainfo/$APPIMAGE_DESKTOP_ID.appdata.xml"
copy_forktty_icon_assets
write_appimage_hicolor_index_theme
copy_appimage_runtime_libs "$ROOT_DIR/target/release/forktty"

ln -s "usr/share/applications/$APPIMAGE_DESKTOP_ID.desktop" "$APPDIR/$APPIMAGE_DESKTOP_ID.desktop"
ln -s usr/share/icons/hicolor/128x128/apps/forktty.png "$APPDIR/forktty.png"

cat > "$APPDIR/AppRun" <<'APPRUN'
#!/bin/sh
set -eu

HERE="${APPDIR:-$(dirname "$(readlink -f "$0")")}"
export PATH="$HERE/usr/bin:$PATH"
export LD_LIBRARY_PATH="$HERE/usr/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
if [ -n "${XDG_DATA_DIRS:-}" ]; then
  export XDG_DATA_DIRS="$HERE/usr/share:$XDG_DATA_DIRS"
else
  export XDG_DATA_DIRS="$HERE/usr/share:/usr/local/share:/usr/share"
fi
exec "$HERE/usr/bin/forktty" "$@"
APPRUN
chmod +x "$APPDIR/AppRun"

export APPIMAGE_EXTRACT_AND_RUN="${APPIMAGE_EXTRACT_AND_RUN:-1}"
export ARCH="$APPIMAGE_ARCH"
export VERSION="$VERSION"

"$APPIMAGETOOL_TOOL" "${APPIMAGETOOL_ARGS[@]}"
sha256sum "$APPIMAGE_PATH" > "$APPIMAGE_PATH.sha256"

echo "$APPIMAGE_PATH"
