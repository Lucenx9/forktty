#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST="${PIUPARTS_DIST:-trixie}"
DEB_PATH="${1:-}"

if [[ -z "$DEB_PATH" ]]; then
  DEB_PATH="$(find "$ROOT_DIR/target/packaging/deb" -maxdepth 1 -name 'forktty_*.deb' -printf '%T@ %p\n' 2>/dev/null |
    sort -nr |
    head -1 |
    cut -d' ' -f2-)"
fi

if [[ -z "$DEB_PATH" || ! -f "$DEB_PATH" ]]; then
  echo "No ForkTTY .deb found. Run: bash scripts/build-deb.sh" >&2
  exit 1
fi

command -v piuparts >/dev/null || {
  echo "piuparts is required to run the Debian install/purge check" >&2
  exit 1
}

command -v debootstrap >/dev/null || {
  echo "debootstrap is required by piuparts when building a fresh chroot" >&2
  exit 1
}

# Arch/AUR installs piuparts' Python modules under Debian's dist-packages path,
# which Arch Python does not include by default.
if [[ -d /usr/lib/python3/dist-packages/piupartslib ]]; then
  export PYTHONPATH="/usr/lib/python3/dist-packages${PYTHONPATH:+:$PYTHONPATH}"
fi

exec piuparts --defaults debian -d "$DIST" "$DEB_PATH"
