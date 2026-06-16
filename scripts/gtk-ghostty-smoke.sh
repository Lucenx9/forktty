#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="${FORKTTY_SMOKE_BIN:-$ROOT_DIR/target/debug/forktty}"

if [[ "${FORKTTY_SMOKE_BUILD:-1}" != "0" || ! -x "$BIN" ]]; then
  cargo build -p forktty-ui-gtk --no-default-features --features gtk-ghostty
fi

if [[ -z "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]]; then
  if command -v xvfb-run >/dev/null 2>&1; then
    exec xvfb-run -a "$0" "$@"
  fi
  echo "gtk-ghostty smoke: no DISPLAY/WAYLAND_DISPLAY and xvfb-run not found" >&2
  exit 77
fi

TMP_DIR="$(mktemp -d)"
APP_PID=""

cleanup() {
  if [[ -n "$APP_PID" ]] && kill -0 "$APP_PID" 2>/dev/null; then
    kill "$APP_PID" 2>/dev/null || true
    wait "$APP_PID" 2>/dev/null || true
  fi
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT

mkdir -p "$TMP_DIR"/{config,data,state,cache,run}
chmod 700 "$TMP_DIR/run"

export XDG_CONFIG_HOME="$TMP_DIR/config"
export XDG_DATA_HOME="$TMP_DIR/data"
export XDG_STATE_HOME="$TMP_DIR/state"
export XDG_CACHE_HOME="$TMP_DIR/cache"
export FORKTTY_SOCKET_PATH="$TMP_DIR/run/forktty.sock"

"$BIN" >"$TMP_DIR/forktty.stdout" 2>"$TMP_DIR/forktty.stderr" &
APP_PID="$!"

for _ in {1..80}; do
  if "$BIN" --socket "$FORKTTY_SOCKET_PATH" ping >/dev/null 2>&1; then
    break
  fi
  if ! kill -0 "$APP_PID" 2>/dev/null; then
    echo "gtk-ghostty smoke: app exited before socket became ready" >&2
    cat "$TMP_DIR/forktty.stderr" >&2 || true
    exit 1
  fi
  sleep 0.25
done

"$BIN" --socket "$FORKTTY_SOCKET_PATH" ping >/dev/null
"$BIN" --socket "$FORKTTY_SOCKET_PATH" list >/dev/null
"$BIN" --socket "$FORKTTY_SOCKET_PATH" surfaces --json |
  python3 -c 'import json,sys; assert len(json.load(sys.stdin)) >= 1'

surface_id="$("$BIN" --socket "$FORKTTY_SOCKET_PATH" read-screen --json |
  python3 -c 'import json,sys; print(json.load(sys.stdin)["surface_id"])')"

for _ in {1..40}; do
  if "$BIN" --socket "$FORKTTY_SOCKET_PATH" send-text --surface-id "$surface_id" $'echo forktty-smoke-ok\r' >/dev/null 2>&1; then
    break
  fi
  sleep 0.25
done

"$BIN" --socket "$FORKTTY_SOCKET_PATH" send-text --surface-id "$surface_id" $'echo forktty-smoke-ok\r' >/dev/null

for _ in {1..40}; do
  if "$BIN" --socket "$FORKTTY_SOCKET_PATH" read-screen --surface-id "$surface_id" | grep -q "forktty-smoke-ok"; then
    break
  fi
  sleep 0.25
done

"$BIN" --socket "$FORKTTY_SOCKET_PATH" read-screen --surface-id "$surface_id" | grep -q "forktty-smoke-ok"
"$BIN" --socket "$FORKTTY_SOCKET_PATH" split-surface --surface-id "$surface_id" --axis vertical >/dev/null
"$BIN" --socket "$FORKTTY_SOCKET_PATH" surfaces --json |
  python3 -c 'import json,sys; assert len(json.load(sys.stdin)) >= 2'

echo "gtk-ghostty smoke: ok"
