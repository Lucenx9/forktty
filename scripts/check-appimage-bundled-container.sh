#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
  cat <<'USAGE'
Usage: scripts/check-appimage-bundled-container.sh [APPIMAGE_OR_APPDIR]

Extracts the packaged AppImage when needed, masks host GTK/libadwaita inside a
bubblewrap sandbox, verifies that default AppRun auto mode falls back to the
bundled stack, and checks ForkTTY's hidden appimage-child-exec helper. The helper
must sanitize LD_LIBRARY_PATH, execute a host target, and preserve
TERM=xterm-ghostty plus Ghostty's Bash integration.
When the path is omitted, exactly one target/packaging/appimage/*.AppImage must
exist.
USAGE
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi
if (( $# > 1 )); then
  usage >&2
  exit 2
fi

input="${1:-}"
if [[ -z "$input" ]]; then
  shopt -s nullglob
  appimages=("$ROOT_DIR"/target/packaging/appimage/*.AppImage)
  shopt -u nullglob
  case "${#appimages[@]}" in
    0)
      echo "appimage bundled container: no AppImage artifacts found under target/packaging/appimage" >&2
      exit 2
      ;;
    1)
      input="${appimages[0]}"
      ;;
    *)
      echo "appimage bundled container: multiple AppImage artifacts found; pass one explicitly" >&2
      printf '  %s\n' "${appimages[@]}" >&2
      exit 2
      ;;
  esac
fi

TMP_DIR="$(mktemp -d)"
cleanup() {
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT

if [[ -d "$input" && -x "$input/AppRun" ]]; then
  APP_ROOT="$(readlink -f "$input")"
elif [[ -f "$input" && -x "$input" ]]; then
  appimage="$(readlink -f "$input")"
  (
    cd "$TMP_DIR"
    "$appimage" --appimage-extract >/dev/null
  )
  APP_ROOT="$TMP_DIR/squashfs-root"
else
  echo "appimage bundled container: expected an executable AppImage or AppDir: $input" >&2
  exit 2
fi

APP_RUN="$APP_ROOT/AppRun"
FORKTTY_BIN="$APP_ROOT/usr/bin/forktty"
BASH_INTEGRATION="$APP_ROOT/usr/share/ghostty/shell-integration/bash/ghostty.bash"
TERMINFO_DIR="$APP_ROOT/usr/share/terminfo"

for path in "$APP_RUN" "$FORKTTY_BIN" "$BASH_INTEGRATION"; do
  if [[ ! -f "$path" ]]; then
    echo "appimage bundled container: required packaged file missing: $path" >&2
    exit 1
  fi
done
if [[ ! -x "$APP_RUN" || ! -x "$FORKTTY_BIN" ]]; then
  echo "appimage bundled container: AppRun and forktty must be executable" >&2
  exit 1
fi
if ! compgen -G "$APP_ROOT/usr/lib/bundled/libgtk-4.so*" >/dev/null; then
  echo "appimage bundled container: bundled GTK library missing" >&2
  exit 1
fi
if ! compgen -G "$APP_ROOT/usr/lib/bundled/libadwaita-1.so*" >/dev/null; then
  echo "appimage bundled container: bundled libadwaita library missing" >&2
  exit 1
fi
if [[ ! -f "$APP_ROOT/usr/lib/ghostty-gtk-embed.so" ]]; then
  echo "appimage bundled container: embedded Ghostty library missing" >&2
  exit 1
fi
if [[ ! -f "$TERMINFO_DIR/x/xterm-ghostty" && ! -f "$TERMINFO_DIR/g/ghostty" ]]; then
  echo "appimage bundled container: Ghostty terminfo missing" >&2
  exit 1
fi

sh -n "$APP_RUN"
if ! command -v readelf >/dev/null 2>&1; then
  echo "appimage bundled container: readelf is required" >&2
  exit 77
fi
if ! readelf -d "$FORKTTY_BIN" | grep -Fq 'libgtk-4.so.1'; then
  echo "appimage bundled container: packaged forktty is not linked to GTK4" >&2
  exit 1
fi
for needle in 'FORKTTY_APPIMAGE_GTK_RUNTIME' 'usr/lib/bundled'; do
  if ! grep -Fq "$needle" "$APP_RUN"; then
    echo "appimage bundled container: AppRun is missing $needle" >&2
    exit 1
  fi
done

if ! command -v bwrap >/dev/null 2>&1; then
  echo "appimage bundled container: static package contract passed; bubblewrap unavailable" >&2
  exit 77
fi
if [[ ! -x /usr/bin/env || ! -x /bin/bash ]]; then
  echo "appimage bundled container: /usr/bin/env and /bin/bash are required" >&2
  exit 77
fi

host_gtk_paths=()
if command -v ldconfig >/dev/null 2>&1; then
  mapfile -t host_gtk_paths < <(
    ldconfig -p 2>/dev/null |
      awk '$1 == "libgtk-4.so.1" || $1 == "libadwaita-1.so.0" { print $NF }' |
      sort -u
  )
elif [[ -x /sbin/ldconfig ]]; then
  mapfile -t host_gtk_paths < <(
    /sbin/ldconfig -p 2>/dev/null |
      awk '$1 == "libgtk-4.so.1" || $1 == "libadwaita-1.so.0" { print $NF }' |
      sort -u
  )
fi

unusable_gtk="$TMP_DIR/unusable-host-gtk"
: >"$unusable_gtk"
# The loader check needs mount masking, not network isolation. Some CI kernels
# allow bubblewrap itself but deny configuring loopback in a new net namespace.
bwrap_args=(
  --ro-bind / /
  --proc /proc
  --dev /dev
  --unshare-pid
  --die-with-parent
)
masked_count=0
for path in "${host_gtk_paths[@]}"; do
  if [[ -e "$path" ]]; then
    bwrap_args+=(--ro-bind "$unusable_gtk" "$path")
    masked_count=$((masked_count + 1))
  fi
done

host_probe_stdout="$TMP_DIR/host-probe.stdout"
host_probe_stderr="$TMP_DIR/host-probe.stderr"
if bwrap "${bwrap_args[@]}" \
  /usr/bin/env -i \
  HOME=/tmp \
  PATH=/usr/bin:/bin \
  FORKTTY_APPIMAGE_GTK_RUNTIME=host \
  APPDIR="$APP_ROOT" \
  "$APP_RUN" --version \
  >"$host_probe_stdout" 2>"$host_probe_stderr"
then
  echo "appimage bundled container: host GTK remained usable inside the sandbox" >&2
  exit 1
fi

auto_probe_stdout="$TMP_DIR/auto-probe.stdout"
auto_probe_stderr="$TMP_DIR/auto-probe.stderr"
if ! bwrap "${bwrap_args[@]}" \
  /usr/bin/env -i \
  HOME=/tmp \
  PATH=/usr/bin:/bin \
  LD_LIBRARY_PATH=/unusable-host-loader \
  APPDIR="$APP_ROOT" \
  "$APP_RUN" --version \
  >"$auto_probe_stdout" 2>"$auto_probe_stderr"
then
  echo "appimage bundled container: AppRun auto mode did not fall back to bundled GTK" >&2
  grep -v '^$' "$auto_probe_stderr" >&2 || true
  exit 1
fi

helper_stdout="$TMP_DIR/helper.stdout"
helper_stderr="$TMP_DIR/helper.stderr"
if ! bwrap "${bwrap_args[@]}" \
  /usr/bin/env -i \
  HOME=/tmp \
  PATH=/usr/bin:/bin \
  FORKTTY_APPIMAGE_GTK_RUNTIME=bundled \
  LD_LIBRARY_PATH=/unusable-host-loader \
  APPDIR="$APP_ROOT" \
  "$APP_RUN" appimage-child-exec \
  --unset APPDIR \
  --unset APPIMAGE \
  --env LD_LIBRARY_PATH=/usr/lib \
  --env TERM=xterm-ghostty \
  -- /usr/bin/env \
  >"$helper_stdout" 2>"$helper_stderr"
then
  echo "appimage bundled container: AppRun could not start appimage-child-exec" >&2
  grep -v '^$' "$helper_stderr" >&2 || true
  exit 1
fi

grep -Fxq 'LD_LIBRARY_PATH=/usr/lib' "$helper_stdout"
grep -Fxq 'TERM=xterm-ghostty' "$helper_stdout"
if grep -Eq '^(APPDIR|APPIMAGE)=' "$helper_stdout"; then
  echo "appimage bundled container: helper leaked AppImage runtime variables" >&2
  exit 1
fi
if grep -Fq "$APP_ROOT/usr/lib" "$helper_stdout"; then
  echo "appimage bundled container: helper leaked AppImage LD_LIBRARY_PATH entries" >&2
  exit 1
fi

shell_stdout="$TMP_DIR/shell.stdout"
shell_stderr="$TMP_DIR/shell.stderr"
# Expand TERM inside the child Bash, not in this outer smoke script.
# shellcheck disable=SC2016
if ! bwrap "${bwrap_args[@]}" \
  /usr/bin/env -i \
  HOME=/tmp \
  PATH=/usr/bin:/bin \
  FORKTTY_APPIMAGE_GTK_RUNTIME=bundled \
  APPDIR="$APP_ROOT" \
  "$APP_RUN" appimage-child-exec \
  --unset APPDIR \
  --unset APPIMAGE \
  --env LD_LIBRARY_PATH=/usr/lib \
  --env TERM=xterm-ghostty \
  --env TERMINFO="$TERMINFO_DIR" \
  --env GHOSTTY_RESOURCES_DIR="$APP_ROOT/usr/share/ghostty" \
  --env GHOSTTY_SHELL_FEATURES=cursor:blink,title \
  --env ENV="$BASH_INTEGRATION" \
  --env 'GHOSTTY_BASH_INJECT=1 --noprofile --norc' \
  -- /bin/bash --posix --noprofile --norc -i -c \
  'printf "forktty-bundled-term=%s\n" "$TERM"; if declare -F __ghostty_precmd >/dev/null; then printf "forktty-bundled-bash-integration=ok\n"; else exit 1; fi' \
  >"$shell_stdout" 2>"$shell_stderr"
then
  echo "appimage bundled container: packaged Bash integration check failed" >&2
  grep -v '^$' "$shell_stderr" >&2 || true
  exit 1
fi

grep -Fxq 'forktty-bundled-term=xterm-ghostty' "$shell_stdout"
grep -Fxq 'forktty-bundled-bash-integration=ok' "$shell_stdout"

echo "appimage bundled container: ok (bubblewrap; masked $masked_count host GTK libraries)"
