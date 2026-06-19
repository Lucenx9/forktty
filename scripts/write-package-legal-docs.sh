#!/usr/bin/env bash

set -euo pipefail

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
} > "$COPYRIGHT_FILE"

install -Dm644 "$ROOT_DIR/THIRD_PARTY_NOTICES.md" "$DOC_DIR/THIRD_PARTY_NOTICES.md"
test -s "$COPYRIGHT_FILE"
test -s "$DOC_DIR/THIRD_PARTY_NOTICES.md"
