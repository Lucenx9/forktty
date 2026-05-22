#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="${FORKTTY_VERSION:-$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$ROOT_DIR/Cargo.toml" | head -1)}"
TARGET_DIR="$ROOT_DIR/target/packaging/appimage"
APPDIR="$TARGET_DIR/ForkTTY.AppDir"
DESKTOP_FILE="$ROOT_DIR/packaging/linux/forktty.desktop"
ICON_FILE="$ROOT_DIR/packaging/linux/icons/forktty.png"
APPIMAGE_DESKTOP_ID="io.github.lucenx9.forktty"
APPSTREAM_FILE="$ROOT_DIR/packaging/linux/$APPIMAGE_DESKTOP_ID.appdata.xml"
BUNDLED_RUNTIME_LIBS=(
  "libgtk-4.so"
  "libadwaita-1.so"
  "libvte-2.91-gtk4.so"
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

APPIMAGETOOL_TOOL="$(resolve_tool APPIMAGETOOL appimagetool)"

if [[ -z "$APPIMAGETOOL_TOOL" ]]; then
  cat >&2 <<'ERROR'
Unable to build the AppImage: appimagetool is not available.

Install appimagetool, or set APPIMAGETOOL=/path/to/appimagetool.
ERROR
  exit 1
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

cargo build -p forktty-ui-gtk --features gtk-vte --release

rm -rf "$APPDIR" "$APPIMAGE_PATH"
install -Dm755 "$ROOT_DIR/target/release/forktty" "$APPDIR/usr/bin/forktty"
install -Dm644 "$DESKTOP_FILE" "$APPDIR/usr/share/applications/$APPIMAGE_DESKTOP_ID.desktop"
install -Dm644 "$ICON_FILE" "$APPDIR/usr/share/icons/hicolor/128x128/apps/forktty.png"
install -Dm644 "$APPSTREAM_FILE" "$APPDIR/usr/share/metainfo/$APPIMAGE_DESKTOP_ID.appdata.xml"
copy_appimage_runtime_libs "$ROOT_DIR/target/release/forktty"

ln -s "usr/share/applications/$APPIMAGE_DESKTOP_ID.desktop" "$APPDIR/$APPIMAGE_DESKTOP_ID.desktop"
ln -s usr/share/icons/hicolor/128x128/apps/forktty.png "$APPDIR/forktty.png"

cat > "$APPDIR/AppRun" <<'APPRUN'
#!/bin/sh
set -eu

HERE="${APPDIR:-$(dirname "$(readlink -f "$0")")}"
export PATH="$HERE/usr/bin:$PATH"
export LD_LIBRARY_PATH="$HERE/usr/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
exec "$HERE/usr/bin/forktty" "$@"
APPRUN
chmod +x "$APPDIR/AppRun"

export APPIMAGE_EXTRACT_AND_RUN="${APPIMAGE_EXTRACT_AND_RUN:-1}"
export ARCH="$APPIMAGE_ARCH"
export VERSION="$VERSION"

"$APPIMAGETOOL_TOOL" "$APPDIR" "$APPIMAGE_PATH"

echo "$APPIMAGE_PATH"
