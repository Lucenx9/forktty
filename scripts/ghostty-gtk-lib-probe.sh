#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/../vendor/ghostty"

zig build \
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

lib_path=""
for candidate in \
  zig-out/lib/ghostty-gtk-embed.so \
  zig-out/lib/libghostty-gtk-embed.so
do
  if [[ -f "$candidate" ]]; then
    lib_path="$candidate"
    break
  fi
done

if [[ -z "$lib_path" ]]; then
  echo "Ghostty GTK probe did not emit an embedding library" >&2
  exit 1
fi

if ! nm -D "$lib_path" | grep -Eq '(^|[[:space:]])ghostty_gtk_surface_exit_code$'; then
  echo "Ghostty GTK embedding library is missing ghostty_gtk_surface_exit_code" >&2
  exit 1
fi
