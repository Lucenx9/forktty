#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="${FORKTTY_SMOKE_BIN:-$ROOT_DIR/target/debug/forktty}"

if [[ "${FORKTTY_SMOKE_BUILD:-1}" != "0" || ! -x "$BIN" ]]; then
  cargo build -p forktty-ui-gtk --no-default-features --features gtk-ghostty
fi

if [[ -z "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]]; then
  if command -v xvfb-run >/dev/null 2>&1; then
    exec env FORKTTY_SMOKE_BUILD=0 xvfb-run -a "$0" "$@"
  fi
  echo "gtk-ghostty smoke: no DISPLAY/WAYLAND_DISPLAY and xvfb-run not found" >&2
  exit 77
fi

if [[ -z "${FORKTTY_SMOKE_DBUS_ISOLATED:-}" ]]; then
  if command -v dbus-run-session >/dev/null 2>&1; then
    exec env FORKTTY_SMOKE_BUILD=0 FORKTTY_SMOKE_DBUS_ISOLATED=1 dbus-run-session -- "$0" "$@"
  fi
  echo "gtk-ghostty smoke: dbus-run-session not found" >&2
  exit 77
fi

if ! command -v python3 >/dev/null 2>&1; then
  echo "gtk-ghostty smoke: python3 not found (required for JSON parsing)" >&2
  exit 77
fi

if [[ -z "${XDG_RUNTIME_DIR:-}" || "${XDG_RUNTIME_DIR:0:1}" != "/" || ! -d "$XDG_RUNTIME_DIR" ]]; then
  echo "gtk-ghostty smoke: XDG_RUNTIME_DIR must be an existing absolute directory" >&2
  exit 77
fi

TMP_DIR="$(mktemp -d "$XDG_RUNTIME_DIR/forktty-smoke.XXXXXX")"
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

mkdir -p "$XDG_CONFIG_HOME/forktty" "$XDG_STATE_HOME/forktty"
cat >"$XDG_CONFIG_HOME/forktty/config.toml" <<'EOF'
[notifications]
desktop = false
sound = false

[telemetry]
anonymous_ping = false
EOF
printf '{"welcomed_version":"smoke"}\n' >"$XDG_STATE_HOME/forktty/welcome-seen.json"

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

read_screen_json() {
  "$BIN" --socket "$FORKTTY_SOCKET_PATH" read-screen --surface-id "$surface_id" --json
}

snapshot_field() {
  local field="$1"
  read_screen_json | python3 -c 'import json,sys; print(json.load(sys.stdin)[sys.argv[1]])' "$field"
}

sent=0
for _ in {1..40}; do
  if "$BIN" --socket "$FORKTTY_SOCKET_PATH" send-text --surface-id "$surface_id" $'echo forktty-smoke-ok\r' >/dev/null 2>&1; then
    sent=1
    break
  fi
  sleep 0.25
done
if [[ "$sent" != "1" ]]; then
  echo "gtk-ghostty smoke: terminal did not become writable" >&2
  exit 1
fi

for _ in {1..40}; do
  if "$BIN" --socket "$FORKTTY_SOCKET_PATH" read-screen --surface-id "$surface_id" | grep -q "forktty-smoke-ok"; then
    break
  fi
  sleep 0.25
done

"$BIN" --socket "$FORKTTY_SOCKET_PATH" read-screen --surface-id "$surface_id" | grep -q "forktty-smoke-ok"

base_cols="$(snapshot_field cols)"
base_rows="$(snapshot_field rows)"
gapplication action dev.forktty.forktty zoom-in >/dev/null
gapplication action dev.forktty.forktty zoom-in >/dev/null
zoom_changed=0
for _ in {1..40}; do
  cols="$(snapshot_field cols)"
  rows="$(snapshot_field rows)"
  if (( cols > 0 && rows > 0 && (cols < base_cols || rows < base_rows) )); then
    zoom_changed=1
    break
  fi
  sleep 0.25
done
if [[ "$zoom_changed" != "1" ]]; then
  echo "gtk-ghostty smoke: terminal dimensions did not change after zoom-in" >&2
  exit 1
fi
"$BIN" --socket "$FORKTTY_SOCKET_PATH" read-screen --surface-id "$surface_id" | grep -q "forktty-smoke-ok"

gapplication action dev.forktty.forktty zoom-out >/dev/null
gapplication action dev.forktty.forktty zoom-reset >/dev/null
zoom_reset=0
for _ in {1..40}; do
  cols="$(snapshot_field cols)"
  rows="$(snapshot_field rows)"
  if (( cols == base_cols && rows == base_rows )); then
    zoom_reset=1
    break
  fi
  sleep 0.25
done
if [[ "$zoom_reset" != "1" ]]; then
  echo "gtk-ghostty smoke: terminal dimensions did not return after zoom reset" >&2
  exit 1
fi

"$BIN" --socket "$FORKTTY_SOCKET_PATH" split-surface --surface-id "$surface_id" --axis vertical >/dev/null
"$BIN" --socket "$FORKTTY_SOCKET_PATH" surfaces --json |
  python3 -c 'import json,sys; assert len(json.load(sys.stdin)) >= 2'

echo "gtk-ghostty smoke: ok"
