# Privacy Notice

## Data Collection

ForkTTY sends a default-on anonymous daily usage ping from the GTK app so the maintainer can estimate whether the app is being used. ForkTTY does not upload crash reports, terminal contents, project data, command data, socket payloads, usernames, hostnames, install identifiers, or event analytics.

## Network Activity

With `telemetry.anonymous_ping = true`, GTK startup sends at most one HTTPS POST
per UTC day to:

```text
https://forktty-site.vercel.app/api/telemetry/ping
```

The JSON body is:

```json
{
  "schema": 1,
  "kind": "daily_ping",
  "app": "forktty",
  "version": "0.2.0-alpha.12",
  "date": "2026-06-13"
}
```

Set this in `~/.config/forktty/config.toml` to disable the ping:

```toml
[telemetry]
anonymous_ping = false
```

On the first launch a one-time welcome dialog shows this toggle (default-on)
with a link to this notice before any ping is sent: the first ping is deferred
until you dismiss that dialog, so you always see and can change the setting
before data leaves the machine.

The ping is sent only by GTK startup. CLI invocations, agent hooks, socket
clients, and the local MCP bridge do not send telemetry pings.

Optional features can make user-directed network requests:

- Update checks, when `updates.auto_check = true`, fetch GitHub Releases at
  most once per day to discover newer ForkTTY versions. AppImage updates only
  download release assets after user confirmation.
- Browser panes built with the `browser` feature load URLs opened by the
  user or by local socket automation.
- Linked PR lookup, when enabled in settings, shells out to the local
  `gh` CLI for the current git branch.

The automation interface is a local Unix domain socket, normally at:

```text
$XDG_RUNTIME_DIR/forktty.sock
```

The socket is intended for same-machine, same-user automation and is removed on exit when possible.

## Locally Stored Files

ForkTTY stores local application data only:

| File | Location | Purpose |
| ---- | -------- | ------- |
| Configuration | `~/.config/forktty/config.toml` | Shell, scrollback, terminal bell, worktree, notification, sidebar, renderer compatibility, and window settings |
| Native session data | `~/.local/share/forktty/session-v2.json` | Workspace layout and metadata needed for GTK/Ghostty session restore |
| Legacy session import | `~/.local/share/forktty/session.json` | Imported if present; native saves do not overwrite it |
| Quarantined sessions | `~/.local/share/forktty/session-v2.json.bad-*` | Invalid/corrupt session files kept for debugging |
| Browser profiles | `~/.local/share/forktty/browser_profiles/profiles.json` | Browser profile names and IDs for the optional browser feature |
| Browser profile data | `~/.local/share/forktty/browser_profiles/<id>/` | Optional WebKit data/cache/cookies plus ForkTTY history/bookmark stores for that profile |
| Update check stamp | `~/.local/state/forktty/update-check.json` | Last update-check attempt time and optional GitHub rate-limit deadline |
| Telemetry ping stamp | `~/.local/state/forktty/telemetry-ping.json` | Last UTC date when GTK startup attempted the anonymous usage ping |
| Logs | `~/.local/share/forktty/logs/` | Local structured logs for debugging |
| IPC socket | `$XDG_RUNTIME_DIR/forktty.sock` | Ephemeral local socket for automation |

These files may contain local paths, usernames embedded in paths, workspace names, branch names, notification text derived from terminal output, and browser-profile URLs/cookies/history/bookmarks when the optional browser feature is used. They remain on your machine unless you share them.

## Desktop Notifications

When desktop notifications are enabled, ForkTTY sends notification title/body text to the local desktop notification service through `notify-rust` / XDG D-Bus. Notification content can include terminal output or agent prompt text.

If `notification_command` is configured, ForkTTY runs the configured local executable and provides the notification payload through:

- `FORKTTY_NOTIFICATION_TITLE`
- `FORKTTY_NOTIFICATION_BODY`

Use a custom notification command only if you trust that executable with local terminal notification content.

## Third-Party Components

ForkTTY uses GTK4/libadwaita for the window shell and a vendored embedded
Ghostty GTK library for terminal widgets. Packaged builds ship the required
Ghostty libraries; source builds build or reuse them from the pinned
`vendor/ghostty` and `vendor/libghostty-rs` trees. Source builds with the
optional browser feature also use WebKitGTK6 to render pages the user opens.

The ForkTTY socket CLI talks only to the local Unix socket.

## How to Delete Local Data

```bash
rm -rf ~/.config/forktty
rm -rf ~/.local/share/forktty
```

The runtime socket is ephemeral and normally lives under `$XDG_RUNTIME_DIR`.

## EU/GDPR Note

ForkTTY minimizes the default anonymous usage ping to audience measurement only
and provides an opt-out setting. You control local data stored on your machine
and any local scripts you configure.

## Contact

For privacy questions, open an issue at https://github.com/Lucenx9/forktty/issues or use [GitHub private vulnerability reporting](https://github.com/Lucenx9/forktty/security/advisories/new).
