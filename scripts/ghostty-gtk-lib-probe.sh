#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GHOSTTY_DIR="$ROOT_DIR/vendor/ghostty"
ENSURE=0
PRINT_PATH=0
BUILT_BEFORE_VERIFY=0

usage() {
  cat <<'USAGE'
Usage: scripts/ghostty-gtk-lib-probe.sh [--ensure] [--print-path]

Builds or verifies the vendored Ghostty GTK embedding library.

Options:
  --ensure      Reuse an existing verified library, building it if missing.
  --print-path  Print the verified ghostty-gtk-embed.so path on stdout.
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --ensure)
      ENSURE=1
      ;;
    --print-path)
      PRINT_PATH=1
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
  shift
done

ghostty_gtk_lib_candidates() {
  printf '%s\n' \
    "$GHOSTTY_DIR/zig-out/lib/ghostty-gtk-embed.so" \
    "$GHOSTTY_DIR/zig-out/lib/libghostty-gtk-embed.so"
}

absolute_path() {
  local path="$1"
  local dir
  dir="$(cd "$(dirname "$path")" && pwd -P)"
  printf '%s/%s\n' "$dir" "$(basename "$path")"
}

find_ghostty_gtk_lib() {
  local candidate
  while IFS= read -r candidate; do
    if [[ -f "$candidate" ]]; then
      absolute_path "$candidate"
      return 0
    fi
  done < <(ghostty_gtk_lib_candidates)
  return 1
}

build_ghostty_gtk_lib() {
  if [[ ! -d "$GHOSTTY_DIR" ]]; then
    echo "Missing Ghostty source tree: $GHOSTTY_DIR" >&2
    echo "Run: git submodule update --init vendor/ghostty" >&2
    exit 1
  fi
  command -v zig >/dev/null || {
    echo "zig is required to build the embedded Ghostty GTK library" >&2
    exit 1
  }

  (
    cd "$GHOSTTY_DIR"
    zig build \
      -Doptimize=ReleaseFast \
      -Dapp-runtime=gtk \
      -Demit-exe=false \
      -Demit-gtk-lib=true \
      -Demit-docs=false \
      -Demit-terminfo=false \
      -Demit-termcap=false \
      -Demit-themes=false \
      -Demit-webdata=false \
      -Demit-helpgen=false \
      -Demit-lib-vt=false \
      -Dsentry=false \
      -Di18n=false \
      -Dsimd=false \
      -Dcpu=baseline \
      -fno-sys=gtk4-layer-shell \
      --summary failures
  ) >&2
}

verify_ghostty_gtk_lib_symbols() {
  local lib_path="$1"
  local symbol

  command -v nm >/dev/null || {
    echo "nm is required to verify the embedded Ghostty GTK library ABI" >&2
    exit 1
  }

  for symbol in \
    ghostty_gtk_surface_new_with_working_directory_and_command \
    ghostty_gtk_surface_exit_code \
    ghostty_gtk_surface_child_pid \
    ghostty_gtk_surface_perform_action \
    ghostty_gtk_surface_restore_scrollback
  do
    if ! nm -D "$lib_path" | grep -Eq "(^|[[:space:]])${symbol}$"; then
      echo "Ghostty GTK embedding library is missing ${symbol}: $lib_path" >&2
      return 1
    fi
  done
}

verified_lib_path() {
  local lib_path=""
  local built="$BUILT_BEFORE_VERIFY"

  if ! lib_path="$(find_ghostty_gtk_lib)"; then
    if [[ "$built" == "1" ]]; then
      echo "Ghostty GTK probe did not emit an embedding library" >&2
      return 1
    fi
    build_ghostty_gtk_lib
    built=1
    if ! lib_path="$(find_ghostty_gtk_lib)"; then
      echo "Ghostty GTK probe did not emit an embedding library" >&2
      return 1
    fi
  fi

  if ! verify_ghostty_gtk_lib_symbols "$lib_path"; then
    if [[ "$ENSURE" == "1" && "$built" == "0" ]]; then
      echo "Existing Ghostty GTK embedding library failed ABI validation; rebuilding." >&2
      build_ghostty_gtk_lib
      lib_path="$(find_ghostty_gtk_lib)" || {
        echo "Ghostty GTK probe did not emit an embedding library" >&2
        return 1
      }
      verify_ghostty_gtk_lib_symbols "$lib_path"
    else
      return 1
    fi
  fi

  printf '%s\n' "$lib_path"
}

if [[ "$ENSURE" != "1" ]]; then
  build_ghostty_gtk_lib
  BUILT_BEFORE_VERIFY=1
fi

LIB_PATH="$(verified_lib_path)"
if [[ "$PRINT_PATH" == "1" ]]; then
  printf '%s\n' "$LIB_PATH"
fi
