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

[appearance]
persistent_scrollback_lines = 200

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

workspace_id="$("$BIN" --socket "$FORKTTY_SOCKET_PATH" surfaces --json |
  python3 -c 'import json,sys; print(json.load(sys.stdin)[0]["workspace_id"])')"
surface_id="$("$BIN" --socket "$FORKTTY_SOCKET_PATH" surfaces --json |
  python3 -c 'import json,sys; print(json.load(sys.stdin)[0]["id"])')"

read_surface_json() {
  local id="$1"
  "$BIN" --socket "$FORKTTY_SOCKET_PATH" read-screen --surface-id "$id" --json
}

read_screen_json() {
  read_surface_json "$surface_id"
}

snapshot_field() {
  local field="$1"
  read_screen_json | python3 -c 'import json,sys; print(json.load(sys.stdin)[sys.argv[1]])' "$field"
}

surface_count() {
  "$BIN" --socket "$FORKTTY_SOCKET_PATH" surfaces --json |
    python3 -c 'import json,sys; print(len(json.load(sys.stdin)))'
}

focused_surface_id() {
  "$BIN" --socket "$FORKTTY_SOCKET_PATH" top --json |
    python3 -c 'import json,sys; data=json.load(sys.stdin); workspaces=data.get("workspaces", []); active=next((workspace for workspace in workspaces if workspace.get("active")), workspaces[0] if workspaces else {}); print(active.get("focused_surface_id", ""))'
}

wait_surface_pid() {
  local id="$1"
  local surfaces_json
  for _ in {1..120}; do
    if surfaces_json="$("$BIN" --socket "$FORKTTY_SOCKET_PATH" surfaces --json)" &&
      printf '%s' "$surfaces_json" |
        python3 -c 'import json,sys; id=sys.argv[1]; surfaces=json.load(sys.stdin); item=next((surface for surface in surfaces if surface.get("id") == id), None); pid=None if item is None else item.get("pid"); sys.exit(0 if isinstance(pid, int) and pid > 0 else 1)' "$id"
    then
      return 0
    fi
    sleep 0.25
  done
  return 1
}

send_text_wait() {
  local id="$1"
  local text="$2"
  local label="$3"
  for _ in {1..40}; do
    if "$BIN" --socket "$FORKTTY_SOCKET_PATH" send-text --surface-id "$id" "$text" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.25
  done
  echo "gtk-ghostty smoke: $label did not become writable" >&2
  exit 1
}

surface_contains() {
  local id="$1"
  local needle="$2"
  "$BIN" --socket "$FORKTTY_SOCKET_PATH" read-screen --surface-id "$id" 2>/dev/null | grep -q "$needle"
}

wait_surface_contains() {
  local id="$1"
  local needle="$2"
  local label="$3"
  for _ in {1..40}; do
    if surface_contains "$id" "$needle"; then
      return 0
    fi
    sleep 0.25
  done
  echo "gtk-ghostty smoke: $label did not appear on surface $id" >&2
  exit 1
}

capture_tail_contains() {
  local id="$1"
  local needle="$2"
  "$BIN" --socket "$FORKTTY_SOCKET_PATH" capture-tail --surface-id "$id" --lines 80 2>/dev/null | grep -q "$needle"
}

wait_capture_tail_contains() {
  local id="$1"
  local needle="$2"
  local label="$3"
  for _ in {1..40}; do
    if capture_tail_contains "$id" "$needle"; then
      return 0
    fi
    sleep 0.25
  done
  echo "gtk-ghostty smoke: $label did not appear in capture-tail for surface $id" >&2
  "$BIN" --socket "$FORKTTY_SOCKET_PATH" capture-tail --surface-id "$id" --lines 80 >&2 || true
  exit 1
}

wait_surface_not_writable() {
  local id="$1"
  local label="$2"
  for _ in {1..40}; do
    if ! "$BIN" --socket "$FORKTTY_SOCKET_PATH" send-text --surface-id "$id" $'echo forktty-smoke-should-not-write\r' >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.25
  done
  echo "gtk-ghostty smoke: $label stayed writable after child exit" >&2
  exit 1
}

wait_surface_status() {
  local workspace="$1"
  local id="$2"
  local expected="$3"
  local key="surface:${id}:status"
  for _ in {1..40}; do
    if "$BIN" --socket "$FORKTTY_SOCKET_PATH" list-status --workspace-id "$workspace" --json |
      python3 -c 'import json,sys; items=json.load(sys.stdin); key=sys.argv[1]; expected=sys.argv[2]; item=next((entry for entry in items if entry.get("key") == key), None); sys.exit(0 if item and item.get("value") == expected else 1)' "$key" "$expected"
    then
      return 0
    fi
    sleep 0.25
  done
  echo "gtk-ghostty smoke: surface $id did not report status $expected" >&2
  "$BIN" --socket "$FORKTTY_SOCKET_PATH" list-status --workspace-id "$workspace" --json >&2 || true
  exit 1
}

