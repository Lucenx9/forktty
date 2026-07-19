#!/usr/bin/env bash

set -euo pipefail

umask 022

if [[ "$#" -ne 1 ]]; then
  echo "usage: $0 <package-root>" >&2
  exit 2
fi

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PACKAGE_ROOT="$1"
DOC_DIR="$PACKAGE_ROOT/usr/share/doc/forktty"
COPYRIGHT_FILE="$DOC_DIR/copyright"

mkdir -p "$DOC_DIR"
{
  cat "$ROOT_DIR/packaging/debian/copyright.header"
  printf '\n\n'
  printf 'License: AGPL-3.0-only\n\n'
  cat "$ROOT_DIR/LICENSE"
  printf '\n\n'
  printf 'License: MIT (Ghostty)\n\n'
  cat "$ROOT_DIR/vendor/ghostty/LICENSE"
  printf '\n\n'
  printf 'License: MIT (libghostty-rs)\n\n'
  cat "$ROOT_DIR/vendor/libghostty-rs/LICENSE"
  printf '\n\n'
  printf 'License: GPL-3.0-or-later (Ghostty shell integration)\n\n'
  cat "$ROOT_DIR/packaging/licenses/GPL-3.0-or-later.txt"
  printf '\n\n'
  printf 'License: MIT (bash-preexec 0.6.0)\n\n'
  cat "$ROOT_DIR/packaging/licenses/bash-preexec-0.6.0-MIT.txt"
  printf '\n\n'
  printf 'License: MIT (gtk4-layer-shell 1.1.0)\n\n'
  cat "$ROOT_DIR/packaging/licenses/gtk4-layer-shell-1.1.0-MIT.txt"
} > "$COPYRIGHT_FILE"

install -Dm644 "$ROOT_DIR/THIRD_PARTY_NOTICES.md" "$DOC_DIR/THIRD_PARTY_NOTICES.md"
test -s "$COPYRIGHT_FILE"
test -s "$DOC_DIR/THIRD_PARTY_NOTICES.md"
