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