send_text_wait "$surface_id" $'echo forktty-smoke-ok\r' "initial terminal"
wait_surface_contains "$surface_id" "forktty-smoke-ok" "initial terminal readback"
"$BIN" --socket "$FORKTTY_SOCKET_PATH" capture-tail --surface-id "$surface_id" --lines 5 |
  grep -q "forktty-smoke-ok"
if ! wait_surface_pid "$surface_id"; then
  echo "gtk-ghostty smoke: surface $surface_id did not expose a child pid" >&2
  cat "$TMP_DIR/forktty.stderr" >&2 || true
  exit 1
fi

scrollback_restore_marker="forktty-smoke-scrollback-restore-before-restart"
send_text_wait "$surface_id" $'echo forktty-smoke-scrollback-restore-before-restart\r' "scrollback restore source terminal"
wait_surface_contains "$surface_id" "$scrollback_restore_marker" "scrollback restore source marker"
"$BIN" --socket "$FORKTTY_SOCKET_PATH" focus-surface "$surface_id" >/dev/null
gapplication action dev.forktty.forktty restart-pane >/dev/null
send_text_wait "$surface_id" $'echo forktty-smoke-after-restart\r' "restarted terminal"
wait_surface_contains "$surface_id" "forktty-smoke-after-restart" "restarted terminal readback"
wait_capture_tail_contains "$surface_id" "$scrollback_restore_marker" "restored scrollback marker"

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

action_split_count="$(surface_count)"
gapplication action dev.forktty.forktty split-horizontal >/dev/null
action_surface_id=""
for _ in {1..40}; do
  candidate="$(focused_surface_id)"
  count="$(surface_count)"
  if (( count > action_split_count )) && [[ "$candidate" != "$surface_id" ]]; then
    action_surface_id="$candidate"
    break
  fi
  sleep 0.25
done
if [[ -z "$action_surface_id" ]]; then
  echo "gtk-ghostty smoke: split-horizontal action did not create and focus a new surface" >&2
  exit 1
fi
send_text_wait "$action_surface_id" $'echo forktty-smoke-action-split-ok\r' "action split terminal"
wait_surface_contains "$action_surface_id" "forktty-smoke-action-split-ok" "action split terminal readback"

new_surface_id="$("$BIN" --socket "$FORKTTY_SOCKET_PATH" split-surface --surface-id "$surface_id" --axis vertical --json |
  python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')"
"$BIN" --socket "$FORKTTY_SOCKET_PATH" surfaces --json |
  python3 -c 'import json,sys; ids={item["id"] for item in json.load(sys.stdin)}; assert sys.argv[1] in ids and sys.argv[2] in ids' "$surface_id" "$new_surface_id"

send_text_wait "$new_surface_id" $'echo forktty-smoke-split-ok\r' "split terminal"
wait_surface_contains "$new_surface_id" "forktty-smoke-split-ok" "split terminal readback"

"$BIN" --socket "$FORKTTY_SOCKET_PATH" focus-surface "$surface_id" >/dev/null
gapplication action dev.forktty.forktty focus-next-pane >/dev/null
next_focus_id=""
for _ in {1..40}; do
  candidate="$(focused_surface_id)"
  if [[ "$candidate" != "$surface_id" ]]; then
    next_focus_id="$candidate"
    break
  fi
  sleep 0.25
done
if [[ -z "$next_focus_id" ]]; then
  echo "gtk-ghostty smoke: focus-next-pane action did not move focus" >&2
  exit 1
fi

gapplication action dev.forktty.forktty focus-previous-pane >/dev/null
previous_focus_ok=0
for _ in {1..40}; do
  if [[ "$(focused_surface_id)" == "$surface_id" ]]; then
    previous_focus_ok=1
    break
  fi
  sleep 0.25
done
if [[ "$previous_focus_ok" != "1" ]]; then
  echo "gtk-ghostty smoke: focus-previous-pane action did not restore focus" >&2
  exit 1
fi

"$BIN" --socket "$FORKTTY_SOCKET_PATH" notify --workspace-id "$workspace_id" --title "Smoke Notification" --kind prompt --body "forktty-smoke-notification" >/dev/null
"$BIN" --socket "$FORKTTY_SOCKET_PATH" notifications --json |
  python3 -c 'import json,sys; items=json.load(sys.stdin); assert any(item.get("workspace_id") == sys.argv[1] and item.get("body") == "forktty-smoke-notification" for item in items)' "$workspace_id"
"$BIN" --socket "$FORKTTY_SOCKET_PATH" clear-notifications >/dev/null
"$BIN" --socket "$FORKTTY_SOCKET_PATH" notifications --json |
  python3 -c 'import json,sys; assert json.load(sys.stdin) == []'

send_text_wait "$new_surface_id" $'exit\r' "split terminal exit"
wait_surface_not_writable "$new_surface_id" "split terminal"
wait_surface_status "$workspace_id" "$new_surface_id" "Closed"

if grep -Eiq 'failed to spawn embedded Ghostty GTK pane|falling back:' "$TMP_DIR/forktty.stderr"; then
  echo "gtk-ghostty smoke: embedded Ghostty pane fell back to the classic renderer" >&2
  cat "$TMP_DIR/forktty.stderr" >&2 || true
  exit 1
fi

echo "gtk-ghostty smoke: ok"
